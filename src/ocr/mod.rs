//! 验证码 **OCR**(字符型验证码识别)。`feature = "ocr"`。
//!
//! 用 **ddddocr 预训练模型**(对常见 4~6 位字母/数字扭曲验证码开箱即用)+ **纯 Rust 推理引擎
//! [`tract`](https://github.com/sonos/tract)**(不引原生 onnxruntime,跨平台干净)。流水线对齐 ddddocr:
//! 解码图 → 灰度 → 等比缩放到高 64 → 归一化 `(p/255-0.5)/0.5` → CNN-LSTM 推理 → CTC 贪心解码。
//!
//! - 模型(beta `common.onnx`,~54MB)**首次使用自动下载**到缓存目录(可用 `DRISSION_OCR_MODEL`
//!   指定本地路径、`DRISSION_OCR_MODEL_URL` 换下载源);字符集 8210 字内置于库。
//! - 核心:[`Ocr::new`](Ocr::new)(异步,确保模型就绪)→ [`Ocr::recognize`](Ocr::recognize)(同步,
//!   传 PNG/JPEG 字节 → 文本)。便捷:[`Tab::ocr_image`](crate::browser::Tab::ocr_image)(定位元素 →
//!   取图(`<img>` data:URL 直接解码,否则元素截图)→ 识别)。
//!
//! ```no_run
//! # use drission::prelude::*;
//! # async fn f(tab: &Tab) -> drission::Result<()> {
//! let code = tab.ocr_image("xpath://form//button/img").await?;   // 一步:定位+识别
//! println!("验证码 = {code}");
//! # Ok(()) }
//! ```
//!
//! > 注:大小写——多数字母在验证码里上下形同(S/s、C/c、W/w…),模型可能输出小写;验证码登录通常
//! > **大小写不敏感**,如需可 `.to_uppercase()`。实测 apizero 登录 4 位字母数字 16/16 命中。

use std::path::{Path, PathBuf};

use tract_onnx::prelude::*;

use crate::browser::Tab;
use crate::util::base64_decode;
use crate::{Error, Result};

/// 内置 ddddocr 字符集(beta `common.json`,8210 字;首项 "" = CTC blank)。
const CHARSET_JSON: &str = include_str!("assets/charset.json");
/// 默认模型下载源(beta `common.onnx`,标准 LSTM,tract 可纯 Rust 推理)。
const MODEL_URL: &str = "https://raw.githubusercontent.com/86maid/ddddocr/master/model/common.onnx";

fn terr(e: impl std::fmt::Display) -> Error {
    Error::msg(format!("OCR: {e}"))
}

/// 验证码 OCR 识别器(持模型 + 字符集,可复用)。
pub struct Ocr {
    model: InferenceModel,
    charset: Vec<String>,
}

impl Ocr {
    /// 加载默认 ddddocr 模型。**首次使用会下载 ~54MB 模型**到缓存(之后复用);
    /// `DRISSION_OCR_MODEL=本地.onnx` 可跳过下载。
    pub async fn new() -> Result<Self> {
        let path = ensure_model().await?;
        Self::from_model_path(&path)
    }

    /// 用本地 onnx 模型路径加载(字符集用库内置 ddddocr 字符集)。
    pub fn from_model_path(onnx: &Path) -> Result<Self> {
        let charset = parse_charset(CHARSET_JSON)?;
        let model = tract_onnx::onnx().model_for_path(onnx).map_err(terr)?;
        Ok(Self { model, charset })
    }

    /// 识别一张验证码图(PNG/JPEG 等字节)→ 文本。
    pub fn recognize(&self, image: &[u8]) -> Result<String> {
        let (data, w) = preprocess(image)?;
        let runnable = self
            .model
            .clone()
            .with_input_fact(
                0,
                InferenceFact::dt_shape(f32::datum_type(), tvec![1, 1, 64, w]),
            )
            .map_err(terr)?
            .into_optimized()
            .map_err(terr)?
            .into_runnable()
            .map_err(terr)?;
        let input =
            tract_ndarray::Array4::<f32>::from_shape_vec((1, 1, 64, w), data).map_err(terr)?;
        let out = runnable
            .run(tvec![Tensor::from(input).into()])
            .map_err(terr)?;
        let t = out[0].clone().into_tensor();
        let view = t.to_plain_array_view::<f32>().map_err(terr)?;
        Ok(ctc_decode(&view, &self.charset))
    }
}

/// 解析字符集 JSON(`{"charset":[...]}`)。
fn parse_charset(s: &str) -> Result<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(s).map_err(terr)?;
    let arr = v["charset"]
        .as_array()
        .ok_or_else(|| Error::msg("OCR: charset 缺失"))?;
    Ok(arr
        .iter()
        .map(|x| x.as_str().unwrap_or("").to_string())
        .collect())
}

/// 预处理:解码 → 灰度 → 等比缩放到高 64 → 归一化。返回 `([f32; 64*w] 行主序, w)`。
fn preprocess(bytes: &[u8]) -> Result<(Vec<f32>, usize)> {
    let img = image::load_from_memory(bytes).map_err(terr)?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Err(Error::msg("OCR: 空图"));
    }
    let new_w = ((w as f32) * 64.0 / (h as f32)).round().max(1.0) as usize;
    let luma = img
        .resize_exact(new_w as u32, 64, image::imageops::FilterType::Lanczos3)
        .to_luma8();
    let mut data = Vec::with_capacity(64 * new_w);
    for y in 0..64u32 {
        for x in 0..new_w as u32 {
            data.push((luma.get_pixel(x, y)[0] as f32 / 255.0 - 0.5) / 0.5);
        }
    }
    Ok((data, new_w))
}

/// CTC 贪心解码:输出形如 `[T,1,C]`/`[1,T,C]`,C=字符集长度。每时间步取 argmax,折叠连续相同、去 blank(0)。
fn ctc_decode(view: &tract_ndarray::ArrayViewD<f32>, charset: &[String]) -> String {
    let shape = view.shape();
    let c = charset.len();
    let cls_axis = shape
        .iter()
        .position(|&d| d == c)
        .unwrap_or(shape.len() - 1);
    let t_axis = (0..shape.len())
        .find(|&a| a != cls_axis && shape[a] > 1)
        .unwrap_or(0);
    let tn = shape[t_axis];
    let mut out = String::new();
    let mut prev = usize::MAX;
    let mut idx = vec![0usize; shape.len()];
    for t in 0..tn {
        let mut best = 0usize;
        let mut bestv = f32::MIN;
        idx[t_axis] = t;
        for k in 0..c {
            idx[cls_axis] = k;
            let v = view[idx.as_slice()];
            if v > bestv {
                bestv = v;
                best = k;
            }
        }
        if best != 0
            && best != prev
            && let Some(ch) = charset.get(best)
        {
            out.push_str(ch);
        }
        prev = best;
    }
    out
}

/// 确保模型文件就绪:`DRISSION_OCR_MODEL` 指定本地路径优先;否则缓存目录,缺则下载。
async fn ensure_model() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("DRISSION_OCR_MODEL") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
        return Err(Error::msg(format!(
            "OCR: DRISSION_OCR_MODEL 路径不存在: {}",
            p.display()
        )));
    }
    let dir = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("drission")
        .join("ocr");
    std::fs::create_dir_all(&dir).map_err(terr)?;
    let path = dir.join("ddddocr_common.onnx");
    // 已存且 > 1MB(避免半截下载)即复用。
    if path.exists()
        && std::fs::metadata(&path)
            .map(|m| m.len() > 1_000_000)
            .unwrap_or(false)
    {
        return Ok(path);
    }
    let url = std::env::var("DRISSION_OCR_MODEL_URL").unwrap_or_else(|_| MODEL_URL.to_string());
    tracing::info!(target: "drission::ocr", "下载 OCR 模型(~54MB,仅首次): {url}");
    let bytes = reqwest::get(&url)
        .await
        .map_err(terr)?
        .bytes()
        .await
        .map_err(terr)?;
    if bytes.len() < 1_000_000 {
        return Err(Error::msg(format!(
            "OCR: 模型下载异常({} bytes)",
            bytes.len()
        )));
    }
    // 先写临时再 rename,避免并发/中断留半截。
    let tmp = path.with_extension("onnx.part");
    std::fs::write(&tmp, &bytes).map_err(terr)?;
    std::fs::rename(&tmp, &path).map_err(terr)?;
    Ok(path)
}

/// 进程内共享的默认 OCR 实例(懒加载,首次触发下载 + 建模)。
static DEFAULT_OCR: tokio::sync::OnceCell<Ocr> = tokio::sync::OnceCell::const_new();

impl Tab {
    /// **一步识别**页面里某元素的验证码图:定位 `selector`(`css:`/`xpath:` 前缀,同 [`Tab::ele`])→
    /// 取图(`<img>` 的 `data:` URL 直接解码,否则元素截图)→ ddddocr 模型识别 → 文本。
    /// 首次调用会懒加载默认模型(可能下载 ~54MB)。
    pub async fn ocr_image(&self, selector: &str) -> Result<String> {
        let ocr = DEFAULT_OCR.get_or_try_init(Ocr::new).await?;
        let bytes = self.fetch_image_bytes(selector).await?;
        ocr.recognize(&bytes)
    }

    /// 取元素的图字节:优先 `<img>` 的 `src`(`data:base64` 直接解码),否则元素浏览器级截图。
    async fn fetch_image_bytes(&self, selector: &str) -> Result<Vec<u8>> {
        let el = self.ele(selector).await?;
        if let Ok(src) = el.run_js("return node.currentSrc||node.src||'';").await
            && let Some(s) = src.as_str()
            && let Some(i) = s.find("base64,")
            && let Some(b) = base64_decode(&s[i + 7..])
            && !b.is_empty()
        {
            return Ok(b);
        }
        el.screenshot_bytes().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charset_loads_and_blank_first() {
        let cs = parse_charset(CHARSET_JSON).unwrap();
        assert!(cs.len() > 1000);
        assert_eq!(cs[0], ""); // CTC blank
        assert!(cs.iter().any(|c| c == "5") && cs.iter().any(|c| c == "z"));
    }

    #[test]
    fn ctc_collapses_repeats_and_blanks() {
        // 构造 [T=5, C=4] 的 one-hot logits,charset=["",a,b,c]。
        // 序列:a a blank b b → 解码 "ab"。
        let charset = vec![
            "".to_string(),
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
        ];
        let seq = [1usize, 1, 0, 2, 2];
        let mut arr = tract_ndarray::Array3::<f32>::zeros((seq.len(), 1, charset.len()));
        for (t, &k) in seq.iter().enumerate() {
            arr[[t, 0, k]] = 1.0;
        }
        let dynv = arr.into_dyn();
        assert_eq!(ctc_decode(&dynv.view(), &charset), "ab");
    }
}
