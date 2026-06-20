//! 端到端最小闭环:启动 → 开标签 → 访问 → 读标题/URL → 查元素读文本 → 退出。
//!
//! 运行:`cargo run --example quickstart`
//! 调试日志:`RUST_LOG=debug cargo run --example quickstart`

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    println!("[1] 启动浏览器(headless)…");
    let opts = BrowserOptions::new()
        .headless(true)
        .locale("zh-CN")
        .timezone("Asia/Shanghai");
    let browser = Browser::launch(opts).await?;
    println!("    已启动,标签数 = {}", browser.tab_count().await);

    let tab = browser.latest_tab().await?;
    println!("    会话 = {}", tab.session_id());

    println!("[2] 访问 https://example.com …");
    tab.get("https://example.com").await?;

    let title = tab.title().await?;
    let url = tab.url().await?;
    println!("[3] 标题 = {title:?}");
    println!("    URL  = {url:?}");

    println!("[4] 查找 h1 文本 …");
    let h1 = tab.ele("tag:h1").await?;
    println!("    h1 = {:?}", h1.text().await?);

    println!("[5] run_js: navigator.userAgent …");
    let ua = tab.run_js("navigator.userAgent").await?;
    println!("    UA = {ua}");

    println!("[6] 退出。");
    browser.quit().await?;
    Ok(())
}
