//! 端到端填表测试:用本库自动填完 `examples/1.html` 的 4 步问卷并校验结果。
//!
//! 覆盖:文本框 `input`、下拉 `select_value`、单选/多选 `click`(真实鼠标)+ `is_checked` 校验、
//! 文本域 `input`、多步"下一步/提交"按钮点击、读取并校验最终生成的 JSON。
//!
//! 运行:`cargo run --example form_fill --no-default-features --features camoufox`(默认 headless;`HL=0 cargo run --example form_fill --no-default-features --features camoufox` 看界面)

use std::time::Duration;

use drission::prelude::*;

/// 本地表单页(编译期拿到 crate 目录,拼成 file:// 绝对路径)。
const PAGE: &str = concat!("file://", env!("CARGO_MANIFEST_DIR"), "/examples/1.html");

const USERNAME: &str = "自动化测试机器人";
const JOB: &str = "dev"; // 研发人员
const TOOL: &str = "Playwright";
const PAINPOINTS: &[&str] = &["dynamic_id", "verification_code"];
const FEEDBACK: &str = "drission-rs 填表链路验证:输入/下拉/单选/多选/文本域/分步提交。";

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let browser = Browser::launch(BrowserOptions::new().headless(headless)).await?;
    let tab = browser.latest_tab().await?;

    println!("打开本地表单: {PAGE}");
    tab.get(PAGE).await?;
    // 屏蔽 alert/confirm,避免提交时模态框阻塞页面 JS。
    let _ = tab
        .run_js("window.alert=function(){};window.confirm=function(){return true};window.prompt=function(){return ''};true")
        .await;

    // ---- 第 1 步:文本 + 下拉 ----
    println!("\n[第1步] 填姓名 + 选职业");
    tab.ele("#username").await?.input(USERNAME).await?;
    tab.ele("#job").await?.select_value(JOB).await?;
    let got_name = tab.ele("#username").await?.value().await?;
    let got_job = tab.ele("#job").await?.value().await?;
    println!("  姓名={got_name:?}  职业={got_job:?}");
    next(&tab).await?;

    // ---- 第 2 步:单选 ----
    println!("[第2步] 选工具(单选 {TOOL})");
    let radio = format!("css:input[name=\"tool\"][value=\"{TOOL}\"]");
    ensure_checked(&tab, &radio).await?;
    next(&tab).await?;

    // ---- 第 3 步:多选 ----
    println!("[第3步] 勾选难点(多选 {PAINPOINTS:?})");
    for v in PAINPOINTS {
        let cb = format!("css:input[name=\"painpoint\"][value=\"{v}\"]");
        ensure_checked(&tab, &cb).await?;
    }
    next(&tab).await?;

    // ---- 第 4 步:文本域 + 提交 ----
    println!("[第4步] 写反馈并提交");
    let fb = tab.ele("#feedback").await?;
    fb.clear().await?; // 页面 HTML 有笔误,textarea 自带残留 </style>,先清空
    fb.input(FEEDBACK).await?;
    next(&tab).await?; // 第 4 步的按钮即"提交表单"
    tokio::time::sleep(Duration::from_millis(500)).await;

    // ---- 读取并校验最终结果 ----
    let result = tab.ele("#result-display").await?.text().await?;
    println!("\n页面生成的提交结果:\n{result}");

    let mut checks: Vec<(&str, bool)> = vec![
        ("姓名", result.contains(USERNAME)),
        ("职业=研发", result.contains("研发") || result.contains(JOB)),
        ("单选=Playwright", result.contains(TOOL)),
        ("反馈备注", result.contains("drission-rs 填表链路验证")),
    ];
    for v in PAINPOINTS {
        checks.push(("多选含", result.contains(v)));
    }

    let ok = checks.iter().all(|(_, b)| *b);
    println!("\n校验:");
    for (name, b) in &checks {
        println!("  {} {name}", if *b { "✅" } else { "❌" });
    }
    println!(
        "\n{}",
        if ok {
            "✅ 填表端到端验证通过"
        } else {
            "❌ 有字段未通过"
        }
    );

    browser.quit().await?;
    if ok {
        Ok(())
    } else {
        Err(Error::Other("填表校验失败".into()))
    }
}

/// 点击"下一步/提交"按钮并稍候,等步骤切换。
async fn next(tab: &Tab) -> Result<()> {
    tab.ele("#next-btn").await?.click().await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    Ok(())
}

/// 选中一个 checkbox/radio:优先真实点击;若未选中再用 set_checked 兜底。打印用了哪条路径。
async fn ensure_checked(tab: &Tab, selector: &str) -> Result<()> {
    let el = tab.ele(selector).await?;
    el.click().await.ok();
    tokio::time::sleep(Duration::from_millis(150)).await;
    if el.is_checked().await.unwrap_or(false) {
        println!("    {selector}  → 点击选中 ✓");
        return Ok(());
    }
    el.set_checked(true).await?;
    println!("    {selector}  → set_checked 兜底 ✓");
    Ok(())
}
