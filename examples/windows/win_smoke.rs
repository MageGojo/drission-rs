//! Windows 冒烟测试:验证 Windows 下「启动 Camoufox → fd3/4 命名管道 Juggler 握手 →
//! 打开页面 → 执行 JS → 退出」整条链路是否通。
//!
//! 这是 Windows 传输分支(命名管道 + CRT `lpReserved2` fd3/4 句柄注入)的端到端验证。
//! macOS/Linux 也能跑(走各自的 unix 管道实现),输出应一致。
//!
//! 运行(默认无头,适合服务器/无桌面环境):
//!   cargo run --example win_smoke --no-default-features --features camoufox
//! 指定网址 / 有头模式:
//!   cargo run --example win_smoke --no-default-features --features camoufox -- https://example.com head
//! 看底层日志(含 Camoufox 的 "Juggler listening to the pipe" 就绪行):
//!   PowerShell:  $env:RUST_LOG="debug"; cargo run --example win_smoke --no-default-features --features camoufox
//!   CMD:         set RUST_LOG=debug && cargo run --example win_smoke --no-default-features --features camoufox

use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // 参数:[url] [head|headless]
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com".to_string());
    let headless = !std::env::args().any(|a| a == "head" || a == "headed");

    println!("== drission-rs Windows 冒烟测试 ==");
    println!("  OS       : {}", std::env::consts::OS);
    println!("  ARCH     : {}", std::env::consts::ARCH);
    println!("  URL      : {url}");
    println!("  headless : {headless}");
    println!();

    // 首次运行会自动下载对应平台的 Camoufox 到 ~/.cache/camoufox(Windows 同理)。
    println!("[1/5] 启动浏览器(首次会自动下载 Camoufox,请耐心等待)…");
    let browser = Browser::launch(BrowserOptions::new().headless(headless)).await?;
    println!("      OK:浏览器已启动,Juggler 管道在线。");

    let tab = browser.latest_tab().await?;

    println!("[2/5] 导航到 {url} …");
    tab.get(&url).await?;
    println!("      OK:页面已加载。");

    println!("[3/5] 读取标题 / HTML 长度 …");
    let title = tab.title().await.unwrap_or_default();
    let html_len = tab.html().await.map(|h| h.len()).unwrap_or(0);
    println!("      title    = {title:?}");
    println!("      html.len = {html_len} 字节");

    println!("[4/5] 在页面里执行 JS …");
    let ua = tab.user_agent().await?;
    let webdriver = tab.run_js("navigator.webdriver").await?;
    println!("      userAgent     = {ua}");
    println!("      webdriver     = {webdriver}(反检测下应为 false)");

    // 给个肉眼可见的停顿(有头模式时)。
    if !headless {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    println!("[5/5] 退出浏览器 …");
    browser.quit().await?;

    // 简单判定:标题非空或 HTML 有内容即视为链路打通。
    let ok = !title.is_empty() || html_len > 100;
    println!();
    println!(
        "结果:{}",
        if ok {
            "通过 ✅ —— Windows 传输链路工作正常"
        } else {
            "可疑 ⚠️ —— 页面看起来是空的,请用 RUST_LOG=debug 复跑看日志"
        }
    );
    Ok(())
}
