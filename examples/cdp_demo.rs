//! CDP 后端 demo(**默认后端**,无需 feature):启动/接管 Google Chrome → 新标签 → 导航 → run_js → 元素文本 → 截图。
//! 运行:`cargo run --example cdp_demo`(无头默认;`HL=0` 有头)。
//! 浏览器自动探测(优先 Google Chrome):`CHROME_BIN`/`DRISSION_CHROME` 环境变量 → 安装路径
//! (Windows 含用户级 `%LOCALAPPDATA%`)→ Windows 注册表 `App Paths` → 系统 `PATH`;找不到可设 `CHROME_BIN`。
//! 接管已开 Chrome:先 `chrome --remote-debugging-port=9222`,再设 `CONNECT=http://127.0.0.1:9222`。

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);

    let browser = if let Ok(url) = std::env::var("CONNECT") {
        println!("[*] 接管已开浏览器 {url}");
        ChromiumBrowser::connect(&url).await?
    } else {
        println!("[*] 启动 Chrome(headless={headless})");
        ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?
    };

    let tab = browser.new_tab(Some("about:blank")).await?;
    tab.get("https://example.com").await?;
    println!("[*] title = {:?}", tab.title().await?);
    println!("[*] url   = {:?}", tab.url().await?);
    println!("[*] 1+2   = {}", tab.run_js("1+2").await?);
    println!("[*] h1    = {:?}", tab.ele_text("h1").await?);
    let png = tab.screenshot_bytes().await?;
    println!(
        "[*] 截图 {} bytes(头 {:02X?})",
        png.len(),
        &png[..png.len().min(4)]
    );

    browser.quit().await?;
    println!("==== CDP demo 完成 ====");
    Ok(())
}
