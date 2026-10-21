//! 自测自己的页面。
//!
//! 1. 本地页：填名称、点保存，确认页面写出结果。
//! 2. 我们的公开站：打开 <https://apizero.cn/>，确认标题，点「文档」进入 `/aidocs`。
//!
//! 不登录，不向线上提交表单。
//!
//! 运行：`cargo run --example own_site`（无头默认；`HL=0` 有头）。

use std::time::{Duration, Instant};

use drission::prelude::*;

const LOCAL: &str = include_str!("../pages/self_check.html");

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;

    tab.set_content(LOCAL).await?;
    tab.input("#name", "极数本源").await?;
    tab.click("#save").await?;
    let out = tab.ele_text("#out").await?.unwrap_or_default();
    if !out.contains("已保存") || !out.contains("极数本源") {
        browser.quit().await?;
        return Err(drission::Error::msg(format!("本地页结果不对: {out}")));
    }
    println!("[ok] 本地页: {out}");

    tab.get("https://apizero.cn/").await?;
    tab.wait().doc_loaded(Some(Duration::from_secs(20))).await?;
    let title = tab.title().await?;
    if !title.contains("ApiZero") {
        browser.quit().await?;
        return Err(drission::Error::msg(format!("首页标题不对: {title}")));
    }
    println!("[ok] 首页: {title}");

    tab.click("css:main a[href='/aidocs']").await?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut url = String::new();
    while Instant::now() < deadline {
        url = tab.url().await.unwrap_or_default();
        if url.contains("/aidocs") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if !url.contains("/aidocs") {
        browser.quit().await?;
        return Err(drission::Error::msg(format!("没有进入文档页: {url}")));
    }
    tab.wait().doc_loaded(Some(Duration::from_secs(20))).await?;
    let doc_title = tab.title().await?;
    println!("[ok] 文档页: {doc_title} ({url})");

    browser.quit().await?;
    println!("==== own_site 完成 ====");
    Ok(())
}
