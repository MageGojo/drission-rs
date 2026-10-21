---
name: drission-rs
description: Rust browser automation skill for the drission crate and drs CLI/MCP. Use when building or generating code with drission-rs for Chromium/CDP or Camoufox, DrissionPage-style selectors, local-page tests, the public apizero.cn pages, network listen on pages you own, PDF/HAR/recorder, or profiles. Do not write captcha, Cloudflare, anti-detect, or third-party bypass workflows.
---

# drission-rs 编程技能

本文件给 AI 编程助手使用。基于 `drission` 写代码时,先按这里选后端、feature、命令和 API。更细的背景只在需要时读对应文档,不要凭记忆猜内部路径或方法签名。

配套入口:

- `docs/API映射.md`: 从 DrissionPage 迁移或查完整 API 对照。
- `docs/CLI.md`: 使用 `drs` CLI / MCP 让 AI 直接操作浏览器。
- `docs/产品级CLI.md`: **AI 怎么选命令**：产品 CLI + 语义 MCP 配方（`browser_network` / `console` / `download`）。
- `docs/标配补齐.md`: CDP PDF/MHTML/HAR/expose/media/network/device/storage/wait 补齐。
- `docs/录制与无障碍.md`: `tab.recorder()` 录制生成 Rust + `ax_tree/ax_snapshot`。
- `docs/并发池.md`: `BrowserPool` / `ChromiumPool`、代理、断点续跑。
- `docs/Chrome自动下载.md`: CDP 自动定位 / 下载 Chrome for Testing。
- `docs/法律风险.md`: 公开示例只测本地页和自己的站。
- `examples/README.md`: 可运行示例。`own_site` 是自己的站。

## 1. 先选执行入口

**强制规则**：浏览器观察、页面内容提取、截图、抓包 — 必须走 `drs` CLI 或 MCP（详见项目 skill `.cursor/skills/drs-browser/SKILL.md`）。不要写 Playwright/Puppeteer/Selenium/WebFetch 替代。只打开自己有权访问的页面。

如果任务只是让 AI 观察页面、点选、输入、截图、抓接口、读无障碍树,优先用 `drs` CLI/MCP。怎么选命令见 [`产品级CLI.md`](产品级CLI.md)。

```bash
cargo install drission-cli --bin drs
drs ensure-serve --headless
drs --json tab open https://example.com
drs --json snapshot
drs --json extract https://example.com --save-out data/browser/page.json
drs --json inspect
drs --json listen start /api/ --xhr-only --method POST
drs --json network inspect --limit 20
drs --json listen wait --count 3 --timeout-ms 5000
drs --json console start --level error
drs --json download start --dir data/browser/downloads
drs --json screenshot --out /tmp/page.png --full
drs --json pdf --out data/browser/page.pdf
drs --json stop
drs --json identity --pool
drs --json identity --pool --snapshots-out ./snapshots.json
drs --json identity --pool --snapshots-out ./snapshots.ndjson --append-snapshots
drs --json identity --pool --gate-preset balanced
drs --json identity --pool --min-score 80 --max-linkability 25 --fail-on-risky-pairs
drs --json identity-pool ./snapshots.json --max-linkability 25
drs --json identity-pool ./snapshots.json --max-concentration-ratio 0.8 --max-concentrated-signals 3 --min-entropy-score 60 --max-nominal-to-effective-ratio 2
drs --json identity-pool ./candidates.json --against ./baseline.ndjson --gate-preset strict
drs --json identity-pool ./candidates.json --policy ./identity-policy.json --against ./baseline.ndjson
drs --json identity-pool ./candidates.json --against ./baseline.ndjson --accept-out ./accepted.json --quarantine-out ./quarantine.json --baseline-out ./next-baseline.json --ledger-out ./ledger.json
drs --json identity-pool ./candidates.json --actions-out ./pool-actions.ndjson --append-actions
drs --json identity-drift ./baseline.json ./current.json --match-by label --max-drift-score 20 --fail-on-high-risk-drift --actions-out ./drift-actions.ndjson --append-actions
drs --json identity-lifecycle ./baseline.json ./current.json --policy ./identity-policy.json --next-baseline-out ./next-baseline.json --actions-out ./lifecycle-actions.ndjson --append-actions
drs --json identity-apply ./lifecycle-actions.ndjson --profile-root ./profiles --journal-out ./apply.ndjson --append-journal
drs --json identity-job run ./runtime-profile-assets.json --job-preset publish_conservative --per-asset --child-result-dir ./child-results --release-out ./runtime-release.ndjson --append-release --runtime-risk-out ./runtime-risk.ndjson --append-runtime-risk -- python publish.py
drs --json identity-ledger compact --release-ledger ./runtime-release.ndjson --runtime-risk-ledger ./runtime-risk.ndjson --window-seconds 86400 --job publish --retain-recent 50 --checkpoint-out ./ledger-checkpoint.json --out ./ledger-compact.json
drs --json identity-ledger dashboard --release-ledger ./runtime-release.ndjson --runtime-risk-ledger ./runtime-risk.ndjson --window-seconds 86400 --job publish --checkpoint-in ./ledger-checkpoint.json --checkpoint-out ./ledger-checkpoint.json --out ./ledger-dashboard.json --html-out ./ledger-dashboard.html
drs --json identity-ledger query --release-ledger ./runtime-release.ndjson --runtime-risk-ledger ./runtime-risk.ndjson --window-seconds 86400 --job publish --out ./ledger-query.json
drs --json identity-ledger explain --release-ledger ./runtime-release.ndjson --runtime-risk-ledger ./runtime-risk.ndjson --window-seconds 86400 --job publish --account-id acct-a --out ./ledger-explain.json
```

MCP（Cursor 项目已配 `.cursor/mcp.json`, server 名 `drs`）:

```bash
drs mcp --backend cdp --headless
```

MCP 默认 attach 到常驻 daemon 的**同一个持久浏览器**(浏览器活在 `drs serve` 进程,固定 profile,MCP 重启不丢标签/登录态,CLI 与 MCP 共享标签)。要一次性独立浏览器时用 `drs mcp --standalone`。

默认 `core` 工具:**`browser_snapshot`**(读页首选)、`browser_extract`、`browser_open`、`browser_tabs`、`browser_click`/`type`/`press`/`wait`、`browser_screenshot`、**`browser_network` / `browser_console` / `browser_download`**。抓 XHR 用 `browser_network action=listen|wait|inspect`,不必改 `DRS_MCP_TOOLS`。`browser` toolset 才有 `browser_pdf` / `browser_inspect` / `browser_record`、旧 `network_*`。identity 治理另开 `identity`/`all`。配方见 [`产品级CLI.md`](产品级CLI.md),读页细节见 [`AI页面快照.md`](AI页面快照.md)。

只有需要嵌进自己的业务、并发或自定义流程时,再写 Rust。目标页必须是你有权操作的。

## 2. Feature / 后端铁律

`drission` 是双后端库。默认后端是 Chromium / CDP,也就是 Google Chrome / Edge / Brave / Electron。

| 任务 | feature / 命令 | 关键点 |
|---|---|---|
| 默认 Chrome/CDP 自动化 | `drission = "0.3"` 或 `--features cdp` | 默认开启;找不到系统 Chrome 会自动下载 Chrome for Testing。 |
| Camoufox/Firefox | `--no-default-features --features camoufox` | Camoufox 页面示例必须关掉默认 `cdp`。 |
| QuickJS | `--features signer` | 默认关。不要在文档里展开用法。 |

铁律:

1. `prelude` 里的统一名按 `cdp` 优先解析。默认构建下 `Browser/Tab/Page/Element/BrowserOptions` 是 CDP 类型。
2. 同时开 `cdp,camoufox` 时,统一名仍是 CDP。要用另一端显式名: `CamoufoxBrowser` / `CamoufoxPage` / `CamoufoxOptions` 或 `ChromiumBrowser` / `ChromiumOptions`。
3. Camoufox 示例一律 `--no-default-features --features camoufox`。否则很容易类型冲突或跑成 CDP。
4. 可选 feature 默认关闭。公开示例不展开它们。
5. 所有 IO 都是 async: `#[tokio::main]`, 返回 `drission::Result<()>`, 调用 `.await?`。
6. 常规业务代码从 `use drission::prelude::*;` 开始。`ensure_chrome` / `download_chrome_for` 从 `drission::cdp::*` 显式导入。

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

录制是开发期能力,会启用 `Runtime` 域。只在自己的页面上用:

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

## 9. 只测自己的页面

公开示例是 `cargo run --example own_site`：本地页填表，再打开 https://apizero.cn/ 点进文档。

不要写验证码、盾、指纹伪装、别人的站点、签名重放或下载器。库里未在本文件展开的模块，只用于自己有权操作的页面，也不要在新的文档或示例里写成教程。

## 10. 常用示例

```bash
cargo run --example own_site
cargo run --example cdp_demo
cargo run --example cdp_agent
cargo run --example cdp_context
cargo run --example quickstart --no-default-features --features camoufox
cargo run --example slider_local --no-default-features --features camoufox,slider
```

## 11. Gotchas

1. `get()` 返回 `Result<bool>`;超时/加载失败通常是 `Ok(false)`,不是 Err。
2. 导航后立刻读 `title/url/html` 可能遇到旧上下文;先等 `doc_loaded/ele_displayed/network_idle`。
3. CDP 的监听 / 拦截 handle 与 Camoufox 的同名 handle 有些签名不同;默认 CDP 按本 skill 写。
4. `expose_function` 需要用户工程直接依赖 `serde_json` 来构造返回值。
5. 文件上传、截图、下载、导出路径用绝对路径。
6. Camoufox 页面示例必须 `--no-default-features`;CDP 示例默认即可。
7. 调试自己的页面时再开 `console` / `recorder`。不要把这些写成对付别人站点的步骤。
8. Windows 高 DPI 已由 CDP 后端处理。点偏时先核对元素坐标是不是视口坐标。
9. `StaticElement` 不是 `Send`;不要跨任务传递。
10. 图片格式仅 PNG/JPEG;不要写 WebP 截图逻辑。
