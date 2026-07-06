//! YouTube base.js 签名/n 参数(逆向实战,用 ②+①):
//!   步骤1(本版):② dump base.js 美化源码到 /tmp/yt_base.js,并 grep 定位 sig/n 解扰函数的确切行。
//!   (步骤2 用 ① 在这些行下断点、触发播放、读入参/出参与函数体——见后续。)
//!
//! 运行:`HL=0 cargo run --example re_yt_sig`(默认无头;`URL=` 换视频)。

use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let url = std::env::var("URL")
        .unwrap_or_else(|_| "https://www.youtube.com/watch?v=7349tcyyE-c".to_string());

    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;
    tab.get(&url).await?;
    println!("[*] title={:?}", tab.title().await.unwrap_or_default());
    tokio::time::sleep(Duration::from_secs(6)).await; // 等 base.js 加载

    let sc = tab.scripts();
    let scripts = sc.list().await.unwrap_or_default();
    println!("[*] 解析到 {} 个脚本", scripts.len());
    let Some(base) = scripts.iter().find(|s| s.url.contains("/base.js")) else {
        println!("[!] 未找到 base.js");
        browser.quit().await?;
        return Ok(());
    };
    println!(
        "[*] base.js: {} (scriptId {}, {} 字符)",
        base.url, base.script_id, base.length
    );

    // ② dump 美化源码落盘(供精读)。
    if let Ok(src) = sc.source(&base.script_id).await {
        let pretty = beautify_js(&src);
        let _ = std::fs::write("/tmp/yt_base.js", &pretty);
        let _ = std::fs::write("/tmp/yt_base_raw.js", &src);
        println!(
            "[*] 已存 /tmp/yt_base.js(美化 {} 字符)+ /tmp/yt_base_raw.js(原始 {} 字符)",
            pretty.len(),
            src.len()
        );
    }

    // ② grep 定位关键点(CDP searchInContent,在 base.js 上)。
    println!("\n========== grep 定位(base.js)==========");
    for kw in [
        "a.split(\"\")", // sig 解扰典型:a=a.split("")...return a.join("")
        "a.join(\"\")",
        "signatureCipher",
        "&n=",
        "get_video_info",
        "enhanced_except", // n 函数失败时常见标记
        ".reverse()",
        ".splice(",
    ] {
        let hits = sc.grep_with(kw, true, false).await.unwrap_or_default();
        let only_base: Vec<_> = hits.iter().filter(|m| m.url.contains("/base.js")).collect();
        println!("  {kw:?} → base.js 命中 {} 处", only_base.len());
        for m in only_base.iter().take(3) {
            let frag: String = m.snippet.chars().take(90).collect();
            println!("     :{}  {frag}", m.line_number);
        }
    }

    browser.quit().await?;
    println!("\n==== 步骤1 完成:base.js 已落盘,关键点已定位 ====");
    Ok(())
}
