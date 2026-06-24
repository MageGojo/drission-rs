//! Chrome 自动下载分发(CDP 后端):对标 CloakBrowser「首次运行自动下载浏览器二进制」。
//!
//! 流程:解析平台 → 下载 / 复用缓存的 **Chrome for Testing** → 用下载的 Chrome 启动 CDP →
//! 导航 example.com → 读标题 / h1 验证。**这就是「用 drission 驱动自己下载的 Chrome」的案例测试。**
//!
//! 运行:`cargo run --example cdp_fetch`(无头默认;`HL=0` 有头)。
//! 跨平台预取(在 mac 上为分发顺带下 win64):`DRISSION_PREFETCH_WIN=1 cargo run --example cdp_fetch`。
//! 缓存目录:`~/.cache/drission/chrome/<platform>`(`DRISSION_CACHE` 可覆盖)。

use drission::cdp::{ChromiumBrowser, cft_platform, download_chrome_for};

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "drission=info".into()),
        )
        .init();

    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let platform = cft_platform()?;
    println!("[*] 当前平台 Chrome for Testing 标记 = {platform}");

    // 下载(或复用缓存)当前平台的 Chrome for Testing,拿到可执行文件路径。
    println!("[*] 确保当前平台 Chrome 已下载(首次约 180–200 MB,之后命中缓存秒回)…");
    let exe = download_chrome_for(platform, "Stable").await?;
    println!("[*] Chrome 可执行文件: {}", exe.display());

    // 跨平台预取:在 mac/linux 上也把 win64 拉到本地(「mac 和 win 都要」),仅分发用、不在本机运行。
    if std::env::var("DRISSION_PREFETCH_WIN").as_deref() == Ok("1") {
        println!("[*] 预取 win64 Chrome(分发用,不在本机运行)…");
        let win = download_chrome_for("win64", "Stable").await?;
        println!("[*] win64 Chrome: {}", win.display());
    }

    // 用**下载的 Chrome** 启动 CDP 并驱动,验证可用。
    println!("[*] 用下载的 Chrome 启动 CDP(headless={headless})…");
    let browser = ChromiumBrowser::launch_with(&exe, headless).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;
    tab.get("https://example.com").await?;

    let title = tab.title().await?;
    let h1 = tab.ele_text("h1").await?.unwrap_or_default();
    println!("[*] title = {title:?}");
    println!("[*] h1    = {h1:?}");
    let ua = tab.user_agent().await?;
    println!("[*] UA    = {ua}");

    browser.quit().await?;

    let ok = title.contains("Example") && h1.contains("Example");
    if ok {
        println!("==== ALL CHECKS PASSED(下载的 Chrome 驱动成功)====");
        Ok(())
    } else {
        Err(drission::Error::msg(format!(
            "校验失败: title={title:?} h1={h1:?}"
        )))
    }
}
