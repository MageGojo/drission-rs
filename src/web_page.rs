//! 双模页面 [`WebPage`]:对标 DrissionPage 的 `WebPage` —— 在 **Driver(浏览器)** 与
//! **Session(HTTP)** 两种模式间切换,**共享 cookie**。
//!
//! 这是"旧电脑/高并发"的易用门面:绝大多数轻量请求走 Session(快、省内存,无渲染),遇到需要 JS
//! 渲染 / 交互 / 过盾时再 [`change_mode`](WebPage::change_mode) 切到 Driver;切换时自动同步 cookie,
//! 登录态无缝衔接。
//!
//! ```ignore
//! use drission::prelude::*;
//! // 先用浏览器过盾/登录
//! let mut page = WebPage::new_driver(BrowserOptions::new().headless(true)).await?;
//! page.get("https://site/login").await?;
//! // ... 登录交互 ...
//! // 切到会话模式高速抓列表(带上登录 cookie)
//! page.change_mode(PageMode::Session).await?;
//! page.get("https://site/api/list?page=1").await?;
//! let rows = page.s_eles("css:.item").await?;
//! page.quit().await?;
//! ```

use crate::browser::{Browser, Element, StaticElement, Tab};
use crate::launcher::BrowserOptions;
use crate::session::{PostData, SessionOptions, SessionPage};
use crate::{Error, Result};

/// 页面模式:Driver(驱动真实浏览器)/ Session(纯 HTTP)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageMode {
    /// 驱动真实浏览器(Camoufox):能跑 JS、交互、过盾,但重。
    Driver,
    /// 纯 HTTP 会话:快、省内存、无渲染;只能拿静态 HTML(`s_ele`)。
    Session,
}

/// 双模页面:同一对象在 Driver / Session 间切换,共享 cookie。
pub struct WebPage {
    mode: PageMode,
    browser: Option<Browser>,
    tab: Option<Tab>,
    session: SessionPage,
    browser_opts: BrowserOptions,
}

impl WebPage {
    /// 以 **Session** 模式起步(不启动浏览器)。无头浏览器选项留作日后切 Driver 时用。
    pub fn new_session() -> Result<Self> {
        Self::new_session_with(SessionOptions::new(), BrowserOptions::new().headless(true))
    }

    /// 同 [`new_session`](Self::new_session),自定义会话与(日后切 Driver 用的)浏览器选项。
    pub fn new_session_with(session: SessionOptions, browser_opts: BrowserOptions) -> Result<Self> {
        Ok(Self {
            mode: PageMode::Session,
            browser: None,
            tab: None,
            session: SessionPage::new(session)?,
            browser_opts,
        })
    }

    /// 以 **Driver** 模式起步(启动浏览器)。
    pub async fn new_driver(browser_opts: BrowserOptions) -> Result<Self> {
        Self::new_driver_with(browser_opts, SessionOptions::new()).await
    }

    /// 同 [`new_driver`](Self::new_driver),并自定义会话选项(切到 Session 时用)。
    pub async fn new_driver_with(
        browser_opts: BrowserOptions,
        session: SessionOptions,
    ) -> Result<Self> {
        let browser = Browser::launch(browser_opts.clone()).await?;
        let tab = browser.latest_tab().await?;
        Ok(Self {
            mode: PageMode::Driver,
            browser: Some(browser),
            tab: Some(tab),
            session: SessionPage::new(session)?,
            browser_opts,
        })
    }

    /// 当前模式。
    pub fn mode(&self) -> PageMode {
        self.mode
    }

    /// 是否处于 Driver 模式。
    pub fn is_driver(&self) -> bool {
        self.mode == PageMode::Driver
    }

    /// 切换模式;切换时**自动同步 cookie**:Driver→Session 把浏览器 cookie 灌进会话,
    /// Session→Driver 把会话 cookie 灌进浏览器(并按需懒启动浏览器)。同模式调用为空操作。
    pub async fn change_mode(&mut self, mode: PageMode) -> Result<()> {
        if mode == self.mode {
            return Ok(());
        }
        match mode {
            PageMode::Session => {
                if let Some(tab) = &self.tab {
                    self.session.load_cookies_from_tab(tab).await?;
                }
                self.mode = PageMode::Session;
            }
            PageMode::Driver => {
                let tab = self.ensure_driver().await?;
                self.session.apply_cookies_to_tab(&tab).await?;
                self.mode = PageMode::Driver;
            }
        }
        Ok(())
    }

    /// 导航 / 请求(按当前模式)。返回是否成功。
    pub async fn get(&mut self, url: &str) -> Result<bool> {
        match self.mode {
            PageMode::Session => self.session.get(url).await,
            PageMode::Driver => self.ensure_driver().await?.get(url).await,
        }
    }

    /// 发 POST(走会话;Driver 模式下也用底层会话发,便于表单/接口提交)。
    pub async fn post(&mut self, url: &str, data: PostData) -> Result<bool> {
        self.session.post(url, data).await
    }

    /// 当前页面 HTML(两模式都支持)。
    pub async fn html(&self) -> Result<String> {
        match self.mode {
            PageMode::Session => Ok(self.session.html().to_string()),
            PageMode::Driver => self.tab.as_ref().ok_or_else(no_tab)?.html().await,
        }
    }

    /// 页面标题(两模式都支持)。
    pub async fn title(&self) -> Result<String> {
        match self.mode {
            PageMode::Session => self.session.title(),
            PageMode::Driver => self.tab.as_ref().ok_or_else(no_tab)?.title().await,
        }
    }

    /// 当前 URL(两模式都支持)。
    pub async fn url(&self) -> Result<String> {
        match self.mode {
            PageMode::Session => Ok(self.session.url().to_string()),
            PageMode::Driver => self.tab.as_ref().ok_or_else(no_tab)?.url().await,
        }
    }

    /// 静态元素查询(两模式都支持:Driver 取实时 HTML 解析,Session 解析其响应)。
    pub async fn s_ele(&self, selector: &str) -> Result<StaticElement> {
        match self.mode {
            PageMode::Session => self.session.s_ele(selector),
            PageMode::Driver => self.tab.as_ref().ok_or_else(no_tab)?.s_ele(selector).await,
        }
    }

    /// 静态元素批量查询(两模式都支持)。
    pub async fn s_eles(&self, selector: &str) -> Result<Vec<StaticElement>> {
        match self.mode {
            PageMode::Session => self.session.s_eles(selector),
            PageMode::Driver => self.tab.as_ref().ok_or_else(no_tab)?.s_eles(selector).await,
        }
    }

    /// 实时元素(**仅 Driver 模式**;Session 模式请改用 [`s_ele`](Self::s_ele))。
    pub async fn ele(&self, selector: &str) -> Result<Element> {
        match self.mode {
            PageMode::Driver => self.tab.as_ref().ok_or_else(no_tab)?.ele(selector).await,
            PageMode::Session => Err(session_no_live("ele")),
        }
    }

    /// 实时元素批量(**仅 Driver 模式**)。
    pub async fn eles(&self, selector: &str) -> Result<Vec<Element>> {
        match self.mode {
            PageMode::Driver => self.tab.as_ref().ok_or_else(no_tab)?.eles(selector).await,
            PageMode::Session => Err(session_no_live("eles")),
        }
    }

    /// 实时标签句柄(Driver 模式下做点击/输入/监听/截图等);尚未启动浏览器时为 `None`。
    pub fn tab(&self) -> Option<&Tab> {
        self.tab.as_ref()
    }

    /// 底层会话(纯 HTTP / cookie 管理 / 存读盘)。
    pub fn session(&self) -> &SessionPage {
        &self.session
    }

    /// 底层会话(可变:`set_cookies` / `load_cookies_file` 等)。
    pub fn session_mut(&mut self) -> &mut SessionPage {
        &mut self.session
    }

    /// 退出:关闭浏览器(若已启动)。Session 资源随之释放。
    pub async fn quit(self) -> Result<()> {
        if let Some(b) = self.browser {
            b.quit().await?;
        }
        Ok(())
    }

    /// 确保浏览器已启动(切 Driver / Driver 操作时懒启动),返回其最新标签。
    async fn ensure_driver(&mut self) -> Result<Tab> {
        if self.tab.is_none() {
            let browser = Browser::launch(self.browser_opts.clone()).await?;
            let tab = browser.latest_tab().await?;
            self.browser = Some(browser);
            self.tab = Some(tab);
        }
        self.tab
            .clone()
            .ok_or_else(|| Error::Other("浏览器启动后仍无标签".into()))
    }
}

fn no_tab() -> Error {
    Error::Other("Driver 模式尚未启动浏览器".into())
}

fn session_no_live(name: &str) -> Error {
    Error::Other(format!(
        "{name}() 仅 Driver 模式可用;Session 模式请用 s_{name}()(静态解析)"
    ))
}
