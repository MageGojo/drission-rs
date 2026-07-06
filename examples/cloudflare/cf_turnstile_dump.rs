//! **dump Turnstile challenge 脚本落盘(save-first)**:进 `challenges.cloudflare.com` 的跨域 iframe,
//! 把入口脚本 + VM 大脚本落盘(原始;非超大者另存美化版),并存 `api.js` / `jsd` 网络响应体原文。
//! 为后续「扣 VM + 补环境 纯算 token」准备原料。
//!
//! 关键:Turnstile 的 VM 是**反复 `eval` 同一份 ~1.3MB 解释器**(28 脚本里 22 个等大)。所以
//! - **一次 `list()` 拿全 scriptId 后,自己逐个 `source()`**(不走 `dump_all` 的内部二次 list —— 它会
//!   `disable` Debugger 致 id 失效,上一版因此只 dump 出 1 个);
//! - 按**内容 hash 去重**(22 份同源 VM 只落 1 份,标注重复次数);
//! - 超大 VM(>800KB)只存原始(美化对压缩 VM 意义不大、且慢),其余存原始 + 美化。
//!
//! 产物(`target/cf-dump/`,已 gitignore):`raw/` 原始、`beautified/` 美化、`network/` CF 网络资源、
//! `index.txt` 脚本清单。运行:`cargo run --example cf_turnstile_dump`(有头默认;`HEADLESS=1` 无头)

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::Duration;

use drission::prelude::*;

const URL: &str = "https://auth.exa.ai/?callbackUrl=https%3A%2F%2Fdashboard.exa.ai%2F";
const EMAIL: &str = "12341423@gmail.com";

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

/// iframe 内脚本的落盘名:入口脚本→`entry.js`;超大 VM→`vm_<len>B_id<id>.js`;
/// 其余 inline→`frag_<len>B_id<id>.js`;有 URL 的→`<末段>_id<id>.js`。
fn name_for(s: &ScriptInfo, src: &str) -> String {
    if s.url.contains("turnstile/f/") {
        return "entry.js".to_string();
    }
    if src.len() > 800_000 {
        return format!("vm_{}B_id{}.js", src.len(), s.script_id);
    }
    if s.url.is_empty() {
        return format!("frag_{}B_id{}.js", src.len(), s.script_id);
    }
    let last = s
        .url
        .split('?')
        .next()
        .unwrap_or("")
        .rsplit('/')
        .find(|x| !x.is_empty())
        .unwrap_or("s");
    let safe: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}_id{}.js", s.script_id)
}

fn net_filename(u: &str) -> String {
    let path = u.split('?').next().unwrap_or(u);
    let last = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("index");
    let host = if u.contains("challenges.cloudflare.com") {
        "cf"
    } else {
        "site"
    };
    format!("{host}_{last}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = matches!(
        std::env::var("HEADLESS").ok().as_deref(),
        Some("1") | Some("true")
    );
    let out = Path::new("target/cf-dump");
    std::fs::create_dir_all(out).ok();
    println!(
        "[*] CF Turnstile dump → {URL}(headless={headless});产物落 {}",
        out.display()
    );

    let browser = Browser::launch(
        BrowserOptions::new()
            .headless(headless)
            .window_size(1280, 800),
    )
    .await?;
    let tab = browser.latest_tab().await?;
    let listen = tab.listen();
    listen.start(&[]).await?;

    tab.get(URL).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    if let Ok(email) = tab.ele("css:input[type=email]").await {
        let _ = email.click().await;
        let _ = email.input_human(EMAIL).await;
        println!("[*] 已填邮箱,等待 Turnstile 渲染…");
    }
    tokio::time::sleep(Duration::from_secs(4)).await;

    let children = tab.attach_oopifs(Duration::from_secs(5)).await?;
    let Some(cf) = children
        .iter()
        .find(|c| c.url_contains("challenges.cloudflare.com"))
    else {
        println!("[!] 未进入 CF iframe,只 dump 网络资源");
        dump_network(&listen, out).await;
        browser.quit().await?;
        return Ok(());
    };
    println!("[*] 进入 CF iframe:{}", short(&cf.url, 100));

    let sc = cf.tab().scripts();
    // ⚠️ 只 list 一次;之后**不再触发会 disable Debugger 的操作**,否则 scriptId 失效。
    let list = sc.list().await.unwrap_or_default();
    println!(
        "[*] iframe 内解析脚本 {} 个;逐个 source() + hash 去重落盘…",
        list.len()
    );

    // 清单 index.txt(先写,save-first)。
    let mut index = format!("CF iframe: {}\n脚本数: {}\n\n", cf.url, list.len());
    for s in &list {
        let url = if s.url.is_empty() {
            "<inline/eval>"
        } else {
            &s.url
        };
        index.push_str(&format!("{:>9}B  {}  id={}\n", s.length, url, s.script_id));
    }
    std::fs::write(out.join("index.txt"), &index).ok();

    let raw_dir = out.join("raw");
    let beau_dir = out.join("beautified");
    std::fs::create_dir_all(&raw_dir).ok();
    std::fs::create_dir_all(&beau_dir).ok();

    let mut seen: HashMap<u64, (String, usize)> = HashMap::new();
    let (mut ok, mut dup, mut fail) = (0usize, 0usize, 0usize);
    for s in &list {
        if s.is_wasm {
            continue;
        }
        let src = match sc.source(&s.script_id).await {
            Ok(t) if !t.is_empty() => t,
            _ => {
                fail += 1;
                continue;
            }
        };
        let mut h = DefaultHasher::new();
        src.hash(&mut h);
        let fp = h.finish();
        if let Some((_, c)) = seen.get_mut(&fp) {
            *c += 1;
            dup += 1;
            continue;
        }
        let name = name_for(s, &src);
        std::fs::write(raw_dir.join(&name), &src).ok();
        if src.len() <= 800_000 {
            std::fs::write(beau_dir.join(&name), beautify_js(&src)).ok();
        }
        seen.insert(fp, (name, 1));
        ok += 1;
    }
    println!(
        "[*] 唯一脚本 {ok} 份落盘(去重 {dup} 个重复 / source 失败 {fail})→ {}/raw/ + beautified/",
        out.display()
    );
    let mut uniq: Vec<&(String, usize)> = seen.values().collect();
    uniq.sort_by(|a, b| b.1.cmp(&a.1));
    for (name, cnt) in uniq {
        println!(
            "    {name}{}",
            if *cnt > 1 {
                format!("  ×{cnt}(VM 多态重复)")
            } else {
                String::new()
            }
        );
    }

    let wasm = sc.dump_wasm(out.join("wasm")).await.unwrap_or_default();
    if !wasm.is_empty() {
        println!("[*] wasm dump {} 个 → {}/wasm/", wasm.len(), out.display());
    }

    dump_network(&listen, out).await;

    listen.stop().await?;
    browser.quit().await?;
    println!("\n==== dump 完成,产物在 {}/ ====", out.display());
    Ok(())
}

/// 把主框架抓到的 CF 网络资源(`api.js` / `jsd/main.js` 等)响应体原文落盘。
async fn dump_network(listen: &Listen, out: &Path) {
    let pkts = listen
        .wait_count(200, Some(Duration::from_secs(2)))
        .await
        .unwrap_or_default();
    let net_dir = out.join("network");
    std::fs::create_dir_all(&net_dir).ok();
    let mut n = 0;
    for p in pkts
        .iter()
        .filter(|p| is_cf_url(&p.url) && !p.response.body.is_empty())
    {
        let name = net_filename(&p.url);
        if std::fs::write(net_dir.join(&name), p.response.body.as_bytes()).is_ok() {
            println!(
                "    network: {} ({}B) → network/{name}",
                short(&p.url, 70),
                p.response.body.len()
            );
            n += 1;
        }
    }
    println!("[*] 网络资源 dump {n} 个 → {}/network/", out.display());
}
