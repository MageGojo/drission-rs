#![cfg_attr(docsrs, feature(doc_cfg))]
//! # drission
//!
//! 一个 **Rust** 编写的浏览器自动化库,提供与 [DrissionPage](https://github.com/g1879/DrissionPage)
//! 一致的简洁语法。**双后端**:
//!
//! - **Chromium / CDP**(默认,`--features cdp`):驱动或接管 Chrome / Edge / Brave / Electron。
//!   同一套 API 也可驱动 [Lightpanda](https://lightpanda.io):`BrowserOptions::new().lightpanda()`
//!   （服务器简单听接口；本机可视化 / 强反爬仍用 Chrome。`drs` 不接 Lightpanda）。
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

/// 无障碍快照(后端无关):`AxNode`/`AxTree` + DOM 快照脚本 + CDP 扁平树重建。
/// `tab.ax_tree()`(cdp 原生)/ `tab.ax_snapshot()`(两后端)各自提供。
pub mod a11y;
/// Agent 编排层:observe / 语义定位 / 等待。仅 cdp。见 `docs/Agent运行时.md`。
#[cfg(feature = "cdp")]
pub mod agent;
/// AI / MCP 友好页面快照(interesting-only + ref):`drs snapshot` / `browser_snapshot`。
pub mod ai_snapshot;
/// Camoufox / Firefox(Juggler)后端 + 全部高层浏览器能力。仅 `--features camoufox`。
#[cfg(feature = "camoufox")]
pub mod browser;
/// Chromium(Chrome/Edge/Brave/Electron)后端,经 CDP。**默认开启**(`default = ["cdp"]`)。
#[cfg(feature = "cdp")]
pub mod cdp;
pub mod codec;
/// 录制 → 生成代码(后端无关):`RecordedAction`/`RecordedScript` + 生成可运行 Rust / JSON。
/// 录制采集见各后端(cdp:`tab.recorder()`)。
pub mod codegen;
/// 通用吐环境(dump browser env)后端无关核心:探针/env.js/导出工程/同构双跑验证 + 指纹回放。
/// 两后端经 `EnvBackend` 复用同一套逻辑;`tab.dump_env()` 各自提供。
#[cfg(any(feature = "camoufox", feature = "cdp"))]
pub mod envkit;
pub mod error;
/// 读取浏览器**实时指纹快照**(后端无关):`tab.fingerprint_snapshot()` 一次性 dump UA/平台/语言/
/// 时区/屏幕/硬件/WebGL renderer/canvas 哈希。区别于 `cdp::fingerprint`/`pool::fingerprint`(设定指纹)。
pub mod fingerprint;
/// HTML → Markdown(`htmd`),供 AI 读页。
pub mod html_md;
pub mod human;
pub mod keys;
/// 启动选项 / 指纹配置 / 代理值类型;Camoufox 自动下载分发与进程启动。`options`(含后端无关的
/// [`Proxy`](launcher::Proxy))随任一后端编译,`fetch`/`process`(真正启动 Camoufox)仅 `--features camoufox`。
#[cfg(any(feature = "camoufox", feature = "cdp"))]
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
/// 四层运行时再导出(Browser / HTTP / AI / Task)。见 `docs/运行时分层.md`。
pub mod runtime;
pub mod scrape;
/// HTTP Session(不开浏览器)+ 与浏览器 cookie 互通(Camoufox 后端)。仅 `--features camoufox`。
#[cfg(feature = "camoufox")]
pub mod session;
/// 通用**滑块验证码**求解(后端无关核心 + [`SliderTab`](slider::SliderTab) trait,`--features slider`)。
/// 缺口算法是页面内 JS,故 camoufox `Tab` 与 cdp `ChromiumTab` 各实现一次原语即两后端通用。
#[cfg(feature = "slider")]
pub mod slider;
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
    pub use crate::a11y::{AxNode, AxTree};
    #[cfg(feature = "cdp")]
    pub use crate::agent::{
        AgentBrowser, AgentElement, AgentPage, FindOp, Observation, SemanticTarget,
    };
    pub use crate::codegen::{RecordedAction, RecordedScript};
    pub use crate::error::{Error, Result};
    pub use crate::fingerprint::{
        FingerprintProbe, FingerprintSnapshot, IdentityAdmissionAction, IdentityAdmissionPlan,
        IdentityCapacityAction, IdentityCapacityPlan, IdentityCapacityStatus, IdentityCluster,
        IdentityDriftRemediationAction, IdentityDriftRemediationPlan,
        IdentityDriftRemediationTarget, IdentityDriftReport, IdentityDriftSeverity,
        IdentityDriftSignal, IdentityEntropyBudget, IdentityEntropySignalBudget,
        IdentityEntropyStatus, IdentityFixAction, IdentityFixPlan, IdentityFixPriority,
        IdentityFixTarget, IdentityIssue, IdentityOffender, IdentityPoolDiversityBucket,
        IdentityPoolDiversityReport, IdentityPoolDiversitySignal, IdentityPoolRemediationAction,
        IdentityPoolRemediationPlan, IdentityPoolRemediationTarget, IdentityPoolReport,
        IdentityPoolSignal, IdentityQuarantinePlan, IdentityReport, IdentitySeverity,
        LinkabilityPair, LinkabilityReport, LinkabilitySignal, LinkabilityStrength,
    };
    pub use crate::human::{HumanClickOpts, Humanize, ImageView, fetch_image};
    pub use crate::keys::{KeyInput, Keys};
    pub use crate::locator::{Locator, Query, parse as parse_locator};
    pub use crate::net::{
        DataPacket, ListenFilter, NetRequest, NetResponse, RequestData, ResponseData, ResumeOptions,
    };
    pub use crate::scrape::{
        TableOptions, print_table, print_table_with_options, records_to_csv, records_to_json,
        rows_to_csv, rows_to_table, rows_to_table_with_options, write_csv, write_json,
    };
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
    /// CDP「主动逆向」能力类型(调试器/调用栈/脚本源码/Hook;Chromium 专有,仅 cdp 后端)。
    /// 详见 `docs/逆向增强.md`。
    #[cfg(feature = "cdp")]
    pub use crate::cdp::{
        CallFrame, ChildTarget, ChromiumDebugger, ChromiumHook, ChromiumScripts, HookHit,
        HookSession, PausedStack, ScriptInfo, ScriptMatch, beautify_js,
    };
    #[cfg(feature = "cdp")]
    pub use crate::cdp::{
        CdpIntercept as Intercept, CdpInterceptedRequest as InterceptedRequest,
        CdpListen as Listen, ChromiumActions as Actions, ChromiumBrowser as Browser,
        ChromiumBrowserContext as BrowserContext, ChromiumConsole as Console,
        ChromiumDownloads as Downloads, ChromiumElement as Element,
        ChromiumElementRect as ElementRect, ChromiumElementWait as ElementWait,
        CdpEngine, ChromiumFrame as Frame, ChromiumOptions as BrowserOptions, ChromiumPage as Page,
        ChromiumRecorder as Recorder, ChromiumScreencast as Screencast, ChromiumScroll as Scroll,
        ChromiumSetTab as SetTab, ChromiumShadowRoot as ShadowRoot, ChromiumTab as Tab,
        ChromiumWait as Wait, ChromiumWindow as Window, ChromiumWsListener as WsListener,
        ContextLease, ContextSpec, NetworkHandle, NetworkRoute, PageCapture, PoolLease, ProcStat,
        ProfileLease, ProfileManager, ProfileManifest, RecycleReport, TabLease, TaskScheduler,
        WaitDownload, WaitElement, WaitNetwork, WaitPopup, WaitText, WorkerResource,
    };
    #[cfg(all(feature = "cdp", not(feature = "camoufox")))]
    pub use crate::cdp::{
        ChromiumContextOverride as ContextOverride, ChromiumPoolOptions as PoolOptions,
    };
    /// 后端无关值类型(cdp 提供 canonical 名)。
    #[cfg(feature = "cdp")]
    pub use crate::cdp::{
        ConsoleData, ConsoleFilter, Cookie, CookieParam, DialogInfo, DownloadInfo, DownloadMission,
        DownloadState, GetOptions, ImageFormat, LoadMode, OriginStorage, PageRect, ShotOpts,
        StorageState, WsDirection, WsFilter, WsMessage,
    };
    /// CDP「标配补齐」能力类型(对标 Playwright/Puppeteer/DP;Chromium 专有,仅 cdp 后端)。
    #[cfg(feature = "cdp")]
    pub use crate::cdp::{
        Device, ExposedFunction, HarLog, HarNotFound, HarPlayer, HarRecorder, HarReplayOptions,
        NetworkConditions, PdfOptions,
    };
    // ── 统一名:仅 camoufox(无 cdp)时为 camoufox ──────────────────
    #[cfg(all(feature = "camoufox", not(feature = "cdp")))]
    pub use crate::browser::Browser;
    #[cfg(all(feature = "camoufox", not(feature = "cdp")))]
    pub use crate::browser::{
        Actions, Console, ConsoleData, ConsoleFilter, Cookie, CookieParam, DialogInfo,
        DownloadInfo, DownloadMission, DownloadState, Downloads, Element, ElementRect, ElementWait,
        Frame, GetOptions, ImageFormat, Intercept, InterceptedRequest, Listen, LoadMode,
        OriginStorage, PageRect, Screencast, Scroll, SetTab, ShadowRoot, ShotOpts, StorageState,
        Tab, Wait, Window, WsDirection, WsFilter, WsListener, WsMessage,
    };
    #[cfg(all(feature = "camoufox", not(feature = "cdp")))]
    pub use crate::launcher::BrowserOptions;
    #[cfg(all(feature = "camoufox", not(feature = "cdp")))]
    pub use crate::page::Page;

    // ── CDP 后端显式名(始终随 cdp 可用)───────────────────────────
    #[cfg(feature = "cdp")]
    pub use crate::cdp::{
        CdpIntercept, CdpInterceptedRequest, CdpListen, ChromiumActions, ChromiumBrowser,
        ChromiumBrowserContext, ChromiumConsole, ChromiumContextOverride, ChromiumDownloads,
        ChromiumElement, ChromiumElementRect, ChromiumElementWait, ChromiumFrame, ChromiumOptions,
        ChromiumPage, ChromiumPool, ChromiumPoolOptions, ChromiumRecorder, ChromiumScreencast,
        ChromiumScroll, ChromiumSetTab, ChromiumShadowRoot, ChromiumTab, ChromiumWait,
        ChromiumWindow, ChromiumWsListener,
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
        BrowserServer, ConsoleSteps, ContextOverride, ListenStream, MouseButton, ScreencastMode,
        WsSocket, WsSteps,
    };
    // ── 通用吐环境(envkit;值类型后端无关,Dumper/Probe 按后端 canonical)──
    #[cfg(all(feature = "camoufox", not(feature = "cdp")))]
    pub use crate::browser::{EnvDumper, EnvProbe};
    #[cfg(feature = "camoufox")]
    pub use crate::browser::{EnvDumper as CamoufoxEnvDumper, EnvProbe as CamoufoxEnvProbe};
    /// 吐环境构建器/句柄 canonical:cdp 在场为 cdp,否则 camoufox;另一后端用 `Camoufox*`/`Chromium*`。
    #[cfg(feature = "cdp")]
    pub use crate::cdp::{
        ChromiumEnvDumper, ChromiumEnvDumper as EnvDumper, ChromiumEnvProbe,
        ChromiumEnvProbe as EnvProbe,
    };
    /// 吐环境值类型(后端无关,camoufox / cdp 共用)。
    #[cfg(any(feature = "camoufox", feature = "cdp"))]
    pub use crate::envkit::{EnvDump, EnvScope, EnvTarget};
    /// 代理值类型 + 代理池 / 健康探测(后端无关:camoufox `BrowserPool` 与 cdp `ChromiumPool` 都用)。
    #[cfg(any(feature = "camoufox", feature = "cdp"))]
    pub use crate::launcher::Proxy;
    #[cfg(feature = "camoufox")]
    pub use crate::launcher::{Fingerprint, Geolocation, OsType};
    #[cfg(feature = "camoufox")]
    pub use crate::pool::{BrowserPool, FingerprintPool, FingerprintProfile, PoolOptions};
    /// 后端无关并发原语(camoufox `BrowserPool` / cdp `ChromiumPool` 共用)。
    #[cfg(any(feature = "camoufox", feature = "cdp"))]
    pub use crate::pool::{Checkpoint, RetryPolicy, RotateStrategy};
    #[cfg(any(feature = "camoufox", feature = "cdp"))]
    pub use crate::pool::{ProxyGeo, ProxyHealth, ProxyPool};
    #[cfg(feature = "camoufox")]
    pub use crate::session::{
        BrowserProfile, PostData, ReplayBuilder, SessionOptions, SessionPage,
    };
    /// 滑块/缺口识别类型 + 后端原语 trait(`--features slider`,**后端无关**,camoufox / cdp 均可用)。
    #[cfg(feature = "slider")]
    pub use crate::slider::{
        GapMethod, ImageSource, SliderConfig, SliderGap, SliderResult, SliderTab, SuccessCheck,
    };
    #[cfg(feature = "camoufox")]
    pub use crate::web_page::{PageMode, WebPage};

    // ── 验证码 OCR(--features ocr)──────────────────────────────────
    #[cfg(feature = "ocr")]
    pub use crate::ocr::{BBox, ClickHit, ClickWord, Det, GlyphMatcher, Ocr, SampleBank};
}
