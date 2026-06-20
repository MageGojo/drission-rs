//! 易盾点选**样本采集**(为阶段3 自训模型攒数据):反复弹挑战 → 截图 → `Det` 切字框落盘 + OCR 猜测标签。
//!
//! 运行:`YIDUN_N=40 YIDUN_DUMP=./yidun_samples cargo run --example yidun_collect --features cdp,ocr`
//!
//! - 默认**无头**(快);`HL=0` 可视化观察。
//! - 产出:`crop_*.png`(单字框)+ `cap_*.png`(整图带语境)+ `samples.csv`(file,tips,guess)。
//! - 标注:人工把 `guess` 改成真值,即可按 dddd_trainer 格式组织训练(见 `docs/OCR模型热替换.md`)。

use std::io::Write;
use std::time::Duration;

use drission::ocr::ClickWord;
use drission::prelude::*;

const URL: &str = "https://dun.163.com/trial/picture-click";

// 触发验证码:点页面里像“在线体验/立即体验/验证”的候选,载入 demo 控件。
const TRIGGER_JS: &str = r#"(() => {
  const want = /点击按钮进行验证|开始验证|立即体验|在线体验|点击验证|验证码|体验|验证/;
  const els = [...document.querySelectorAll('button,a,div,span,input')].filter(e => e.offsetParent !== null);
  for (const e of els) { const t = (e.innerText||e.value||'').trim(); if (want.test(t) && t.length <= 16) { e.click(); return 'clicked:'+t; } }
  return 'no-trigger';
})()"#;

// 验证码图是否在显示(rect 宽 > 2)。
const PRESENT_JS: &str = r#"(()=>{const e=document.querySelector('.yidun_bgimg');if(!e)return 0;const r=e.getBoundingClientRect();return r.width>2?1:0;})()"#;

// 提示文本(要点哪些字)。
const TIPS_JS: &str = r#"(()=>{const e=document.querySelector('.yidun_tips__answer,.yidun_tips,.yidun_tips__text');return e?e.innerText.trim():'';})()"#;

// 换图(刷新键 JS 兜底)。
const REFRESH_JS: &str = r#"(()=>{for(const s of ['.yidun_refresh','.yidun-refresh','.yidun_panel-refresh']){const e=document.querySelector(s);if(e&&e.offsetParent!==null){e.click();return 'refreshed:'+s;}}return 'no-refresh';})()"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = !matches!(
        std::env::var("HL").ok().as_deref(),
        Some("0") | Some("false")
    );
    let n: u32 = std::env::var("YIDUN_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);
    let dir = std::env::var("YIDUN_DUMP").unwrap_or_else(|_| "./yidun_samples".into());
    std::fs::create_dir_all(&dir).ok();

    println!("[collect] 加载 det+ocr 模型…");
    let cw = ClickWord::new().await?;
    let browser = ChromiumBrowser::launch(
        ChromiumOptions::new()
            .headless(headless)
            .window_size(1200, 900),
    )
    .await?;
    let tab = browser.new_tab(Some(URL)).await?;
    tokio::time::sleep(Duration::from_secs(5)).await;
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

    let mut csv = std::fs::File::create(format!("{dir}/samples.csv"))?;
    writeln!(csv, "file,tips,guess").ok();
    let (mut shots, mut total) = (0u32, 0u32);

    for i in 0..n {
        // 确保有挑战:没有就重新触发(挑战会超时关闭)。
        let present = tab
            .run_js(PRESENT_JS)
            .await
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
            > 0.5;
        if !present {
            let _ = tab.run_js(TRIGGER_JS).await;
            tokio::time::sleep(Duration::from_millis(800)).await;
            for s in ["css:.yidun_control", "css:.yidun"] {
                if let Ok(e) = tab.ele(s).await
                    && e.click().await.is_ok()
                {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        // 截验证码图(取不到就跳过,下轮重触发)。
        let cap = match tab.ele("css:.yidun_bgimg").await {
            Ok(e) => e.screenshot_bytes().await.ok(),
            Err(_) => None,
        };
        let Some(cap) = cap.filter(|b| b.len() > 2000) else {
            println!("[collect] {i}: 无验证码,重触发");
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        };

        let tips = tab
            .run_js(TIPS_JS)
            .await
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        std::fs::write(format!("{dir}/cap_{i}.png"), &cap).ok();
        shots += 1;

        // 切字框(crops)+ 逐框 OCR 猜测(chars,同检测序),写盘 + CSV(供人工纠正标签)。
        let crops = cw.crops(&cap).unwrap_or_default();
        let guesses = cw.chars(&cap).unwrap_or_default();
        for (j, (_b, png)) in crops.iter().enumerate() {
            let f = format!("crop_{i}_{j}.png");
            std::fs::write(format!("{dir}/{f}"), png).ok();
            let g = guesses.get(j).map(|(_, s)| s.as_str()).unwrap_or("");
            writeln!(csv, "{f},{},{g}", tips.replace(',', ";")).ok();
            total += 1;
        }
        println!("[collect] {i}: tips「{tips}」 {} 字框", crops.len());

        // 换下一题。
        let mut refreshed = false;
        if let Ok(e) = tab.ele("css:.yidun_refresh").await
            && e.click().await.is_ok()
        {
            refreshed = true;
        }
        if !refreshed {
            let _ = tab.run_js(REFRESH_JS).await;
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }

    println!(
        "[collect] 完成:{shots} 张图、{total} 个字框 → {dir}/(samples.csv 待人工纠正 guess→真值)"
    );
    browser.quit().await?;
    Ok(())
}
