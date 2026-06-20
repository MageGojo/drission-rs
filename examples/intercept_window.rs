//! `tab.intercept()` 句柄 + 窗口尺寸句柄 `tab.set().window()` 端到端自验证(完全离线)。
//!
//! - **拦截句柄**:`about:blank` 里发跨源 `fetch`,用 `intercept().next()` 取到后 `fulfill` 伪造
//!   带 CORS 头的 JSON 响应(不走真实网络),页面 JS 读到伪造内容;`is_intercepting()` 前后翻转。
//! - **窗口句柄**:`tab.set().window().size(w,h)` 经 `Page.setViewportSize` 设尺寸,读 `innerWidth`
//!   校验;`max()` 铺满可用屏幕。**注意**:Firefox/Juggler 不支持最小化/全屏/移动主窗口(见库文档)。
//!
//! 运行:`cargo run --example intercept_window`
//!
//! 末尾打印 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`,关键校验失败则进程非 0 退出。

use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    println!("[*] 启动 Camoufox(headless)…");
    let browser = Browser::launch(
        BrowserOptions::new()
            .headless(true)
            .bypass_csp(true)
            .ignore_https_errors(true),
    )
    .await?;
    let tab = browser.latest_tab().await?;
    tab.get("about:blank").await?;

    // ---------- 窗口尺寸句柄 ----------
    tab.set().window().size(800, 600).await?;
    // setViewportSize 后视口应变;轮询等一拍(协议返回后视口已生效,但读取保险起见容忍 1~2 拍)。
    let mut iw = 0.0;
    let mut ih = 0.0;
    for _ in 0..20 {
        let v = tab.run_js("[window.innerWidth, window.innerHeight]").await?;
        iw = v.get(0).and_then(|x| x.as_f64()).unwrap_or(0.0);
        ih = v.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0);
        if iw as u32 == 800 && ih as u32 == 600 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let size_ok = iw as u32 == 800 && ih as u32 == 600;
    println!("[1] window().size(800,600) → innerWidth={iw} innerHeight={ih} (ok={size_ok})");

    // max():铺满可用屏幕。
    let avail = tab.run_js("[screen.availWidth, screen.availHeight]").await?;
    let aw = avail.get(0).and_then(|x| x.as_f64()).unwrap_or(0.0) as u32;
    tab.set().window().max().await?;
    let mut max_w = 0u32;
    for _ in 0..20 {
        let v = tab.run_js("window.innerWidth").await?;
        max_w = v.as_f64().unwrap_or(0.0) as u32;
        if max_w == aw && aw > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let max_ok = aw > 0 && max_w == aw;
    println!("[2] window().max() → innerWidth={max_w} (availWidth={aw}, ok={max_ok})");

    // ---------- 拦截句柄 ----------
    let before = tab.intercept().is_intercepting().await;
    tab.intercept().start_xhr(&["/api/"]).await?;
    let during = tab.intercept().is_intercepting().await;

    // 跨源 fetch:被拦截 + fulfill(带 CORS 头)即可在 about:blank 读到,不走真实网络。
    tab.run_js(
        "window.__hijacked = null; \
         fetch('http://drission.test/api/profile') \
           .then(r => r.json()).then(j => window.__hijacked = j) \
           .catch(e => window.__hijacked = { error: String(e) }); true",
    )
    .await?;

    let req = tab.intercept().next().await?;
    println!("[3] 拦截到: {} {} [{}]", req.method, req.url, req.resource_type);
    req.fulfill(
        200,
        vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("access-control-allow-origin".to_string(), "*".to_string()),
        ],
        r#"{"id":1,"name":"drission","hijacked":true}"#,
    )
    .await?;

    let mut got = serde_json::Value::Null;
    for _ in 0..40 {
        let v = tab.run_js("window.__hijacked").await?;
        if !v.is_null() {
            got = v;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let intercept_ok = got.get("name").and_then(|v| v.as_str()) == Some("drission")
        && got.get("hijacked").and_then(|v| v.as_bool()) == Some(true);
    println!("[4] 页面读到(已伪造)={got} (ok={intercept_ok})");

    tab.intercept().stop().await?;
    let after = tab.intercept().is_intercepting().await;
    let state_ok = !before && during && !after;
    println!("[5] is_intercepting 前={before} 中={during} 后={after} (ok={state_ok})");

    let pass = size_ok && max_ok && intercept_ok && state_ok;
    println!(
        "\n==== {} ====",
        if pass {
            "ALL CHECKS PASSED"
        } else {
            "SOME CHECKS FAILED"
        }
    );

    browser.quit().await?;
    if pass {
        Ok(())
    } else {
        Err(drission::Error::msg("intercept_window 自验证未通过"))
    }
}
