//! 过 CF 盾验证:访问一个 Cloudflare 保护的测试页,观察是否能自动通过 challenge。
//!
//! 能否过盾取决于目标站点的 CF 配置与本机网络;Camoufox 默认硬化 + `humanize` +
//! 无 `webdriver` 痕迹 + `block_webrtc` 是过盾的基础设施。
//! 运行:`cargo run --example cf_check --no-default-features --features camoufox`

use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    // 反检测(humanize + block_webrtc)已是默认;这里仅按需自定义地区(与目标/IP 匹配再设)。
    let browser = Browser::launch(
        BrowserOptions::new()
            .locale("en-US")
            .timezone("America/New_York"),
    )
    .await?;
    let tab = browser.latest_tab().await?;

    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://www.scrapingcourse.com/cloudflare-challenge".to_string());
    println!("访问: {url}");
    tab.get(&url).await?;

    // 自动过盾:非交互式 challenge 等待其自动放行;交互式 Turnstile 自动**可信点击**复选框。
    let passed = tab.pass_cloudflare(Duration::from_secs(40)).await?;
    let title = tab.title().await.unwrap_or_default();
    println!(
        "\n结果: {}",
        if passed {
            "已过 CF 盾"
        } else {
            "仍被 CF 拦截 / 未过"
        }
    );
    println!("最终标题: {title:?}");

    browser.quit().await?;
    Ok(())
}
