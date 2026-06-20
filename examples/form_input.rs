//! 输入与点击(**大道至简** · `Page` 门面 + 捷径):注入一个输入框 + 按钮,
//! 用 `page.input(sel, text)` / `page.click(sel)` 一步「找+做」,读回结果。
//!
//! 对比底层写法:`tab.ele(sel).await?.input(text).await?`(两次 `.await?`)
//! 捷径写法:    `page.input(sel, text).await?`(一次)。
//!
//! 运行:`cargo run --example form_input --no-default-features --features camoufox`

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let page = Page::headless().await?;
    page.get("https://example.com").await?;

    // 注入一个简单表单:点击按钮把输入框的值写到 window.__result。
    page.run_js(
        "document.body.innerHTML = '<input id=\"kw\" style=\"width:300px;height:40px\">' + \
         '<button id=\"go\" style=\"width:120px;height:40px\">go</button>'; \
         document.getElementById('go').onclick = () => { \
            window.__result = document.getElementById('kw').value; }; true",
    )
    .await?;

    println!("[exists] #kw 存在? {}", page.exists("#kw").await?);

    println!("[输入] page.input(\"#kw\", \"hello drission\")");
    page.input("#kw", "hello drission").await?;

    let typed = page.ele("#kw").await?.value().await?;
    println!("  输入框当前 value = {typed:?}");

    println!("[点击] page.click(\"#go\")");
    page.click("#go").await?;

    let result = page.run_js("window.__result").await?;
    println!("  按钮回调读到的值 = {result}");

    let ok = result.as_str() == Some("hello drission");
    println!(
        "\n{}",
        if ok {
            "✅ 输入+点击 验证通过"
        } else {
            "❌ 验证失败"
        }
    );

    page.quit().await?;
    if ok { Ok(()) } else { std::process::exit(1) }
}
