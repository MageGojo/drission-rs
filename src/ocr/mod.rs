//! 验证码 **OCR**(字符型验证码识别)。`feature = "ocr"`。
//!
//! 用 **ddddocr 预训练模型**(对常见 4~6 位字母/数字扭曲验证码开箱即用)+ **纯 Rust 推理引擎
//! [`tract`](https://github.com/sonos/tract)**(不引原生 onnxruntime,跨平台干净)。流水线对齐 ddddocr:
//! 解码图 → 灰度 → 等比缩放到高 64 → 归一化 `(p/255-0.5)/0.5` → CNN-LSTM 推理 → CTC 贪心解码。
//!
//! - 模型(beta `common.onnx`,~54MB)**首次使用自动下载**到缓存目录(可用 `DRISSION_OCR_MODEL`
//!   指定本地路径、`DRISSION_OCR_MODEL_URL` 换下载源);字符集 8210 字内置于库。
//! - 核心(后端无关):[`Ocr::new`](Ocr::new)(异步,确保模型就绪)→ [`Ocr::recognize`](Ocr::recognize)
//!   (同步,传 PNG/JPEG 字节 → 文本)。
//! - 便捷(**Camoufox 后端**,`--features camoufox`):`Tab::ocr_image`(定位元素 → 取图
//!   (`<img>` data:URL 直接解码,否则元素截图)→ 识别)。
//!
//! ```no_run
//! # async fn f() -> drission::Result<()> {
//! use drission::ocr::Ocr;
//! let ocr = Ocr::new().await?;                       // 首次会下载 ~54MB 模型到缓存
//! let png = std::fs::read("captcha.png")?;
//! println!("验证码 = {}", ocr.recognize(&png)?);
//! # Ok(()) }
//! ```
//!
//! > 注:大小写——多数字母在验证码里上下形同(S/s、C/c、W/w…),模型可能输出小写;验证码登录通常
//! > **大小写不敏感**,如需可 `.to_uppercase()`。实测 apizero 登录 4 位字母数字 16/16 命中。

use std::path::{Path, PathBuf};

use tract_onnx::prelude::*;

#[cfg(feature = "camoufox")]
use crate::browser::Tab;
#[cfg(feature = "camoufox")]
use crate::util::base64_decode;
use crate::{Error, Result};

mod glyph;
pub use glyph::{GlyphMatcher, SampleBank};

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
        let charset = match std::env::var("DRISSION_OCR_CHARSET") {
            Ok(p) => load_charset_file(Path::new(&p))?,
            Err(_) => parse_charset(CHARSET_JSON)?,
        };
        Self::from_model_path_with_charset(&path, charset)
    }

    /// 用本地 onnx 模型路径加载(字符集用库内置 ddddocr 8210 字)。
    pub fn from_model_path(onnx: &Path) -> Result<Self> {
        Self::from_model_path_with_charset(onnx, parse_charset(CHARSET_JSON)?)
    }

    /// 用**自定义模型 + 自定义字符集**加载——**自训模型必用**(dddd_trainer 产出的 onnx 配套自己的字符集)。
    /// 约定:`charset[0]` 必须是 CTC blank(空串 `""`)、`charset.len()` == 模型输出类别数(同 ddddocr);
    /// 首项非空会告警(多半是漏了 blank)。
    pub fn from_model_path_with_charset(onnx: &Path, charset: Vec<String>) -> Result<Self> {
        if charset.len() < 2 {
            return Err(Error::msg("OCR: 字符集过小(至少 blank + 1 字)"));
        }
        if charset.first().map(String::as_str) != Some("") {
            tracing::warn!(target: "drission::ocr",
                "OCR 自定义字符集首项不是 CTC blank(空串);若识别结果全乱,请在 charset[0] 处补一个空串");
        }
        let model = tract_onnx::onnx().model_for_path(onnx).map_err(terr)?;
        Ok(Self { model, charset })
    }

    /// 用**模型文件 + 字符集文件**加载(字符集支持 `{"charset":[...]}` / 纯 JSON 数组 / 每行一字 文本)。
    pub fn from_files(onnx: &Path, charset: &Path) -> Result<Self> {
        Self::from_model_path_with_charset(onnx, load_charset_file(charset)?)
    }

    /// **热替换模型**(保留当前字符集);自训模型与内置字符集兼容时用。无需新建对象,即刻生效。
    pub fn set_model(&mut self, onnx: &Path) -> Result<()> {
        self.model = tract_onnx::onnx().model_for_path(onnx).map_err(terr)?;
        Ok(())
    }

    /// **热替换模型 + 字符集**(自训模型常配套新字符集);无需新建对象,即刻生效。
    pub fn set_model_with_charset(&mut self, onnx: &Path, charset: Vec<String>) -> Result<()> {
        if charset.len() < 2 {
            return Err(Error::msg("OCR: 字符集过小(至少 blank + 1 字)"));
        }
        self.model = tract_onnx::onnx().model_for_path(onnx).map_err(terr)?;
        self.charset = charset;
        Ok(())
    }

    /// 当前字符集大小(含 blank)。
    pub fn charset_len(&self) -> usize {
        self.charset.len()
    }

    /// 默认 ddddocr 模型在缓存中的路径(缺则下载);便于"先拿默认、再 [`set_model`](Self::set_model) 热替换"。
    pub async fn default_model_path() -> Result<PathBuf> {
        ensure_model().await
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

    /// **受约束识别**:给一张(单字)图,返回它是 `chars` 里**每个候选字**的亲和度(0–1,各时间步
    /// softmax 概率取最大)。用于点选——提示已给标准答案字,以"这个框最像哪个目标字"挑框,远比
    /// 全字符集 argmax 鲁棒(艺术体单字 argmax 易误判)。
    pub fn char_affinity(&self, image: &[u8], chars: &[char]) -> Result<Vec<f32>> {
        let (data, w) = preprocess(image)?;
        // 用 into_typed(免每次全图优化,点选要逐框多次调用,优化开销过大会拖慢到验证码超时)。
        let runnable = self
            .model
            .clone()
            .with_input_fact(
                0,
                InferenceFact::dt_shape(f32::datum_type(), tvec![1, 1, 64, w]),
            )
            .map_err(terr)?
            .into_typed()
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
        let shape = view.shape();
        let c = self.charset.len();
        let cls_axis = shape
            .iter()
            .position(|&d| d == c)
            .unwrap_or(shape.len() - 1);
        let t_axis = (0..shape.len())
            .find(|&a| a != cls_axis && shape[a] > 1)
            .unwrap_or(0);
        let tn = shape[t_axis];
        let idxs: Vec<Option<usize>> = chars
            .iter()
            .map(|ch| {
                let s = ch.to_string();
                self.charset.iter().position(|x| x == &s)
            })
            .collect();
        let mut best = vec![0f32; chars.len()];
        let mut idx = vec![0usize; shape.len()];
        for ti in 0..tn {
            idx[t_axis] = ti;
            let mut maxl = f32::MIN;
            for k in 0..c {
                idx[cls_axis] = k;
                let v = view[idx.as_slice()];
                if v > maxl {
                    maxl = v;
                }
            }
            let mut sum = 0f32;
            for k in 0..c {
                idx[cls_axis] = k;
                sum += (view[idx.as_slice()] - maxl).exp();
            }
            if sum <= 0.0 {
                continue;
            }
            for (j, oi) in idxs.iter().enumerate() {
                if let Some(k) = oi {
                    idx[cls_axis] = *k;
                    let p = (view[idx.as_slice()] - maxl).exp() / sum;
                    if p > best[j] {
                        best[j] = p;
                    }
                }
            }
        }
        Ok(best)
    }
}

// ─────────────────────── 目标检测(点选/文字点选验证码用)───────────────────────

/// 检测框(**原图像素**坐标,左上 `(x1,y1)` / 右下 `(x2,y2)`)+ 置信度。
#[derive(Debug, Clone, Copy)]
pub struct BBox {
    pub x1: u32,
    pub y1: u32,
    pub x2: u32,
    pub y2: u32,
    /// 置信度(obj × cls)。
    pub score: f32,
}

impl BBox {
    /// 中心点(用于点击)。
    pub fn center(&self) -> (u32, u32) {
        ((self.x1 + self.x2) / 2, (self.y1 + self.y2) / 2)
    }
    pub fn width(&self) -> u32 {
        self.x2.saturating_sub(self.x1)
    }
    pub fn height(&self) -> u32 {
        self.y2.saturating_sub(self.y1)
    }
}

const DET_SIZE: usize = 416;
const DET_STRIDES: [usize; 3] = [8, 16, 32];
const DET_SCORE_THR: f32 = 0.1;
const DET_NMS_THR: f32 = 0.45;
const DET_MODEL_URL: &str =
    "https://raw.githubusercontent.com/86maid/ddddocr/master/model/common_det.onnx";

/// ddddocr **目标检测器**(`common_det.onnx`,YOLOX)。定位图中字符/图标区域 → [`BBox`] 列表;
/// 配合 [`Ocr::recognize`] 逐框识别即可做**点选/文字点选验证码**(检测 → 逐框 OCR → 按提示顺序匹配)。
///
/// 流水线对齐 ddddocr:416×416 灰边(114)letterbox、**原始 0–255 RGB(不归一化)**、NCHW →
/// YOLOX 解码(`(xy+grid)*stride` / `wh=exp*stride`)→ 分数阈值 0.1 → NMS 0.45 → 还原原图坐标。
pub struct Det {
    model: TypedModel,
}

impl Det {
    /// 加载默认检测模型(首次下载 `common_det.onnx` 到缓存;`DRISSION_DET_MODEL` 指本地、
    /// `DRISSION_DET_MODEL_URL` 换源)。
    pub async fn new() -> Result<Self> {
        let path = ensure_det_model().await?;
        Self::from_model_path(&path)
    }

    /// 用本地 onnx 检测模型路径加载。
    pub fn from_model_path(onnx: &Path) -> Result<Self> {
        Ok(Self {
            model: load_det_model(onnx)?,
        })
    }

    /// **热替换检测模型**(自训检测模型时用);无需新建对象,即刻生效。
    pub fn set_model(&mut self, onnx: &Path) -> Result<()> {
        self.model = load_det_model(onnx)?;
        Ok(())
    }

    /// 默认检测模型在缓存中的路径(缺则下载)。
    pub async fn default_model_path() -> Result<PathBuf> {
        ensure_det_model().await
    }

    /// 检测图中目标 → 按置信度降序的 [`BBox`](已 NMS,原图像素坐标)。
    pub fn detect(&self, image: &[u8]) -> Result<Vec<BBox>> {
        let img = image::load_from_memory(image).map_err(terr)?;
        let (ow, oh) = (img.width(), img.height());
        if ow == 0 || oh == 0 {
            return Err(Error::msg("DET: 空图"));
        }
        let ratio = (DET_SIZE as f32 / ow as f32).min(DET_SIZE as f32 / oh as f32);
        let rw = ((ow as f32 * ratio).round().max(1.0) as u32).min(DET_SIZE as u32);
        let rh = ((oh as f32 * ratio).round().max(1.0) as u32).min(DET_SIZE as u32);
        let resized = img
            .resize_exact(rw, rh, image::imageops::FilterType::Triangle)
            .to_rgb8();
        // 灰边 416×416、原始 0–255、NCHW(YOLOX 无归一化)。
        let plane = DET_SIZE * DET_SIZE;
        let mut data = vec![114f32; 3 * plane];
        for y in 0..rh {
            for x in 0..rw {
                let p = resized.get_pixel(x, y);
                let idx = y as usize * DET_SIZE + x as usize;
                data[idx] = p[0] as f32;
                data[plane + idx] = p[1] as f32;
                data[2 * plane + idx] = p[2] as f32;
            }
        }
        let input = tract_ndarray::Array4::<f32>::from_shape_vec((1, 3, DET_SIZE, DET_SIZE), data)
            .map_err(terr)?;
        let runnable = self.model.clone().into_runnable().map_err(terr)?;
        let out = runnable
            .run(tvec![Tensor::from(input).into()])
            .map_err(terr)?;
        let t = out[0].clone().into_tensor();
        let view = t.to_plain_array_view::<f32>().map_err(terr)?;
        let flat: Vec<f32> = view.iter().copied().collect();
        Ok(decode_det(&flat, ratio, ow, oh))
    }
}

/// 加载并优化检测模型(固定输入 `1×3×416×416`,供 `Det::from_model_path` / `set_model` 复用)。
fn load_det_model(onnx: &Path) -> Result<TypedModel> {
    tract_onnx::onnx()
        .model_for_path(onnx)
        .map_err(terr)?
        .with_input_fact(
            0,
            InferenceFact::dt_shape(f32::datum_type(), tvec![1, 3, DET_SIZE, DET_SIZE]),
        )
        .map_err(terr)?
        .into_optimized()
        .map_err(terr)
}

/// 网格 + 步幅(对齐 ddddocr meshgrid;stride 8→16→32,各 `(416/s)²` 个,行主序 (x,y))。
fn det_grids() -> Vec<(f32, f32, f32)> {
    let mut g = Vec::new();
    for &s in &DET_STRIDES {
        let n = DET_SIZE / s;
        for i in 0..n {
            for j in 0..n {
                g.push((j as f32, i as f32, s as f32));
            }
        }
    }
    g
}

/// YOLOX 解码(flat = `[N,6]` 行主序:x,y,w,h,obj,cls)+ NMS → 原图坐标 BBox(score 降序)。
fn decode_det(flat: &[f32], ratio: f32, ow: u32, oh: u32) -> Vec<BBox> {
    let grids = det_grids();
    let n = (flat.len() / 6).min(grids.len());
    let (owf, ohf) = (ow as f32, oh as f32);
    let mut cand: Vec<BBox> = Vec::new();
    for (k, &(gx, gy, s)) in grids.iter().enumerate().take(n) {
        let o = k * 6;
        let score = flat[o + 4] * flat[o + 5];
        if score < DET_SCORE_THR {
            continue;
        }
        let cx = (flat[o] + gx) * s;
        let cy = (flat[o + 1] + gy) * s;
        let w = flat[o + 2].exp() * s;
        let h = flat[o + 3].exp() * s;
        let x1 = ((cx - w / 2.0) / ratio).clamp(0.0, owf - 1.0);
        let y1 = ((cy - h / 2.0) / ratio).clamp(0.0, ohf - 1.0);
        let x2 = ((cx + w / 2.0) / ratio).clamp(0.0, owf - 1.0);
        let y2 = ((cy + h / 2.0) / ratio).clamp(0.0, ohf - 1.0);
        if x2 > x1 && y2 > y1 {
            cand.push(BBox {
                x1: x1 as u32,
                y1: y1 as u32,
                x2: x2 as u32,
                y2: y2 as u32,
                score,
            });
        }
    }
    nms_boxes(cand, DET_NMS_THR)
}

fn nms_boxes(mut boxes: Vec<BBox>, thr: f32) -> Vec<BBox> {
    boxes.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut keep: Vec<BBox> = Vec::new();
    'outer: for b in boxes {
        for k in &keep {
            if box_iou(&b, k) > thr {
                continue 'outer;
            }
        }
        keep.push(b);
    }
    keep
}

fn box_iou(a: &BBox, b: &BBox) -> f32 {
    let x1 = a.x1.max(b.x1);
    let y1 = a.y1.max(b.y1);
    let x2 = a.x2.min(b.x2);
    let y2 = a.y2.min(b.y2);
    if x2 <= x1 || y2 <= y1 {
        return 0.0;
    }
    let inter = ((x2 - x1) as f32) * ((y2 - y1) as f32);
    let aa = ((a.x2 - a.x1) as f32) * ((a.y2 - a.y1) as f32);
    let ab = ((b.x2 - b.x1) as f32) * ((b.y2 - b.y1) as f32);
    inter / (aa + ab - inter)
}

/// 框**中心**是否落在矩形 `region`(原图像素,用 [`BBox`] 表示边界)内。
/// 用于 [`ClickWord::solve_excluding`] 排除工具栏等非文字区域的检测框。
fn box_center_in(b: &BBox, region: &BBox) -> bool {
    let (cx, cy) = b.center();
    cx >= region.x1 && cx <= region.x2 && cy >= region.y1 && cy <= region.y2
}

async fn ensure_det_model() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("DRISSION_DET_MODEL") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
        return Err(Error::msg(format!(
            "DET: DRISSION_DET_MODEL 路径不存在: {}",
            p.display()
        )));
    }
    let dir = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("drission")
        .join("ocr");
    std::fs::create_dir_all(&dir).map_err(terr)?;
    let path = dir.join("ddddocr_common_det.onnx");
    if path.exists()
        && std::fs::metadata(&path)
            .map(|m| m.len() > 1_000_000)
            .unwrap_or(false)
    {
        return Ok(path);
    }
    let url = std::env::var("DRISSION_DET_MODEL_URL").unwrap_or_else(|_| DET_MODEL_URL.to_string());
    tracing::info!(target: "drission::ocr", "下载检测模型(仅首次): {url}");
    let bytes = reqwest::get(&url)
        .await
        .map_err(terr)?
        .bytes()
        .await
        .map_err(terr)?;
    if bytes.len() < 1_000_000 {
        return Err(Error::msg(format!(
            "DET: 模型下载异常({} bytes)",
            bytes.len()
        )));
    }
    let tmp = path.with_extension("onnx.part");
    std::fs::write(&tmp, &bytes).map_err(terr)?;
    std::fs::rename(&tmp, &path).map_err(terr)?;
    Ok(path)
}

// ─────────────────────── 点选 / 文字点选验证码求解 ───────────────────────

/// 点选**命中**:某目标字被指派到的检测框 + 点击点(原图像素)+ **亲和度**(0–1,越高越可信)。
/// 由 [`ClickWord::solve`] 返回;`affinity` 供调用方设阈值决定"点击 / 换图重试"(置信度低多半误配)。
#[derive(Debug, Clone, Copy)]
pub struct ClickHit {
    /// 目标字(提示里要依次点击的那个字)。
    pub target: char,
    /// 被指派到的检测框(原图像素坐标)。
    pub bbox: BBox,
    /// 点击点(框中心,原图像素坐标)。
    pub point: (u32, u32),
    /// 该框对该目标字的 OCR 亲和度(受约束识别概率,0–1;越高越可信)。
    pub affinity: f32,
    /// 字形**模板**相似度(0–1;有系统 CJK 字体时给出,`None`=未启用模板)。低于 OCR 多半是"确信误读"。
    pub template: Option<f32>,
}

/// **点选 / 文字点选验证码**求解器:检测主图所有字符/图标(`common_det.onnx`)→ 逐框裁剪 OCR
/// (`common.onnx`)→ 按提示顺序匹配出**依次点击点**。配合浏览器可信点击即可自动点选。
///
/// ```no_run
/// # async fn f() -> drission::Result<()> {
/// use drission::ocr::ClickWord;
/// let cw = ClickWord::new().await?;                 // 首次下载两个模型
/// let main_png = std::fs::read("captcha.png")?;     // 带文字的大图
/// // 提示顺序(可由 OCR 提示条得到):依次点「天」「空」「树」
/// let targets = ["天","空","树"].map(String::from);
/// let points = cw.points_for(&main_png, &targets)?; // 返回各目标中心点(原图坐标)
/// for (x, y) in points { /* 浏览器可信点击 (x,y) */ }
/// # Ok(()) }
/// ```
pub struct ClickWord {
    pub det: Det,
    pub ocr: Ocr,
    /// 字形模板匹配器(渲染字体第二信号);`new()` 自动探测系统 CJK 字体,无字体则 `None`(降级纯 OCR)。
    pub font: Option<GlyphMatcher>,
    /// **真样本模板库**(第二信号·优先):有标注真样本时按最近邻匹配,比渲染字体更贴目标字体。
    /// `new()` 读环境变量 `DRISSION_GLYPH_SAMPLES=目录` 自动加载。
    pub bank: Option<SampleBank>,
}

/// 字形模板信号在指派中的(满)权重(`combo = ocr_affinity + 权重 × gate × 模板相似度`)。
const TEMPLATE_WEIGHT: f32 = 1.5;
/// 模板**置信门控**上/下阈值:OCR 该框 top 亲和度 ≥ HI ⇒ 模板权重 0(完全信 OCR);≤ LO ⇒ 满权重
/// (完全放手模板);之间线性。见 [`template_gate`]。
/// 阈值取**较高**(0.6/0.2):易盾艺术字 OCR 多在 0.00–0.10 纯噪声,**不能**被它当「OCR 有信心」而压掉
/// 真样本模板(实测 0.09 噪声把模板压到 0.11× → 目标被挤到假阳性框,故意调高门槛只在 OCR 真自信(≥0.6)时才信 OCR)。
const TEMPLATE_GATE_HI: f32 = 0.60;
const TEMPLATE_GATE_LO: f32 = 0.20;

/// 由「OCR 对某框的 top 亲和度」算模板门控权重 ∈ `[0,1]`(0=完全信 OCR、1=完全放手模板)。
/// 让真样本模板**只在 OCR 读不出(≈0)时**主导,绝不推翻确信且多半正确的 OCR —— 避免里程碑 79
/// 自评里 always-on 融合把清晰字拖偏的回归。
fn template_gate(ocr_top: f32) -> f32 {
    if TEMPLATE_GATE_HI <= TEMPLATE_GATE_LO {
        return 1.0;
    }
    ((TEMPLATE_GATE_HI - ocr_top) / (TEMPLATE_GATE_HI - TEMPLATE_GATE_LO)).clamp(0.0, 1.0)
}

impl ClickWord {
    /// 加载检测 + 识别两个模型(首次会下载),自动探测系统 CJK 字体(渲染模板)与 `DRISSION_GLYPH_SAMPLES`
    /// 真样本库,启用**字形模板**第二信号。
    pub async fn new() -> Result<Self> {
        Ok(Self {
            det: Det::new().await?,
            ocr: Ocr::new().await?,
            font: GlyphMatcher::from_system().ok(),
            bank: load_sample_bank_env(),
        })
    }

    /// 用已加载的模型组装(同样自动探测字体 + 真样本库)。
    pub fn from_models(det: Det, ocr: Ocr) -> Self {
        Self {
            det,
            ocr,
            font: GlyphMatcher::from_system().ok(),
            bank: load_sample_bank_env(),
        }
    }

    /// 指定/替换字形模板字体(`None` = 关闭渲染字体模板)。
    pub fn set_font(&mut self, font: Option<GlyphMatcher>) {
        self.font = font;
    }

    /// 指定/替换真样本模板库(`None` = 关闭)。优先级高于渲染字体模板。
    pub fn set_sample_bank(&mut self, bank: Option<SampleBank>) {
        self.bank = bank;
    }

    /// 模板第二信号是否可用(真样本库或系统字体二者之一)。
    pub fn has_font(&self) -> bool {
        self.font.is_some() || self.bank.is_some()
    }

    /// 检测主图所有目标 → 逐框 OCR,返回 `(框, 识别文本)`(按检测置信度降序)。
    pub fn chars(&self, image: &[u8]) -> Result<Vec<(BBox, String)>> {
        let img = image::load_from_memory(image).map_err(terr)?;
        let (iw, ih) = (img.width(), img.height());
        let mut out = Vec::new();
        for b in self.det.detect(image)? {
            let crop = crop_padded(&img, &b, iw, ih);
            let mut buf = std::io::Cursor::new(Vec::new());
            crop.write_to(&mut buf, image::ImageFormat::Png)
                .map_err(terr)?;
            let txt = self.ocr.recognize(buf.get_ref()).unwrap_or_default();
            out.push((b, txt));
        }
        Ok(out)
    }

    /// 返回每个检测框的**裁剪图 PNG**(与 [`solve`](Self::solve) 同样的外扩裁剪)。用于**采集自训样本**
    /// (逐字落盘 → 人工标注 → dddd_trainer 训练 → [`Ocr::set_model_with_charset`] 热替换)与可视化调试。
    pub fn crops(&self, image: &[u8]) -> Result<Vec<(BBox, Vec<u8>)>> {
        let img = image::load_from_memory(image).map_err(terr)?;
        let (iw, ih) = (img.width(), img.height());
        let mut out = Vec::new();
        for b in self.det.detect(image)? {
            let crop = crop_padded(&img, &b, iw, ih);
            let mut buf = std::io::Cursor::new(Vec::new());
            crop.write_to(&mut buf, image::ImageFormat::Png)
                .map_err(terr)?;
            out.push((b, buf.into_inner()));
        }
        Ok(out)
    }

    /// 按**单个** [`BBox`] 从原图裁出该字图 PNG(与 [`solve`](Self::solve)/[`crops`](Self::crops) **同样的外扩裁剪**)。
    /// 用于「过盾即验真」采样:拿到命中 [`ClickHit`] 后,按其 `bbox` 从干净图裁字 → [`SampleBank::save_labeled`]。
    pub fn crop_bbox(&self, image: &[u8], b: &BBox) -> Result<Vec<u8>> {
        let img = image::load_from_memory(image).map_err(terr)?;
        let (iw, ih) = (img.width(), img.height());
        let crop = crop_padded(&img, b, iw, ih);
        let mut buf = std::io::Cursor::new(Vec::new());
        crop.write_to(&mut buf, image::ImageFormat::Png)
            .map_err(terr)?;
        Ok(buf.into_inner())
    }

    /// **「过盾即验真」自动采样**:把一组**已验证正确**的命中按其 `bbox` 从 `image`(干净图)裁字,按
    /// `{目标字}/` 存进样本库目录 `dir`(内容寻址去重)。返回**本次新增**样本数。
    ///
    /// 由来:易盾 `api/check` 回 `result:true` ⇒ 这一题各 [`ClickHit`] 的 `target` 就是其 `bbox` 框里
    /// **真实的字**(点击序与 `front` 一致且被服务端判过)。于是每过一次盾就白捡若干**标签已验证**的真样本,
    /// bank 越跑越厚、[`GlyphMatcher`]/[`SampleBank`] 模板信号越准——**零人工**破解里程碑 59 的「数据墙」。
    /// 采到的样本下次进程启动即被 `DRISSION_GLYPH_SAMPLES` 自动加载,或本进程内调
    /// [`reload_sample_bank`](Self::reload_sample_bank) 即时生效。
    pub fn harvest_verified(&self, image: &[u8], hits: &[ClickHit], dir: &Path) -> Result<usize> {
        let img = image::load_from_memory(image).map_err(terr)?;
        let (iw, ih) = (img.width(), img.height());
        let mut n = 0;
        for h in hits {
            let crop = crop_padded(&img, &h.bbox, iw, ih);
            let mut buf = std::io::Cursor::new(Vec::new());
            if crop.write_to(&mut buf, image::ImageFormat::Png).is_err() {
                continue;
            }
            if SampleBank::save_labeled(dir, h.target, buf.get_ref())?.is_some() {
                n += 1;
            }
        }
        Ok(n)
    }

    /// 从目录**重载真样本库**(采样后即时生效:让同一进程内后续题就能用上刚白捡的样本)。
    /// 目录为空/不存在 ⇒ 置 `None`(退化为渲染字体模板或纯 OCR)。
    pub fn reload_sample_bank(&mut self, dir: &Path) {
        self.bank = SampleBank::from_dir(dir).ok();
    }

    /// **受约束求解**(点选内核):检测主图 → 逐框对**每个目标字**算亲和度([`Ocr::char_affinity`])→
    /// **全局最优指派**([`assign_optimal`]:每个目标分到**互不相同**的框、最大化总亲和度)→ 按目标顺序
    /// 返回命中 [`ClickHit`](含框 / 点击点 / **置信度** `affinity`)。
    ///
    /// 相比旧的"按目标序贪心分配",全局指派纠正了**多候选误配**(前面的目标抢走后面目标更需要的框);
    /// 比 [`points_for_text`](Self::points_for_text) 的"全字符集 argmax 再配文本"鲁棒得多(艺术体单字
    /// argmax 易误判)。`affinity` 让调用方可设阈值:置信度过低就**换图重试**而非乱点。框不足时只返回
    /// 已分到框的目标(`hits.len() < targets.len()`)。
    pub fn solve(&self, image: &[u8], targets: &[String]) -> Result<Vec<ClickHit>> {
        self.solve_excluding(image, targets, &[])
    }

    /// 同 [`solve`](Self::solve),但**先丢弃中心落在 `exclude` 任一矩形(原图像素)内的检测框**,
    /// 再做识别 / 全局指派。用于排除验证码图上的**非文字干扰区**——典型如易盾弹窗右上角的
    /// 刷新 / 语音 / 反馈工具栏:截图回退路径会把这些图标拍进图里,Det 易把它们误检成字框,
    /// 一旦被指派点击就会把验证码**切成语音模式**(用户实测踩坑)。`exclude` 为空时等价于 `solve`。
    pub fn solve_excluding(
        &self,
        image: &[u8],
        targets: &[String],
        exclude: &[BBox],
    ) -> Result<Vec<ClickHit>> {
        let chars: Vec<char> = targets
            .iter()
            .filter_map(|s| s.trim().chars().next())
            .collect();
        if chars.is_empty() {
            return Ok(vec![]);
        }
        let img = image::load_from_memory(image).map_err(terr)?;
        let (iw, ih) = (img.width(), img.height());
        // 丢弃中心落在排除区(工具栏带等)内的检测框 —— 从源头杜绝"点到语音/刷新开关"。
        let boxes: Vec<BBox> = self
            .det
            .detect(image)?
            .into_iter()
            .filter(|b| !exclude.iter().any(|r| box_center_in(b, r)))
            // 丢弃**非字形检测框**:汉字框近似方形,排除细条/小点假阳性(如图边竖条、桅杆、纹理)——
            // 否则它们会被指派去承接某目标字(实测易盾点选把目标点到 8px 宽的边缘条上 → 必失败)。
            .filter(|b| {
                let (w, h) = (b.width().max(1), b.height().max(1));
                let aspect = (w as f32 / h as f32).max(h as f32 / w as f32);
                w >= 10 && h >= 10 && aspect <= 3.0
            })
            .collect();
        let mut aff: Vec<Vec<f32>> = Vec::with_capacity(boxes.len());
        let mut tpl: Vec<Vec<f32>> = Vec::with_capacity(boxes.len());
        for b in &boxes {
            let crop = crop_padded(&img, b, iw, ih);
            // ① OCR:多预处理变体(原图 / 自动对比度 / Otsu 去背景)各识别一次,**按锐度(top1-top2)选最佳
            //    那版的整向量** —— "把背景抹掉让字浮出来"且安全(不会把某变体对错字的虚高分混入指派)。
            let mut vecs: Vec<Vec<f32>> = Vec::new();
            for v in glyph_variants(&crop) {
                let mut buf = std::io::Cursor::new(Vec::new());
                if v.write_to(&mut buf, image::ImageFormat::Png).is_err() {
                    continue;
                }
                if let Ok(a) = self.ocr.char_affinity(buf.get_ref(), &chars) {
                    vecs.push(a);
                }
            }
            if vecs.is_empty() {
                vecs.push(vec![0.0; chars.len()]);
            }
            let pick = select_by_margin(&vecs);
            aff.push(vecs.swap_remove(pick));
            // ② 字形模板第二信号:**真样本库优先**(更贴目标字体)、回退渲染字体;都无则全 0(退化纯 OCR)。
            tpl.push(if self.font.is_some() || self.bank.is_some() {
                let cf = glyph::crop_feat(&crop);
                chars
                    .iter()
                    .map(|&ch| {
                        if let Some(b) = self.bank.as_ref().filter(|b| b.has_char(ch)) {
                            b.similarity(&cf, ch)
                        } else if let Some(f) = &self.font {
                            f.similarity(&cf, ch)
                        } else {
                            0.0
                        }
                    })
                    .collect()
            } else {
                vec![0.0; chars.len()]
            });
        }
        // 融合:combo = OCR亲和度 + **置信门控**权重 × 模板相似度,在 combo 上做全局最优指派。
        // 门控:按「OCR 对该框的 top 亲和度」衰减模板权重——OCR 已自信(≥HI)就别让模板翻案(权重 0),
        // 只有 OCR≈0(艺术字读不出,≤LO)才放手让真样本模板主导(权重 = TEMPLATE_WEIGHT)。
        // 依据:离线留一法自评(`examples/clickword_eval`)显示清晰字的受约束 OCR 已 ~97%,一味 always-on
        // 叠加模板反把「确信且正确」的 OCR 拖偏;门控既保住易框、又保留 OCR≈0 时模板救场(里程碑 78 实证)。
        let combo: Vec<Vec<f32>> = aff
            .iter()
            .zip(&tpl)
            .map(|(a, t)| {
                let ocr_top = a.iter().copied().fold(0.0f32, f32::max);
                let gate = template_gate(ocr_top);
                a.iter()
                    .zip(t)
                    .map(|(&av, &tv)| av + TEMPLATE_WEIGHT * gate * tv)
                    .collect()
            })
            .collect();
        let assign = assign_optimal(&combo, chars.len());
        let has_font = self.has_font();
        let mut hits = Vec::new();
        for (t, ch) in chars.iter().enumerate() {
            if let Some(bi) = assign[t] {
                let bbox = boxes[bi];
                hits.push(ClickHit {
                    target: *ch,
                    bbox,
                    point: bbox.center(),
                    affinity: aff[bi].get(t).copied().unwrap_or(0.0),
                    template: has_font.then(|| tpl[bi].get(t).copied().unwrap_or(0.0)),
                });
            }
        }
        Ok(hits)
    }

    /// 按 `targets`(提示给的标准答案字,有序)返回**依次点击点**。等价于 [`solve`](Self::solve) 再取
    /// 各命中的点击点(丢弃置信度);需要置信度做阈值/重试请直接用 [`solve`](Self::solve)。
    pub fn points_for(&self, image: &[u8], targets: &[String]) -> Result<Vec<(u32, u32)>> {
        Ok(self
            .solve(image, targets)?
            .into_iter()
            .map(|h| h.point)
            .collect())
    }

    /// 文本法点击点(逐框全字符集 OCR → 文本相等/包含匹配)。鲁棒性弱于 [`points_for`],保留作回退/对照。
    pub fn points_for_text(&self, image: &[u8], targets: &[String]) -> Result<Vec<(u32, u32)>> {
        Ok(match_order(&self.chars(image)?, targets))
    }
}

/// 顺序匹配(纯函数,便于单测):在 `items`(框+识别文本)里按 `targets` 顺序挑框,返回各中心点。
/// 优先文本相等,其次互相包含;命中的框不复用;匹配不到的目标跳过。
fn match_order(items: &[(BBox, String)], targets: &[String]) -> Vec<(u32, u32)> {
    let mut used = vec![false; items.len()];
    let mut pts = Vec::new();
    for t in targets {
        let t = t.trim();
        if t.is_empty() {
            continue;
        }
        let mut pick = items
            .iter()
            .enumerate()
            .find(|(i, (_, s))| !used[*i] && s.trim() == t)
            .map(|(i, _)| i);
        if pick.is_none() {
            pick = items
                .iter()
                .enumerate()
                .find(|(i, (_, s))| {
                    let s = s.trim();
                    !used[*i] && !s.is_empty() && (s.contains(t) || t.contains(s))
                })
                .map(|(i, _)| i);
        }
        if let Some(i) = pick {
            used[i] = true;
            pts.push(items[i].0.center());
        }
    }
    pts
}

/// "多分配一个目标"相对"亲和度更高"的优先权重:取 > 单框最大亲和度(亲和度 ≤ 1),
/// 保证框够时**每个目标都分到框**(而非为了凑高亲和度让某目标空缺)。
const ASSIGN_BONUS: f32 = 1000.0;

/// 亲和度矩阵的**全局最优指派**(纯函数,便于单测)。`aff[i][t]` = 第 `i` 个检测框对第 `t` 个目标字的
/// 亲和度;为每个目标分配**互不相同**的框,**优先"尽量多分到框"**(框足够时每个目标都有框),其次
/// **最大化总亲和度**。返回 `assign[t] = Some(框下标)`(无框可分时 `None`)。
///
/// 胜过"按目标序逐个挑最高未用框"的贪心:贪心会让靠前的目标抢走靠后目标更需要的框(经典指派问题)。
/// 点选规模小(一般 ≤ 6 字、≤ 一二十框),用 **DFS + 分支定界**精确求最优。
fn assign_optimal(aff: &[Vec<f32>], n_targets: usize) -> Vec<Option<usize>> {
    if n_targets == 0 {
        return vec![];
    }
    let n_boxes = aff.len();
    // 后缀上界:剩余目标都按"各自列最大亲和度 + BONUS"乐观估计(分支定界用;过估只影响剪枝力度,不影响正确性)。
    let col_max: Vec<f32> = (0..n_targets)
        .map(|t| {
            aff.iter()
                .map(|r| r.get(t).copied().unwrap_or(0.0))
                .fold(0.0f32, f32::max)
        })
        .collect();
    let mut suffix = vec![0.0f32; n_targets + 1];
    for t in (0..n_targets).rev() {
        suffix[t] = suffix[t + 1] + ASSIGN_BONUS + col_max[t];
    }
    let mut used = vec![false; n_boxes];
    let mut cur = vec![None; n_targets];
    let mut best_assign = vec![None; n_targets];
    let mut best_score = f32::MIN;
    assign_dfs(
        aff,
        n_targets,
        &suffix,
        0,
        0.0,
        &mut used,
        &mut cur,
        &mut best_score,
        &mut best_assign,
    );
    best_assign
}

#[allow(clippy::too_many_arguments)]
fn assign_dfs(
    aff: &[Vec<f32>],
    n_targets: usize,
    suffix: &[f32],
    t: usize,
    score: f32,
    used: &mut [bool],
    cur: &mut [Option<usize>],
    best_score: &mut f32,
    best_assign: &mut [Option<usize>],
) {
    if t == n_targets {
        if score > *best_score {
            *best_score = score;
            best_assign.copy_from_slice(cur);
        }
        return;
    }
    // 分支定界:当前得分 + 剩余最乐观上界 仍不优于已知最好 → 剪枝。
    if score + suffix[t] <= *best_score {
        return;
    }
    // 选项 A:把某个未用框分给目标 t。
    for b in 0..used.len() {
        if used[b] {
            continue;
        }
        used[b] = true;
        cur[t] = Some(b);
        let a = aff[b].get(t).copied().unwrap_or(0.0);
        assign_dfs(
            aff,
            n_targets,
            suffix,
            t + 1,
            score + ASSIGN_BONUS + a,
            used,
            cur,
            best_score,
            best_assign,
        );
        cur[t] = None;
        used[b] = false;
    }
    // 选项 B:跳过目标 t(留空,把框让给后面更需要它的目标)。框足够时 BONUS 保证这条不会胜出,
    // 框不足时它让"哪些目标空缺"也由总亲和度最优决定(而非僵硬地空缺靠后的)。
    cur[t] = None;
    assign_dfs(
        aff,
        n_targets,
        suffix,
        t + 1,
        score,
        used,
        cur,
        best_score,
        best_assign,
    );
}

// ─────────────────────── 点选识别增强:预处理变体(TTA)───────────────────────

/// 从环境变量 `DRISSION_GLYPH_SAMPLES` 指定的目录加载真样本模板库(失败/未设则 `None`)。
fn load_sample_bank_env() -> Option<SampleBank> {
    std::env::var("DRISSION_GLYPH_SAMPLES")
        .ok()
        .and_then(|d| SampleBank::from_dir(Path::new(&d)).ok())
}

/// 按检测框**适度外扩**裁出单字图(外扩 = 长边/6,至少 2px),利于单字识别;`chars`/`solve`/`crops` 共用。
fn crop_padded(img: &image::DynamicImage, b: &BBox, iw: u32, ih: u32) -> image::DynamicImage {
    let pad = (b.width().max(b.height()) / 6).max(2);
    let x = b.x1.saturating_sub(pad);
    let y = b.y1.saturating_sub(pad);
    let w = (b.x2 + pad)
        .min(iw.saturating_sub(1))
        .saturating_sub(x)
        .max(1);
    let h = (b.y2 + pad)
        .min(ih.saturating_sub(1))
        .saturating_sub(y)
        .max(1);
    img.crop_imm(x, y, w, h)
}

/// 单字框的多种预处理**变体**:`[原图, 自动对比度, Otsu 二值(背景抹平)]`。点选求解逐变体各识别一次,
/// 按"置信锐度"选最佳那版(见 [`select_by_margin`])——把"背景去掉让字浮出来"做成可证的图像操作,
/// 对艺术体/低对比字尤其有用,且只增不减(没有更好就退回原图那版)。
fn glyph_variants(crop: &image::DynamicImage) -> Vec<image::DynamicImage> {
    vec![crop.clone(), autocontrast(crop), otsu_binarize(crop)]
}

/// 自动对比度:转灰度后按 2%/98% 分位线性拉伸到 0–255(去掉两端长尾),让偏淡/偏暗的字更清晰。
fn autocontrast(img: &image::DynamicImage) -> image::DynamicImage {
    let luma = img.to_luma8();
    let (w, h) = (luma.width(), luma.height());
    let mut hist = [0u32; 256];
    for p in luma.pixels() {
        hist[p[0] as usize] += 1;
    }
    let total = (w * h).max(1);
    let cut = (total as f32 * 0.02) as u32;
    let mut lo = 0u8;
    let mut acc = 0u32;
    for (i, &c) in hist.iter().enumerate() {
        acc += c;
        if acc > cut {
            lo = i as u8;
            break;
        }
    }
    let mut hi = 255u8;
    acc = 0;
    for i in (0..256).rev() {
        acc += hist[i];
        if acc > cut {
            hi = i as u8;
            break;
        }
    }
    if hi <= lo {
        return image::DynamicImage::ImageLuma8(luma);
    }
    let span = (hi - lo) as f32;
    let out = image::ImageBuffer::from_fn(w, h, |x, y| {
        let v = luma.get_pixel(x, y)[0];
        let nv = ((v.saturating_sub(lo) as f32 / span) * 255.0).clamp(0.0, 255.0) as u8;
        image::Luma([nv])
    });
    image::DynamicImage::ImageLuma8(out)
}

/// Otsu 全局阈值二值化(灰度):自动找前景/背景分界 → 把杂乱(常是照片)背景抹成纯色,字成纯黑/白。
/// 照片背景未必干净,但配 [`select_by_margin`] 只在它更"锐"时才被采用,故安全。
fn otsu_binarize(img: &image::DynamicImage) -> image::DynamicImage {
    let luma = img.to_luma8();
    let (w, h) = (luma.width(), luma.height());
    let n = (w * h).max(1) as f32;
    let mut hist = [0u32; 256];
    for p in luma.pixels() {
        hist[p[0] as usize] += 1;
    }
    let sum: f32 = (0..256).map(|i| i as f32 * hist[i] as f32).sum();
    let (mut sumb, mut wb, mut maxv, mut thr) = (0f32, 0f32, 0f32, 0u8);
    for (i, &c) in hist.iter().enumerate() {
        wb += c as f32;
        if wb == 0.0 {
            continue;
        }
        let wf = n - wb;
        if wf <= 0.0 {
            break;
        }
        sumb += i as f32 * c as f32;
        let mb = sumb / wb;
        let mf = (sum - sumb) / wf;
        let between = wb * wf * (mb - mf) * (mb - mf);
        if between > maxv {
            maxv = between;
            thr = i as u8;
        }
    }
    let out = image::ImageBuffer::from_fn(w, h, |x, y| {
        image::Luma([if luma.get_pixel(x, y)[0] > thr {
            255
        } else {
            0
        }])
    });
    image::DynamicImage::ImageLuma8(out)
}

/// 在多变体的亲和度向量里选**最"锐"**的那个的下标:锐度 = `top1 - top2`(对该框最能区分出某个目标字的
/// 那一版),并列取最小下标(优先原图)。比"逐元素取 max"安全:不会把某变体对**错字**的虚高分混进来。
fn select_by_margin(vectors: &[Vec<f32>]) -> usize {
    let mut best = 0usize;
    let mut bestm = f32::MIN;
    for (i, v) in vectors.iter().enumerate() {
        let m = margin(v);
        if m > bestm {
            bestm = m;
            best = i;
        }
    }
    best
}

/// 向量的 `top1 - top2`(单元素时即该值;空向量返回 0)。
fn margin(v: &[f32]) -> f32 {
    let (mut top1, mut top2) = (f32::MIN, f32::MIN);
    for &x in v {
        if x > top1 {
            top2 = top1;
            top1 = x;
        } else if x > top2 {
            top2 = x;
        }
    }
    if top1 == f32::MIN {
        0.0
    } else if top2 == f32::MIN {
        top1
    } else {
        top1 - top2
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

/// 读字符集文件(用于自训模型热替换):自动识别三种格式 ——
/// `{"charset":[...]}`(ddddocr / dddd_trainer 的 `charsets.json`)、纯 JSON 数组 `[...]`、
/// 每行一字的纯文本(首个空行即 CTC blank 占位)。
pub fn load_charset_file(path: &Path) -> Result<Vec<String>> {
    let s = std::fs::read_to_string(path).map_err(terr)?;
    let t = s.trim_start();
    if t.starts_with('{') {
        return parse_charset(&s);
    }
    if t.starts_with('[') {
        let v: serde_json::Value = serde_json::from_str(&s).map_err(terr)?;
        let arr = v
            .as_array()
            .ok_or_else(|| Error::msg("OCR: charset 文件不是 JSON 数组"))?;
        return Ok(arr
            .iter()
            .map(|x| x.as_str().unwrap_or("").to_string())
            .collect());
    }
    // 纯文本:每行一字(保留空行作 blank 占位,仅去行尾 CR)。
    Ok(s.lines()
        .map(|l| l.trim_end_matches('\r').to_string())
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
#[cfg(feature = "camoufox")]
static DEFAULT_OCR: tokio::sync::OnceCell<Ocr> = tokio::sync::OnceCell::const_new();

/// `tab.ocr_image` 的**热替换槽**:一旦设置,优先于懒加载的默认实例(全局即时生效,无需重启)。
#[cfg(feature = "camoufox")]
static OCR_OVERRIDE: tokio::sync::RwLock<Option<std::sync::Arc<Ocr>>> =
    tokio::sync::RwLock::const_new(None);

/// **热替换** `tab.ocr_image` 用的进程级识别器:传入自训 [`Ocr`](自定义模型 + 字符集)即全局生效,
/// 之后所有 `tab.ocr_image` 都走它,无需重启进程。传入前用 [`Ocr::from_files`] / [`Ocr::from_model_path_with_charset`] 构造。
#[cfg(feature = "camoufox")]
pub async fn set_default_ocr(ocr: Ocr) {
    *OCR_OVERRIDE.write().await = Some(std::sync::Arc::new(ocr));
}

/// `Tab::ocr_image` 便捷方法(需 Camoufox 后端的 [`Tab`])。
#[cfg(feature = "camoufox")]
impl Tab {
    /// **一步识别**页面里某元素的验证码图:定位 `selector`(`css:`/`xpath:` 前缀,同 [`Tab::ele`])→
    /// 取图(`<img>` 的 `data:` URL 直接解码,否则元素截图)→ ddddocr 模型识别 → 文本。
    /// 首次调用会懒加载默认模型(可能下载 ~54MB)。
    pub async fn ocr_image(&self, selector: &str) -> Result<String> {
        let bytes = self.fetch_image_bytes(selector).await?;
        // 优先用热替换槽里的自训模型(若设过 set_default_ocr),否则懒加载默认 ddddocr 模型。
        if let Some(ocr) = OCR_OVERRIDE.read().await.clone() {
            return ocr.recognize(&bytes);
        }
        let ocr = DEFAULT_OCR.get_or_try_init(Ocr::new).await?;
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

    #[test]
    fn det_grids_count_matches_yolox() {
        // 52² + 26² + 13² = 3549(stride 8/16/32 在 416 上的锚点总数)。
        assert_eq!(det_grids().len(), 2704 + 676 + 169);
    }

    fn bb(cx: u32, cy: u32) -> BBox {
        BBox {
            x1: cx - 5,
            y1: cy - 5,
            x2: cx + 5,
            y2: cy + 5,
            score: 0.9,
        }
    }

    #[test]
    fn match_order_follows_targets_and_skips_missing() {
        let items = vec![
            (bb(100, 100), "体".to_string()),
            (bb(20, 30), "验".to_string()),
            (bb(200, 50), "安".to_string()),
        ];
        // 提示顺序 验→体:应按该顺序返回各自中心,而非检测顺序。
        let targets = vec!["验".to_string(), "体".to_string()];
        assert_eq!(match_order(&items, &targets), vec![(20, 30), (100, 100)]);
        // 含匹配不到的 "元":跳过,只返回命中的。
        let targets2 = vec!["元".to_string(), "安".to_string()];
        assert_eq!(match_order(&items, &targets2), vec![(200, 50)]);
        // 同字不复用:两个 "体" 目标但只有一个框 → 只点一次。
        let targets3 = vec!["体".to_string(), "体".to_string()];
        assert_eq!(match_order(&items, &targets3), vec![(100, 100)]);
    }

    #[test]
    fn box_center_in_excludes_toolbar_corner() {
        // 右上角工具栏带:x∈[200,320]、y∈[0,40](原图像素)。
        let band = BBox {
            x1: 200,
            y1: 0,
            x2: 320,
            y2: 40,
            score: 0.0,
        };
        assert!(box_center_in(&bb(260, 20), &band)); // 工具栏图标(角内)→ 丢弃
        assert!(!box_center_in(&bb(100, 90), &band)); // 图中部文字 → 保留
        assert!(!box_center_in(&bb(260, 120), &band)); // 同列但在下方文字 → 保留
    }

    #[test]
    fn assign_optimal_beats_greedy_order() {
        // aff[box][target]:按目标序贪心会让 t0 抢走 box0(它对 t1 也高),逼 t1 拿到差框 box1。
        let aff = vec![vec![0.9, 0.8], vec![0.85, 0.1]];
        // 全局最优:t0→box1(0.85)+ t1→box0(0.8)=1.65 > 贪心 t0→box0 + t1→box1 = 1.0。
        assert_eq!(assign_optimal(&aff, 2), vec![Some(1), Some(0)]);
    }

    #[test]
    fn assign_optimal_all_distinct_when_enough() {
        // 3 框 2 目标:每个目标分到不同框,取总亲和度最大者。
        let aff = vec![vec![0.2, 0.9], vec![0.9, 0.2], vec![0.5, 0.5]];
        let a = assign_optimal(&aff, 2);
        assert_eq!(a, vec![Some(1), Some(0)]);
        assert_ne!(a[0], a[1]); // 互不相同
    }

    #[test]
    fn assign_optimal_partial_when_fewer_boxes() {
        // 框少于目标:每框只用一次,只分给亲和度最高的目标,其余 None。
        let aff = vec![vec![0.1, 0.9, 0.2]];
        let a = assign_optimal(&aff, 3);
        assert_eq!(a[1], Some(0));
        assert_eq!(a.iter().filter(|x| x.is_some()).count(), 1);
        // 无框 / 无目标的边界。
        assert_eq!(assign_optimal(&Vec::<Vec<f32>>::new(), 2), vec![None, None]);
        assert_eq!(assign_optimal(&aff, 0), Vec::<Option<usize>>::new());
    }

    #[test]
    fn select_by_margin_picks_sharpest() {
        // v0 平(锐度 0.01)、v1 峰(锐度 0.5)→ 选 v1。
        let v = vec![vec![0.40, 0.39, 0.38], vec![0.60, 0.10, 0.05]];
        assert_eq!(select_by_margin(&v), 1);
        // 并列取最小下标(优先原图 = 0)。
        let v2 = vec![vec![0.5, 0.2], vec![0.5, 0.2]];
        assert_eq!(select_by_margin(&v2), 0);
        // 单目标:用值本身当锐度。
        let v3 = vec![vec![0.2], vec![0.7]];
        assert_eq!(select_by_margin(&v3), 1);
        assert_eq!(margin(&[]), 0.0);
    }

    #[test]
    fn load_charset_file_three_formats() {
        let dir = std::env::temp_dir();
        let p1 = dir.join("drission_cs_obj.json");
        std::fs::write(&p1, r#"{"charset":["","a","b"]}"#).unwrap();
        assert_eq!(load_charset_file(&p1).unwrap(), vec!["", "a", "b"]);
        let p2 = dir.join("drission_cs_arr.json");
        std::fs::write(&p2, r#"["","x","y","z"]"#).unwrap();
        assert_eq!(load_charset_file(&p2).unwrap(), vec!["", "x", "y", "z"]);
        let p3 = dir.join("drission_cs_lines.txt");
        std::fs::write(&p3, "\n甲\n乙\n").unwrap(); // 首行空 = blank
        assert_eq!(load_charset_file(&p3).unwrap(), vec!["", "甲", "乙"]);
        for p in [p1, p2, p3] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn template_gate_suppresses_when_ocr_confident() {
        // OCR 自信(≥HI)→ 模板权重 0;OCR≈0(≤LO)→ 满权重 1;之间单调递减且夹在 [0,1]。
        assert_eq!(template_gate(0.99), 0.0);
        assert_eq!(template_gate(TEMPLATE_GATE_HI), 0.0);
        assert_eq!(template_gate(0.0), 1.0);
        assert_eq!(template_gate(TEMPLATE_GATE_LO), 1.0);
        let mid = template_gate((TEMPLATE_GATE_HI + TEMPLATE_GATE_LO) / 2.0);
        assert!((mid - 0.5).abs() < 1e-5);
        // 单调:置信越高门控越小。
        assert!(template_gate(0.05) > template_gate(0.15));
        // 永远夹在 [0,1]。
        for c in [-1.0, 0.0, 0.1, 0.5, 2.0] {
            let g = template_gate(c);
            assert!((0.0..=1.0).contains(&g));
        }
    }

    #[test]
    fn glyph_variants_keep_size_and_count() {
        let mut im = image::RgbImage::new(9, 7);
        for (x, y, p) in im.enumerate_pixels_mut() {
            *p = image::Rgb([(x * 25) as u8, (y * 30) as u8, 90]);
        }
        let d = image::DynamicImage::ImageRgb8(im);
        let vs = glyph_variants(&d);
        assert_eq!(vs.len(), 3); // 原图 / 自动对比度 / Otsu
        for v in &vs {
            assert_eq!((v.width(), v.height()), (9, 7));
        }
    }
}
