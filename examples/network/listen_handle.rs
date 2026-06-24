//! DP 风格网络监听句柄 `tab.listen()` 的端到端自验证。
//!
//! 覆盖:`start`(装 hook)/ `wait`(等 1 个包)/ `wait_count`(等 N 个包)/
//! `steps`(长监听流式句柄,后台不丢包)/ `stop`(停止)。
//!
//! 触发用 `fetch('data:...')`:hook 是 JS 层包裹 `fetch`/`XHR`,对 `data:` 同样生效,
//! 因此本例**完全离线、确定性强**(真实跨域网络抓取见 `douyin_listen_long` 等示例)。
//!
//! 运行:`cargo run --example listen_handle --no-default-features --features camoufox`
//! 末行打印 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`,关键校验失败则进程非 0 退出。

use std::time::Duration;

use drission::prelude::*;

/// 触发一次对 `data:` URL 的 fetch;响应体即 `drission-listen-{n}`(供按内容核对)。
async fn fire(tab: &Tab, n: u32) -> drission::Result<()> {
    tab.run_js(&format!(
        "fetch('data:text/plain,drission-listen-{n}').catch(() => {{}}); true"
    ))
    .await?;
    Ok(())
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    println!("[*] 启动 Camoufox(headless)…");
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;

    // ---------- A: start + wait(1) ----------
    // 关键词过滤 "drission-listen":只抓我们触发的这些 data: 请求。
    tab.listen().start(&["drission-listen"]).await?;
    let listening = tab.listen().is_listening().await;
    fire(&tab, 0).await?;
    let pkt = tab.listen().wait().await?;
    let a_ok = listening && pkt.response.body == "drission-listen-0";
    println!(
        "[A] start+wait: listening={listening} body={:?} status={}  (ok={a_ok})",
        pkt.response.body, pkt.response.status
    );

    // ---------- B: wait_count(3) ----------
    for n in 1..=3 {
        fire(&tab, n).await?;
    }
    let pkts = tab
        .listen()
        .wait_count(3, Some(Duration::from_secs(10)))
        .await?;
    let b_ok = pkts.len() == 3
        && pkts
            .iter()
            .all(|p| p.response.body.starts_with("drission-listen"));
    println!("[B] wait_count(3): 抓到 {} 个包  (ok={b_ok})", pkts.len());

    // ---------- C: steps() 长监听流式句柄 ----------
    let stream = tab.listen().steps().await?;
    for n in 10..12 {
        fire(&tab, n).await?;
    }
    let mut step_n = 0;
    while step_n < 2 {
        match stream.next_timeout(Duration::from_secs(10)).await? {
            Some(p) => {
                step_n += 1;
                println!("    steps#{step_n}: body={:?}", p.response.body);
            }
            None => break,
        }
    }
    let c_ok = step_n == 2;
    println!("[C] steps(): 流式抓到 {step_n} 个包  (ok={c_ok})");

    // ---------- stop ----------
    tab.listen().stop().await?;
    let listening_after = tab.listen().is_listening().await;
    println!(
        "[D] stop: is_listening={listening_after}  (ok={})",
        !listening_after
    );

    let pass = a_ok && b_ok && c_ok && !listening_after;
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
        Err(drission::Error::msg("listen_handle 自验证未通过"))
    }
}
