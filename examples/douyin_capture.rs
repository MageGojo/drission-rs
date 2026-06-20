//! 抓【指定视频】的 detail 请求(含它自己配套的 a_bogus),并导出一套脱浏览器可复现的包到
//! `dump-env/replay.json`(url + cookie + user-agent 自洽)。配合 `scripts/douyin_detail_test.py --replay` 验证。
//!
//! 为什么要这样:a_bogus 是每个视频【现算】的签名(绑定 query+UA+cookie),不能拿别的视频的旧值套;
//! 所以先用浏览器把目标视频的签名请求和同会话 cookie/UA 一起抓出来,才能脱浏览器复现。
//!
//! 运行:`cargo run --example douyin_capture -- "https://www.douyin.com/video/<aweme_id>"`

use std::time::{Duration, Instant};

use drission::prelude::*;
use serde_json::json;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://www.douyin.com/video/7625611652485877001".into());
    let aweme_id = url
        .split('?')
        .next()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string();
    println!("目标视频 aweme_id = {aweme_id}");

    let browser = Browser::launch(
        BrowserOptions::new()
            .headless(true)
            .locale("zh-CN")
            .timezone("Asia/Shanghai"),
    )
    .await?;
    let tab = browser.latest_tab().await?;
    tab.listen_xhr(&["aweme/v1/web/aweme/detail"]).await?;
    tab.get(&url).await?;

    // 等到【目标视频】的 detail(按 aweme_id 匹配)。
    let stream = tab.listen_stream().await?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut hit = None;
    while Instant::now() < deadline && hit.is_none() {
        tokio::time::sleep(Duration::from_millis(800)).await;
        for p in stream.drain_ready().await {
            if p.url_has("aweme/detail")
                && p.query("aweme_id").as_deref() == Some(aweme_id.as_str())
            {
                hit = Some(p);
                break;
            }
        }
    }

    let Some(pkt) = hit else {
        println!("❌ 未抓到视频 {aweme_id} 的 detail(加载失败 / 被风控 / 该视频不发此接口)");
        browser.quit().await?;
        return Ok(());
    };

    // 同会话的 UA 与 cookie——和 a_bogus 是同一套,脱浏览器复现才自洽。
    let ua = tab
        .run_js("navigator.userAgent")
        .await?
        .as_str()
        .unwrap_or_default()
        .to_string();
    let cookies = tab.cookies().await?;
    let cookie = cookies
        .iter()
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ");

    let out = std::env::current_dir()?.join("dump-env");
    std::fs::create_dir_all(&out)?;
    let replay = json!({
        "aweme_id": aweme_id,
        "url": pkt.url,
        "user_agent": ua,
        "cookie": cookie,
        "a_bogus": pkt.query("a_bogus"),
        "browser_resp_bytes": pkt.response.body.chars().count(),
    });
    std::fs::write(
        out.join("replay.json"),
        serde_json::to_string_pretty(&replay)?,
    )?;

    println!(
        "✅ 抓到 detail(浏览器内 {} 字),a_bogus = {}",
        pkt.response.body.chars().count(),
        pkt.query("a_bogus").unwrap_or_default()
    );
    println!(
        "   导出同会话 cookie {} 项 + UA → dump-env/replay.json",
        cookies.len()
    );
    println!("   脱浏览器复现验证: python3 scripts/douyin_detail_test.py --replay");
    browser.quit().await?;
    Ok(())
}
