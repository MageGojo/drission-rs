//! 控制台监听(对标 DrissionPage `tab.console`)端到端自验证。
//!
//! 覆盖:DP 经典例子(`console.log('DrissionPage')`)、多参拼接、对象/数组回页面序列化、
//! `body()` JSON 解析、级别(warn→warning / error)、`messages()` 批量取、以及 drission 增强的
//! 级别过滤 `start_with`。全程 `about:blank` + `run_js`,不依赖网络。
//!
//! 运行:`cargo run --example console_listen`
//!
//! 末尾打印 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`,失败则进程非 0 退出。

use std::time::Duration;

use drission::prelude::*;

const T: Option<Duration> = Some(Duration::from_secs(5));

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    println!("[*] 启动 Camoufox(headless)…");
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;
    tab.get("about:blank").await?;

    let console = tab.console();
    console.start().await?;
    println!("[*] console.start() listening={}", console.listening());

    // ---------- 1. DP 经典例子:console.log('DrissionPage') ----------
    tab.run_js("console.log('DrissionPage')").await?;
    let d1 = console.wait(T).await?.expect("应收到一条控制台消息");
    let case1 = d1.text == "DrissionPage";
    println!("[1] log('DrissionPage') → text={:?} level={:?} (ok={case1})", d1.text, d1.level);

    // ---------- 2. 多参数拼接(字符串/数字/布尔/null)----------
    tab.run_js("console.log('a', 1, true, null)").await?;
    let d2 = console.wait(T).await?.expect("应收到多参消息");
    let case2 = d2.text == "a 1 true null";
    println!("[2] log('a',1,true,null) → text={:?} (ok={case2})", d2.text);

    // ---------- 3. 对象 / 数组:回页面 JSON 序列化 + body() ----------
    tab.run_js("console.log({a:1, b:[2,3]})").await?;
    let d3 = console.wait(T).await?.expect("应收到对象消息");
    let body3 = d3.body();
    let case3 = d3.text == r#"{"a":1,"b":[2,3]}"#
        && body3.as_ref().map(|b| b["b"][1] == 3).unwrap_or(false);
    println!("[3] log({{a:1,b:[2,3]}}) → text={:?} body.b[1]={:?} (ok={case3})",
        d3.text, body3.map(|b| b["b"][1].clone()));

    // ---------- 4. JSON 字符串 → body() 解析 ----------
    tab.run_js(r#"console.log(JSON.stringify({x: 9, y: "z"}))"#).await?;
    let d4 = console.wait(T).await?.expect("应收到 JSON 字符串消息");
    let case4 = d4.body().map(|b| b["x"] == 9 && b["y"] == "z").unwrap_or(false);
    println!("[4] log(JSON.stringify(...)) → text={:?} body.x={:?} (ok={case4})",
        d4.text, d4.body().map(|b| b["x"].clone()));

    // ---------- 5. 级别:warn → warning,error → error ----------
    console.clear().await; // 清掉前面残留
    tab.run_js("console.warn('careful'); console.error('boom')").await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let msgs = console.messages().await;
    let warn = msgs.iter().find(|m| m.text == "careful");
    let err = msgs.iter().find(|m| m.text == "boom");
    let case5 = warn.map(|m| m.level == "warning").unwrap_or(false)
        && err.map(|m| m.level == "error").unwrap_or(false);
    println!("[5] warn/error 级别 → {:?} (ok={case5})",
        msgs.iter().map(|m| (m.level.as_str(), m.text.as_str())).collect::<Vec<_>>());

    // ---------- 6. messages() 批量取(并清空)----------
    console.clear().await;
    tab.run_js("for (let i=0;i<3;i++) console.log('item'+i)").await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let batch = console.messages().await;
    let case6 = batch.len() == 3 && batch[2].text == "item2" && console.messages().await.is_empty();
    println!("[6] messages() 批量 → {:?} (ok={case6})",
        batch.iter().map(|m| m.text.clone()).collect::<Vec<_>>());

    console.stop().await?;

    // ---------- 7. 增强:级别过滤 start_with(只收 error) ----------
    console.start_with(ConsoleFilter::new().level("error")).await?;
    tab.run_js("console.log('ignored'); console.error('kept')").await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let filtered = console.messages().await;
    let case7 = !filtered.is_empty()
        && filtered.iter().all(|m| m.level == "error")
        && filtered.iter().any(|m| m.text == "kept")
        && !filtered.iter().any(|m| m.text == "ignored");
    println!("[7] 过滤 level=error → {:?} (ok={case7})",
        filtered.iter().map(|m| (m.level.as_str(), m.text.as_str())).collect::<Vec<_>>());
    console.stop().await?;
    println!("[*] console.stop() listening={}", console.listening());

    let pass = case1 && case2 && case3 && case4 && case5 && case6 && case7;
    println!(
        "\n==== {} ====",
        if pass { "ALL CHECKS PASSED" } else { "SOME CHECKS FAILED" }
    );

    browser.quit().await?;
    if pass {
        Ok(())
    } else {
        Err(drission::Error::msg("console_listen 自验证未通过"))
    }
}
