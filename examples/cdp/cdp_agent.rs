//! Agent / wait / 视觉兜底 / 分层池 离线自验证。
//!
//! 运行:`cargo run --example cdp_agent`

use std::time::Duration;

use drission::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const HOME: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Login</title></head>
<body>
<label>用户名 <input id="user" placeholder="用户名"></label>
<button id="login">登录</button>
<div id="dash" hidden>Dashboard</div>
<p><span>视觉登录</span> <button id="ghost" style="width:72px;height:28px"></button></p>
<a id="pop" href="/popup" target="_blank">开弹窗</a>
<script>
document.getElementById('login').onclick=function(){
  document.getElementById('dash').hidden=false;
  document.title='Dashboard';
};
document.getElementById('ghost').onclick=function(){
  document.title='VisualOk';
};
fetch('/api/ping').then(function(){});
</script>
</body></html>"#;

const POPUP: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Popup</title></head>
<body><p>popup-ok</p></body></html>"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(serve(listener));
    let base = format!("http://127.0.0.1:{port}/");

    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(true)).await?;
    let tab = browser.new_tab(Some(&base)).await?;
    tab.wait().doc_loaded(Some(Duration::from_secs(5))).await?;

    let mut failed = false;
    macro_rules! check {
        ($cond:expr, $($arg:tt)*) => {{
            let ok = $cond;
            println!("[{}] {}", if ok { "ok" } else { "FAIL" }, format!($($arg)*));
            if !ok { failed = true; }
        }};
    }

    let net_ok = tab
        .wait()
        .network()
        .url("/api/ping")
        .timeout(Duration::from_secs(5))
        .await?;
    check!(net_ok, "wait().network().url(/api/ping)");

    let idle = tab
        .wait()
        .network()
        .idle()
        .timeout(Duration::from_secs(5))
        .await?;
    check!(idle, "wait().network().idle()");

    let agent = tab.agent();
    agent
        .type_text(Locator::placeholder("用户名"), "alice")
        .await?;
    agent.find("登录按钮").click().await?;
    check!(agent.wait_for("Dashboard").await?, "wait_for Dashboard");

    let pop_wait = tab.wait().popup().timeout(Duration::from_secs(5));
    tab.click("#pop").await?;
    let popup = pop_wait.await?;
    check!(popup.is_some(), "wait().popup()");
    if let Some(p) = popup {
        let t = p.title().await.unwrap_or_default();
        check!(t.contains("Popup"), "弹窗标题");
        let _ = p.close().await;
    }

    tab.get(&base).await?;
    tab.wait().doc_loaded(Some(Duration::from_secs(5))).await?;
    agent.find(Locator::text("视觉登录")).click().await?;
    check!(
        tab.wait()
            .title("VisualOk")
            .timeout(Duration::from_secs(5))
            .await?,
        "视觉兜底点空按钮"
    );

    browser.quit().await?;

    let pool = ChromiumPool::launch(
        ChromiumPoolOptions::new()
            .size(1)
            .tabs_per_worker(1)
            .idle_recycle(Duration::from_secs(60))
            .memory_limit_mb(4096)
            .headless_pool(),
    )
    .await?;
    {
        let ctx = pool.acquire_context("acct_visual").await?;
        let t = ctx.new_tab(Some(&base)).await?;
        t.wait().doc_loaded(Some(Duration::from_secs(5))).await?;
        check!(
            t.title().await.unwrap_or_default().contains("Login"),
            "ContextPool 开页"
        );
    }
    let report = pool.recycle().await?;
    let _ = report.tabs_closed;
    check!(pool.proxies().is_none(), "未配 ProxyPool 时 proxies() 为空");
    let _ = pool.resource_stats().await;
    pool.shutdown().await?;

    if failed {
        std::process::exit(1);
    }
    Ok(())
}

trait HeadlessPool {
    fn headless_pool(self) -> Self;
}

impl HeadlessPool for ChromiumPoolOptions {
    fn headless_pool(self) -> Self {
        self.base_options(ChromiumOptions::new().headless(true))
    }
}

async fn serve(listener: TcpListener) {
    loop {
        let Ok((mut s, _)) = listener.accept().await else {
            continue;
        };
        let mut buf = [0u8; 2048];
        let n = s.read(&mut buf).await.unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        let path = req.split_whitespace().nth(1).unwrap_or("/");
        let (status, ctype, body): (&str, &str, &[u8]) = if path.starts_with("/api/") {
            ("200 OK", "application/json", b"{\"ok\":true}")
        } else if path.starts_with("/popup") {
            ("200 OK", "text/html; charset=utf-8", POPUP.as_bytes())
        } else {
            ("200 OK", "text/html; charset=utf-8", HOME.as_bytes())
        };
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = s.write_all(head.as_bytes()).await;
        let _ = s.write_all(body).await;
    }
}
