//! 每浏览器不同指纹(CDP 后端)实跑验证。
//!
//! 起 N 个浏览器,每个套一份不同指纹([`CdpFingerprintPool`]),各自 dump
//! UA/platform/语言/时区/屏幕/硬件并发/内存/WebGL/canvas 哈希,打印成表 —— 直观看出"每浏览器各异"。
//!
//! 运行:
//! - `cargo run --example cdp_fingerprint`                  默认无头、3 个、同 OS 变体(Turnstile 友好)
//! - `N=5 cargo run --example cdp_fingerprint`              5 个
//! - `PERSONA=1 cargo run --example cdp_fingerprint`        完整跨 OS 画像(伪装 UA/platform/WebGL)
//! - `HEADFUL=1 cargo run --example cdp_fingerprint`        有头可视

use drission::Result;
use drission::cdp::{CdpFingerprintPool, ChromiumBrowser, ChromiumOptions};
use serde_json::Value;

const DUMP_JS: &str = r#"(function(){
  function canvasHash(){
    try{
      var c=document.createElement('canvas'); c.width=220;c.height=50;
      var x=c.getContext('2d');
      x.textBaseline='top'; x.font='14px Arial'; x.fillStyle='#069'; x.fillText('drission-fp-😀',2,2);
      x.fillStyle='rgba(102,200,0,0.7)'; x.fillText('drission-fp',4,17);
      var u=c.toDataURL(); var h=0;
      for(var i=0;i<u.length;i++){ h=(h*31+u.charCodeAt(i))>>>0; }
      return ('00000000'+h.toString(16)).slice(-8);
    }catch(e){ return 'err'; }
  }
  function webgl(){
    try{
      var c=document.createElement('canvas'); var gl=c.getContext('webgl')||c.getContext('experimental-webgl');
      if(!gl) return 'none';
      var dbg=gl.getExtension('WEBGL_debug_renderer_info');
      return dbg? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
    }catch(e){ return 'err'; }
  }
  return JSON.stringify({
    ua: navigator.userAgent,
    platform: navigator.platform,
    languages: (navigator.languages||[]).join(','),
    hc: navigator.hardwareConcurrency,
    dm: navigator.deviceMemory,
    screen: screen.width+'x'+screen.height,
    dpr: window.devicePixelRatio,
    tz: (Intl.DateTimeFormat().resolvedOptions()||{}).timeZone,
    webgl: webgl(),
    canvas: canvasHash()
  });
})()"#;

#[tokio::main]
async fn main() -> Result<()> {
    let n: usize = std::env::var("N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let headful = std::env::var("HEADFUL").as_deref() == Ok("1");
    let persona = std::env::var("PERSONA").as_deref() == Ok("1");

    let pool = if persona {
        CdpFingerprintPool::personas(n)
    } else {
        CdpFingerprintPool::generate(n)
    };
    println!(
        "模式: {} | 浏览器数: {} | headless: {}\n",
        if persona {
            "完整跨 OS 画像(伪装 UA/platform/WebGL)"
        } else {
            "同 OS 变体(保真 UA/WebGL,Turnstile 友好)"
        },
        n,
        !headful
    );

    let base = ChromiumOptions::new().headless(!headful);
    for (i, fp) in pool.profiles().iter().enumerate() {
        let opts = fp.apply_to_options(base.clone());
        let browser = ChromiumBrowser::launch(opts).await?;
        let tab = browser.new_tab(Some("about:blank")).await?;
        // 必须导航到一份新文档,导航前注入脚本才会对其生效(addScriptToEvaluateOnNewDocument 只作用于后续文档)。
        tab.get(
            "data:text/html,<!doctype html><meta charset=utf-8><title>fp</title><body>fp</body>",
        )
        .await?;
        let raw = tab
            .run_js(DUMP_JS)
            .await?
            .as_str()
            .map(String::from)
            .unwrap_or_default();
        let v: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        println!("── 浏览器 #{} ───────────────────────────────", i + 1);
        println!("  UA       : {}", v["ua"].as_str().unwrap_or("?"));
        println!("  platform : {}", v["platform"].as_str().unwrap_or("?"));
        println!("  languages: {}", v["languages"].as_str().unwrap_or("?"));
        println!("  timezone : {}", v["tz"].as_str().unwrap_or("?"));
        println!(
            "  screen   : {}  dpr={}",
            v["screen"].as_str().unwrap_or("?"),
            v["dpr"]
        );
        println!("  hw cores : {}   memory: {} GB", v["hc"], v["dm"]);
        println!("  WebGL    : {}", v["webgl"].as_str().unwrap_or("?"));
        println!("  canvas#  : {}", v["canvas"].as_str().unwrap_or("?"));
        println!();
        let _ = browser.quit().await;
    }

    println!(
        "完成。canvas# / 屏幕 / 时区 / 硬件 各浏览器应各不相同(persona 模式连 UA/WebGL 也不同)。"
    );
    Ok(())
}
