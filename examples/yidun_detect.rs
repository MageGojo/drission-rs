//! 易盾点选**风控检测侦察**:搞清它靠什么识别自动化,好针对性写"行为轨迹拟人化"。
//!
//! 做三件事:① 包裹 `addEventListener` 普查易盾**监听了哪些事件**(看它是否追踪 mousemove/pointermove 轨迹);
//! ② 记录我们当前点击**实际产生了多少鼠标事件**(轨迹是否过稀);③ 监听网络抓**提交/check 请求**看submit 了什么。
//!
//! 运行:`cargo run --example yidun_detect --features cdp`(默认无头;`HL=0` 可视)。

use std::time::Duration;

use drission::prelude::*;

const URL: &str = "https://dun.163.com/trial/picture-click";

const TRIGGER_JS: &str = r#"(() => {
  const want = /在线体验|立即体验|点击验证|验证码|体验|验证/;
  const els = [...document.querySelectorAll('button,a,div,span,input')].filter(e => e.offsetParent !== null);
  for (const e of els) { const t = (e.innerText||e.value||'').trim(); if (want.test(t) && t.length <= 16) { e.click(); return 'clicked:'+t; } }
  return 'no-trigger';
})()"#;

// 安装:包裹 addEventListener 做监听普查 + 全局(捕获阶段)记录鼠标/指针事件密度。
const CENSUS_JS: &str = r#"(() => {
  if (window.__censusInstalled) return 'already';
  window.__ev = []; window.__rec = {};
  const seen = new Set();
  const orig = EventTarget.prototype.addEventListener;
  EventTarget.prototype.addEventListener = function (type, fn, opt) {
    try {
      let tgt = this === document ? 'document' : (this === window ? 'window'
        : (this.className ? '.' + String(this.className).split(' ')[0] : (this.tagName || 'node')));
      const key = type + '@' + tgt;
      if (!seen.has(key)) { seen.add(key); window.__ev.push({ type, tgt }); }
    } catch (e) {}
    return orig.call(this, type, fn, opt);
  };
  const rec = (e) => { const k = e.type + (e.isTrusted ? '(trusted)' : '(synth)'); window.__rec[k] = (window.__rec[k] || 0) + 1; };
  for (const t of ['mousemove','pointermove','mousedown','mouseup','pointerdown','pointerup','click','touchstart','touchmove'])
    document.addEventListener(t, rec, true);
  window.__censusInstalled = true; return 'installed';
})()"#;

const BG_RECT_JS: &str = r#"(()=>{const e=document.querySelector('.yidun_bg-img')||document.querySelector('.yidun_bgimg');
  if(!e)return '';const r=e.getBoundingClientRect();return JSON.stringify({x:r.x,y:r.y,w:r.width,h:r.height});})()"#;

const READ_JS: &str = r#"(()=>JSON.stringify({ev:window.__ev||[], rec:window.__rec||{}}))()"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = !matches!(
        std::env::var("HL").ok().as_deref(),
        Some("0") | Some("false")
    );
    let browser = ChromiumBrowser::launch(
        ChromiumOptions::new()
            .headless(headless)
            .window_size(1200, 900),
    )
    .await?;
    let tab = browser.new_tab(Some(URL)).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // ① 尽早安装普查(赶在挑战弹出、SDK 挂监听之前)。
    println!("[detect] census = {:?}", tab.run_js(CENSUS_JS).await.ok());

    // 触发挑战 + 可信点验证按钮。
    let _ = tab.run_js(TRIGGER_JS).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    for s in ["css:.yidun_control", "css:.yidun"] {
        if let Ok(e) = tab.ele(s).await
            && e.click().await.is_ok()
        {
            break;
        }
    }
    tokio::time::sleep(Duration::from_secs(3)).await;

    // ③ 开始抓网络(提交/check)。
    tab.listen().start(&["dun.163"]).await?;

    // 读验证码 rect,在图上取 3 个点,用**当前(非拟人)**方式点击,测事件密度并触发 submit。
    let rect = tab
        .run_js(BG_RECT_JS)
        .await
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let rj: serde_json::Value = serde_json::from_str(&rect).unwrap_or(serde_json::Value::Null);
    let (rx, ry, rw, rh) = (
        rj["x"].as_f64().unwrap_or(0.0),
        rj["y"].as_f64().unwrap_or(0.0),
        rj["w"].as_f64().unwrap_or(0.0),
        rj["h"].as_f64().unwrap_or(0.0),
    );
    if rw > 1.0 {
        for f in [0.3f64, 0.5, 0.7] {
            let (cx, cy) = (rx + rw * f, ry + rh * 0.5);
            let _ = tab.mouse_move(cx - 6.0, cy - 4.0).await;
            tokio::time::sleep(Duration::from_millis(120)).await;
            let _ = tab.mouse_move(cx, cy).await;
            tokio::time::sleep(Duration::from_millis(80)).await;
            let _ = tab.mouse_down(cx, cy).await;
            tokio::time::sleep(Duration::from_millis(60)).await;
            let _ = tab.mouse_up(cx, cy).await;
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    } else {
        println!("[detect] 未取到验证码 rect(rw={rw})");
    }

    tokio::time::sleep(Duration::from_secs(3)).await;
    let pkts = tab
        .listen()
        .wait_count(100, Some(Duration::from_secs(4)))
        .await?;
    tab.listen().stop().await?;

    // 读普查 + 事件密度。
    let info = tab
        .run_js(READ_JS)
        .await
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let ij: serde_json::Value = serde_json::from_str(&info).unwrap_or(serde_json::Value::Null);

    println!("\n=== ① 易盾监听的事件(mouse/pointer/touch/key/对设备相关)===");
    if let Some(evs) = ij["ev"].as_array() {
        for e in evs {
            let t = e["type"].as_str().unwrap_or("");
            if t.contains("mouse")
                || t.contains("pointer")
                || t.contains("touch")
                || t.contains("key")
                || t.contains("device")
                || t.contains("motion")
                || t.contains("scroll")
                || t.contains("wheel")
            {
                println!("  {t:<14} @ {}", e["tgt"].as_str().unwrap_or(""));
            }
        }
    }

    println!("\n=== ② 我们这 3 次点击实际产生的事件密度(越稀越像机器)===");
    if let Some(rec) = ij["rec"].as_object() {
        let mut items: Vec<_> = rec.iter().collect();
        items.sort_by_key(|(k, _)| k.to_string());
        for (k, v) in items {
            println!("  {k:<22} {v}");
        }
    }

    println!("\n=== ③ 提交 / check 等网络请求(看 submit 了什么行为数据)===");
    for p in &pkts {
        if !p.url.contains("dun.163") {
            continue;
        }
        let short: String = p.url.chars().take(120).collect();
        println!("\n[{} {}] {}", p.method, p.response.status, short);
        if let Some(pd) = &p.request.post_data {
            println!("  postData: {}", pd.chars().take(300).collect::<String>());
        }
        let body = p.response.body.trim();
        if body.starts_with('{') || body.contains("JSONP") || body.starts_with("__JSONP") {
            println!("  resp: {}", body.chars().take(300).collect::<String>());
        }
    }

    if !headless {
        tokio::time::sleep(Duration::from_secs(15)).await;
    }
    browser.quit().await?;
    Ok(())
}
