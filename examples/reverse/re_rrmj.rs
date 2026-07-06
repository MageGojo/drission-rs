//! 真站端到端(侦察):用 ③ crypto tap + ⑤ 抓包,偷出 `api.rrmj.plus` 的 `x-ca-sign` 签名串与 key。
//!
//! 关键:**不开 Debugger 域** → 站点的 `debugger` 反调试是 no-op(不附调试器就不触发),页面正常跑;
//! `tab.hook().crypto_js()` 直接 hook `CryptoJS.HmacSHA256(签名串, key)` 偷明文输入,
//! `tab.hook().xhr()` 抓 `setRequestHeader('x-ca-sign', 值)`,`tab.listen()` 抓真实 API 请求。
//!
//! 运行:`cargo run --example re_rrmj`(无头默认;`HL=0` 有头;`URL=` 换页;`SEC=12` 采集秒数)。

use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let url = std::env::var("URL").unwrap_or_else(|_| "https://mh.yichengwlkj.com/pc".to_string());
    let secs: u64 = std::env::var("SEC")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);

    println!("[*] 目标 {url}(headless={headless},采集 {secs}s)");
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;

    // ③ crypto tap + XHR 头 + base64,带调用栈。导航前装好。**不碰 Debugger 域**(避开反调试)。
    let hook = tab
        .hook()
        .crypto_js()
        .crypto_subtle()
        .xhr()
        .base64()
        .with_stack()
        .start()
        .await?;
    // ⑤ 抓包(全捕,后面筛 API/带签名的)。
    let listen = tab.listen();
    listen.start(&[]).await?;

    tab.get(&url).await?;
    println!(
        "[*] 已加载: title={:?}",
        tab.title().await.unwrap_or_default()
    );
    tokio::time::sleep(Duration::from_secs(secs)).await;

    // ── ③ crypto tap:HmacSHA256 的(签名串, key)+ x-ca-sign 头的最终值 ──
    let hits = hook.drain().await;
    println!(
        "\n========== ③ Hook 命中(共 {} 条,筛 crypto / sign 头)==========",
        hits.len()
    );
    let mut sign_msgs = Vec::new();
    for h in &hits {
        let lc = h.sink.to_lowercase();
        let is_hmac = lc.contains("hmac");
        let is_crypto = lc.contains("crypto");
        let is_sign_hdr =
            h.func == "setRequestHeader" && h.arg_str(0).to_ascii_lowercase().contains("sign");
        if !(is_hmac || is_crypto || is_sign_hdr) {
            continue;
        }
        println!("── [{}] {}", h.sink, h.func);
        for (i, a) in h.args.iter().enumerate() {
            println!("    arg[{i}] = {}", trunc(&a.to_string(), 240));
        }
        if is_hmac {
            sign_msgs.push((h.arg_str(0), h.arg_str(1)));
        }
        if !h.stack.is_empty() {
            for l in h.stack.lines().filter(|l| l.contains("http")).take(2) {
                println!("    @ {}", l.trim());
            }
        }
    }
    if let Some((msg, key)) = sign_msgs.first() {
        println!("\n[★] crypto tap 偷到:");
        println!("    签名串 message = {}", trunc(msg, 300));
        println!("    密钥   key     = {key}");
    }

    // ── ⑤ 抓包:落到 API 的请求(method/url/postData + 是否带 x-ca-sign)──
    let pkts = listen.wait_count(120, Some(Duration::from_secs(2))).await?;
    println!(
        "\n========== ⑤ 抓到 {} 个请求(筛 API / 带签名)==========",
        pkts.len()
    );
    for p in &pkts {
        let is_api =
            p.url.contains("rrmj.plus") || p.url.contains("/v1/") || p.url.contains("/api/");
        let sign = p
            .request
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("x-ca-sign"))
            .map(|(_, v)| v.clone());
        if !is_api && sign.is_none() {
            continue;
        }
        println!("── {} {}", p.method, trunc(&p.url, 140));
        if let Some(s) = &sign {
            println!("    x-ca-sign: {s}");
        }
        if let Some(b) = &p.request.post_data {
            println!("    postData: {}", trunc(b, 200));
        }
        // 响应能否当 JSON 直接读(若是密文则不是),给个长度提示。
        println!(
            "    resp[{}B] {}",
            p.response.body.len(),
            trunc(&p.response.body, 80)
        );
    }

    // ── ② 按 URL dump 签名 chunk 2112 的美化源码(x-ca-sign 名是动态构造,字面 grep 命中不了,改按 chunk URL 抓)──
    println!("\n========== ② dump 签名 chunk(按 URL 含 2112-)==========");
    let sc = tab.scripts();
    let scripts = sc.list().await.unwrap_or_default();
    let chunk = std::env::var("CHUNK").unwrap_or_else(|_| "2112-".to_string());
    if let Some(s) = scripts.iter().find(|s| s.url.contains(&chunk)) {
        println!("[*] 命中 chunk: {} (scriptId {})", s.url, s.script_id);
        if let Ok(src) = sc.source(&s.script_id).await {
            let pretty = beautify_js(&src);
            let _ = std::fs::write("/tmp/rrmj_sign_chunk.js", &pretty);
            println!(
                "[*] 已美化并保存 {} 字符到 /tmp/rrmj_sign_chunk.js",
                pretty.len()
            );
        }
    } else {
        println!(
            "[*] 未找到含 {chunk} 的脚本(共 {} 个);可设 CHUNK= 指定。",
            scripts.len()
        );
    }

    hook.stop().await?;
    listen.stop().await?;
    browser.quit().await?;
    println!("\n==== 侦察完成 ====");
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
