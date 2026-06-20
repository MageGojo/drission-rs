//! 长监听:在 bilibili 多P视频页连续抓「每个分集各自的 playurl 签名」(wbi 的 `w_rid`/`wts`)。
//!
//! 机制:bilibili 视频页**首集**的播放地址是服务端渲染(SSR)注入 `window.__playinfo__` 的,
//! 首屏**不发** `x/player/wbi/playurl` 这个 XHR;只有在页面内**切换分集**(SPA, `pushState` 改
//! `?p=N`)时,前端才会带 wbi 签名请求 playurl 拿新分集的流地址。所以这里用长监听常驻 hook +
//! 后台抽取(不丢包),通过**点击分集列表项**(`.video-pod__item`)逐集推进,把每个分集各自签名的
//! playurl 连续抓下来。
//!
//! 这里只有 bilibili 业务(点哪个分集、读哪些参数);通用能力都在库里:`listen_xhr` 常驻 hook、
//! `listen_stream`/`drain_ready` 后台不丢包批量取、`eles`+`click` 点元素、`packet.query()` 取参数。
//!
//! 运行:`cargo run --example bilibili_listen_long [-- <视频URL> <数量>]`

use std::collections::HashSet;
use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let mut a = std::env::args().skip(1);
    let start = a
        .next()
        .unwrap_or_else(|| "https://www.bilibili.com/video/BV1wJwCzjELS?p=1".into());
    let want: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(5);

    let browser = Browser::launch(
        BrowserOptions::new()
            .headless(true)
            .locale("zh-CN")
            .timezone("Asia/Shanghai"),
    )
    .await?;
    let tab = browser.latest_tab().await?;

    // 导航前开 hook(常驻 fetch/XHR 拦截),只盯 wbi 版 playurl。
    tab.listen_xhr(&["x/player/wbi/playurl"]).await?;
    tab.get(&start).await?;
    let stream = tab.listen_stream().await?; // 开后台抽取,边切边抓不丢包

    let mut seen: HashSet<String> = HashSet::new(); // 按分集 cid 去重
    let mut got = 0usize;
    let mut idx = 1usize; // 跳过首集(items[0] 已 active 且走 SSR,无 playurl)

    while got < want {
        // 切到下一个分集:每轮重新定位(点击触发 SPA 重渲染,旧元素句柄会失效)。
        let items = tab.eles(".video-pod__item").await?;
        if idx >= items.len() {
            println!("已到最后一个分集(共 {} 个),提前结束。", items.len());
            break;
        }
        items[idx].click().await?;
        idx += 1;

        // 等这一集的 playurl 发出并到齐;慢一拍也不丢——后台抽取会持续搬进缓冲,下一轮补抓。
        tokio::time::sleep(Duration::from_millis(2500)).await;
        for p in stream.drain_ready().await {
            if !p.url_has("playurl") {
                continue;
            }
            let Some(cid) = p
                .query("cid")
                .filter(|c| !c.is_empty() && seen.insert(c.clone()))
            else {
                continue;
            };
            got += 1;
            println!("\n#{got}  分集 cid={cid}");
            println!("   bvid  = {}", p.query("bvid").unwrap_or_default());
            println!("   w_rid = {}", p.query("w_rid").unwrap_or_default());
            println!("   wts   = {}", p.query("wts").unwrap_or_default());
            println!(
                "   qn={} fnval={} fnver={}",
                p.query("qn").unwrap_or_default(),
                p.query("fnval").unwrap_or_default(),
                p.query("fnver").unwrap_or_default()
            );
            println!("   playurl 响应体 {} 字", p.response.body.chars().count());
            if got >= want {
                break;
            }
        }
    }

    println!("\n完成:连续抓到 {got} 个分集各自的 playurl 签名(w_rid/wts)。");
    browser.quit().await?;
    Ok(())
}
