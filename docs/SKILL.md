---
name: drission-rs
description: Rust browser automation skill for the drission crate and drs CLI/MCP. Use when building, debugging, or generating code with drission-rs for Chromium/CDP or Camoufox browser automation, DrissionPage-style selectors, captcha OCR/slider/click-word solving, Cloudflare/anti-detect workflows, network listen/intercept/WebSocket/console capture, CDP extras such as PDF/MHTML/HAR/expose_function/device emulation, recorder/codegen/accessibility snapshots, browser fingerprinting, Session/WebPage/TLS impersonation, high-concurrency pools, environment dumping/signing, or active web reverse engineering.
---

# drission-rs 编程技能

本文件给 AI 编程助手使用。基于 `drission` 写代码时,先按这里选后端、feature、命令和 API。更细的背景只在需要时读对应文档,不要凭记忆猜内部路径或方法签名。

配套入口:

- `docs/API映射.md`: 从 DrissionPage 迁移或查完整 API 对照。
- `docs/CLI.md`: 使用 `drs` CLI / MCP 让 AI 直接操作浏览器。
- `docs/标配补齐.md`: CDP PDF/MHTML/HAR/expose/media/network/device/storage/wait 补齐。
- `docs/录制与无障碍.md`: `tab.recorder()` 录制生成 Rust + `ax_tree/ax_snapshot`。
- `docs/指纹.md`: CDP 每浏览器不同指纹 `CdpFingerprint*`。
- `docs/逆向增强.md`: XHR 断点、脚本 dump、Hook、反调试、WASM、重放。
- `docs/OCR模型热替换.md`: 自训 OCR、字符集、真样本模板库、过盾即采样。
- `docs/并发池.md`: `BrowserPool` / `ChromiumPool`、代理、指纹、断点续抓。
- `docs/长监听与滑动.md`: 长监听 SPA/短视频 feed + 滑动/按键驱动。
- `docs/TLS指纹.md`: Session TLS/JA3/JA4 + HTTP2 指纹伪装。
- `docs/Chrome自动下载.md`: CDP 自动定位 / 下载 Chrome for Testing。
- `examples/README.md`: 60+ 可运行示例和对应 feature 命令。

## 1. 先选执行入口

如果任务只是让 AI 观察页面、点选、输入、截图、抓接口、读无障碍树,优先用 `drs` CLI/MCP:

```bash
cargo install drission-cli --bin drs
drs serve --backend cdp --headless
drs --json open https://example.com
drs --json ax --json
drs --json listen start /api/ --xhr-only
drs --json listen wait --count 3 --timeout-ms 5000
drs --json screenshot --out /tmp/page.png --full
```

MCP:

```bash
drs mcp --backend cdp --headless
```

稳定 MCP 工具名包括 `browser_open`、`browser_tabs`、`browser_ax`、`browser_html`、`browser_text`、`browser_eval`、`browser_click`、`browser_type`、`browser_wait`、`browser_screenshot`、`network_listen_start`、`network_listen_wait`、`network_listen_stop`、`browser_pass_cf`。

只有需要嵌入业务、复杂并发、验证码闭环、协议逆向、补环境/签名、HAR 回放或自定义流程时,再写 Rust。

## 2. Feature / 后端铁律

`drission` 是双后端库。默认后端是 Chromium / CDP,也就是 Google Chrome / Edge / Brave / Electron。

| 任务 | feature / 命令 | 关键点 |
|---|---|---|
| 默认 Chrome/CDP 自动化 | `drission = "0.3"` 或 `--features cdp` | 默认开启;找不到系统 Chrome 会自动下载 Chrome for Testing。 |
| Camoufox/Firefox 反检测后端 | `--no-default-features --features camoufox` | Camoufox 页面示例必须关掉默认 `cdp`。 |
| 字符 OCR / 点选验证码 | `--features ocr` 或 `--features cdp,ocr` | 默认 CDP 可直接叠加;`ClickWord` 后端无关。 |
| 图片滑块缺口 | `--no-default-features --features slider` | `slider` 自动带入 Camoufox。 |
| Session TLS/JA3/JA4 指纹 | `--features impersonate` | `impersonate` 自动带入 Camoufox;纯 Session 示例可不关默认 feature。 |
| 纯算签名 QuickJS | `--features signer` | 不开浏览器也可跑签名。 |

铁律:

1. `prelude` 里的统一名按 `cdp` 优先解析。默认构建下 `Browser/Tab/Page/Element/BrowserOptions` 是 CDP 类型。
2. 同时开 `cdp,camoufox` 时,统一名仍是 CDP。要用另一端显式名: `CamoufoxBrowser` / `CamoufoxPage` / `CamoufoxOptions` 或 `ChromiumBrowser` / `ChromiumOptions`。
3. Camoufox 示例一律 `--no-default-features --features camoufox`。否则很容易类型冲突或跑成 CDP。
4. `ocr` / `signer` 是后端无关能力,通常直接叠加默认 CDP。
5. 所有 IO 都是 async: `#[tokio::main]`, 返回 `drission::Result<()>`, 调用 `.await?`。
6. 常规业务代码从 `use drission::prelude::*;` 开始。只有 `CdpFingerprint*`、`ensure_chrome/download_chrome_for` 等 CDP 专属项从 `drission::cdp::*` 显式导入。

标准模板:

```rust
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    Ok(())
}
```

## 3. 启动与接管

默认 CDP 一行起步:

```rust
use drission::prelude::*;

let page = Page::headless().await?;          // 默认 CDP: ChromiumPage
page.get("https://example.com").await?;
println!("{}", page.title().await?);
page.quit().await?;
```

CDP 底层控制:

```rust
let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(true)).await?;
let tab = browser.new_tab(Some("https://example.com")).await?;
let attached = ChromiumBrowser::connect("http://127.0.0.1:9222").await?; // 接管调试端口
browser.quit().await?;
```

Chrome 自动定位 / 下载:

```rust
let found = ChromiumBrowser::find_chrome()?;        // 只定位,不下载
let exe = ChromiumBrowser::download_chrome().await?; // 定位失败则下载 CfT
let win = drission::cdp::download_chrome_for("win64", "Stable").await?;
```

Camoufox 单后端:

```rust
// cargo run --no-default-features --features camoufox
use drission::prelude::*;

let page = Page::new().await?;                  // 此时 Page 是 CamoufoxPage
page.get("https://example.com").await?;
page.quit().await?;
```

多标签 / per-context 代理:

```rust
let tab = browser.new_tab(Some("https://example.com")).await?;
let iso = browser.new_tab_with(
    &ChromiumContextOverride::new()
        .proxy("http://127.0.0.1:8080")
        .timezone("America/New_York")
).await?;
```

## 4. 页面、定位、元素

定位语法对齐 DrissionPage:

```rust
let el = tab.ele("#kw").await?;                    // id/CSS
let el = tab.ele(".btn").await?;                   // class/CSS
let el = tab.ele("@id:kw").await?;                 // 属性包含
let el = tab.ele("@name=wd").await?;               // 属性精确
let el = tab.ele("tag:li").await?;                 // 标签
let el = tab.ele("text:登录").await?;              // 文本包含
let el = tab.ele("css:div.box > a").await?;        // CSS
let el = tab.ele("xpath://a[@href]").await?;       // 浏览器原生 XPath
let items = tab.eles("tag:li").await?;
```

元素操作:

```rust
tab.click("#submit").await?;
tab.input("#kw", "drission").await?;
let exists = tab.exists("#result").await?;
let text = tab.ele("#result").await?.text().await?;
let href = tab.ele("tag:a").await?.attr("href").await?;

let input = tab.ele("#kw").await?;
input.clear().await?;
input.input_human("hello").await?;
input.input_keys(&[KeyInput::text("abc"), KeyInput::key(Keys::ENTER)]).await?;
input.shortcut(&[Keys::CONTROL, "a"]).await?;      // CDP 支持真实组合键

tab.ele("#agree").await?.set_checked(true).await?;
tab.ele("#lang").await?.select_value("rs").await?;
tab.ele("input[type=file]").await?.set_files(&["/abs/file.txt"]).await?;
```

iframe / Shadow / 静态解析:

```rust
let frame = tab.get_frame("#ifr").await?;
frame.ele("#inner").await?.click().await?;

let root = tab.ele("#host").await?.shadow_root().await?;
root.ele(".inside").await?;

let s = tab.s_ele("xpath://li[2]").await?;         // 离线解析 HTML 快照
println!("{}", s.text()?);
```

注意: `StaticElement` 不是 `Send`,不要跨 `tokio::spawn` 传。

## 5. 等待、截图、下载、storage

```rust
use std::time::Duration;

tab.wait().doc_loaded(None).await?;
tab.wait().ele_loaded("#app", Some(Duration::from_secs(10))).await?;
tab.wait().ele_displayed("#result", None).await?;
tab.wait().title_contains("Dashboard", None).await?;
tab.wait().url_contains("/home", None).await?;

let png = tab.screenshot(&ShotOpts::new().full_page(true)).await?;
tab.get_screenshot("/abs/page.png", true).await?;
tab.ele("#logo").await?.get_screenshot("/abs/logo.png").await?;

tab.set_download_path("/abs/downloads").await?;
let dl = tab.downloads();
dl.start().await?;
// 触发下载...
let missions = dl.missions().await;
dl.stop().await?;

let state = tab.storage_state().await?;
tab.apply_storage_state(&state).await?;
tab.set().local_storage_set("token", "abc").await?;
let token = tab.set().local_storage_get("token").await?;
```

CDP 没有 `tab.wait().secs(f64)`。需要睡眠时用 `tokio::time::sleep(Duration::from_millis(...)).await`。Camoufox 后端有 `wait().secs`。

## 6. 网络监听、拦截、控制台、WebSocket

默认 CDP 写法:

```rust
use std::time::Duration;

let listen = tab.listen();
listen.start(&["/api/"]).await?;                  // 先 start 再导航/触发
tab.get("https://site/page").await?;
if let Some(packet) = listen.wait(Some(Duration::from_secs(10))).await? {
    println!("{} {} {}", packet.method, packet.url, packet.response.body);
}
let packets = listen.wait_count(3, Some(Duration::from_secs(5))).await?;
listen.stop().await?;

let intercept = tab.intercept();
intercept.start(&["/api/data"]).await?;
if let Some(req) = intercept.next(Some(Duration::from_secs(5))).await? {
    req.fulfill(
        200,
        vec![("content-type".into(), "application/json".into())],
        r#"{"ok":true}"#
    ).await?;
}
intercept.stop().await?;

let console = tab.console();
console.start().await?;
if let Some(msg) = console.wait(Some(Duration::from_secs(5))).await? {
    println!("{:?}", msg.body());
}
console.stop().await?;

let ws = tab.websocket();
ws.start().await?;
let frames = ws.wait_count(5, Some(Duration::from_secs(10))).await?;
ws.stop().await?;
```

Camoufox 的 `listen().wait()` / `intercept().next()` 等签名略有不同。写跨后端库代码前查 `docs/API映射.md`;写默认 CDP 脚本就按上面写。

## 7. CDP 标配补齐

这些主要是 CDP 专属能力,默认后端可直接用:

```rust
tab.set_content("<h1 id='x'>hi</h1>").await?;
tab.save_pdf("/abs/page.pdf", &PdfOptions::default()).await?;  // PDF 建议无头
tab.save_mhtml("/abs/page.mhtml").await?;

tab.set().emulate_dark(true).await?;
tab.set().emulate_media(Some("print"), &[("prefers-reduced-motion", "reduce")]).await?;
tab.set().offline(false).await?;
tab.set().network_conditions(&NetworkConditions::slow_3g()).await?;
tab.set().cpu_throttling(4.0).await?;
tab.set().grant_permissions("https://example.com", &["geolocation", "clipboard-read"]).await?;
tab.set().device(&Device::iphone_13()).await?;
tab.set().clear_device().await?;

let rec = tab.har_record().await?;                // 导航前 start
tab.get("https://example.com").await?;
let har = rec.stop().await?;
har.save("/abs/traffic.har").await?;

let player = tab.route_from_har("/abs/traffic.har", &HarReplayOptions::default()).await?;
tab.get("https://example.com").await?;
player.stop().await?;

// expose_function 的用户工程需直接依赖 serde_json
let _guard = tab.expose_function("addRust", |args| {
    Ok(serde_json::Value::from(args.len() as u64))
}).await?;
```

弹窗 / 新标签:

```rust
let waiter = tab.wait();
let (popup, _) = tokio::join!(waiter.new_tab(Some(Duration::from_secs(8))), async {
    let _ = tab.ele("#open").await?.click().await;
    Ok::<(), drission::Error>(())
});
if let Some(popup) = popup? {
    popup.wait().doc_loaded(None).await?;
}
```

`wait().new_tab()` 返回 `Result<Option<Tab>>`,超时是 `Ok(None)`。

## 8. 录制生成代码与无障碍快照

录制是开发期能力,会启用 `Runtime` 域,不要放进强反检测生产链路:

```rust
let rec = tab.recorder();
rec.start().await?;                 // 导航前调用
tab.get("https://example.com").await?;
// 人工或程序操作页面...
let script = rec.stop().await?;
println!("{}", script.to_rust());   // 完整可运行 Rust
println!("{}", script.to_json());   // 中间表示
```

无障碍树适合 AI 页面理解和抗改版断言:

```rust
let snap = tab.ax_snapshot().await?;      // DOM 派生,CDP/Camoufox 都可用
println!("{}", snap.to_outline());
let buttons = snap.find_by_role("button");

let native = tab.ax_tree().await?;        // CDP 原生,更准
let links = tab.ax_find("link").await?;
```

## 9. 反检测、Cloudflare、指纹

Cloudflare:

```rust
let passed = tab.pass_cloudflare(Duration::from_secs(30)).await?;
// 或 tab.pass_cloudflare_default().await?;
```

CDP 默认有头 + stealth;无头会自动做 UA / WebGL / 屏幕等补齐。过 Turnstile 仍优先有头,出口 IP/地区/时区/语言要自洽。

读取实时指纹:

```rust
use drission::prelude::*;

let fp = tab.fingerprint_snapshot().await?;
println!("ua={} canvas#={} webgl={}", fp.ua, fp.canvas_hash, fp.webgl_renderer);
```

设置 CDP 每浏览器不同指纹时显式导入 `drission::cdp`:

```rust
use drission::cdp::{CdpFingerprintPool, ChromiumBrowser, ChromiumOptions};
use drission::prelude::FingerprintProbe;

let pool_fp = CdpFingerprintPool::generate(3);      // 同 OS 变体,Turnstile 更友好
for fp in pool_fp.profiles() {
    let opts = fp.apply_to_options(ChromiumOptions::new().headless(true));
    let browser = ChromiumBrowser::launch(opts).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;
    let snap = tab.fingerprint_snapshot().await?;
    browser.quit().await?;
}
```

跨 OS persona (`CdpFingerprintPool::personas`) 需要配套对应地区代理,否则 UA/时区/WebGL/IP 自相矛盾。

## 10. 验证码

字符 OCR:

```rust
let code = tab.ocr_image("xpath://form//img").await?; // --features ocr

let ocr = Ocr::new().await?;
let text = ocr.recognize(&png_bytes)?;
```

OCR / 检测模型热替换:

```bash
DRISSION_DET_MODEL=yidun_det.onnx \
DRISSION_OCR_MODEL=yidun_ocr.onnx \
DRISSION_OCR_CHARSET=yidun_charset.json \
cargo run --example yidun_click --features cdp,ocr
```

真样本模板库:

```bash
DRISSION_GLYPH_SAMPLES=/abs/yidun_samples/bank \
cargo run --example yidun_click_stable --features cdp,ocr
```

点选验证码正确流程:

```rust
use drission::prelude::*;

let mut cw = ClickWord::new().await?;
let view = tab.image_view(".yidun_bg-img,.geetest_item_img,img").await?;
let img = fetch_image(&view.src).await?;       // 干净源图,避免 canvas taint / 工具栏污染
let targets: Vec<String> = ["元", "验", "体"].iter().map(|s| s.to_string()).collect();

let hits = cw.solve(&img, &targets)?;
let pts: Vec<(f64, f64)> = hits.iter()
    .filter(|h| h.affinity >= 0.30)
    .map(|h| view.map_u32(h.point))
    .collect();
tab.human_click(&pts).await?;

// 如果服务端已确认本轮 result:true,可把已验证字图白捡进样本库
let added = cw.harvest_verified(&img, &hits, std::path::Path::new("/abs/bank"))?;
cw.reload_sample_bank(std::path::Path::new("/abs/bank"));
```

要点:

- `ClickWord::solve(image, &[String])`,不是 `&[&str]`。
- 低置信度换图重试,不要乱点。
- 工具栏/语音/刷新区会误检时用 `solve_excluding(&img, &targets, &[BBox{...}])`。
- `tab.image_view` / `fetch_image` / `tab.human_click` 后端无关。

图片滑块:

```rust
// cargo run --no-default-features --features slider
tab.apply_pointer_stealth().await?;       // Camoufox 导航前调用
tab.get("https://demos.geetest.com/slide-float.html").await?;
let r = tab.solve_geetest_slide().await?;
let gap = tab.dingxiang_slide_gap(4).await?;

let cfg = SliderConfig::geetest_v4().max_attempts(3);
let r = tab.solve_slider(&cfg).await?;
```

## 11. Session / WebPage / TLS 指纹

Session 和 WebPage 在 Camoufox feature 下可用;`impersonate` 给纯 HTTP 套浏览器 TLS/JA3/JA4/HTTP2 指纹:

```rust
let mut sess = SessionPage::new_default()?;
sess.get("https://example.com").await?;
println!("{} {}", sess.status(), sess.title()?);
for a in sess.s_eles("tag:a")? {
    println!("{:?}", a.attr("href")?);
}

sess.load_cookies_from_cdp_tab(&tab).await?;  // CDP 浏览器 cookie -> Session

let mut sess = SessionPage::new(
    SessionOptions::new().profile(BrowserProfile::Chrome)
)?;
sess.get("https://target").await?;
```

WebPage 双模:

```rust
let mut page = WebPage::new_driver(CamoufoxOptions::new().headless(true)).await?;
page.get("https://site/login").await?;
page.change_mode(PageMode::Session).await?;
page.get("https://site/list").await?;
let rows = page.s_eles("css:.item").await?;
page.quit().await?;
```

抓包转重放:

```rust
let packet = tab.listen().wait(Some(Duration::from_secs(10))).await?.unwrap();
let ok = sess.replay(&packet)
    .set_query("t", "123")
    .set_header("x-sign", "...")
    .send()
    .await?;
```

## 12. 高并发池与导出

CDP 池:

```rust
let pool = ChromiumPool::launch(
    ChromiumPoolOptions::new()
        .size(2)
        .tabs_per_worker(2)
        .base_options(ChromiumOptions::new().headless(true))
).await?;

let results = pool.map(urls, |url, tab| async move {
    tab.get(&url).await?;
    Ok::<String, drission::Error>(tab.ele_text("h1").await?.unwrap_or_default())
}).await;

pool.shutdown().await?;
```

断点续抓:

```rust
let ckpt = Checkpoint::load("/abs/ckpt.jsonl").await?;
let out = pool.map_resumable(items, |i| format!("key-{i}"), &ckpt, |i, tab| async move {
    tab.get(&format!("https://example.com/{i}")).await?;
    Ok::<(), drission::Error>(())
}).await;
```

Camoufox 池:

```rust
let pool = BrowserPool::launch(
    PoolOptions::new()
        .size(2)
        .tabs_per_worker(2)
        .base_options(BrowserOptions::new().headless(true))
        .fingerprints(FingerprintPool::presets())
        .retry(RetryPolicy::new(2))
).await?;
```

CSV / JSON / 表格:

```rust
write_csv("/abs/out.csv", &rows).await?;
write_json("/abs/out.json", &records).await?;
println!("{}", rows_to_table(&rows));
```

## 13. 吐环境 / 纯算签名

```rust
let mut probe = tab.dump_env()
    .target_query("a_bogus")
    .target_header("x-sign")
    .match_url("/api/")
    .start()
    .await?;

tab.get("https://target").await?;
let dump = probe.collect().await?;
dump.write_to("/abs/dump-env")?;
dump.export_project("/abs/site-env", EnvScope::All)?;
```

用 `--features signer` 可把导出的 env/sign 逻辑放进 QuickJS 单二进制;参考 `examples/env/env_signer.rs`。

## 14. 主动逆向

CDP 逆向入口:

```rust
// ① XHR/事件断点 + 调用栈/局部变量
let dbg = tab.debugger();
dbg.enable().await?;
dbg.break_on_xhr("/api").await?;
if let Some(stack) = dbg.wait_paused(Some(Duration::from_secs(20))).await? {
    println!("{}", stack.backtrace());
    let v = stack.eval(0, "typeof sign").await?;
    stack.resume().await?;
}

// ② dump / grep / beautify 脚本
let scripts = tab.scripts();
let matches = scripts.grep("x-sign").await?;
scripts.dump_all("/abs/js").await?;

// ③ Hook crypto / JSON / base64 / fetch / XHR
let hook = tab.hook()
    .crypto_subtle()
    .crypto_js()
    .json()
    .base64()
    .xhr()
    .fetch()
    .with_stack()
    .start()
    .await?;
let hits = hook.drain().await;
hook.stop().await?;

// ④ 反无限 debugger:导航前注入,或用 Debugger 原生通杀
tab.anti_anti_debug().await?;
tab.debugger().set_skip_all_pauses(true).await?;

// ⑤ 进跨域 OOPIF,如 Turnstile iframe
if let Some(cf) = tab.wait_oopif("challenges.cloudflare.com", Some(Duration::from_secs(8))).await? {
    let list = cf.tab().scripts().list().await?;
}
```

遇到签名、混淆、无限 debugger、Turnstile iframe、WASM、抓包重放,先读 `docs/逆向增强.md`;CF 协议化再读 `docs/CF协议过.md`。

## 15. 常用示例命令

```bash
cargo run --example cdp_demo
cargo run --example cdp_extras --features cdp
cargo run --example cdp_recorder
cargo run --example cdp_pool
cargo run --example cdp_fingerprint
cargo run --example yidun_click_stable --features cdp,ocr
cargo run --example ocr_hotswap --features ocr
cargo run --example session_tls --features impersonate
cargo run --example env_signer --features signer
cargo run --example re_probe --features cdp

cargo run --example quickstart --no-default-features --features camoufox
cargo run --example geetest_slide --no-default-features --features slider
cargo run --example ocr_captcha --no-default-features --features camoufox,ocr
```

## 16. Gotchas

1. `get()` 返回 `Result<bool>`;超时/加载失败通常是 `Ok(false)`,不是 Err。
2. 导航后立刻读 `title/url/html` 可能遇到旧上下文;先等 `doc_loaded/ele_displayed/network_idle`。
3. `ClickWord::solve` 的 targets 是 `&[String]`。
4. CDP 的监听 / 拦截 handle 与 Camoufox 的同名 handle 有些签名不同;默认 CDP 按本 skill 写。
5. `CdpFingerprint*` 在 `drission::cdp`,不在 `prelude`。
6. `expose_function` 需要用户工程直接依赖 `serde_json` 来构造返回值。
7. 文件上传、截图、下载、导出路径用绝对路径。
8. Camoufox 页面/滑块示例必须 `--no-default-features`;CDP 示例默认即可。
9. `Runtime.enable` / `Debugger.enable` 是反检测探测面。`console`、`recorder`、`hook`、`expose_function`、`debugger/scripts` 用于开发/逆向,强过盾生产链路谨慎启用。
10. Windows 高 DPI 已由 CDP 后端强制修正;若仍点偏,优先检查页面坐标是否从原图自然尺寸正确映射到 CSS 视口坐标。
11. `StaticElement` 不是 `Send`;不要跨任务传递。
12. 图片格式仅 PNG/JPEG;不要写 WebP 截图逻辑。
