//! CDP 键盘能力端到端真机自验证(完全离线,about:blank + run_js 建页)。
//!
//! 覆盖:① 逐字符**拟人输入** `input_human` ② **修饰组合键**(`ele.shortcut` / `tab.key_combo`)
//! —— CDP 原生 `modifiers` 位掩码,页面读得到 `e.ctrlKey`/`metaKey` 为 `true`,且真正触发
//! 浏览器编辑命令(全选)。注:全选修饰键 macOS 是 Cmd、其余平台是 Ctrl,示例按平台选。
//!
//! 运行:`cargo run --example cdp_keyboard`(无头默认;`HL=0` 开窗口)。
//! 任一关键校验失败即非 0 退出;全部通过打印 `ALL CHECKS PASSED`。

use drission::prelude::*;

const BUILD_PAGE: &str = r#"
  document.body.innerHTML = '<input id="t" value="" style="font-size:20px;width:300px">';
  window.__keys = [];
  const t = document.getElementById('t');
  t.addEventListener('keydown', e => {
    window.__keys.push({ key: e.key, ctrl: e.ctrlKey, meta: e.metaKey, shift: e.shiftKey, alt: e.altKey });
  });
  'ok'
"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    println!("[*] 启动 Chrome(headless={headless})");
    let browser = Browser::launch(BrowserOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;
    tab.get("about:blank").await?;
    tab.run_js(BUILD_PAGE).await?;

    let mut failed = false;
    macro_rules! check {
        ($cond:expr, $($arg:tt)*) => {{
            let ok = $cond;
            println!("[{}] {}", if ok { "ok" } else { "FAIL" }, format!($($arg)*));
            if !ok { failed = true; }
        }};
    }

    // ── ① 逐字符拟人输入 ───────────────────────────────────────────────────
    let inp = tab.ele("#t").await?;
    inp.click().await?; // 可信点击聚焦
    inp.input_human("hello world").await?;
    let val = inp.value().await?;
    check!(
        val == "hello world",
        "input_human → value == \"hello world\"(实得 {val:?})"
    );
    let typed_keys = tab
        .run_js("window.__keys.length")
        .await?
        .as_u64()
        .unwrap_or(0);
    check!(
        typed_keys >= 11,
        "逐字符 keydown 计数 >= 11(实得 {typed_keys})"
    );

    // ── ② 修饰组合键:全选(macOS=Cmd+A,其余=Ctrl+A)──────────────────────
    let on_mac = cfg!(target_os = "macos");
    let sel_mod = if on_mac { Keys::META } else { Keys::CONTROL };
    let mod_name = if on_mac { "meta" } else { "ctrl" };
    inp.shortcut(&[sel_mod, "a"]).await?;

    // (a) 页面 keydown 收到的最后一个 key=="a" 事件,对应修饰键标志为 true(证明 modifiers 真下发)。
    let mod_seen = tab
        .run_js(&format!(
            "(()=>{{const k=window.__keys.filter(x=>x.key==='a'); return k.length>0 && k[k.length-1].{mod_name}===true;}})()"
        ))
        .await?
        .as_bool()
        .unwrap_or(false);
    check!(
        mod_seen,
        "组合键 keydown 的 e.{mod_name}Key === true(modifiers 已下发)"
    );

    // (b) 组合键**不插入字符**:value 仍是 "hello world"(没有键入 "a")。
    let val_after = inp.value().await?;
    check!(
        val_after == "hello world",
        "组合键不键入字符(value 仍为 \"hello world\",实得 {val_after:?})"
    );

    // (c) 真正触发"全选":selection 覆盖全部文本。
    let all_selected = tab
        .run_js("(()=>{const t=document.getElementById('t');return t.selectionStart===0 && t.selectionEnd===t.value.length && t.value.length>0;})()")
        .await?
        .as_bool()
        .unwrap_or(false);
    check!(all_selected, "全选生效:selection 覆盖整个输入框");

    // (d) 全选后 Backspace 应清空(进一步坐实 selection 真实存在)。
    tab.press_key(Keys::BACKSPACE).await?;
    let val_cleared = inp.value().await?;
    check!(
        val_cleared.is_empty(),
        "全选 + Backspace 清空(实得 {val_cleared:?})"
    );

    // ── ③ tab.key_combo 也可用(对焦点生效):再次输入后用 tab 级组合键全选清空 ──
    inp.input("abc").await?;
    tab.key_combo(&[sel_mod, "a"]).await?;
    tab.press_key(Keys::BACKSPACE).await?;
    let val2 = inp.value().await?;
    check!(
        val2.is_empty(),
        "tab.key_combo 全选+删除清空(实得 {val2:?})"
    );

    browser.quit().await?;
    if failed {
        eprintln!("==== SOME CHECKS FAILED ====");
        std::process::exit(1);
    }
    println!("\n==== ALL CHECKS PASSED ====");
    Ok(())
}
