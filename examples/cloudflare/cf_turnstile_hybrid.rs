//! **混合方案 · 浏览器侧**:浏览器只过 CF 盾铸 Turnstile token,其余全走纯协议(curl_cffi)。
//!
//! - `HYBRID_MODE=mint`(默认):铸一张**未消费**的新鲜 token + 导出**全部 cookie**(含 HttpOnly
//!   `cf_clearance`)→ `cf-protocol-poc/recon/fresh_token.json`,交给 `jsd/hybrid_replay.py` 纯协议
//!   复刻 `verify-turnstile` + `signin/email`。**铸完不点 Continue**,token 不被页面消费,留给协议侧用。
//! - `HYBRID_MODE=capture`:点 email 的 Continue 走真实流程,用 `tab.listen()` 抓**完整**的
//!   `verify-turnstile` / `signin/email` 请求体 + 响应 → `cf-protocol-poc/recon/verify_shape.json`
//!   (学请求形状:provider 上下文到底在 body 字段还是 cookie)。
//!
//! 不开 Debugger/Runtime ⇒ 干净 Chrome 能自然出 token(`exa_cf` 同后端,与 CDP 探测代价无关)。
//!
//! 运行:`cargo run --example cf_turnstile_hybrid`(默认 mint;`HYBRID_MODE=capture` 学形状;`HEADLESS=1` 无头)。

use std::time::Duration;

use drission::prelude::*;
use serde_json::{Value, json};

const URL: &str = "https://auth.exa.ai/?callbackUrl=https%3A%2F%2Fdashboard.exa.ai%2F";
const EMAIL_DEFAULT: &str = "12341423@gmail.com";
const OUT_DIR: &str = "cf-protocol-poc/recon";
const TOKEN_JS: &str =
    "(()=>{const e=document.querySelector('[name=cf-turnstile-response]');return e?e.value:'';})()";

fn short(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{t}…")
    } else {
        t
    }
}

fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cookies_json(cookies: &[Cookie]) -> Value {
    Value::Array(
        cookies
            .iter()
            .map(|c| {
                json!({
                    "name": c.name,
                    "value": c.value,
                    "domain": c.domain,
                    "path": c.path,
                    "expires": c.expires,
                    "http_only": c.http_only,
                    "secure": c.secure,
                })
            })
            .collect(),
    )
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    let mode = std::env::var("HYBRID_MODE").unwrap_or_else(|_| "mint".to_string());
    let headless = matches!(
        std::env::var("HEADLESS").ok().as_deref(),
        Some("1") | Some("true")
    );
    let email = std::env::var("EXA_EMAIL").unwrap_or_else(|_| EMAIL_DEFAULT.to_string());
    println!("[*] CF Turnstile 混合·浏览器侧 → mode={mode} headless={headless} email={email}");
    std::fs::create_dir_all(OUT_DIR).ok();

    let browser = Browser::launch(
        BrowserOptions::new()
            .headless(headless)
            .window_size(1280, 800),
    )
    .await?;
    let tab = browser.latest_tab().await?;

    let listen = tab.listen();
    listen.start(&["api/auth"]).await?;

    tab.get(URL).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // 等 email 框出现(否则被整页托管挑战拦在外面)。
    let mut has_email = false;
    for _ in 0..15 {
        if tab.ele("css:input[type=email]").await.is_ok() {
            has_email = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if !has_email {
        println!("[!] 没出现邮箱框 → 被 CF 拦在整页挑战(IP 风险过高)。换代理或等冷却。");
        browser.quit().await?;
        return Ok(());
    }

    let email_el = tab.ele("css:input[type=email]").await?;
    email_el.click().await?;
    email_el.input_human(&email).await?;
    println!("[*] 已填邮箱 {email}");

    // 等 Turnstile 自动出 token。
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
    if let Ok(b) = tab.screenshot_bytes().await {
        std::fs::write("cf_hybrid_shot.png", &b).ok();
    }
    if tok.len() <= 20 {
        println!("[!] 未出 token → 本次没过盾。可能 IP 风控,挂代理或等冷却再试。");
        browser.quit().await?;
        return Ok(());
    }
    println!("[*] token = {} (len {})", short(&tok, 48), tok.len());

    if mode == "capture" {
        // 点 email 的 Continue(精确文本,排除 SSO),触发页面自己带 provider 上下文调 verify。
        match tab
            .ele("xpath://button[normalize-space(.)='Continue']")
            .await
        {
            Ok(btn) => {
                btn.click().await?;
                println!("[*] 已点 Continue → 等页面调 verify-turnstile/signin");
            }
            Err(_) => println!("[!] 未找到 Continue 按钮(结构可能变化)"),
        }
        tokio::time::sleep(Duration::from_secs(4)).await;
        let pkts = listen
            .wait_count(200, Some(Duration::from_secs(2)))
            .await
            .unwrap_or_default();

        let dump_pkt = |needle: &str| -> Value {
            match pkts.iter().find(|p| p.url.contains(needle)) {
                Some(p) => json!({
                    "url": p.url,
                    "method": p.method,
                    "req_body": p.request.post_data,
                    "status": p.response.status,
                    "resp_body": p.response.body,
                }),
                None => Value::Null,
            }
        };
        let verify = dump_pkt("verify-turnstile");
        let signin = dump_pkt("signin/email");
        let cookies = tab.cookies().await.unwrap_or_default();

        println!("\n========== 抓到的接口 ==========");
        for p in &pkts {
            println!(
                "  {} {} [status {}]",
                p.method,
                short(&p.url, 70),
                p.response.status
            );
        }
        let out = json!({
            "ts": now_ts(),
            "token_len": tok.len(),
            "verify_turnstile": verify,
            "signin_email": signin,
            "cookies": cookies_json(&cookies),
        });
        let path = format!("{OUT_DIR}/verify_shape.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&out).unwrap_or_default(),
        )
        .ok();
        println!("\n[✓] 形状落盘 → {path}(看 verify_turnstile.req_body 里 provider 上下文)");
    } else {
        // mint:不点 Continue,token 保持未消费;导出 token + 全 cookie 给协议侧。
        let cookies = tab.cookies().await.unwrap_or_default();
        let has_clear = cookies.iter().any(|c| c.name == "cf_clearance");
        let out = json!({
            "ts": now_ts(),
            "url": URL,
            "email": email,
            "token": tok,
            "cookies": cookies_json(&cookies),
        });
        let path = format!("{OUT_DIR}/fresh_token.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&out).unwrap_or_default(),
        )
        .ok();
        println!(
            "[✓] 新鲜 token + {} 个 cookie(cf_clearance={})落盘 → {path}",
            cookies.len(),
            has_clear
        );
        println!("[!] token 有时效,尽快跑:python3 jsd/hybrid_replay.py");
    }

    listen.stop().await?;
    browser.quit().await?;
    Ok(())
}
