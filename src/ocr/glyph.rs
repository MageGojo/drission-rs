//! 字形**模板匹配**(点选第二信号,推理期、**无需训练**)。
//!
//! 点选提示已给出标准答案字(如"请依次点击 库 扩 类"),所以我们可以**自己用系统 CJK 字体把目标字
//! 渲染出来**当模板,与每个检测框的字做**形状相似度**,再和 ddddocr 的识别亲和度**融合** —— 当 OCR
//! "确信误读"(高置信但读错)时,形状对不上会把它压下去,从而少踩误配。
//!
//! 表示:都归约为 `N×N` 的零均值、L2 归一向量后做点积(= 归一化互相关 NCC,对亮度/对比免疫)。
//! - **模板**:字体光栅化的覆盖度(`fontdue`,纯 Rust),并按几个角度旋转(易盾字常有倾斜)取最大。
//! - **字框**:灰度后 Sobel **梯度幅值**(字的笔画/描边处梯度强,与模板覆盖度对齐),色彩无关、较稳。

use std::path::{Path, PathBuf};

use fontdue::{Font, FontSettings};

use crate::{Error, Result};

/// 特征网格边长(`N×N`)。
const N: usize = 32;
/// 模板尝试的旋转角(度);取各角相似度最大值,吸收易盾字形倾斜。
const ROTS: [f32; 5] = [-20.0, -10.0, 0.0, 10.0, 20.0];

/// 字形模板匹配器(持一份 CJK 字体)。由 [`GlyphMatcher::from_system`] 探测系统字体加载。
pub struct GlyphMatcher {
    font: Font,
}

impl GlyphMatcher {
    /// 从指定字体文件(`.ttf`/`.ttc`/`.otf`)加载。
    pub fn from_font_path(path: &Path) -> Result<Self> {
        let bytes =
            std::fs::read(path).map_err(|e| Error::msg(format!("CJK 字体读取失败: {e}")))?;
        let font = Font::from_bytes(bytes, FontSettings::default())
            .map_err(|e| Error::msg(format!("CJK 字体解析失败: {e}")))?;
        Ok(Self { font })
    }

    /// 探测系统 CJK 字体(优先 `DRISSION_CJK_FONT` 指定);找不到返回 `Err`(调用方可降级为纯 OCR)。
    pub fn from_system() -> Result<Self> {
        if let Ok(p) = std::env::var("DRISSION_CJK_FONT") {
            return Self::from_font_path(Path::new(&p));
        }
        for p in cjk_font_candidates() {
            if p.exists()
                && let Ok(m) = Self::from_font_path(&p)
            {
                return Ok(m);
            }
        }
        Err(Error::msg(
            "未找到系统 CJK 字体(可设 DRISSION_CJK_FONT 指向 .ttf/.ttc)",
        ))
    }

    /// 字框梯度特征(由 [`crop_feat`] 得)对目标字 `ch` 的最大模板相似度,夹到 `[0,1]`。
    pub fn similarity(&self, crop_feat: &[f32], ch: char) -> f32 {
        let upright = rasterize_centered(&self.font, ch, N);
        if upright.iter().all(|&v| v == 0.0) {
            return 0.0; // 字体无此字形
        }
        let mut best = 0f32;
        for &deg in &ROTS {
            let t = if deg == 0.0 {
                normalize(upright.clone())
            } else {
                normalize(rotate_grid(&upright, N, deg))
            };
            let s = dot(crop_feat, &t);
            if s > best {
                best = s;
            }
        }
        best.clamp(0.0, 1.0)
    }
}

/// 字框灰度 **Sobel 梯度幅值**特征(`N×N`,零均值 + L2 归一),供 [`GlyphMatcher::similarity`]。
pub fn crop_feat(crop: &image::DynamicImage) -> Vec<f32> {
    let g = crop
        .resize_exact(N as u32, N as u32, image::imageops::FilterType::Triangle)
        .to_luma8();
    let at = |x: usize, y: usize| g.get_pixel(x as u32, y as u32)[0] as f32;
    let mut mag = vec![0f32; N * N];
    for y in 1..N - 1 {
        for x in 1..N - 1 {
            let gx = (at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1))
                - (at(x - 1, y - 1) + 2.0 * at(x - 1, y) + at(x - 1, y + 1));
            let gy = (at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1))
                - (at(x - 1, y - 1) + 2.0 * at(x, y - 1) + at(x + 1, y - 1));
            mag[y * N + x] = (gx * gx + gy * gy).sqrt();
        }
    }
    normalize(mag)
}

/// 把字 `ch` 光栅化并**居中**到 `n×n` 覆盖度(0–1)。字体无此字形则全 0。
fn rasterize_centered(font: &Font, ch: char, n: usize) -> Vec<f32> {
    let px = n as f32 * 0.85;
    let (m, bmp) = font.rasterize(ch, px);
    let mut out = vec![0f32; n * n];
    if m.width == 0 || m.height == 0 {
        return out;
    }
    let ox = (n as i32 - m.width as i32) / 2;
    let oy = (n as i32 - m.height as i32) / 2;
    for y in 0..m.height {
        for x in 0..m.width {
            let dx = ox + x as i32;
            let dy = oy + y as i32;
            if dx >= 0 && dy >= 0 && (dx as usize) < n && (dy as usize) < n {
                out[dy as usize * n + dx as usize] = bmp[y * m.width + x] as f32 / 255.0;
            }
        }
    }
    out
}

/// 绕中心旋转 `deg` 度(双线性,边界外补 0)。
fn rotate_grid(src: &[f32], n: usize, deg: f32) -> Vec<f32> {
    let rad = deg.to_radians();
    let (s, c) = rad.sin_cos();
    let cen = (n as f32 - 1.0) / 2.0;
    let mut out = vec![0f32; n * n];
    for y in 0..n {
        for x in 0..n {
            let dx = x as f32 - cen;
            let dy = y as f32 - cen;
            // 反向映射回源坐标采样。
            let sx = c * dx + s * dy + cen;
            let sy = -s * dx + c * dy + cen;
            out[y * n + x] = bilinear(src, n, sx, sy);
        }
    }
    out
}

fn bilinear(src: &[f32], n: usize, x: f32, y: f32) -> f32 {
    if x < 0.0 || y < 0.0 || x > n as f32 - 1.0 || y > n as f32 - 1.0 {
        return 0.0;
    }
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(n - 1);
    let y1 = (y0 + 1).min(n - 1);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let a = src[y0 * n + x0];
    let b = src[y0 * n + x1];
    let c = src[y1 * n + x0];
    let d = src[y1 * n + x1];
    a * (1.0 - fx) * (1.0 - fy) + b * fx * (1.0 - fy) + c * (1.0 - fx) * fy + d * fx * fy
}

/// 零均值 + L2 归一(便于点积即 NCC)。
fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let len = v.len().max(1) as f32;
    let mean = v.iter().sum::<f32>() / len;
    for x in &mut v {
        *x -= mean;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-6 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// 各平台常见 CJK 字体候选路径。
fn cjk_font_candidates() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    let list = [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/Supplemental/Songti.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
    ];
    #[cfg(target_os = "windows")]
    let list = [
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\simsun.ttc",
        "C:\\Windows\\Fonts\\msyh.ttf",
    ];
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let list = [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
        "/usr/share/fonts/truetype/arphic/uming.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    ];
    list.iter().map(PathBuf::from).collect()
}

/// **真样本模板库**:载入已标注的**真实字图**(每字若干张),按最近邻给"某框是某字"的相似度。
/// 比渲染字体模板更贴合目标验证码的具体字体——对**小固定字表**(如易盾试用 ~10 字)几张真样本即奏效,
/// 无需训练/torch。目录结构:`{dir}/{字}/任意.png`(子目录名即该字)。
pub struct SampleBank {
    samples: Vec<Sample>,
}

/// 一条真样本:字、`N×N` 归一梯度特征、原图 **FNV-1a 内容哈希**(留一法据此识别并排除「自己」)。
struct Sample {
    ch: char,
    feat: Vec<f32>,
    hash: u64,
}

impl SampleBank {
    /// 从目录加载:每个子目录名是一个字,内含该字的若干样本图(png/jpg/bmp)。
    pub fn from_dir(dir: &Path) -> Result<Self> {
        let mut samples = Vec::new();
        let rd = std::fs::read_dir(dir).map_err(|e| Error::msg(format!("样本库目录: {e}")))?;
        for ent in rd.flatten() {
            let p = ent.path();
            if !p.is_dir() {
                continue;
            }
            let Some(ch) = p
                .file_name()
                .and_then(|s| s.to_str())
                .and_then(|s| s.chars().next())
            else {
                continue;
            };
            let Ok(files) = std::fs::read_dir(&p) else {
                continue;
            };
            for f in files.flatten() {
                let fp = f.path();
                let ok_ext = fp
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|e| {
                        matches!(
                            e.to_ascii_lowercase().as_str(),
                            "png" | "jpg" | "jpeg" | "bmp"
                        )
                    })
                    .unwrap_or(false);
                if !ok_ext {
                    continue;
                }
                if let Ok(bytes) = std::fs::read(&fp)
                    && let Ok(img) = image::load_from_memory(&bytes)
                {
                    samples.push(Sample {
                        ch,
                        feat: crop_feat(&img),
                        hash: content_hash(&bytes),
                    });
                }
            }
        }
        if samples.is_empty() {
            return Err(Error::msg("样本库为空(目录下需有 {字}/ 子目录及样本图)"));
        }
        Ok(Self { samples })
    }

    /// 样本总数。
    pub fn len(&self) -> usize {
        self.samples.len()
    }
    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
    /// 是否含某字的样本。
    pub fn has_char(&self, ch: char) -> bool {
        self.samples.iter().any(|s| s.ch == ch)
    }

    /// 某框梯度特征(由 [`crop_feat`])对字 `ch` 的最大相似度:对该字所有样本 × 几个旋转取最大,夹到 `[0,1]`。
    pub fn similarity(&self, crop_feat: &[f32], ch: char) -> f32 {
        self.best_similarity(crop_feat, ch, None)
    }

    /// [`similarity`](Self::similarity) 的内核,但可**排除内容哈希等于 `exclude` 的样本**。
    /// `exclude=Some(h)` 即留一法:评测时排除「待测图自己」那条,避免库就是用这些图建的导致**自我匹配**得到虚高 1.0。
    fn best_similarity(&self, crop_feat: &[f32], ch: char, exclude: Option<u64>) -> f32 {
        if !self.has_char(ch) {
            return 0.0;
        }
        // 预旋转查询特征(吸收角度差;真样本本身也含多种角度)。
        let queries: Vec<Vec<f32>> = ROTS
            .iter()
            .map(|&d| {
                if d == 0.0 {
                    crop_feat.to_vec()
                } else {
                    normalize(rotate_grid(crop_feat, N, d))
                }
            })
            .collect();
        let mut best = 0f32;
        for sample in &self.samples {
            if sample.ch != ch || Some(sample.hash) == exclude {
                continue;
            }
            for q in &queries {
                let s = dot(q, &sample.feat);
                if s > best {
                    best = s;
                }
            }
        }
        best.clamp(0.0, 1.0)
    }

    /// 对一张图(PNG/JPEG 等**字节**)算它是字 `ch` 的模板相似度:自动解码 + 取梯度特征 →
    /// [`similarity`](Self::similarity)。便于无需先手算 `crop_feat` 的调用方(如离线精度自评)。
    pub fn similarity_image(&self, image: &[u8], ch: char) -> Result<f32> {
        let img = image::load_from_memory(image)
            .map_err(|e| Error::msg(format!("样本图解码失败: {e}")))?;
        Ok(self.best_similarity(&crop_feat(&img), ch, None))
    }

    /// **留一法**相似度:同 [`similarity_image`](Self::similarity_image),但排除与本图**内容完全相同**的
    /// 样本(按 FNV-1a 内容哈希识别「自己」)。用于「样本库正是用这些图建的」场景下做**无泄漏**精度自评——
    /// 否则待测图会匹配到库里的自己得到虚高 1.0,精度被夸大。
    pub fn similarity_image_excluding(&self, image: &[u8], ch: char) -> Result<f32> {
        let img = image::load_from_memory(image)
            .map_err(|e| Error::msg(format!("样本图解码失败: {e}")))?;
        Ok(self.best_similarity(&crop_feat(&img), ch, Some(content_hash(image))))
    }

    /// 把一张**已知真值**的单字图(PNG 字节)存进样本库目录 `dir/{ch}/`,**内容寻址去重**(同图只存一次),
    /// 返回 `Some(写入路径)`;命中去重则 `None`。
    ///
    /// 用于**「过盾即验真」自动采样**:易盾 check 回 `result:true` 时,提示 `front` 的字序就是各点中框的
    /// 真值——把这些框裁出来按 `{字}/` 落盘,即得**零人工、标签已验证**的真样本,bank 越跑越厚、模板信号
    /// 越准(见 [`SampleBank::similarity`])。配合 [`crate::ocr::ClickWord::harvest_verified`] 一行采集。
    pub fn save_labeled(dir: &Path, ch: char, crop_png: &[u8]) -> Result<Option<PathBuf>> {
        let sub = dir.join(ch.to_string());
        std::fs::create_dir_all(&sub).map_err(|e| Error::msg(format!("样本库建目录: {e}")))?;
        // **内容寻址**文件名:同一张图无论来自种子还是采样都得到同一文件名 ⇒ 天然去重、不重复堆积。
        let path = sub.join(format!("{:016x}.png", content_hash(crop_png)));
        if path.exists() {
            return Ok(None); // 去重:同一张图已存过。
        }
        std::fs::write(&path, crop_png).map_err(|e| Error::msg(format!("样本写盘: {e}")))?;
        Ok(Some(path))
    }
}

/// FNV-1a 64 位内容哈希(**确定性**、无外部依赖):仅用于真样本去重的文件名,碰撞概率对此用途可忽略
/// (即便碰撞也只是少存一张近似图)。
fn content_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_zero_mean_unit_norm() {
        let v = normalize(vec![1.0, 2.0, 3.0, 4.0]);
        let mean: f32 = v.iter().sum::<f32>() / v.len() as f32;
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(mean.abs() < 1e-5);
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn dot_self_is_one_for_normalized() {
        let v = normalize(vec![0.0, 1.0, 0.0, 2.0, 0.0, 3.0]);
        assert!((dot(&v, &v) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn rotate_zero_is_identity() {
        let mut src = vec![0f32; N * N];
        src[10 * N + 12] = 1.0;
        let r = rotate_grid(&src, N, 0.0);
        assert!((r[10 * N + 12] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn save_labeled_dedupes_and_routes_by_char() {
        let dir = std::env::temp_dir().join(format!("drission_bank_{}", content_hash(b"unique-seed")));
        let _ = std::fs::remove_dir_all(&dir);
        // 第一次写入返回路径且落到 `{字}/` 子目录;同图第二次去重返回 None。
        let png = b"\x89PNG\r\n\x1a\n-fake-but-stable-bytes";
        let first = SampleBank::save_labeled(&dir, '特', png).unwrap();
        assert!(first.is_some());
        let p = first.unwrap();
        assert!(p.exists());
        assert_eq!(p.parent().unwrap().file_name().unwrap(), "特");
        assert!(SampleBank::save_labeled(&dir, '特', png).unwrap().is_none());
        // 不同图 → 不同文件名(不去重)。
        assert!(SampleBank::save_labeled(&dir, '特', b"other-bytes").unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn content_hash_is_deterministic() {
        assert_eq!(content_hash(b"abc"), content_hash(b"abc"));
        assert_ne!(content_hash(b"abc"), content_hash(b"abd"));
    }
}
