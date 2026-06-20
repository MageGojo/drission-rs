//! 请求拦截示例:**伪造响应** / 中止 / 改写放行。
//!
//! 这里用 `fulfill` 伪造一个本不存在接口(`/api/profile`)的响应:页面里的 `fetch`
//! 实际拿到的就是我们伪造的 JSON,请求并不会真正发往服务器。
//! 只拦截匹配 `"/api/"` 的请求,其余请求由库自动放行,页面照常加载。
//!
//! 运行:`cargo run --example intercept --no-default-features --features camoufox`

use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let browser = Browser::launch(
        BrowserOptions::new()
            .headless(true)
            .bypass_csp(true)
            .ignore_https_errors(true),
    )
    .await?;
    let tab = browser.latest_tab().await?;
    tab.get("https://example.com/").await?;

    // 只拦截发往 /api/ 的 XHR/fetch,其余自动放行。
    tab.intercept_xhr(&["/api/"]).await?;

    // 页面里发起一个对不存在接口的 fetch,把结果写入全局变量便于稍后读取。
    tab.run_js(
        "window.__hijacked = null; \
         fetch('https://example.com/api/profile') \
           .then(r => r.json()).then(j => window.__hijacked = j) \
           .catch(e => window.__hijacked = { error: String(e) }); true",
    )
    .await?;

    // 取到被拦请求,用伪造 JSON 直接满足它(不真正发往服务器)。
    let req = tab.intercept_next().await?;
    println!("拦截到: {} {} [{}]", req.method, req.url, req.resource_type);
    req.fulfill(
        200,
        vec![("content-type".to_string(), "application/json".to_string())],
        r#"{"id":1,"name":"drission","hijacked":true}"#,
    )
    .await?;

    // 轮询读取页面拿到的(已被伪造的)响应。
    let mut got = serde_json::Value::Null;
    for _ in 0..30 {
        let v = tab.run_js("window.__hijacked").await?;
        if !v.is_null() {
            got = v;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!("页面 JS 实际拿到的响应(已被伪造): {got}");

    tab.intercept_stop().await?;

    // 其他决策方式(仅注释说明,本例未实跑):
    //   req.abort("blockedbyclient").await?;  // 直接中止,适合拦广告/统计/打点
    //   req.resume().await?;                   // 原样放行
    //   req.resume_with(                       // 改写后放行
    //       ResumeOptions::new().headers(vec![("X-Inject".into(), "1".into())]),
    //   ).await?;

    browser.quit().await?;
    println!("完成。");
    Ok(())
}
