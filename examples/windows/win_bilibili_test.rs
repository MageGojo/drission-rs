//! Windows 端到端**硬核**验证:在 bilibili 多P视频页用「长监听 + 后台抽取(不丢包)+ 点分集
//! 推进」连续抓每个分集各自的 wbi 签名(`w_rid`/`wts`)——这条链路同时压满了 Windows 传输
//! (命名管道 fd3/4)、Juggler 事件泵、页面 hook、元素点击、SPA 重渲染。
//!
//! 逻辑同 `examples/bilibili_listen_long.rs`,额外**把结果落盘成 JSON**(`drission_win_test_result.json`)
//! 作为可核对的数据产物——跑完把这个文件发回即可判定是否成功。
//!
//! 运行(原生 Windows 构建,或用 scripts/win-cross-build.sh 交叉后在 Win 上跑 .exe):
//!   `cargo run --example win_bilibili_test --no-default-features --features camoufox` /  `win_bilibili_test.exe [视频URL] [数量]`
//!   (默认抓 3 个分集)
//!   PowerShell 看详细日志:`$env:RUST_LOG="debug"; .\win_bilibili_test.exe`

use std::collections::HashSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use drission::prelude::*;
use serde_json::{Value, json};

/// 页面现场探针:为何没抓到 playurl?——是单P(无分集)、登录墙、还是结构变了。
const PAGE_PROBE_JS: &str = r#"(function () {
  var pages = -1;
  try { pages = (window.__INITIAL_STATE__ && window.__INITIAL_STATE__.videoData
    && window.__INITIAL_STATE__.videoData.pages || []).length; } catch (e) {}
  var txt = (document.body && document.body.innerText || '');
  return JSON.stringify({
    title: document.title,
    href: location.href,
    podItems: document.querySelectorAll('.video-pod__item').length,
    podAny: document.querySelectorAll('[class*="pod"]').length,
    pages: pages,
    hasPlayinfo: (typeof window.__playinfo__ !== 'undefined'),
    // 仅认实际的登录弹窗元素(文本里的"登录/注册"在 bili 页眉常驻,会误判)。
    loginWall: !!document.querySelector('.bili-mini-mask, .login-panel, .bili-login, .unlogin-popover, .login-tip'),
    // 具体错误措辞(避免"验证"等常驻词误判)。
    errText: txt.indexOf('出错了') >= 0 || txt.indexOf('地区限制') >= 0
      || txt.indexOf('该地区') >= 0 || txt.indexOf('稍后再试') >= 0 || txt.indexOf('请求过于频繁') >= 0,
    bodyLen: (document.body && document.body.innerHTML || '').length
  });
})()"#;

/// `run_js` 返回的多为 `JSON.stringify` 字符串;解析成对象,失败则原样返回。
fn parse_json_str(v: Value) -> Value {
    v.as_str()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or(v)
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let mut a = std::env::args().skip(1);
    let start = a
        .next()
        .unwrap_or_else(|| "https://www.bilibili.com/video/BV1wJwCzjELS?p=1".into());
    let want: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(3);

    let t0 = SystemTime::now();
    let started_ms = t0
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    println!("== drission-rs Windows 硬核测试 (bilibili 长监听) ==");
    println!(
        "  OS/ARCH  : {}/{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("  URL      : {start}");
    println!("  目标数量 : {want}");

    // 解析浏览器路径(Windows 下若设了 CAMOUFOX_BIN 指向随包浏览器,这步是秒回)。
    let cam = drission::launcher::ensure_camoufox(None).await?;
    println!("  Camoufox : {}", cam.display());
    println!();

    let mut packets: Vec<Value> = Vec::new();
    let mut err_msg: Option<String> = None;
    let mut user_agent = String::new();
    let mut webdriver = String::new();
    let mut page_probe: Option<Value> = None;
    let mut xhr_seen: Vec<String> = Vec::new();

    // 用闭包跑核心流程,任何错误转成数据里的 error 字段(不让进程直接崩,保证落盘)。
    let run = async {
        let browser = Browser::launch(
            BrowserOptions::new()
                .headless(false) // 有头模式:会弹出可见的 Camoufox 窗口
                .locale("zh-CN")
                .timezone("Asia/Shanghai"),
        )
        .await?;
        let tab = browser.latest_tab().await?;

        // 广捕所有 XHR/fetch(空关键字=全部):既收 playurl,也记录全部请求路径用于诊断。
        tab.listen_xhr(&[]).await?;
        tab.get(&start).await?;

        user_agent = tab.user_agent().await?;
        webdriver = tab.run_js("navigator.webdriver").await?.to_string();

        // 落盘页面现场:标题/URL/分集数/登录墙/__INITIAL_STATE__ pages,定位"为何 0 数据"。
        tokio::time::sleep(Duration::from_millis(1500)).await;
        page_probe = tab.run_js(PAGE_PROBE_JS).await.ok().map(parse_json_str);
        println!("页面现场: {page_probe:?}");

        let stream = tab.listen_stream().await?;
        let mut seen: HashSet<String> = HashSet::new();
        let mut seen_urls: HashSet<String> = HashSet::new();
        let mut got = 0usize;

        // 1) 等分集列表渲染就绪——慢机/冷启动下首屏可能还没出分集面板(Windows 上常见)。
        let mut list_n = 0usize;
        for _ in 0..30 {
            list_n = tab.eles(".video-pod__item").await?.len();
            if list_n > 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        println!("分集列表项: {list_n}");

        // 2) 自适应抓取:逐集点击,点后**轮询**(每 400ms 抽一次,最多 ~10s)等它的 playurl
        //    到达——慢机上 playurl 常晚于固定等待窗口;轮询 + 累积抽取既不丢包也不空等。
        let mut idx = 1usize; // 跳过首集(SSR,无 playurl XHR)
        let overall = std::time::Instant::now() + Duration::from_secs(180);
        while got < want && std::time::Instant::now() < overall {
            let items = tab.eles(".video-pod__item").await?;
            if items.is_empty() {
                println!("没找到分集列表项,提前结束。");
                break;
            }
            if idx >= items.len() {
                // 列表可能虚拟滚动:把最后一项滚进视口尝试加载更多,没有更多就结束。
                if let Some(last) = items.last() {
                    let _ = last.scroll_into_view().await;
                }
                tokio::time::sleep(Duration::from_millis(700)).await;
                if tab.eles(".video-pod__item").await?.len() <= items.len() {
                    println!("已到最后一个分集(共 {} 个),提前结束。", items.len());
                    break;
                }
                continue;
            }

            items[idx].click().await?;
            idx += 1;

            let click_dl = std::time::Instant::now() + Duration::from_secs(10);
            let mut got_this = false;
            while !got_this && std::time::Instant::now() < click_dl {
                tokio::time::sleep(Duration::from_millis(400)).await;
                let batch = stream.drain_ready().await;
                // 记录本批所有请求路径(去重、去 query、上限 80 条)用于诊断。
                for p in &batch {
                    let path = p.url.split('?').next().unwrap_or(&p.url).to_string();
                    if seen_urls.insert(path.clone()) && xhr_seen.len() < 80 {
                        xhr_seen.push(path);
                    }
                }
                for p in batch {
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
                    got_this = true;
                    println!(
                        "#{got}  cid={cid}  w_rid={}  wts={}  body={}字",
                        p.query("w_rid").unwrap_or_default(),
                        p.query("wts").unwrap_or_default(),
                        p.response.body.chars().count()
                    );
                    packets.push(json!({
                        "n": got,
                        "cid": cid,
                        "bvid": p.query("bvid").unwrap_or_default(),
                        "w_rid": p.query("w_rid").unwrap_or_default(),
                        "wts": p.query("wts").unwrap_or_default(),
                        "qn": p.query("qn").unwrap_or_default(),
                        "fnval": p.query("fnval").unwrap_or_default(),
                        "fnver": p.query("fnver").unwrap_or_default(),
                        "status": p.response.status,
                        "resp_body_chars": p.response.body.chars().count(),
                        "url_head": p.url.chars().take(80).collect::<String>(),
                    }));
                    if got >= want {
                        break;
                    }
                }
            }
        }

        // 收尾再抽一次,把剩余请求路径也记进诊断。
        for p in stream.drain_ready().await {
            let path = p.url.split('?').next().unwrap_or(&p.url).to_string();
            if seen_urls.insert(path.clone()) && xhr_seen.len() < 80 {
                xhr_seen.push(path);
            }
        }

        browser.quit().await?;
        Ok::<usize, drission::Error>(got)
    }
    .await;

    let got = match run {
        Ok(n) => n,
        Err(e) => {
            err_msg = Some(e.to_string());
            packets.len()
        }
    };

    let elapsed_ms = t0.elapsed().map(|d| d.as_millis()).unwrap_or(0);
    let success = err_msg.is_none() && got > 0;

    let result = json!({
        "tool": "drission-rs",
        "test": "win_bilibili_listen_long",
        "platform": { "os": std::env::consts::OS, "arch": std::env::consts::ARCH },
        "camoufox_path": cam.display().to_string(),
        "start_url": start,
        "want": want,
        "got": got,
        "success": success,
        "error": err_msg,
        "navigator": { "userAgent": user_agent, "webdriver": webdriver },
        "page_probe": page_probe,
        "xhr_seen": xhr_seen,
        "started_unix_ms": started_ms,
        "elapsed_ms": elapsed_ms,
        "packets": packets,
    });

    let out = "drission_win_test_result.json";
    let pretty = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
    if let Err(e) = std::fs::write(out, &pretty) {
        eprintln!("写结果文件失败: {e}");
    }

    println!();
    println!("抓到分集签名:{got}/{want}");
    println!("结果文件:{out}(请把这个文件发回以核对数据)");
    println!(
        "判定:{}",
        if success {
            "通过 ✅ —— Windows 传输 + 长监听链路工作正常"
        } else {
            "失败 ❌ —— 见结果文件里的 error / 用 RUST_LOG=debug 复跑"
        }
    );

    // 给个非零退出码方便脚本判断(成功 0,失败 1)。
    if !success {
        std::process::exit(1);
    }
    Ok(())
}
