//! **拟人指针 + 图像视图**(后端无关,点选/拖拽通用,不针对任何厂商)。
//!
//! 两后端(CDP `ChromiumTab` / Camoufox `Tab`)共用同一套逻辑:
//! - **视图** [`ImageView`]:把"图像像素坐标"(检测/OCR 在图上得到的点)映射到"页面坐标"(可信点击用);
//!   `Humanize::image_view` 读某 `<img>`/元素的显示 rect + 自然尺寸 + src。
//! - **轨迹** [`Humanize::human_click`]:点与点之间走**连续曲线**(二次贝塞尔 + minimum-jerk 变速 + 手抖
//!   + 落点微停),产生密集 `mousemove/pointermove`,对冲"轨迹稀疏/机械"类行为风控(如易盾/极验点选)。
//!
//! ```no_run
//! # async fn f(tab: &drission::cdp::ChromiumTab) -> drission::Result<()> {
//! use drission::prelude::*;
//! let view = tab.image_view("img.captcha-bg").await?;       // 显示 rect + 自然尺寸 + src
//! let img = drission::human::fetch_image(&view.src).await?; // 取干净源图(无叠加 UI)
//! // …检测/识别得到图内像素点 pts_px: Vec<(u32,u32)> …
//! # let pts_px: Vec<(u32,u32)> = vec![];
//! let pts: Vec<(f64,f64)> = pts_px.iter().map(|&p| view.map_u32(p)).collect();
//! tab.human_click(&pts).await?;                             // 拟人轨迹依次点击
//! # Ok(()) }
//! ```

use std::time::Duration;

use serde_json::Value;

use crate::{Error, Result};

/// 图像视图:元素显示 rect(`x/y/w/h`,页面坐标)+ 图像自然尺寸(`natural_w/h`)+ `src`。
/// [`map`](Self::map) 把图像像素坐标按 x/y 各自缩放映射到页面坐标。
#[derive(Debug, Clone, Default)]
pub struct ImageView {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub natural_w: f64,
    pub natural_h: f64,
    /// 图源 URL(`<img>` 的 `currentSrc`/`src`;非 img 元素为空)。
    pub src: String,
}

impl ImageView {
    /// 横向缩放(显示宽 / 自然宽);自然宽不可用时回退 1。
    pub fn scale_x(&self) -> f64 {
        if self.natural_w > 1.0 {
            self.w / self.natural_w
        } else {
            1.0
        }
    }
    /// 纵向缩放(显示高 / 自然高);自然高不可用时回退横向缩放(再回退 1)。
    pub fn scale_y(&self) -> f64 {
        if self.natural_h > 1.0 {
            self.h / self.natural_h
        } else {
            self.scale_x()
        }
    }
    /// 图像像素 `(px,py)` → 页面坐标。
    pub fn map(&self, px: f64, py: f64) -> (f64, f64) {
        (self.x + px * self.scale_x(), self.y + py * self.scale_y())
    }
    /// 同 [`map`](Self::map),接收 `(u32,u32)` 像素点(检测框中心等)。
    pub fn map_u32(&self, p: (u32, u32)) -> (f64, f64) {
        self.map(p.0 as f64, p.1 as f64)
    }
    /// 显示 rect 是否有效(宽 > 1)。
    pub fn is_valid(&self) -> bool {
        self.w > 1.0
    }
}

/// 拟人点击参数(都有合理默认)。
#[derive(Debug, Clone, Copy)]
pub struct HumanClickOpts {
    /// 每多少像素一个轨迹点(越小越密)。
    pub px_per_point: f64,
    /// 每段轨迹点数下限 / 上限。
    pub min_points: usize,
    pub max_points: usize,
    /// 弧线弓高占距离的比例(0=直线)。
    pub bow: f64,
    /// 每点手抖像素。
    pub jitter: f64,
}

impl Default for HumanClickOpts {
    fn default() -> Self {
        Self {
            px_per_point: 7.0,
            min_points: 22,
            max_points: 64,
            bow: 0.2,
            jitter: 1.3,
        }
    }
}

/// std-only xorshift 随机源(不引 `rand`)。
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    /// `[0,1)`。
    fn f(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn seed_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        | 1
}

/// 生成从 `from` 到 `to` 的拟人轨迹点 `(x, y, delay_ms)`:二次贝塞尔(随机垂直弓高)按 minimum-jerk
/// 参数取点(慢起→中段快→慢收)+ 每点手抖 + 变间隔。纯函数,便于复用/测试。
fn track_between(
    from: (f64, f64),
    to: (f64, f64),
    rng: &mut Rng,
    o: &HumanClickOpts,
) -> Vec<(f64, f64, u64)> {
    let dist = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
    let n = ((dist / o.px_per_point.max(1.0)) as usize).clamp(o.min_points, o.max_points);
    let (mx, my) = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
    let (perp_x, perp_y) = (-(to.1 - from.1), to.0 - from.0);
    let plen = (perp_x * perp_x + perp_y * perp_y).sqrt().max(1.0);
    let bow = (rng.f() - 0.5) * dist * o.bow;
    let (cx, cy) = (mx + perp_x / plen * bow, my + perp_y / plen * bow);
    let mut out = Vec::with_capacity(n);
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let s = 10.0 * t.powi(3) - 15.0 * t.powi(4) + 6.0 * t.powi(5); // minimum-jerk
        let u = 1.0 - s;
        let x = u * u * from.0 + 2.0 * u * s * cx + s * s * to.0 + (rng.f() - 0.5) * o.jitter;
        let y = u * u * from.1 + 2.0 * u * s * cy + s * s * to.1 + (rng.f() - 0.5) * o.jitter;
        out.push((x, y, 5 + (rng.f() * 11.0) as u64));
    }
    out
}

/// 读元素视图的通用 JS(任意 `<img>`/元素:显示 rect + naturalWidth/Height + src)。
const IMAGE_VIEW_JS: &str = r#"((sel)=>{const e=document.querySelector(sel);if(!e)return '';const r=e.getBoundingClientRect();
return JSON.stringify({x:r.x,y:r.y,w:r.width,h:r.height,nw:e.naturalWidth||0,nh:e.naturalHeight||0,src:e.currentSrc||e.src||''});})"#;

/// **拟人指针 + 视图**能力(CDP / Camoufox 两后端实现;`use drission::prelude::*` 后挂到 `tab` 上)。
#[async_trait::async_trait]
pub trait Humanize {
    /// 底层:可信移动 / 按下 / 抬起 / 求值(各后端委托给固有方法)。
    async fn hm_move(&self, x: f64, y: f64) -> Result<()>;
    async fn hm_down(&self, x: f64, y: f64) -> Result<()>;
    async fn hm_up(&self, x: f64, y: f64) -> Result<()>;
    async fn hm_eval(&self, js: &str) -> Result<Value>;

    /// 读 `selector` 元素的 [`ImageView`](显示 rect + 自然尺寸 + src),用于像素→页面映射与取源图。
    async fn image_view(&self, selector: &str) -> Result<ImageView> {
        let call = format!(
            "({IMAGE_VIEW_JS})({})",
            serde_json::to_string(selector).unwrap_or_default()
        );
        let v = self.hm_eval(&call).await?;
        let s = v.as_str().unwrap_or_default();
        if s.is_empty() {
            return Err(Error::msg(format!("image_view: 未找到元素 {selector}")));
        }
        let j: Value =
            serde_json::from_str(s).map_err(|e| Error::msg(format!("image_view: {e}")))?;
        Ok(ImageView {
            x: j["x"].as_f64().unwrap_or(0.0),
            y: j["y"].as_f64().unwrap_or(0.0),
            w: j["w"].as_f64().unwrap_or(0.0),
            h: j["h"].as_f64().unwrap_or(0.0),
            natural_w: j["nw"].as_f64().unwrap_or(0.0),
            natural_h: j["nh"].as_f64().unwrap_or(0.0),
            src: j["src"].as_str().unwrap_or("").to_string(),
        })
    }

    /// 依次**拟人点击**多个**页面坐标**点(默认参数)。
    async fn human_click(&self, points: &[(f64, f64)]) -> Result<()> {
        self.human_click_with(points, &HumanClickOpts::default())
            .await
    }

    /// 同 [`human_click`](Self::human_click),可定制 [`HumanClickOpts`]。点间走连续曲线轨迹(密集
    /// `mousemove`)+ 落点微停 + 可信按下/抬起;首点之前从其上方一随机点移入(模拟从别处移过来)。
    async fn human_click_with(&self, points: &[(f64, f64)], opts: &HumanClickOpts) -> Result<()> {
        if points.is_empty() {
            return Ok(());
        }
        let mut rng = Rng::new(seed_now());
        let first = points[0];
        let mut cur = (
            first.0 + (rng.f() - 0.5) * 60.0,
            first.1 - 24.0 - rng.f() * 40.0,
        );
        for &p in points {
            for (x, y, d) in track_between(cur, p, &mut rng, opts) {
                self.hm_move(x, y).await?;
                tokio::time::sleep(Duration::from_millis(d)).await;
            }
            tokio::time::sleep(Duration::from_millis(40 + (rng.f() * 90.0) as u64)).await;
            self.hm_move(p.0 + (rng.f() - 0.5) * 1.2, p.1 + (rng.f() - 0.5) * 1.2)
                .await?;
            tokio::time::sleep(Duration::from_millis(30 + (rng.f() * 40.0) as u64)).await;
            self.hm_down(p.0, p.1).await?;
            tokio::time::sleep(Duration::from_millis(45 + (rng.f() * 55.0) as u64)).await;
            self.hm_up(p.0, p.1).await?;
            cur = p;
            tokio::time::sleep(Duration::from_millis(130 + (rng.f() * 220.0) as u64)).await;
        }
        Ok(())
    }
}

/// 取图源字节(后端无关,服务端直拉、避开浏览器跨域;用于拿无 UI 叠加的**干净源图**喂检测/OCR)。
pub async fn fetch_image(url: &str) -> Result<Vec<u8>> {
    let bytes = reqwest::get(url)
        .await
        .map_err(|e| Error::msg(format!("fetch_image: {e}")))?
        .bytes()
        .await
        .map_err(|e| Error::msg(format!("fetch_image: {e}")))?;
    Ok(bytes.to_vec())
}

#[cfg(feature = "camoufox")]
#[async_trait::async_trait]
impl Humanize for crate::browser::Tab {
    async fn hm_move(&self, x: f64, y: f64) -> Result<()> {
        self.mouse_move(x, y).await
    }
    async fn hm_down(&self, x: f64, y: f64) -> Result<()> {
        self.mouse_down(x, y).await
    }
    async fn hm_up(&self, x: f64, y: f64) -> Result<()> {
        self.mouse_up(x, y).await
    }
    async fn hm_eval(&self, js: &str) -> Result<Value> {
        self.run_js(js).await
    }
}

#[cfg(feature = "cdp")]
#[async_trait::async_trait]
impl Humanize for crate::cdp::ChromiumTab {
    async fn hm_move(&self, x: f64, y: f64) -> Result<()> {
        self.mouse_move(x, y).await
    }
    async fn hm_down(&self, x: f64, y: f64) -> Result<()> {
        self.mouse_down(x, y).await
    }
    async fn hm_up(&self, x: f64, y: f64) -> Result<()> {
        self.mouse_up(x, y).await
    }
    async fn hm_eval(&self, js: &str) -> Result<Value> {
        self.run_js(js).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_view_maps_with_scale() {
        let v = ImageView {
            x: 100.0,
            y: 200.0,
            w: 320.0,
            h: 160.0,
            natural_w: 640.0,
            natural_h: 320.0,
            src: String::new(),
        };
        assert_eq!(v.scale_x(), 0.5);
        assert_eq!(v.scale_y(), 0.5);
        // 图像像素 (200,100) → 页面 (100+100, 200+50)
        assert_eq!(v.map_u32((200, 100)), (200.0, 250.0));
    }

    #[test]
    fn track_dense_and_endpoints() {
        let mut rng = Rng::new(42);
        let o = HumanClickOpts::default();
        let pts = track_between((0.0, 0.0), (300.0, 0.0), &mut rng, &o);
        // 300/7 ≈ 42 个点,落在 [min,max]。
        assert!(pts.len() >= o.min_points && pts.len() <= o.max_points);
        // 终点应接近目标(末点 t=1,仅手抖偏移)。
        let last = pts.last().unwrap();
        assert!((last.0 - 300.0).abs() < 3.0 && last.1.abs() < 3.0);
    }
}
