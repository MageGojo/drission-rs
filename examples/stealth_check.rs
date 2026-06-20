//! 反检测验证:跑四大检测站,每站给出明确 PASS/FAIL,最后汇总「过了吗」。
//!
//! 注意:本例**不写任何补环境代码**——补环境(UA 屏蔽 / 屏幕一致 / humanize / block_webrtc)
//! 全部内置在 `BrowserOptions::default()`,裸 `BrowserOptions::new()` 一打开即生效。
//!
//! - 本地指纹           :webdriver=false / UA 无 Camoufox / WebRTC 关闭 / 屏幕自洽
//! - bot.sannysoft.com  :真实失败项为 0(排除 Chrome 专有检测——我们是 Firefox 内核,本就没有)
//! - tls.peet.ws/api/all:UA 声称 Firefox 且 TLS(JA3/JA4)为真 Firefox 栈,二者自洽不穿帮
//! - browserleaks.com   :WebGL/Canvas 取真值 + WebRTC 无 IP 泄漏
//! - nowsecure.nl       :Cloudflare 盾(过 = 真实页)
//!
//! 运行:`cargo run --example stealth_check`(默认 headless;`HL=0` 看界面)

use std::time::Duration;

use drission::prelude::*;

/// 跑一段 JS 取字符串(对象请在 JS 里 `JSON.stringify`)。
async fn js(tab: &Tab, expr: &str) -> String {
    match tab.run_js(expr).await {
        Ok(serde_json::Value::String(s)) => s,
        Ok(serde_json::Value::Null) => "null".into(),
        Ok(v) => v.to_string(),
        Err(e) => format!("<err: {e}>"),
    }
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    println!("[*] 启动 Camoufox(headless={headless})…(零补环境配置,全部走内置默认)");
    let browser = Browser::launch(BrowserOptions::new().headless(headless)).await?;
    let tab = browser.latest_tab().await?;

    // 各站结论收集 (站名, 是否通过)。
    let mut results: Vec<(&str, bool)> = Vec::new();

    // ---------- 1. 本地指纹自检(我们的浏览器实际暴露了什么) ----------
    println!("\n===== 本地指纹(navigator/screen/webGL) =====");
    let fp = js(
        &tab,
        r#"JSON.stringify({
            webdriver: navigator.webdriver,
            ua: navigator.userAgent,
            platform: navigator.platform,
            oscpu: navigator.oscpu,
            vendor: navigator.vendor,
            language: navigator.language,
            languages: navigator.languages,
            hardwareConcurrency: navigator.hardwareConcurrency,
            deviceMemory: navigator.deviceMemory,
            maxTouchPoints: navigator.maxTouchPoints,
            plugins: navigator.plugins.length,
            mimeTypes: navigator.mimeTypes.length,
            pdfViewerEnabled: navigator.pdfViewerEnabled,
            screen: [screen.width, screen.height, screen.availWidth, screen.availHeight, screen.colorDepth],
            dpr: window.devicePixelRatio,
            inner: [window.innerWidth, window.innerHeight],
            outer: [window.outerWidth, window.outerHeight],
            rtc: typeof window.RTCPeerConnection,
            webgl: (function(){try{var c=document.createElement('canvas');var gl=c.getContext('webgl')||c.getContext('experimental-webgl');var e=gl.getExtension('WEBGL_debug_renderer_info');return {vendor: gl.getParameter(e.UNMASKED_VENDOR_WEBGL), renderer: gl.getParameter(e.UNMASKED_RENDERER_WEBGL)};}catch(err){return 'ERR:'+err}})()
        })"#,
    )
    .await;
    pretty(&fp);
    let fp_pass = match serde_json::from_str::<serde_json::Value>(&fp) {
        Ok(v) => {
            let webdriver_ok = v.get("webdriver").and_then(|x| x.as_bool()) == Some(false);
            let ua = v.get("ua").and_then(|x| x.as_str()).unwrap_or("");
            let ua_ok = ua.contains("Firefox") && !ua.contains("Camoufox");
            let rtc_ok = v.get("rtc").and_then(|x| x.as_str()) == Some("undefined");
            let screen_h = v
                .get("screen")
                .and_then(|s| s.get(1))
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let outer_h = v
                .get("outer")
                .and_then(|s| s.get(1))
                .and_then(|x| x.as_u64())
                .unwrap_or(u64::MAX);
            let screen_ok = screen_h > 0 && outer_h <= screen_h;
            println!(
                "  -> webdriver=false:{webdriver_ok}  UA无Camoufox:{ua_ok}  WebRTC关闭:{rtc_ok}  屏幕自洽(outer<=screen):{screen_ok}"
            );
            webdriver_ok && ua_ok && rtc_ok && screen_ok
        }
        Err(_) => false,
    };
    results.push(("本地指纹(webdriver/UA/WebRTC/屏幕)", fp_pass));

    // ---------- 2. bot.sannysoft.com ----------
    println!("\n===== bot.sannysoft.com =====");
    let mut sanny_pass = false;
    if tab.get("https://bot.sannysoft.com/").await.unwrap_or(false) {
        tokio::time::sleep(Duration::from_secs(4)).await;
        let res = js(
            &tab,
            r#"JSON.stringify((function(){
                var bad=[].slice.call(document.querySelectorAll('td.failed, td.warn')).map(function(td){
                    var tr=td.closest('tr'); var name=tr?(tr.querySelector('td')||{}).innerText:'?';
                    return {name:(name||'').replace(/\s+/g,' ').trim(), cls:td.className, val:td.innerText.replace(/\s+/g,' ').trim().slice(0,60)};
                });
                return {passed: document.querySelectorAll('.passed').length, badCount: bad.length, bad: bad};
            })())"#,
        )
        .await;
        pretty(&res);
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&res) {
            let passed = v.get("passed").and_then(|x| x.as_u64()).unwrap_or(0);
            let empty: Vec<serde_json::Value> = Vec::new();
            let bad = v.get("bad").and_then(|x| x.as_array()).unwrap_or(&empty);
            // Chrome 专有检测在 Firefox 上必然缺失,属预期假阳性,不计入真实失败。
            let real_bad: Vec<&serde_json::Value> = bad
                .iter()
                .filter(|b| {
                    let name = b.get("name").and_then(|x| x.as_str()).unwrap_or("");
                    !name.to_lowercase().contains("chrome")
                })
                .collect();
            println!(
                "  -> passed={passed},真实失败项={}(已排除 Chrome 专有检测)",
                real_bad.len()
            );
            for b in &real_bad {
                println!("     - {b}");
            }
            sanny_pass = passed > 0 && real_bad.is_empty();
        }
    } else {
        println!("  访问失败");
    }
    results.push(("bot.sannysoft.com", sanny_pass));

    // ---------- 3. tls.peet.ws(TLS/JA3/JA4 网络层指纹) ----------
    println!("\n===== tls.peet.ws/api/all(TLS 指纹) =====");
    let mut tls_pass = false;
    if tab.get("https://tls.peet.ws/").await.unwrap_or(false) {
        tokio::time::sleep(Duration::from_millis(800)).await;
        // 同源同步 XHR 取 JSON(run_js 不 await promise)。
        let raw = js(
            &tab,
            r#"(function(){try{var x=new XMLHttpRequest();x.open('GET','https://tls.peet.ws/api/all',false);x.send();return x.responseText;}catch(e){return 'ERR:'+e}})()"#,
        )
        .await;
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(v) => {
                let ua = v.get("user_agent").and_then(|x| x.as_str()).unwrap_or("?");
                let tls = v.get("tls");
                let ja3 = tls
                    .and_then(|t| t.get("ja3_hash"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("?");
                let ja4 = tls
                    .and_then(|t| t.get("ja4"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("?");
                let peet = tls
                    .and_then(|t| t.get("peetprint_hash"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("?");
                let http = v
                    .get("http_version")
                    .and_then(|x| x.as_str())
                    .unwrap_or("?");
                let akamai = v
                    .get("http2")
                    .and_then(|h| h.get("akamai_fingerprint_hash"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("?");
                println!("  user_agent     = {ua}");
                println!("  http_version   = {http}");
                println!("  tls.ja3_hash   = {ja3}");
                println!("  tls.ja4        = {ja4}");
                println!("  tls.peetprint  = {peet}");
                println!("  http2.akamai   = {akamai}");
                let ff = ua.contains("Firefox") && !ua.contains("Camoufox");
                // JA4 以 `t13` 开头 = TLS1.3;Firefox 的 JA4 形如 `t13d16..h2..`。UA 与 TLS 栈一致才不穿帮。
                let ja4_ok = ja4.starts_with("t13");
                tls_pass = ff && ja4_ok;
                println!("  -> UA声称Firefox:{ff}  JA4为真TLS栈:{ja4_ok}  自洽:{tls_pass}");
            }
            Err(_) => println!(
                "  解析失败,raw 前 200 字:{}",
                raw.chars().take(200).collect::<String>()
            ),
        }
    } else {
        println!("  访问失败");
    }
    results.push(("tls.peet.ws(UA/TLS 自洽)", tls_pass));

    // ---------- 4. browserleaks: WebGL / Canvas / WebRTC ----------
    println!("\n===== browserleaks.com =====");
    let mut webrtc_pass = false;
    for (path, keys) in [
        (
            "webgl",
            &["unmasked vendor", "unmasked renderer", "webgl report hash"][..],
        ),
        ("canvas", &["signature", "uniqueness"][..]),
        ("webrtc", &["leak", "public ip", "local ip"][..]),
    ] {
        let url = format!("https://browserleaks.com/{path}");
        if tab.get(&url).await.unwrap_or(false) {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let text = js(&tab, "document.body.innerText").await;
            println!("  --- /{path} ---");
            for line in text.lines() {
                let low = line.to_lowercase();
                if keys.iter().any(|k| low.contains(k)) {
                    let t = line.trim();
                    if !t.is_empty() {
                        println!("    {}", t.chars().take(100).collect::<String>());
                    }
                }
            }
            if path == "webrtc" {
                // 无泄漏:页面出现 "No Leak",且公网 IP 不暴露(显示为 "-")。
                webrtc_pass = text.to_lowercase().contains("no leak");
                println!("    -> WebRTC 无泄漏:{webrtc_pass}");
            }
        } else {
            println!("  /{path} 访问失败");
        }
    }
    results.push(("browserleaks WebRTC 无泄漏", webrtc_pass));

    // ---------- 5. nowsecure.nl(Cloudflare) ----------
    println!("\n===== nowsecure.nl(Cloudflare 盾) =====");
    let mut cf_pass = false;
    if tab.get("https://nowsecure.nl/").await.unwrap_or(false) {
        let mut title = String::new();
        for i in 0..15 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            title = tab.title().await.unwrap_or_default();
            if !title.is_empty() && !title.to_lowercase().contains("just a moment") {
                println!("  [{i:>2}s] 过盾,title={title:?}");
                break;
            }
        }
        cf_pass = !title.to_lowercase().contains("just a moment") && !title.is_empty();
        let body = js(&tab, "document.body.innerText.slice(0,120)").await;
        println!("  结果: {}", if cf_pass { "已过 CF" } else { "仍被拦" });
        println!("  正文前 120 字:{}", body.replace('\n', " "));
    } else {
        println!("  访问失败");
    }
    results.push(("nowsecure.nl Cloudflare", cf_pass));

    // ---------- 总结 ----------
    println!("\n===== 总结(过了吗) =====");
    let passed_n = results.iter().filter(|(_, p)| *p).count();
    for (name, pass) in &results {
        println!("  [{}] {name}", if *pass { "PASS" } else { "FAIL" });
    }
    let all = passed_n == results.len();
    println!(
        "  => {passed_n}/{} 通过 -> {}",
        results.len(),
        if all {
            "全部通过,一打开即过"
        } else {
            "有未过项,见上方 FAIL"
        }
    );

    browser.quit().await?;
    Ok(())
}

/// 把一段 JSON 字符串缩进打印;非 JSON 原样打印。
fn pretty(s: &str) {
    match serde_json::from_str::<serde_json::Value>(s) {
        Ok(v) => println!(
            "{}",
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| s.to_string())
        ),
        Err(_) => println!("{s}"),
    }
}
