//! ②【主动逆向 · 脚本源码 dump + 全文搜索 + 美化】把站点 JS 全 dump 落盘、grep 签名字样。
//!
//! 运行:`cargo run --example re_scripts`(无头默认;`HL=0` 有头;`URL=...` 指定目标站)。
//! 产出:`out/scripts/` 下美化后的 .js;并打印 grep 命中(脚本 + 行 + 片段)。

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let url = std::env::var("URL").unwrap_or_else(|_| "https://example.com".to_string());
    let needle = std::env::var("NEEDLE").unwrap_or_else(|_| "function".to_string());

    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;
    tab.get(&url).await?;

    let sc = tab.scripts();

    // 列出全部脚本。
    let list = sc.list().await?;
    println!("[*] 解析到 {} 个脚本:", list.len());
    for s in list.iter().take(20) {
        let kind = if s.is_wasm { "wasm" } else { "js" };
        println!("    [{kind}] id={} len={} {}", s.script_id, s.length, s.url);
    }

    // 全文搜索(CDP 原生 searchInContent;片段对压缩代码也可读)。
    let hits = sc.grep(&needle).await?;
    println!("\n[*] grep \"{needle}\" 命中 {} 处:", hits.len());
    for m in hits.iter().take(15) {
        println!("    {}:{}  {}", m.url, m.line_number, m.snippet);
    }

    // dump 全部 JS(自动美化)。
    let files = sc.dump_all("out/scripts").await?;
    println!("\n[*] dump 了 {} 个 JS 到 out/scripts/", files.len());

    browser.quit().await?;
    println!("==== ② 脚本 dump/搜索 demo 完成 ====");
    Ok(())
}
