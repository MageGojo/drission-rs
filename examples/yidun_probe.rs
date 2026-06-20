//! 易盾点选**请求侦察**:监听网络,看①验证码图片是什么**格式/哪来的**、②提示**文字怎么下发**。
//!
//! 运行:`cargo run --example yidun_probe --features cdp`(默认有头;`HL=1` 无头)。
//! 用 CDP 原生 `Network.*` 抓相关域(dun.163 / 127.net / nosdn …)的请求 + 响应体并打印。

use std::time::Duration;

use drission::prelude::*;

const URL: &str = "https://dun.163.com/trial/picture-click";

const TRIGGER_JS: &str = r#"(() => {
  const want = /在线体验|立即体验|点击验证|验证码|体验|验证/;
  const els = [...document.querySelectorAll('button,a,div,span,input')].filter(e => e.offsetParent !== null);
  for (const e of els) { const t = (e.innerText||e.value||'').trim(); if (want.test(t) && t.length <= 16) { e.click(); return 'clicked:'+t; } }
  return 'no-trigger';
})()"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = matches!(
        std::env::var("HL").ok().as_deref(),
        Some("1") | Some("true")
    );
    let browser = ChromiumBrowser::launch(
        ChromiumOptions::new()
            .headless(headless)
            .window_size(1200, 900),
    )
    .await?;
    let tab = browser.new_tab(Some(URL)).await?;
    tokio::time::sleep(Duration::from_secs(4)).await;

    // 只抓验证码相关域,减少噪声与 getResponseBody 负担。
    tab.listen()
        .start(&["dun.163", "127.net", "126.net", "nosdn", "captcha", "yidun"])
        .await?;

    // 触发挑战 → 点验证按钮 → 再点刷新,确保抓到“新一题”的 api/get + 图片请求。
    let _ = tab.run_js(TRIGGER_JS).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    for s in ["css:.yidun_control", "css:.yidun"] {
        if let Ok(e) = tab.ele(s).await
            && e.click().await.is_ok()
        {
            break;
        }
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    if let Ok(e) = tab.ele("css:.yidun_refresh").await {
        let _ = e.click().await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;

    let pkts = tab
        .listen()
        .wait_count(400, Some(Duration::from_secs(6)))
        .await?;
    tab.listen().stop().await?;

    println!("\n=== 抓到 {} 个验证码相关请求 ===", pkts.len());
    for p in &pkts {
        let ct = p
            .response
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let short: String = p.url.chars().take(130).collect();
        println!(
            "\n[{} {}] ({}) {}",
            p.method, p.response.status, p.resource_type, short
        );
        println!("    content-type: {ct}");
        if ct.contains("image") {
            let b64 = p.response.body_base64.len();
            println!("    >> 图片:格式={ct}  体积≈{} bytes", b64 * 3 / 4);
        } else if ct.contains("json")
            || ct.contains("javascript")
            || p.response.body.trim_start().starts_with('{')
        {
            let body: String = p.response.body.chars().take(1600).collect();
            println!("    >> body({} 字节):\n{}", p.response.body.len(), body);
        }
    }

    if !headless {
        tokio::time::sleep(Duration::from_secs(20)).await;
    }
    browser.quit().await?;
    Ok(())
}
