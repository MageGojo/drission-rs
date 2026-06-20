//! CDP 后端的便捷句柄:[`ChromiumScroll`](`tab.scroll()`)/ [`ChromiumSetTab`](`tab.set()`)/
//! [`ChromiumWindow`](`tab.set().window()`)/ [`ChromiumWait`](`tab.wait()`),对齐 camoufox 同名句柄。

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::{Instant, sleep};

use crate::Result;
use crate::cdp::core::CdpCore;
use crate::cdp::element::ChromiumElement;
use crate::cdp::tab::doc_query_expr;
use crate::cdp::types::CookieParam;

// ── 滚动 ────────────────────────────────────────────────────────────────────

/// 页面滚动句柄(`tab.scroll()`)。
pub struct ChromiumScroll {
    core: Arc<CdpCore>,
}

impl ChromiumScroll {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self { core }
    }
    async fn js(&self, expr: &str) -> Result<()> {
        self.core.eval_value(expr).await?;
        Ok(())
    }
    /// 滚到顶部。
    pub async fn to_top(&self) -> Result<()> {
        self.js("window.scrollTo(0,0)").await
    }
    /// 滚到底部。
    pub async fn to_bottom(&self) -> Result<()> {
        self.js("window.scrollTo(0, document.documentElement.scrollHeight)")
            .await
    }
    /// 滚到最左。
    pub async fn to_left(&self) -> Result<()> {
        self.js("window.scrollTo(0, window.scrollY)").await
    }
    /// 滚到最右。
    pub async fn to_right(&self) -> Result<()> {
        self.js("window.scrollTo(document.documentElement.scrollWidth, window.scrollY)")
            .await
    }
    /// 相对滚动 `(x, y)`。
    pub async fn by(&self, x: f64, y: f64) -> Result<()> {
        self.js(&format!("window.scrollBy({x},{y})")).await
    }
    /// 滚到绝对位置 `(x, y)`。
    pub async fn to_location(&self, x: f64, y: f64) -> Result<()> {
        self.js(&format!("window.scrollTo({x},{y})")).await
    }
    /// 滚动直到元素进入视口(居中)。
    pub async fn to_see(&self, ele: &ChromiumElement) -> Result<()> {
        ele.scroll_into_view().await
    }
}

// ── 设置 ────────────────────────────────────────────────────────────────────

/// 标签设置句柄(`tab.set()`)。
pub struct ChromiumSetTab {
    core: Arc<CdpCore>,
}

impl ChromiumSetTab {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self { core }
    }
    /// 设默认超时(影响 `ele` 等待 / 监听 / wait)。
    pub fn timeout(&self, d: Duration) -> &Self {
        self.core.set_timeout(d);
        self
    }
    /// 覆盖 User-Agent(`Emulation.setUserAgentOverride`)。
    pub async fn user_agent(&self, ua: &str) -> Result<()> {
        self.core
            .send("Emulation.setUserAgentOverride", json!({ "userAgent": ua }))
            .await?;
        Ok(())
    }
    /// 批量设置 cookie。
    pub async fn cookies(&self, cookies: Vec<CookieParam>) -> Result<()> {
        let arr: Vec<Value> = cookies.iter().map(cookie_param_json).collect();
        self.core
            .send("Storage.setCookies", json!({ "cookies": arr }))
            .await?;
        Ok(())
    }
    /// 窗口句柄(尺寸 / 最大化)。
    pub fn window(&self) -> ChromiumWindow {
        ChromiumWindow::new(self.core.clone())
    }
}

// ── 窗口 ────────────────────────────────────────────────────────────────────

/// 浏览器窗口句柄(`tab.set().window()`)。
pub struct ChromiumWindow {
    core: Arc<CdpCore>,
}

impl ChromiumWindow {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self { core }
    }
    async fn window_id(&self) -> Result<i64> {
        let r = self
            .core
            .send(
                "Browser.getWindowForTarget",
                json!({ "targetId": self.core.target_id }),
            )
            .await?;
        Ok(r["windowId"].as_i64().unwrap_or(0))
    }
    /// 设窗口大小 `(width, height)`。
    pub async fn size(&self, width: u32, height: u32) -> Result<()> {
        let id = self.window_id().await?;
        self.core
            .send(
                "Browser.setWindowBounds",
                json!({ "windowId": id, "bounds": { "width": width, "height": height, "windowState": "normal" } }),
            )
            .await?;
        Ok(())
    }
    /// 设视口大小(`Emulation.setDeviceMetricsOverride`,不改外层窗口)。
    pub async fn viewport(&self, width: u32, height: u32) -> Result<()> {
        self.core
            .send(
                "Emulation.setDeviceMetricsOverride",
                json!({ "width": width, "height": height, "deviceScaleFactor": 0, "mobile": false }),
            )
            .await?;
        Ok(())
    }
    /// 最大化窗口。
    pub async fn max(&self) -> Result<()> {
        let id = self.window_id().await?;
        self.core
            .send(
                "Browser.setWindowBounds",
                json!({ "windowId": id, "bounds": { "windowState": "maximized" } }),
            )
            .await?;
        Ok(())
    }
}

// ── 等待 ────────────────────────────────────────────────────────────────────

/// 标签级等待句柄(`tab.wait()`)。各方法超时返回 `false`、不报错。
pub struct ChromiumWait {
    core: Arc<CdpCore>,
}

impl ChromiumWait {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self { core }
    }
    fn deadline(&self, timeout: Option<Duration>) -> Instant {
        Instant::now() + timeout.unwrap_or_else(|| self.core.timeout())
    }
    /// 等 `document.readyState === 'complete'`。
    pub async fn doc_loaded(&self, timeout: Option<Duration>) -> Result<bool> {
        let deadline = self.deadline(timeout);
        loop {
            let rs = self
                .core
                .eval_value("document.readyState")
                .await
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            if rs == "complete" {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(Duration::from_millis(80)).await;
        }
    }
    /// 等元素出现且可见。
    pub async fn ele_displayed(&self, selector: &str, timeout: Option<Duration>) -> Result<bool> {
        let deadline = self.deadline(timeout);
        loop {
            if let Some(oid) = self
                .core
                .eval_handle(&doc_query_expr(selector, true))
                .await?
            {
                let el = ChromiumElement::new(self.core.clone(), oid);
                if el.is_displayed().await.unwrap_or(false) {
                    return Ok(true);
                }
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(Duration::from_millis(80)).await;
        }
    }
    /// 等元素从 DOM 消失(查不到)。
    pub async fn ele_deleted(&self, selector: &str, timeout: Option<Duration>) -> Result<bool> {
        let deadline = self.deadline(timeout);
        loop {
            let gone = self
                .core
                .eval_handle(&doc_query_expr(selector, true))
                .await?
                .is_none();
            if gone {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(Duration::from_millis(80)).await;
        }
    }
    /// 等标题包含子串。
    pub async fn title_contains(&self, sub: &str, timeout: Option<Duration>) -> Result<bool> {
        let deadline = self.deadline(timeout);
        loop {
            let t = self
                .core
                .eval_value("document.title")
                .await
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            if t.contains(sub) {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(Duration::from_millis(80)).await;
        }
    }
    /// 等 URL 包含子串。
    pub async fn url_contains(&self, sub: &str, timeout: Option<Duration>) -> Result<bool> {
        let deadline = self.deadline(timeout);
        loop {
            let u = self
                .core
                .eval_value("location.href")
                .await
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            if u.contains(sub) {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(Duration::from_millis(80)).await;
        }
    }
}

/// `CookieParam` → CDP `Storage.setCookies` 的单项 JSON。
pub(crate) fn cookie_param_json(c: &CookieParam) -> Value {
    let mut o = json!({ "name": c.name, "value": c.value });
    if let Some(u) = &c.url {
        o["url"] = json!(u);
    }
    if let Some(d) = &c.domain {
        o["domain"] = json!(d);
    }
    if let Some(p) = &c.path {
        o["path"] = json!(p);
    }
    if let Some(v) = c.secure {
        o["secure"] = json!(v);
    }
    if let Some(v) = c.http_only {
        o["httpOnly"] = json!(v);
    }
    if let Some(v) = c.expires {
        o["expires"] = json!(v);
    }
    o
}
