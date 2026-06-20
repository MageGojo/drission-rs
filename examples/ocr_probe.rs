//! 数字验证码取样:打开 apizero 登录页,定位验证码 `<img>`(xpath),浏览器级截图抓 N 张样本,
//! 记录其 `src` 形态(data: / URL)。供离线分析扭曲程度、定识别方案。
//! 运行:`HL=0 N=12 cargo run --example ocr_probe`(默认无头 12 张)。

use std::time::Duration;

use drission::prelude::*;
use tokio::time::sleep;

const URL: &str = "https://apizero.cn/login";
const XP: &str = "xpath:/html/body/main/div/main/div/form/div[3]/button/img";

/// 极简 base64 解码(data:URL 取干净原图,避开按钮刷新图标对截图的污染)。
fn b64(s: &str) -> Vec<u8> {
    fn v(b: u8) -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as u32),
            b'a'..=b'z' => Some((b - b'a' + 26) as u32),
            b'0'..=b'9' => Some((b - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let (mut bits, mut n, mut o) = (0u32, 0u32, Vec::new());
    for &b in s.as_bytes() {
        if b == b'=' {
            break;
        }
        let Some(x) = v(b) else { continue };
        bits = (bits << 6) | x;
        n += 6;
        if n >= 8 {
            n -= 8;
            o.push((bits >> n) as u8);
        }
    }
    o
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let n: u32 = std::env::var("N").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ocr-diag");
    std::fs::create_dir_all(&out).ok();

    let browser = Browser::launch(BrowserOptions::new().headless(headless)).await?;
    let tab = browser.latest_tab().await?;
    tab.apply_pointer_stealth().await?;
    println!("[*] 打开 {URL}");
    tab.get(URL).await?;
    sleep(Duration::from_secs(3)).await;

    // 等验证码 img 出现。
    if tab.wait().ele_displayed(XP, Some(Duration::from_secs(10))).await.is_err() {
        println!("[!] 验证码 img 未出现(xpath 可能变了)");
        browser.quit().await?;
        return Ok(());
    }

    for k in 1..=n {
        let img = match tab.ele(XP).await {
            Ok(e) => e,
            Err(e) => {
                println!("[!] #{k} 找不到验证码 img: {e}");
                break;
            }
        };
        // 取完整 data:URL 原图(干净,无按钮刷新图标叠加)。
        let src = tab
            .run_js(&format!(
                "(function(){{var e=document.evaluate({xp},document,null,9,null).singleNodeValue; return e?(e.currentSrc||e.src||''):'';}})()",
                xp = serde_json::json!(XP.trim_start_matches("xpath:"))
            ))
            .await
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        if let Some(idx) = src.find("base64,") {
            let bytes = b64(&src[idx + 7..]);
            std::fs::write(out.join(format!("cap_{k:02}.png")), &bytes).ok();
            println!("[*] #{k:02} 原图 {} bytes  src 前缀 {}", bytes.len(), &src[..src.len().min(40)]);
        } else {
            // 退回截图(若不是 data:URL)。
            let _ = img.get_screenshot(out.join(format!("cap_{k:02}.png"))).await;
            println!("[*] #{k:02} 截图(非 data:URL)  src 前缀 {}", &src[..src.len().min(40)]);
        }
        // 刷新:点验证码(在 button 里,通常点击即换一张)。检测是否误触发导航。
        let url_before = tab.url().await.unwrap_or_default();
        let _ = img.click().await;
        sleep(Duration::from_millis(1300)).await;
        let url_after = tab.url().await.unwrap_or_default();
        if url_before != url_after {
            println!("[!] 点击后页面跳转({url_before} -> {url_after}),改用重置 src 刷新");
            tab.get(URL).await?;
            sleep(Duration::from_secs(2)).await;
        }
    }
    println!("[*] 样本已存:{}/cap_*.png", out.display());
    if !headless {
        sleep(Duration::from_secs(2)).await;
    }
    browser.quit().await?;
    Ok(())
}
