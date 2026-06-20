//! 下载管理 `tab.downloads()` 端到端自验证(完全离线)。
//!
//! 本地 `file://` 页放两个 `<a download href="data:...">`,点击触发**两次下载**;用下载句柄收集、
//! 等待完成、读任务列表、自定义重命名、读已下载字节数。
//!
//! 运行:`cargo run --example download_manager`
//!
//! 末尾打印 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`,关键校验失败则进程非 0 退出。

use std::time::Duration;

use drission::prelude::*;

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>dl</title></head>
<body>
  <a id="d1" download="alpha.txt" href="data:text/plain,alpha-content">下载 alpha</a>
  <a id="d2" download="beta.txt" href="data:text/plain,beta-content">下载 beta</a>
</body></html>"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("drission-dl");
    let _ = tokio::fs::remove_dir_all(&base).await; // 清理上次残留
    let dl_dir = base.join("downloads");
    tokio::fs::create_dir_all(&dl_dir).await?;
    let page_path = base.join("page.html");
    tokio::fs::write(&page_path, PAGE).await?;
    let url = format!("file://{}", page_path.display());

    println!("[*] 启动 Camoufox(headless),下载目录={}", dl_dir.display());
    let browser =
        Browser::launch(BrowserOptions::new().headless(true).download_path(&dl_dir)).await?;
    let tab = browser.latest_tab().await?;
    tab.get(&url).await?;
    tab.wait().ele_displayed("#d1", None).await?;

    let dl = tab.downloads();
    dl.start().await?;
    let listening = dl.listening();

    // 触发两次下载(顺序点击,各等其完成)。
    tab.ele("#d1").await?.click().await?;
    let m1 = dl.wait_done(Duration::from_secs(15)).await?;
    tab.ele("#d2").await?.click().await?;
    let m2 = dl.wait_done(Duration::from_secs(15)).await?;

    let m1 = match m1 {
        Some(m) => m,
        None => return finish(&browser, false, "未等到第 1 个下载完成").await,
    };
    let m2 = match m2 {
        Some(m) => m,
        None => return finish(&browser, false, "未等到第 2 个下载完成").await,
    };
    println!(
        "[1] 下载1: {} → {:?} 成功={}",
        m1.suggested_filename,
        m1.path,
        m1.succeeded()
    );
    println!(
        "[2] 下载2: {} → {:?} 成功={}",
        m2.suggested_filename,
        m2.path,
        m2.succeeded()
    );

    // 内容核对(data:text/plain,alpha-content → "alpha-content")。
    let c1 = tokio::fs::read_to_string(&m1.path)
        .await
        .unwrap_or_default();
    let bytes1 = m1.downloaded_bytes().await;
    let content_ok = c1 == "alpha-content" && bytes1 == c1.len() as u64;
    println!("[3] 下载1 内容={c1:?} 字节={bytes1} (ok={content_ok})");

    // 任务列表快照。
    let missions = dl.missions().await;
    let list_ok = missions.len() == 2 && missions.iter().all(|m| m.succeeded());
    println!(
        "[4] missions 数={} 全部成功={} (ok={list_ok})",
        missions.len(),
        list_ok
    );

    // 自定义重命名(把下载2 移走)。
    let renamed = base.join("renamed-beta.txt");
    let saved = m2.save_as(&renamed).await?;
    let rename_ok = tokio::fs::try_exists(&saved).await.unwrap_or(false)
        && tokio::fs::read_to_string(&saved).await.unwrap_or_default() == "beta-content";
    println!("[5] save_as → {:?} (ok={rename_ok})", saved);

    dl.stop().await?;
    let stopped = !dl.listening();

    let names_ok = m1.suggested_filename == "alpha.txt" && m2.suggested_filename == "beta.txt";
    let pass = listening
        && stopped
        && m1.succeeded()
        && m2.succeeded()
        && content_ok
        && list_ok
        && rename_ok
        && names_ok;
    finish(&browser, pass, "download_manager 自验证未通过").await
}

async fn finish(browser: &Browser, pass: bool, err: &str) -> drission::Result<()> {
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
        Err(drission::Error::msg(err.to_string()))
    }
}
