//! # drission
//!
//! 一个 **Rust** 编写的浏览器自动化库,默认驱动 [Camoufox](https://github.com/daijro/camoufox)
//! 反检测浏览器,提供与 [DrissionPage](https://github.com/g1879/DrissionPage) 一致的
//! 简洁语法。面向高并发爬虫与自动化:多标签并发、独立 cookie、XHR 监听拦截、过盾等。
//!
//! ## 设计目标
//! - **语法像 DP**:`tab.get(url)`、`tab.ele("@id:kw")`、`ele.input(..)`、`ele.click()`、
//!   `tab.listen` 等。
//! - **默认 Camoufox + 自动分发**:首次运行自动下载对应平台的浏览器到 `~/.cache/camoufox`。
//! - **高性能 / 并发**:基于 `tokio` 异步,多标签可并发操作,各标签独立会话与 cookie。
//!
//! ## 模块
//! - [`codec`]:Juggler 线格式(null 分隔 JSON)编解码。
//! - [`protocol`]:Juggler 协议消息类型(后续:连接 / 会话 / 方法封装)。
//! - [`locator`]:DP 风格元素定位语法解析。
//! - [`launcher`]:启动选项、指纹配置、Camoufox 自动下载分发。
//! - [`error`]:统一错误类型。
//!
//! > 当前进度见 `docs/进度.md`。底层控制协议是 Firefox 的 **Juggler**(非 CDP),
//! > 因为 Camoufox 只支持 Juggler。

pub mod browser;
pub mod codec;
pub mod error;
pub mod launcher;
pub mod locator;
#[cfg(feature = "ocr")]
pub mod ocr;
pub mod pool;
pub mod protocol;
pub mod session;
pub mod transport;
pub(crate) mod util;

pub use error::{Error, Result};

/// 常用类型一站式导入。
///
/// ```
/// use drission::prelude::*;
/// let _opts = BrowserOptions::new().headless(true);
/// ```
pub mod prelude {
    pub use crate::browser::{
        Actions, Browser, BrowserServer, Console, ConsoleData, ConsoleFilter, ConsoleSteps,
        ContextOverride, Cookie, CookieParam, DataPacket, DialogInfo, DownloadInfo, DownloadMission,
        DownloadState, Downloads, Element, ElementRect, ElementWait, EnvDump, EnvDumper, EnvProbe,
        EnvScope, EnvTarget, Frame,
        GetOptions, ImageFormat, Intercept, InterceptedRequest, KeyInput, Keys, Listen, ListenFilter,
        ListenStream, LoadMode, MouseButton, OriginStorage, PageRect, Screencast, ScreencastMode,
        Scroll, SetTab,
        ShadowRoot, ShotOpts, StaticElement, StorageState, Tab, Wait, Window, WsDirection, WsFilter,
        WsListener, WsMessage, WsSocket, WsSteps,
    };
    #[cfg(feature = "slider")]
    pub use crate::browser::{GapMethod, ImageSource, SliderConfig, SliderGap, SliderResult, SuccessCheck};
    pub use crate::error::{Error, Result};
    pub use crate::launcher::{BrowserOptions, Fingerprint, Geolocation, OsType, Proxy};
    pub use crate::locator::{Query, parse as parse_locator};
    #[cfg(feature = "ocr")]
    pub use crate::ocr::Ocr;
    pub use crate::pool::{
        BrowserPool, Checkpoint, FingerprintPool, FingerprintProfile, PoolOptions, ProxyPool,
        RetryPolicy, RotateStrategy,
    };
    pub use crate::session::{PostData, SessionOptions, SessionPage};
}
