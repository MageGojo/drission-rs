//! 长监听:连续抓"下一个视频"各自的签名 detail 参数。
//!
//! 机制:抖音直链页沉浸播放器的"下一批视频"来自预取的 `aweme/related`(它的 body 里是下一批
//! aweme_id),滑动切换播放用这份缓存、不再发 `aweme/detail`。所以这里**同时盯 detail+related**,
//! 用 related 给的 id 逐个开直链——每个视频都会发它**自己签名**(独立 a_bogus)的 detail。
//!
//! 这里只有 douyin 业务(从 related 取 id、开下一条直链);通用能力(取 query/转 JSON/批量取包)
//! 都在库里:`packet.query()` / `packet.json()` / `stream.drain_ready()`。
//!
//! 运行:`cargo run --example douyin_listen_long --no-default-features --features camoufox [-- <短链/视频URL> <数量>]`

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let mut a = std::env::args().skip(1);
    let start = a
        .next()
        .unwrap_or_else(|| "https://v.douyin.com/I1mlU0fBFhI/".into());
    let want: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(5);

    let browser = Browser::launch(
        BrowserOptions::new()
            .headless(true)
            .locale("zh-CN")
            .timezone("Asia/Shanghai"),
    )
    .await?;
    let tab = browser.latest_tab().await?;

    tab.listen_xhr(&["aweme/v1/web/aweme/detail", "aweme/v1/web/aweme/related"])
        .await?;
    tab.get(&start).await?;
    let stream = tab.listen_stream().await?;
    tab.press_key("ArrowDown").await?; // 触发当前视频的 related 预取

    let mut queue: VecDeque<String> = VecDeque::new(); // 待抓的下一批 id
    let mut seen: HashSet<String> = HashSet::new();
    let mut got = 0usize;

    while got < want {
        tokio::time::sleep(Duration::from_millis(1500)).await; // 等这一拍的请求到齐
        for p in stream.drain_ready().await {
            if p.url_has("aweme/detail") {
                if let Some(id) = p.query("aweme_id").filter(|id| seen.insert(id.clone())) {
                    got += 1;
                    println!("\n#{got}  视频 {id}");
                    println!("   a_bogus  = {}", p.query("a_bogus").unwrap_or_default());
                    println!("   verifyFp = {}", p.query("verifyFp").unwrap_or_default());
                    println!("   detail 响应体 {} 字", p.response.body.chars().count());
                    if got >= want {
                        break;
                    }
                }
            } else if p.url_has("aweme/related") {
                queue.extend(related_ids(&p).into_iter().filter(|id| !seen.contains(id)));
            }
        }
        // 打开下一个未抓过的视频(触发它自己的签名 detail)。
        while let Some(id) = queue.pop_front() {
            if !seen.contains(&id) {
                tab.get(&format!("https://www.douyin.com/video/{id}"))
                    .await?;
                tab.press_key("ArrowDown").await?;
                break;
            }
        }
    }

    println!("\n完成:连续抓到 {got} 个视频各自的签名 detail。");
    browser.quit().await?;
    Ok(())
}

/// 业务逻辑:从 `aweme/related` 响应体里取下一批视频 id(`aweme_list[].aweme_id`)。
fn related_ids(p: &DataPacket) -> Vec<String> {
    p.json()
        .and_then(|v| {
            v.get("aweme_list").and_then(|a| a.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|x| x["aweme_id"].as_str().map(String::from))
                    .collect()
            })
        })
        .unwrap_or_default()
}
