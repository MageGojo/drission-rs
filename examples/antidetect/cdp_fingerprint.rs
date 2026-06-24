//! 每浏览器不同指纹(CDP 后端)实跑验证。
//!
//! 起 N 个浏览器,每个套一份不同指纹([`CdpFingerprintPool`]),各自 dump
//! UA/platform/语言/时区/屏幕/硬件并发/内存/WebGL/canvas 哈希,打印成表 —— 直观看出"每浏览器各异"。
//!
//! 运行:
//! - `cargo run --example cdp_fingerprint`                  默认无头、3 个、同 OS 变体(Turnstile 友好)
//! - `N=5 cargo run --example cdp_fingerprint`              5 个
//! - `PERSONA=1 cargo run --example cdp_fingerprint`        完整跨 OS 画像(伪装 UA/platform/WebGL)
//! - `HEADFUL=1 cargo run --example cdp_fingerprint`        有头可视

use drission::Result;
use drission::cdp::{CdpFingerprintPool, ChromiumBrowser, ChromiumOptions};
use drission::prelude::FingerprintProbe;

#[tokio::main]
async fn main() -> Result<()> {
    let n: usize = std::env::var("N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let headful = std::env::var("HEADFUL").as_deref() == Ok("1");
    let persona = std::env::var("PERSONA").as_deref() == Ok("1");

    let pool = if persona {
        CdpFingerprintPool::personas(n)
    } else {
        CdpFingerprintPool::generate(n)
    };
    println!(
        "模式: {} | 浏览器数: {} | headless: {}\n",
        if persona {
            "完整跨 OS 画像(伪装 UA/platform/WebGL)"
        } else {
            "同 OS 变体(保真 UA/WebGL,Turnstile 友好)"
        },
        n,
        !headful
    );

    let base = ChromiumOptions::new().headless(!headful);
    for (i, fp) in pool.profiles().iter().enumerate() {
        let opts = fp.apply_to_options(base.clone());
        let browser = ChromiumBrowser::launch(opts).await?;
        let tab = browser.new_tab(Some("about:blank")).await?;
        // 必须导航到一份新文档,导航前注入脚本才会对其生效(addScriptToEvaluateOnNewDocument 只作用于后续文档)。
        tab.get(
            "data:text/html,<!doctype html><meta charset=utf-8><title>fp</title><body>fp</body>",
        )
        .await?;
        // 内置能力:一行读取实时指纹快照(库的 `tab.fingerprint_snapshot()`,探针 JS 已沉到库里)。
        let fp = tab.fingerprint_snapshot().await?;
        println!("── 浏览器 #{} ───────────────────────────────", i + 1);
        println!("  UA       : {}", fp.ua);
        println!("  platform : {}", fp.platform);
        println!("  languages: {}", fp.languages);
        println!("  timezone : {}", fp.timezone);
        println!("  screen   : {}  dpr={}", fp.screen, fp.device_pixel_ratio);
        println!(
            "  hw cores : {}   memory: {} GB",
            fp.hardware_concurrency, fp.device_memory
        );
        println!("  WebGL    : {}", fp.webgl_renderer);
        println!("  canvas#  : {}", fp.canvas_hash);
        println!();
        let _ = browser.quit().await;
    }

    println!(
        "完成。canvas# / 屏幕 / 时区 / 硬件 各浏览器应各不相同(persona 模式连 UA/WebGL 也不同)。"
    );
    Ok(())
}
