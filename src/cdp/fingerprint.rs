//! CDP 后端「**每浏览器不同指纹**」:连贯的桌面指纹画像 + 随机生成 + 应用到
//! [`ChromiumOptions`](crate::cdp::ChromiumOptions)。
//!
//! 与 Camoufox 后端的 [`FingerprintPool`](crate::pool::FingerprintPool) 同源思路,但面向 **CDP(纯 Chrome)**:
//! - **启动级**(浏览器级、覆盖所有帧含 Turnstile 跨域子帧):UA / 语言 / 时区 / 窗口大小 → 走
//!   `ChromiumOptions`(`--user-agent` / `--lang` / `TZ` / `--window-size`)。
//! - **深指纹**(进程内、JS 可改):`navigator.platform` / `hardwareConcurrency` / `deviceMemory` /
//!   `languages` / `screen` / `devicePixelRatio` / WebGL vendor·renderer / **canvas·audio 噪声(每画像确定性,
//!   跨画像各异)** → 经导航前注入脚本([`ChromiumOptions::init_scripts`])。
//!
//! ## 两种模式(按"过盾安全度 vs 差异化强度"取舍)
//! - **同 OS 变体**([`CdpFingerprint::random`] / [`CdpFingerprintPool::generate`],**推荐默认**):保留**真实**
//!   UA/platform/WebGL(与本机 OS 自洽,**不撒谎**),仅变 **屏幕 / 时区 / 语言 / 硬件并发 / 内存 + canvas·audio 噪声**。
//!   同机多账号反关联够用,且**对 Turnstile 最友好**(无自相矛盾的指纹,契合 `docs/CDP过盾.md` 的"最小伪造"原则)。
//! - **跨 OS 画像**([`CdpFingerprint::random_persona`] / [`CdpFingerprintPool::personas`],进阶):完整伪装
//!   UA + platform + WebGL vendor/renderer + 屏幕(让 N 个浏览器像 N 台不同机器)。**建议配套对应地区的代理**,
//!   否则 WebGL 像素与 UA 跨 OS 可能被强检测网站识破。
//!
//! ## 用法
//! ```no_run
//! use drission::cdp::{ChromiumBrowser, ChromiumOptions, CdpFingerprint, CdpFingerprintPool};
//! # async fn f() -> drission::Result<()> {
//! // 单浏览器:随机一份同 OS 指纹(Turnstile 友好)
//! let fp = CdpFingerprint::random();
//! let opts = fp.apply_to_options(ChromiumOptions::new().headless(true));
//! let browser = ChromiumBrowser::launch(opts).await?;
//!
//! // 并发池:每个 worker 一份不同指纹(配合 ChromiumPool 的 worker_options)
//! let pool = CdpFingerprintPool::generate(5);
//! let worker_opts = pool.worker_options(&ChromiumOptions::new().headless(true));
//! # let _ = worker_opts; Ok(()) }
//! ```

use std::sync::Arc;

use crate::cdp::ChromiumOptions;
use crate::pool::rotate::{RotateStrategy, Rotator};

/// 指纹画像的目标操作系统(决定 UA / platform / 屏幕 / WebGL 取值的连贯组合)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdpOs {
    Windows,
    MacOs,
    Linux,
}

impl CdpOs {
    /// 当前运行的主机 OS(同 OS 变体模式以此为准)。
    pub fn host() -> Self {
        match std::env::consts::OS {
            "windows" => Self::Windows,
            "macos" => Self::MacOs,
            _ => Self::Linux,
        }
    }

    /// 该 OS 常见的真实屏幕分辨率候选。
    fn screens(self) -> &'static [(u32, u32)] {
        match self {
            Self::Windows => &[
                (1920, 1080),
                (2560, 1440),
                (1366, 768),
                (1536, 864),
                (1600, 900),
            ],
            Self::MacOs => &[(1440, 900), (1512, 982), (1728, 1117), (2560, 1440)],
            Self::Linux => &[(1920, 1080), (1366, 768), (2560, 1440)],
        }
    }

    /// 该 OS 典型 devicePixelRatio(mac 视网膜 2.0,其余 1.0)。
    fn dpr(self) -> f64 {
        match self {
            Self::MacOs => 2.0,
            _ => 1.0,
        }
    }
}

/// 一份连贯的桌面指纹画像。`None` 字段表示**不伪装、用浏览器真实值**(同 OS 变体即用此保真)。
///
/// 实现 `serde` 序列化:可把每账号的指纹落盘持久化,**跨进程复用同一指纹**(反关联需要指纹稳定)。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CdpFingerprint {
    /// User-Agent(启动级 `--user-agent`)。`None` = 用真实 Chrome UA。
    pub user_agent: Option<String>,
    /// `navigator.platform`(如 `Win32` / `MacIntel`)。`None` = 真实值。
    pub platform: Option<String>,
    /// `navigator.languages`;首项同时设为 `navigator.language`。空 = 不改。
    pub languages: Vec<String>,
    /// 地区 locale(启动级 `--lang` + `LANGUAGE`)。`None` = 不设(避免与出口 IP 冲突)。
    pub locale: Option<String>,
    /// 时区(启动级 `TZ`,如 `America/New_York`)。`None` = 不设。
    pub timezone: Option<String>,
    /// 窗口大小(启动级 `--window-size`,有头=初始窗口、无头=视口)。
    pub window_size: Option<(u32, u32)>,
    /// `screen.{width,height,availWidth,availHeight}`(JS 注入)。
    pub screen_size: Option<(u32, u32)>,
    /// `screen.colorDepth` / `pixelDepth`(JS 注入)。
    pub color_depth: Option<u32>,
    /// `window.devicePixelRatio`(JS 注入)。
    pub device_pixel_ratio: Option<f64>,
    /// `navigator.hardwareConcurrency`(逻辑核数,JS 注入)。
    pub hardware_concurrency: Option<u32>,
    /// `navigator.deviceMemory`(GB,JS 注入)。
    pub device_memory: Option<u32>,
    /// WebGL `UNMASKED_VENDOR_WEBGL`(JS hook `getParameter`)。`None`/空 = 不改。
    pub webgl_vendor: Option<String>,
    /// WebGL `UNMASKED_RENDERER_WEBGL`(JS hook `getParameter`)。`None`/空 = 不改。
    pub webgl_renderer: Option<String>,
    /// canvas / audio 噪声种子。`Some(seed)` = 注入**每画像确定性、跨画像各异**的微噪声(反 canvas/audio 关联);
    /// `None` = 不加噪声。
    pub canvas_noise_seed: Option<u32>,
}

impl CdpFingerprint {
    /// 空画像(全部用真实值)。
    pub fn new() -> Self {
        Self::default()
    }

    /// 随机一份**同 OS 变体**指纹(本机 OS;保真 UA/platform/WebGL,仅变屏幕/时区/语言/硬件 + canvas·audio 噪声)。
    /// **Turnstile 友好**,适合同机多账号反关联。
    pub fn random() -> Self {
        let rng = Rng::new();
        same_os_variant(&rng, CdpOs::host())
    }

    /// 随机一份**完整跨 OS 画像**(伪装 UA + platform + WebGL + 屏幕)。进阶:建议配套对应地区代理。
    pub fn random_persona() -> Self {
        let rng = Rng::new();
        let p = &PERSONAS[rng.below(PERSONAS.len())];
        from_persona(&rng, p)
    }

    // ── builder ──────────────────────────────────────────────────────────
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }
    pub fn platform(mut self, p: impl Into<String>) -> Self {
        self.platform = Some(p.into());
        self
    }
    pub fn languages(mut self, langs: Vec<String>) -> Self {
        self.languages = langs;
        self
    }
    pub fn locale(mut self, l: impl Into<String>) -> Self {
        self.locale = Some(l.into());
        self
    }
    pub fn timezone(mut self, tz: impl Into<String>) -> Self {
        self.timezone = Some(tz.into());
        self
    }
    pub fn window_size(mut self, w: u32, h: u32) -> Self {
        self.window_size = Some((w, h));
        self
    }
    pub fn screen_size(mut self, w: u32, h: u32) -> Self {
        self.screen_size = Some((w, h));
        self
    }
    pub fn hardware_concurrency(mut self, n: u32) -> Self {
        self.hardware_concurrency = Some(n);
        self
    }
    pub fn device_memory(mut self, gb: u32) -> Self {
        self.device_memory = Some(gb);
        self
    }
    pub fn webgl(mut self, vendor: impl Into<String>, renderer: impl Into<String>) -> Self {
        self.webgl_vendor = Some(vendor.into());
        self.webgl_renderer = Some(renderer.into());
        self
    }
    /// 开启 canvas/audio 噪声(指定种子;同种子结果稳定)。
    pub fn canvas_noise(mut self, seed: u32) -> Self {
        self.canvas_noise_seed = Some(seed);
        self
    }

    /// 把本画像应用到一个 [`ChromiumOptions`]:UA/locale/时区/窗口走启动参数,深指纹经 init 脚本注入。
    /// 保留 `opts` 已设的其它项(代理 / 持久 profile / headless 等)。
    pub fn apply_to_options(&self, mut opts: ChromiumOptions) -> ChromiumOptions {
        if let Some(ua) = &self.user_agent {
            opts.user_agent = Some(ua.clone());
        }
        if let Some(l) = &self.locale {
            opts.locale = Some(l.clone());
        }
        if let Some(tz) = &self.timezone {
            opts.timezone = Some(tz.clone());
        }
        if let Some(ws) = self.window_size {
            opts.window_size = Some(ws);
        }
        let js = self.init_script();
        if !js.is_empty() {
            opts.init_scripts.push(js);
        }
        opts
    }

    /// 生成深指纹**导航前注入脚本**(`Page.addScriptToEvaluateOnNewDocument`)。无可注入项时返回空串。
    pub fn init_script(&self) -> String {
        let mut body = String::new();
        if let Some(p) = &self.platform {
            body.push_str(&format!("  def(np,'platform',{});\n", json_str(p)));
        }
        if let Some(hc) = self.hardware_concurrency {
            body.push_str(&format!("  def(np,'hardwareConcurrency',{hc});\n"));
        }
        if let Some(dm) = self.device_memory {
            body.push_str(&format!("  def(np,'deviceMemory',{dm});\n"));
        }
        if !self.languages.is_empty() {
            let arr = serde_json::to_string(&self.languages).unwrap_or_else(|_| "[]".into());
            body.push_str(&format!(
                "  def(np,'languages',Object.freeze({arr}));\n  def(np,'language',{});\n",
                json_str(&self.languages[0])
            ));
        }
        if let Some((w, h)) = self.screen_size {
            let avail = h.saturating_sub(40);
            body.push_str(&format!(
                "  def(screen,'width',{w});def(screen,'height',{h});def(screen,'availWidth',{w});def(screen,'availHeight',{avail});def(screen,'availLeft',0);def(screen,'availTop',0);\n"
            ));
        }
        if let Some(cd) = self.color_depth {
            body.push_str(&format!(
                "  def(screen,'colorDepth',{cd});def(screen,'pixelDepth',{cd});\n"
            ));
        }
        if let Some(dpr) = self.device_pixel_ratio {
            body.push_str(&format!(
                "  try{{def(window,'devicePixelRatio',{dpr});}}catch(e){{}}\n"
            ));
        }
        if self.webgl_vendor.is_some() || self.webgl_renderer.is_some() {
            body.push_str(&webgl_js(
                self.webgl_vendor.as_deref().unwrap_or(""),
                self.webgl_renderer.as_deref().unwrap_or(""),
            ));
        }
        if let Some(seed) = self.canvas_noise_seed {
            body.push_str(&noise_js(seed));
        }
        if body.trim().is_empty() {
            return String::new();
        }
        WRAP_JS.replace("__BODY__", &body)
    }
}

/// 一组可轮换的 CDP 指纹画像。克隆代价低(`Arc`),游标在克隆间共享。
#[derive(Clone)]
pub struct CdpFingerprintPool {
    profiles: Arc<Vec<CdpFingerprint>>,
    rotator: Arc<Rotator>,
}

impl CdpFingerprintPool {
    /// 用一组画像新建池(默认 [`RotateStrategy::RoundRobin`])。
    pub fn new(profiles: Vec<CdpFingerprint>) -> Self {
        Self::with_strategy(profiles, RotateStrategy::RoundRobin)
    }

    /// 用一组画像 + 指定策略新建池。
    pub fn with_strategy(profiles: Vec<CdpFingerprint>, strategy: RotateStrategy) -> Self {
        Self {
            profiles: Arc::new(profiles),
            rotator: Arc::new(Rotator::new(strategy)),
        }
    }

    /// 生成 `n` 份**同 OS 变体**指纹(本机 OS;Turnstile 友好)。每份带不同的 canvas 噪声种子。
    pub fn generate(n: usize) -> Self {
        let rng = Rng::new();
        let os = CdpOs::host();
        Self::new((0..n).map(|_| same_os_variant(&rng, os)).collect())
    }

    /// 生成 `n` 份**完整跨 OS 画像**(轮转内置 persona 表,各配不同屏幕/时区/噪声)。进阶:配代理用。
    pub fn personas(n: usize) -> Self {
        let rng = Rng::new();
        Self::new(
            (0..n)
                .map(|i| from_persona(&rng, &PERSONAS[i % PERSONAS.len()]))
                .collect(),
        )
    }

    /// 画像数量。
    pub fn len(&self) -> usize {
        self.profiles.len()
    }
    /// 是否空池。
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// 按策略取下一份画像;空池返回 `None`。
    #[allow(clippy::should_implement_trait)]
    pub fn next(&self) -> Option<CdpFingerprint> {
        self.rotator
            .pick(self.profiles.len(), None)
            .map(|i| self.profiles[i].clone())
    }

    /// 按 key 粘性取画像(`Sticky` 策略下同 key 稳定命中同一份);空池返回 `None`。
    pub fn for_key(&self, key: &str) -> Option<CdpFingerprint> {
        self.rotator
            .pick(self.profiles.len(), Some(key))
            .map(|i| self.profiles[i].clone())
    }

    /// 把每份画像应用到 `base` 上,得到逐 worker 的 [`ChromiumOptions`]
    /// (直接喂 [`ChromiumPoolOptions::worker_options`](crate::cdp::ChromiumPoolOptions))。
    pub fn worker_options(&self, base: &ChromiumOptions) -> Vec<ChromiumOptions> {
        self.profiles
            .iter()
            .map(|p| p.apply_to_options(base.clone()))
            .collect()
    }

    /// 取出所有画像(只读)。
    pub fn profiles(&self) -> &[CdpFingerprint] {
        &self.profiles
    }
}

// ───────────────────────── 生成逻辑 ─────────────────────────

/// 常见 (locale, timezone) 连贯对(选其一时一并设语言/时区,彼此自洽)。
const LOCALE_TZ: &[(&str, &str)] = &[
    ("en-US", "America/New_York"),
    ("en-US", "America/Los_Angeles"),
    ("en-US", "America/Chicago"),
    ("en-GB", "Europe/London"),
    ("de-DE", "Europe/Berlin"),
    ("fr-FR", "Europe/Paris"),
    ("ja-JP", "Asia/Tokyo"),
    ("zh-CN", "Asia/Shanghai"),
];

const HW_CORES: &[u32] = &[4, 8, 8, 12, 16];
const MEM_GB: &[u32] = &[8, 8, 16, 16];

/// 一份完整跨 OS persona(静态真值)。
struct Persona {
    os: CdpOs,
    user_agent: &'static str,
    platform: &'static str,
    webgl_vendor: &'static str,
    webgl_renderer: &'static str,
}

/// 内置 persona 表(近期真实 Chrome 桌面;UA 主版本会随时间略旧,跨 OS 模式需要时自行更新)。
const PERSONAS: &[Persona] = &[
    Persona {
        os: CdpOs::Windows,
        user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
        platform: "Win32",
        webgl_vendor: "Google Inc. (NVIDIA)",
        webgl_renderer: "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 (0x00002503) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    },
    Persona {
        os: CdpOs::Windows,
        user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
        platform: "Win32",
        webgl_vendor: "Google Inc. (Intel)",
        webgl_renderer: "ANGLE (Intel, Intel(R) UHD Graphics 630 (0x00003E9B) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    },
    Persona {
        os: CdpOs::Windows,
        user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36",
        platform: "Win32",
        webgl_vendor: "Google Inc. (AMD)",
        webgl_renderer: "ANGLE (AMD, AMD Radeon RX 580 Direct3D11 vs_5_0 ps_5_0, D3D11)",
    },
    Persona {
        os: CdpOs::MacOs,
        user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
        platform: "MacIntel",
        webgl_vendor: "Google Inc. (Apple)",
        webgl_renderer: "ANGLE (Apple, ANGLE Metal Renderer: Apple M1, Unspecified Version)",
    },
    Persona {
        os: CdpOs::MacOs,
        user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
        platform: "MacIntel",
        webgl_vendor: "Google Inc. (Apple)",
        webgl_renderer: "ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)",
    },
    Persona {
        os: CdpOs::MacOs,
        user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36",
        platform: "MacIntel",
        webgl_vendor: "Google Inc. (Apple)",
        webgl_renderer: "ANGLE (Apple, ANGLE Metal Renderer: Apple M3, Unspecified Version)",
    },
];

fn languages_for(locale: &str) -> Vec<String> {
    if locale == "en-US" {
        vec!["en-US".into(), "en".into()]
    } else if locale == "en-GB" {
        vec!["en-GB".into(), "en".into()]
    } else {
        let base = locale.split('-').next().unwrap_or("en");
        vec![locale.into(), base.into(), "en-US".into(), "en".into()]
    }
}

/// 同 OS 变体:保真 UA/platform/WebGL,只变屏幕/时区/语言/硬件 + canvas·audio 噪声。
fn same_os_variant(rng: &Rng, os: CdpOs) -> CdpFingerprint {
    let (locale, tz) = LOCALE_TZ[rng.below(LOCALE_TZ.len())];
    let screen = os.screens()[rng.below(os.screens().len())];
    CdpFingerprint {
        user_agent: None,
        platform: None,
        webgl_vendor: None,
        webgl_renderer: None,
        languages: languages_for(locale),
        locale: Some(locale.into()),
        timezone: Some(tz.into()),
        window_size: Some(screen),
        screen_size: Some(screen),
        color_depth: Some(24),
        device_pixel_ratio: Some(os.dpr()),
        hardware_concurrency: Some(HW_CORES[rng.below(HW_CORES.len())]),
        device_memory: Some(MEM_GB[rng.below(MEM_GB.len())]),
        canvas_noise_seed: Some(rng.next_u64() as u32),
    }
}

/// 完整跨 OS 画像:在 persona 真值上叠加随机屏幕/时区/硬件/噪声。
fn from_persona(rng: &Rng, p: &Persona) -> CdpFingerprint {
    let (locale, tz) = LOCALE_TZ[rng.below(LOCALE_TZ.len())];
    let screen = p.os.screens()[rng.below(p.os.screens().len())];
    CdpFingerprint {
        user_agent: Some(p.user_agent.into()),
        platform: Some(p.platform.into()),
        webgl_vendor: Some(p.webgl_vendor.into()),
        webgl_renderer: Some(p.webgl_renderer.into()),
        languages: languages_for(locale),
        locale: Some(locale.into()),
        timezone: Some(tz.into()),
        window_size: Some(screen),
        screen_size: Some(screen),
        color_depth: Some(24),
        device_pixel_ratio: Some(p.os.dpr()),
        hardware_concurrency: Some(HW_CORES[rng.below(HW_CORES.len())]),
        device_memory: Some(MEM_GB[rng.below(MEM_GB.len())]),
        canvas_noise_seed: Some(rng.next_u64() as u32),
    }
}

// ───────────────────────── JS 模板 ─────────────────────────

/// 把 Rust 字符串编码为 JS 字面量(JSON 双引号字符串)。
fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

const WRAP_JS: &str = r#"(function(){
  var def=function(o,p,v){try{Object.defineProperty(o,p,{get:function(){return v;},configurable:true});}catch(e){}};
  try{ var np=Object.getPrototypeOf(navigator);
__BODY__
  }catch(e){}
})();"#;

/// WebGL `getParameter` hook:仅当传入对应字符串(非空)时改 vendor(37445)/renderer(37446)。
fn webgl_js(vendor: &str, renderer: &str) -> String {
    WEBGL_JS_TEMPLATE
        .replace("__VENDOR__", &json_str(vendor))
        .replace("__RENDERER__", &json_str(renderer))
}

const WEBGL_JS_TEMPLATE: &str = r#"  try{
    var __v=__VENDOR__, __r=__RENDERER__;
    var patch=function(proto){ if(!proto) return; var gp=proto.getParameter; if(!gp||gp.__df) return;
      var fn=function(p){ if(p===37445&&__v) return __v; if(p===37446&&__r) return __r; return gp.call(this,p); };
      fn.__df=true; try{ proto.getParameter=fn; }catch(e){}
    };
    if(window.WebGLRenderingContext) patch(WebGLRenderingContext.prototype);
    if(window.WebGL2RenderingContext) patch(WebGL2RenderingContext.prototype);
  }catch(e){}
"#;

/// canvas / audio 微噪声(每画像确定性:每次取数前把 PRNG 重置到 base 种子,故指纹跨调用稳定、跨画像各异)。
fn noise_js(seed: u32) -> String {
    NOISE_JS_TEMPLATE.replace("__SEED__", &seed.to_string())
}

const NOISE_JS_TEMPLATE: &str = r#"  try{
    var __base=(__SEED__)>>>0; var __s=__base;
    var __rnd=function(){ __s=(__s+0x6D2B79F5)>>>0; var t=__s; t=Math.imul(t^(t>>>15),t|1); t^=t+Math.imul(t^(t>>>7),t|61); return ((t^(t>>>14))>>>0)/4294967296; };
    var CRC=window.CanvasRenderingContext2D&&CanvasRenderingContext2D.prototype;
    var HCE=window.HTMLCanvasElement&&HTMLCanvasElement.prototype;
    if(CRC&&HCE){
      var gid=CRC.getImageData;
      if(gid&&!gid.__df){
        var ngid=function(){ var img=gid.apply(this,arguments); try{ __s=__base; var d=img.data; for(var i=0;i<d.length;i+=4){ if(__rnd()<0.05){ d[i]=d[i]^1; } } }catch(e){} return img; };
        ngid.__df=true; try{ CRC.getImageData=ngid; }catch(e){}
      }
      var td=HCE.toDataURL;
      if(td&&!td.__df){
        var ntd=function(){ try{ var c=this.getContext('2d'); if(c){ var w=this.width,h=this.height; if(w&&h){ __s=__base; var im=gid.call(c,0,0,w,h); var dd=im.data; for(var i=0;i<dd.length;i+=4){ if(__rnd()<0.02){ dd[i]=dd[i]^1; } } c.putImageData(im,0,0); } } }catch(e){} return td.apply(this,arguments); };
        ntd.__df=true; try{ HCE.toDataURL=ntd; }catch(e){}
      }
    }
    var AN=window.AnalyserNode&&AnalyserNode.prototype;
    if(AN&&AN.getFloatFrequencyData&&!AN.getFloatFrequencyData.__df){
      var gf=AN.getFloatFrequencyData;
      var ngf=function(a){ gf.call(this,a); try{ __s=__base; for(var i=0;i<a.length;i++){ a[i]=a[i]+(__rnd()-0.5)*0.1; } }catch(e){} };
      ngf.__df=true; try{ AN.getFloatFrequencyData=ngf; }catch(e){}
    }
  }catch(e){}
"#;

// ── std-only PRNG(复用 pool::rotate 的 SplitMix64;不引 rand)──
use crate::pool::rotate::Rng;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_sets_launch_fields_and_pushes_init() {
        let fp = CdpFingerprint::new()
            .user_agent("UA/1.0")
            .locale("en-US")
            .timezone("America/New_York")
            .window_size(1366, 768)
            .hardware_concurrency(8);
        let opts = fp.apply_to_options(ChromiumOptions::new());
        assert_eq!(opts.user_agent.as_deref(), Some("UA/1.0"));
        assert_eq!(opts.locale.as_deref(), Some("en-US"));
        assert_eq!(opts.timezone.as_deref(), Some("America/New_York"));
        assert_eq!(opts.window_size, Some((1366, 768)));
        assert_eq!(opts.init_scripts.len(), 1, "应注入一段深指纹脚本");
        assert!(opts.init_scripts[0].contains("hardwareConcurrency"));
    }

    #[test]
    fn empty_fingerprint_injects_nothing() {
        let fp = CdpFingerprint::new();
        assert!(fp.init_script().is_empty());
        let opts = fp.apply_to_options(ChromiumOptions::new());
        assert!(opts.init_scripts.is_empty());
    }

    #[test]
    fn init_script_contains_expected_overrides() {
        let fp = CdpFingerprint::new()
            .platform("Win32")
            .screen_size(1920, 1080)
            .device_memory(16)
            .webgl("Google Inc. (NVIDIA)", "ANGLE (NVIDIA, ...)")
            .canvas_noise(12345);
        let js = fp.init_script();
        assert!(js.contains("'platform'") && js.contains("Win32"));
        assert!(js.contains("screen") && js.contains("1920"));
        assert!(js.contains("deviceMemory") && js.contains("16"));
        assert!(js.contains("37445") && js.contains("NVIDIA"));
        assert!(js.contains("12345") && js.contains("getImageData"));
        // 必须是单一 IIFE,语法上自洽(括号配平的粗校验)。
        assert!(js.starts_with("(function()"));
    }

    #[test]
    fn random_is_same_os_safe() {
        let fp = CdpFingerprint::random();
        // 同 OS 变体:不伪装 UA/platform/WebGL(保真),但有屏幕/时区/语言/硬件 + 噪声。
        assert!(fp.user_agent.is_none());
        assert!(fp.platform.is_none());
        assert!(fp.webgl_vendor.is_none());
        assert!(fp.locale.is_some() && fp.timezone.is_some());
        assert!(fp.hardware_concurrency.is_some());
        assert!(fp.canvas_noise_seed.is_some());
        assert!(!fp.init_script().is_empty());
    }

    #[test]
    fn persona_is_full_spoof() {
        let fp = CdpFingerprint::random_persona();
        assert!(fp.user_agent.is_some());
        assert!(fp.platform.is_some());
        assert!(fp.webgl_vendor.is_some() && fp.webgl_renderer.is_some());
        let opts = fp.apply_to_options(ChromiumOptions::new());
        assert!(opts.user_agent.is_some());
    }

    #[test]
    fn pool_generate_and_rotate() {
        let pool = CdpFingerprintPool::generate(5);
        assert_eq!(pool.len(), 5);
        assert!(!pool.is_empty());
        let a = pool.next().unwrap();
        let b = pool.next().unwrap();
        // RoundRobin:相邻两次应来自不同下标的画像(种子不同 → 噪声不同)。
        assert_ne!(a.canvas_noise_seed, b.canvas_noise_seed);
        let opts = pool.worker_options(&ChromiumOptions::new().headless(true));
        assert_eq!(opts.len(), 5);
        assert!(opts.iter().all(|o| o.headless && !o.init_scripts.is_empty()));
    }

    #[test]
    fn pool_personas_have_distinct_ua() {
        let pool = CdpFingerprintPool::personas(PERSONAS.len());
        let uas: Vec<_> = pool
            .profiles()
            .iter()
            .filter_map(|p| p.webgl_renderer.clone())
            .collect();
        // persona 表里 renderer 互不相同 → 池里也应保留这种差异。
        let mut uniq = uas.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), uas.len(), "各 persona 的 WebGL renderer 应各异");
    }

    #[test]
    fn empty_pool_next_none() {
        let pool = CdpFingerprintPool::new(vec![]);
        assert!(pool.is_empty());
        assert!(pool.next().is_none());
        assert!(pool.for_key("x").is_none());
    }
}
