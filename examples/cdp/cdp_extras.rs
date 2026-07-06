//! CDP「标配补齐」端到端自验证(完全离线:进程内起极简 HTTP 服务,Chrome 访问 localhost)。
//!
//! 覆盖对标 Playwright/Puppeteer/DrissionPage 的通用能力:
//! ① `set_content` 直接灌 HTML  ② 媒体模拟(深色 `prefers-color-scheme`)
//! ③ localStorage 便捷读写  ④ `wait().network_idle` 网络空闲等待
//! ⑤ 设备模拟(`set().device`,移动端 UA + 视口 + 触摸)  ⑥ `save_pdf` 导出 PDF(无头)
//! ⑦ `save_mhtml` 整页快照  ⑧ HAR 录制 `har_record`(落盘,符合"请求先保存")
//! ⑨ `expose_function` 页面回调 Rust  ⑩ `offline` 离线模拟。
//!
//! 运行:`cargo run --example cdp_extras --features cdp`(无头默认,PDF 需无头;`CHROME_BIN` 可指定浏览器)。
//! 任一关键校验失败即非 0 退出;全部通过打印 `ALL CHECKS PASSED`。

use std::time::Duration;

use drission::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const PAGE_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Extras</title></head>
<body><h1 id="t">标配补齐</h1><div id="out">idle</div>
<script>
  fetch('/api/data').then(r=>r.json()).then(j=>{document.getElementById('out').textContent='got:'+j.hello;});
</script></body></html>"#;

const API_JSON: &str = r#"{"hello":"extras"}"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(serve(listener));
    let base = format!("http://127.0.0.1:{port}");
    println!("[*] 本地服务: {base}");

    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;

    let mut failed = false;
    macro_rules! check {
        ($cond:expr, $($arg:tt)*) => {{
            let ok = $cond;
            println!("[{}] {}", if ok { "ok" } else { "FAIL" }, format!($($arg)*));
            if !ok { failed = true; }
        }};
    }

    // ── ① set_content ─────────────────────────────────────────────────────
    tab.set_content("<h1 id='x'>HELLO-SET-CONTENT</h1>").await?;
    let x = tab.ele("#x").await?.text().await?;
    check!(x == "HELLO-SET-CONTENT", "set_content → #x == {x}");

    // ── ② 媒体模拟:深色模式 ───────────────────────────────────────────────
    tab.set().emulate_dark(true).await?;
    let dark = tab
        .run_js("matchMedia('(prefers-color-scheme: dark)').matches")
        .await?;
    check!(
        dark.as_bool() == Some(true),
        "emulate_dark → matchMedia dark (实得 {dark})"
    );
    tab.set().emulate_dark(false).await?;

    // ── ③ localStorage 便捷读写 ───────────────────────────────────────────
    tab.get(&format!("{base}/")).await?; // 需真实 origin 才有 storage
    tab.set().local_storage_set("k", "v123").await?;
    let got = tab.set().local_storage_get("k").await?;
    check!(
        got.as_deref() == Some("v123"),
        "localStorage roundtrip (实得 {got:?})"
    );
    let none = tab.set().local_storage_get("missing").await?;
    check!(none.is_none(), "localStorage 缺失键 == None");

    // ── ④ network_idle ────────────────────────────────────────────────────
    let idle = tab
        .wait()
        .network_idle(0.3, Some(Duration::from_secs(5)))
        .await?;
    check!(idle, "network_idle 在超时内达成");
    let out = tab.ele("#out").await?.text().await?;
    check!(out == "got:extras", "页面 fetch 完成 → #out == {out}");

    // ── ⑤ 设备模拟(移动端 UA)─────────────────────────────────────────────
    tab.set().device(&Device::iphone_13()).await?;
    let ua = tab.run_js("navigator.userAgent").await?;
    check!(
        ua.as_str().unwrap_or_default().contains("iPhone"),
        "device(iPhone) → UA 含 iPhone"
    );
    tab.set().clear_device().await?;

    // ── ⑥ save_pdf(仅无头可靠)─────────────────────────────────────────────
    if headless {
        let pdf = std::env::temp_dir().join("drission_extras.pdf");
        let p = tab.save_pdf(&pdf, &PdfOptions::default()).await?;
        let sz = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        check!(sz > 1000, "save_pdf 产出非空 PDF(实得 {sz} 字节)");
        let head_ok = std::fs::read(&p)
            .map(|b| b.starts_with(b"%PDF"))
            .unwrap_or(false);
        check!(head_ok, "PDF 文件头为 %PDF");
        let _ = std::fs::remove_file(&p);
    }

    // ── ⑦ save_mhtml ──────────────────────────────────────────────────────
    let mht = std::env::temp_dir().join("drission_extras.mhtml");
    let p = tab.save_mhtml(&mht).await?;
    let mz = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    check!(mz > 100, "save_mhtml 产出快照(实得 {mz} 字节)");
    let _ = std::fs::remove_file(&p);

    // ── ⑧ HAR 录制(落盘)──────────────────────────────────────────────────
    let rec = tab.har_record().await?;
    tab.get(&format!("{base}/")).await?;
    let _ = tab
        .wait()
        .network_idle(0.3, Some(Duration::from_secs(5)))
        .await?;
    let har = rec.stop().await?;
    check!(
        har.entry_count() > 0,
        "HAR 录到条目(实得 {})",
        har.entry_count()
    );
    let har_path = std::env::temp_dir().join("drission_extras.har");
    har.save(&har_path).await?;
    check!(har_path.exists(), "HAR 落盘成功");
    let _ = std::fs::remove_file(&har_path);

    // ── ⑨ expose_function(页面回调 Rust)─────────────────────────────────
    let _guard = tab
        .expose_function("addRust", |args| {
            let sum: f64 = args.iter().filter_map(|v| v.as_f64()).sum();
            Ok(serde_json::json!(sum))
        })
        .await?;
    // 重新灌一个干净文档让 stub 在新文档生效,再调用。
    tab.set_content("<h1>expose</h1>").await?;
    let r = tab.run_js("window.addRust(2,3)").await?;
    check!(
        r.as_f64() == Some(5.0),
        "expose_function addRust(2,3)==5 (实得 {r})"
    );

    // ── ⑩ offline 离线模拟 ────────────────────────────────────────────────
    tab.set().offline(true).await?;
    let off = tab
        .run_js("fetch('/api/data').then(()=>'ok').catch(()=>'blocked')")
        .await?;
    check!(
        off.as_str() == Some("blocked"),
        "offline → fetch 被阻断 (实得 {off})"
    );
    tab.set().offline(false).await?;

    // ── ⑪ HAR 回放(用第 ⑧ 步录的 har 重放;未命中 Abort → 可证明路由生效)────────
    let player = tab
        .route_from_har_log(&har, &HarReplayOptions::default())
        .await?;
    tab.get(&format!("{base}/")).await?; // "/" 在 HAR 中 → 由 HAR 满足
    let t = tab.ele("#t").await?.text().await?;
    check!(t == "标配补齐", "HAR 回放命中 → #t == {t}");
    let miss = tab
        .run_js("fetch('/not-in-har').then(()=>'ok').catch(()=>'blocked')")
        .await?; // 不在 HAR → Abort → fetch 被拒
    check!(
        miss.as_str() == Some("blocked"),
        "HAR 未命中(Abort)→ fetch 被拒(证明路由生效,实得 {miss})"
    );
    player.stop().await?;

    // ── ⑫ wait.new_tab(可信点击 target=_blank 链接 → 捕获弹窗为新 Tab)──────────
    tab.set_content(&format!(
        "<a id='lnk' href='{base}/' target='_blank'>open</a>"
    ))
    .await?;
    let waiter = tab.wait();
    let (popup, _) = tokio::join!(waiter.new_tab(Some(Duration::from_secs(8))), async {
        if let Ok(el) = tab.ele("#lnk").await {
            let _ = el.click().await; // 可信点击 → 绕过弹窗拦截
        }
    });
    match popup? {
        Some(pt) => {
            let _ = pt.wait().doc_loaded(Some(Duration::from_secs(5))).await;
            let tt = pt.ele("#t").await?.text().await?;
            check!(tt == "标配补齐", "wait.new_tab → 弹窗 #t == {tt}");
            let _ = pt.close().await;
        }
        None => check!(false, "wait.new_tab 未捕获弹窗"),
    }

    browser.quit().await?;

    if failed {
        eprintln!("==== 有校验未通过 ====");
        std::process::exit(1);
    }
    println!("ALL CHECKS PASSED");
    Ok(())
}

/// 极简 HTTP/1.1 服务:`/api/data` 返回 JSON,其余返回页面 HTML。
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
