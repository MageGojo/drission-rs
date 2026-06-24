//! Windows 端到端验证(过 CF 盾版):有头启动 → 访问一个 Cloudflare 保护的页面 →
//! 轮询标题看 challenge 是否自动通过。比 bilibili 用例更干净(不点元素,只导航+读标题),
//! 适合判定「Windows 传输 + 浏览器渲染 + 反检测」整条链路到底通不通。
//!
//! 逻辑同 `examples/cf_check.rs`,额外把结果落盘成 JSON(`drission_win_test_result.json`)。
//!
//! 运行(原生 Windows 构建,或用 scripts/win-cross-build.sh 交叉后在 Win 上跑 .exe):
//!   `cargo run --example win_cf_test --no-default-features --features camoufox`  /  `win_cf_test.exe [URL]`
//!   (默认 scrapingcourse 的 cloudflare-challenge 页)

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use drission::prelude::*;
use serde_json::{Value, json};

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://www.scrapingcourse.com/cloudflare-challenge".to_string());

    let t0 = SystemTime::now();
    let started_ms = t0
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    println!("== drission-rs Windows 测试 (过 CF 盾) ==");
    println!(
        "  OS/ARCH : {}/{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("  URL     : {url}");

    let cam = drission::launcher::ensure_camoufox(None).await?;
    println!("  Camoufox: {}", cam.display());
    println!();

    let mut title_history: Vec<String> = Vec::new();
    let mut final_title = String::new();
    let mut passed = false;
    let mut user_agent = String::new();
    let mut webdriver = String::new();
    let mut err_msg: Option<String> = None;
    let mut cf_debug_before: Option<Value> = None;
    let mut cf_debug_after: Option<Value> = None;

    let run = async {
        // 反检测默认开;CF 对地区敏感,这里给 en-US / 纽约时区(与多数测试页/IP 匹配)。
        let browser = Browser::launch(
            BrowserOptions::new()
                .headless(false) // 有头:弹出可见 Camoufox 窗口
                .locale("en-US")
                .timezone("America/New_York"),
        )
        .await?;
        let tab = browser.latest_tab().await?;

        println!("访问中...");
        tab.get(&url).await?;

        user_agent = tab.user_agent().await?;
        webdriver = tab.run_js("navigator.webdriver").await?.to_string();

        // 点击前的 CF 现场(iframe 列表/位置/标记),万一点歪/没点能据此核对。
        cf_debug_before = tab.cloudflare_debug().await.ok();
        println!("点击前 CF 现场: {cf_debug_before:?}");

        // 自动过盾:交互式 Turnstile 会被拟人**可信点击**复选框,非交互式等待自动放行。
        println!("尝试自动通过 Cloudflare(最多 40s)…");
        passed = tab
            .pass_cloudflare(Duration::from_secs(40))
            .await
            .unwrap_or(false);

        cf_debug_after = tab.cloudflare_debug().await.ok();
        final_title = tab.title().await.unwrap_or_default();
        title_history.push(final_title.clone());
        println!("最终标题: {final_title:?} / 过盾: {passed}");

        browser.quit().await?;
        Ok::<(), drission::Error>(())
    }
    .await;

    if let Err(e) = run {
        err_msg = Some(e.to_string());
    }

    let elapsed_ms = t0.elapsed().map(|d| d.as_millis()).unwrap_or(0);
    // 链路判定:能拿到非空标题(说明浏览器真的渲染了页面)就算「传输通」;passed 另算「是否过盾」。
    let transport_ok = err_msg.is_none() && !final_title.is_empty();

    let result = json!({
        "tool": "drission-rs",
        "test": "win_cf_check",
        "platform": { "os": std::env::consts::OS, "arch": std::env::consts::ARCH },
        "camoufox_path": cam.display().to_string(),
        "url": url,
        "transport_ok": transport_ok,
        "cf_passed": passed,
        "final_title": final_title,
        "title_history": title_history,
        "navigator": { "userAgent": user_agent, "webdriver": webdriver },
        "cf_debug_before": cf_debug_before,
        "cf_debug_after": cf_debug_after,
        "error": err_msg,
        "started_unix_ms": started_ms,
        "elapsed_ms": elapsed_ms,
    });

    let out = "drission_win_test_result.json";
    let pretty = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
    if let Err(e) = std::fs::write(out, &pretty) {
        eprintln!("写结果文件失败: {e}");
    }

    println!();
    println!(
        "传输链路:{}",
        if transport_ok {
            "通 ✅"
        } else {
            "不通 ❌"
        }
    );
    println!(
        "CF 盾   :{}",
        if passed {
            "已过 ✅"
        } else {
            "未过(可能被风控,与传输无关)"
        }
    );
    println!("最终标题:{final_title:?}");
    println!("结果文件:{out}(请把这个文件发回核对)");

    if !transport_ok {
        std::process::exit(1);
    }
    Ok(())
}
