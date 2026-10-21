# drission

用 Rust 控本机 Chrome（CDP）。同仓库另有一个二进制 `drs`，给终端脚本和 Cursor / Codex 的 MCP 用。

[![crates.io](https://img.shields.io/crates/v/drission.svg)](https://crates.io/crates/drission)
[![docs.rs](https://docs.rs/drission/badge.svg)](https://docs.rs/drission)
[![Rust](https://img.shields.io/badge/Rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-blue.svg)](#兼容性)
[![License](https://img.shields.io/badge/license-source--available-lightgrey.svg)](LICENSE)

**简体中文** · [English](README.en.md) · [docs.rs](https://docs.rs/drission/0.6.5) · [CHANGELOG](CHANGELOG.md)

> 只在你自己的、或书面授权过的系统上用。别拿去绕登录、验证码、付费墙。条款见[负责任使用](#负责任使用)和 [LICENSE](LICENSE)。

## 这一版

**0.6.5**。库 `drission` 和命令行 `drs`（crate `drission-cli`）同一号。

```bash
cargo add drission
cargo install drission-cli --bin drs
```

`drs --version` 打出 `drs 0.6.5`。没有 Rust 工具链时，下 [GitHub Releases](https://github.com/MageGojo/drission-rs/releases/latest) 或 GitCode 镜像里对应平台的 `drs` 压缩包。`install/` 下有脚本，默认拉 Latest。

## 1. 当库用

`Cargo.toml`：

```toml
[dependencies]
drission = "0.6"
tokio = { version = "1", features = ["full"] }
```

默认 feature 是 `cdp`，会找本机 Chrome / Edge / Brave / Chromium。

```rust
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.new_tab(Some("https://example.com")).await?;
    println!("{}", tab.title().await?);
    browser.quit().await?;
    Ok(())
}
```

三个常用扩展（都在 CDP 上）：

```rust
// 按名字隔离 Cookie / 代理 / UA。同名会复用，关 context 才拆。
let ctx = browser.context("account_001").await?;
let tab = ctx.new_tab(Some("https://example.com")).await?;

// 听、拦、假响应、改写、录 HAR。旧的 tab.listen() / intercept() 还能用。
let listen = tab.network().filter("/api/").method("POST").listen().await?;

// 磁盘登录态。目录是 profiles/<id>/chrome，下次进程还能接着用。
let profiles = ProfileManager::open("profiles")?;
let lease = profiles.acquire("user_001").await?;
let browser = lease.launch().await?;
```

Agent 读页用无障碍大纲，不要喂整页 HTML：

```rust
let page = tab.agent();
let obs = page.observe().await?;          // role + name，不是 outerHTML
page.find("登录按钮").click().await?;
page.type_text(Locator::placeholder("用户名"), "alice").await?;
page.wait_for("Dashboard").await?;
tab.wait().element("#dash").visible().timeout(Duration::from_secs(10)).await?;
let html = tab.capture().html().await?;
```

类型和方法：[docs.rs/drission](https://docs.rs/drission/0.6.5)。仓库里可跑的例子：`cargo run --example own_site`、`cargo run --example cdp_demo`、`cargo run --example cdp_agent`。`own_site` 先在本地页填表，再打开 [apizero.cn](https://apizero.cn/) 的公开文档页。

### Lightpanda

同一套 API，换无头引擎（[Lightpanda](https://lightpanda.io)，讲 CDP）。找不到二进制会下载；无原生 Windows。`drs` 仍用本机 Chrome。

```rust
let browser = Browser::launch(BrowserOptions::new().lightpanda()).await?;
```

说明：[Lightpanda.md](https://github.com/MageGojo/drission-rs/blob/main/docs/Lightpanda.md)。示例：`cargo run --example lightpanda_demo`。

## 2. 当 `drs` 用

`drs` 会在本机起一个 daemon，浏览器活在那个进程里。后面的命令都打给它。机器读加 `--json`。AI 怎么选命令见 [`docs/产品级CLI.md`](docs/产品级CLI.md)。

```bash
drs ensure-serve --backend cdp --headless
drs --json tab open https://example.com
drs --json snapshot          # 大纲 + ref=e1 + Markdown，给脚本和 Agent 读页用
drs --json click "ref:e1"
drs --json stop              # 停 daemon 和浏览器。Cookie 在磁盘 profile 里，下次 ensure-serve 还在
```

同一套 daemon 上还能：

```bash
drs --json tab list
drs --json inspect
drs --json network inspect --limit 20
drs listen start /api/ --xhr-only --method POST
drs --json network mock /api/user --body '{"ok":true}'
drs --json console start --level error
drs --json download start --dir ./data/downloads
drs --json pdf --out ./page.pdf
drs --json context use account_001
drs --json tab open https://example.com --context account_001
drs --json profile list
```

`open` 仍等于 `tab open`。`context` / HAR replay / `pdf` / `record` 只支持 CDP。

接到 Cursor / Codex：

```bash
drs setup --dry-run
drs setup
```

会往现有 MCP 配置里**合并**一项 `drs`，不覆盖别人的 server。默认 `DRS_MCP_TOOLS=core`：开页、snapshot、点按、截图，以及语义化的 `browser_network` / `browser_console` / `browser_download`。要 PDF / inspect / Context / 旧 `network_*`，把环境变量改成 `browser` 或 `all` 再重启 MCP。

怎么选命令：[`docs/产品级CLI.md`](https://github.com/MageGojo/drission-rs/blob/main/docs/产品级CLI.md)。字段细节：[`docs/CLI.md`](https://github.com/MageGojo/drission-rs/blob/main/docs/CLI.md)（crates.io 打包不含 `docs/`）。

## Feature

| Feature | 做什么 | 默认 |
|---|---|---|
| `cdp` | Chrome / Edge / Brave / Chromium / Electron | 开 |
| `camoufox` | Camoufox / Firefox（Juggler） | 关 |
| `signer` | 内嵌 QuickJS | 关 |

```toml
drission = { version = "0.6", default-features = false, features = ["camoufox"] }
```

两端差哪些能力见仓库 [后端能力矩阵](https://github.com/MageGojo/drission-rs/blob/main/docs/后端能力矩阵.md)。Lightpanda 是 CDP 上的可选引擎，不是新 feature。

## 兼容性

| | |
|---|---|
| Rust | 1.85+，edition 2024 |
| 系统 | macOS / Linux / Windows |
| 浏览器 | 先找本机 Google Chrome，也认 Edge、Brave、Chromium、Electron |
| 无桌面 | `headless`；Linux 依赖见仓库「服务器部署」。库也可换 Lightpanda（macOS / Linux） |

## 文档（都在 GitHub，不进 crates.io 包）

- https://github.com/MageGojo/drission-rs — 源码、`docs/`、`examples/`
- https://docs.rs/drission/0.6.5 — rustdoc
- [CLI.md](https://github.com/MageGojo/drission-rs/blob/main/docs/CLI.md) — `drs` 全命令
- [Lightpanda.md](https://github.com/MageGojo/drission-rs/blob/main/docs/Lightpanda.md)
- [CHANGELOG](CHANGELOG.md)

## 负责任使用

浏览器自动化会碰到登录态、个人数据和能写库的操作。用之前至少确认：

1. 只碰你有权用的系统和账号。示例只覆盖本地页和我们自己的公开站（[apizero.cn](https://apizero.cn/)）。
2. 守法律、站点条款、`robots.txt`、频率限制。
3. 不绕过登录、验证码、付费墙，不冒充别人。
4. 不采集没权处理的个人 / 机密 / 版权数据。
5. 下单、删除这类动作放测试环境，并要人确认。
6. profile、Cookie、截图别提交到 git。

安全问题按 [SECURITY.md](SECURITY.md) 私下报。

## 许可证

自定义 **source-available、非商业许可**，不是 OSI 开源许可。个人学习和合法非盈利用途要满足 [LICENSE](LICENSE) 全部条款。商用、付费再分发、当付费产品核心，先拿书面授权。

## 致谢

- [DrissionPage](https://github.com/g1879/DrissionPage)
- [Camoufox](https://github.com/daijro/camoufox)
- [Lightpanda](https://lightpanda.io)
- [ddddocr](https://github.com/sml2h3/ddddocr)
- [tract](https://github.com/sonos/tract)

[极数本源](https://apizero.cn)
