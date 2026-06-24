//! Camoufox `tab.wait().new_tab()` 端到端自验证(完全离线:进程内极简 HTTP 服务)。
//!
//! 可信点击一个 `target=_blank` 链接弹出新标签,用 `tab.wait().new_tab()` 捕获为可驱动的新 `Tab`,
//! 并读取弹窗页面元素验证。对标 Playwright `expect_popup` / DrissionPage `tab.wait.new_tab`。
//!
//! 运行:`cargo run --example new_tab --no-default-features --features camoufox`
//! 末尾打印 `ALL CHECKS PASSED`;关键校验失败则非 0 退出。

use std::time::Duration;

use drission::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>opener</title></head>
<body><h1 id="t">标配补齐</h1><a id="lnk" href="/" target="_blank">open</a></body></html>"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(serve(listener));
    let base = format!("http://127.0.0.1:{port}");
    println!("[*] 本地服务: {base}");

    println!("[*] 启动 Camoufox(headless)…");
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;
    tab.get(&base).await?;

    let mut failed = false;

    // 先调用 new_tab(内部即订阅事件),再可信点击触发弹窗 —— 二者并发。
    let waiter = tab.wait();
    let (popup, _) = tokio::join!(waiter.new_tab(Some(Duration::from_secs(10))), async {
        if let Ok(el) = tab.ele("#lnk").await {
            let _ = el.click().await; // 可信点击 → 绕过弹窗拦截
        }
    });

    match popup? {
        Some(pt) => {
            let _ = pt.wait().doc_loaded(Some(Duration::from_secs(5))).await;
            let t = pt.ele("#t").await?.text().await?;
            let ok = t == "标配补齐";
            println!(
                "[{}] wait.new_tab → 弹窗 #t == {t}",
                if ok { "ok" } else { "FAIL" }
            );
            if !ok {
                failed = true;
            }
            let _ = pt.close().await;
        }
        None => {
            println!("[FAIL] wait.new_tab 未捕获弹窗");
            failed = true;
        }
    }

    browser.quit().await?;

    if failed {
        eprintln!("==== 有校验未通过 ====");
        std::process::exit(1);
    }
    println!("ALL CHECKS PASSED");
    Ok(())
}

/// 极简 HTTP/1.1 服务:任何路径都返回同一页面(含 `target=_blank` 链接)。
async fn serve(listener: TcpListener) {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            break;
        };
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                PAGE.len(),
                PAGE
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
    }
}
