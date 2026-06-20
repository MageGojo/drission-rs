#![cfg_attr(docsrs, feature(doc_auto_cfg))]
//! # drission
//!
//! 一个 **Rust** 编写的浏览器自动化库,提供与 [DrissionPage](https://github.com/g1879/DrissionPage)
//! 一致的简洁语法。**双后端**:
//!
//! - **Chromium / CDP**(默认,`--features cdp`):驱动或接管 Chrome / Edge / Brave / Electron 应用。
//! - **Camoufox / Firefox(Juggler)**(`--features camoufox`):反检测浏览器,附带自动过盾 / 吐环境 /
//!   滑块识别 / 高并发池 / Session(HTTP)双模等**全部高层能力**(`Page`/`WebPage`/`SessionPage`…)。
//!
//! 默认构建 = **纯 CDP**(最精简,不含任何 Camoufox 代码);要用 Camoufox 及其高层能力,请显式开
//! `--features camoufox`。两后端可同时开(`--features camoufox,cdp`)。
//!
//! ## 设计目标
//! - **语法像 DP**:`tab.get(url)`、`tab.ele("@id:kw")`、`ele.input(..)`、`ele.click()`、`tab.listen` 等。
//! - **高性能 / 并发**:基于 `tokio` 异步,多标签可并发操作。
//!
//! ## 后端无关模块(始终编译)
//! - [`codec`]:线格式编解码。[`protocol`]:连接 / 请求响应 / 事件。[`transport`]:管道 / WebSocket。
//! - [`locator`]:DP 风格元素定位语法解析。[`keys`]:键名常量与按键序列。
//! - [`net`]:网络监听 / 拦截共享数据类型。[`scrape`]:采集导出(CSV/JSON)。[`error`]:统一错误。
//!
//! > 更新历史见 [`CHANGELOG.md`](https://github.com/MageGojo/drission-rs/blob/main/CHANGELOG.md);
//! > 设计与 API 映射文档见 [`docs/`](https://github.com/MageGojo/drission-rs/tree/main/docs)。

/// Camoufox / Firefox(Juggler)后端 + 全部高层浏览器能力。仅 `--features camoufox`。
#[cfg(feature = "camoufox")]
pub mod browser;
/// Chromium(Chrome/Edge/Brave/Electron)后端,经 CDP。**默认开启**(`default = ["cdp"]`)。
#[cfg(feature = "cdp")]
pub mod cdp;
pub mod codec;
/// 通用吐环境(dump browser env)后端无关核心:探针/env.js/导出工程/同构双跑验证 + 指纹回放。
/// 两后端经 `EnvBackend` 复用同一套逻辑;`tab.dump_env()` 各自提供。
#[cfg(any(feature = "camoufox", feature = "cdp"))]
pub mod envkit;
pub mod error;
pub mod human;
pub mod keys;
/// Camoufox 启动选项 / 指纹配置 / 自动下载分发。仅 `--features camoufox`。
#[cfg(feature = "camoufox")]
pub mod launcher;
pub mod locator;
pub mod net;
#[cfg(feature = "ocr")]
pub mod ocr;
/// 大道至简 `Page` 一行起步门面(Camoufox 后端)。仅 `--features camoufox`。
#[cfg(feature = "camoufox")]
pub mod page;
/// 高并发浏览器池 / 代理池 / 指纹池 / 断点续抓。
/// 后端无关核心(`RetryPolicy`/`RotateStrategy`/`Checkpoint`)对 camoufox / cdp 均编译;
/// `BrowserPool`(camoufox)与 `ChromiumPool`(cdp,见 `cdp::pool`)各按后端提供。
#[cfg(any(feature = "camoufox", feature = "cdp"))]
pub mod pool;
pub mod protocol;
pub mod scrape;
/// HTTP Session(不开浏览器)+ 与浏览器 cookie 互通(Camoufox 后端)。仅 `--features camoufox`。
#[cfg(feature = "camoufox")]
pub mod session;
/// 静态(离线)元素解析(后端无关,基于 `scraper`):`StaticElement` 的 `s_ele`/`s_eles`/`table`。
pub mod static_element;
pub mod transport;
pub(crate) mod util;
/// WebPage 双模门面(Driver/Session,Camoufox 后端)。仅 `--features camoufox`。
#[cfg(feature = "camoufox")]
pub mod web_page;
/// 静态 XPath 1.0 子集求值器(后端无关,供 `StaticElement` 的 `xpath:` 查询)。
pub(crate) mod xpath;

pub use error::{Error, Result};

/// 常用类型一站式导入。
///
/// ```
/// use drission::prelude::*;
/// let _f = ListenFilter::default(); // 后端无关类型始终可用
/// ```
pub mod prelude {
    // ── 后端无关(始终可用)──────────────────────────────────────────
    pub use crate::error::{Error, Result};
    pub use crate::human::{HumanClickOpts, Humanize, ImageView, fetch_image};
    pub use crate::keys::{KeyInput, Keys};
    pub use crate::locator::{Query, parse as parse_locator};
    pub use crate::net::{DataPacket, ListenFilter, RequestData, ResponseData, ResumeOptions};
    pub use crate::scrape::{records_to_csv, records_to_json, rows_to_csv, write_csv, write_json};
    pub use crate::static_element::StaticElement;

    // ════════════════════════════════════════════════════════════════════
    // 统一接口(canonical):同一份用户代码,切 feature 即换协议后端。
    //   `Browser`/`Tab`/`Element`/`Page`/`Listen`/`Intercept`/`BrowserOptions`…
    // 解析规则:**cdp 优先**(两后端都开时 cdp 胜出);仅 camoufox 时为 camoufox。
    // 另一后端始终可用其显式名(`Chromium*` / `Camoufox*`)。
    // ════════════════════════════════════════════════════════════════════

    // ── 统一名:cdp 在场即为 cdp ────────────────────────────────────
    /// 高并发池(cdp canonical):`Pool` 始终随 cdp;`PoolOptions`/`ContextOverride` 仅 cdp-only 时
    /// 作 canonical(两后端并存时这两个名归 camoufox,cdp 用 `Chromium*` 显式名)。
    #[cfg(feature = "cdp")]
    pub use crate::cdp::ChromiumPool as Pool;
    #[cfg(feature = "cdp")]
    pub use crate::cdp::{
        CdpIntercept as Intercept, CdpInterceptedRequest as InterceptedRequest,
        CdpListen as Listen, ChromiumActions as Actions, ChromiumBrowser as Browser,
        ChromiumConsole as Console, ChromiumDownloads as Downloads, ChromiumElement as Element,
        ChromiumElementRect as ElementRect, ChromiumElementWait as ElementWait,
        ChromiumFrame as Frame, ChromiumOptions as BrowserOptions, ChromiumPage as Page,
        ChromiumScreencast as Screencast, ChromiumScroll as Scroll, ChromiumSetTab as SetTab,
        ChromiumShadowRoot as ShadowRoot, ChromiumTab as Tab, ChromiumWait as Wait,
        ChromiumWindow as Window, ChromiumWsListener as WsListener,
    };
    #[cfg(all(feature = "cdp", not(feature = "camoufox")))]
    pub use crate::cdp::{
        ChromiumContextOverride as ContextOverride, ChromiumPoolOptions as PoolOptions,
    };
    /// 后端无关值类型(cdp 提供 canonical 名)。
    #[cfg(feature = "cdp")]
    pub use crate::cdp::{
        ConsoleData, ConsoleFilter, Cookie, CookieParam, DialogInfo, DownloadInfo, DownloadMission,
        DownloadState, GetOptions, ImageFormat, LoadMode, PageRect, ShotOpts, WsDirection,
        WsFilter, WsMessage,
    };
    // ── 统一名:仅 camoufox(无 cdp)时为 camoufox ──────────────────
    #[cfg(all(feature = "camoufox", not(feature = "cdp")))]
    pub use crate::browser::Browser;
    #[cfg(all(feature = "camoufox", not(feature = "cdp")))]
    pub use crate::browser::{
        Actions, Console, ConsoleData, ConsoleFilter, Cookie, CookieParam, DialogInfo,
        DownloadInfo, DownloadMission, DownloadState, Downloads, Element, ElementRect, ElementWait,
        Frame, GetOptions, ImageFormat, Intercept, InterceptedRequest, Listen, LoadMode, PageRect,
        Screencast, Scroll, SetTab, ShadowRoot, ShotOpts, Tab, Wait, Window, WsDirection, WsFilter,
        WsListener, WsMessage,
    };
    #[cfg(all(feature = "camoufox", not(feature = "cdp")))]
    pub use crate::launcher::BrowserOptions;
    #[cfg(all(feature = "camoufox", not(feature = "cdp")))]
    pub use crate::page::Page;

    // ── CDP 后端显式名(始终随 cdp 可用)───────────────────────────
    #[cfg(feature = "cdp")]
    pub use crate::cdp::{
        CdpIntercept, CdpInterceptedRequest, CdpListen, ChromiumActions, ChromiumBrowser,
        ChromiumConsole, ChromiumContextOverride, ChromiumDownloads, ChromiumElement,
        ChromiumElementRect, ChromiumElementWait, ChromiumFrame, ChromiumOptions, ChromiumPage,
        ChromiumPool, ChromiumPoolOptions, ChromiumScreencast, ChromiumScroll, ChromiumSetTab,
        ChromiumShadowRoot, ChromiumTab, ChromiumWait, ChromiumWindow, ChromiumWsListener,
    };

    // ── Camoufox 后端显式名(始终随 camoufox 可用,供两后端并存时取用)─
    #[cfg(feature = "camoufox")]
    pub use crate::browser::{
        Actions as CamoufoxActions, Browser as CamoufoxBrowser, Console as CamoufoxConsole,
        Downloads as CamoufoxDownloads, Element as CamoufoxElement,
        ElementRect as CamoufoxElementRect, ElementWait as CamoufoxElementWait,
        Frame as CamoufoxFrame, Intercept as CamoufoxIntercept,
        InterceptedRequest as CamoufoxInterceptedRequest, Listen as CamoufoxListen,
        Screencast as CamoufoxScreencast, Scroll as CamoufoxScroll, SetTab as CamoufoxSetTab,
        ShadowRoot as CamoufoxShadowRoot, Tab as CamoufoxTab, Wait as CamoufoxWait,
        Window as CamoufoxWindow, WsListener as CamoufoxWsListener,
    };
    #[cfg(feature = "camoufox")]
    pub use crate::launcher::BrowserOptions as CamoufoxOptions;
    #[cfg(feature = "camoufox")]
    pub use crate::page::Page as CamoufoxPage;

    // ── Camoufox 后端独有能力(不与 cdp 冲突的类型,始终随 camoufox)─
    #[cfg(feature = "camoufox")]
    pub use crate::browser::{
        BrowserServer, ConsoleSteps, ContextOverride, ListenStream, MouseButton, OriginStorage,
        ScreencastMode, StorageState, WsSocket, WsSteps,
    };
    // ── 通用吐环境(envkit;值类型后端无关,Dumper/Probe 按后端 canonical)──
    #[cfg(all(feature = "camoufox", not(feature = "cdp")))]
    pub use crate::browser::{EnvDumper, EnvProbe};
    #[cfg(feature = "camoufox")]
    pub use crate::browser::{EnvDumper as CamoufoxEnvDumper, EnvProbe as CamoufoxEnvProbe};
    /// 滑块/缺口识别类型(`--features slider`,自动带入 camoufox)。
    #[cfg(feature = "slider")]
    pub use crate::browser::{
        GapMethod, ImageSource, SliderConfig, SliderGap, SliderResult, SuccessCheck,
    };
    /// 吐环境构建器/句柄 canonical:cdp 在场为 cdp,否则 camoufox;另一后端用 `Camoufox*`/`Chromium*`。
    #[cfg(feature = "cdp")]
    pub use crate::cdp::{
        ChromiumEnvDumper, ChromiumEnvDumper as EnvDumper, ChromiumEnvProbe,
        ChromiumEnvProbe as EnvProbe,
    };
    /// 吐环境值类型(后端无关,camoufox / cdp 共用)。
    #[cfg(any(feature = "camoufox", feature = "cdp"))]
    pub use crate::envkit::{EnvDump, EnvScope, EnvTarget};
    #[cfg(feature = "camoufox")]
    pub use crate::launcher::{Fingerprint, Geolocation, OsType, Proxy};
    #[cfg(feature = "camoufox")]
    pub use crate::pool::{
        BrowserPool, FingerprintPool, FingerprintProfile, PoolOptions, ProxyGeo, ProxyHealth,
        ProxyPool,
    };
    /// 后端无关并发原语(camoufox `BrowserPool` / cdp `ChromiumPool` 共用)。
    #[cfg(any(feature = "camoufox", feature = "cdp"))]
    pub use crate::pool::{Checkpoint, RetryPolicy, RotateStrategy};
    #[cfg(feature = "camoufox")]
    pub use crate::session::{PostData, SessionOptions, SessionPage};
    #[cfg(feature = "camoufox")]
    pub use crate::web_page::{PageMode, WebPage};

    // ── 验证码 OCR(--features ocr)──────────────────────────────────
    #[cfg(feature = "ocr")]
    pub use crate::ocr::{BBox, ClickHit, ClickWord, Det, GlyphMatcher, Ocr, SampleBank};
}
