//! 元素相对定位 + Shadow DOM 端到端自验证(对标 DrissionPage)。
//!
//! 用本地 `file://` 页面(含嵌套列表 + open shadow root),全程不依赖网络、确定性强。
//! 覆盖:
//! - 相对定位:`parent`/`parent_n`/`parent_until`/`children`/`child`/`next`/`prev`/
//!   `nexts`/`prevs`/`siblings`;
//! - shadow:`ele.shadow_root()` → `root.ele/eles/html`,以及 shadow 内 `xpath:` 应报错。
//!
//! 运行:`cargo run --example relative_shadow`
//!
//! 末尾打印 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`,关键校验失败则进程非 0 退出。

use drission::prelude::*;

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>relshadow</title></head>
<body>
  <div id="box" class="container">
    <h1 id="title">Rel</h1>
    <ul id="list">
      <li class="item" id="i1">一</li>
      <li class="item" id="i2">二</li>
      <li class="item" id="i3">三</li>
    </ul>
  </div>
  <div id="host"></div>
  <script>
    const r = document.getElementById('host').attachShadow({mode: 'open'});
    r.innerHTML =
      '<div class="wrap">' +
      '<button class="sbtn">shadow-btn</button>' +
      '<span id="sp">in-shadow</span>' +
      '<span class="dot">x</span><span class="dot">y</span>' +
      '</div>';
  </script>
</body></html>"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // 写到项目目录(home 下),规避 macOS 沙箱拒读 /var/folders 的 file://(见 page_extras 注释)。
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("drission-relshadow");
    tokio::fs::create_dir_all(&dir).await?;
    let page_path = dir.join("page.html");
    tokio::fs::write(&page_path, PAGE).await?;
    let url = format!("file://{}", page_path.display());

    println!("[*] 启动 Camoufox(headless)…");
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;
    let ok_get = tab.get(&url).await?;
    println!("[*] get({url}) ok={ok_get}");

    let html = tab.html().await?;
    if html.len() < 80 {
        return Err(drission::Error::msg(format!(
            "页面未正确加载(html_len={},疑似 file:// 被沙箱拒读)",
            html.len()
        )));
    }
    tab.wait().ele_displayed("#host", None).await?;

    // ---------- 相对定位:以中间的 #i2 为锚 ----------
    let i2 = tab.ele("#i2").await?;

    let parent = i2.parent().await?;
    let parent_ok = parent.tag().await? == "ul" && parent.attr("id").await?.as_deref() == Some("list");
    println!("[1] parent(): <{}> id={:?} (ok={parent_ok})", parent.tag().await?, parent.attr("id").await?);

    let gp = i2.parent_n(2).await?;
    let parent_n_ok = gp.attr("id").await?.as_deref() == Some("box");
    println!("[2] parent_n(2): id={:?} (ok={parent_n_ok})", gp.attr("id").await?);

    let until_tag = i2.parent_until("tag:div").await?;
    let until_cls = i2.parent_until(".container").await?;
    let parent_until_ok = until_tag.attr("id").await?.as_deref() == Some("box")
        && until_cls.attr("id").await?.as_deref() == Some("box");
    println!("[3] parent_until(tag:div / .container): id={:?} (ok={parent_until_ok})", until_tag.attr("id").await?);

    let next_t = i2.next().await?.text().await?;
    let prev_t = i2.prev().await?.text().await?;
    let next_prev_ok = next_t == "三" && prev_t == "一";
    println!("[4] next()={next_t:?} prev()={prev_t:?} (ok={next_prev_ok})");

    let nexts_n = i2.nexts().await?.len();
    let prevs_n = i2.prevs().await?.len();
    let siblings_n = i2.siblings().await?.len();
    let multi_ok = nexts_n == 1 && prevs_n == 1 && siblings_n == 2;
    println!("[5] nexts()={nexts_n} prevs()={prevs_n} siblings()={siblings_n} (ok={multi_ok})");

    // prevs 顺序应为文档序(此处只有 #i1)。
    let prevs = i2.prevs().await?;
    let prevs_order_ok = match prevs.first() {
        Some(e) => e.text().await? == "一",
        None => false,
    };

    // ---------- children / child:以 #list 为锚 ----------
    let list = tab.ele("#list").await?;
    let children_n = list.children().await?.len();
    let c0 = list.child(0).await?.text().await?;
    let c2 = list.child(2).await?.text().await?;
    let child_ok = children_n == 3 && c0 == "一" && c2 == "三";
    println!("[6] children()={children_n} child(0)={c0:?} child(2)={c2:?} (ok={child_ok})");

    // 越界 / 无兄弟 应报错(返回 Err 而非 panic)。
    let oob_err = list.child(9).await.is_err();
    let no_next_err = c_text_is_err(&tab).await;
    let errors_ok = oob_err && no_next_err;
    println!("[7] child(9) is_err={oob_err} #i3.next() is_err={no_next_err} (ok={errors_ok})");

    // ---------- Shadow DOM ----------
    let host = tab.ele("#host").await?;
    let root = host.shadow_root().await?;
    let sbtn_text = root.ele(".sbtn").await?.text().await?;
    root.ele(".sbtn").await?.click().await?; // 点击 shadow 内按钮(验证不报错)
    let sp_text = root.ele("#sp").await?.text().await?;
    let dots = root.eles(".dot").await?.len();
    let shadow_html = root.html().await?;
    let shadow_ok = sbtn_text == "shadow-btn"
        && sp_text == "in-shadow"
        && dots == 2
        && shadow_html.contains("shadow-btn");
    println!("[8] shadow: .sbtn={sbtn_text:?} #sp={sp_text:?} .dot 数={dots} (ok={shadow_ok})");

    // shadow 内 xpath 应被拒绝(无 document.evaluate)。
    let shadow_xpath_err = root.ele("xpath://button").await.is_err();
    println!("[9] shadow 内 xpath 被拒={shadow_xpath_err}");

    let pass = ok_get
        && parent_ok
        && parent_n_ok
        && parent_until_ok
        && next_prev_ok
        && multi_ok
        && prevs_order_ok
        && child_ok
        && errors_ok
        && shadow_ok
        && shadow_xpath_err;
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
        Err(drission::Error::msg("relative_shadow 自验证未通过"))
    }
}

/// 最后一个 li(#i3)没有下一个兄弟元素,`next()` 应返回 Err。
async fn c_text_is_err(tab: &Tab) -> bool {
    match tab.ele("#i3").await {
        Ok(i3) => i3.next().await.is_err(),
        Err(_) => false,
    }
}
