//! **CF jsd 第一方信任侦察(Camoufox 能过基准)** —— 给纯协议逆 jsd 用。
//!
//! 记忆 purediff 结论:exa Turnstile 的 token 来自 CF 第一方信任(jsd / JS Detections),
//! 浏览器在 auth.exa.ai 上跑 `jsd/main.js` + 回传 `jsd/oneshot`(指纹)建立可信后,
//! Turnstile 直接放行。本例用能过盾的 Camoufox 跑完整 exa 流程,`tab.listen()` 抓**完整**
//! jsd 流量并落盘:`jsd/oneshot` 的完整请求体(golden 指纹,供 lzcodec 解码对照)、
//! `jsd/main.js` 源、turnstile `api.js`、出 token 时 auth.exa.ai 的全部 cookie、token。
//!
//! 运行:`cargo run --example cf_jsd_recon --features camoufox`(有头默认;`HEADLESS=1` 无头)。
//! 产物(save-first):`cf-protocol-poc/jsd/recon/`。

use std::time::Duration;

use drission::prelude::*;

const URL: &str = "https://auth.exa.ai/?callbackUrl=https%3A%2F%2Fdashboard.exa.ai%2F";
const EMAIL: &str = "12341423@gmail.com";
const TOKEN_JS: &str =
    "(()=>{const e=document.querySelector('[name=cf-turnstile-response]');return e?e.value:'';})()";

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = matches!(
        std::env::var("HEADLESS").ok().as_deref(),
        Some("1") | Some("true")
    );
    let out_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("cf-protocol-poc/jsd/recon");
    std::fs::create_dir_all(&out_dir).ok();
    println!(
        "[*] CF jsd 侦察 → {URL}(headless={headless})\n[*] 产物目录 {}",
        out_dir.display()
    );

    let browser = Browser::launch(
        BrowserOptions::new()
            .headless(headless)
            .window_size(1280, 800),
    )
    .await?;
    let tab = browser.latest_tab().await?;

    let listen = tab.listen();
    listen.start(&[] as &[&str]).await?; // 全量抓

    tab.get(URL).await?;

    // 填邮箱(触发完整流程)
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
    if tok.is_empty() {
        println!("[!] 未出 token(IP 风控/后端检测?)——仍落盘已抓到的 jsd 流量");
    }

    tokio::time::sleep(Duration::from_secs(2)).await;
    let pkts = listen
        .wait_count(1000, Some(Duration::from_secs(2)))
        .await
        .unwrap_or_default();

    // 落盘:token、cookie
    std::fs::write(out_dir.join("token.txt"), &tok).ok();
    if let Ok(cookies) = tab.cookies().await {
        let mut cj = String::from("[\n");
        for (i, c) in cookies.iter().enumerate() {
            if i > 0 {
                cj.push_str(",\n");
            }
            cj.push_str(&format!(
                "  {{\"name\":\"{}\",\"value\":\"{}\",\"domain\":\"{}\",\"path\":\"{}\",\"expires\":{},\"httpOnly\":{},\"secure\":{}}}",
                esc(&c.name), esc(&c.value), esc(&c.domain), esc(&c.path), c.expires, c.http_only, c.secure
            ));
        }
        cj.push_str("\n]\n");
        std::fs::write(out_dir.join("cookies.json"), &cj).ok();
        println!("[*] cookie 共 {} 条 → cookies.json", cookies.len());
        for c in &cookies {
            // CF 第一方信任最相关的 cookie
            if c.name.starts_with("cf")
                || c.name.starts_with("__cf")
                || c.name.contains("clearance")
            {
                println!(
                    "    [CF cookie] {}={}…",
                    c.name,
                    esc(&c.value.chars().take(24).collect::<String>())
                );
            }
        }
    }

    // 落盘:jsd / 全量流程
    let mut flow = String::new();
    flow.push_str(&format!(
        "token_len={}\n全量 {} 条\n\n",
        tok.len(),
        pkts.len()
    ));
    let mut got_oneshot = false;
    let mut got_main = false;
    for p in &pkts {
        let u = &p.url;
        let is_cf = u.contains("challenge-platform")
            || u.contains("challenges.cloudflare")
            || u.contains("/cdn-cgi/");
        if !is_cf {
            continue;
        }
        let reqlen = p.request.post_data.as_ref().map(|b| b.len()).unwrap_or(0);
        flow.push_str(&format!(
            "{} {}\n  req_len={reqlen} resp[{}] resp_len={}\n",
            p.method,
            p.path(),
            p.response.status,
            p.response.body.len()
        ));
        // jsd/oneshot —— golden 指纹请求体
        if u.contains("/jsd/oneshot/") {
            if let Some(body) = &p.request.post_data {
                std::fs::write(out_dir.join("oneshot_body.txt"), body).ok();
                let meta = format!(
                    "url: {}\nmethod: {}\nreq_len: {}\nstatus: {}\nresp_len: {}\nresp_body: {}\n",
                    p.url,
                    p.method,
                    body.len(),
                    p.response.status,
                    p.response.body.len(),
                    p.response.body
                );
                std::fs::write(out_dir.join("oneshot_meta.txt"), meta).ok();
                got_oneshot = true;
                println!(
                    "[*] golden jsd/oneshot body {}B → oneshot_body.txt",
                    body.len()
                );
            }
        }
        // jsd/main.js —— 浏览器实际用的源
        if u.contains("/scripts/jsd/") && u.ends_with("main.js") && !p.response.body.is_empty() {
            std::fs::write(out_dir.join("browser_main.js"), &p.response.body).ok();
            got_main = true;
        }
        // turnstile api.js
        if u.contains("/turnstile/") && u.ends_with("api.js") && !p.response.body.is_empty() {
            std::fs::write(out_dir.join("apijs.js"), &p.response.body).ok();
        }
    }
    std::fs::write(out_dir.join("flow.txt"), &flow).ok();
    println!(
        "[*] 落盘完成:oneshot={} main.js={} | flow.txt",
        got_oneshot, got_main
    );
    if !got_oneshot {
        println!("[!] 没抓到 jsd/oneshot(可能这次走了别的路径/未触发)——看 flow.txt");
    }

    listen.stop().await?;
    browser.quit().await?;
    Ok(())
}
