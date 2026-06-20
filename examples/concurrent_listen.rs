//! 演示三大核心能力:
//! 1. **并发**:同一浏览器内并行开多个标签并各自导航;
//! 2. **每标签独立 cookie**:各标签处于独立 BrowserContext,cookie 互不可见;
//! 3. **XHR 监听**:抓取页面发出的 fetch/XHR 请求与响应体。
//!
//! 运行:`cargo run --example concurrent_listen`

use std::sync::Arc;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let browser = Arc::new(Browser::launch(BrowserOptions::new().headless(true)).await?);

    // ---------- 并发 + 每标签独立 cookie ----------
    println!("== 并发开 3 个标签,各设不同 cookie 并验证隔离 ==");
    let mut handles = Vec::new();
    for i in 0..3 {
        let b = browser.clone();
        handles.push(tokio::spawn(async move {
            let tab = b.new_tab(None).await?;
            tab.set_cookies(vec![
                CookieParam::new("tabid", format!("tab{i}")).url("https://example.com"),
            ])
            .await?;
            tab.get("https://example.com").await?;
            let cookies = tab.cookies().await?;
            let seen: Vec<String> = cookies
                .iter()
                .map(|c| format!("{}={}", c.name, c.value))
                .collect();
            Ok::<_, drission::Error>((i, tab.title().await?, seen))
        }));
    }
    for h in handles {
        let (i, title, cookies) = h.await.expect("任务 join 失败")?;
        println!("  标签{i}: 标题={title:?} 可见cookie={cookies:?}");
    }

    // ---------- XHR 监听 ----------
    println!("\n== XHR 监听:抓取页面发出的 fetch 请求 ==");
    let tab = browser.new_tab(Some("https://example.com")).await?;
    tab.listen_xhr(&["api.github.com"]).await?;
    // 触发一个跨域 fetch(即使 JS 侧因 CORS 读不到,网络层仍可抓到完整响应)。
    tab.run_js("fetch('https://api.github.com/zen').catch(() => {}); true")
        .await?;

    let packet = tab.listen_wait().await?;
    let body_preview: String = packet.response.body.chars().take(80).collect();
    println!("  捕获: {} [{}]", packet.url, packet.method);
    println!(
        "  状态: {} {}",
        packet.response.status, packet.response.status_text
    );
    println!("  资源类型: {}", packet.resource_type);
    println!("  响应体预览: {body_preview:?}");

    browser.quit().await?;
    println!("\n完成。");
    Ok(())
}
