//! 元素几何/状态/属性 + 元素级 wait + 键盘(组合键/序列输入)端到端自验证(完全离线)。
//!
//! 运行:`cargo run --example ele_extras`
//! 末尾打印 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`,关键校验失败则进程非 0 退出。

use std::time::Duration;

use drission::prelude::*;

const PAGE: &str = r##"<!doctype html><html><head><meta charset="utf-8"></head><body>
  <input id="inp" value="">
  <button id="btn" style="position:fixed;left:200px;top:200px">Btn</button>
  <a id="under" href="#" style="position:fixed;left:8px;top:8px">U</a>
  <div id="cover" style="position:fixed;left:0;top:0;width:60px;height:60px;background:rgb(255,0,0)"></div>
  <input id="dis" disabled>
  <div id="lazy" style="display:none">lazy</div>
  <form id="f" onsubmit="window.__submitted=true;return false;"><input id="finp"></form>
  <div id="rm">remove-me</div>
  <div id="far" style="margin-top:3000px">far</div>
  <script>setTimeout(()=>{document.getElementById('lazy').style.display='block'},500)</script>
</body></html>"##;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("drission-eleextras");
    tokio::fs::create_dir_all(&dir).await?;
    let page_path = dir.join("page.html");
    tokio::fs::write(&page_path, PAGE).await?;
    let url = format!("file://{}", page_path.display());

    println!("[*] 启动 Camoufox(headless)…");
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;
    tab.get(&url).await?;
    tab.wait().ele_displayed("#inp", None).await?;

    // 几何
    let r = tab.ele("#inp").await?.rect().await?;
    let (lw, lh) = tab.ele("#inp").await?.size().await?;
    let geo_ok = r.width > 0.0 && r.height > 0.0 && lw == r.width && lh == r.height;
    println!("[1] rect={r:?} size=({lw},{lh}) (ok={geo_ok})");

    // 状态
    let inp_enabled = tab.ele("#inp").await?.is_enabled().await?;
    let dis_enabled = tab.ele("#dis").await?.is_enabled().await?;
    let btn_vp = tab.ele("#btn").await?.is_in_viewport().await?;
    let far_vp = tab.ele("#far").await?.is_in_viewport().await?;
    let under_cov = tab.ele("#under").await?.is_covered().await?;
    let btn_cov = tab.ele("#btn").await?.is_covered().await?;
    let btn_click = tab.ele("#btn").await?.is_clickable().await?;
    let under_click = tab.ele("#under").await?.is_clickable().await?;
    let dis_click = tab.ele("#dis").await?.is_clickable().await?;
    let states_ok = inp_enabled
        && !dis_enabled
        && btn_vp
        && !far_vp
        && under_cov
        && !btn_cov
        && btn_click
        && !under_click
        && !dis_click;
    println!(
        "[2] enabled(inp={inp_enabled},dis={dis_enabled}) viewport(btn={btn_vp},far={far_vp}) \
         covered(under={under_cov},btn={btn_cov}) clickable(btn={btn_click},under={under_click},dis={dis_click}) (ok={states_ok})"
    );

    // 属性 / 样式 / property
    let attrs = tab.ele("#inp").await?.attrs().await?;
    let bg = tab.ele("#cover").await?.style("background-color").await?;
    let dis_prop = tab.ele("#dis").await?.property("disabled").await?;
    let attr_ok = attrs.get("id").map(|s| s == "inp").unwrap_or(false)
        && bg.replace(' ', "") == "rgb(255,0,0)"
        && dis_prop.as_bool() == Some(true);
    println!(
        "[3] attrs.id={:?} bg={bg:?} dis.disabled={dis_prop} (ok={attr_ok})",
        attrs.get("id")
    );

    // remove + ele.wait().deleted
    let rm = tab.ele("#rm").await?;
    rm.remove().await?;
    let deleted = rm.wait().deleted(Some(Duration::from_secs(2))).await?;
    let gone = tab.ele("#rm").await.is_err();
    let remove_ok = deleted && gone;
    println!("[4] remove → wait.deleted={deleted} 再查不到={gone} (ok={remove_ok})");

    // ele.wait().displayed(惰性显示)
    let lazy_shown = tab
        .ele("#lazy")
        .await?
        .wait()
        .displayed(Some(Duration::from_secs(2)))
        .await?;
    println!("[5] #lazy wait.displayed={lazy_shown}");

    // input_keys: 文本 + Enter 触发表单提交
    tab.ele("#finp")
        .await?
        .input_keys(&[KeyInput::text("abc"), KeyInput::key(Keys::ENTER)])
        .await?;
    let finp_val = tab.run_js("document.getElementById('finp').value").await?;
    let submitted = tab.run_js("window.__submitted===true").await?;
    let inputkeys_ok = finp_val.as_str() == Some("abc") && submitted.as_bool() == Some(true);
    println!("[6] input_keys: finp={finp_val} submitted={submitted} (ok={inputkeys_ok})");

    // 单个特殊键(press_key + Keys 常量):在 #inp 末尾按一次 Backspace 删一个字符。
    tab.ele("#inp").await?.input("hello").await?;
    tab.ele("#inp").await?.focus().await?;
    tab.press_key(Keys::BACKSPACE).await?;
    let after_bs = tab.run_js("document.getElementById('inp').value").await?;
    let key_ok = after_bs.as_str() == Some("hell");
    println!("[7] press_key(BACKSPACE): inp={after_bs}(应为 \"hell\",ok={key_ok})");

    let pass = geo_ok && states_ok && attr_ok && remove_ok && lazy_shown && inputkeys_ok && key_ok;
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
        Err(drission::Error::msg("ele_extras 自验证未通过"))
    }
}
