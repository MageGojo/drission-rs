//! Probe whether headless gets a REAL GPU (vs SwiftShader) — reads WebGL UNMASKED_RENDERER_WEBGL.
//! Turnstile's strongest headless tell is a SwiftShader/llvmpipe WebGL renderer; if headless can
//! report a real D3D11/Metal GPU, true headless (no window at all) becomes viable.
//!
//! Run headless (default):  cargo run --example gpu_probe
//! Headed compare:          HL=0 cargo run --example gpu_probe
//! Use bundled CloakBrowser: set DRISSION_CHROME / CHROME_BIN to its chrome.exe.

use std::time::Duration;

use drission::cdp::{ChromiumBrowser, ChromiumOptions};

const WEBGL_JS: &str = r#"(()=>{try{
  const c=document.createElement('canvas');
  const gl=c.getContext('webgl')||c.getContext('experimental-webgl');
  if(!gl) return 'NO_WEBGL';
  const dbg=gl.getExtension('WEBGL_debug_renderer_info');
  const vendor=dbg?gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL):'(no-ext)';
  const renderer=dbg?gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL):'(no-ext)';
  return vendor+' || '+renderer;
}catch(e){return 'ERR:'+e;}})()"#;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> drission::Result<()> {
    let headless = !matches!(std::env::var("HL").ok().as_deref(), Some("0"));
    let exe = std::env::var("DRISSION_CHROME")
        .or_else(|_| std::env::var("CHROME_BIN"))
        .ok();

    let mut opts = ChromiumOptions::new().headless(headless).window_size(1280, 900);
    if let Some(e) = &exe {
        opts = opts.binary_path(std::path::PathBuf::from(e));
    }
    let browser = ChromiumBrowser::launch(opts).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let webgl = tab
        .run_js(WEBGL_JS)
        .await?
        .as_str()
        .unwrap_or("?")
        .to_string();
    // chrome://gpu would be richer, but the WebGL renderer string is the exact signal Turnstile reads.
    println!("GPU_PROBE headless={headless}");
    println!("WEBGL_RENDERER= {webgl}");
    let soft = webgl.to_lowercase();
    let is_soft = soft.contains("swiftshader")
        || soft.contains("llvmpipe")
        || soft.contains("software");
    println!(
        "VERDICT= {}",
        if is_soft { "SOFTWARE (SwiftShader) -> headless detectable" } else { "HARDWARE GPU -> headless viable" }
    );
    browser.quit().await?;
    Ok(())
}
