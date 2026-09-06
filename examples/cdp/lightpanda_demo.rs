//! Lightpanda 适配 demo:同一套 `Browser` / `Tab` / `ele`,只换引擎。
//!
//! 运行:`cargo run --example lightpanda_demo`
//! 二进制:`LIGHTPANDA_BIN` / `DRISSION_LIGHTPANDA` → PATH → 缓存 → GitHub nightly。
//! 接管已开 serve:先 `lightpanda serve --port 9222`,再 `CONNECT=ws://127.0.0.1:9222`。

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let browser = if let Ok(url) = std::env::var("CONNECT") {
        println!("[*] 接管 Lightpanda / CDP {url}");
        ChromiumBrowser::connect_with(&url, ChromiumOptions::new().lightpanda()).await?
    } else {
        println!("[*] 启动 Lightpanda(CDP,同一套 Tab API)…");
        ChromiumBrowser::launch(ChromiumOptions::new().lightpanda()).await?
    };

    let tab = browser.new_tab(Some("https://example.com")).await?;
    println!("[*] title = {:?}", tab.title().await?);
    println!("[*] url   = {:?}", tab.url().await?);
    println!("[*] 1+2   = {}", tab.run_js("1+2").await?);
    println!("[*] h1    = {:?}", tab.ele_text("h1").await?);

    browser.quit().await?;
    println!("==== Lightpanda demo 完成 ====");
    Ok(())
}
