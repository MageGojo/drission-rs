//! Context / Profile / 网络门面离线自验证。
//!
//! 运行:`cargo run --example cdp_context`(无头默认;`HL=0` 开窗口)。
//! 校验:两个命名 Context cookie 不串;network().filter().mock() 页面实收伪造 JSON。

use std::time::Duration;

use drission::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>ctx</title></head>
<body>
<div id="who">?</div>
<div id="api">idle</div>
<script>
document.cookie.split(';').forEach(function(c){
  var p=c.trim().split('=');
  if(p[0]==='acct') document.getElementById('who').textContent=p[1];
});
fetch('/api/me').then(r=>r.json()).then(j=>{
  document.getElementById('api').textContent=j.hello;
}).catch(e=>{ document.getElementById('api').textContent='err'; });
</script>
</body></html>"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(serve(listener));
    let base = format!("http://127.0.0.1:{port}");

    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;

    let mut failed = false;
    macro_rules! check {
        ($cond:expr, $($arg:tt)*) => {{
            let ok = $cond;
            println!("[{}] {}", if ok { "ok" } else { "FAIL" }, format!($($arg)*));
            if !ok { failed = true; }
        }};
    }

    let a = browser.context("account_001").await?;
    let b = browser.context("account_002").await?;
    check!(a.id() != b.id(), "两个 context id 不同");
    let a2 = browser.context("account_001").await?;
    check!(a.id() == a2.id(), "同名 context 复用");

    a.set_cookies(vec![CookieParam {
        name: "acct".into(),
        value: "u1".into(),
        url: Some(base.clone()),
        domain: None,
        path: Some("/".into()),
        secure: None,
        http_only: None,
        expires: None,
    }])
    .await?;
    b.set_cookies(vec![CookieParam {
        name: "acct".into(),
        value: "u2".into(),
        url: Some(base.clone()),
        domain: None,
        path: Some("/".into()),
        secure: None,
        http_only: None,
        expires: None,
    }])
    .await?;

    let ta = a.new_tab(None).await?;
    let tb = b.new_tab(None).await?;
    ta.get(&format!("{base}/")).await?;
    tb.get(&format!("{base}/")).await?;
    check!(ta.ele_text("#who").await?.as_deref() == Some("u1"), "context A cookie=u1");
    check!(tb.ele_text("#who").await?.as_deref() == Some("u2"), "context B cookie=u2");

    let route = ta
        .network()
        .filter("/api/me")
        .method("GET")
        .mock(200, vec![("content-type".into(), "application/json".into())], r#"{"hello":"mocked"}"#)
        .await?;
    ta.get(&format!("{base}/")).await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    check!(
        ta.ele_text("#api").await?.as_deref() == Some("mocked"),
        "network().mock 页面实收"
    );
    route.stop().await?;

    let root = std::env::temp_dir().join(format!("drs-ctx-profile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let profiles = ProfileManager::open(&root)?;
    let lease = profiles.acquire("user_001").await?;
    check!(lease.id() == "user_001", "profile acquire id");
    check!(profiles.list()?.contains(&"user_001".into()), "profile list");
    drop(lease);
    let _ = std::fs::remove_dir_all(&root);

    let _ = a.close().await;
    let _ = b.close().await;
    browser.quit().await?;
    if failed {
        panic!("cdp_context checks failed");
    }
    println!("ALL CHECKS PASSED");
    Ok(())
}

async fn serve(listener: TcpListener) {
    loop {
        let Ok((mut s, _)) = listener.accept().await else {
            continue;
        };
        tokio::spawn(async move {
            let mut buf = vec![0u8; 1024];
            let _ = s.read(&mut buf).await;
            let req = String::from_utf8_lossy(&buf);
            let (status, ctype, body): (&str, &str, &[u8]) = if req.starts_with("GET /api/me") {
                ("200 OK", "application/json", br#"{"hello":"real"}"#)
            } else {
                ("200 OK", "text/html; charset=utf-8", PAGE.as_bytes())
            };
            let head = format!(
                "HTTP/1.1 {status}\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes()).await;
            let _ = s.write_all(body).await;
        });
    }
}
