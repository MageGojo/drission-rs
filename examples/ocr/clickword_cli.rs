//! ClickWord 点选 OCR **命令行工具**:给一张干净文字底图 + 目标字串,输出**按目标顺序**的点击坐标 JSON。
//! 供「易盾补环境」点选注入用(probe 下载 bg → 调本工具拿坐标 → 注入点击),也是 drission `ClickWord`
//! 能力的最小无浏览器入口(纯字节 det+OCR)。
//!
//! 运行:`cargo run --example clickword_cli --features ocr -- <图片路径> <目标字,如 税实企>`
//! 输出(stdout 最后一行):`[[x,y],[x,y],...]`(目标顺序;未命中的字给 [0,0])。
//! 模型:首次自动下载 ddddocr `common_det.onnx`/`common.onnx` 到缓存(需网),之后离线。

use drission::ocr::ClickWord;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_default();
    let targets_s = args.next().unwrap_or_default();
    if path.is_empty() {
        eprintln!("用法: clickword_cli <图片路径> <目标字串>");
        println!("[]");
        return Ok(());
    }
    let targets: Vec<String> = targets_s
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_string())
        .collect();
    let bytes = std::fs::read(&path)?;
    let cw = ClickWord::new().await?;

    // 「只输出检测框」模式(CLICKWORD_BOXES=1):给人机协作用——core dump 底图后调它拿所有候选字框中心,
    // 供 AI 看图后把 front 字逐一指派到框(det 精确定位 + AI 识别,各取所长)。输出最后一行:
    // [[idx,cx,cy,x1,y1,x2,y2,score],...](按检测置信度降序;坐标为原图像素)。
    if std::env::var("CLICKWORD_BOXES").is_ok() {
        let crops = cw.crops(&bytes).unwrap_or_default();
        let mut rows: Vec<[i64; 8]> = Vec::new();
        for (i, (b, _png)) in crops.iter().enumerate() {
            let (cx, cy) = b.center();
            rows.push([
                i as i64, cx as i64, cy as i64, b.x1 as i64, b.y1 as i64, b.x2 as i64, b.y2 as i64,
                (b.score * 100.0) as i64,
            ]);
        }
        for r in &rows {
            eprintln!(
                "  #{} 中心({},{}) 框({},{}-{},{}) score={}",
                r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7]
            );
        }
        println!("{}", serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into()));
        return Ok(());
    }

    // 调试:DEBUG=1 时 dump 所有检测框 + 每个框对各目标字的 OCR 亲和度 / 模板相似度矩阵(定位误配根因)。
    if std::env::var("DEBUG").is_ok() {
        let chars: Vec<char> = targets.iter().filter_map(|s| s.chars().next()).collect();
        eprintln!("[debug] 目标字: {chars:?}  样本库={}", if cw.bank.is_some() { "有" } else { "无" });
        if let Ok(crops) = cw.crops(&bytes) {
            eprintln!("[debug] 检测到 {} 个框(框 x1,y1-x2,y2 score | 各目标 aff/tpl):", crops.len());
            let dump_dir = std::env::var("DEBUG_CROPS").ok();
            for (i, (b, png)) in crops.iter().enumerate() {
                if let Some(d) = &dump_dir {
                    let _ = std::fs::write(format!("{d}/box_{i}.png"), png);
                }
                let aff = cw.ocr.char_affinity(png, &chars).unwrap_or_default();
                let mut parts = Vec::new();
                for (j, ch) in chars.iter().enumerate() {
                    let a = aff.get(j).copied().unwrap_or(0.0);
                    let t = cw.bank.as_ref().and_then(|bk| bk.similarity_image(png, *ch).ok()).unwrap_or(0.0);
                    parts.push(format!("{ch}:aff{a:.2}/tpl{t:.2}"));
                }
                eprintln!("  #{i} ({},{}-{},{}) c{:.2} | {}", b.x1, b.y1, b.x2, b.y2, b.score, parts.join("  "));
            }
        }
    }

    // 诊断:逐字命中(亲和度)到 stderr。
    if let Ok(hits) = cw.solve(&bytes, &targets) {
        for h in &hits {
            eprintln!("  「{}」 aff={:.2} 点({},{})", h.target, h.affinity, h.point.0, h.point.1);
        }
    }
    // 按目标顺序的点击中心点(原图像素)。
    let points = cw.points_for(&bytes, &targets).unwrap_or_default();
    let pts: Vec<[u32; 2]> = points.iter().map(|&(x, y)| [x, y]).collect();
    println!("{}", serde_json::to_string(&pts).unwrap_or_else(|_| "[]".into()));
    Ok(())
}
