//! ①【主动逆向 · XHR 断点 + 调用栈】定位「某请求在哪一行发起 / 签名在哪生成」。
//!
//! 运行:`cargo run --example re_xhr_break`(无头默认;`HL=0` 有头)。
//! 思路:在「URL 含 /get 的 XHR/fetch 发起处」断下 → 打印调用栈(函数名/脚本/行列)→ 在断点
//! 上下文里求值读私有变量 → 放行。触发用 **fire-and-forget**(`setTimeout` 调度),避免卡死。

use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;

    // 用 set_content 造一个会发 fetch 的页面(自带签名逻辑,便于演示读私有变量)。
    tab.get("https://example.com").await?;
    tab.set_content(
        r#"<html><body><h1>re</h1><script>
        function buildSign(t){ var key='SECRET_KEY'; var sig=key+'-'+t; return sig; }
        window.go=function(){ var t=Date.now(); var sign=buildSign(t);
            fetch('https://example.com/?sig='+encodeURIComponent(sign)); };
        </script></body></html>"#,
    )
    .await?;

    let dbg = tab.debugger();
    dbg.break_on_xhr("example.com/?sig=").await?;
    println!("[*] 已设 XHR 断点,触发请求…");

    // fire-and-forget:setTimeout 调度后立即返回(run_js 不会卡在断点上)。
    tab.run_js("setTimeout(window.go,0); 1").await?;

    match dbg.wait_paused(Some(Duration::from_secs(10))).await? {
        Some(stack) => {
            println!("[*] 已断下,reason={}", stack.reason());
            println!("---- 调用栈 ----\n{}", stack.backtrace());
            // 往上找到 buildSign 帧,读它的私有变量 key / 入参 t。
            for (i, f) in stack.frames().iter().enumerate() {
                if f.function_name == "buildSign" {
                    println!("[*] buildSign 帧 #{i}:");
                    println!("    key  = {}", stack.eval(i, "key").await?);
                    println!("    t    = {}", stack.eval(i, "t").await?);
                    println!("    sig  = {}", stack.eval(i, "key + '-' + t").await?);
                    println!("    局部变量: {:?}", stack.locals(i).await?);
                }
            }
            stack.resume().await?;
            println!("[*] 已放行。");
        }
        None => println!("[!] 超时未断下(检查触发/断点 URL 子串)。"),
    }

    browser.quit().await?;
    println!("==== ① XHR 断点 demo 完成 ====");
    Ok(())
}
