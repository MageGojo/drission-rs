//! GeeTest **诊断探针**:不为过验证码,只为"取证"——看极验从我们 `tab.mouse_*` 的拖拽里
//! 到底收到了哪些事件、关键属性长啥样,以此定位本库还缺什么(指针事件?movementX?screenX?)。
//!
//! 运行:`HL=0 cargo run --example geetest_probe`

use std::time::Duration;

use drission::prelude::*;
use tokio::time::sleep;

const URL: &str = "https://demos.geetest.com/slide-float.html";

/// 把合成 PointerEvent 的空 `pointerType` 修补成 `"mouse"`(真实鼠标值)。
const POINTERTYPE_FIX: &str = r#"(function(){
  try {
    var proto = PointerEvent.prototype;
    var d = Object.getOwnPropertyDescriptor(proto, 'pointerType');
    if (d && d.get) {
      var orig = d.get;
      Object.defineProperty(proto, 'pointerType', {
        configurable: true, enumerable: d.enumerable,
        get: function(){ var v = orig.call(this); return (v === '' || v == null) ? 'mouse' : v; }
      });
    }
  } catch(e){}
})()"#;

/// 在滑块把手 + document(捕获阶段)装事件记录器,记录 pointer/mouse/touch 三类事件的关键属性。
const INSTALL_LOGGER: &str = r#"(function(){
  window.__evlog = [];
  var types = ['pointerdown','pointermove','pointerup',
               'mousedown','mousemove','mouseup',
               'touchstart','touchmove','touchend'];
  function rec(e){
    var t = (e.touches && e.touches[0]) || e;
    window.__evlog.push({
      type: e.type,
      trusted: e.isTrusted,
      clientX: t.clientX, clientY: t.clientY,
      screenX: t.screenX, screenY: t.screenY,
      movementX: e.movementX, movementY: e.movementY,
      button: e.button, buttons: e.buttons,
      pointerType: e.pointerType, pressure: e.pressure, pointerId: e.pointerId,
      ts: Math.round(performance.now())
    });
  }
  var el = document.querySelector('.geetest_slider_button') || document;
  types.forEach(function(ty){ document.addEventListener(ty, rec, true); });
  return types.join(',');
})()"#;

/// 极验最可能据此判 bot 的环境信号一把抓。
const ENV_SIGNALS: &str = r#"(function(){
  function g(f){ try{ return f(); }catch(e){ return 'ERR:'+e; } }
  return JSON.stringify({
    webdriver: g(function(){ return navigator.webdriver; }),
    userAgent: g(function(){ return navigator.userAgent; }),
    platform: g(function(){ return navigator.platform; }),
    languages: g(function(){ return (navigator.languages||[]).join(','); }),
    hardwareConcurrency: g(function(){ return navigator.hardwareConcurrency; }),
    deviceMemory: g(function(){ return navigator.deviceMemory; }),
    maxTouchPoints: g(function(){ return navigator.maxTouchPoints; }),
    plugins: g(function(){ return navigator.plugins.length; }),
    screen: g(function(){ return screen.width+'x'+screen.height+' avail '+screen.availWidth+'x'+screen.availHeight; }),
    innerWH: g(function(){ return window.innerWidth+'x'+window.innerHeight; }),
    outerWH: g(function(){ return window.outerWidth+'x'+window.outerHeight; }),
    devicePixelRatio: g(function(){ return window.devicePixelRatio; }),
    screenXY: g(function(){ return window.screenX+','+window.screenY; }),
    pointerEnabled: g(function(){ return window.PointerEvent !== undefined; }),
    touchEnabled: g(function(){ return 'ontouchstart' in window; }),
    permissionsQuery: g(function(){ return typeof navigator.permissions !== 'undefined'; }),
    webglVendor: g(function(){
      var c=document.createElement('canvas'); var gl=c.getContext('webgl');
      var dbg=gl.getExtension('WEBGL_debug_renderer_info');
      return gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL)+' / '+gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL);
    })
  });
})()"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let fix = std::env::var("FIX").map(|v| v == "1").unwrap_or(false);
    let browser = Browser::launch(BrowserOptions::new().headless(headless)).await?;
    let tab = browser.latest_tab().await?;

    // 实验:导航前注入 pointerType 修补(把合成事件的空 pointerType 修成 "mouse")。
    if fix {
        tab.add_init_script(POINTERTYPE_FIX).await?;
        println!("[*] 已注入 pointerType 修补(FIX=1)");
    }

    println!("[*] 打开 {URL}");
    tab.get(URL).await?;
    sleep(Duration::from_secs(3)).await;

    // 环境信号(导航后即可读;极验在加载验证码前就采集了一部分)。
    let env = tab.run_js(ENV_SIGNALS).await?;
    println!("\n==== 环境信号 ====");
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(env.as_str().unwrap_or("{}")) {
        if let Some(obj) = v.as_object() {
            for (k, val) in obj {
                println!("  {k:<20} = {val}");
            }
        }
    }

    println!("\n[*] 点击雷达按钮弹出滑块…");
    tab.ele("css:.geetest_radar_btn").await?.click().await?;
    let _ = tab.ele("css:.geetest_slider_button").await?;
    sleep(Duration::from_millis(800)).await;

    // 装事件记录器,然后做一段小拖拽(不求解,只看事件)。
    let types = tab.run_js(INSTALL_LOGGER).await?;
    println!("\n[*] 已挂记录器,监听: {}", types.as_str().unwrap_or(""));

    let (hx, hy) = handle_center(&tab).await?;
    println!("[*] 把手中心 ({hx:.0},{hy:.0}),开始小拖拽…");
    tab.mouse_move(hx - 40.0, hy - 20.0).await?;
    sleep(Duration::from_millis(60)).await;
    tab.mouse_move(hx, hy).await?;
    sleep(Duration::from_millis(60)).await;
    tab.mouse_down(hx, hy).await?;
    sleep(Duration::from_millis(80)).await;
    let mut x = hx;
    // 用 fire 快路径 + ~12ms sleep,验证能达到真人级密集采样。
    for _ in 0..30 {
        x += 3.0;
        tab.mouse_drag_fast(x, hy + 1.0)?;
        sleep(Duration::from_millis(12)).await;
    }
    tab.mouse_up(x, hy).await?;
    sleep(Duration::from_millis(200)).await;

    // 读回事件记录。
    let log = tab
        .run_js("JSON.stringify({n: window.__evlog.length, ev: window.__evlog})")
        .await?;
    let lv: serde_json::Value =
        serde_json::from_str(log.as_str().unwrap_or("{}")).unwrap_or_default();
    let n = lv["n"].as_u64().unwrap_or(0);

    println!("\n==== 捕获到 {n} 个事件 ====");
    // 按类型统计。
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    if let Some(arr) = lv["ev"].as_array() {
        for e in arr {
            *counts
                .entry(e["type"].as_str().unwrap_or("?").to_string())
                .or_default() += 1;
        }
    }
    println!("  类型分布: {counts:?}");

    // 打印前若干条样本(含关键属性)。
    if let Some(arr) = lv["ev"].as_array() {
        println!("\n  样本(前 6 条 + 后 2 条):");
        let show: Vec<&serde_json::Value> = arr
            .iter()
            .take(6)
            .chain(arr.iter().rev().take(2).rev())
            .collect();
        for e in show {
            println!(
                "    {:<11} trusted={} client=({},{}) screen=({},{}) move=({},{}) btn={}/{} ptr={}/{}",
                e["type"].as_str().unwrap_or("?"),
                e["trusted"],
                e["clientX"], e["clientY"],
                e["screenX"], e["screenY"],
                e["movementX"], e["movementY"],
                e["button"], e["buttons"],
                e["pointerType"], e["pressure"],
            );
        }
    }

    // 采样间隔(pointermove 之间的 ts 差):真实拖拽约 8~16ms 一帧。
    if let Some(arr) = lv["ev"].as_array() {
        let ts: Vec<f64> = arr
            .iter()
            .filter(|e| e["type"].as_str() == Some("pointermove"))
            .filter_map(|e| e["ts"].as_f64())
            .collect();
        if ts.len() >= 2 {
            let deltas: Vec<f64> = ts.windows(2).map(|w| w[1] - w[0]).collect();
            let avg = deltas.iter().sum::<f64>() / deltas.len() as f64;
            println!("\n  pointermove 采样间隔(ms): {deltas:?}  平均≈{avg:.0}");
        }
    }

    // 诊断小结。
    println!("\n==== 诊断 ====");
    let has_pointer = counts.keys().any(|k| k.starts_with("pointer"));
    let has_mouse = counts.keys().any(|k| k.starts_with("mouse"));
    println!(
        "  指针事件(pointer*): {}",
        if has_pointer { "✅ 有" } else { "❌ 无(极验 v4 主要监听 pointer*,缺失则轨迹为空)" }
    );
    println!("  鼠标事件(mouse*): {}", if has_mouse { "✅ 有" } else { "❌ 无" });
    // 检查 movementX/screenX 是否恒为可疑值。
    if let Some(arr) = lv["ev"].as_array() {
        let moves: Vec<&serde_json::Value> = arr
            .iter()
            .filter(|e| e["type"].as_str() == Some("mousemove") || e["type"].as_str() == Some("pointermove"))
            .collect();
        let all_move_zero = !moves.is_empty()
            && moves.iter().all(|e| e["movementX"].as_f64().unwrap_or(0.0) == 0.0);
        let screen_eq_client = moves.iter().take(3).all(|e| {
            e["screenX"].as_f64().unwrap_or(-1.0) == e["clientX"].as_f64().unwrap_or(-2.0)
        });
        println!(
            "  movementX 恒为 0: {}",
            if all_move_zero { "⚠️ 是(真实鼠标移动必非 0,这是 bot tell)" } else { "✅ 否" }
        );
        println!(
            "  screenX==clientX: {}",
            if screen_eq_client { "⚠️ 是(真实环境 screenX=clientX+window.screenX+边框,相等可疑)" } else { "✅ 否" }
        );
    }

    if !headless {
        sleep(Duration::from_secs(2)).await;
    }
    browser.quit().await?;
    Ok(())
}

async fn handle_center(tab: &Tab) -> drission::Result<(f64, f64)> {
    let v = tab
        .run_js(
            "(function(){var e=document.querySelector('.geetest_slider_button');\
             var r=e.getBoundingClientRect(); return [r.left+r.width/2, r.top+r.height/2];})()",
        )
        .await?;
    let x = v.get(0).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
    let y = v.get(1).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
    Ok((x, y))
}
