# drission

A Rust browser automation library with the `drs` CLI and local MCP server for scripts and AI clients.

[![crates.io](https://img.shields.io/crates/v/drission.svg)](https://crates.io/crates/drission)
[![docs.rs](https://docs.rs/drission/badge.svg)](https://docs.rs/drission)
[![Rust](https://img.shields.io/badge/Rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-blue.svg)](#compatibility)
[![License](https://img.shields.io/badge/license-source--available-lightgrey.svg)](LICENSE)

[简体中文](README.md) · **English** · [API docs](https://docs.rs/drission) · [Changelog](CHANGELOG.md)

`drission` provides an asynchronous, `tokio`-based browser control API. Its default Chrome DevTools
Protocol backend supports Chrome, Edge, Brave, Chromium, and Electron. The `drs` binary exposes the
same foundation through a command-line interface, a JSON protocol, and a local MCP server for test
tools, data-processing scripts, and AI coding clients.

> [!IMPORTANT]
> Use this project only on systems you own or are explicitly authorized to test. Follow applicable
> laws, site terms, access controls, `robots.txt`, and rate limits. Do not use it to bypass
> authentication or security controls, access accounts without authorization, collect protected
> data, or conduct attacks or harassment. See [Responsible use](#responsible-use) and [LICENSE](LICENSE).

![drs CLI, MCP, and local Chrome CDP workflow](docs/images/drs-readme-hero.png)

## Choose an interface

| Interface | Best for | Get started |
|---|---|---|
| `drission` | Drive a browser from Rust | `cargo add drission` (crates.io 0.6.0) |
| `drs` CLI | Terminal / scripts, JSON output | `cargo install drission-cli --bin drs` |
| `drs` MCP | Cursor, Codex, other local MCP clients | Install `drs`, then `drs setup` |

## Quick start

### Rust library

Current version is **0.6.0**. Chromium/CDP is the default. crates.io is in sync:

```bash
cargo add drission
```

```toml
[dependencies]
drission = "0.6"
tokio = { version = "1", features = ["full"] }
```

```rust
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let ctx = browser.context("account_001").await?;
    let tab = ctx.new_tab(Some("https://example.com")).await?;

    println!("title: {:?}", tab.title().await?);
    println!("h1: {:?}", tab.ele_text("h1").await?);

    browser.quit().await?;
    Ok(())
}
```

Run the minimal repository example:

```bash
cargo run --example cdp_demo
```

The default backend detects a locally installed Chromium-family browser. See
[Chrome download](docs/Chrome自动下载.md) and [server deployment](docs/服务器部署.md) for browser
selection, managed downloads, and headless environments.

### `drs` CLI and MCP

`drs` is **0.3.3**. Install from crates.io:

```bash
cargo install drission-cli --bin drs
```

Or grab a prebuilt binary from [GitHub Releases](https://github.com/MageGojo/drission-rs/releases)
or [GitCode Releases](https://gitcode.com/Roufsi/drission-rs/releases). Scripts live under
[`install/`](install/); read them before running.

```bash
drs ensure-serve --backend cdp --headless
drs --json open https://example.com
drs --json snapshot
drs --json context use account_001
drs --json open https://example.com --context account_001
drs listen start /api/ --xhr-only --method POST
drs --json network mock /api/user --body '{"ok":true}'
drs --json profile list
```

Preview MCP configuration changes before applying them:

```bash
drs setup --dry-run
drs setup
```

`drs setup` merges the Cursor project configuration and Codex user configuration without replacing
other MCP servers. By default, MCP connects to a persistent local browser process so tabs and browser
configuration can survive MCP process restarts. See the [CLI / MCP guide](docs/CLI.md) and
[persistent browser guide](docs/mcp-持久浏览器.md) for commands, JSON responses, tools, and manual setup.

## What 0.6.0 adds

Library:

- `browser.context("account_001")` — isolate cookies / proxy / UA / timezone by name. Same name reuses the context; closing the context disposes it.
- `ProfileManager` — on-disk `profiles/<id>/{manifest.json,chrome/}` so logins survive process restarts.
- `tab.network().filter("/api/").method("POST")` — listen / block / mock / rewrite / record. Old `listen` / `intercept` stay.
- `ChromiumPool::acquire()` — take a leased browser, then `context` / `new_tab`.

`drs`:

| Command | What it does |
|---|---|
| `snapshot` | Current page: outline + `ref=eN` + Markdown |
| `context list\|use\|close` | Named contexts; `open --context` opens into one |
| `network block\|mock\|rewrite\|record\|stop` | Block, fake responses, rewrite, HAR |
| `listen start … --method POST` | Existing listener, now with method filter |
| `profile list\|show` | Disk profiles; `serve --profile <id>` uses that chrome dir |

MCP defaults to `DRS_MCP_TOOLS=core` (open / snapshot / click / screenshot). Context and `network_*` live under `browser` or `all`.

See [CLI.md](docs/CLI.md) and [runtime layering](docs/运行时分层.md).

## Core capabilities

- Async browser control: navigation, locators, click, type, keys, scroll, upload, iframe, Shadow DOM, tabs.
- Context / Profile: in-process account isolation, disk profiles for logins.
- Network: XHR listen, intercept, mock, rewrite, HAR; console and WebSocket too.
- CLI + JSONL daemon + stdio MCP. Prefer `snapshot` over dumping full HTML.
- Pools, proxy health, retries, checkpoints; identity leases / cooldown / ledger unchanged.
- OCR and slider helpers are optional features. Use them only on systems you own or are allowed to test.

## Features

| Feature | Contents | Default |
|---|---|---|
| `cdp` | CDP backend for Chrome, Edge, Brave, Chromium, and Electron | Yes |
| `camoufox` | Camoufox / Firefox Juggler compatibility backend | No |
| `ocr` | Offline text recognition using `tract` | No |
| `slider` | Image-position analysis for authorized tests; enables `camoufox` | No |
| `signer` | Embedded QuickJS for local JavaScript compatibility testing | No |
| `impersonate` | HTTP client compatibility profiles; requires CMake and a C toolchain | No |

Example configurations:

```toml
# Default CDP plus OCR
drission = { version = "0.6", features = ["ocr"] }

# Camoufox only
# drission = { version = "0.6", default-features = false, features = ["camoufox"] }
```

Refer to [Cargo.toml](Cargo.toml) and the [API documentation](https://docs.rs/drission) for the
authoritative feature dependency and build requirements.

## Compatibility

| Item | Supported range |
|---|---|
| Rust | 1.85 or newer, edition 2024 |
| Operating systems | macOS, Linux, Windows |
| Default backend | Chromium / CDP |
| Default browser selection | Google Chrome first; Edge, Brave, Chromium, and Electron are also supported |
| Optional backend | Camoufox / Firefox Juggler |

Use headless mode on systems without a desktop. See [server deployment](docs/服务器部署.md) for
container and Linux system dependencies.

## Documentation and examples

- [Documentation index](docs/README.md): architecture, deployment, network observation, pools, and API mapping.
- [CLI / MCP](docs/CLI.md): `drs` commands, JSON protocol, MCP tools, and configuration.
- [Examples](examples/README.md): runnable examples and commands grouped by capability.
- [DrissionPage API mapping](docs/API映射.md): corresponding Rust APIs for Python users.
- [API reference](https://docs.rs/drission): types, methods, and feature markers.
- [Changelog](CHANGELOG.md): release features and compatibility changes.
- [Contributing guide](CONTRIBUTING.md) and [security policy](SECURITY.md).

## Responsible use

Browser automation can process login state, personal data, copyrighted material, or operations with
real business impact. Before deployment:

1. Access only systems, accounts, and data that you own or have explicit written authorization to use.
2. Follow applicable laws, contracts, platform terms, `robots.txt`, access controls, and rate limits.
3. Do not bypass paywalls, authentication, CAPTCHAs, or other security controls; do not evade bans or impersonate others.
4. Do not collect personal, confidential, copyrighted, or restricted data without the right to process it; minimize collected data.
5. Put write, publish, purchase, and delete operations behind isolated tests, least privilege, and human confirmation.
6. Protect browser profiles, cookies, logs, screenshots, and exports; do not commit sensitive data to version control.

Optional OCR, image analysis, browser configuration, and network observation features do not grant
permission to access any third-party system. Third-party names and trademarks belong to their
respective owners; their mention does not imply endorsement, affiliation, or warranty.

This section describes project usage boundaries. It is not legal advice and does not replace an
assessment for your jurisdiction and use case. Report vulnerabilities privately through
[SECURITY.md](SECURITY.md).

## License

This project uses a custom **source-available, non-commercial license**. It is not an OSI-approved
open-source license. Personal learning and lawful nonprofit use must comply with every term in
[LICENSE](LICENSE). Commercial use, paid redistribution, or using the project as a core component of
a paid product or service requires prior written authorization from the copyright holder.

Users remain responsible for evaluating their use case. The license and disclaimers do not exclude
liability that cannot be excluded under applicable law.

## Acknowledgements

- [DrissionPage](https://github.com/g1879/DrissionPage): API design reference.
- [Camoufox](https://github.com/daijro/camoufox): optional browser backend.
- [ddddocr](https://github.com/sml2h3/ddddocr): OCR model source.
- [tract](https://github.com/sonos/tract): Rust ONNX inference engine.

Maintained by [API Zero](https://apizero.cn).
