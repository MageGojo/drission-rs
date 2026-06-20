//! 监听测试:打开抖音视频短链,抓取 `aweme/v1/web/aweme/detail` 接口的响应(一次)。
//!
//! 用我们库的网络监听能力:页面 JS 自己带签名发出请求,我们只负责抓响应体。
//! 运行:`cargo run --example douyin_listen`

use drission::prelude::*;

const SHORT_URL: &str = "https://v.douyin.com/I1mlU0fBFhI/";
const TARGET_API: &str = "aweme/v1/web/aweme/detail";

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
            .locale("zh-CN")
            .timezone("Asia/Shanghai"),
    )
    .await?;
    let tab = browser.latest_tab().await?;

    // 导航前先开监听,避免漏掉早期请求。
    tab.listen_xhr(&[TARGET_API]).await?;
    println!("已开启监听,打开抖音短链: {SHORT_URL}");
    tab.get(SHORT_URL).await?;
    println!("落地 URL: {}", tab.url().await.unwrap_or_default());

    // 抓一次目标接口的响应。
    match tab.listen_wait().await {
        Ok(p) => {
            println!("\n=== 抓到目标请求 ===");
            println!("URL: {}", p.url);
            println!("方法: {}", p.method);
            println!("状态: {} {}", p.response.status, p.response.status_text);
            println!("资源类型: {}", p.resource_type);
            println!("响应体总长: {} 字符", p.response.body.chars().count());
            let preview: String = p.response.body.chars().take(1500).collect();
            println!("响应体(前 1500 字符):\n{preview}");
        }
        Err(e) => {
            println!("\n未抓到目标 API: {e}");
            println!("最终 URL: {}", tab.url().await.unwrap_or_default());
            println!("标题: {}", tab.title().await.unwrap_or_default());
        }
    }

    browser.quit().await?;
    Ok(())
}
