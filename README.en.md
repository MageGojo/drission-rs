# drission

Control a local Chrome from Rust and test a site you operate: open a page, fill a form, click, and check the result.

The same repository ships `drs`, a command-line tool for scripts and for Cursor / Codex. Current release: **0.6.6**. The library and `drs` share that version.

[![crates.io](https://img.shields.io/crates/v/drission.svg)](https://crates.io/crates/drission)
[![docs.rs](https://docs.rs/drission/badge.svg)](https://docs.rs/drission)
[![Rust](https://img.shields.io/badge/Rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-blue.svg)](#environment)
[![License](https://img.shields.io/badge/license-source--available-lightgrey.svg)](LICENSE)

[简体中文](README.md) · **English** · [docs.rs](https://docs.rs/drission/0.6.6) · [CHANGELOG](CHANGELOG.md)

## Install

```bash
cargo add drission
cargo install drission-cli --bin drs
```

```toml
[dependencies]
drission = "0.6"
tokio = { version = "1", features = ["full"] }
```

Without a Rust toolchain, download `drs` from [GitHub Releases](https://github.com/MageGojo/drission-rs/releases/latest) or the [GitCode](https://gitcode.com/Roufsi/drission-rs) mirror. Scripts in `install/` fetch the latest build.

The default browser is local Google Chrome. Edge, Brave, and Chromium are also recognized.

## Examples

The samples below use [apizero.cn](https://apizero.cn/). For your own site, replace the URL with your test environment.

### Open a page and read the title

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

Use `headless(false)` when you want to watch the window.

### Type, click, and check the result

This is the same page as `examples/pages/self_check.html`: type a name, click Save, and read the text the page writes back.

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

### Run the full check in the repository

```bash
cargo run --example own_site
```

The example does three things:

1. Fills the local page with「极数本源」and checks that the page shows「已保存」.
2. Opens `https://apizero.cn/` and checks the title.
3. Clicks「文档」, checks that the URL is `/aidocs`, then quits the browser.

Show the window:

```bash
HL=0 cargo run --example own_site
```

### Do the same from the terminal

`drs` starts a local background process and keeps the browser there. Later commands talk to that process.

```bash
drs ensure-serve --backend cdp --headless
drs --json tab open https://apizero.cn/
drs --json snapshot
drs --json click "ref:e7"
drs --json stop
```

`snapshot` returns an outline. Each control has a `ref` such as `e1`. Click the ref from that snapshot. `--json` is for scripts.

`stop` ends the background process and the browser. Cookies stay in the on-disk profile and are available on the next `ensure-serve`.

Listen to your own API, save a PDF, or switch a test account:

```bash
drs listen start /api/ --xhr-only --method POST
drs --json network inspect --limit 20
drs --json pdf --out ./page.pdf
drs --json context use staging
drs --json tab open https://apizero.cn/ --context staging
drs --json profile list
```

### Click by the text on the page

Useful when the label is stable and the selector is not.

```rust
let page = tab.agent();
let obs = page.observe().await?;
println!("{}", obs.outline);

page.find("文档").click().await?;
page.wait_for("API").await?;
```

### Separate test accounts

A context name is reused until you close it. Closing it drops that context's cookies.

```rust
let ctx = browser.context("staging").await?;
let tab = ctx.new_tab(Some("https://apizero.cn/")).await?;
```

To keep a login for the next run, store the profile on disk:

```rust
let profiles = ProfileManager::open("profiles")?;
let lease = profiles.acquire("tester").await?;
let browser = lease.launch().await?;
```

The directory is `profiles/tester/chrome`. Do not commit it.

### Watch requests the page sends

Use this to confirm your own API calls.

```rust
let listen = tab.network().filter("/api/").method("POST").listen().await?;
```

### Connect Cursor / Codex

```bash
drs setup --dry-run
drs setup
```

`setup` adds a `drs` entry to the existing MCP config and leaves other servers in place. The default tool set can open pages, read the outline, click, take screenshots, and inspect the network. For PDF, recording, and contexts, set `DRS_MCP_TOOLS` to `browser` or `all`, then reload MCP.

How to choose a command: [`docs/产品级CLI.md`](docs/产品级CLI.md). Fields: [`docs/CLI.md`](docs/CLI.md).

## Other engines

On a machine without a desktop, the library can use [Lightpanda](https://lightpanda.io). The API stays the same. `drs` still uses local Chrome.

```rust
let browser = Browser::launch(BrowserOptions::new().lightpanda()).await?;
```

See [Lightpanda.md](https://github.com/MageGojo/drission-rs/blob/main/docs/Lightpanda.md). Example: `cargo run --example lightpanda_demo`.

The Firefox backend is opt-in:

```toml
drission = { version = "0.6", default-features = false, features = ["camoufox"] }
```

| Feature | Role | Default |
|---|---|---|
| `cdp` | Chrome / Edge / Brave / Chromium / Electron | on |
| `camoufox` | Camoufox / Firefox | off |
| `signer` | embedded QuickJS | off |

Backend differences: [capability matrix](https://github.com/MageGojo/drission-rs/blob/main/docs/后端能力矩阵.md).

## Environment

| | |
|---|---|
| Rust | 1.85+, edition 2024 |
| OS | macOS / Linux / Windows |
| Browser | Google Chrome first; also Edge, Brave, Chromium, Electron |
| No desktop | `headless(true)`. The library can use Lightpanda on macOS and Linux |

## Docs

- [Source and examples](https://github.com/MageGojo/drission-rs)
- [docs.rs/drission/0.6.6](https://docs.rs/drission/0.6.6)
- [Example index](examples/README.md)
- [`drs` commands](https://github.com/MageGojo/drission-rs/blob/main/docs/CLI.md)
- [Changelog](CHANGELOG.md)

`docs/` lives on GitHub and is not packed into the crates.io crate.

## Scope of use

Use drission on systems you are allowed to operate: your own site, a local page, or a test environment you have written permission to use. Follow the law, the site terms, and rate limits.

Do not use it to bypass login, CAPTCHAs, or paywalls, to impersonate someone, or to collect data you have no right to process. Keep purchases, deletions, and other writes in a test environment, and have a person confirm them. Do not commit cookies, profiles, or screenshots.

Report security issues in private, as described in [SECURITY.md](SECURITY.md). The license terms are in [LICENSE](LICENSE).

## License

Source-available, for personal learning and lawful non-commercial use. This is not an OSI open-source license. Commercial use, paid redistribution, or using the project as the core of a paid product requires written permission.

## Acknowledgements

- [DrissionPage](https://github.com/g1879/DrissionPage)
- [Camoufox](https://github.com/daijro/camoufox)
- [Lightpanda](https://lightpanda.io)
- [ddddocr](https://github.com/sml2h3/ddddocr)
- [tract](https://github.com/sonos/tract)

[API Zero](https://apizero.cn)
