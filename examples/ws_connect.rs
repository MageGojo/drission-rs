//! **WS 接管浏览器**(`BrowserServer` + `Browser::connect`)端到端自验证。
//!
//! 对标 DrissionPage「接管已运行的浏览器」:先把浏览器跑成一个常驻 ws 服务,再用客户端通过
//! `ws://...` 接管并驱动。全程离线(用 `data:` 页),确定性强、不依赖外网。
//!
//! 覆盖:
//! 1. `BrowserServer::launch` 暴露 `ws://127.0.0.1:<port>/<token>` 端点。
//! 2. `Browser::connect` 经 ws 完整驱动:导航 / title / 元素文本 / run_js / 截图。
//! 3. **单实例**:已有客户端时再次 connect 应被拒。
//! 4. **接管语义**:首个客户端断开后,远端浏览器仍在 → 可再次 connect 驱动。
//! 5. 错误 token 应被拒。
//!
//! 运行:`cargo run --example ws_connect --no-default-features --features camoufox`
//! 末行打印 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`,关键校验失败则进程非 0 退出。

use std::time::Duration;

use drission::prelude::*;

const PNG_MAGIC: &[u8] = &[0x89, 0x50, 0x4E, 0x47];

/// 把一段 HTML 包成 `data:text/html,...`(百分号编码,保证是合法 URL)。
fn data_url_html(html: &str) -> String {
    let mut enc = String::with_capacity(html.len() * 2);
    for &b in html.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            enc.push(b as char);
        } else {
            enc.push('%');
            enc.push_str(&format!("{b:02X}"));
        }
    }
    format!("data:text/html,{enc}")
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    println!("[*] 启动 BrowserServer(headless)…");
    let server = BrowserServer::launch(BrowserOptions::new().headless(true)).await?;
    let endpoint = server.ws_endpoint().to_string();
    println!("[*] ws 端点: {endpoint}");
    let endpoint_ok = endpoint.starts_with("ws://127.0.0.1:")
        && endpoint.rsplit('/').next().is_some_and(|t| !t.is_empty());

    let page = "<!doctype html><meta charset=utf-8><title>WSOK</title><h1 id=x>hello-ws</h1>";
    let url = data_url_html(page);

    // ---- 客户端 1:接管并驱动(放进作用域,块末彻底析构以释放 ws) ----
    println!("[*] 客户端1 connect…");
    let (title_ok, ele_ok, js_ok, shot_ok, concurrent_rejected) = {
        let b1 = Browser::connect(&endpoint).await?;
        let t1 = b1.latest_tab().await?;
        t1.get(&url).await?;
        // 导航后等元素就绪(同 screencast 例:首读会落到旧执行上下文,先等就绪再读)。
        t1.wait()
            .ele_displayed("#x", Some(Duration::from_secs(5)))
            .await?;
        let title = t1.title().await?;
        let txt = t1.ele("#x").await?.text().await?;
        let js = t1.run_js("6*7").await?;
        let shot = t1.screenshot_bytes(false).await?;

        // 单实例:b1 仍在线时再次 connect 应失败。
        let concurrent_rejected = Browser::connect(&endpoint).await.is_err();

        println!(
            "[客户端1] title={title:?} ele={txt:?} js={js} png={}B 并发再连被拒={concurrent_rejected}",
            shot.len()
        );
        (
            title == "WSOK",
            txt.trim() == "hello-ws",
            js.as_i64() == Some(42),
            shot.starts_with(PNG_MAGIC),
            concurrent_rejected,
        )
    };

    // 让服务端观察到客户端1的 ws 关闭并释放单客户端槽位(实测 teardown ~100ms,留足余量)。
    tokio::time::sleep(Duration::from_millis(800)).await;

    // ---- 客户端 2:再次接管,证明远端浏览器没被关 ----
    println!("[*] 客户端2 connect(验证接管语义)…");
    let reconnect_ok = {
        let b2 = Browser::connect(&endpoint).await?;
        let t2 = b2.latest_tab().await?;
        t2.get(&url).await?;
        t2.wait()
            .ele_displayed("#x", Some(Duration::from_secs(5)))
            .await?;
        let title2 = t2.title().await?;
        println!("[客户端2] title={title2:?}");
        title2 == "WSOK"
    };

    // ---- 错误 token 应被拒(握手阶段失败) ----
    let bad = format!("ws://127.0.0.1:{}/definitely-wrong-token", server.port());
    let bad_rejected = Browser::connect(&bad).await.is_err();
    println!("[错误token] 拒绝={bad_rejected}");

    // ---- 汇总 ----
    let checks = [
        ("端点格式 ws://127.0.0.1:port/token", endpoint_ok),
        ("ws 驱动:title=WSOK", title_ok),
        ("ws 驱动:元素文本=hello-ws", ele_ok),
        ("ws 驱动:run_js 6*7=42", js_ok),
        ("ws 驱动:截图为 PNG", shot_ok),
        ("单实例:并发再连被拒", concurrent_rejected),
        ("接管语义:断开后可再次接管", reconnect_ok),
        ("错误 token 被拒", bad_rejected),
    ];
    let pass = checks.iter().all(|&(_, ok)| ok);
    for (name, ok) in checks {
        println!("    [{}] {name}", if ok { "PASS" } else { "FAIL" });
    }
    println!(
        "\n==== {} ====",
        if pass {
            "ALL CHECKS PASSED"
        } else {
            "SOME CHECKS FAILED"
        }
    );

    server.stop().await?;
    pass.then_some(())
        .ok_or_else(|| drission::Error::msg("ws_connect 自验证未通过"))
}
