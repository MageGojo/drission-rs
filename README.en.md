# drission · Anti-Detect Browser Automation in Rust + Built-in Captcha Solving (OCR / Slider-Gap)

[![crates.io](https://img.shields.io/crates/v/drission.svg)](https://crates.io/crates/drission)
[![docs.rs](https://docs.rs/drission/badge.svg)](https://docs.rs/drission)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-blue.svg)](#-supported-platforms--browsers)
[![GitHub](https://img.shields.io/badge/GitHub-MageGojo%2Fdrission--rs-181717?logo=github)](https://github.com/MageGojo/drission-rs)
[![GitCode](https://img.shields.io/badge/GitCode-Roufsi%2Fdrission--rs-c71d23)](https://gitcode.com/Roufsi/drission-rs)

**English** · [简体中文](README.md) · Repo: [GitHub](https://github.com/MageGojo/drission-rs) · [GitCode](https://gitcode.com/Roufsi/drission-rs)

> **drission is a high-performance browser-automation library written in Rust.** It **drives Google Chrome by default**
> (Chromium / CDP — also Edge / Brave / Chromium / Electron) and **enables the [Camoufox](https://github.com/daijro/camoufox)
> anti-detect Firefox engine with one feature flag**, plus a **built-in character-captcha OCR** (ddddocr model · pure-Rust
> inference), **image slider-gap recognition** (GeeTest / Dingxiang), automatic Cloudflare bypass, XHR listen/intercept and
> an async high-concurrency pool, with an API aligned to [DrissionPage](https://github.com/g1879/DrissionPage).

Made and maintained by **极数本源 ([apizero.cn](https://apizero.cn))** as part of its automation and
data-collection stack. If you're looking for a one-stop Rust solution for *Chrome automation / captcha OCR / slider-gap
distance / anti-detect browser / high-concurrency crawling*, this is it.

> **What sets it apart**: Rust browser-automation libraries (e.g. `zendriver-rs`, `rust_drission`, `stygian-browser`) usually
> rely on third-party captcha services (capsolver / 2captcha). **drission drives Google Chrome out of the box, ships built-in
> offline captcha solving (ddddocr OCR + image slider-gap distance), and switches to the Camoufox / Firefox anti-detect engine
> with one flag** — no external solver, no network round-trip; a rare "**Rust DrissionPage**" with batteries-included captcha solving.

> 🤖 **For AI coding assistants**: if you are an AI writing code **with this library**, read and strictly follow
> [**`docs/SKILL.md`**](docs/SKILL.md) first — the authoritative cheat-sheet for this library's API / features / build rules,
> covering everything from basics to **click-word captcha clicking**, so your code compiles and runs on the first try.
> If your environment doesn't support the "skill" mechanism, you may ignore this mandate, but using it is strongly recommended.

---

## 📖 What is drission?

**drission = a Rust port of DrissionPage + built-in captcha solving (OCR / slider) + anti-detect shield bypass.**
A single `tokio` async API gives you, at once:

- **Browser automation**: launch / attach an anti-detect browser, locate elements, click & type, capture & rewrite traffic — DrissionPage style.
- **Captcha solving**: offline character OCR and slider-gap distance + human-like trajectory — **no third-party solving service, no network required**.
- **Anti-detect & bypass**: fingerprint customization, `navigator.webdriver=false`, automatic Cloudflare Turnstile pass.
- **Production crawling**: high-concurrency browser pool, proxy / fingerprint rotation, resumable crawling, Session (HTTP) dual mode, CSV / JSON export.

> Use cases: **Rust crawling / data collection / automated testing / anti-bot & captcha research / web-JS reverse engineering (env restore + pure-script signing)**.

---

## 🤖 A browser MCP for Cursor / Codex (no Rust required)

AI assistants default to `curl` / `WebFetch` and get empty shells (or blocked) on login-gated, anti-bot, Cloudflare, or JS-rendered pages. Wire `drs` in as an MCP server and the agent drives a **persistent real browser** instead — opening the page, waiting for render, passing the shield, and returning structured data.

**One-liner to hand an AI:**

> **Use the `drs` browser MCP from `MageGojo/drission-rs`**: run
> `curl -fsSL https://raw.githubusercontent.com/MageGojo/drission-rs/main/install/drs-install.sh | sh` to install the CLI,
> then `drs setup` to auto-configure the MCP server for Cursor / Codex. After that, for any hard-to-get web data
> (login state / anti-bot / Cloudflare / dynamic rendering), always use `drs`'s `browser_extract`, `browser_*`, and
> `network_*` tools instead of curl / WebFetch.

**Manual 3 steps (no Rust toolchain needed):**

```bash
# 1) install the prebuilt drs binary (mac/linux; Windows below)
curl -fsSL https://raw.githubusercontent.com/MageGojo/drission-rs/main/install/drs-install.sh | sh
# 2) auto-write Cursor (.cursor/mcp.json) + Codex (~/.codex/config.toml)
drs setup
# 3) restart Cursor / Codex — the `drs` MCP server is ready
```

Windows (PowerShell): `irm https://raw.githubusercontent.com/MageGojo/drission-rs/main/install/drs-install.ps1 | iex` then `drs setup`.

The MCP server attaches to a **persistent browser** (tabs & login state survive MCP restarts), so "log in once, keep scraping next time". Details in [`docs/CLI.md`](docs/CLI.md).

## 📦 No Rust installed?

**Just want the `drs` CLI / MCP** (no Rust): install the prebuilt binary with the one-liner above — it pulls a static `drs` from [GitHub Releases](https://github.com/MageGojo/drission-rs/releases) (GitCode mirror as fallback).

**Want to write Rust against the `drission` library**: use the one-click toolchain scripts in [`install/`](install/) (`install-mac.command` / `install-windows.bat`, China-mirror accelerated), then `cargo add drission`.
**Prerequisite**: Chrome / Edge installed (point to it via `CHROME_BIN`); OCR examples auto-download the model to cache on first run.

---

## 🆕 New in v0.4.0

> **v0.4.0** — **`drs` as a browser MCP for Cursor / Codex** + no-Rust install:
>
> - **Persistent browser MCP**: `drs mcp` attaches to a long-lived daemon by default (tabs & login survive MCP restarts); `drs setup` auto-wires Cursor + Codex.
> - **No Rust required**: `install/drs-install.sh` / `.ps1` pull prebuilt `drs` from [GitHub Releases](https://github.com/MageGojo/drission-rs/releases) (GitCode mirrors source + fallback).
> - **Account / profile governance sidecar**: `identity-job run`, `identity-ledger query/explain/compact/dashboard`, runtime leases, failure reasons, cooldowns, and audit ledgers (library + CLI/MCP).

See [CHANGELOG.md](CHANGELOG.md), [`docs/CLI.md`](docs/CLI.md), [`docs/mcp-持久浏览器.md`](docs/mcp-持久浏览器.md).

## 🆕 New in v0.3.2

> **v0.3.2** — standard browser features plus AI-facing runtime:
>
> - **`drs` CLI / MCP (AI Agent runtime)**: new workspace package `drission-cli`, binary `drs`. It supports `drs serve` as a local daemon, stable `drs --json` output, page observation/actions/network listen/screenshots/Cloudflare pass commands, and `drs mcp` as a stdio MCP server. Details: [`docs/CLI.md`](docs/CLI.md).
> - **Recorder → Rust codegen**: `tab.recorder()` records page operations and emits runnable DrissionPage-style Rust code, covering click/fill/check/select/key/hover/drag/iframe/multi-tab flows.
> - **Accessibility snapshots**: `tab.ax_tree()` / `ax_snapshot()` compress a page into a `role "name"` semantic tree for robust assertions or LLM context.
> - **Runtime fingerprint snapshot**: dump UA / platform / timezone / screen / WebGL / canvas signals to verify the browser persona actually changed.
> - **CDP standard-feature fill-in**: PDF / MHTML / `set_content` / HAR record + replay / `expose_function` / media, network and CPU emulation / mobile device presets / permissions / storage helpers / `wait().new_tab`. See [`docs/标配补齐.md`](docs/标配补齐.md) and [`docs/录制与无障碍.md`](docs/录制与无障碍.md).
>
> Full history in [CHANGELOG.md](CHANGELOG.md). The previous **v0.3.1** release focuses on **Windows real-machine click / bypass precision** and **headless anti-detect authenticity**:
>
> - **Windows high-DPI click alignment**: force `device-scale=1`, fixing the physical-pixel offset under 125% / 150% scaling that made Cloudflare Turnstile / click-word captchas "unclickable".
> - **Headless GPU adaptive**: real GPU → hardware ANGLE, no GPU (VM / RDP) → D3D11 WARP, so WebGL stays real (avoids the SwiftShader software-render tell).
> - **Consistent anti-detect identity**: when masking as Chrome UA, always backfill matching high-entropy Client Hints / `userAgentMetadata` (no empty `fullVersionList`, no Edge-brand contradiction).
> - **Cloudflare inline-Turnstile bypass**: 3-level locating (incl. closed shadow DOM) + token-presence pass criterion, supporting form-embedded Turnstile.
> - **CDP isolated-context cookie fix**; release-size optimization (`opt-level=z` + LTO + strip).
>
> Since **v0.3.0**: full dual-protocol API parity, Session TLS fingerprinting, per-browser fingerprints, and an AI coding skill doc.

- **CDP backend at full parity with Camoufox (same code, swap a feature to switch backend)**: adds iframe / Shadow DOM / action chains / console & WebSocket listening / screenshot & screencast / upload / dialogs / **env-dump `dump_env`** / **concurrency pool `ChromiumPool`** / **modifier hotkeys** (headless really executes Ctrl+A/C/V edit commands) / **Windows process-tree cleanup (Job Object)**.
- **Session browser TLS / JA3 / JA4 + HTTP2 fingerprint (`--features impersonate`)**: wear a **real browser handshake fingerprint** on the pure-HTTP dual mode (`wreq` + BoringSSL, `BrowserProfile::Chrome/Firefox/Safari/Edge`), so "pass the shield in the browser → continue over HTTP" isn't blocked by modern WAFs (Akamai / CF / DataDome) on the TLS fingerprint; Windows (incl. mingw cross-compile) verified to produce a real `.exe`.
- **Per-browser fingerprints `CdpFingerprint` / `CdpFingerprintPool`** (mirroring Camoufox's fingerprint pool): spin up N browsers each wearing a **coherent fingerprint** (UA / platform / languages / timezone / screen / hardware / WebGL / canvas·audio noise); same-OS variants stay authentic (Turnstile-friendly), cross-OS personas fully spoof.
- **🤖 AI coding skill [`docs/SKILL.md`](docs/SKILL.md) (AI must-read)**: an authoritative cheat-sheet for this library's API / features / build rules, from basics to **click-word captcha clicking**; the README top declares "AI developing with this library must follow this skill".
- **All examples are copy-runnable**: after the default backend flipped to cdp, fixed ~45 Camoufox / slider / ocr example header run-commands (Camoufox-family needs `--no-default-features --features camoufox`).

> Earlier `0.1.x` / `0.2.x` capabilities (default CDP / Google Chrome drive & auto-download, stable Windows + Chrome path detection, captcha OCR, image slider, **click-word captcha real bypass**, Session / `WebPage` dual mode, pure-script signing runner, Cloudflare bypass, proxy-pool health, login-state persistence, Shadow DOM, download manager) — see [CHANGELOG.md](CHANGELOG.md).

---

## ✨ Highlights

### 1. Built-in captcha OCR (character type, `feature = "ocr"`)

**Offline** recognition of letter / digit / distorted-and-merged captchas — **no third-party solving service,
no network required**: powered by [ddddocr](https://github.com/sml2h3/ddddocr) pretrained models running on the
**pure-Rust inference engine [tract](https://github.com/sonos/tract)** (no native onnxruntime, clean
cross-compilation). Pipeline: grayscale → aspect-resize to height 64 → normalize → CNN-LSTM → CTC decode.

```rust
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;
    tab.get("https://apizero.cn/login").await?;

    // One call: locate the captcha <img> → grab it → recognize (model auto-downloads on first use)
    let code = tab.ocr_image("xpath://form//button/img").await?;
    println!("captcha = {code}");                      // e.g. "P38W"
    Ok(())
}
```

> **End-to-end tested** (`examples/apizero_login`): opening [apizero.cn](https://apizero.cn)'s login page with
> this library → auto-filling the form → OCR-recognizing the captcha and submitting → clicking login; the site
> only returns "wrong account or password" rather than "wrong captcha", i.e. **the captcha was recognized
> correctly** (headed / headless: **5/5, 4/4** pass).

### 2. Image slider-gap distance recognition (`feature = "slider"`)

Computes "how far the piece must move" accurately + drags it there with a human-like trajectory — a
**vendor-agnostic** capability with built-in GeeTest / Dingxiang presets:

```rust
use drission::prelude::*;

# async fn demo(tab: &Tab) -> drission::Result<()> {
// GeeTest v4: three-image template matching, gap distance + closed-loop correction
let r = tab.solve_geetest_slide().await?;

// Dingxiang: cross-origin tainted puzzle (unreadable pixels) → screenshot + green-ring mask + color NCC + darkness/outline
let gap = tab.dingxiang_slide_gap(4).await?;   // gap displacement (CSS px) + confidence
println!("move {:.0}px, confidence {:.2}", gap.displace, gap.confidence);
# Ok(()) }
```

- **GeeTest v4**: three `canvas` images (bg / fullbg / slice) template matching, alignment error ≤1px, passes headless.
- **Dingxiang popup**: busy real photos + same-shape decoys + heavy darkening, solved with **color-content NCC +
  darkness gating + outline alignment**; offline-calibrated gap hits 6/6 (shipped as `GapMethod::ContentNcc`).
- Generic `SliderConfig` + `tab.slider_gap()` / `tab.solve_slider()`; switch vendors by swapping the config.

---

## 🧰 Also supports

- **Anti-detect / shield bypass**: `navigator.webdriver=false`, canvas / webgl / audio fingerprint customization,
  `block_webrtc`; **automatic Cloudflare bypass** (trusted Turnstile checkbox click).
- **Elements & interaction**: DrissionPage-style locators (`@id:` / `css:` / `xpath:` / `text:`), click / input /
  per-character human typing, action chains, drag, select / radio / checkbox form filling, file upload, iframe, JS dialogs.
- **Network**: XHR / Fetch **response-body capture**, **request interception/rewrite** (fulfill / abort / resume),
  WebSocket frame listening, console listening.
- **Multi-tab & high concurrency**: per-tab cookie isolation, `BrowserPool` (proxy / fingerprint rotation + retry +
  **resumable crawling from checkpoint**).
- **Driver + Session dual mode**: browser and pure-HTTP session modes with two-way cookie sharing (memory-friendly); Session can optionally wear a **browser TLS / JA3 / JA4 + HTTP2 fingerprint via `--features impersonate`** (`wreq` + BoringSSL), so "pass the shield in the browser → continue over HTTP" isn't blocked by modern WAFs on the TLS fingerprint.
- **Screenshots & recording**: element / full-page / region screenshots, viewport recording muxed to mp4.
- **Environment dumping ("env restore")**: collect real canvas / webgl / audio fingerprints + signature-sink
  location, export a `node`-runnable env-restore project in one click; with `signer`, compile it into a no-Node single binary.
- **Take over a browser**: `BrowserServer` exposes a WebSocket endpoint; `Browser::connect` attaches to a running browser.
- **Multiple backends**: **Chromium / CDP by default** (drive / attach Chrome / Edge / Brave / Chromium / Electron); `--features camoufox` adds the Camoufox / Firefox (Juggler) anti-detect backend with all high-level capabilities.

---

## 🆚 drission vs alternatives

| Dimension | **drission** (Rust) | DrissionPage (Python) | Playwright / Selenium |
|---|---|---|---|
| Language / runtime | Rust · `tokio` async · single binary | Python | multi-language |
| Default backend | ✅ Google Chrome (CDP), one flag to Camoufox | Chromium | many browsers |
| Built-in anti-detect engine | ✅ Camoufox (`--features camoufox`) | ⚠️ DIY hardening | ❌ easily detected |
| Built-in captcha OCR | ✅ offline, pure Rust | ❌ | ❌ |
| Slider-gap recognition | ✅ GeeTest / Dingxiang | ❌ | ❌ |
| Auto Cloudflare bypass | ✅ `pass_cloudflare()` | ⚠️ partial | ❌ |
| XHR listen / body capture | ✅ built-in | ✅ | ⚠️ manual |
| Concurrency pool + resume | ✅ `BrowserPool` built-in | ⚠️ DIY | ❌ |
| Backends | Chromium / CDP (default) + optional Camoufox | Chromium | many browsers |

> In short: **if you want "DrissionPage's ergonomics + Rust's performance + built-in solving & anti-detect", choose drission.**

---

## 📦 Install

```toml
[dependencies]
drission = "0.3"                                           # default = Chromium / CDP (Google Chrome)

# Want the Camoufox anti-detect engine + all high-level capabilities? disable default cdp, then enable camoufox:
# drission = { version = "0.3", default-features = false, features = ["camoufox", "ocr", "slider", "signer", "impersonate"] }
#
# Want default CDP plus OCR / signer only:
# drission = { version = "0.3", features = ["ocr", "signer"] }
```

| feature | capability | deps | default |
|---|---|---|---|
| `cdp` | Chromium / CDP backend (Chrome / Edge / Brave / Chromium / Electron) | std, no extra heavy deps | **on** |
| `camoufox` | Camoufox / Firefox (Juggler) anti-detect backend + all high-level capabilities | std, auto-downloads Camoufox | off |
| `ocr` | character captcha recognition (ddddocr + tract) | `image` + `tract-onnx` | off |
| `slider` | image slider-gap recognition (GeeTest / Dingxiang) | pure JS + std, pulls in `camoufox` | off |
| `signer` | pure-script signing runner (embedded QuickJS, no Node) | `rquickjs` | off |
| `impersonate` | **Session browser TLS / JA3 / JA4 + HTTP2 fingerprint** (dual-mode WAF bypass) | `wreq` + BoringSSL (needs `cmake`+`nasm`; Windows see below), pulls in `camoufox` | off |

---

## 🚀 Quick start

**Default backend = Google Chrome (CDP)** — no feature needed; auto-detects local Chrome (Windows includes registry / user-level installs):

```rust
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    // Auto-locate Google Chrome (CHROME_BIN/DRISSION_CHROME → install paths → Windows registry → PATH).
    // To pin a browser: ChromiumBrowser::launch_with("C:\\...\\chrome.exe", true)
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(true)).await?; // headless; for headed use launch_default()
    let tab = browser.new_tab(Some("https://example.com")).await?;

    println!("title = {:?}", tab.title().await?);
    println!("h1    = {:?}", tab.ele_text("h1").await?);

    browser.quit().await?;
    Ok(())
}
```

**Camoufox anti-detect engine** (`--no-default-features --features camoufox`) — auto-downloaded, with shield bypass / env-dump / pool / slider and all high-level capabilities:

```rust
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;

    tab.listen_start(&["api/data"]).await?;        // start listening first
    tab.get("https://example.com").await?;          // then navigate
    tab.ele("@id:kw").await?.input("drission").await?;

    let packet = tab.listen_wait().await?;          // capture the target XHR (with response body)
    println!("{}", packet.response.body);

    browser.quit().await?;
    Ok(())
}
```

Examples (Camoufox-based examples need a single-backend `--no-default-features` build):

```bash
cargo run --example cdp_demo                                  # default Chromium / CDP backend (Google Chrome)
cargo run --example quickstart    --no-default-features --features camoufox      # Camoufox minimal loop
cargo run --example pool_crawl    --no-default-features --features camoufox      # high-concurrency pool + proxy/fingerprint rotation + resume
cargo run --example ocr_captcha   --no-default-features --features camoufox,ocr  # captcha OCR
cargo run --example geetest_slide --no-default-features --features slider        # GeeTest slider (slider pulls in camoufox)
cargo run --example dx_slide      --no-default-features --features slider        # Dingxiang slider-gap recognition (HL=0 to see the UI)
cargo run --example env_signer    --features signer           # embedded-QuickJS pure-script signing (no Node)
```

See the full [Examples index (48+)](examples/README.md).

---

## 🖥️ Supported platforms & browsers

- **Platforms**: macOS (arm64, primary) · Linux · **Windows (stable)** — CDP launches the local browser directly; the Camoufox backend uses named-pipe transport.
- **Browser**: **Google Chrome by default** (also Edge / Brave / Chromium / Electron, CDP) with smart path detection (Windows registry `App Paths` + user-level `%LOCALAPPDATA%` + `PATH`, mirroring DrissionPage). Optional [Camoufox](https://github.com/daijro/camoufox) (anti-detect Firefox fork, use `default-features = false, features = ["camoufox"]`, **auto-downloaded** on first run).
- **Protocol**: the Chromium backend speaks **CDP** (Chrome DevTools Protocol); Camoufox speaks Firefox's **Juggler** (this library implements its own async Juggler client on `tokio`).
- **Rust**: ≥ 1.85 (edition 2024).

---

## ❓ FAQ

**Q: How does drission relate to DrissionPage?**
A: The API is deliberately aligned with DrissionPage, so migrating from Python DP is near-zero cost (see [API mapping](docs/API映射.md)); but drission is a **native Rust rewrite** with higher performance and **built-in captcha solving and anti-detect**.

**Q: Does captcha solving need the network or a solving service?**
A: No. Character OCR runs **offline** with ddddocr pretrained models + pure-Rust inference; slider-gap distance is a local image algorithm. Only the model is auto-downloaded once to cache.

**Q: Does it support Chrome? Which browser is the default?**
A: **Google Chrome is the default** (Chromium / CDP backend, out of the box; also Edge / Brave / Chromium / Electron). The local Chrome path is auto-detected (`CHROME_BIN` / `DRISSION_CHROME` → install paths → **Windows registry `App Paths`** → `PATH`, mirroring DrissionPage); if not found, pin it via `ChromiumBrowser::launch_with(path, headless)`. For the Firefox anti-detect engine, disable default cdp and enable `camoufox`.

**Q: Can it pass Cloudflare?**
A: Yes. `tab.pass_cloudflare()` supports interactive trusted Turnstile clicks and non-interactive auto-pass.

**Q: How do I do high-concurrency crawling?**
A: Use the `BrowserPool` with built-in proxy / fingerprint rotation, retries and **resume-from-checkpoint**; switch to Session (HTTP) dual mode to save memory.

**Q: Is it cross-platform? What Rust version?**
A: macOS (primary) · Linux · Windows (named-pipe transport working); Rust ≥ 1.85 (edition 2024).

---

## 📚 Documentation

- [🤖 **Coding SKILL (AI must-read)**](docs/SKILL.md) — authoritative API / feature / build cheat-sheet, basics → click-word captcha, copy-correct
- [Docs overview `docs/`](docs/) — design · API mapping · pool · long-listen
- [**DrissionPage → drission API mapping**](docs/API映射.md) — migrate from DP by swapping Python for Rust, near-zero cost
- [Design doc](docs/设计.md) — layered architecture / Juggler / concurrency model / wiring
- [Concurrency pool design](docs/并发池.md) — `BrowserPool` / proxy pool / fingerprint pool / resume
- [Examples index (48+)](examples/README.md) · [API reference (docs.rs)](https://docs.rs/drission) · [Changelog](CHANGELOG.md)

---

## 🗺️ Shipped And Next

Already shipped:

- Click / text-click captcha pipeline: `Det` boxes → per-box OCR → glyph-template second signal → globally optimal assignment → trusted clicks; NetEase/Yidun examples include collection, probing and stable-click flows.
- Runtime OCR self-training integration: load and hot-swap `dddd_trainer` onnx + `charsets.json` outputs, with a documented workflow.
- Generic slider gap detection and human-like motion: GeeTest v4 / Dingxiang examples, minimum-jerk tracks, closed-loop correction and trusted mouse events.
- Anti-detect and env-restore coverage: CDP/Camoufox fingerprints, font enumeration, pixel-level canvas, WebRTC, plugins/mimeTypes, WebGL/audio replay.
- Browser WS takeover: `BrowserServer` + `Browser::connect` support one active client, reconnects and token checks.
- Static XPath 1.0 common subset, Windows Job Object process-tree cleanup, Linux Docker/musl/CI build matrix.

Next:

- Arithmetic captchas, more click/text-click vendor templates and sample libraries.
- Slider / click behavior-trajectory modeling, moving behavior risk handling from heuristics toward reusable models.
- True multi-client multiplexing for WS takeover and `wss://` TLS.
- More static XPath axes/functions, plus more vendor slider / shield presets.
- More real Linux cloud-vendor, distro and headed/headless test matrices.

---

## ⚠️ Disclaimer

This project is for learning and lawful, non-profit use only. Users must obey target sites' `robots` policy and
local laws and regulations. It **must not** be used for anything illegal, harmful to others, harassing/attacking,
or for collecting protected data. All actions and consequences arising from using drission are borne solely by the
user and are unrelated to the copyright holder (极数本源); the copyright holder is not liable for any loss caused by
possible defects in this project.

**Without authorization, selling, reselling, or using this project (modified or not) as the core of a paid
product/service for profit is prohibited.** See [`LICENSE`](LICENSE).

---

## 🙏 Acknowledgements

- [DrissionPage](https://github.com/g1879/DrissionPage): API design inspiration (incl. Chrome path detection).
- [Camoufox](https://github.com/daijro/camoufox): the optional anti-detect Firefox engine.
- [ddddocr](https://github.com/sml2h3/ddddocr): captcha OCR pretrained models.
- [tract](https://github.com/sonos/tract): pure-Rust ONNX inference engine.

## 📄 License

Custom license (source-available · personal learning and lawful non-profit use only · no unauthorized commercial use
or resale), see [`LICENSE`](LICENSE).

---

<sub>keywords: Rust browser automation · captcha solver · ddddocr · captcha OCR · slider-gap distance · GeeTest · Dingxiang ·
anti-detect · undetectable · stealth · Cloudflare bypass · web scraping · crawler · proxy rotation · fingerprint · env restore ·
pure-script signing · Camoufox · Firefox Juggler · Chromium CDP · DrissionPage · Rust DrissionPage · alternative to rust_drission / zendriver-rs ·
by [极数本源 apizero.cn](https://apizero.cn).</sub>
