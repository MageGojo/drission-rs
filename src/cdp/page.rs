//! CDP 后端的一行起步门面 [`ChromiumPage`] —— 对齐 camoufox 的 `Page`(DrissionPage 的 `ChromiumPage`)。
//!
//! 持有浏览器 + 活动标签,经 [`Deref`] 把**全部** [`ChromiumTab`] 方法直接挂到 page 上。
//! prelude 在 `cdp` feature 下把它导出为统一名 `Page`(与 camoufox 后端接口一致)。
//!
//! ```no_run
//! # async fn demo() -> drission::Result<()> {
//! use drission::prelude::*;
//! let page = Page::new().await?;          // 一行起步(有头 + 反检测开箱即用)
//! page.get("https://example.com").await?; // Tab 方法直接用
//! println!("{}", page.title().await?);
//! page.quit().await?;
//! # Ok(()) }
//! ```

use std::ops::Deref;

use crate::Result;
use crate::cdp::{ChromiumBrowser, ChromiumOptions, ChromiumTab};

/// 一行起步的 Chromium 页面(浏览器 + 活动标签合一)。`Deref` 到 [`ChromiumTab`]。
pub struct ChromiumPage {
    browser: ChromiumBrowser,
    tab: ChromiumTab,
}

impl ChromiumPage {
    /// 一行起步:**有头 + 反检测开箱即用**(对齐 camoufox `Page::new`)。无头用 [`headless`](Self::headless)。
    pub async fn new() -> Result<Self> {
        Self::from_browser(ChromiumBrowser::launch_default().await?).await
    }

    /// 一行起步:**无头 + 反检测**(无头自动补 UA/WebGL/屏幕,见 `docs/CDP过盾.md`)。
    pub async fn headless() -> Result<Self> {
        Self::with(ChromiumOptions::new().headless(true)).await
    }

    /// 用自定义 [`ChromiumOptions`] 起步(代理 / locale / 时区 / 窗口 / 持久 profile 等)。
    pub async fn with(opts: ChromiumOptions) -> Result<Self> {
        Self::from_browser(ChromiumBrowser::launch(opts).await?).await
    }

    /// 通过 CDP 调试端点**接管**已运行浏览器(对齐 camoufox `Page::connect`)。
    pub async fn connect(debug_http_url: &str) -> Result<Self> {
        Self::from_browser(ChromiumBrowser::connect(debug_http_url).await?).await
    }

    /// 同 [`connect`](Self::connect),可指定 [`ChromiumOptions`]。
    pub async fn connect_with(debug_http_url: &str, opts: ChromiumOptions) -> Result<Self> {
        Self::from_browser(ChromiumBrowser::connect_with(debug_http_url, opts).await?).await
    }

    /// 用一个已启动的 [`ChromiumBrowser`] 构造 `Page`,活动标签取其最新标签。
    pub async fn from_browser(browser: ChromiumBrowser) -> Result<Self> {
        let tab = browser.latest_tab().await?;
        Ok(Self { browser, tab })
    }

    /// 底层浏览器句柄(多标签 / 接管等高级用法入口)。
    pub fn browser(&self) -> &ChromiumBrowser {
        &self.browser
    }

    /// 当前活动标签(`Page` 的 `Deref` 目标)。
    pub fn tab(&self) -> &ChromiumTab {
        &self.tab
    }

    /// 新开一个标签(可选直接访问 `url`),返回该标签句柄;不改变 `Page` 的活动标签。
    pub async fn new_tab(&self, url: Option<&str>) -> Result<ChromiumTab> {
        self.browser.new_tab(url).await
    }

    /// 退出:优雅关闭浏览器并清理(接管模式下仅断开)。
    pub async fn quit(&self) -> Result<()> {
        self.browser.quit().await
    }
}

impl Deref for ChromiumPage {
    type Target = ChromiumTab;
    fn deref(&self) -> &ChromiumTab {
        &self.tab
    }
}
