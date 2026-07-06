//! auth.exa.ai 交互式过盾(**CDP 后端 · 谷歌浏览器 · 有头**)。
//!
//! 填邮箱 → 触发 Cloudflare Turnstile → 等待其自动产出 token。
//! 过盾判定:`input[name=cf-turnstile-response]` 的 value 变为非空(有效 token)。
//! 只做到过盾为止,不真正完成登录。
//!
//! 这是 CDP 后端的反检测验证案例(里程碑 52):用**本机 Google Chrome、有头**驱动,
//! 反检测默认开启(`stealth=true`)—— 反检测启动参数 + 导航前注入 + **不调用 `Runtime.enable`**
//! (经典 CDP 探测泄漏)。详见 `docs/CDP过盾.md`。
//!
//! 运行:`cargo run --example exa_cf --features cdp`
//! (默认构建即含 cdp,也可直接 `cargo run --example exa_cf`)

use std::time::Duration;

use drission::prelude::*;

const URL: &str = "https://auth.exa.ai/?callbackUrl=https%3A%2F%2Fdashboard.exa.ai%2F";
const EMAIL: &str = "12341423@gmail.com";
const TURNSTILE_TOKEN_JS: &str = "(() => { const e = document.querySelector('[name=cf-turnstile-response]'); return e ? e.value : ''; })()";

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    // 有头 + 反检测(默认)的谷歌浏览器。未指定 binary_path 时自动定位系统已装 Chrome
    // (找不到才下载 Chrome for Testing)。默认有头;`HEADLESS=1` 跑无头对照。
    let headless = matches!(
        std::env::var("HEADLESS").ok().as_deref(),
        Some("1") | Some("true")
    );
    // FULLCH=1:无头补全高熵 Client Hints(`full_ua_metadata`),验证补环境不破坏过盾。
    let full_ch = matches!(std::env::var("FULLCH").ok().as_deref(), Some("1"));
    // HIDDEN=1:**有头但隐藏**(窗口移到屏幕外 -32000,-32000 + 关闭遮挡节流)——保留真实 GPU 渲染
    // (过 Turnstile 的关键:无头常退化 SwiftShader 软渲染被识破),但用户看不到窗口。等价"视觉无头"。
    let hidden = matches!(
        std::env::var("HIDDEN").ok().as_deref(),
        Some("1") | Some("true")
    );
    println!(
        "模式: {} | full_ua_metadata={full_ch} | hidden={hidden}",
        if headless { "无头" } else { "有头" }
    );
    // 统一接口名:`Browser`/`BrowserOptions`(cdp feature 下=Chromium 后端,camoufox 下=Camoufox 后端)。
    // 同一份代码切 feature 即换协议。
    let mut opts = BrowserOptions::new()
        .headless(headless)
        .full_ua_metadata(full_ch)
        .window_size(1280, 800);
    // CF_PROXY=http://127.0.0.1:7890 → 走住宅出口(与纯协议实验同 IP,apples-to-apples 消歧)
    if let Ok(p) = std::env::var("CF_PROXY") {
        if !p.is_empty() {
            println!("[*] 走代理出口: {p}");
            opts = opts.proxy(p);
        }
    }
    if hidden {
        opts = opts
            .add_arg("--window-position=-32000,-32000")
            .add_arg("--disable-backgrounding-occluded-windows")
            .add_arg("--disable-renderer-backgrounding")
            .add_arg("--disable-features=CalculateNativeWinOcclusion");
    }
    let browser = Browser::launch(opts).await?;
    let tab = browser.latest_tab().await?;

    tab.get(URL).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 反检测自检:有头干净 Chrome 应为 false(被 CF 一眼识破的头号信号)。
    let webdriver = tab.run_js("navigator.webdriver").await.unwrap_or_default();
    let ua = tab.user_agent().await.unwrap_or_default();
    // 客户端提示品牌(Sec-CH-UA):应不含 "HeadlessChrome"。
    let brands = tab
        .run_js("JSON.stringify((navigator.userAgentData&&navigator.userAgentData.brands)||[])")
        .await
        .unwrap_or_default();
    println!("navigator.webdriver = {webdriver}");
    println!("userAgent          = {ua}");
    println!("uaData.brands      = {brands}");

    // 填邮箱(只碰 type=email 的真实输入框,避开 name=website 蜜罐)。逐字符拟人输入更利于
    // Turnstile 的行为判定。
    let email = tab.ele("css:input[type=email]").await?;
    email.click().await?; // 可信点击(isTrusted=true),给 Turnstile 真实交互信号
    email.input_human(EMAIL).await?;
    println!("已填邮箱: {EMAIL}");

    // 轮询等待 Turnstile 自动产出 token。
    let mut token_len = 0usize;
    for i in 0..30 {
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let token = tab.run_js(TURNSTILE_TOKEN_JS).await.unwrap_or_default();
        token_len = token.as_str().unwrap_or("").len();
        println!("  [{i:>2}s] turnstile_token_len = {token_len}");
        if token_len > 20 {
            break;
        }
    }

    if token_len <= 20 {
        // 兜底:交互式 Turnstile(需点复选框)再试一次可信点击;**仍以 token 为准**(不看 pass 的返回)。
        println!("\n未自动出 token,尝试交互式 Turnstile 可信点击…");
        let _ = tab.pass_cloudflare(Duration::from_secs(15)).await;
        let token = tab.run_js(TURNSTILE_TOKEN_JS).await.unwrap_or_default();
        token_len = token.as_str().unwrap_or("").len();
    }

    if token_len > 20 {
        println!("\n结果: 已过 CF 盾(Turnstile 有效 token,长度 {token_len})");
    } else {
        println!("\n结果: 未过(无 Turnstile token)。可重试,或检查出网 IP 是否被风控。");
    }

    // 视觉证据:存一张视口截图到工作目录(过盾后 Turnstile 应显示「成功!」)。
    match tab.screenshot_bytes().await {
        Ok(bytes) => match std::fs::write("exa_cf_shot.png", &bytes) {
            Ok(_) => println!("SHOT_SAVED exa_cf_shot.png ({} bytes)", bytes.len()),
            Err(e) => println!("SHOT_WRITE_FAIL {e}"),
        },
        Err(e) => println!("SHOT_CAPTURE_FAIL {e}"),
    }

    browser.quit().await?;
    Ok(())
}
