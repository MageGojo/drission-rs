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
pub mod error;
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
/// 高并发浏览器池 / 代理池 / 指纹池 / 断点续抓(Camoufox 后端)。仅 `--features camoufox`。
#[cfg(feature = "camoufox")]
pub mod pool;
pub mod protocol;
pub mod scrape;
/// HTTP Session(不开浏览器)+ 与浏览器 cookie 互通(Camoufox 后端)。仅 `--features camoufox`。
#[cfg(feature = "camoufox")]
pub mod session;
pub mod transport;
pub(crate) mod util;
/// WebPage 双模门面(Driver/Session,Camoufox 后端)。仅 `--features camoufox`。
#[cfg(feature = "camoufox")]
pub mod web_page;

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
    pub use crate::keys::{KeyInput, Keys};
    pub use crate::locator::{Query, parse as parse_locator};
    pub use crate::net::{DataPacket, ListenFilter, RequestData, ResponseData, ResumeOptions};
    pub use crate::scrape::{records_to_csv, records_to_json, rows_to_csv, write_csv, write_json};

    // ── Camoufox 后端(--features camoufox)──────────────────────────
    #[cfg(feature = "camoufox")]
    pub use crate::browser::{
        Actions, Browser, BrowserServer, Console, ConsoleData, ConsoleFilter, ConsoleSteps,
        ContextOverride, Cookie, CookieParam, DialogInfo, DownloadInfo, DownloadMission,
        DownloadState, Downloads, Element, ElementRect, ElementWait, EnvDump, EnvDumper, EnvProbe,
        EnvScope, EnvTarget, Frame, GetOptions, ImageFormat, Intercept, InterceptedRequest, Listen,
        ListenStream, LoadMode, MouseButton, OriginStorage, PageRect, Screencast, ScreencastMode,
        Scroll, SetTab, ShadowRoot, ShotOpts, StaticElement, StorageState, Tab, Wait, Window,
        WsDirection, WsFilter, WsListener, WsMessage, WsSocket, WsSteps,
    };
    /// 滑块/缺口识别类型(`--features slider`,自动带入 camoufox)。
    #[cfg(feature = "slider")]
    pub use crate::browser::{
        GapMethod, ImageSource, SliderConfig, SliderGap, SliderResult, SuccessCheck,
    };
    #[cfg(feature = "camoufox")]
    pub use crate::launcher::{BrowserOptions, Fingerprint, Geolocation, OsType, Proxy};
    #[cfg(feature = "camoufox")]
    pub use crate::page::Page;
    #[cfg(feature = "camoufox")]
    pub use crate::pool::{
        BrowserPool, Checkpoint, FingerprintPool, FingerprintProfile, PoolOptions, ProxyGeo,
        ProxyHealth, ProxyPool, RetryPolicy, RotateStrategy,
    };
    #[cfg(feature = "camoufox")]
    pub use crate::session::{PostData, SessionOptions, SessionPage};
    #[cfg(feature = "camoufox")]
    pub use crate::web_page::{PageMode, WebPage};

    // ── Chromium / CDP 后端(--features cdp)─────────────────────────
    /// Chromium/CDP 后端类型(默认开启;`--no-default-features` 可关)。
    #[cfg(feature = "cdp")]
    pub use crate::cdp::{
        CdpIntercept, CdpInterceptedRequest, CdpListen, ChromiumBrowser, ChromiumElement,
        ChromiumElementRect, ChromiumTab,
    };

    // ── 验证码 OCR(--features ocr)──────────────────────────────────
    #[cfg(feature = "ocr")]
    pub use crate::ocr::Ocr;
}
