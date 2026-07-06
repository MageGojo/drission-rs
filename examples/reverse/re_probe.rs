//! **通用逆向探针**(对站点零硬编码):对任意 `URL=` 跑全套 ②脚本 / ③hook / ④反调试度量 / ⑤抓包。
//!
//! 证明这些是**通用库能力**(`tab.debugger()/scripts()/hook()` + `tab.listen()`),不针对任何特定站点。
//! 运行:`URL=<任意站> [GREP=<词>] [API=<api子串>] [SEC=12] [HL=0] cargo run --example re_probe`
//! 例:`URL='https://www.youtube.com/watch?v=...' GREP=signature API=/youtubei/ cargo run --example re_probe`

use std::collections::BTreeMap;
use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let url = std::env::var("URL").unwrap_or_else(|_| "https://example.com".to_string());
    let grep = std::env::var("GREP").unwrap_or_else(|_| "signature".to_string());
    let api = std::env::var("API").unwrap_or_default();
    let secs: u64 = std::env::var("SEC")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);

    println!("[*] 通用探针 → {url}(headless={headless});本程序对该 URL 零硬编码");
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let none = GetOptions::new().load_mode(LoadMode::None);

    // ── ③ Hook + ⑤ 抓包(同一 tab,导航前装)──
    let tab = browser.new_tab(Some("about:blank")).await?;
    let hook = tab
        .hook()
        .crypto_subtle()
        .crypto_js()
        .json()
        .base64()
        .xhr()
        .fetch()
        .with_stack()
        .start()
        .await?;
    let listen = tab.listen();
    if api.is_empty() {
        listen.start(&[]).await?;
    } else {
        listen.start(&[api.as_str()]).await?;
    }

    tab.get(&url).await?;
    println!(
        "[*] title={:?},采集 {secs}s …",
        tab.title().await.unwrap_or_default()
    );
    tokio::time::sleep(Duration::from_secs(secs)).await;

    // ③ 报告:sink 直方图 + 几个 fetch/xhr URL 样本。
    let hits = hook.drain().await;
    let mut hist: BTreeMap<String, usize> = BTreeMap::new();
    for h in &hits {
        *hist.entry(h.sink.clone()).or_default() += 1;
    }
    println!(
        "\n========== ③ Hook 命中 {} 条(按 sink)==========",
        hits.len()
    );
    for (k, v) in &hist {
        println!("   {k}: {v}");
    }
    println!("   —— 网络发起样本(fetch/XHR.open 的 URL):");
    for h in hits
        .iter()
        .filter(|h| h.func == "fetch" || h.func == "open")
        .take(6)
    {
        println!("     [{}] {}", h.sink, trunc(&h.arg_str(0), 90));
    }

    // ⑤ 报告:抓到的请求样本。
    let pkts = listen.wait_count(120, Some(Duration::from_secs(2))).await?;
    let sel: Vec<&DataPacket> = pkts
        .iter()
        .filter(|p| api.is_empty() || p.url.contains(&api))
        .collect();
    println!(
        "\n========== ⑤ 抓到 {} 个请求(筛 {:?} 得 {} 个,样本)==========",
        pkts.len(),
        api,
        sel.len()
    );
    for p in sel.iter().take(8) {
        println!(
            "   {} {} [{}B]",
            p.method,
            trunc(&p.url, 95),
            p.response.body.len()
        );
    }

    // ② 报告:脚本 dump + grep(先 set_skip_all_pauses 防个别站反调试卡收集)。
    let _ = tab.debugger().set_skip_all_pauses(true).await;
    let sc = tab.scripts();
    let scripts = sc.list().await.unwrap_or_default();
    println!(
        "\n========== ② 解析到 {} 个脚本;grep {:?} ==========",
        scripts.len(),
        grep
    );
    let gh = sc.grep(&grep).await.unwrap_or_default();
    println!("   命中 {} 处:", gh.len());
    for m in gh.iter().take(6) {
        println!(
            "     {} :{}  {}",
            short(&m.url),
            m.line_number,
            trunc(&m.snippet, 80)
        );
    }

    hook.stop().await?;
    listen.stop().await?;

    // ── ④ 反调试度量(新 tab,通用):开 Debugger 数暂停;若有则 set_skip_all_pauses 通杀 ──
    println!("\n========== ④ 反调试度量(count_pauses,通用)==========");
    let tab2 = browser.new_tab(Some("about:blank")).await?;
    let d2 = tab2.debugger();
    d2.enable().await?;
    tab2.get_with(&url, &none).await?;
    let (n_a, bt) = d2.count_pauses(Duration::from_secs(5), true).await?;
    println!("   不 defuse:5s 内 Debugger.paused {n_a} 次");
    if let Some(first) = bt.lines().next() {
        if !first.is_empty() {
            println!("   首栈:{first}");
        }
    }
    if n_a > 0 {
        let tab3 = browser.new_tab(Some("about:blank")).await?;
        let d3 = tab3.debugger();
        d3.enable().await?;
        d3.set_skip_all_pauses(true).await?;
        tab3.get_with(&url, &none).await?;
        d3.set_skip_all_pauses(true).await?;
        let (n_c, _) = d3.count_pauses(Duration::from_secs(5), true).await?;
        println!("   set_skip_all_pauses 通杀后:{n_c} 次(应 ~0)");
    } else {
        println!("   该站未触发 debugger 反调试(count_pauses 度量本身通用,有就会显示)");
    }

    browser.quit().await?;
    println!("\n==== 通用探针完成(全程对 URL 零硬编码,任意站可跑)====");
    Ok(())
}

fn trunc(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{t}…")
    } else {
        t
    }
}

fn short(u: &str) -> String {
    u.rsplit('/').next().unwrap_or(u).chars().take(46).collect()
}
