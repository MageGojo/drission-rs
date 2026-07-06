# drission 编程技能(SKILL)· AI 必读

> 🤖 **给 AI 编程助手的指令**:如果你是 AI,要**基于 `drission` 库写代码**,请**严格遵循本文件**的接口签名、
> feature / 构建规则与约定,以产出**一次就能编译、运行正确**的代码。本文件覆盖从最基础用法到点选验证码全流程。
> 若你所在环境不支持 "skill" 机制,可忽略此条强制要求,但仍**强烈建议**据此使用本库(它就是本库接口的权威速查)。
>
> 配套:`docs/API映射.md`(DP→drission 全表)、`examples/`(63 个端到端可运行示例)、`docs/设计.md`。

---

## AI 优先入口:先用 `drs` CLI / MCP,再写 Rust API

如果任务是“让 AI 操作网页 / 观察页面 / 抓接口 / 截图 / 提取文本”,优先使用同仓库 CLI:

```bash
cargo install --path crates/drission-cli --bin drs
drs serve --backend cdp --headless
drs --json open https://example.com
drs --json ax --json
drs --json listen wait --count 3 --timeout-ms 5000
```

需要接入 MCP 客户端时用:

```bash
drs mcp --backend cdp --headless
```

`drs` 输出统一为 `{ "ok": true, "data": ... }` / `{ "ok": false, "error": ... }`,适合 Agent 直接解析。只有在需要定制复杂流程、并发池、协议逆向、验证码闭环或把能力嵌入业务服务时,再按下面规则编写 Rust 程序。

---

## 0. 最重要:选后端 + feature/构建规则(这步错了,代码一定跑不起来)

`drission` 是**双后端**库,统一接口(`Browser`/`Tab`/`Element`/`Page`…)按 feature 切换实现:

| 你要做的事 | 选哪个 feature | Cargo.toml | 运行 / 构建命令 |
|---|---|---|---|
| 驱动 **Google Chrome**(CDP,默认,最贴近 DrissionPage) | `cdp`(**默认开**) | `drission = "0.3"` | `cargo run`(无需 feature) |
| **Camoufox/Firefox 反检测**内核 + 过盾/吐环境/池/Session | `camoufox` | `features=["camoufox"]` | `cargo run --no-default-features --features camoufox` |
| **字符验证码 OCR / 点选验证码**(检测+识别) | `ocr` | `features=["ocr"]` | `cargo run --features ocr`(cdp 默认在) |
| **图片滑块**缺口(极验/顶象) | `slider`(自带 camoufox) | `features=["slider"]` | `cargo run --no-default-features --features slider` |
| **Session 浏览器 TLS/JA3 指纹** | `impersonate`(自带 camoufox) | `features=["impersonate"]` | `cargo run --example session_tls --features impersonate` |
| **纯算签名**(内嵌 QuickJS,无浏览器) | `signer` | `features=["signer"]` | `cargo build --features signer` |

### ⚠️ 铁律(AI 必须遵守,否则编译失败)
1. **默认后端是 CDP**(`default=["cdp"]`)。要用 **Camoufox 后端**,构建命令**必须带 `--no-default-features`**:
   `--no-default-features --features camoufox`。
   原因:`cdp` 与 `camoufox` 同时开启时,统一接口(`Browser`/`Tab`/`Page`)按「**cdp 优先**」解析,
   会与 Camoufox 代码期望的类型冲突(编译报错或跑成 cdp 后端)。**单后端**才正确。
2. **`ocr` / `signer` 是后端无关的**,可直接叠加在默认 cdp 上(`--features ocr`,不要 `--no-default-features`)。
   点选验证码(`ClickWord`/`Det`/`human_click`)就用 `--features ocr`(或 `--features cdp,ocr`)。
3. **`slider` 会自动带入 camoufox**;页面/浏览器后端示例用 `--no-default-features --features slider`。
   **`impersonate` 也会带入 camoufox**,但它主要服务 `SessionPage`;跑指定 `session_tls` 示例可直接
   `--features impersonate`。批量检查或运行 Camoufox 页面示例时,仍按单后端规则带 `--no-default-features`。
4. 一切从 `use drission::prelude::*;` 开始。不要去 `use` 内部模块路径。
5. 所有 IO 方法都是 **`async`**,要 `.await?`;返回值用 `drission::Result<T>`。运行时用 `#[tokio::main]`。
6. 文件路径(上传/截图保存)用**绝对路径**。

### 标准 main 模板
```rust
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    // ... 见下方各节
    Ok(())
}
```

---

## 1. 起步:三种门面,优先用 `Page`(大道至简)

```rust
use drission::prelude::*;

// 【推荐】Page 门面:开浏览器 + 驱动当前标签合一,像写 Python。
let page = Page::headless().await?;            // 无头;有头用 Page::new();自定义 Page::with(opts)
page.get("https://example.com").await?;        // 导航(返回 bool,失败为 false 不报错)
println!("{}", page.title().await?);
page.click("@id:more").await?;                 // 「找+点」一步
page.input("#kw", "drission").await?;          // 「找+输」一步
let ok = page.exists("#result").await?;        // 立即判断存在(不等待)
page.quit().await?;
// page 经 Deref 拥有全部 Tab 方法(ele/run_js/listen/actions/dump_env…)。
```

更底层(多标签 / 并发 / 接管)用 `Browser` + `Tab`:
```rust
// CDP(默认):
let browser = ChromiumBrowser::launch(true).await?;          // true=无头
let tab = browser.new_tab(Some("https://example.com")).await?;
// Camoufox(--no-default-features --features camoufox):
let browser = Browser::launch_default().await?;              // 有头 + 反检测默认开
let tab = browser.latest_tab().await?;
```

---

## 2. 元素定位(语法与 DrissionPage 完全一致)、点击、输入

```rust
// 定位语法(字符串):
tab.ele("#kw").await?;            // id(CSS)
tab.ele(".btn").await?;           // class
tab.ele("@id:kw").await?;         // 属性 id 含 kw     @name=foo 精确
tab.ele("tag:li").await?;         // 标签名(亦 t:li)
tab.ele("text:登录").await?;      // 文本含「登录」(亦直接写「登录」)
tab.ele("css:div.box>a").await?;  // CSS
tab.ele("xpath://a[@href]").await?; // XPath(实时,走浏览器原生)
let items = tab.eles("tag:li").await?;   // 多个 -> Vec<Element>

// 元素操作:
let el = tab.ele("#kw").await?;
el.input("hello").await?;         // 输入(快)
el.input_human("hello").await?;   // 逐字符拟人输入(触发站点 keydown/keyup)
el.click().await?;                // 可信点击(isTrusted=true)
el.clear().await?;
let t = el.text().await?;
let href = el.attr("href").await?;
let vis = el.is_displayed().await?;
// 相对定位:el.parent()/children()/next()/prev()/siblings() ...
// Shadow DOM:let root = el.shadow_root().await?; root.ele(".x").await?;
// iframe:let f = tab.get_frame("#ifr").await?; f.ele("#inner").await?;
```

> 找不到元素统一返回 `Err(Error::ElementNotFound)`,用 `?` 传播或 `match`/`if let Ok` 处理。

---

## 3. 等待 + 过 Cloudflare 盾(两后端都支持)

```rust
use std::time::Duration;

tab.wait().ele_displayed("#result", None).await?;     // 等元素出现(None=默认超时)
tab.wait().doc_loaded(None).await?;                   // 等文档加载完
el.wait().clickable(None).await?;                     // 等元素可点

// 过 CF 盾(cdp 默认已自动反检测;交互式 Turnstile 用这个):
let passed = tab.pass_cloudflare(Duration::from_secs(30)).await?;
// 或 tab.pass_cloudflare_default().await?;
```

---

## 4. 网络监听 / 拦截 / WebSocket / 控制台

```rust
// 监听 XHR/fetch 抓响应体(务必先 start 再导航/触发):
tab.listen().start(&["api/data"]).await?;
tab.get("https://site/page").await?;
let packet = tab.listen().wait().await?;              // 抓到一个目标请求
println!("{}", packet.response.body);
// 多个:tab.listen().wait_count(3, None).await? -> Vec<DataPacket>
tab.listen().stop().await?;

// 请求拦截改写:
tab.intercept_start(&["/api/"]).await?;
let req = tab.intercept_next().await?;
req.fulfill(200, vec![], r#"{"hello":"fake"}"#.into()).await?;  // 伪造响应
// 或 req.resume().await? / req.abort("blockedbyclient").await?
tab.intercept_stop().await?;

// WebSocket 帧监听(建连前 start):tab.websocket().start().await?; ... wait().await?
// 控制台:tab.console().start().await?; let d = tab.console().wait(None).await?;
```

---

## 5. 验证码(本库护城河)

### 5.1 字符验证码 OCR(`--features ocr`,两后端通用)
```rust
// 便捷:定位 <img> -> 取图 -> 识别(首次自动下载 ~54MB 模型到缓存)
let code = tab.ocr_image("xpath://form//img").await?;   // 例 "P38W"
el.input(&code).await?;

// 或后端无关、直接喂字节:
let ocr = Ocr::new().await?;                  // async
let text = ocr.recognize(&png_bytes)?;        // sync,返回 String
```

### 5.2 图片滑块缺口(`--no-default-features --features slider`,camoufox)
```rust
tab.apply_pointer_stealth().await?;           // 导航前注入(滑块/行为风控反检测)
tab.get("https://demos.geetest.com/slide-float.html").await?;
let r = tab.solve_geetest_slide().await?;     // 极验:弹出->匹配->拟人拖动->判定->换图重试
println!("通过={} 尝试={}次", r.passed, r.attempts);

// 顶象:let gap = tab.dingxiang_slide_gap(4).await?;  // .displace / .confidence
// 通用任意厂商:用 SliderConfig 描述图源/把手/判定,tab.solve_slider(&cfg).await?
```

### 5.3 点选 / 文字点选验证码(`--features ocr`;cdp 或 camoufox 均可)
**完整、正确的现代流程**(检测 → 取干净源图 → 识别+全局指派 → 映射页面坐标 → 拟人轨迹点击):
```rust
use drission::prelude::*;

// 1) 构造求解器(首次下载检测 common_det.onnx + 识别 common.onnx 到缓存)
let cw = ClickWord::new().await?;

// 2) 读验证码图元素的「显示 rect + 自然尺寸 + src」,并直接拉**干净源图字节**(避开工具栏/跨域)
let view = tab.image_view(".yidun_bg-img,.geetest_item_img,img").await?;
let img = fetch_image(&view.src).await?;          // 自由函数:服务端直拉,不被跨域 taint

// 3) 提示要点的字(明文,通常在 DOM 里,如易盾 data.front / .yidun_tips__text)
let targets: Vec<String> = ["元", "验", "体"].iter().map(|s| s.to_string()).collect();

// 4) 求解:Vec<ClickHit>{ target, bbox, point:(u32,u32), affinity:f32, template }
let hits = cw.solve(&img, &targets)?;             // 注意 targets 是 &[String]

// 5) 置信度门控 + 把「图内像素点」映射到「页面坐标」(map_u32 按显示/自然尺寸缩放)
let pts: Vec<(f64, f64)> = hits.iter()
    .filter(|h| h.affinity >= 0.30)               // 置信过低就别乱点(可换图重试)
    .map(|h| view.map_u32(h.point))
    .collect();

// 6) 拟人轨迹依次点击(贝塞尔曲线 + minimum-jerk 变速,击穿行为风控)
tab.human_click(&pts).await?;
```
要点:
- `cw.solve(image, &[String])` 内部做**检测 + 逐框 OCR + 字形模板融合 + 全局最优指派**,返回逐目标命中与**置信度**。
- 若验证码图右上角有刷新/语音工具栏,改用 `cw.solve_excluding(&img, &targets, &[exclude_rect])` 排除该区域。
- `ClickHit.affinity` 用来**做阈值**:低于阈值就点刷新换图重试,而不是乱点。
- `tab.human_click(&[(f64,f64)])` 与 `tab.image_view`/`fetch_image` 都是**后端无关、始终可用**(无需任何 feature)。

---

## 6. Session(HTTP)双模 + TLS/JA3 指纹

```rust
// 纯 HTTP(不开浏览器,省内存):需 camoufox / impersonate 构建
let mut sess = SessionPage::new_default()?;
sess.get("https://example.com").await?;
println!("{} {}", sess.status(), sess.title()?);
for a in sess.s_eles("tag:a")? { println!("{:?}", a.attr("href")?); }

// 浏览器过盾 -> 把 cookie 交接给 Session 继续纯 HTTP 抓:
sess.load_cookies_from_tab(&tab).await?;          // camoufox tab
// CDP:sess.load_cookies_from_cdp_tab(&chromium_tab).await?;

// 【--features impersonate】给 Session 套真实浏览器 TLS 指纹(过现代 WAF):
let mut sess = SessionPage::new(
    SessionOptions::new().profile(BrowserProfile::Chrome)   // None/Chrome/Firefox/Safari/Edge
)?;
sess.get("https://target").await?;   // JA3/JA4/HTTP2 指纹=Chrome,不再被 TLS 指纹拦
```

---

## 7. 高并发池

```rust
// CDP(默认):
let pool = ChromiumPool::launch(ChromiumPoolOptions::new().size(2).tabs_per_worker(2)).await?;
let results = pool.map(urls, |tab, url| async move {
    tab.get(&url).await?;
    tab.ele_text("h1").await
}).await;
pool.shutdown().await?;
// Camoufox 版:BrowserPool(+ ProxyPool/FingerprintPool/Checkpoint 断点续抓)。
```

---

## 8. 吐环境(补环境)/ 纯算签名

```rust
// 采集真实指纹 + 签名 sink,导出可 node 运行的补环境工程:
let dump = tab.dump_env().start().await?.collect().await?;
dump.export_project("./site-env", EnvScope::All)?;
// 配 --features signer 可把 env.js 编进单二进制做纯算签名(见 examples/env_signer)。
```

---

## 9. 必记 gotchas(AI 易错点)

1. **导航后首读竞态**:`get()` 刚返回时 `title()/url()` 可能读到旧上下文。**先 `wait().ele_displayed(..)`
   或先查一个元素**,再读 `title`/`html`。
2. **`ClickWord::solve` 的 targets 是 `&[String]`**,不是 `&[&str]`。要 `.map(|s| s.to_string()).collect::<Vec<String>>()`。
3. **`StaticElement`(s_ele 系)不是 `Send`**,别跨 `tokio::spawn` 传;在单任务里顺序用。
4. **Camoufox 平台限制**:无法最小化/全屏/移动主窗口;Juggler 无修饰组合键(Ctrl+A 等)——
   **CDP 后端可以**(`tab.key_combo`/`ele.shortcut`)。shadow DOM 内不支持 `xpath:`。
5. **截图格式仅 Png/Jpeg**(无 webp)。
6. **`get()` 返回 `bool`**(失败 false,不是 Err);要重试/超时用 `tab.get_with(url, &GetOptions::new()...)`。
7. **`apply_pointer_stealth` 是 camoufox 专属**(滑块/点选**导航前**调,补 Juggler 合成事件的空 `pointerType`);
   **CDP 后端没有也不需要**(`Input.dispatchMouseEvent` 原生带 `pointerType`)。`tab.wait().secs(f64)` 也是 camoufox 专属,
   CDP 等待用 `tokio::time::sleep` 或 `wait().ele_displayed/doc_loaded`(两后端都有)。
8. **构建命令**:Camoufox 页面示例和 slider 示例一律 `--no-default-features`;`session_tls` 这种纯 Session
   impersonate 示例可直接 `--features impersonate`(见第 0 节铁律)。

---

## 10. 完整实战:易盾点选验证码端到端(可直接改用)

`Cargo.toml`: `drission = { version = "0.3", features = ["ocr"] }`(默认 cdp + ocr)
运行:`cargo run --features ocr`

```rust
use drission::prelude::*;
use std::time::Duration;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let browser = ChromiumBrowser::launch(false).await?;   // 有头更易过行为风控
    let tab = browser.new_tab(Some("https://dun.163.com/trial/picture-click")).await?;
    // 注:CDP 后端经 Input.dispatchMouseEvent 原生带 pointerType,无需 apply_pointer_stealth
    // (该方法是 camoufox 专属;见 gotcha #7)。

    let cw = ClickWord::new().await?;

    for _try in 0..3 {                                      // 最多换图重试 3 次
        // 触发挑战 + 等验证码图出现(按目标站点选择器调整)
        tab.wait().ele_displayed(".yidun_bgimg", Some(Duration::from_secs(10))).await?;

        // 取干净源图 + 提示字(易盾提示在 .yidun_tips__point;此处示意写死)
        let view = tab.image_view(".yidun_bg-img,.yidun_bgimg img").await?;
        let img = fetch_image(&view.src).await?;
        let targets: Vec<String> = ["元","验","体"].iter().map(|s| s.to_string()).collect();

        let hits = cw.solve(&img, &targets)?;
        let min_conf = hits.iter().map(|h| h.affinity).fold(1.0_f32, f32::min);
        if min_conf < 0.30 {                               // 置信不足 -> 换图
            tab.ele(".yidun_refresh").await?.click().await?;
            continue;
        }
        let pts: Vec<(f64,f64)> = hits.iter().map(|h| view.map_u32(h.point)).collect();
        tab.human_click(&pts).await?;                      // 拟人轨迹点击

        tokio::time::sleep(Duration::from_millis(1500)).await;   // 等结果(cdp 的 wait() 无 secs)
        if tab.html().await?.contains("验证成功") { break; }
        tab.ele(".yidun_refresh").await?.click().await?;   // 没过 -> 换图重试
    }
    browser.quit().await?;
    Ok(())
}
```
> 真过盾 = 干净源图(高识别率)+ 置信度门控 + 全局指派 + **拟人轨迹**(`human_click`)三者合一;
> 仍受出口 IP 风险分影响,换 IP/冷却后更稳。完整版见 `examples/yidun_click.rs`。
