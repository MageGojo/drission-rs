//! 页面基础能力(对标 DrissionPage)端到端自验证:覆盖本次新增的 A/B/C/D 四块。
//!
//! - **A** 健壮 `get`(retry/load_mode)+ 页面状态(`url`/`title`/`user_agent`/`ready_state`/`url_available`)。
//! - **B** 静态元素 `s_ele`/`s_eles`(离线解析),并与实时 `ele` 交叉核对文本是否一致。
//! - **C** 句柄对象 `tab.wait.*` / `tab.scroll.*` / `tab.set.*`。
//! - **D** 截图 `get_screenshot` + 页面尺寸 `size`/`page_size`/`rect`。
//!
//! 运行:`cargo run --example page_basics --no-default-features --features camoufox`
//!
//! 末尾会打印一行 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`,关键校验不通过则进程以非 0 退出。

use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let url = "https://example.com";
    println!("[*] 启动 Camoufox(headless)…");
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;

    // ---------- A:健壮 get + 页面状态 ----------
    let ok = tab
        .get_with(
            url,
            &GetOptions::new()
                .retry(2)
                .interval(0.5)
                .load_mode(LoadMode::Normal),
        )
        .await?;
    let title = tab.title().await?;
    let cur_url = tab.url().await?;
    let ua = tab.user_agent().await?;
    let ready = tab.ready_state().await?;
    println!("[A] get ok={ok}  url_available={}", tab.url_available());
    println!("    title={title:?}");
    println!("    url={cur_url:?}");
    println!("    readyState={ready:?}");
    println!("    UA={ua:?}");

    // ---------- C:wait 句柄 ----------
    let doc_loaded = tab.wait().doc_loaded(None).await?;
    let h1_displayed = tab
        .wait()
        .ele_displayed("tag:h1", Some(Duration::from_secs(5)))
        .await?;
    println!("[C] wait.doc_loaded={doc_loaded}  wait.ele_displayed(h1)={h1_displayed}");

    // ---------- D:尺寸 + 截图 ----------
    let (vw, vh) = tab.size().await?;
    let (pw, ph) = tab.page_size().await?;
    let rect = tab.rect().await?;
    println!(
        "[D] viewport={vw}x{vh}  page={pw}x{ph}  dpr={}",
        rect.device_pixel_ratio
    );
    let shot_path = std::env::temp_dir().join("drission-page-basics/example.png");
    let saved = tab.get_screenshot(&shot_path, true).await?;
    let shot_len = tokio::fs::metadata(&saved).await?.len();
    println!("[D] 截图已存:{} ({} bytes)", saved.display(), shot_len);

    // ---------- C:scroll 句柄 ----------
    tab.scroll().to_bottom().await?;
    tab.scroll().to_top().await?;
    println!("[C] scroll.to_bottom()/to_top() 已执行");

    // ---------- B:静态元素 + 与实时 ele 交叉核对 ----------
    let live_h1 = tab.ele("tag:h1").await?.text().await?;
    let static_h1 = tab.s_ele("tag:h1").await?.text()?;
    let p_count = tab.s_eles("tag:p").await?.len();
    let links = tab.s_eles("tag:a").await?;
    let first_link = match links.first() {
        Some(a) => Some((a.attr("href")?, a.text()?)),
        None => None,
    };
    let h1_match = live_h1.trim() == static_h1.trim();
    println!("[B] live ele(h1).text()   = {live_h1:?}");
    println!("[B] static s_ele(h1).text()= {static_h1:?}  (一致={h1_match})");
    println!("[B] s_eles(p)={p_count}  s_eles(a)={}", links.len());
    if let Some((href, text)) = &first_link {
        println!("    首个链接 href={href:?} text={text:?}");
    }

    // ---------- C:set 句柄(改超时 / 加载模式 / UA),再访问验证 UA 生效 ----------
    tab.set().timeout(Duration::from_secs(20));
    tab.set().load_mode(LoadMode::Eager);
    let custom_ua = "drission-rs/0.1 (set.user_agent test)";
    tab.set().user_agent(custom_ua).await?;
    tab.get(url).await?;
    let ua2 = tab.user_agent().await?;
    let ua_applied = ua2.contains("set.user_agent test");
    println!("[C] set.user_agent 后 UA={ua2:?}  (生效={ua_applied})");

    // ---------- 汇总自验证 ----------
    let pass = ok
        && doc_loaded
        && h1_displayed
        && h1_match
        && p_count > 0
        && shot_len > 0
        && first_link.is_some()
        && ua_applied;
    println!(
        "\n==== {} ====",
        if pass {
            "ALL CHECKS PASSED"
        } else {
            "SOME CHECKS FAILED"
        }
    );

    browser.quit().await?;
    if pass {
        Ok(())
    } else {
        Err(drission::Error::msg("page_basics 自验证未通过"))
    }
}
