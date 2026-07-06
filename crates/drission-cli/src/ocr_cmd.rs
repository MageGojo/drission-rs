#[cfg(feature = "ocr")]
use std::path::Path;

#[cfg(feature = "ocr")]
use anyhow::Result;

#[cfg(feature = "ocr")]
use crate::protocol::JsonResponse;

#[cfg(feature = "ocr")]
pub async fn clickword(image: &Path, targets: &str) -> Result<JsonResponse> {
    let bytes = tokio::fs::read(image).await?;
    let targets: Vec<String> = targets
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .map(|ch| ch.to_string())
        .collect();
    let solver = drission::ocr::ClickWord::new().await?;
    let hits = solver.solve(&bytes, &targets)?;
    let points: Vec<[u32; 2]> = hits.iter().map(|hit| [hit.point.0, hit.point.1]).collect();
    let hits_json: Vec<_> = hits
        .iter()
        .map(|hit| {
            serde_json::json!({
                "target": hit.target.to_string(),
                "point": [hit.point.0, hit.point.1],
                "bbox": {
                    "x1": hit.bbox.x1,
                    "y1": hit.bbox.y1,
                    "x2": hit.bbox.x2,
                    "y2": hit.bbox.y2,
                    "score": hit.bbox.score,
                },
                "affinity": hit.affinity,
                "template": hit.template,
            })
        })
        .collect();
    Ok(JsonResponse::ok(serde_json::json!({
        "image": image,
        "targets": targets,
        "points": points,
        "hits": hits_json,
    })))
}
