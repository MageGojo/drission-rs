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

**强制规则**：浏览器观察、页面内容提取、截图、抓包、过盾 — 必须走 `drs` CLI 或 MCP（详见项目 skill `.cursor/skills/drs-browser/SKILL.md`）。不要写 Playwright/Puppeteer/Selenium/WebFetch 替代。

如果任务只是让 AI 观察页面、点选、输入、截图、抓接口、读无障碍树,优先用 `drs` CLI/MCP:

```bash
cargo install drission-cli --bin drs
drs ensure-serve --headless
drs --json extract https://example.com --save-out data/browser/page.json
drs --ensure-serve --ensure-headless --json open https://example.com
drs --json ax --json
drs --json listen start /api/ --xhr-only
drs --json listen wait --count 3 --timeout-ms 5000
drs --json screenshot --out /tmp/page.png --full
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

MCP 默认 attach 到常驻 daemon 的**同一个持久浏览器**(浏览器活在 `drs serve` 进程,固定 profile,MCP 重启不丢标签/登录态,CLI 与 MCP 共享标签),因此适合“AI 下次直接接着查数据、不用重登”。要一次性独立浏览器时用 `drs mcp --standalone`。稳定 MCP 工具名包括 `browser_extract`、`browser_open`、`browser_tabs`、`browser_ax`、`browser_html`、`browser_title`、`browser_url`、`browser_text`、`browser_eval`、`browser_click`、`browser_type`、`browser_wait`、`browser_screenshot`、`browser_close`、`network_listen_start`、`network_listen_wait`、`network_listen_stop`、`browser_identity`、`browser_identity_pool`、`browser_pass_cf`,以及账号/Profile 运行态治理工具 `identity_assets_validate`、`identity_assets_status`、`identity_assets_forecast`、`identity_assets_gate`、`identity_assets_select`、`identity_assets_release`、`identity_assets_reconcile_runtime`、`identity_assets_health`、`identity_assets_sweep`。

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

let report = tab.identity_report().await?;
eprintln!("identity id = {}", report.identity_id);
if report.has_high_risk() {
    for issue in &report.issues {
        eprintln!("{:?} {}: {}", issue.severity, issue.code, issue.suggestion);
    }
}
for action in &report.fix_plan.actions {
    eprintln!("fix {:?} {} fields={:?}", action.priority, action.code, action.fields);
}

// tab_a / tab_b 是不同账号或不同 BrowserContext 的 Tab。
let fp_a = tab_a.fingerprint_snapshot().await?;
let fp_b = tab_b.fingerprint_snapshot().await?;
let link = fp_a.linkability_to(&fp_b);
if link.same_identity_likely {
    eprintln!("identity linkability score = {}", link.score);
}

let pool = IdentityPoolReport::analyze(&[fp_a, fp_b]);
if pool.has_risky_pairs() {
    eprintln!("max linkability = {}", pool.max_linkability);
}
```

`identity_report()` 是启发式身份一致性诊断:检查 `HeadlessChrome`、`navigator.webdriver`、UA/platform/Client Hints/WebGL OS 冲突、软件渲染、语言与时区强冲突、移动端触摸点缺失、canvas/WebGL 不可用等。`identity_id` / `stable_hash` 用于跨轮 baseline 和日志追踪。`report.fix_plan.actions` 会把 issue 合并为机器可执行动作,例如 `ua.normalize`、`client_hints.sync`、`profile.align_os`、`stealth.webdriver_false`、`gpu.enable_webgl`、`canvas.restore_hash`。`linkability_to()` 比较两份快照是否仍可被 UA、时区、screen/DPR、WebGL、canvas 等稳定信号关联;`IdentityPoolReport::analyze()` 面向整批账号 / BrowserContext,汇总最高关联分、风险 pair 和重复稳定信号。它们适合放在高并发账号池/画像池的自检步骤里。不要把高分理解成"保证过盾"。
池级结果优先读取离线 CLI 顶层 `actionQueue.actions[]`;它把候选隔离、池级修复和容量扩容合成扁平动作,字段包含 `source`、`actionCode`、`target`、`priority`、`indexes`、`identityIds`、`reasons`、`signalCodes` 和关联到的内部/基线 ID。`source=capacity` 的动作来自 `capacityPlan`,例如 `capacity.disperse_canvas_seed`、`capacity.disperse_webgl_renderer`、`capacity.rotate_locale_proxy`,并带 `estimatedGain`。`report.remediationPlan` 或顶层 `remediation` 仍保留池级修复计划,例如 `pool.quarantine_offenders`、`pool.disperse_canvas_seed`、`pool.disperse_webgl_renderer`、`pool.rotate_locale_proxy`。需要落盘时传 `--actions-out actions.json`,需要长期流水时传 `--actions-out actions.ndjson --append-actions`。
离线 `identity-pool` 的 `admission` 和 `againstReport` 同时提供 index 与稳定 ID 字段:优先用 `acceptIds` / `quarantineIds` / `candidateId` / `baselineId` 做跨轮追踪,下标只用于映射当前输入文件。
调度器优先读离线 `ledger.entries[]`:每个候选都有 `identityId`、`decision`、`knownInBaseline`、`duplicateInBatch`、内部/基线 linked IDs、最高关联分、`signalCodes` 和 `reasons`。
池级集中度读 `diversity` 或 `report.diversity`:它按稳定信号给出 `uniqueCount`、`repeatedValueCount`、`maxBucketRatio` 和 `buckets`,用于发现 UA / 时区 / WebGL / canvas 等画像过度集中。准入时可传 `--max-concentration-ratio` / `--max-concentrated-signals`;MCP 对应字段是 `max_concentration_ratio` / `max_concentrated_signals`。
池级容量判断读 `entropyBudget` 或 `report.entropyBudget`:优先看 `effectiveIdentityCount`、`nominalToEffectiveRatio`、`entropyScore`、`status` 和 `bottleneckSignals[]`。如果名义账号数很大但 `effectiveIdentityCount` 很低,说明模板/指纹种子/代理地域过度集中,调度器应优先执行 `pool.disperse_*` 或隔离高风险画像。准入时可传 `--min-entropy-score`、`--min-effective-identities`、`--max-nominal-to-effective-ratio`;MCP 对应字段是 `min_entropy_score`、`min_effective_identities`、`max_nominal_to_effective_ratio`。
扩容建议读 `capacityPlan` 或 `report.capacityPlan`:优先看 `additionalDistinctProfilesNeeded`、`missingEffectiveIdentityCount`、`status` 和 `actions[]`。当它给出 `capacity.disperse_canvas_seed` / `capacity.disperse_webgl_renderer` / `capacity.rotate_locale_proxy` / `capacity.rotate_browser_persona` 时,调度器应优先补充这些维度的差异,而不是盲目增加同模板账号。离线 `identity-pool` 已把这些动作同步进 `actionQueue.actions[]`,机器消费时优先读 actionQueue。
跨轮稳定性读 `identity-drift`:优先用 `--match-by label` 按 `accountId` / `profileId` / `id` / `label` 匹配两轮 snapshots,输出每个账号/profile 的 `label`、`beforeIndex`、`afterIndex`、`score`、`severity`、`stableHashChanged`、`signals` 和 `remediation`;高风险 signal 如 `webgl.changed`、`canvas.changed`、`webdriver.changed` 应优先按 `drift.quarantine_current` 隔离当前画像,再按 `drift.restore_canvas_seed` / `drift.restore_webgl_renderer` / `drift.hide_webdriver` 等动作恢复或重新生成 profile。调度器可直接读顶层 `actionQueue.actions[]`;需要落盘时传 `--actions-out actions.json`,需要长期流水时传 `--actions-out actions.ndjson --append-actions`。巡检门禁可传 `--max-drift-score` / `--fail-on-high-risk-drift`。
跨轮账号池状态读 `identity-lifecycle`:它把 baseline/current 两轮采样合成 `active`、`repair`、`quarantine`、`missing_current`、`new_current` 生命周期账本。调度器优先读 `ledger.entries[]` 的 `state` 和顶层 `actionQueue.actions[]`;状态机动作包括 `lifecycle.quarantine_profile`、`lifecycle.investigate_missing_current`、`lifecycle.review_new_profile`,漂移修复动作仍保留 `drift.*` 动作码。巡检门禁可传 `--max-drift-score`、`--fail-on-high-risk-drift`、`--fail-on-missing-current`、`--fail-on-new-current`;需要归档时传 `--ledger-out lifecycle.json`,需要变更审计时传 `--delta-out lifecycle-delta.json`,需要多轮审计流水时传 `--journal-out lifecycle-journal.ndjson --append-journal`,需要按状态分发任务时传 `--state-out-dir lifecycle-states`,需要动作流水时传 `--actions-out lifecycle-actions.ndjson --append-actions`。需要把审计结果写回资产池时传 `--next-baseline-out next-baseline.json`;默认 `conservative` 策略会保留 active 当前画像、repair/missing 旧基线并排除 quarantine/new,可用 `--next-baseline-policy active-only|accept-current-repair` 调整。
团队级治理规则优先写成 `--policy identity-policy.json`,而不是每次命令重复阈值。policy 支持 `gatePreset`、`minScore`、`maxLinkability`、`maxConcentratedSignals`、`minEntropyScore`、`minEffectiveIdentities`、`maxNominalToEffectiveRatio`、`drift.matchBy`、`drift.maxDriftScore`、`drift.failOnHighRiskDrift`、`lifecycle.failOnMissingCurrent`、`lifecycle.failOnNewCurrent`、`lifecycle.nextBaselinePolicy`、`health.windowSeconds`、`health.repairThreshold`、`health.quarantineThreshold`、`health.cooldownSeconds`、`job.preset`、`job.maxFailedAssets`、`job.maxFailedAssetsPerReason`、`job.failureReasonRules`、`job.runtimeRiskLedgers`、`job.runtimeRiskWindowSeconds`、`job.runtimeRiskOut`、`job.appendRuntimeRisk`、`job.explainOut`;命令行显式数值和 true flag 只覆盖本次运行。响应里的 `policy.path` / `policy.rules` 是审计依据,需要随 journal 一起保留。`job.preset` / `--job-preset` 支持 `publish_conservative`、`login_sensitive`、`scrape_aggressive`,先选业务风险模型,再局部覆盖阈值。
执行动作队列读 `identity-apply`:输入可以是 `--actions-out` JSON、追加式 NDJSON 或完整 `drs --json` 输出。默认 dry-run,输出 `operations[]` 和 `assetPatches[]`;只有显式 `--execute` 才会把 `--profile-root/<label-or-identityId>` 或 `--profile-map` 指向的 profile 移动到 `--quarantine-dir`(默认 `_quarantine`)。`--profile-map` 既支持简单 label/path 映射,也支持 `{ "profileAssets": [...] }` / `{ "assets": [...] }` manifest,条目可带 `accountId`、`profileId`、`identityId`、`profileDir`、`proxyId`、`fingerprintSeed`、`state`;命中后读 `operations[].asset` 和 `assetPatches[]`,需要把状态变更落盘时传 `--asset-state-out asset-state.json`,长期流水用 `--append-asset-state`。它只直接执行 `*.quarantine*` 这类隔离动作;`drift.restore_*`、`pool.disperse_*`、`lifecycle.review_new_profile` 等动作保留为计划项,由调度器另行生成修复配置。需要审计时传 `--journal-out apply.ndjson --append-journal`。
多产物复盘读 `identity-plan`:把 `identity-pool` / `identity-drift` / `identity-lifecycle` / `identity-apply` 的完整 JSON、action NDJSON、asset-state NDJSON 一起传入,例如 `drs --json identity-plan pool.json lifecycle-actions.ndjson apply.ndjson asset-state.ndjson --out plan.json --html-out plan.html`。优先读输出里的 `summary`、`dispatchQueue.items[]`、`executionRunbook[]`、`recommendations[]`、归一化 `actions[]` 和 `assetPatches[]`;它会统一统计 gate failure、隔离/修复/capacity 动作、apply failed/unresolved 和资产状态变更。调度器应优先消费 `dispatchQueue.items[]`,它已按阶段展开并带 `sortRank`、`dedupeKey`、`leaseKey`、目标账号/profile 字段和可选命令提示;需要落盘给调度器时传 `--dispatch-out dispatch.ndjson --append-dispatch`。人读流程时看 `executionRunbook[]`:先 `gate_review`,再 `quarantine`,再 `repair` / `capacity` / `review`,然后 `manifest_writeback`,最后 `resample_verify`;每步的 `actionIndexes[]` / `assetPatchIndexes[]` 回指完整 payload。需要给人复盘时使用 `--html-out` 生成 HTML 审计报告。需要闭环回资产池时传 `--asset-manifest profile-assets.json --asset-manifest-out next-profile-assets.json`;它会按 `accountId` / `profileId` / `identityId` / `label` / `profileDir` 匹配并写回 `state`、已执行隔离后的 `profileDir` 和 `lastIdentityPlan*` 审计字段。
领取调度工作读 `identity-dispatch`:输入 `identity-plan --dispatch-out` 的 JSON/NDJSON 或完整 plan 输出,例如 `drs --json identity-dispatch dispatch.ndjson --worker worker-a --limit 10 --claim-ledger claims.ndjson --completion-ledger completed.ndjson --claim-out claims.ndjson --append-claim`。它按 `sortRank` / `dispatchIndex` 排序,默认跳过 `blockedByGate`,按 `dedupeKey` 去重,再读取 `--claim-ledger` 跳过未过期 active lease,读取 `--completion-ledger` 跳过最新状态为 `succeeded` / `cancelled` / 不可重试 `failed` 的终态任务,输出 `claimId`、`leaseExpiresUnixSeconds`、`activeLeaseCount`、`skippedLeasedCount`、`skippedCompletedCount`、`terminalCompletionCount`、`retryableCompletionCount` 和 `items[]`;每个 item 保留原始 `dispatch` payload。需要强制领取被 gate 阻塞项时才传 `--include-blocked`;需要重领 active lease 时才传 `--include-leased`;需要重领终态完成项时才传 `--include-completed`。长任务执行中用 `identity-dispatch-renew claims.ndjson --worker worker-a --lease-seconds 900 --claim-out claims.ndjson --append-claim` 追加续租心跳,续租 item 会带 `renewalId`、`renewedAtUnixSeconds`、`previousLeaseExpiresUnixSeconds`;默认只续未过期 `leased` item,抢救过期租约才传 `--include-expired`。执行完成后用 `identity-dispatch-complete claims.ndjson --status succeeded|failed|retry|cancelled --complete-out completed.ndjson --append-complete` 写 completion ledger;`failed` 默认终态,需要重派时用 `--status retry` 或 `--status failed --retryable`,可加 `--message` / `--result-json` 记录执行结果。每轮调度后用 `identity-dispatch-reconcile profile-assets.json --claim-ledger claims.ndjson --completion-ledger completed.ndjson --asset-manifest-out runtime-profile-assets.json` 把运行态回写到资产池:活跃租约写 `dispatchState=leased`,completion 写 `succeeded|failed|retry|cancelled` 和 `lastDispatch*`,成功结果可同步 `state` / `profileDir`;匹配字段沿用 `accountId` / `profileId` / `identityId` / `label` / `profileDir`。业务任务启动前先用 `identity-assets-status runtime-profile-assets.json --desired-concurrency 5 --status-out asset-status.json` 做只读容量看板:优先看 `runnableCount`、`capacityStatus`、`capacityShortageCount`、`blockReasonCounts`、active/expired lease/cooldown 统计和 `recommendations[]`,确认“还能跑多少账号、为什么跑不了”。如果 `capacityStatus=shortage`,继续用 `identity-assets-forecast runtime-profile-assets.json --desired-concurrency 5 --horizon-seconds 3600 --forecast-out asset-forecast.json` 看 cooldown/lease/retry 到期后的恢复时间线和 `enoughAtUnixSeconds`。启动 worker 前用 `identity-assets-gate runtime-profile-assets.json --desired-concurrency 5 --max-wait-seconds 600 --gate-out asset-gate.json` 做硬门禁;`decision=run_now` 才默认退出码 0,`decision=wait` 默认退出码 2,只有显式 `--allow-wait` 才把可等待恢复当作通过。随后用 `identity-assets-select runtime-profile-assets.json --limit 5 --worker worker-a --job publish --asset-manifest-out leased-profile-assets.json --selection-out selection.json` 做 profile 准入和 runtime lease:默认只选 `state=active` 且无 active dispatch/runtime lease、retry 冷却、failed/cancelled 的资产,并写 `runtimeLease*` 字段防并发 worker 抢同一账号。业务结束后用 `identity-assets-release leased-profile-assets.json --worker worker-a --job publish --status succeeded|failed|cancelled --asset-manifest-out released-profile-assets.json --release-out runtime-release.ndjson --append-release` 释放 runtime lease;失败可加 `--cooldown-seconds` 和 `--next-state repair`,释放会写 `lastRuntime*` 审计字段并清掉活跃 `runtimeLeaseId/WorkerId/JobId/Expires`;加 `--append-release` 时每个 released asset 会追加为一行 NDJSON runtime release ledger,长期保留业务成功、失败、冷却和状态变更证据。随后用 `identity-assets-reconcile-runtime runtime-profile-assets.json --release-ledger runtime-release.ndjson --asset-manifest-out reconciled-profile-assets.json` 把一个或多个 worker 的 release ledger 回放到中心资产池,合并 `lastRuntime*`、cooldown、next state 并清掉活跃运行租约。再用 `identity-assets-health reconciled-profile-assets.json --policy identity-policy.json --release-ledger runtime-release.ndjson --asset-manifest-out health-profile-assets.json --health-out asset-health.json` 按历史 release 结果计算健康分、连续失败和建议动作;触发阈值的资产会写成 `repair` / `quarantine` 并加 cooldown。定期用 `identity-assets-sweep health-profile-assets.json --asset-manifest-out clean-profile-assets.json --sweep-out sweep.json` 清理过期残留:过期 runtime lease 标 `expired` 并归档,过期 dispatch lease 标 `expired`,到期 cooldown 自动清除;可用 `--runtime-grace-seconds` / `--dispatch-grace-seconds` / `--cooldown-grace-seconds` 设置宽限期。
已有 Python/Node/shell 业务脚本优先用 `identity-job run`:例如 `drs --json identity-job run clean-profile-assets.json --job-preset publish_conservative --policy identity-policy.json --per-asset --child-concurrency 5 --runtime-renew-interval-seconds 300 --child-timeout-seconds 1800 --child-result-dir child-results --max-failed-assets 1 --max-failed-assets-per-reason 2 --worker worker-a --job publish --asset-manifest-out job-profile-assets.json --release-out runtime-release.ndjson --append-release --runtime-risk-out runtime-risk.ndjson --append-runtime-risk --explain-out job-explain.json -- python publish.py`。它会自动 sweep/validate/runtime-risk-gate/gate/select,把 `DRS_IDENTITY_SELECTED_ASSETS_JSON`、`DRS_IDENTITY_ASSET_MANIFEST`、`DRS_IDENTITY_WORKER`、`DRS_IDENTITY_JOB` 注入子进程,结束后按子进程退出码 release 为 `succeeded` 或 `failed`;失败治理可写入 policy 的 `job.failureCooldownSeconds` / `job.failureNextState`,本次命令也可用 `--failure-cooldown-seconds` 和 `--failure-next-state repair` 覆盖。若脚本是一进程一账号模型,加 `--per-asset` 或 policy `job.perAsset=true`:每个子进程会拿到 `DRS_IDENTITY_ASSET_JSON`、`DRS_IDENTITY_LABEL`、`DRS_IDENTITY_PROFILE_DIR`、`DRS_IDENTITY_RUNTIME_LEASE_ID` 等单资产变量,`--child-concurrency` / `job.childConcurrency` 可由 drission-rs 侧并发拉起旧脚本实例,`--runtime-renew-interval-seconds` / `job.runtimeRenewIntervalSeconds` 会在长任务运行中自动续 runtime lease,`--child-timeout-seconds` / `job.childTimeoutSeconds` 会把卡死子进程记录为 `timedOut=true` 并按失败 release,`--child-result-dir` / `job.childResultDir` 会注入 `DRS_IDENTITY_RESULT_OUT` 让脚本写 `status/message/reason/result` JSON;脚本显式 `cooldownSeconds/nextState` 优先,否则由 `job.failureReasonRules.<reason>.cooldownSeconds/nextState` 决定冷却和状态迁移,同一规则也可用 `recommendedAction` / `runtimeRiskSeverity` / `nextSuggestedLimit` / `nextSuggestedDesiredConcurrency` / `runtimeRiskMessage` / `runtimeRiskCooldownSeconds` 直接覆盖本轮 `runtimeRisk`,并生成精确的 `suppressUntilUnixSeconds`。`--max-failed-assets` / `job.maxFailedAssets` 会在失败数达到阈值后停止启动剩余账号,`--max-failed-assets-per-reason` / `job.maxFailedAssetsPerReason` 会在同一个 `reason` 或 `result.reason` 达到阈值后停止启动剩余账号并把未执行租约释放为 `cancelled`,且失败只 release 自己的 lease。调度器下一轮优先读顶层 `runtimeRisk`:当 `recommendedAction=pause_pool` 暂停整池,`pause_failure_reason` 暂停该业务原因,`reduce_concurrency` 使用 `nextSuggestedLimit` / `nextSuggestedDesiredConcurrency` 降速,`continue_current` 维持当前参数。需要跨 worker / 跨轮聚合时加 `--runtime-risk-out runtime-risk.ndjson --append-runtime-risk` 或 policy `job.runtimeRiskOut/job.appendRuntimeRisk`,它会把同一建议写成顶层可查询的 `identity_job_runtime_risk_event` 风险流水。下一轮加 `--runtime-risk-ledger runtime-risk.ndjson --runtime-risk-window-seconds 900` 或 policy `job.runtimeRiskLedgers/job.runtimeRiskWindowSeconds`,sidecar 会在 select 前读取最近同 job 风险事件:带 `suppressUntilUnixSeconds` 的事件按精确截止时间生效,即使已经超出普通窗口仍会拦截;`pause_pool` / `pause_failure_reason` 直接停止且不写 runtime lease,`reduce_concurrency` 自动降低本轮 `limit` / `desiredConcurrency`。每次响应的 `explain.stageDecisions[]` 解释本轮阶段级通过/阻断/调整原因,`explain.assetDecisions[]` 解释账号/Profile 被 selected、blocked、child failed/succeeded 或 released 的原因;`--explain-out` / `job.explainOut` 可单独落盘。调度器排查“为什么没跑”时优先读 explain,再 drill down 到完整 job report。这条路径适合“不迁移 Python 业务逻辑,只把账号/Profile 租约、续租、超时熔断、池级失败熔断、按业务原因熔断、policy preset、policy 化业务原因治理、业务判责、风险摘要、风险流水、下一轮风险门禁、explain 审计、冷却、并发、健康和 ledger 交给 drission-rs”。
Python 旧脚本接入时优先用 `python/drission_sidecar`:设置 `PYTHONPATH=/path/to/drission-rs/python`,脚本里 `from drission_sidecar import asset, profile_dir, succeeded, failed`;业务成功调用 `succeeded("published", result={...})`,业务失败调用 `failed("rate_limited", cooldown_seconds=900, next_state="repair")`。这个 helper 只读 `DRS_IDENTITY_*` 环境变量并写 `DRS_IDENTITY_RESULT_OUT`,不提供浏览器 API,避免把治理逻辑搬回 Python。
运行态复盘优先用 `identity-ledger query`:例如 `drs --json identity-ledger query --release-ledger runtime-release.ndjson --runtime-risk-ledger runtime-risk.ndjson --window-seconds 86400 --job publish --reason rate_limited --out ledger-query.json`。重点读 `failureReasonCounts`、`topAssets`、`runtimeRiskActionCounts`、`runtimeRiskSeverityCounts`、`activeSuppressions` 和 `recommendations[]`;带 `suppressUntilUnixSeconds` 且仍生效的 risk event 会被保留,用来解释下一轮为什么继续暂停或降速。
单账号/单原因排查用 `identity-ledger explain`:例如 `drs --json identity-ledger explain --release-ledger runtime-release.ndjson --runtime-risk-ledger runtime-risk.ndjson --window-seconds 86400 --job publish --reason rate_limited --account-id acct-a --out ledger-explain.json`。重点读 `decision`、`blockingReasons[]`、`nextRunnableUnixSeconds`、`activeSuppressions`、`activeCooldowns`、`releaseEvidence[]` 和 `runtimeRiskEvidence[]`;`decision=blocked_by_runtime_risk_suppression` 或 `blocked_by_asset_cooldown` 可直接作为调度阻断依据。
长期账本归档前用 `identity-ledger compact`:例如 `drs --json identity-ledger compact --release-ledger runtime-release.ndjson --runtime-risk-ledger runtime-risk.ndjson --window-seconds 86400 --job publish --retain-recent 50 --checkpoint-out ledger-checkpoint.json --out ledger-compact.json`。重点读 `compactedThroughUnixSeconds`、`assetSummaries[]`、`activeSuppressions`、`nextSuppressionUntilUnixSeconds`、`sourceCheckpoints[]`、`retainedReleaseEvidence[]` 和 `retainedRuntimeRiskEvidence[]`;compact 文件保留 active suppression 和最近证据,适合调度器快速读取,原始 NDJSON 再转冷归档。下一轮传 `--checkpoint-in ledger-checkpoint.json` 只读新增 tail;如果源 ledger 轮转/截断,读 `sourceCheckpoints[].reset` 判断本轮是否从 0 重读。
人工复盘用 `identity-ledger dashboard`:例如 `drs --json identity-ledger dashboard --release-ledger runtime-release.ndjson --runtime-risk-ledger runtime-risk.ndjson --window-seconds 86400 --job publish --checkpoint-in ledger-checkpoint.json --checkpoint-out ledger-checkpoint.json --out ledger-dashboard.json --html-out ledger-dashboard.html`。重点读 JSON 顶层 `summary.status`、`summary.recommendedAction`、`summary.failureRatePermille` 和 `summary.activeSuppressionCount`;HTML 给人看 active suppression、top assets、失败原因、risk action 和最近证据。

MCP 中同一运行态治理链路使用 snake_case 工具名和参数名:例如 `identity_assets_gate { "asset_manifest": "runtime-profile-assets.json", "desired_concurrency": 5, "max_wait_seconds": 600 }`、`identity_assets_select { "asset_manifest": "runtime-profile-assets.json", "limit": 5, "worker": "worker-a", "job": "publish", "asset_manifest_out": "leased-profile-assets.json" }`、`identity_assets_release { "asset_manifest": "leased-profile-assets.json", "worker": "worker-a", "job": "publish", "status": "failed", "cooldown_seconds": 600, "result_json": {"reason":"rate_limited"} }`。MCP 不做进程退出码门禁,调度器应读结构化结果里的 `passed`、`decision`、`selectedCount`、`releasedCount`、`updatedAssetCount` 等字段。
资产池进入调度前优先跑 `identity-assets-validate runtime-profile-assets.json --strict --validate-out asset-validate.json`:读取 `valid`、`errorCount`、`warningCount`、`issueCodeCounts` 和 `issues[]`;重复 `accountId/profileId/identityId/label/profileDir`、坏 Unix 时间戳、缺稳定匹配键是 error,`--strict` 会在打印报告后退出码 `2`;缺 profileDir、未知 state、过期 lease/cooldown 是 warning,通常先修 manifest 或跑 `identity-assets-sweep` 后再 `identity-assets-status`。
需要归档时传 `--ledger-out ledger.json`;需要长期流水时传 `--ledger-out ledger.ndjson --append-ledger`,此时每行是一条 candidate ledger entry。

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
