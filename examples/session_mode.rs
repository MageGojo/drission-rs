//! Session(HTTP)双模 + cookie 互通自验证(对标 DrissionPage 的 Driver+Session)。
//!
//! 两部分:
//! 1. **纯 HTTP**(完全不开浏览器,极省内存/CPU):`SessionPage` 抓 example.com,读状态/标题、
//!    用与 Driver 同语法的 `s_ele/s_eles` 离线解析。
//! 2. **浏览器 → Session 的 cookie 交接**:启动浏览器、在其 BrowserContext 写一个 cookie,
//!    `load_cookies_from_tab` 灌进 Session,验证拿到;再 `save_cookies`/`load_cookies_file`
//!    存盘读盘(复用登录态),并 `apply_cookies_to_tab` 回灌浏览器。
//!
//! 运行:`cargo run --example session_mode`
//! 结果落 `drission_session_result.json`,末行打印 ALL CHECKS PASSED / FAILED。

use drission::prelude::*;
use serde_json::json;

fn record(checks: &mut Vec<(String, bool, String)>, name: &str, ok: bool, detail: String) {
    println!("  [{}] {name}: {detail}", if ok { "OK" } else { "!!" });
    checks.push((name.to_string(), ok, detail));
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com".to_string());

    let mut checks: Vec<(String, bool, String)> = Vec::new();

    println!("== drission-rs Session 双模自验证 ==");
    println!(
        "  OS/ARCH : {}/{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("  URL     : {url}\n");

    // ── 1) 纯 HTTP(不开浏览器)────────────────────────────────────────────
    println!("[1] 纯 HTTP(SessionPage,不开浏览器)…");
    let mut sess = match SessionPage::new_default() {
        Ok(s) => s,
        Err(e) => {
            record(&mut checks, "session_new", false, e.to_string());
            finish(&checks, &url);
            return;
        }
    };
    match sess.get(&url).await {
        Ok(ok2xx) => {
            record(
                &mut checks,
                "http_get",
                ok2xx,
                format!("status={} url={}", sess.status(), sess.url()),
            );
            let title = sess.title().unwrap_or_default();
            record(
                &mut checks,
                "http_title",
                !title.is_empty(),
                format!("{title:?}"),
            );
            match sess.s_ele("tag:h1") {
                Ok(h1) => {
                    let t = h1.text().unwrap_or_default();
                    record(&mut checks, "s_ele_h1", !t.is_empty(), t);
                }
                Err(e) => record(&mut checks, "s_ele_h1", false, e.to_string()),
            }
            let links = sess.s_eles("tag:a").map(|v| v.len()).unwrap_or(0);
            record(
                &mut checks,
                "s_eles_links",
                links >= 1,
                format!("{links} 个链接"),
            );
        }
        Err(e) => record(&mut checks, "http_get", false, e.to_string()),
    }

    // ── 2) 浏览器 → Session cookie 交接 ───────────────────────────────────
    println!("\n[2] 浏览器 → Session 的 cookie 交接…");
    let interop = browser_cookie_interop(&url).await;
    match interop {
        Ok((got_in_session, after_disk_roundtrip, applied_back)) => {
            record(
                &mut checks,
                "cookie_browser_to_session",
                got_in_session,
                "灌入会话".into(),
            );
            record(
                &mut checks,
                "cookie_save_load_disk",
                after_disk_roundtrip,
                "存盘读盘复用".into(),
            );
            record(
                &mut checks,
                "cookie_session_to_browser",
                applied_back,
                "回灌浏览器".into(),
            );
        }
        Err(e) => {
            // 浏览器不可用时不算硬失败(纯 HTTP 已证 Session 模式);但记录原因。
            record(
                &mut checks,
                "cookie_interop",
                false,
                format!("浏览器交接跳过/失败: {e}"),
            );
        }
    }

    finish(&checks, &url);
}

/// 启动浏览器 → 写 cookie → 灌进 Session → 存盘读盘 → 回灌浏览器。
/// 返回 (灌入成功, 存盘读盘后仍在, 回灌成功)。
async fn browser_cookie_interop(url: &str) -> drission::Result<(bool, bool, bool)> {
    let host = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_else(|| "example.com".to_string());

    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;

    // 在浏览器 BrowserContext 写一个测试 cookie(无需导航)。
    tab.set_cookies(vec![CookieParam {
        name: "drission_sess".to_string(),
        value: "hello-from-browser".to_string(),
        url: Some(url.to_string()),
        domain: Some(host.clone()),
        path: Some("/".to_string()),
        secure: Some(false),
        http_only: Some(false),
        expires: None,
    }])
    .await?;

    // 浏览器 → Session。
    let mut sess = SessionPage::new_default()?;
    sess.load_cookies_from_tab(&tab).await?;
    let got_in_session = sess
        .cookies()
        .iter()
        .any(|c| c.name == "drission_sess" && c.value == "hello-from-browser");

    // 存盘 → 清空 → 读盘,验证持久化复用登录态。
    let cookie_file = std::env::temp_dir()
        .join("drission_session_cookies.json")
        .to_string_lossy()
        .to_string();
    sess.save_cookies(&cookie_file)?;
    sess.clear_cookies();
    sess.load_cookies_file(&cookie_file)?;
    let after_disk_roundtrip = sess.cookies().iter().any(|c| c.name == "drission_sess");
    let _ = std::fs::remove_file(&cookie_file);

    // Session → 浏览器(回灌)。
    let applied_back = sess.apply_cookies_to_tab(&tab).await.is_ok();

    browser.quit().await?;
    Ok((got_in_session, after_disk_roundtrip, applied_back))
}

fn finish(checks: &[(String, bool, String)], url: &str) {
    let all_ok = checks.iter().all(|(_, ok, _)| *ok);
    let result = json!({
        "tool": "drission-rs",
        "test": "session_mode",
        "platform": { "os": std::env::consts::OS, "arch": std::env::consts::ARCH },
        "url": url,
        "all_passed": all_ok,
        "checks": checks.iter().map(|(n, ok, d)| json!({"name": n, "ok": ok, "detail": d})).collect::<Vec<_>>(),
    });
    let out = "drission_session_result.json";
    let pretty = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
    if let Err(e) = std::fs::write(out, &pretty) {
        eprintln!("写结果文件失败: {e}");
    }
    println!("\n结果文件:{out}");
    println!(
        "{}",
        if all_ok {
            "ALL CHECKS PASSED ✅"
        } else {
            "SOME CHECKS FAILED ❌"
        }
    );
}
