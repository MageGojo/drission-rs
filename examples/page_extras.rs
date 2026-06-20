//! 进阶页面能力端到端自验证:iframe 内元素 / JS 对话框 / 文件上传 / 静态 XPath。
//!
//! 用本地 `file://` 页面(含 iframe srcdoc、file input、confirm/prompt 按钮、列表),
//! 全程不依赖网络,确定性强。
//!
//! 运行:`cargo run --example page_extras`
//!
//! 末尾打印 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`,关键校验失败则进程非 0 退出。

use drission::prelude::*;

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>extras</title></head>
<body>
  <h1 id="title">Extras</h1>
  <ul id="list"><li class="item">一</li><li class="item">二</li><li class="item">三</li></ul>
  <input type="file" id="file">
  <iframe id="ifr" srcdoc="<!doctype html><html><body><p id='inner'>inside-iframe</p><button id='ibtn'>ok</button></body></html>"></iframe>
</body></html>"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // 准备本地页面与上传文件。
    // 关键:写到项目目录(用户 home 下)而非系统临时目录——Camoufox/Firefox 的 macOS
    // 内容进程沙箱默认允许读 home 下的文件,但拒读 `/var/folders` 系统临时目录,后者会让
    // file:// 加载成空白文档(html 仅 `<html><head></head><body></body></html>`)。
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("drission-extras");
    tokio::fs::create_dir_all(&dir).await?;
    let page_path = dir.join("page.html");
    tokio::fs::write(&page_path, PAGE).await?;
    let upload_path = dir.join("upload.txt");
    tokio::fs::write(&upload_path, b"hello upload").await?;
    let url = format!("file://{}", page_path.display());

    println!("[*] 启动 Camoufox(headless)…");
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;
    let ok_get = tab.get(&url).await?;
    println!("[*] get({url}) ok={ok_get}");

    // 加载健全性检查:沙箱拒读 / 加载失败会得到空白文档(~39 字节)。
    let html = tab.html().await?;
    if html.len() < 80 {
        return Err(drission::Error::msg(format!(
            "页面未正确加载(html_len={},疑似 file:// 被沙箱拒读):{url}",
            html.len()
        )));
    }

    // ---------- 静态 XPath ----------
    let li_count = tab.s_eles("xpath://ul[@id='list']/li").await?.len();
    let li2 = tab.s_ele("xpath://li[2]").await?.text()?;
    let title = tab.s_ele("xpath://*[@id='title']").await?.text()?;
    let xpath_ok = li_count == 3 && li2 == "二" && title == "Extras";
    println!("[1] 静态XPath: //ul/li 数={li_count}  //li[2]={li2:?}  //*[@id=title]={title:?}  (ok={xpath_ok})");

    // ---------- iframe 内元素 ----------
    let frame = tab.get_frame("#ifr").await?;
    let inner_text = frame.ele("#inner").await?.text().await?;
    frame.ele("#ibtn").await?.click().await?; // 点击 iframe 内按钮(验证不报错)
    let iframe_ok = inner_text == "inside-iframe";
    println!("[2] iframe: #inner 文本={inner_text:?}  点击 #ibtn ok  (ok={iframe_ok})");

    // ---------- 文件上传 ----------
    let upload_str = upload_path.to_string_lossy().to_string();
    tab.ele("#file").await?.set_files(&[upload_str.as_str()]).await?;
    let fname = tab
        .run_js("(document.getElementById('file').files[0]||{}).name || ''")
        .await?;
    let upload_ok = fname.as_str() == Some("upload.txt");
    println!("[3] 文件上传: input.files[0].name={fname}  (ok={upload_ok})");

    // ---------- JS 对话框(confirm + prompt)----------
    // confirm:与触发动作并发——run_js 会阻塞到对话框被处理。
    let (confirm_res, confirm_info) =
        tokio::join!(tab.run_js("confirm('go?')"), tab.handle_next_dialog(true, None));
    let confirm_info = confirm_info?;
    let confirm_ok = confirm_res?.as_bool() == Some(true) && confirm_info.message == "go?";
    println!(
        "[4] confirm: 返回={confirm_ok}  type={:?} message={:?}",
        confirm_info.dialog_type, confirm_info.message
    );

    // prompt:接受并填入文本。
    let (prompt_res, prompt_info) = tokio::join!(
        tab.run_js("prompt('name?', 'def')"),
        tab.handle_next_dialog(true, Some("hello"))
    );
    let _ = prompt_info?;
    let prompt_ok = prompt_res?.as_str() == Some("hello");
    println!("[4] prompt: 返回={prompt_ok}(应为 \"hello\")");

    let pass = ok_get && xpath_ok && iframe_ok && upload_ok && confirm_ok && prompt_ok;
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
        Err(drission::Error::msg("page_extras 自验证未通过"))
    }
}
