//! 锦上添花(批次一)自验证:**逐字符拟人输入** + **登录态全量持久化(storageState)**。
//!
//! - A. `ele.input_human(text)`:逐字符敲(keydown+insertText+keyup,随机停顿),读回 value 核对。
//! - B. `tab.save_storage_state` / `load_storage_state`:在一个标签设置 cookie + localStorage,
//!   导出到磁盘;在**另一个全新标签(独立 BrowserContext)**导航同源后导入,验证 cookie 与
//!   localStorage 都被还原(= 跨会话复用登录态)。
//!
//! 运行:`cargo run --example extras_demo --no-default-features --features camoufox`   末行 ALL CHECKS PASSED / FAILED。

use drission::prelude::*;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let mut checks: Vec<(String, bool, String)> = Vec::new();
    let ok = run(&mut checks).await;
    if let Err(e) = ok {
        checks.push(("run".into(), false, e.to_string()));
    }
    let all = checks.iter().all(|(_, ok, _)| *ok);
    println!();
    for (n, ok, d) in &checks {
        println!("  [{}] {n}: {d}", if *ok { "OK" } else { "!!" });
    }
    println!(
        "\n{}",
        if all {
            "ALL CHECKS PASSED ✅"
        } else {
            "SOME CHECKS FAILED ❌"
        }
    );
    if !all {
        std::process::exit(1);
    }
}

async fn run(checks: &mut Vec<(String, bool, String)>) -> drission::Result<()> {
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;

    // ── A. 逐字符拟人输入 ────────────────────────────────────────────────
    tab.get("data:text/html,<input id=q style='width:300px'>")
        .await?;
    tab.wait()
        .ele_displayed("#q", Some(std::time::Duration::from_secs(5)))
        .await?;
    let q = tab.ele("#q").await?;
    let typed = "hello drission 自动化";
    q.input_human(typed).await?;
    let got = q.value().await?;
    checks.push(("input_human".into(), got == typed, format!("value={got:?}")));

    // ── B. 登录态持久化(cookie + localStorage)──────────────────────────
    let site = "https://example.com";
    tab.get(site).await?;
    // 等真实页面就绪(避免首读落到旧 about:blank 上下文 → localStorage 报 insecure)。
    tab.wait()
        .ele_displayed("h1", Some(std::time::Duration::from_secs(8)))
        .await?;
    // 在该源写 localStorage + 一个 cookie。
    tab.run_js("localStorage.setItem('drission_k','v1'); sessionStorage.setItem('s_k','s1'); true")
        .await?;
    tab.set_cookies(vec![CookieParam {
        name: "drission_login".into(),
        value: "token-123".into(),
        url: Some(site.into()),
        domain: Some("example.com".into()),
        path: Some("/".into()),
        secure: Some(true),
        http_only: Some(false),
        expires: None,
    }])
    .await?;

    let state_file = std::env::temp_dir()
        .join("drission_storage_state.json")
        .to_string_lossy()
        .to_string();
    tab.save_storage_state(&state_file).await?;
    let saved = tab.storage_state().await?;
    let has_origin = saved.origins.iter().any(|o| {
        o.local_storage
            .iter()
            .any(|(k, v)| k == "drission_k" && v == "v1")
    });
    checks.push((
        "storage_export".into(),
        has_origin && !saved.cookies.is_empty(),
        format!(
            "cookies={} origins={}",
            saved.cookies.len(),
            saved.origins.len()
        ),
    ));

    // 全新标签 = 独立 BrowserContext(无 cookie / 无 storage),模拟新会话。
    let tab2 = browser.new_tab(Some(site)).await?;
    tab2.wait()
        .ele_displayed("h1", Some(std::time::Duration::from_secs(8)))
        .await?;
    let before = tab2
        .run_js("localStorage.getItem('drission_k')")
        .await?
        .as_str()
        .map(|s| s.to_string());
    checks.push((
        "fresh_context_empty".into(),
        before.is_none(),
        format!("localStorage(before)={before:?}"),
    ));

    // 导入登录态:cookie 全量 + 当前源 storage。
    tab2.load_storage_state(&state_file).await?;
    let after = tab2
        .run_js("localStorage.getItem('drission_k')")
        .await?
        .as_str()
        .unwrap_or_default()
        .to_string();
    checks.push((
        "storage_restored".into(),
        after == "v1",
        format!("localStorage(after)={after:?}"),
    ));
    let ck = tab2.cookies().await?;
    let cookie_ok = ck
        .iter()
        .any(|c| c.name == "drission_login" && c.value == "token-123");
    checks.push((
        "cookie_restored".into(),
        cookie_ok,
        format!("{} cookies in new context", ck.len()),
    ));

    let _ = std::fs::remove_file(&state_file);
    browser.quit().await?;
    Ok(())
}
