//! **Chromium 后端**(Chrome / Edge / Brave / Electron 应用),经 **CDP**(Chrome DevTools Protocol)。
//!
//! 与默认的 Camoufox/Juggler 后端并行:用于**驱动或接管 Chromium 系浏览器**,以及**控制 Electron 桌面
//! 应用**(它们都内置 CDP)。CDP 线消息格式(`id`/`method`/`params`/`sessionId` + `result`/`error`/事件)
//! 与 Juggler 高度一致,故**复用** [`crate::protocol::Connection`] 的请求/响应/事件机制,只换方法名与目标管理。
//!
//! ```no_run
//! use drission::prelude::*;
//! # async fn f() -> drission::Result<()> {
//! // 启动 Chrome(headless),或 connect("http://127.0.0.1:9222") 接管已开的 Chrome / Electron
//! let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(true)).await?;
//! let tab = browser.new_tab(Some("https://example.com")).await?;
//! // 元素句柄 + 原生可信点击 + 拟人输入
//! let h1 = tab.ele("h1").await?;
//! println!("{}", h1.text().await?);
//! // 网络监听(原生 Network 域 + getResponseBody)
//! let listen = tab.listen();
//! listen.start_xhr(&["/api/"]).await?;
//! // ... 触发请求 ...
//! if let Some(pkt) = listen.wait(None).await? { println!("{}", pkt.url); }
//! browser.quit().await?;
//! # Ok(()) }
//! ```
//!
//! 能力(与 Juggler 后端对齐):
//! - 启动/接管(`launch`/`connect`)、新标签、导航、`run_js`、标题/URL、整页/区域截图;
//! - **浏览器自动下载** [`fetch`]:找不到系统 Chrome 时,自动从 **Chrome for Testing** 下载并缓存
//!   (`mac-arm64`/`mac-x64`/`win64`/`win32`/`linux64`,对标 CloakBrowser 首次运行自动下载);
//! - **元素句柄** [`ChromiumElement`]:`ele`/`eles`/相对定位 + 读文本/属性 + **原生可信点击**
//!   (`Input.dispatchMouseEvent`)+ **拟人逐字符输入**(`input_human`)+ 表单填充 + 元素截图;
//! - **网络监听** [`CdpListen`]:原生 `Network` 域事件 + `Network.getResponseBody`;
//! - **请求拦截** [`CdpIntercept`]:`Fetch` 域 `requestPaused` + `continue`/`fulfill`/`fail`。

mod actions;
mod browser;
mod cloudflare;
mod console;
mod container;
mod core;
mod download;
mod dump_env;
pub mod element;
pub mod fetch;
mod fingerprint;
pub mod frame;
mod handles;
pub mod interceptor;
pub mod listener;
pub mod locate;
mod options;
mod page;
mod pool;
mod screencast;
mod shadow;
mod stealth;
mod tab;
mod types;
mod websocket;

pub use actions::ChromiumActions;
pub use browser::ChromiumBrowser;
pub use console::{ChromiumConsole, ConsoleData, ConsoleFilter};
pub use download::{ChromiumDownloads, DownloadMission, DownloadState};
pub use dump_env::{ChromiumEnvDumper, ChromiumEnvProbe};
pub use element::{ChromiumElement, ChromiumElementWait, ElementRect as ChromiumElementRect};
pub use fetch::{cft_platform, download_chrome_for, download_chrome_milestone, ensure_chrome};
pub use fingerprint::{CdpFingerprint, CdpFingerprintPool, CdpOs};
pub use frame::ChromiumFrame;
pub use handles::{ChromiumScroll, ChromiumSetTab, ChromiumWait, ChromiumWindow};
pub use interceptor::{CdpIntercept, CdpInterceptedRequest};
pub use listener::CdpListen;
pub use locate::chrome_path;
pub use options::{ChromiumContextOverride, ChromiumOptions};
pub use page::ChromiumPage;
pub use pool::{ChromiumPool, ChromiumPoolOptions};
pub use screencast::ChromiumScreencast;
pub use shadow::ChromiumShadowRoot;
pub use tab::ChromiumTab;
pub use types::{
    Cookie, CookieParam, DialogInfo, DownloadInfo, GetOptions, ImageFormat, LoadMode, PageRect,
    ShotOpts,
};
pub use websocket::{ChromiumWsListener, WsDirection, WsFilter, WsMessage};
