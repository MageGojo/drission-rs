//! ⑤【主动逆向 · 抓包 → impersonate 重放闭环】浏览器抓请求 → cookie 交接 → 改字段重放验真。
//!
//! 运行:`cargo run --example re_replay --features impersonate`(需浏览器 TLS/JA3 指纹后端)。
//! 链路:CDP 浏览器 `listen` 抓到带签名的请求 → `load_cookies_from_cdp_tab` 把登录态灌给
//! Session → `replay(&pkt).set("t", now)` 改时间戳(模拟重签)→ `send()` 走 Chrome 指纹发出验真。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);

    // 1) CDP 浏览器抓一个 XHR/fetch 请求。
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;
    let listen = tab.listen();
    listen.start_xhr(&["example.com/?"]).await?;
    tab.get("https://example.com").await?;
    tab.run_js("setTimeout(()=>fetch('https://example.com/?a=1&t='+Date.now()),0); 1")
        .await?;

    let Some(pkt) = listen.wait(Some(Duration::from_secs(10))).await? else {
        println!("[!] 没抓到请求,退出。");
        browser.quit().await?;
        return Ok(());
    };
    listen.stop().await?;
    println!("[*] 抓到: {} {}", pkt.method, pkt.url);

    // 2) 浏览器 → Session 的 cookie 交接 + Chrome TLS/JA3 指纹。
    let mut sess = SessionPage::new(SessionOptions::new().profile(BrowserProfile::Chrome))?;
    sess.load_cookies_from_cdp_tab(&tab).await?;

    // 3) 重放:改 t= 时间戳(实战中此处替换为「重签后的新签名头/参数」)→ 发送验真。
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default();
    let ok = sess.replay(&pkt).set("t", &now_ms).send().await?;
    println!("[*] 重放结果: status={} ok={ok}", sess.status());
    let preview: String = sess.text().chars().take(120).collect();
    println!("[*] body[..120] = {preview}");

    browser.quit().await?;
    println!("==== ⑤ 抓包重放 demo 完成 ====");
    Ok(())
}
