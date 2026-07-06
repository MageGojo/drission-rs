//! ③【主动逆向 · JS Hook / crypto tap】hook 常见 sink,命中回传参数 + 调用栈;偷 key/iv/明文。
//!
//! 运行:`cargo run --example re_hook`(无头默认;`HL=0` 有头)。
//! 演示:hook `crypto.subtle` + `JSON.stringify` + 自定义函数 `buildSign`,触发后打印命中。

use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;
    tab.get("https://example.com").await?;

    // 先装 hook(crypto + json + 自定义函数 + 调用栈),再注入会触发它们的页面逻辑。
    let hook = tab
        .hook()
        .crypto_subtle()
        .json()
        .custom("buildSign")
        .with_stack()
        .start()
        .await?;
    println!("[*] hook 已装载,触发页面逻辑…");

    tab.run_js(
        r#"
        window.buildSign = function(t){ return 'SECRET-' + t; };
        (async function(){
            // 触发自定义函数 hook
            var s = window.buildSign(Date.now());
            // 触发 JSON.stringify hook
            JSON.stringify({ sign: s, nonce: 42 });
            // 触发 crypto.subtle.digest hook(明文回传为 bytes/base64)
            var data = new TextEncoder().encode('hello-' + s);
            await crypto.subtle.digest('SHA-256', data);
        })(); 1
    "#,
    )
    .await?;

    // 收集命中。
    let hits = hook.wait_count(8, Some(Duration::from_secs(5))).await?;
    println!("[*] 收到 {} 条 hook 命中:", hits.len());
    for h in &hits {
        println!("  ── [{}] {}", h.sink, h.func);
        for (i, a) in h.args.iter().enumerate() {
            println!("       arg[{i}] = {a}");
        }
        if !h.stack.is_empty() {
            let first = h.stack.lines().take(3).collect::<Vec<_>>().join(" | ");
            println!("       stack: {first}");
        }
    }

    hook.stop().await?;
    browser.quit().await?;
    println!("==== ③ Hook / crypto tap demo 完成 ====");
    Ok(())
}
