//! 一行起步的浏览器页面门面 [`Page`] —— 对标 DrissionPage 的 `ChromiumPage`,**大道至简**。
//!
//! DrissionPage 里 `page = ChromiumPage()` 一行就能开浏览器并直接 `page.get(...)`/`page.ele(...)`。
//! 本库底层是「[`Browser`](crate::browser::Browser)(进程/多标签)+ [`Tab`](crate::browser::Tab)(单页)」
//! 两层,功能更可控,但日常脚本几乎总是「开一个浏览器、驱动它的当前标签」。`Page` 就是把这两步合一:
//! 持有浏览器与其活动标签,并通过 [`Deref`] 把**全部** `Tab` 方法直接挂到 `page` 上。
//!
//! ```no_run
//! # async fn demo() -> drission::Result<()> {
//! use drission::prelude::*;
//!
//! let page = Page::new().await?;                 // 一行起步(有头 + 反检测开箱即用)
//! page.get("https://example.com").await?;        // Tab 的方法直接用
//! println!("{}", page.title().await?);
//! let h1 = page.ele("tag:h1").await?;            // 定位语法同 DP
//! println!("{}", h1.text().await?);
//! page.click("@id:more").await?;                 // 「找+点」一步到位
//! page.quit().await?;
//! # Ok(())
//! # }
//! ```
//!
//! 想要更底层的多标签 / 接管 / 并发,仍可用 [`Page::browser`] 拿到 [`Browser`](crate::browser::Browser),
//! 或直接用 `Browser` / `Tab`(`Page` 是**附加**门面,不替代它们)。需要 Driver/Session 双模见
//! [`WebPage`](crate::web_page::WebPage);纯 HTTP 见 [`SessionPage`](crate::session::SessionPage)。

use std::ops::Deref;

use crate::Result;
use crate::browser::{Browser, Tab};
use crate::launcher::BrowserOptions;

/// 一行起步的浏览器页面(浏览器 + 活动标签合一)。`Deref` 到 [`Tab`],故 `page.get/ele/click/...`
/// 等**所有标签方法**可直接调用;[`Page`] 自身只提供进程级方法(`browser`/`new_tab`/`quit`)。
pub struct Page {
    browser: Browser,
    tab: Tab,
}

impl Page {
    /// 一行起步:**有头 + 反检测开箱即用**(等价 DrissionPage 的 `ChromiumPage()`)。
    ///
    /// 要无头用 [`headless`](Self::headless);要自定义用 [`with`](Self::with)。
    pub async fn new() -> Result<Self> {
        Self::from_browser(Browser::launch_default().await?).await
    }

    /// 一行起步:**无头 + 反检测**(其余默认同 [`new`](Self::new))。
    pub async fn headless() -> Result<Self> {
        Self::with(BrowserOptions::new().headless(true)).await
    }

    /// 用自定义 [`BrowserOptions`] 起步(代理 / locale / 时区 / 指纹 / 窗口等)。
    ///
    /// ```no_run
    /// # async fn demo() -> drission::Result<()> {
    /// use drission::prelude::*;
    /// let page = Page::with(BrowserOptions::new().headless(true).locale("zh-CN")).await?;
    /// # let _ = page; Ok(()) }
    /// ```
    pub async fn with(opts: BrowserOptions) -> Result<Self> {
        Self::from_browser(Browser::launch(opts).await?).await
    }

    /// 通过 **WebSocket** 接管一个已运行的浏览器([`BrowserServer`](crate::browser::BrowserServer) 暴露的端点)。
    /// 语义同 [`Browser::connect`]:不启动子进程、[`quit`](Self::quit) 只断开本地连接。
    pub async fn connect(ws_url: &str) -> Result<Self> {
        Self::from_browser(Browser::connect(ws_url).await?).await
    }

    /// 同 [`connect`](Self::connect),可指定 [`BrowserOptions`](crate::launcher::BrowserOptions)(会话级覆盖与反检测项)。
    pub async fn connect_with(ws_url: &str, opts: BrowserOptions) -> Result<Self> {
        Self::from_browser(Browser::connect_with(ws_url, opts).await?).await
    }

    /// 用一个已启动的 [`Browser`] 构造 `Page`,活动标签取其最新标签。
    pub async fn from_browser(browser: Browser) -> Result<Self> {
        let tab = browser.latest_tab().await?;
        Ok(Self { browser, tab })
    }

    /// 底层浏览器句柄(多标签 / 接管 / per-context 覆盖等高级用法的入口)。
    pub fn browser(&self) -> &Browser {
        &self.browser
    }

    /// 当前活动标签(`Page` 的 `Deref` 目标;需要 `Tab` 的克隆传给别处时用)。
    pub fn tab(&self) -> &Tab {
        &self.tab
    }

    /// 新开一个独立标签(独立 BrowserContext / cookie),可选直接访问 `url`,返回该标签句柄。
    ///
    /// 注意:不改变 `Page` 的活动标签(`page.*` 仍驱动初始标签);多标签场景直接驱动返回的 [`Tab`]。
    pub async fn new_tab(&self, url: Option<&str>) -> Result<Tab> {
        self.browser.new_tab(url).await
    }

    /// 退出:优雅关闭浏览器并清理(接管模式下仅断开,见 [`Browser::quit`])。
    ///
    /// 可省略——`Page` 析构时 [`Browser`] 的 `Drop` 会兜底杀进程 + 删临时 profile;
    /// 但显式 `quit().await` 会 `wait` 回收子进程(无僵尸),推荐。
    pub async fn quit(&self) -> Result<()> {
        self.browser.quit().await
    }
}

impl Deref for Page {
    type Target = Tab;
    fn deref(&self) -> &Tab {
        &self.tab
    }
}
