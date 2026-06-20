//! auth.exa.ai 交互式过盾:填邮箱 → 触发 Cloudflare Turnstile → 等待其自动产出 token。
//!
//! 过盾判定:`input[name=cf-turnstile-response]` 的 value 变为非空(有效 token)。
//! 只做到过盾为止,不真正完成登录。
//! 运行:`cargo run --example exa_cf`

use std::time::Duration;

use drission::prelude::*;

const EMAIL: &str = "12341423@gmail.com";
const TURNSTILE_TOKEN_JS: &str =
    "(() => { const e = document.querySelector('[name=cf-turnstile-response]'); return e ? e.value : ''; })()";

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let browser = Browser::launch(
        BrowserOptions::new()
            .headless(true)
            .locale("en-US")
            .timezone("America/New_York"),
    )
    .await?;
    let tab = browser.latest_tab().await?;

    tab.get("https://auth.exa.ai/?callbackUrl=https%3A%2F%2Fdashboard.exa.ai%2F").await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 填邮箱(只碰 type=email 的真实输入框,避开 name=website 蜜罐)。
    let email = tab.ele("css:input[type=email]").await?;
    email.click().await?; // 鼠标点击(humanize 轨迹),有助于 Turnstile 行为判定
    email.input(EMAIL).await?;
    println!("已填邮箱: {EMAIL}");

    // 轮询等待 Turnstile 自动产出 token。
    let mut token_len = 0usize;
    for i in 0..30 {
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let token = tab.run_js(TURNSTILE_TOKEN_JS).await.unwrap_or_default();
        token_len = token.as_str().unwrap_or("").len();
        println!("  [{i:>2}s] turnstile_token_len = {token_len}");
        if token_len > 20 {
            break;
        }
    }

    if token_len > 20 {
        println!("\n结果: 已过 CF 盾(Turnstile 拿到有效 token,长度 {token_len})");
    } else {
        println!("\n结果: 未拿到 Turnstile token(可能需要点击交互或更强指纹)");
    }

    browser.quit().await?;
    Ok(())
}
