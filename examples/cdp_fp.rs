//! 指纹探针:dump 一份全面的浏览器指纹 JSON(有头/无头各跑,diff 出无头暴露的差异)。
//! 用法:`HEADLESS=1 cargo run --example cdp_fp --features cdp > fp_headless.json`
//!       `cargo run --example cdp_fp --features cdp > fp_headful.json`

use drission::prelude::*;

const PROBE: &str = r#"(async () => {
  const out = {};
  const nav = navigator;
  out.webdriver = nav.webdriver;
  out.userAgent = nav.userAgent;
  out.platform = nav.platform;
  out.vendor = nav.vendor;
  out.language = nav.language;
  out.languages = nav.languages;
  out.hardwareConcurrency = nav.hardwareConcurrency;
  out.deviceMemory = nav.deviceMemory;
  out.maxTouchPoints = nav.maxTouchPoints;
  out.pdfViewerEnabled = nav.pdfViewerEnabled;
  out.pluginsLen = nav.plugins ? nav.plugins.length : -1;
  out.plugins = nav.plugins ? Array.from(nav.plugins).map(p=>p.name) : [];
  out.mimeTypesLen = nav.mimeTypes ? nav.mimeTypes.length : -1;
  out.cookieEnabled = nav.cookieEnabled;
  try {
    out.uaData = nav.userAgentData ? {mobile:nav.userAgentData.mobile, platform:nav.userAgentData.platform, brands:nav.userAgentData.brands} : null;
    if (nav.userAgentData) {
      out.uaDataHigh = await nav.userAgentData.getHighEntropyValues(['architecture','bitness','model','platformVersion','uaFullVersion','fullVersionList','wow64']);
    }
  } catch(e){ out.uaDataErr = ''+e; }
  out.hasChrome = typeof window.chrome;
  out.chromeKeys = window.chrome ? Object.keys(window.chrome) : [];
  out.hasChromeRuntime = !!(window.chrome && window.chrome.runtime);
  out.hasChromeLoadTimes = !!(window.chrome && window.chrome.loadTimes);
  out.hasChromeCsi = !!(window.chrome && window.chrome.csi);
  try {
    out.notifPerm = (typeof Notification!=='undefined') ? Notification.permission : 'no-Notification';
    const p = await navigator.permissions.query({name:'notifications'});
    out.notifQuery = p.state;
  } catch(e){ out.permErr=''+e; }
  out.screen = {w:screen.width,h:screen.height,availW:screen.availWidth,availH:screen.availHeight,colorDepth:screen.colorDepth,pixelDepth:screen.pixelDepth};
  out.dpr = window.devicePixelRatio;
  out.win = {innerW:innerWidth,innerH:innerHeight,outerW:outerWidth,outerH:outerHeight,screenX:screenX,screenY:screenY};
  out.doc = {hidden:document.hidden, visibility:document.visibilityState, hasFocus:document.hasFocus()};
  out.mq = {
    dark: matchMedia('(prefers-color-scheme: dark)').matches,
    light: matchMedia('(prefers-color-scheme: light)').matches,
    reducedMotion: matchMedia('(prefers-reduced-motion: reduce)').matches,
    hover: matchMedia('(hover: hover)').matches,
    pointerFine: matchMedia('(pointer: fine)').matches,
  };
  function gl(type){
    try {
      const c = document.createElement('canvas');
      const g = c.getContext(type) || c.getContext('experimental-'+type);
      if(!g) return {ok:false};
      const dbg = g.getExtension('WEBGL_debug_renderer_info');
      return {
        ok:true,
        vendor: g.getParameter(g.VENDOR),
        renderer: g.getParameter(g.RENDERER),
        unmaskedVendor: dbg ? g.getParameter(dbg.UNMASKED_VENDOR_WEBGL) : null,
        unmaskedRenderer: dbg ? g.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : null,
        version: g.getParameter(g.VERSION),
        sl: g.getParameter(g.SHADING_LANGUAGE_VERSION),
        maxTex: g.getParameter(g.MAX_TEXTURE_SIZE),
        extCount: (g.getSupportedExtensions()||[]).length,
      };
    } catch(e){ return {ok:false, err:''+e}; }
  }
  out.webgl = gl('webgl');
  out.webgl2 = gl('webgl2');
  try { out.hasGPU = !!navigator.gpu; if(navigator.gpu){ const a=await navigator.gpu.requestAdapter(); out.gpuAdapter = !!a; } } catch(e){ out.gpuErr=''+e; }
  try { const AC=window.AudioContext||window.webkitAudioContext; const ac=new AC(); out.audio={sampleRate:ac.sampleRate, state:ac.state, maxChannels:ac.destination.maxChannelCount}; if(ac.close) ac.close(); } catch(e){ out.audioErr=''+e; }
  try { const cn=navigator.connection; out.connection = cn ? {effectiveType:cn.effectiveType, rtt:cn.rtt, downlink:cn.downlink, saveData:cn.saveData} : null; } catch(e){}
  try { const d = await navigator.mediaDevices.enumerateDevices(); out.mediaDevices = d.map(x=>x.kind); } catch(e){ out.mediaErr=''+e; }
  out.hasBattery = typeof navigator.getBattery;
  try { out.tz = Intl.DateTimeFormat().resolvedOptions().timeZone; out.locale = Intl.DateTimeFormat().resolvedOptions().locale; } catch(e){}
  out.fnToStringNative = (''+Function.prototype.toString).includes('native code');
  return JSON.stringify(out);
})()"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = matches!(std::env::var("HEADLESS").ok().as_deref(), Some("1"));
    // FULLCH=1:开启无头高熵 Client Hints 补全(验证补环境后 fullVersionList 等不再为空)。
    let full_ch = matches!(std::env::var("FULLCH").ok().as_deref(), Some("1"));
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com".to_string());
    let browser = Browser::launch(
        BrowserOptions::new()
            .headless(headless)
            .full_ua_metadata(full_ch)
            .window_size(1280, 800),
    )
    .await?;
    let tab = browser.latest_tab().await?;
    tab.get(&url).await?;
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let v = tab.run_js(PROBE).await?;
    // run_js 返回 JSON 字符串;直接打印(便于重定向落盘 diff)。
    println!("{}", v.as_str().unwrap_or("null"));
    browser.quit().await?;
    Ok(())
}
