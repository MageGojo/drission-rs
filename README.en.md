# drission · Anti-Detect Browser Automation in Rust + Built-in Captcha Solving (OCR / Slider-Gap)

[![crates.io](https://img.shields.io/crates/v/drission.svg)](https://crates.io/crates/drission)
[![docs.rs](https://docs.rs/drission/badge.svg)](https://docs.rs/drission)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-blue.svg)](#-supported-platforms--browsers)

**English** · [简体中文](README.md)

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

## 🆕 New in v0.3.0

> Full history in [CHANGELOG.md](CHANGELOG.md). This release brings **full dual-protocol API parity**, Session TLS fingerprinting, per-browser fingerprints, and an AI coding skill doc.

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

# Want the Camoufox anti-detect engine + all high-level capabilities? enable camoufox:
# drission = { version = "0.3", features = ["camoufox", "ocr", "slider", "signer", "impersonate"] }
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

**Camoufox anti-detect engine** (`--features camoufox`) — auto-downloaded, with shield bypass / env-dump / pool / slider and all high-level capabilities:

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

Examples (Camoufox-based examples need `--features camoufox`):

```bash
cargo run --example cdp_demo                                  # default Chromium / CDP backend (Google Chrome)
cargo run --example quickstart    --features camoufox         # Camoufox minimal loop
cargo run --example pool_crawl    --features camoufox         # high-concurrency pool + proxy/fingerprint rotation + resume
cargo run --example ocr_captcha   --features camoufox,ocr     # captcha OCR
cargo run --example geetest_slide --features slider           # GeeTest slider (slider pulls in camoufox)
cargo run --example dx_slide      --features slider           # Dingxiang slider-gap recognition (HL=0 to see the UI)
cargo run --example env_signer    --features signer           # embedded-QuickJS pure-script signing (no Node)
```

See the full [Examples index (48+)](examples/README.md).

---

## 🖥️ Supported platforms & browsers

- **Platforms**: macOS (arm64, primary) · Linux · **Windows (stable)** — CDP launches the local browser directly; the Camoufox backend uses named-pipe transport.
- **Browser**: **Google Chrome by default** (also Edge / Brave / Chromium / Electron, CDP) with smart path detection (Windows registry `App Paths` + user-level `%LOCALAPPDATA%` + `PATH`, mirroring DrissionPage). Optional [Camoufox](https://github.com/daijro/camoufox) (anti-detect Firefox fork, `--features camoufox`, **auto-downloaded** on first run).
- **Protocol**: the Chromium backend speaks **CDP** (Chrome DevTools Protocol); Camoufox speaks Firefox's **Juggler** (this library implements its own async Juggler client on `tokio`).
- **Rust**: ≥ 1.85 (edition 2024).

---

## ❓ FAQ

**Q: How does drission relate to DrissionPage?**
A: The API is deliberately aligned with DrissionPage, so migrating from Python DP is near-zero cost (see [API mapping](docs/API映射.md)); but drission is a **native Rust rewrite** with higher performance and **built-in captcha solving and anti-detect**.

**Q: Does captcha solving need the network or a solving service?**
A: No. Character OCR runs **offline** with ddddocr pretrained models + pure-Rust inference; slider-gap distance is a local image algorithm. Only the model is auto-downloaded once to cache.

**Q: Does it support Chrome? Which browser is the default?**
A: **Google Chrome is the default** (Chromium / CDP backend, out of the box; also Edge / Brave / Chromium / Electron). The local Chrome path is auto-detected (`CHROME_BIN` / `DRISSION_CHROME` → install paths → **Windows registry `App Paths`** → `PATH`, mirroring DrissionPage); if not found, pin it via `ChromiumBrowser::launch_with(path, headless)`. For the Firefox anti-detect engine, enable `--features camoufox`.

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

## 🗺️ Roadmap

- Captcha: click / text-click selection, arithmetic, slider behavior-trajectory modeling, OCR self-training
  (`dddd_trainer`).
- Deeper anti-detect fingerprint injection and "env restore" completeness (font enumeration, pixel-level canvas, WebRTC).
- WS takeover with multi-client multiplexing, `wss://` TLS.
- Static XPath subset expansion, more vendor slider / shield presets.
- More complete Windows process lifecycle (Job Object fallback) and a Linux tested matrix.

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
