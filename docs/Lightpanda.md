# Lightpanda 适配（CDP 引擎变体）

> Lightpanda 讲 **同一套 Chrome DevTools Protocol**。上层 `Browser` / `Tab` / `ele` / `click`
> 与 Chromium 后端完全相同，只换启动命令：`lightpanda serve --host 127.0.0.1 --port <port>`。
>
> 模块：`src/cdp/lightpanda.rs`（定位 / 下载 / spawn）；启动入口仍是
> [`ChromiumBrowser::launch`](../src/cdp/browser.rs)，由 [`CdpEngine`](../src/cdp/options.rs) 分流。

| 场景 | 用什么 |
|---|---|
| 本机看见窗口、强反爬、要真截图 | Chrome（默认） |
| 服务器简单听接口、能接受无真渲染 | `BrowserOptions::new().lightpanda()` |
| Cursor / MCP / 本机 Agent | `drs`（只接 Chrome，不接 Lightpanda） |

## 为什么不是第三个后端

Camoufox 走 Juggler，必须单独一套 `browser/`。Lightpanda **不是**新协议：

- 线格式、`Target.createTarget` / `Page.*` / `Runtime.evaluate` / `Network` / `Fetch` 与 Chrome 同一套
- 连上 WebSocket 之后，现有 `ChromiumTab` 原样工作
- 因此做成 **CDP 引擎变体**，不新增 feature、不复制 Tab API

```
ChromiumOptions::new().lightpanda()
        │
        ▼
lightpanda serve --port N
        │
        ▼
ws://127.0.0.1:N  ──►  同一套 Connection / ChromiumBrowser / ChromiumTab
```

## 怎么用（库）

```rust
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let browser = Browser::launch(BrowserOptions::new().lightpanda()).await?;
    let tab = browser.new_tab(Some("https://example.com")).await?;
    println!("{}", tab.title().await?);
    tab.ele("h1").await?.text().await?;
    browser.quit().await?;
    Ok(())
}
```

显式名：`ChromiumOptions::new().engine(CdpEngine::Lightpanda)`。

接管已开的 `lightpanda serve` 或 Cloud：

```rust
// 本地 serve（HTTP 或直连 WS 都行）
ChromiumBrowser::connect("http://127.0.0.1:9222").await?;
ChromiumBrowser::connect("ws://127.0.0.1:9222").await?;

// Lightpanda Cloud
ChromiumBrowser::connect("wss://euwest.cloud.lightpanda.io/ws?token=YOUR_TOKEN").await?;
```

## 和 `drs` 的分工

`drs` 是本机 Agent / MCP 入口，默认本机 Chrome，**不接 Lightpanda**。服务器上简单听接口用库：

```rust
Browser::launch(BrowserOptions::new().lightpanda()).await?;
```

不需要新 feature，走默认 `cdp`。

## 二进制从哪来

解析优先级（[`ensure_lightpanda`](../src/cdp/lightpanda.rs)）：

1. `LIGHTPANDA_BIN` / `DRISSION_LIGHTPANDA`
2. **当前可执行文件旁边**的 `lightpanda`（或 `bin/lightpanda`）——`dist/` 打包内置
3. `ChromiumOptions::binary_path`
4. `PATH` + 常见安装路径（Homebrew `/opt/homebrew/bin/lightpanda`、`~/.local/bin/lightpanda`）
5. 缓存 `~/.cache/drission/lightpanda/<tag>/<platform>/lightpanda`（`DRISSION_CACHE` 可改根）
6. 从 GitHub Releases 下载（默认 `nightly`，`DRISSION_LIGHTPANDA_TAG` 可钉 `0.3.7` 等）

资产名：`lightpanda-aarch64-macos` / `x86_64-macos` / `x86_64-linux` / `aarch64-linux`。

**没有原生 Windows 二进制**。Windows 上 `launch(.lightpanda())` 会报 `UnsupportedPlatform`，
请用 WSL2 或改回 Chromium。已运行的 WSL `lightpanda serve` 可用本机 `connect("http://127.0.0.1:9222")`
（WSL 会转发 localhost）。

启动时自动设 `LIGHTPANDA_DISABLE_TELEMETRY=true`。

## 启动参数映射

| `ChromiumOptions` | Lightpanda CLI |
|---|---|
| （内部端口） | `--host 127.0.0.1 --port <idle>` |
| `proxy` | `--http-proxy` |
| `user_data_dir` | `--http-cache-dir`（HTTP 缓存，不是 Chrome profile） |
| `user_agent` | `--user-agent`（**禁止含 `Mozilla`**，Lightpanda 会拒；含 Mozilla 则跳过并打日志） |
| `args` | 原样追加（勿传 `--host` / `--port`，由库管理） |
| `headless` | 忽略（Lightpanda 永远无头） |
| `stealth` / GPU / UA mask | 不套用（Chrome 专用启动参数与 stealth JS 不注入） |

## 能力边界（和 Chrome 不完全对称）

Lightpanda 无 Chromium 渲染管线。同一套 API 能调，但部分域是 stub / 弱实现：

| 能力 | 预期 |
|---|---|
| 导航 / `run_js` / `ele` / `click` / `input` | ✅ 主路径 |
| 验证码取图 `ele.image_bytes()` / `tab.ocr_image` / `ocr_calc` / `ocr_code` | ✅ 走 `data:` / canvas / `fetch(src)`，不靠截图 |
| 网络监听 / Fetch 拦截 | ✅ 官方支持 |
| 无障碍 `ax_tree` / snapshot | ✅ 有 Accessibility 域 |
| `Page.captureScreenshot` | ⚠️ 常为占位图，不要当视觉真源 |
| `Page.printToPDF` / 设备模拟 / 高熵 CH | ⚠️ 弱或无 |
| Chrome 反检测 stealth JS | — 不注入 |
| 原生 Windows | — 无 |

站点兼容性不如 Chrome：复杂 SPA / 强指纹 / 要真截图时，改回默认 Chromium。

想自己用 Rust 重写这套内核？先读 [`分析-Lightpanda能否用Rust复现.md`](分析-Lightpanda能否用Rust复现.md)：语言能换，工作量几乎不减，本库不立项。

`Target.createTarget` 在 Lightpanda 上可能忽略初始 url。`ChromiumBrowser::new_tab(Some(url))`
会在附着后发现仍是 `about:blank` 时再走一次 `Page.navigate`，Chrome 已导航则不重复。

## 验证

```bash
cargo test --lib --features cdp lightpanda
cargo run --example lightpanda_demo
# 初和报价监听(账号写 chuhe.config.local.json)
cargo run --example chuhe_price --features cdp,ocr
```

验收：example.com 能 `new_tab` + `title` + `ele("h1")`；`quit` 杀掉 `lightpanda` 子进程。

站点案例见 [`案例-chuhe价格监听.md`](案例-chuhe价格监听.md)。
