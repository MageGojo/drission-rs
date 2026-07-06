//! **Cloudflare Turnstile 侦察**(CDP 后端 · 进跨域 iframe 逆向的第一步)。
//!
//! 目标 auth.exa.ai 的 Turnstile 跑在 `challenges.cloudflare.com` 的 **OOPIF(跨域 iframe)** 里,
//! 主框架的 `run_js`/`createIsolatedWorld` 进不去。本例用新增通用能力 `tab.attach_oopifs()` 把这些
//! 子 target 收进来、当成普通 tab,然后:
//! - 在 iframe 内部 `scripts().list()/grep()` dump 它的 challenge 脚本(证明真的进去了);
//! - 下钻嵌套 OOPIF(Turnstile 内常有 managed/interactive 子帧);
//! - dump CF 的网络请求(`api.js` / `cdn-cgi/challenge-platform/` 编排)。
//!
//! 摸清 Turnstile 结构 = 为后续「扣 VM + 补环境 纯算 token」定位入口。本例对站点零硬编码逻辑
//! (只有 URL/邮箱是靶场参数),`attach_oopifs` 是通用的「进任意跨域 iframe 逆向」能力。
//!
//! 运行:`cargo run --example cf_turnstile_recon`(默认有头;`HEADLESS=1` 无头对照)

use std::time::Duration;

use drission::prelude::*;

const URL: &str = "https://auth.exa.ai/?callbackUrl=https%3A%2F%2Fdashboard.exa.ai%2F";
const EMAIL: &str = "12341423@gmail.com";
const TOKEN_JS: &str =
    "(()=>{const e=document.querySelector('[name=cf-turnstile-response]');return e?e.value:'';})()";

fn is_cf_url(u: &str) -> bool {
    u.contains("challenges.cloudflare.com")
        || u.contains("cdn-cgi/challenge")
        || u.contains("/turnstile/")
}

fn short(u: &str, n: usize) -> String {
    let t: String = u.chars().take(n).collect();
    if u.chars().count() > n {
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
    println!("[*] CF Turnstile 侦察 → {URL}(headless={headless})");

    let browser = Browser::launch(
        BrowserOptions::new()
            .headless(headless)
            .window_size(1280, 800),
    )
    .await?;
    let tab = browser.latest_tab().await?;

    // 主框架抓包(全抓,后筛 CF)。
    let listen = tab.listen();
    listen.start(&[]).await?;

    tab.get(URL).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let webdriver = tab.run_js("navigator.webdriver").await.unwrap_or_default();
    println!("[*] navigator.webdriver = {webdriver}");

    // 填邮箱触发 Turnstile(只碰 type=email,避开蜜罐 name=website)。
    if let Ok(email) = tab.ele("css:input[type=email]").await {
        let _ = email.click().await;
        let _ = email.input_human(EMAIL).await;
        println!("[*] 已填邮箱,等待 Turnstile 渲染…");
    } else {
        println!("[!] 未找到邮箱输入框(页面结构可能变了),继续侦察…");
    }
    tokio::time::sleep(Duration::from_secs(4)).await;

    // ── 核心:附着所有跨域 iframe / worker 子 target(首次开 autoAttach,会补发已存在子 target)──
    let children = tab.attach_oopifs(Duration::from_secs(5)).await?;
    println!(
        "\n========== 子 target 总览(attach_oopifs)共 {} 个 ==========",
        children.len()
    );
    for c in &children {
        let sid = &c.session_id[..c.session_id.len().min(12)];
        println!("  [{}] {}  sess={sid}", c.kind, short(&c.url, 80));
    }

    // 找 Cloudflare 的 iframe(challenges.cloudflare.com)。
    match children
        .iter()
        .find(|c| c.url_contains("challenges.cloudflare.com"))
    {
        Some(cf) => {
            println!(
                "\n========== ✅ 进入 CF iframe:{} ==========",
                short(&cf.url, 100)
            );
            // 在 iframe 内部 dump 它解析的脚本(证明真的进到了跨域 iframe 上下文)。
            let sc = cf.tab().scripts();
            let scripts = sc.list().await.unwrap_or_default();
            println!("  iframe 内解析脚本 {} 个:", scripts.len());
            for s in scripts.iter().take(12) {
                let wasm = if s.is_wasm { " [WASM]" } else { "" };
                println!("    {} ({}B){wasm}", short(&s.url, 80), s.length);
            }
            // grep challenge 关键字,定位 VM/编排入口。
            for kw in ["turnstile", "challenge", "0x", "chl"] {
                let hits = sc.grep(kw).await.unwrap_or_default();
                if !hits.is_empty() {
                    println!(
                        "  grep {kw:?}:{} 处,样本 {}",
                        hits.len(),
                        short(&hits[0].snippet, 70)
                    );
                }
            }
            // 下钻:CF iframe 内是否还有嵌套 OOPIF(managed/interactive 子帧)。
            let nested = cf
                .tab()
                .attach_oopifs(Duration::from_secs(2))
                .await
                .unwrap_or_default();
            if !nested.is_empty() {
                println!("  嵌套子 target {} 个:", nested.len());
                for n in &nested {
                    println!("    [{}] {}", n.kind, short(&n.url, 80));
                }
            }
        }
        None => {
            println!(
                "\n[!] 未在子 target 里发现 challenges.cloudflare.com(Turnstile 可能还没出现 / 该 IP 走非交互式)"
            );
        }
    }

    // ── CF 网络请求(主框架抓到的:Turnstile 的 api.js / challenge-platform 编排)──
    let pkts = listen
        .wait_count(200, Some(Duration::from_secs(2)))
        .await
        .unwrap_or_default();
    let cf_pkts: Vec<&DataPacket> = pkts.iter().filter(|p| is_cf_url(&p.url)).collect();
    println!(
        "\n========== CF 网络请求 {} 个(共抓 {})==========",
        cf_pkts.len(),
        pkts.len()
    );
    for p in cf_pkts.iter().take(15) {
        println!(
            "  {} {} [{}B]",
            p.method,
            short(&p.url, 90),
            p.response.body.len()
        );
    }

    let token = tab.run_js(TOKEN_JS).await.unwrap_or_default();
    let tok_len = token.as_str().unwrap_or("").len();
    println!("\n[*] Turnstile token 长度 = {tok_len}");

    listen.stop().await?;
    browser.quit().await?;
    println!("\n==== 侦察完成 ====");
    Ok(())
}
