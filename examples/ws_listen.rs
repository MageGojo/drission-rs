//! WebSocket 帧监听 `tab.websocket()` 的端到端自验证。
//!
//! 思路:本进程内起一个**本地 WS echo 服务**(`ws://127.0.0.1:<随机端口>`),浏览器页面用
//! `new WebSocket(...)` 连上去并收发文本/二进制帧;监听句柄 `tab.websocket()` 应抓到**双向**的帧。
//! 因为只连 `127.0.0.1`,本例**完全离线、确定性强**(不依赖外网)。
//!
//! 服务约定:收到文本 `t` 回 `echo:t`;收到二进制原样回。于是可逐项核对:
//! - 发出文本 `hello-text`(opcode 1)
//! - 收到文本 `echo:hello-text`(opcode 1)
//! - 发出二进制 `[1,2,3,4]`(opcode 2 → data 为 base64,`bytes()` 还原)
//! - 收到二进制 `[1,2,3,4]`
//! - `sockets()` 含该连接 URL;`stop()` 后不再监听。
//!
//! 运行:`cargo run --example ws_listen`
//! 末行打印 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`,关键校验失败则进程非 0 退出。

use std::time::Duration;

use drission::prelude::*;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

/// 把一段 HTML 包成 `data:text/html,...`(百分号编码,保证是合法 URL)。
fn data_url_html(html: &str) -> String {
    let mut enc = String::with_capacity(html.len() * 2);
    for &b in html.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            enc.push(b as char);
        } else {
            enc.push('%');
            enc.push_str(&format!("{b:02X}"));
        }
    }
    format!("data:text/html,{enc}")
}

/// 本地 WS echo 服务:文本回 `echo:<原文>`,二进制原样回。
async fn run_echo_server(listener: TcpListener) {
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(async move {
            let Ok(mut ws) = accept_async(stream).await else {
                return;
            };
            while let Some(Ok(msg)) = ws.next().await {
                if msg.is_text() {
                    let t = msg.into_text().unwrap_or_default();
                    let _ = ws.send(Message::text(format!("echo:{}", t.as_str()))).await;
                } else if msg.is_binary() {
                    let _ = ws.send(Message::binary(msg.into_data())).await;
                } else if msg.is_close() {
                    break;
                }
            }
        });
    }
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    // 起本地 echo 服务(随机端口)。
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(run_echo_server(listener));
    println!("[*] 本地 WS echo 服务: ws://127.0.0.1:{port}");

    println!("[*] 启动 Camoufox(headless)…");
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;

    // 关键:在建立连接之前 start。只抓 127.0.0.1 的连接。
    let ws = tab.websocket();
    ws.start_with(WsFilter::new().url_contains("127.0.0.1"))
        .await?;
    let listening = ws.listening();

    // 导航到一个**自包含**的 data: 页:其内联脚本在一个稳定上下文里建立连接,onopen 后即发帧。
    // (用 about:blank + 跨次 run_js 会因执行上下文被重建而丢失 window 状态。)
    let html = format!(
        r#"<!doctype html><meta charset=utf-8><title>ws</title><script>
          var s = new WebSocket('ws://127.0.0.1:{port}');
          s.binaryType = 'arraybuffer';
          s.onopen = function(){{
            s.send('hello-text');
            s.send(new Uint8Array([1,2,3,4]).buffer);
          }};
        </script>"#
    );
    tab.get(&data_url_html(&html)).await?;

    // 收集 4 帧(发文本/收文本/发二进制/收二进制),最多等 12s。
    let msgs = ws.wait_count(4, Some(Duration::from_secs(12))).await?;
    println!("[*] 抓到 {} 帧:", msgs.len());
    for m in &msgs {
        let body = m.text().unwrap_or_else(|| format!("{:?}", m.bytes()));
        println!("    {} {} -> {body}", m.direction.as_str(), m.opcode_name());
    }

    let text = |dir| {
        msgs.iter()
            .find(|m| m.direction == dir && m.is_text())
            .and_then(|m| m.text())
    };
    let bin = |dir| {
        msgs.iter()
            .any(|m| m.direction == dir && m.is_binary() && m.bytes() == [1u8, 2, 3, 4])
    };
    let socks = ws.sockets().await;
    ws.stop().await?;

    // 逐项核对(任一失败则进程非 0 退出)。
    let checks = [
        ("start 后在监听", listening),
        (
            "发出文本 hello-text",
            text(WsDirection::Sent).as_deref() == Some("hello-text"),
        ),
        (
            "收到文本 echo:hello-text",
            text(WsDirection::Received).as_deref() == Some("echo:hello-text"),
        ),
        ("发出二进制 [1,2,3,4]", bin(WsDirection::Sent)),
        ("收到二进制 [1,2,3,4]", bin(WsDirection::Received)),
        (
            "sockets() 含连接 URL",
            socks
                .iter()
                .any(|s| s.url.contains(&format!("127.0.0.1:{port}"))),
        ),
        ("stop 后不再监听", !ws.listening()),
    ];
    let pass = checks.iter().all(|&(_, ok)| ok);
    for (name, ok) in checks {
        println!("    [{}] {name}", if ok { "PASS" } else { "FAIL" });
    }
    println!(
        "\n==== {} ====",
        if pass {
            "ALL CHECKS PASSED"
        } else {
            "SOME CHECKS FAILED"
        }
    );

    browser.quit().await?;
    pass.then_some(())
        .ok_or_else(|| drission::Error::msg("ws_listen 自验证未通过"))
}
