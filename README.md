# drission

用 Rust 控制本机 Chrome，对你自己的网站做自动化测试：打开页面、填写、点击、核对结果。

同一仓库提供命令行 `drs`，给终端脚本和 Cursor / Codex 用。当前版本 **0.6.6**，库和 `drs` 使用同一版本号。

[![crates.io](https://img.shields.io/crates/v/drission.svg)](https://crates.io/crates/drission)
[![docs.rs](https://docs.rs/drission/badge.svg)](https://docs.rs/drission)
[![Rust](https://img.shields.io/badge/Rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-blue.svg)](#环境)
[![License](https://img.shields.io/badge/license-source--available-lightgrey.svg)](LICENSE)

**简体中文** · [English](README.en.md) · [docs.rs](https://docs.rs/drission/0.6.6) · [CHANGELOG](CHANGELOG.md)

## 安装

```bash
cargo add drission
cargo install drission-cli --bin drs
```

`Cargo.toml` 里同时加上 `tokio`：

```toml
[dependencies]
drission = "0.6"
tokio = { version = "1", features = ["full"] }
```

没有 Rust 工具链时，从 [GitHub Releases](https://github.com/MageGojo/drission-rs/releases/latest) 或 [GitCode](https://gitcode.com/Roufsi/drission-rs) 下载对应平台的 `drs`。`install/` 里的脚本默认安装最新版。

默认会使用本机 Chrome，也识别 Edge、Brave、Chromium。

## 使用示例

下面以 [apizero.cn](https://apizero.cn/) 为例。测试你自己的网站时，把地址换成测试环境。

### 打开页面，读标题

```rust
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.new_tab(Some("https://apizero.cn/")).await?;
    println!("{}", tab.title().await?);
    browser.quit().await?;
    Ok(())
}
```

要看见窗口，把 `headless(true)` 改成 `headless(false)`。

### 填写、点击、核对结果

这一段和仓库里的本地测试页 `examples/pages/self_check.html` 相同：填入名称，点击保存，确认页面写出结果。

```rust
const PAGE: &str = r#"
<!doctype html>
<meta charset="utf-8">
<title>本地自测页</title>
<label>名称 <input id="name"></label>
<button id="save" type="button">保存</button>
<p id="out"></p>
<script>
document.getElementById("save").onclick = function () {
  document.getElementById("out").textContent =
    "已保存 " + document.getElementById("name").value;
};
</script>
"#;

tab.set_content(PAGE).await?;
tab.input("#name", "极数本源").await?;
tab.click("#save").await?;
println!("{}", tab.ele_text("#out").await?.unwrap_or_default());
// 已保存 极数本源
```

### 跑仓库里的完整检查

```bash
cargo run --example own_site
```

这条命令依次做完三件事：

1. 在本地页填入「极数本源」，确认出现「已保存」。
2. 打开 `https://apizero.cn/`，确认标题。
3. 点击「文档」，确认进入 `/aidocs`，然后关闭浏览器。

需要看见窗口时：

```bash
HL=0 cargo run --example own_site
```

### 在终端里操作

`drs` 在本机启动一个后台进程，浏览器由它保管。后续命令都发给这个进程。

```bash
drs ensure-serve --backend cdp --headless
drs --json tab open https://apizero.cn/
drs --json snapshot
drs --json click "ref:e7"
drs --json stop
```

`snapshot` 返回页面大纲，每个可操作的元素带有 `ref=e1` 这样的编号。点击时使用当次返回的编号。`--json` 便于脚本读取。

`stop` 会结束后台进程和浏览器。登录状态写在磁盘上的 profile 里，下次 `ensure-serve` 仍然可用。

监听自己站点的接口、导出 PDF、切换测试账号：

```bash
drs listen start /api/ --xhr-only --method POST
drs --json network inspect --limit 20
drs --json pdf --out ./page.pdf
drs --json context use staging
drs --json tab open https://apizero.cn/ --context staging
drs --json profile list
```

### 按页面上的文字点击

适合按钮文字稳定、选择器经常改的测试页。

```rust
let page = tab.agent();
let obs = page.observe().await?;
println!("{}", obs.outline);

page.find("文档").click().await?;
page.wait_for("API").await?;
```

### 多个测试账号

同名 context 会复用。关闭 context 后，其中的 Cookie 一并清除。

```rust
let ctx = browser.context("staging").await?;
let tab = ctx.new_tab(Some("https://apizero.cn/")).await?;
```

登录后若希望下次测试接着用，把 profile 放到磁盘：

```rust
let profiles = ProfileManager::open("profiles")?;
let lease = profiles.acquire("tester").await?;
let browser = lease.launch().await?;
```

目录为 `profiles/tester/chrome`。不要把这个目录提交到 git。

### 查看页面发出的请求

用于核对你自己的接口是否按预期发出。

```rust
let listen = tab.network().filter("/api/").method("POST").listen().await?;
```

### 接到 Cursor / Codex

```bash
drs setup --dry-run
drs setup
```

`setup` 会在现有 MCP 配置里增加 `drs`，不覆盖其他服务。默认可以开页、读取大纲、点击、截图和查看网络。PDF、录制和 Context 需要把 `DRS_MCP_TOOLS` 设为 `browser` 或 `all`，然后重新加载 MCP。

命令怎么选，见 [`docs/产品级CLI.md`](docs/产品级CLI.md)。字段说明见 [`docs/CLI.md`](docs/CLI.md)。

## 其他运行方式

服务器上没有桌面时，库可以改用 [Lightpanda](https://lightpanda.io)。接口不变，`drs` 仍然使用本机 Chrome。

```rust
let browser = Browser::launch(BrowserOptions::new().lightpanda()).await?;
```

说明见 [Lightpanda.md](https://github.com/MageGojo/drission-rs/blob/main/docs/Lightpanda.md)。示例：`cargo run --example lightpanda_demo`。

Firefox 后端需要单独打开：

```toml
drission = { version = "0.6", default-features = false, features = ["camoufox"] }
```

| Feature | 作用 | 默认 |
|---|---|---|
| `cdp` | Chrome / Edge / Brave / Chromium / Electron | 开 |
| `camoufox` | Camoufox / Firefox | 关 |
| `signer` | 内嵌 QuickJS | 关 |

两端能力对照见 [后端能力矩阵](https://github.com/MageGojo/drission-rs/blob/main/docs/后端能力矩阵.md)。

## 环境

| | |
|---|---|
| Rust | 1.85+，edition 2024 |
| 系统 | macOS / Linux / Windows |
| 浏览器 | 优先使用本机 Google Chrome，也识别 Edge、Brave、Chromium、Electron |
| 无桌面 | `headless(true)`。库在 macOS / Linux 上可改用 Lightpanda |

## 文档

- [源码与示例](https://github.com/MageGojo/drission-rs)
- [docs.rs/drission/0.6.6](https://docs.rs/drission/0.6.6)
- [示例索引](examples/README.md)
- [drs 命令](https://github.com/MageGojo/drission-rs/blob/main/docs/CLI.md)
- [更新记录](CHANGELOG.md)

`docs/` 在 GitHub 上，不打进 crates.io 的包。

## 使用范围

请在你有权操作的系统上使用，例如自己的网站、本地页面，以及书面授权的测试环境。使用时遵守法律、站点条款和访问频率。

不要用本库绕过登录、验证码或付费墙，不要冒充他人，不要采集无权处理的数据。下单、删除等会改动数据的操作，放在测试环境中，并由人确认后再执行。Cookie、profile 和截图不要提交到 git。

安全问题请按 [SECURITY.md](SECURITY.md) 私下报告。许可范围以 [LICENSE](LICENSE) 为准。

## 许可证

本项目是 source-available，供个人学习和合法的非盈利用途使用，不是 OSI 开源许可。商业使用、付费再分发，或作为付费产品的核心，需要事先取得书面授权。

## 致谢

- [DrissionPage](https://github.com/g1879/DrissionPage)
- [Camoufox](https://github.com/daijro/camoufox)
- [Lightpanda](https://lightpanda.io)
- [ddddocr](https://github.com/sml2h3/ddddocr)
- [tract](https://github.com/sonos/tract)

[极数本源](https://apizero.cn)
