//! 抗检测基本面验证:`navigator.webdriver` 应为 false,并到 bot.sannysoft.com
//! 统计 passed/failed 数量。Camoufox 专为反指纹检测设计,headless 也被打补丁伪装。
//!
//! 运行:`cargo run --example anti_detect`

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    // 反检测开箱即用:humanize + block_webrtc 已是默认值(见 BrowserOptions::default),
    // 一行起步即可;想无头就 `Browser::launch(BrowserOptions::new().headless(true))`。
    let browser = Browser::launch_default().await?;
    let tab = browser.latest_tab().await?;

    println!("== 基础指纹 ==");
    println!("  webdriver = {}", tab.run_js("navigator.webdriver").await?);
    println!("  platform  = {}", tab.run_js("navigator.platform").await?);
    println!(
        "  languages = {}",
        tab.run_js("JSON.stringify(navigator.languages)").await?
    );
    println!(
        "  hardwareConcurrency = {}",
        tab.run_js("navigator.hardwareConcurrency").await?
    );
    println!(
        "  RTCPeerConnection (block_webrtc 后应为 undefined) = {}",
        tab.run_js("typeof window.RTCPeerConnection").await?
    );

    println!("\n== bot.sannysoft.com 检测 ==");
    tab.get("https://bot.sannysoft.com").await?;
    // 等页面里的异步检测脚本跑完。
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let passed = tab
        .run_js("document.querySelectorAll('.passed').length")
        .await?;
    let failed = tab
        .run_js("document.querySelectorAll('.failed, .warn').length")
        .await?;
    println!("  passed = {passed}, failed/warn = {failed}");

    let webdriver_row = tab
        .run_js(
            "(() => { const r = [...document.querySelectorAll('tr')] \
             .find(tr => /webdriver/i.test(tr.textContent)); \
             return r ? r.innerText.replace(/\\s+/g, ' ').trim() : '(未找到)'; })()",
        )
        .await?;
    println!("  WebDriver 行: {webdriver_row}");

    browser.quit().await?;
    Ok(())
}
