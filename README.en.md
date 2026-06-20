# drission · Anti-Detect Browser Automation in Rust + Built-in Captcha Solving (OCR / Slider-Gap)

[![crates.io](https://img.shields.io/crates/v/drission.svg)](https://crates.io/crates/drission)
[![docs.rs](https://docs.rs/drission/badge.svg)](https://docs.rs/drission)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-blue.svg)](#-supported-platforms--browsers)

**English** · [简体中文](README.md)

> **drission is a high-performance, anti-detect browser-automation library written in Rust.** By default it drives the
> [Camoufox](https://github.com/daijro/camoufox) anti-detect browser (optional Chromium / CDP backend), ships a
> **built-in character-captcha OCR** (ddddocr model · pure-Rust inference) and **image slider-gap recognition**
> (GeeTest / Dingxiang), supports XHR listen/intercept, automatic Cloudflare bypass and an async high-concurrency pool,
> with an API aligned to [DrissionPage](https://github.com/g1879/DrissionPage).

Made and maintained by **极数本源 ([apizero.cn](https://apizero.cn))** as part of its automation and
data-collection stack. If you're looking for a one-stop Rust solution for *captcha OCR / slider-gap distance /
anti-detect browser / high-concurrency crawling*, this is it.

> **What sets it apart**: Rust browser-automation libraries (e.g. `zendriver-rs`, `rust_drission`, `stygian-browser`) are mostly
> Chromium / CDP and rely on third-party captcha services (capsolver / 2captcha). **drission ships built-in offline captcha solving
> (ddddocr OCR + image slider-gap distance) and defaults to the Camoufox / Firefox anti-detect engine** — no external solver, no
> network round-trip; a rare "**Rust DrissionPage**" with batteries-included captcha solving.

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

## 🆕 New in v0.1.1

> Full history in [CHANGELOG.md](CHANGELOG.md). The following are **backward-compatible additions** accumulated after `0.1.0`:

- **CDP / Chromium backend** (`--features cdp`): new `ChromiumBrowser` / `ChromiumTab` / `ChromiumElement` to drive or attach to **Chrome / Edge / Brave / Electron**, with native trusted clicks, human typing, `Network` listening and `Fetch` interception, sharing data types with the Camoufox backend.
- **Session (HTTP) dual mode + `WebPage` facade**: a pure-HTTP session without a browser, with two-way cookie sharing and `change_mode` auto-syncing login state (the Driver + Session model of DrissionPage; memory-friendly).
- **Data export**: a `scrape` module (`records_to_csv` / `records_to_json` / `write_csv` / `write_json`), table extraction (`Element::table`), pagination (`Tab::paginate`).
- **Proxy-pool health checks**: `ProxyHealth` / `ProxyGeo`, exit-IP geo ↔ fingerprint consistency, residential proxy rotation.
- **Pure-script signing runner** (`--features signer`): embeds QuickJS so the exported `env.js` is compiled into a single binary — **replay the restored env and sign with no Node / no browser**.
- **Automatic Cloudflare bypass** `tab.pass_cloudflare()`, **generic Dingxiang gap algorithm** `GapMethod::ContentNcc`, **login-state persistence** `storageState`, **per-character human typing** `ele.input_human`, **Shadow DOM** (`ShadowRoot`), **download manager** `tab.downloads()`, **intercept handle** `tab.intercept()`.
- **Engineering infra**: multi-platform CI (fmt / clippy / test / feature matrix / cross-platform check / docs.rs build), offline integration tests, criterion benches, `CHANGELOG` / `CONTRIBUTING` / `SECURITY`.

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
- **Driver + Session dual mode**: browser and pure-HTTP session modes with two-way cookie sharing (memory-friendly).
- **Screenshots & recording**: element / full-page / region screenshots, viewport recording muxed to mp4.
- **Environment dumping ("env restore")**: collect real canvas / webgl / audio fingerprints + signature-sink
  location, export a `node`-runnable env-restore project in one click; with `signer`, compile it into a no-Node single binary.
- **Take over a browser**: `BrowserServer` exposes a WebSocket endpoint; `Browser::connect` attaches to a running browser.
- **Multiple backends**: Camoufox / Juggler by default; `--features cdp` adds a Chromium / CDP backend for Chrome / Edge / Brave / Electron.

---

## 🆚 drission vs alternatives

| Dimension | **drission** (Rust) | DrissionPage (Python) | Playwright / Selenium |
|---|---|---|---|
| Language / runtime | Rust · `tokio` async · single binary | Python | multi-language |
| Anti-detect by default | ✅ Camoufox (anti-detect Firefox) | ⚠️ DIY hardening | ❌ easily detected |
| Built-in captcha OCR | ✅ offline, pure Rust | ❌ | ❌ |
| Slider-gap recognition | ✅ GeeTest / Dingxiang | ❌ | ❌ |
| Auto Cloudflare bypass | ✅ `pass_cloudflare()` | ⚠️ partial | ❌ |
| XHR listen / body capture | ✅ built-in | ✅ | ⚠️ manual |
| Concurrency pool + resume | ✅ `BrowserPool` built-in | ⚠️ DIY | ❌ |
| Backends | Camoufox + optional Chromium/CDP | Chromium | many browsers |

> In short: **if you want "DrissionPage's ergonomics + Rust's performance + built-in solving & anti-detect", choose drission.**

---

## 📦 Install

```toml
[dependencies]
drission = "0.1"

# Enable captcha / backend capabilities on demand (off by default to keep the core lean):
# drission = { version = "0.1", features = ["ocr", "slider", "cdp", "signer"] }
```

| feature | capability | deps | default |
|---|---|---|---|
| `camoufox` | Camoufox / Firefox (Juggler) backend | core, always compiled | **on** |
| `ocr` | character captcha recognition (ddddocr + tract) | `image` + `tract-onnx` | off |
| `slider` | image slider-gap recognition (GeeTest / Dingxiang) | pure JS + std, zero extra deps | off |
| `cdp` | Chromium backend (Chrome / Edge / Brave / Electron) | std, no extra heavy deps | off |
| `signer` | pure-script signing runner (embedded QuickJS, no Node) | `rquickjs` | off |

---

## 🚀 Quick start

```rust
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    // Leave binary_path empty to auto-download a Camoufox build to ~/.cache/camoufox
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;

    tab.listen_start(&["api/data"]).await?;        // start listening first
    tab.get("https://example.com").await?;          // then navigate
    tab.ele("@id:kw").await?.input("drission").await?;
    tab.ele("#submit").await?.click().await?;

    let packet = tab.listen_wait().await?;          // capture the target XHR (with response body)
    println!("{}", packet.response.body);

    browser.quit().await?;
    Ok(())
}
```

Examples:

```bash
cargo run --example quickstart                          # minimal end-to-end loop
cargo run --example pool_crawl                          # high-concurrency pool + proxy/fingerprint rotation + resume
cargo run --example ocr_captcha   --features ocr        # captcha OCR
cargo run --example apizero_login --features ocr        # end-to-end: fill form + OCR captcha + login
cargo run --example geetest_slide --features slider     # GeeTest slider
cargo run --example dx_slide      --features slider      # Dingxiang slider-gap recognition (HL=0 to see the UI)
cargo run --example cdp_demo      --features cdp         # Chromium / CDP backend
cargo run --example env_signer    --features signer      # embedded-QuickJS pure-script signing (no Node)
```

See the full [Examples index (48+)](examples/README.md).

---

## 🖥️ Supported platforms & browsers

- **Platforms**: macOS (arm64, primary) · Linux · Windows (named-pipe transport, working).
- **Browser**: [Camoufox](https://github.com/daijro/camoufox) (anti-detect Firefox fork), **auto-downloaded** on first run; optional Chromium backend (Chrome / Edge / Brave / Electron, `--features cdp`).
- **Protocol**: Firefox's **Juggler** (not CDP) — Camoufox only supports Juggler; this library implements its own
  async Juggler client on `tokio`. The Chromium backend speaks **CDP**.
- **Rust**: ≥ 1.85 (edition 2024).

---

## ❓ FAQ

**Q: How does drission relate to DrissionPage?**
A: The API is deliberately aligned with DrissionPage, so migrating from Python DP is near-zero cost (see [API mapping](docs/API映射.md)); but drission is a **native Rust rewrite** with higher performance and **built-in captcha solving and anti-detect**.

**Q: Does captcha solving need the network or a solving service?**
A: No. Character OCR runs **offline** with ddddocr pretrained models + pure-Rust inference; slider-gap distance is a local image algorithm. Only the model is auto-downloaded once to cache.

**Q: Firefox only, or does it support Chrome?**
A: The default backend is Camoufox (anti-detect Firefox); enable `--features cdp` for a **Chromium / CDP** backend that drives or attaches to Chrome / Edge / Brave / Electron.

**Q: Can it pass Cloudflare?**
A: Yes. `tab.pass_cloudflare()` supports interactive trusted Turnstile clicks and non-interactive auto-pass.

**Q: How do I do high-concurrency crawling?**
A: Use the `BrowserPool` with built-in proxy / fingerprint rotation, retries and **resume-from-checkpoint**; switch to Session (HTTP) dual mode to save memory.

**Q: Is it cross-platform? What Rust version?**
A: macOS (primary) · Linux · Windows (named-pipe transport working); Rust ≥ 1.85 (edition 2024).

---

## 📚 Documentation

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

- [DrissionPage](https://github.com/g1879/DrissionPage): API design inspiration.
- [Camoufox](https://github.com/daijro/camoufox): default browser engine.
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
