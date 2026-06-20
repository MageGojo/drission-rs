//! 目标检测可行性探针:确认 tract 能**加载并运行** ddddocr `common_det.onnx`(YOLOX)。
//! 用 CDP 截一张图喂给 [`Det`](不依赖 image crate)。
//! 运行:`cargo run --example det_probe --features ocr`(可传 URL:`-- https://...`)。

use std::time::Duration;

use drission::ocr::Det;
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    println!("[probe] 加载检测模型(首次下载 common_det.onnx 到缓存)…");
    let det = Det::new().await?;
    println!("[probe] ✓ tract 成功加载 + 优化检测模型");

    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com".to_string());
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(true)).await?;
    let tab = browser.new_tab(Some(&url)).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let png = tab.screenshot_bytes().await?;
    println!("[probe] 截图 {} bytes,运行检测…", png.len());
    let boxes = det.detect(&png)?;
    println!("[probe] ✓ 检测运行成功,框数 = {}", boxes.len());
    for (i, b) in boxes.iter().take(8).enumerate() {
        println!(
            "  #{i} ({},{})-({},{}) score={:.3}",
            b.x1, b.y1, b.x2, b.y2, b.score
        );
    }
    browser.quit().await?;
    println!("[probe] DONE —— tract 可运行 ddddocr 检测模型 ✓");
    Ok(())
}
