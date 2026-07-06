//! **CF Turnstile 真实流程抓取(Camoufox 能过的基准)** —— 给纯协议 diff 用。
//!
//! 用能过盾的 Camoufox 后端跑完整 exa Turnstile,`tab.listen()` 抓全量
//! `challenge-platform` 流量:几轮请求、打哪些端点、每轮 payload 大小、响应、token 从哪出。
//! 对比我们 Node-VM(cf-protocol-poc/run.js)只打一轮 `chl_api_m` 的结构差异。
//!
//! 运行:`cargo run --example cf_flow_capture --features camoufox`(有头默认;`HEADLESS=1` 无头)。
//! 产物:`cf_flow_capture.txt`(save-first)。

use std::time::Duration;

use drission::prelude::*;

const URL: &str = "https://auth.exa.ai/?callbackUrl=https%3A%2F%2Fdashboard.exa.ai%2F";
const EMAIL: &str = "12341423@gmail.com";
const TOKEN_JS: &str =
    "(()=>{const e=document.querySelector('[name=cf-turnstile-response]');return e?e.value:'';})()";

fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = matches!(
        std::env::var("HEADLESS").ok().as_deref(),
        Some("1") | Some("true")
    );
    println!("[*] CF flow capture(Camoufox 基准)→ {URL}(headless={headless})");

    let browser = Browser::launch(
        BrowserOptions::new()
            .headless(headless)
            .window_size(1280, 800),
    )
    .await?;
    let tab = browser.latest_tab().await?;

    let listen = tab.listen();
    // 全量抓(空关键词=匹配所有),便于看完整序列与跨域 iframe 是否可见
    listen.start(&[] as &[&str]).await?;

    tab.get(URL).await?;

    // 等邮箱框
    for _ in 0..15 {
        if tab.ele("css:input[type=email]").await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if let Ok(email) = tab.ele("css:input[type=email]").await {
        email.click().await?;
        email.input_human(EMAIL).await?;
        println!("[*] 已填邮箱 {EMAIL}");
    } else {
        println!("[!] 没出现邮箱框(可能整页挑战)");
    }

    // 等 token
    let mut tok = String::new();
    for i in 0..40 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let v = tab.run_js(TOKEN_JS).await.unwrap_or_default();
        let s = v.as_str().unwrap_or("").to_string();
        if s.len() > 20 {
            tok = s;
            println!("[*] [{i}s] 出 token,长度 {}", tok.len());
            break;
        }
    }

    tokio::time::sleep(Duration::from_secs(2)).await;
    let pkts = listen
        .wait_count(800, Some(Duration::from_secs(2)))
        .await
        .unwrap_or_default();

    // 按 host 归类统计
    use std::collections::BTreeMap;
    let mut by_host: BTreeMap<String, usize> = BTreeMap::new();
    for p in &pkts {
        let host = p.url.split('/').nth(2).unwrap_or("?").to_string();
        *by_host.entry(host).or_default() += 1;
    }
    println!("\n===== 全量 {} 条,按 host =====", pkts.len());
    for (h, c) in &by_host {
        println!("  {c:>3}  {h}");
    }

    // 重点看 CF 盾相关(challenges.cloudflare / cdn-cgi/challenge-platform / turnstile / jsd)
    let cp: Vec<&DataPacket> = pkts
        .iter()
        .filter(|p| {
            let u = &p.url;
            u.contains("challenge-platform")
                || u.contains("challenges.cloudflare")
                || u.contains("turnstile")
                || u.contains("/cdn-cgi/")
        })
        .collect();

    let mut out = String::new();
    out.push_str(&format!(
        "token_len={}\n全量 {} 条; CF盾相关 {} 条\nhost 统计: {:?}\n\n",
        tok.len(),
        pkts.len(),
        cp.len(),
        by_host
    ));
    println!("\n===== CF 盾相关请求 {} 条 =====", cp.len());
    for (i, p) in cp.iter().enumerate() {
        let reqlen = p.request.post_data.as_ref().map(|b| b.len()).unwrap_or(0);
        // path 含 flow/ov1/<nums>/<rayId>/<ticket> 或 jsd / orchestrate 等
        let line = format!(
            "[{i:>2}] {} {}\n     req_len={reqlen} req_head={:?}\n     resp[{}] len={} body_head={:?}\n",
            p.method,
            head(p.path(), 110),
            head(p.request.post_data.as_deref().unwrap_or(""), 90),
            p.response.status,
            p.response.body.len(),
            head(&p.response.body, 140),
        );
        println!("{line}");
        out.push_str(&line);
        out.push('\n');
    }

    out.push_str(&format!("\nTOKEN(完整):{tok}\n"));
    std::fs::write("cf_flow_capture.txt", &out).ok();
    println!(
        "[*] 已存 cf_flow_capture.txt | token head={}",
        head(&tok, 40)
    );

    listen.stop().await?;
    browser.quit().await?;
    Ok(())
}
