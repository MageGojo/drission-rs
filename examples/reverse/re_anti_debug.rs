//! ④【主动逆向 · 反「无限 debugger」】defuse `setInterval(debugger)` / `Function("debugger")()`。
//!
//! 运行:`cargo run --example re_anti_debug`(无头默认;`HL=0` 有头)。
//! 验证:注入 defuse 后,页面里的反调试 `debugger` 被致残——`setInterval` 回调与 `Function("debugger")`
//! 都不再触发暂停;脚本继续正常跑(打印计数器递增证明没卡住)。

use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;

    // 关键:**导航前**注入 defuse(对新文档生效)。
    tab.anti_anti_debug().await?;
    println!("[*] 已注入反「无限 debugger」defuse。");

    tab.get("https://example.com").await?;
    // 注入典型反调试:每 50ms 一个 debugger;再用 Function 构造器跑一个。
    tab.run_js(
        r#"
        window.__tick = 0;
        setInterval(function(){ debugger; window.__tick++; }, 50);
        try { Function("debugger; window.__fnDebug=1;")(); } catch(e){}
        1
    "#,
    )
    .await?;

    // 等一会,看计数器是否在涨(若被 debugger 卡住则不会涨)。
    tokio::time::sleep(Duration::from_millis(600)).await;
    let tick = tab.run_js("window.__tick").await?;
    let fn_debug = tab.run_js("window.__fnDebug||0").await?;
    println!("[*] setInterval 计数 __tick = {tick}(>0 说明没被 debugger 卡住)");
    println!("[*] Function 构造器 __fnDebug = {fn_debug}(致残后函数体的 debugger 被剔除仍能执行)");

    browser.quit().await?;
    println!("==== ④ 反「无限 debugger」demo 完成 ====");
    Ok(())
}
