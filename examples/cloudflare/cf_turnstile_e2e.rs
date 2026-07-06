//! **端到端验真 v3(全程用 drission-rs 库能力)**:干净 Chrome 过 Turnstile 出 token → 截图 →
//! **点 email 的 `Continue`**(magic-link 流程、不跳转外部、只验人机关,带上 provider 上下文)→
//! 用 `tab.listen()` 抓 exa 自己调的 `/api/auth/verify-turnstile` 返回,看 `success` 判定 token 真能用。
//!
//! 用库:`Browser::launch` / `tab.ele().input_human()` / `tab.ele().click()` / `tab.listen()` /
//! `tab.screenshot_bytes()` —— 不手搓 fetch。验真姿势对比 v2(裸 fetch 缺 provider → Invalid auth
//! provider):这里让**页面自己**带登录上下文调 verify,才是真实流程。
//!
//! 运行:`cargo run --example cf_turnstile_e2e`(有头默认;`HEADLESS=1` 无头)。产物 `cf_e2e_*.png`。

use std::time::Duration;

use drission::prelude::*;

const URL: &str = "https://auth.exa.ai/?callbackUrl=https%3A%2F%2Fdashboard.exa.ai%2F";
const EMAIL_DEFAULT: &str = "12341423@gmail.com";
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

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = matches!(
        std::env::var("HEADLESS").ok().as_deref(),
        Some("1") | Some("true")
    );
    let email = std::env::var("EXA_EMAIL").unwrap_or_else(|_| EMAIL_DEFAULT.to_string());
    println!("[*] CF Turnstile 端到端验真 v3(库能力)→ {URL}(headless={headless}, email={email})");

    let browser = Browser::launch(
        BrowserOptions::new()
            .headless(headless)
            .window_size(1280, 800),
    )
    .await?;
    let tab = browser.latest_tab().await?;

    // 库能力①:网络监听,聚焦 exa auth 接口(含 verify-turnstile)。
    let listen = tab.listen();
    listen.start(&["api/auth"]).await?;

    tab.get(URL).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    if let Ok(b) = tab.screenshot_bytes().await {
        std::fs::write("cf_e2e_state.png", &b).ok();
        println!(
            "[*] 落地截图 → cf_e2e_state.png(title={:?})",
            tab.title().await.unwrap_or_default()
        );
    }

    // 等 email 框出现。
    let mut has_email = false;
    for _ in 0..15 {
        if tab.ele("css:input[type=email]").await.is_ok() {
            has_email = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if !has_email {
        println!("[!] 没出现邮箱框 → 被 CF 拦在整页挑战(IP 风险过高)。见 cf_e2e_state.png");
        browser.quit().await?;
        return Ok(());
    }

    // 库能力②:可信点击 + 拟人输入。
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
        std::fs::write("cf_e2e_shot.png", &b).ok();
        println!("[*] 出 token 后截图 → cf_e2e_shot.png(看 widget 是否绿「成功!」)");
    }
    if tok.len() <= 20 {
        println!(
            "[!] 未出 token → 本次没过盾(widget 应为红/转圈)。可能 IP 风控,挂代理或等冷却再试。"
        );
        browser.quit().await?;
        return Ok(());
    }
    println!("[*] token = {}", short(&tok, 60));

    // 库能力③:点 email 的 Continue(精确文本,排除 "Continue with Google" / SSO),触发 magic-link
    // 流程 —— 页面会带 provider 上下文自己调 verify-turnstile。
    match tab
        .ele("xpath://button[normalize-space(.)='Continue']")
        .await
    {
        Ok(btn) => {
            btn.click().await?;
            println!("[*] 已点 email 的 Continue(magic-link,不跳转外部)");
        }
        Err(_) => println!("[!] 未找到 email 的 Continue 按钮(结构可能变化)"),
    }

    // 库能力①:抓 exa 自己调的 verify-turnstile 返回。
    tokio::time::sleep(Duration::from_secs(4)).await;
    let pkts = listen
        .wait_count(200, Some(Duration::from_secs(2)))
        .await
        .unwrap_or_default();
    println!(
        "\n========== /api/auth/ 请求(共抓 {})==========",
        pkts.len()
    );
    for p in &pkts {
        println!(
            "  {} {} [status {}]",
            p.method,
            short(&p.url, 70),
            p.response.status
        );
    }

    let verify = pkts.iter().find(|p| p.url.contains("verify-turnstile"));
    println!("\n========== 结论 ==========");
    match verify {
        Some(p) => {
            println!("  verify-turnstile [status {}]", p.response.status);
            if let Some(b) = &p.request.post_data {
                println!("    req : {}", short(b, 160));
            }
            println!("    resp: {}", short(&p.response.body, 200));
            let body = p.response.body.to_lowercase();
            let ok = p.response.status == 200
                && (body.contains("verified") || body.contains("\"success\":true"));
            if ok {
                println!("✅ exa(带 provider 上下文)接受了这个 token → token 真有效、能用");
            } else {
                println!("❌ verify 未通过(见上方)→ 本次 token 无效 / 被风控");
            }
        }
        None => println!("⚠️ 未抓到 verify-turnstile(可能点击未触发 / 流程变化,见上方请求列表)"),
    }

    // signin/email —— 真发 magic-link 的关键(403 Turnstile required = token 没被接受;200/302 = 已发信)
    let signin = pkts
        .iter()
        .find(|p| p.url.contains("signin/email") || p.url.contains("/signin/"));
    match signin {
        Some(p) => {
            println!(
                "\n  signin/email [status {}]  resp: {}",
                p.response.status,
                short(&p.response.body, 200)
            );
            if p.response.status == 200 || p.response.status == 302 {
                println!("✉️  signin/email 被接受 → magic-link 已发出(去信箱核实收信)");
            } else {
                println!(
                    "❌ signin/email status {} → 未发信(token 未被接受?)",
                    p.response.status
                );
            }
        }
        None => println!("⚠️ 未抓到 signin/email(可能 magic-link 流程未走到,见上方列表)"),
    }
    println!("[*] token(完整):{tok}");

    listen.stop().await?;
    browser.quit().await?;
    Ok(())
}
