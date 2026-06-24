//! 无头 Client Hints 诊断:对比 `mask_ua` 开/关时,无头 Chrome 的 UA 与**高熵 Client Hints**。
//! 验证假设:无头 + `mask_ua` 时库加的 `--user-agent` 启动参数把
//! `getHighEntropyValues(['fullVersionList',...])` 的高熵字段清空(留下空 `fullVersionList`)。
//!
//! 运行(用 `DRISSION_CHROME` 指定原版 Chrome):
//!   `MASK=1 cargo run --example hl_ch`  默认:带 `--user-agent`(看高熵是否被清空)
//!   `MASK=0 cargo run --example hl_ch`  不带:看原生高熵 CH,以及 UA 是否含 `Headless`

use drission::cdp::{ChromiumBrowser, ChromiumOptions};

const PROBE: &str = r#"(async () => {
  const out = {};
  out.userAgent = navigator.userAgent;
  out.headlessInUA = navigator.userAgent.includes('Headless');
  try {
    out.brands = navigator.userAgentData ? navigator.userAgentData.brands : null;
    if (navigator.userAgentData) {
      out.high = await navigator.userAgentData.getHighEntropyValues(
        ['architecture','bitness','model','platformVersion','uaFullVersion','fullVersionList','wow64']
      );
    }
  } catch (e) { out.err = '' + e; }
  return JSON.stringify(out);
})()"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let mask = std::env::var("MASK").map(|v| v != "0").unwrap_or(true);
    println!("[*] headless + mask_ua={mask}");
    let mut opts = ChromiumOptions::new().headless(true).mask_ua(mask);
    if let Some(p) = std::env::var_os("DRISSION_CHROME") {
        opts = opts.binary_path(std::path::PathBuf::from(p));
    }
    let browser = ChromiumBrowser::launch(opts).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;
    tab.get("https://example.com").await?;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let v = tab.run_js(PROBE).await?;
    println!("{}", v.as_str().unwrap_or("null"));
    browser.quit().await?;
    Ok(())
}
