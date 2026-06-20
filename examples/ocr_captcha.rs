//! 验证码 OCR 端到端 demo:库能力 [`Tab::ocr_image`](drission::prelude::Tab::ocr_image)
//! (定位元素 → 取图 → ddddocr 模型纯 Rust 推理 → 文本)。需 `--features ocr`。
//!
//! 目标:apizero 登录页的 4 位字母数字验证码。逐张换图识别打印(不登录,只验证识别)。
//! **首次运行会下载 ~54MB 模型**到缓存(之后复用;`DRISSION_OCR_MODEL=本地.onnx` 可跳过)。
//!
//! 运行:`HL=0 N=8 cargo run --example ocr_captcha --no-default-features --features camoufox,ocr`

use std::time::Duration;

use drission::prelude::*;
use tokio::time::sleep;

const URL: &str = "https://apizero.cn/login";
const XP: &str = "xpath:/html/body/main/div/main/div/form/div[3]/button/img";

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let n: u32 = std::env::var("N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);

    let browser = Browser::launch(BrowserOptions::new().headless(headless)).await?;
    let tab = browser.latest_tab().await?;
    tab.apply_pointer_stealth().await?;
    println!("[*] 打开 {URL}");
    tab.get(URL).await?;
    sleep(Duration::from_secs(3)).await;
    if tab
        .wait()
        .ele_displayed(XP, Some(Duration::from_secs(10)))
        .await
        .is_err()
    {
        println!("[!] 验证码 img 未出现");
        browser.quit().await?;
        return Ok(());
    }
    println!("[*] 首次识别会下载 ~54MB 模型(之后复用)...");

    let mut ok = 0u32;
    for k in 1..=n {
        match tab.ocr_image(XP).await {
            Ok(code) => {
                println!(
                    "[*] #{k}:验证码 = {:?}  (大写 {:?})",
                    code,
                    code.to_uppercase()
                );
                if code.chars().count() == 4 {
                    ok += 1;
                }
            }
            Err(e) => println!("[!] #{k}:{e}"),
        }
        // 换一张:点验证码(在 button 里,点击即刷新)。
        if let Ok(el) = tab.ele(XP).await {
            let _ = el.click().await;
        }
        sleep(Duration::from_millis(1300)).await;
    }
    println!("\n==== OCR 完成:{ok}/{n} 张识别出 4 位(大小写不敏感场景可直接用)====");
    if !headless {
        sleep(Duration::from_secs(2)).await;
    }
    browser.quit().await?;
    Ok(())
}
