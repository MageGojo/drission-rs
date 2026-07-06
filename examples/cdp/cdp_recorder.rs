//! 录制生成代码(codegen)+ 无障碍快照(accessibility)端到端自验证(完全离线:进程内起极简 HTTP 服务)。
//!
//! 覆盖:① `tab.recorder()` 起录 → 程序化驱动表单(输入/勾选/下拉/点击)+ **悬停** + **拖拽** +
//! **iframe 内输入** + **打开新标签** → `stop()` 得 [`RecordedScript`],生成可运行 Rust(DP 风格选择器,
//! 含 `get_frame(..)` 框限定与 `wait().new_tab()` 多标签)。② `tab.ax_snapshot()`(DOM 派生,跨后端)与
//! `tab.ax_tree()`(CDP 原生)拿语义树,按角色/名断言关键节点。
//!
//! 运行:`cargo run --example cdp_recorder`(无头默认;`HL=0` 开窗口;`CHROME_BIN` 指定浏览器)。
//! 任一关键校验失败即非 0 退出;全部通过打印 `ALL CHECKS PASSED`。

use std::time::Duration;

use drission::codegen::RecordedAction;
use drission::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::{Instant, sleep};

const PAGE_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Recorder Demo</title></head>
<body>
<h1 id="title">登录</h1>
<a id="menu" style="cursor:pointer">菜单</a>
<form id="f" onsubmit="return false">
  <label>关键词 <input name="q" type="text" placeholder="搜索词"></label>
  <label><input id="agree" type="checkbox"> 同意条款</label>
  <select id="lang">
    <option value="">请选择</option>
    <option value="rs">Rust</option>
    <option value="py">Python</option>
  </select>
  <button id="go" type="button">提交</button>
</form>
<iframe id="ifr" srcdoc="<!doctype html><meta charset=utf-8><input id=inframe placeholder=框内>"></iframe>
<div id="src" draggable="true" style="cursor:grab">拖我</div>
<div id="dst" ondragover="event.preventDefault()">放这</div>
<a id="open" href="/popup" target="_blank">开新标签</a>
<div id="out">idle</div>
</body></html>"#;

const POPUP_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Popup</title></head>
<body><h1 id="ptitle">弹窗页</h1><button id="pbtn" type="button">弹窗按钮</button></body></html>"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(serve(listener));
    let base = format!("http://127.0.0.1:{port}/");
    println!("[*] 本地服务: {base}");

    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    println!("[*] 启动 Chrome(headless={headless})");
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;

    let mut failed = false;
    macro_rules! check {
        ($cond:expr, $($arg:tt)*) => {{
            let ok = $cond;
            println!("[{}] {}", if ok { "ok" } else { "FAIL" }, format!($($arg)*));
            if !ok { failed = true; }
        }};
    }

    // ── ① 录制:导航前起录,再驱动各类交互 ───────────────────────────────
    let rec = tab.recorder();
    rec.start().await?;
    check!(rec.is_recording(), "recorder.is_recording == true");

    tab.get(&base).await?; // 主框架导航 → Navigate

    // 文本输入 + 触发 change → Fill
    tab.input("@name:q", "rust").await?;
    tab.run_js(
        "(()=>{const e=document.querySelector('[name=q]');\
         e.dispatchEvent(new Event('input',{bubbles:true}));\
         e.dispatchEvent(new Event('change',{bubbles:true}));})()",
    )
    .await?;

    // 勾选复选框:可信点击 → Check
    tab.ele("#agree").await?.click().await?;

    // 下拉选择:设值并派发 change → Select
    tab.run_js(
        "(()=>{const s=document.querySelector('#lang');s.value='rs';\
         s.dispatchEvent(new Event('change',{bubbles:true}));})()",
    )
    .await?;

    // 悬停菜单(防抖 250ms)→ Hover
    tab.ele("#menu").await?.hover().await?;
    sleep(Duration::from_millis(450)).await;

    // iframe 内输入(同源 srcdoc,recorder 经 frameElement 算出 frame="#ifr")→ Fill(frame)
    let frame = tab.get_frame("#ifr").await?;
    frame
        .run_js(
            "(()=>{const e=document.querySelector('#inframe');e.value='框内';\
             e.dispatchEvent(new Event('input',{bubbles:true}));\
             e.dispatchEvent(new Event('change',{bubbles:true}));})()",
        )
        .await?;

    // 拖拽(HTML5 DnD:dragstart#src → drop#dst)→ Drag
    tab.run_js(
        "(()=>{const s=document.querySelector('#src'),d=document.querySelector('#dst');\
         const dt=new DataTransfer();\
         s.dispatchEvent(new DragEvent('dragstart',{bubbles:true,dataTransfer:dt}));\
         d.dispatchEvent(new DragEvent('drop',{bubbles:true,dataTransfer:dt}));})()",
    )
    .await?;

    // 可信点击按钮 → Click
    tab.ele("#go").await?.click().await?;

    // 打开新标签(target=_blank)→ Click(#open) + NewTab(最后一步,便于多标签代码生成)
    tab.ele("#open").await?.click().await?;

    // 等录到 NewTab(弹窗 targetCreated → 附着 → 记 NewTab)
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        if rec
            .script()
            .await
            .actions
            .iter()
            .any(|a| matches!(a, RecordedAction::NewTab))
        {
            break;
        }
        sleep(Duration::from_millis(80)).await;
    }

    let script = rec.stop().await?;
    check!(!rec.is_recording(), "stop 后 is_recording == false");
    println!("[*] 录到 {} 个动作", script.len());

    let acts = &script.actions;
    check!(
        matches!(acts.first(), Some(RecordedAction::Navigate { url }) if url.starts_with("http://127.0.0.1")),
        "首动作是 Navigate"
    );
    check!(
        acts.iter().any(|a| matches!(a, RecordedAction::Fill { selector, text, frame } if selector == "@name:q" && text == "rust" && frame.is_none())),
        "录到 Fill(@name:q = rust)"
    );
    check!(
        acts.iter().any(|a| matches!(a, RecordedAction::Check { selector, checked, .. } if selector == "#agree" && *checked)),
        "录到 Check(#agree = true)"
    );
    check!(
        acts.iter().any(|a| matches!(a, RecordedAction::Select { selector, value, .. } if selector == "#lang" && value == "rs")),
        "录到 Select(#lang = rs)"
    );
    check!(
        acts.iter()
            .any(|a| matches!(a, RecordedAction::Hover { selector, .. } if selector == "#menu")),
        "录到 Hover(#menu)"
    );
    check!(
        acts.iter().any(|a| matches!(a, RecordedAction::Fill { selector, frame, .. } if selector == "#inframe" && frame.as_deref() == Some("#ifr"))),
        "录到 iframe 内 Fill(#inframe, frame=#ifr)"
    );
    check!(
        acts.iter().any(
            |a| matches!(a, RecordedAction::Drag { from, to, .. } if from == "#src" && to == "#dst")
        ),
        "录到 Drag(#src → #dst)"
    );
    check!(
        acts.iter()
            .any(|a| matches!(a, RecordedAction::Click { selector, .. } if selector == "#go")),
        "录到 Click(#go)"
    );
    check!(
        acts.iter().any(|a| matches!(a, RecordedAction::NewTab)),
        "录到 NewTab(弹窗打开)"
    );

    // ── 生成可运行 Rust,核对关键行(含 frame 限定 + 多标签 + 拖拽链)─────────
    let code = script.to_rust();
    println!("\n──────── 生成的 Rust 代码 ────────\n{code}────────────────────────────────\n");
    check!(
        code.contains("use drission::prelude::*;"),
        "代码含 prelude 引入"
    );
    check!(
        code.contains("tab.input(\"@name:q\", \"rust\").await?;"),
        "代码含 input 行"
    );
    check!(
        code.contains(
            "tab.get_frame(\"#ifr\").await?.ele(\"#inframe\").await?.input(\"框内\").await?;"
        ),
        "代码含 iframe 限定 input 行"
    );
    check!(
        code.contains("move_to_ele(&_from, 0.2).hold()"),
        "代码含拖拽动作链"
    );
    check!(
        code.contains(".wait().new_tab(None).await?"),
        "代码含新标签获取"
    );

    // ── ② 无障碍快照:DOM 派生(跨后端)+ CDP 原生 ───────────────────────
    let snap = tab.ax_snapshot().await?;
    println!(
        "──────── ax_snapshot 大纲(DOM 派生)────────\n{}",
        snap.to_outline()
    );
    check!(snap.count() > 3, "快照节点数 > 3(实得 {})", snap.count());
    check!(
        snap.find_by_name("提交").iter().any(|n| n.role == "button"),
        "快照里有 button \"提交\""
    );
    check!(
        !snap.find_by_role("checkbox").is_empty(),
        "快照里有 checkbox"
    );

    let tree = tab.ax_tree().await?;
    println!(
        "──────── ax_tree 大纲(CDP 原生)────────\n{}",
        tree.to_outline()
    );
    let buttons = tab.ax_find("button").await?;
    check!(
        !buttons.is_empty(),
        "原生树有 button(实得 {})",
        buttons.len()
    );
    check!(
        tree.find_by_role("heading")
            .iter()
            .any(|h| h.name.contains("登录")),
        "原生树有 heading \"登录\""
    );

    browser.quit().await?;

    if failed {
        eprintln!("==== 有校验未通过 ====");
        std::process::exit(1);
    }
    println!("ALL CHECKS PASSED");
    Ok(())
}

/// 极简 HTTP/1.1 服务:`/popup` 回弹窗页,其余回表单页。每连接应答后关闭。
async fn serve(listener: TcpListener) {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            break;
        };
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let path = req
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/");
            let body = if path.starts_with("/popup") {
                POPUP_HTML
            } else {
                PAGE_HTML
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
    }
}
