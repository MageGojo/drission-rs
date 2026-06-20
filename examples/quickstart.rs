//! 端到端最小闭环(**大道至简** · `Page` 门面):一行起步 → 访问 → 读标题/URL → 查元素 → 退出。
//!
//! `Page` 对标 DrissionPage 的 `ChromiumPage`:把「开浏览器 + 驱动当前标签」合一,像写 Python 脚本。
//! `page` 通过 `Deref` 拥有全部 `Tab` 方法(`get`/`ele`/`title`/`run_js`/`click`/`input`/`listen`…)。
//!
//! 需要更底层(多标签 / 接管 / 并发池)时,仍可用 `Browser` + `Tab`(见其它示例);
//! 需要 Driver/Session 双模见 `WebPage`,纯 HTTP 见 `SessionPage`。
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

    // 一行起步:无头 + 反检测开箱即用。
    //   有头:Page::new()        自定义:Page::with(BrowserOptions::new().headless(true).locale("zh-CN"))
    println!("[1] 启动浏览器(headless)…");
    let page = Page::headless().await?;

    println!("[2] 访问 https://example.com …");
    page.get("https://example.com").await?;

    // 小贴士:导航刚返回时,首个读取可能落在旧执行上下文(标题/URL 读到空)。
    // `ele` 自带「超时内等待」,先查个元素即等到新页面就绪,再读标题/URL 就稳了。
    println!("[3] 查 h1 文本 …");
    let h1 = page.ele("tag:h1").await?;
    println!("    h1 = {:?}", h1.text().await?);

    println!("[4] 标题 = {:?}", page.title().await?);
    println!("    URL  = {:?}", page.url().await?);

    println!("[5] run_js: navigator.userAgent …");
    println!("    UA = {}", page.run_js("navigator.userAgent").await?);

    println!("[6] 退出。");
    page.quit().await?;
    Ok(())
}
