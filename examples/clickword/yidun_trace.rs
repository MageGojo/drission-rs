//! 易盾点选**提交侦察**(为「补轨迹/纯算」可行性评估):跑一遍真实点击,把易盾**上报与校验**请求的
//! **请求参数 / 请求体**全 dump 出来,定位「行为轨迹」到底编码在哪个请求的哪个字段、什么格式。
//!
//! 关注三类:
//! - `c.dun.163.com/api/v3/get`   —— 取图 + front + **token**(本题会话密钥)。
//! - `c.dun.163.com/api/v3/check` —— 校验:**点击点 + 加密行为数据**多半在它的 query 参数里(JSONP GET)。
//! - `ir-sdk.dun.163.com/v4/j/up` —— 设备/行为遥测(POST,ed/es/td/tk 加密)。
//!
//! 运行:`cargo run --example yidun_trace --features cdp,ocr`(默认有头;`HL=1` 无头)。
//! 产物:`target/yidun/trace.txt`(把抓到的关键请求逐条落盘,便于离线分析)。

use std::time::{Duration, Instant};

use drission::cdp::ChromiumTab;
use drission::ocr::ClickWord;
use drission::prelude::*;

const URL: &str = "https://dun.163.com/trial/picture-click";

const TRIGGER_JS: &str = r#"(() => {
  const want = /点击按钮进行验证|开始验证|立即体验|在线体验|点击验证|验证码|体验|验证/;
  const els = [...document.querySelectorAll('button,a,div,span,input')].filter(e => e.offsetParent !== null);
  for (const e of els) { const t=(e.innerText||e.value||'').trim(); if (want.test(t)&&t.length<=16){e.click();return 'clicked:'+t;} }
  return 'no-trigger';
})()"#;

const BG_PROBE_JS: &str = r#"(()=>{const e=document.querySelector('.yidun_bg-img,img.yidun_bg-img,.yidun_bgimg');
  if(!e)return JSON.stringify({vis:false});const b=e.getBoundingClientRect();const s=getComputedStyle(e);
  const vis=e.offsetParent!==null&&s.visibility!=='hidden'&&parseFloat(s.opacity)>0.01&&b.width>2;
  return JSON.stringify({vis,x:b.x,y:b.y,w:b.width,h:b.height,src:(e.currentSrc||e.src||'').slice(-80)});})()"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = matches!(
        std::env::var("HL").ok().as_deref(),
        Some("1") | Some("true")
    );
    let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("yidun");
    std::fs::create_dir_all(&out_dir).ok();
    let trace_path = out_dir.join("trace.txt");
    let mut log = String::new();

    println!("[trace] 加载 det+ocr 模型…");
    let cw = ClickWord::new().await?;

    let browser = ChromiumBrowser::launch(
        ChromiumOptions::new()
            .headless(headless)
            .window_size(1200, 900)
            .add_arg("--force-device-scale-factor=1"),
    )
    .await?;
    let tab = browser.new_tab(Some(URL)).await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // 广谱监听:把易盾相关域全收(get/check/up/图片),便于看清提交链路全貌。
    tab.listen()
        .start(&["dun.163", "ir-sdk", "nosdn", "127.net", "126.net"])
        .await?;

    // 触发 + 等点选图出现 + 主动换一次(逼出监听已就绪后的 get)。
    let _ = tab.run_js(TRIGGER_JS).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    for s in ["css:.yidun_control", "css:.yidun_tips", "css:.yidun"] {
        if let Ok(e) = tab.ele(s).await
            && e.click().await.is_ok()
        {
            break;
        }
    }
    wait_bg_shown(&tab, Duration::from_secs(10)).await;
    if let Ok(e) = tab.ele("css:.yidun_refresh").await {
        let _ = e.click().await;
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    wait_bg_shown(&tab, Duration::from_secs(8)).await;

    // 取本题 get(bg + front + token)。
    let (bg_url, front, token) = match wait_get(&tab, Duration::from_secs(10)).await {
        Some(t) => t,
        None => {
            println!("[trace] 未抓到 api/get;退出");
            browser.quit().await?;
            return Ok(());
        }
    };
    let targets: Vec<String> = front
        .chars()
        .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
        .map(|c| c.to_string())
        .collect();
    println!(
        "[trace] get: front=「{front}」 token={} bg={}",
        short(&token, 24),
        short(&bg_url, 70)
    );
    log.push_str(&format!(
        "=== api/v3/get ===\nfront={front}\ntoken={token}\nbg={bg_url}\n\n"
    ));

    // 识别 + 拟人轨迹点击(产生一次真实 check 提交)。
    if let Ok(cap) = fetch_image(&bg_url).await
        && cap.len() > 1000
    {
        let (cw_w, cw_h) = image::load_from_memory(&cap)
            .map(|im| (im.width() as f64, im.height() as f64))
            .unwrap_or((1.0, 1.0));
        let hits = cw.solve(&cap, &targets).unwrap_or_default();
        if let Some(c) = control_point(&tab).await {
            tab.mouse_move(c.0, c.1).await.ok();
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        if let Ok(view) = tab
            .image_view(".yidun_bg-img, img.yidun_bg-img, .yidun_bgimg")
            .await
            && view.w > 1.0
        {
            let points: Vec<(f64, f64)> = hits
                .iter()
                .map(|h| {
                    (
                        view.x + (h.point.0 as f64 / cw_w.max(1.0)) * view.w,
                        view.y + (h.point.1 as f64 / cw_h.max(1.0)) * view.h,
                    )
                })
                .collect();
            println!(
                "[trace] 命中 {}/{},拟人轨迹点击触发 check…",
                hits.len(),
                targets.len()
            );
            tab.human_click(&points).await?;
        }
    }
    tokio::time::sleep(Duration::from_secs(3)).await;

    // 收集所有抓到的包,dump 关键请求(URL 全量 + postData 全量 + 关键响应体)。
    let pkts = tab
        .listen()
        .wait_count(200, Some(Duration::from_secs(4)))
        .await?;
    tab.listen().stop().await.ok();

    println!(
        "\n[trace] ====== 抓到 {} 个包,dump 关键请求 ======",
        pkts.len()
    );
    for p in &pkts {
        let is_get = p.url.contains("/api/v3/get");
        let is_check = p.url.contains("/api/v3/check") || p.url.contains("/check");
        let is_up = p.url.contains("/v4/j/up") || p.url.contains("ir-sdk");
        if !(is_get || is_check || is_up) {
            continue;
        }
        let tag = if is_check {
            "CHECK"
        } else if is_up {
            "UP"
        } else {
            "GET"
        };
        let mut block = format!("\n=== [{tag}] {} {} ===\n", p.method, p.url);
        // 把 query 拆成参数逐行(check 的轨迹/点击数据多半在这)。
        if let Some(q) = p.url.split_once('?').map(|(_, q)| q) {
            block.push_str("-- query 参数 --\n");
            for kv in q.split('&') {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                let v = urldecode_lossy(v);
                block.push_str(&format!("  {k} = {} (len={})\n", short(&v, 160), v.len()));
            }
        }
        if let Some(pd) = &p.request.post_data {
            block.push_str(&format!(
                "-- postData (len={}) --\n  {}\n",
                pd.len(),
                short(pd, 400)
            ));
        }
        let body = p.response.body.trim();
        if !body.is_empty() {
            block.push_str(&format!(
                "-- response (len={}) --\n  {}\n",
                body.len(),
                short(body, 300)
            ));
        }
        println!("{block}");
        log.push_str(&block);
    }

    std::fs::write(&trace_path, &log).ok();
    println!("\n[trace] 关键请求已落盘 → {}", trace_path.display());

    if !headless {
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    browser.quit().await?;
    Ok(())
}

async fn wait_bg_shown(tab: &ChromiumTab, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(c) = control_point(tab).await {
            tab.mouse_move(c.0, c.1).await.ok();
        }
        if let Ok(v) = tab.run_js(BG_PROBE_JS).await
            && let Some(s) = v.as_str()
            && let Ok(j) = serde_json::from_str::<serde_json::Value>(s)
            && j["vis"].as_bool().unwrap_or(false)
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

async fn control_point(tab: &ChromiumTab) -> Option<(f64, f64)> {
    tab.image_view(".yidun_control, .yidun_tips")
        .await
        .ok()
        .filter(|b| b.w > 1.0)
        .map(|b| (b.x + b.w / 2.0, b.y + b.h / 2.0))
}

/// 取最新 api/get 的 `(bg, front, token)`。
async fn wait_get(tab: &ChromiumTab, timeout: Duration) -> Option<(String, String, String)> {
    let deadline = Instant::now() + timeout;
    let mut latest = None;
    loop {
        match tab.listen().wait(Some(Duration::from_millis(300))).await {
            Ok(Some(p)) => {
                if p.url.contains("/get")
                    && let Some(t) = parse_get(&p.response.body)
                {
                    latest = Some(t);
                }
            }
            Ok(None) => {
                if latest.is_some() || Instant::now() >= deadline {
                    return latest;
                }
            }
            Err(_) => return latest,
        }
        if Instant::now() >= deadline {
            return latest;
        }
    }
}

fn parse_get(body: &str) -> Option<(String, String, String)> {
    let a = body.find('{')?;
    let b = body.rfind('}')?;
    let v: serde_json::Value = serde_json::from_str(&body[a..=b]).ok()?;
    let d = &v["data"];
    let bg = d["bg"]
        .get(0)
        .and_then(|x| x.as_str())
        .or_else(|| d["bg"].as_str())?;
    Some((
        bg.to_string(),
        d["front"].as_str().unwrap_or("").to_string(),
        d["token"].as_str().unwrap_or("").to_string(),
    ))
}

fn short(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 极简 urldecode(只还原 %XX 与 +;失败原样保留),便于看清参数原貌。
fn urldecode_lossy(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let h = u8::from_str_radix(&s[i + 1..i + 3], 16);
                if let Ok(b) = h {
                    out.push(b);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
