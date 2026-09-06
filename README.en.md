# drission

Drive a local Chrome from Rust over CDP. The same repo ships `drs`, a CLI / MCP binary for scripts and Cursor / Codex.

[![crates.io](https://img.shields.io/crates/v/drission.svg)](https://crates.io/crates/drission)
[![docs.rs](https://docs.rs/drission/badge.svg)](https://docs.rs/drission)
[![Rust](https://img.shields.io/badge/Rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-blue.svg)](#compatibility)
[![License](https://img.shields.io/badge/license-source--available-lightgrey.svg)](LICENSE)

[简体中文](README.md) · **English** · [docs.rs](https://docs.rs/drission/0.6.3) · [CHANGELOG](CHANGELOG.md)

> Use this only on systems you own or are written-authorized to test. Do not bypass login, CAPTCHAs, or paywalls. See [Responsible use](#responsible-use) and [LICENSE](LICENSE).

## This release

**0.6.3**. The `drission` library and the `drs` binary (`drission-cli`) share that number.

The library can switch to [Lightpanda](https://lightpanda.io): same `Browser` / `Tab` API, change the launch for simple server-side listen jobs. Keep Chrome on a desktop when you need a window or a hard anti-bot site. `drs` is for local Agent / MCP and does **not** speak Lightpanda.

```bash
cargo add drission
cargo install drission-cli --bin drs
```

`drs --version` prints `drs 0.6.3`. No Rust toolchain: download a `drs` archive from [GitHub Releases](https://github.com/MageGojo/drission-rs/releases/latest) (GitCode mirror available). Scripts under `install/` fetch Latest.

## 1. Library

```toml
[dependencies]
drission = "0.6"
tokio = { version = "1", features = ["full"] }
```

Default feature is `cdp`. It looks for a local Chrome / Edge / Brave / Chromium.

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

On a server, same API, switch to Lightpanda (Zig headless, still CDP). Missing binary is fetched from GitHub nightly. No native Windows build.

```rust
let browser = Browser::launch(BrowserOptions::new().lightpanda()).await?;
```

Keep Chrome for a visible window or a tight anti-bot site. Details: [Lightpanda.md](https://github.com/MageGojo/drission-rs/blob/main/docs/Lightpanda.md).

Three extras (CDP):

```rust
// Isolate cookies / proxy / UA by name. Same name is reused until you close the context.
let ctx = browser.context("account_001").await?;
let tab = ctx.new_tab(Some("https://example.com")).await?;

// Listen, block, mock, rewrite, HAR. tab.listen() / intercept() still work.
let listen = tab.network().filter("/api/").method("POST").listen().await?;

// On-disk login. Lives in profiles/<id>/chrome across processes.
let profiles = ProfileManager::open("profiles")?;
let lease = profiles.acquire("user_001").await?;
let browser = lease.launch().await?;
```

Agent pages use the accessibility outline, not the full HTML:

```rust
let page = tab.agent();
let obs = page.observe().await?;
page.find("登录按钮").click().await?;
page.type_text(Locator::placeholder("用户名"), "alice").await?;
page.wait_for("Dashboard").await?;
tab.wait().element("#dash").visible().timeout(Duration::from_secs(10)).await?;
```

API: [docs.rs/drission](https://docs.rs/drission/0.6.3). In-repo: `cargo run --example cdp_demo`, `cargo run --example lightpanda_demo`, `cargo run --example cdp_context`, `cargo run --example cdp_agent`.

## 2. `drs`

`drs` starts a local daemon that owns the browser (local Chrome by default). Later commands talk to that process. Use `--json` for scripts. How to pick a command: [`docs/产品级CLI.md`](docs/产品级CLI.md). `drs` does not speak Lightpanda; use `.lightpanda()` in the library on servers.

```bash
drs ensure-serve --backend cdp --headless
drs --json tab open https://example.com
drs --json snapshot          # outline + ref=e1 + Markdown
drs --json click "ref:e1"
drs --json stop              # kills daemon + browser. Cookies stay on disk
```

Also:

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

`open` still equals `tab open`. `context`, HAR replay, `pdf`, and `record` are CDP-only.

Cursor / Codex:

```bash
drs setup --dry-run
drs setup
```

Merges a `drs` MCP entry; does not wipe other servers. Default `DRS_MCP_TOOLS=core` (open, snapshot, click, screenshot, plus semantic `browser_network` / `browser_console` / `browser_download`). Set `browser` or `all` for PDF / inspect / Context / legacy `network_*`, then restart MCP.

How to pick a command: [`docs/产品级CLI.md`](https://github.com/MageGojo/drission-rs/blob/main/docs/产品级CLI.md). Field details: [`docs/CLI.md`](https://github.com/MageGojo/drission-rs/blob/main/docs/CLI.md) (`docs/` is not in the crates.io crate).

## Features

| Feature | What | Default |
|---|---|---|
| `cdp` | Chrome / Edge / Brave / Chromium / Electron; same API via `.lightpanda()` | on |
| `camoufox` | Camoufox / Firefox (Juggler) | off |
| `ocr` | offline OCR (`tract`) | off |
| `slider` | gap distance for authorized slider tests | off |
| `signer` | embedded QuickJS | off |
| `impersonate` | HTTP TLS fingerprinting; needs CMake | off |

```toml
drission = { version = "0.6", features = ["ocr"] }
# drission = { version = "0.6", default-features = false, features = ["camoufox"] }
```

## Compatibility

| | |
|---|---|
| Rust | 1.85+, edition 2024 |
| OS | macOS / Linux / Windows |
| Browser | Google Chrome first; also Edge, Brave, Chromium, Electron |
| Server headless | library: `BrowserOptions::new().lightpanda()` (macOS / Linux; always headless) |
| No desktop | Chrome `headless` |

## Docs (GitHub; not packed on crates.io)

- https://github.com/MageGojo/drission-rs
- https://docs.rs/drission/0.6.3
- [CLI.md](https://github.com/MageGojo/drission-rs/blob/main/docs/CLI.md)
- [Lightpanda.md](https://github.com/MageGojo/drission-rs/blob/main/docs/Lightpanda.md)
- [CHANGELOG](CHANGELOG.md)

## Responsible use

1. Only systems and accounts you are allowed to use.
2. Follow the law, site terms, `robots.txt`, rate limits.
3. Do not bypass login, CAPTCHAs, paywalls, or impersonate people.
4. Do not collect personal / secret / copyrighted data you cannot process.
5. Keep purchases and deletes in a test env with a human in the loop.
6. Do not commit profiles, cookies, or screenshots.

OCR, image helpers, and request intercept do not authorize access to third-party sites.

Report security issues via [SECURITY.md](SECURITY.md).

## License

Custom **source-available, non-commercial**. Not OSI open source. See [LICENSE](LICENSE). Commercial use or paid redistribution needs written permission.

## Acknowledgements

- [DrissionPage](https://github.com/g1879/DrissionPage)
- [Camoufox](https://github.com/daijro/camoufox)
- [Lightpanda](https://lightpanda.io)
- [ddddocr](https://github.com/sml2h3/ddddocr)
- [tract](https://github.com/sonos/tract)

[API Zero](https://apizero.cn)
