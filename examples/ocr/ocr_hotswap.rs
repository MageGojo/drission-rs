//! OCR / 检测模型**热替换**(自训模型上线):`feature = "ocr"`,后端无关。
//!
//! 演示:① 加载默认 ddddocr 模型;② **原地热替换**模型 + 自定义字符集(无需新建对象、无需重启);
//! ③ 用字符集文件加载;④ 让现有 `yidun_click` 直接吃自训模型(**纯环境变量,零改码**)。
//!
//! 运行:
//!   - `cargo run --example ocr_hotswap --features ocr`
//!   - 识别一张图:`OCR_IMG=captcha.png cargo run --example ocr_hotswap --features ocr`
//!   - 上自训模型:`DRISSION_OCR_MODEL=yidun.onnx DRISSION_OCR_CHARSET=yidun.json OCR_IMG=cap.png \`
//!     `cargo run --example ocr_hotswap --features ocr`

use std::path::Path;

use drission::ocr::{Ocr, load_charset_file};

#[tokio::main]
async fn main() -> drission::Result<()> {
    let img = std::env::var("OCR_IMG").ok();

    // ① 默认 ddddocr 模型(8210 字)。
    println!("[hotswap] 加载默认模型…");
    let mut ocr = Ocr::new().await?;
    println!("[hotswap] 默认字符集 = {} 字", ocr.charset_len());
    if let Some(p) = &img {
        match std::fs::read(p) {
            Ok(b) => println!("[hotswap] 默认模型识别 {p} =「{}」", ocr.recognize(&b)?),
            Err(e) => println!("[hotswap] 读不到 OCR_IMG={p}: {e}"),
        }
    }

    // ② 原地热替换:模型(此处仍用默认缓存模型演示机制)+ 自定义字符集 → 即刻生效,无需新建 Ocr。
    let model_path = Ocr::default_model_path().await?;
    let demo_charset: Vec<String> = ["", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    ocr.set_model_with_charset(&model_path, demo_charset)?;
    println!(
        "[hotswap] 原地热替换后字符集 = {} 字(演示:换成纯数字集)",
        ocr.charset_len()
    );

    // ③ 用字符集文件加载(支持 {"charset":[...]} / [...] / 每行一字)。
    let cs_file = std::env::temp_dir().join("drission_demo_charset.json");
    std::fs::write(&cs_file, r#"{"charset":["","验","证","码"]}"#)?;
    let ocr2 = Ocr::from_files(&model_path, &cs_file)?;
    println!(
        "[hotswap] from_files 加载字符集 = {} 字",
        ocr2.charset_len()
    );
    let _ = std::fs::remove_file(&cs_file);

    // ④ 自训模型上线(env 指定):有 DRISSION_OCR_MODEL 就热替换成它(可配套 DRISSION_OCR_CHARSET)。
    if let Ok(m) = std::env::var("DRISSION_OCR_MODEL") {
        match std::env::var("DRISSION_OCR_CHARSET") {
            Ok(cs) => {
                ocr.set_model_with_charset(Path::new(&m), load_charset_file(Path::new(&cs))?)?;
                println!(
                    "[hotswap] 上自训模型 {m} + 字符集 {cs}({} 字)",
                    ocr.charset_len()
                );
            }
            Err(_) => {
                ocr.set_model(Path::new(&m))?;
                println!(
                    "[hotswap] 上自训模型 {m}(沿用内置字符集 {} 字)",
                    ocr.charset_len()
                );
            }
        }
        if let Some(p) = &img
            && let Ok(b) = std::fs::read(p)
        {
            println!("[hotswap] 自训模型识别 {p} =「{}」", ocr.recognize(&b)?);
        }
    } else {
        println!(
            "[hotswap] (设 DRISSION_OCR_MODEL=自训.onnx [+ DRISSION_OCR_CHARSET=自训.json] 即热替换为自训模型)"
        );
    }

    println!("\n[hotswap] 让现有点选示例直接吃自训模型(零改码):");
    println!("  DRISSION_DET_MODEL=yidun_det.onnx \\");
    println!("  DRISSION_OCR_MODEL=yidun_ocr.onnx DRISSION_OCR_CHARSET=yidun_charset.json \\");
    println!("  cargo run --example yidun_click --features cdp,ocr");
    Ok(())
}
