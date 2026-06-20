//! 验证输入与点击:注入一个输入框 + 按钮,输入文本后点击按钮,读回结果。
//!
//! 运行:`cargo run --example form_input`

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;
    tab.get("https://example.com").await?;

    // 注入一个简单表单:点击按钮把输入框的值写到 window.__result。
    tab.run_js(
        "document.body.innerHTML = '<input id=\"kw\" style=\"width:300px;height:40px\">' + \
         '<button id=\"go\" style=\"width:120px;height:40px\">go</button>'; \
         document.getElementById('go').onclick = () => { \
            window.__result = document.getElementById('kw').value; }; true",
    )
    .await?;

    println!("[输入] 在 #kw 里输入 'hello drission'");
    tab.ele("#kw").await?.input("hello drission").await?;

    let typed = tab.ele("#kw").await?.value().await?;
    println!("  输入框当前 value = {typed:?}");

    println!("[点击] 点击 #go 按钮");
    tab.ele("#go").await?.click().await?;

    let result = tab.run_js("window.__result").await?;
    println!("  按钮回调读到的值 = {result}");

    let ok = result.as_str() == Some("hello drission");
    println!("\n{}", if ok { "✅ 输入+点击 验证通过" } else { "❌ 验证失败" });

    browser.quit().await?;
    Ok(())
}
