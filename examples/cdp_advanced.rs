//! CDP 后端**深化能力**端到端自验证(完全离线:进程内起一个极简 HTTP 服务,Chrome 访问 localhost)。
//!
//! 覆盖:① 元素句柄(`ele`/`eles`/相对定位/读文本属性)② **原生可信点击**(`isTrusted=true`)
//! ③ **拟人逐字符输入** `input_human` ④ **网络监听**(原生 Network 域 + getResponseBody)
//! ⑤ **请求拦截**(Fetch 域 `fulfill` 伪造响应,页面实收伪造内容)。
//!
//! 运行:`cargo run --example cdp_advanced`(无头默认;`HL=0` 开窗口;`CHROME_BIN` 指定浏览器)。
//! 任一关键校验失败即 panic 非 0 退出;全部通过打印 `ALL CHECKS PASSED`。

use std::time::Duration;

use drission::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::{Instant, sleep};

const PAGE_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>CDP Advanced</title></head>
<body>
<h1 id="title">CDP 高级能力</h1>
<ul id="list"><li>one</li><li>two</li><li>three</li></ul>
<input id="name" type="text" />
<button id="go">GO</button>
<div id="out">idle</div>
<script>
  window.__trusted = null;
  document.getElementById('go').addEventListener('click', async function(e){
    window.__trusted = e.isTrusted;
    try {
      const r = await fetch('/api/data');
      const j = await r.json();
      document.getElementById('out').textContent = 'got:' + j.hello;
    } catch (err) { document.getElementById('out').textContent = 'err:' + err; }
  });
</script>
</body></html>"#;

const API_JSON: &str = r#"{"hello":"cdp","n":42}"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    // ── 进程内极简 HTTP 服务(localhost,不受出网拦截影响)──────────────────
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(serve(listener));
    let base = format!("http://127.0.0.1:{port}");
    println!("[*] 本地服务: {base}");

    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    println!("[*] 启动 Chrome(headless={headless})");
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;
    tab.get(&format!("{base}/")).await?;

    let mut failed = false;
    macro_rules! check {
        ($cond:expr, $($arg:tt)*) => {{
            let ok = $cond;
            println!("[{}] {}", if ok { "ok" } else { "FAIL" }, format!($($arg)*));
            if !ok { failed = true; }
        }};
    }

    // ── ① 元素句柄:读文本 / eles / 相对定位 / 属性 ─────────────────────────
    let title = tab.ele("#title").await?;
    check!(title.text().await? == "CDP 高级能力", "ele(#title).text");

    let lis = tab.eles("css:li").await?;
    check!(lis.len() == 3, "eles(li) == 3 (实得 {})", lis.len());
    check!(lis[0].text().await? == "one", "li[0].text == one");
    let second = lis[0].next().await?;
    check!(second.text().await? == "two", "li[0].next().text == two");
    let list = tab.ele("#list").await?;
    check!(list.children().await?.len() == 3, "#list.children == 3");
    let go = tab.ele("#go").await?;
    check!(go.tag().await? == "button", "ele(#go).tag == button");

    // ── ② 拟人逐字符输入 ───────────────────────────────────────────────────
    let name = tab.ele("#name").await?;
    name.input_human("drission").await?;
    check!(
        name.value().await? == "drission",
        "input_human → value == drission"
    );

    // ── ③ 原生可信点击 + ④ 网络监听(原生 Network 域 + getResponseBody)─────
    let listen = tab.listen();
    listen.start_xhr(&["/api/"]).await?;
    check!(listen.is_listening().await, "listen.is_listening == true");

    tab.ele("#go").await?.click().await?; // 可信点击触发 fetch
    let pkt = listen.wait(Some(Duration::from_secs(5))).await?;
    match pkt {
        Some(p) => {
            check!(
                p.url.contains("/api/data"),
                "监听到 /api/data(实得 {})",
                p.url
            );
            check!(
                p.response.status == 200,
                "响应状态 200(实得 {})",
                p.response.status
            );
            let body_hello = p
                .json()
                .and_then(|j| j["hello"].as_str().map(str::to_string));
            check!(
                body_hello.as_deref() == Some("cdp"),
                "响应体 hello==cdp(实得 {:?})",
                body_hello
            );
        }
        None => check!(false, "监听超时,没抓到 /api/data"),
    }
    // 可信点击校验:页面 click handler 记录的 isTrusted。
    let trusted = tab.run_js("window.__trusted").await?;
    check!(
        trusted.as_bool() == Some(true),
        "click 的 isTrusted == true(实得 {trusted})"
    );
    let out_real = poll_text(&tab, "#out", "got:cdp").await?;
    check!(
        out_real == "got:cdp",
        "真实 fetch 后 #out == got:cdp(实得 {out_real})"
    );
    listen.stop().await?;

    // ── ⑤ 请求拦截(Fetch 域:fulfill 伪造响应,页面实收伪造内容)──────────
    tab.run_js("document.getElementById('out').textContent='idle'")
        .await?;
    let intercept = tab.intercept();
    intercept.start_xhr(&["/api/data"]).await?;
    tab.ele("#go").await?.click().await?; // 再次点击触发 fetch(将被拦截)
    let req = intercept.next(Some(Duration::from_secs(5))).await?;
    match req {
        Some(r) => {
            check!(
                r.url.contains("/api/data"),
                "拦截到 /api/data(实得 {})",
                r.url
            );
            r.fulfill(
                200,
                vec![
                    ("Content-Type".into(), "application/json".into()),
                    ("Access-Control-Allow-Origin".into(), "*".into()),
                ],
                r#"{"hello":"FAKE"}"#,
            )
            .await?;
        }
        None => check!(false, "拦截超时,没拦到 /api/data"),
    }
    let out_fake = poll_text(&tab, "#out", "got:FAKE").await?;
    check!(
        out_fake == "got:FAKE",
        "拦截 fulfill 后 #out == got:FAKE(实得 {out_fake})"
    );
    intercept.stop().await?;

    browser.quit().await?;

    if failed {
        eprintln!("==== 有校验未通过 ====");
        std::process::exit(1);
    }
    println!("ALL CHECKS PASSED");
    Ok(())
}

/// 轮询某元素文本直到等于 `want` 或超时(2s);返回最终读到的文本。
async fn poll_text(tab: &ChromiumTab, selector: &str, want: &str) -> drission::Result<String> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let t = tab.ele(selector).await?.text().await?;
        if t == want || Instant::now() >= deadline {
            return Ok(t);
        }
        sleep(Duration::from_millis(60)).await;
    }
}

/// 极简 HTTP/1.1 服务:`/api/data` 返回 JSON,其余返回页面 HTML。每连接应答后关闭。
async fn serve(listener: TcpListener) {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            break;
        };
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let path = req
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/");
            let (ctype, body) = if path.starts_with("/api/data") {
                ("application/json", API_JSON)
            } else {
                ("text/html; charset=utf-8", PAGE_HTML)
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{}",
                ctype,
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
    }
}
