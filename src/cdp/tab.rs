//! [`ChromiumTab`]:CDP 后端的标签页对象,对标 Juggler 后端的 [`Tab`](crate::browser::Tab)。
//!
//! 高层能力:导航 / `run_js` / 标题·URL / **元素句柄查找**(`ele`/`eles`)/ 便捷点击输入 /
//! 原生可信低层鼠标 / 键盘 / 截图 / **网络监听**([`listen`](Self::listen))/ **请求拦截**
//! ([`intercept`](Self::intercept))。所有句柄共享同一 [`CdpCore`]。

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::{Instant, sleep, timeout_at};

use crate::browser::keys::KeyInput;
use crate::cdp::core::CdpCore;
use crate::cdp::element::ChromiumElement;
use crate::cdp::interceptor::CdpIntercept;
use crate::cdp::listener::CdpListen;
use crate::locator::{self, Query};
use crate::{Error, Result};

/// 一个 Chromium 标签页(或附着的 Electron 窗口)。克隆共享同一底层标签。
#[derive(Clone)]
pub struct ChromiumTab {
    core: Arc<CdpCore>,
}

impl ChromiumTab {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self { core }
    }

    /// 设置默认超时(影响 `ele` 等待 / 监听 / 拦截)。
    pub fn set_timeout(&self, d: Duration) {
        self.core.set_timeout(d);
    }

    /// 当前默认超时。
    pub fn timeout(&self) -> Duration {
        self.core.timeout()
    }

    /// 导航到 `url` 并等待 `load` 事件(最多默认超时)。
    pub async fn get(&self, url: &str) -> Result<()> {
        let mut events = self.core.conn.subscribe();
        self.core
            .send("Page.navigate", json!({ "url": url }))
            .await?;
        let sid = self.core.session_id.clone();
        let deadline = Instant::now() + self.core.timeout();
        let _ = timeout_at(deadline, async {
            loop {
                match events.recv().await {
                    Ok(ev)
                        if ev.method == "Page.loadEventFired"
                            && ev.session_id.as_deref() == Some(&sid) =>
                    {
                        break;
                    }
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        })
        .await;
        Ok(())
    }

    /// 在页面执行 JS 表达式,返回结果值(`Runtime.evaluate`,自动 await Promise)。
    pub async fn run_js(&self, expression: &str) -> Result<Value> {
        self.core.eval_value(expression).await
    }

    /// 页面标题。
    pub async fn title(&self) -> Result<String> {
        Ok(self
            .run_js("document.title")
            .await?
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    /// 当前 URL。
    pub async fn url(&self) -> Result<String> {
        Ok(self
            .run_js("location.href")
            .await?
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    /// 页面 HTML(`documentElement.outerHTML`)。
    pub async fn html(&self) -> Result<String> {
        Ok(self
            .run_js("document.documentElement.outerHTML")
            .await?
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    // ── 元素句柄 ──────────────────────────────────────────────────────────

    /// 查找第一个匹配元素(DP 定位语法);未找到立即返回 [`Error::ElementNotFound`]。
    pub async fn ele(&self, selector: &str) -> Result<ChromiumElement> {
        match self
            .core
            .eval_handle(&doc_query_expr(selector, true))
            .await?
        {
            Some(oid) => Ok(ChromiumElement::new(self.core.clone(), oid)),
            None => Err(Error::ElementNotFound(selector.to_string())),
        }
    }

    /// 查找第一个匹配元素,在默认超时内**轮询等待**它出现;超时返回 [`Error::ElementNotFound`]。
    pub async fn wait_ele(
        &self,
        selector: &str,
        timeout: Option<Duration>,
    ) -> Result<ChromiumElement> {
        let deadline = Instant::now() + timeout.unwrap_or_else(|| self.core.timeout());
        loop {
            if let Some(oid) = self
                .core
                .eval_handle(&doc_query_expr(selector, true))
                .await?
            {
                return Ok(ChromiumElement::new(self.core.clone(), oid));
            }
            if Instant::now() >= deadline {
                return Err(Error::ElementNotFound(selector.to_string()));
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    /// 查找所有匹配元素。
    pub async fn eles(&self, selector: &str) -> Result<Vec<ChromiumElement>> {
        let Some(arr) = self
            .core
            .eval_handle(&doc_query_expr(selector, false))
            .await?
        else {
            return Ok(Vec::new());
        };
        let oids = self.core.array_object_ids(&arr).await?;
        Ok(oids
            .into_iter()
            .map(|oid| ChromiumElement::new(self.core.clone(), oid))
            .collect())
    }

    /// 元素可见文本(CSS/xpath 定位);未找到返回 `None`。便捷封装。
    pub async fn ele_text(&self, selector: &str) -> Result<Option<String>> {
        match self.ele(selector).await {
            Ok(el) => Ok(Some(el.text().await?)),
            Err(Error::ElementNotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// **可信点击**定位到的元素;未找到返回 `false`。便捷封装(底层走元素句柄的原生点击)。
    pub async fn click(&self, selector: &str) -> Result<bool> {
        match self.ele(selector).await {
            Ok(el) => {
                el.click().await?;
                Ok(true)
            }
            Err(Error::ElementNotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// 给输入框填值(一次性插入);未找到返回 `false`。便捷封装。
    pub async fn input(&self, selector: &str, text: &str) -> Result<bool> {
        match self.ele(selector).await {
            Ok(el) => {
                el.input(text).await?;
                Ok(true)
            }
            Err(Error::ElementNotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    // ── 低层输入(原生可信)──────────────────────────────────────────────

    /// 移动鼠标到视口坐标 `(x, y)`(未按下)。
    pub async fn mouse_move(&self, x: f64, y: f64) -> Result<()> {
        self.core
            .dispatch_mouse("mouseMoved", x, y, "none", 0, 0)
            .await
    }

    /// 在 `(x, y)` 按下左键。
    pub async fn mouse_down(&self, x: f64, y: f64) -> Result<()> {
        self.core
            .dispatch_mouse("mousePressed", x, y, "left", 1, 1)
            .await
    }

    /// 在 `(x, y)` 松开左键。
    pub async fn mouse_up(&self, x: f64, y: f64) -> Result<()> {
        self.core
            .dispatch_mouse("mouseReleased", x, y, "left", 0, 1)
            .await
    }

    /// 按住左键移动到 `(x, y)`(拖拽中的 move,`buttons=1`)。
    pub async fn mouse_drag(&self, x: f64, y: f64) -> Result<()> {
        self.core
            .dispatch_mouse("mouseMoved", x, y, "none", 1, 0)
            .await
    }

    /// 敲一个键(普通字符或特殊键名,见 [`Keys`](crate::browser::Keys))。
    pub async fn press_key(&self, key: &str) -> Result<()> {
        self.core.press_key(key).await
    }

    /// 按**序列**输入(文本片段直接插入、特殊键派发按键)。需先聚焦目标(如先 `ele.click()`)。
    pub async fn type_keys(&self, parts: &[KeyInput]) -> Result<()> {
        for p in parts {
            match p {
                KeyInput::Text(t) => self.core.insert_text(t).await?,
                KeyInput::Key(k) => self.core.press_key(k).await?,
            }
        }
        Ok(())
    }

    // ── 截图 ──────────────────────────────────────────────────────────────

    /// 可视区截图(PNG 字节)。
    pub async fn screenshot_bytes(&self) -> Result<Vec<u8>> {
        let r = self
            .core
            .send("Page.captureScreenshot", json!({ "format": "png" }))
            .await?;
        let data = r["data"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: 无截图数据"))?;
        crate::util::base64_decode(data).ok_or_else(|| Error::msg("CDP: 截图 base64 解码失败"))
    }

    /// 整页截图(PNG 字节,`captureBeyondViewport`)。
    pub async fn screenshot_full_bytes(&self) -> Result<Vec<u8>> {
        let r = self
            .core
            .send(
                "Page.captureScreenshot",
                json!({ "format": "png", "captureBeyondViewport": true }),
            )
            .await?;
        let data = r["data"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: 无整页截图数据"))?;
        crate::util::base64_decode(data).ok_or_else(|| Error::msg("CDP: 整页截图 base64 解码失败"))
    }

    // ── 监听 / 拦截句柄 ──────────────────────────────────────────────────

    /// 网络监听句柄(对标 DP `tab.listen`):`start`/`wait`/`wait_count`/`stop`。
    pub fn listen(&self) -> CdpListen {
        CdpListen::new(self.core.clone())
    }

    /// 请求拦截句柄(对标 DP 拦截增强):`start`/`next`/`stop`,请求级 `resume`/`fulfill`/`abort`。
    pub fn intercept(&self) -> CdpIntercept {
        CdpIntercept::new(self.core.clone())
    }
}

/// 在 `document` 上查元素的 JS 表达式(CSS 走 `querySelector(All)`、xpath 走 `document.evaluate`)。
fn doc_query_expr(selector: &str, single: bool) -> String {
    match locator::parse(selector) {
        Query::Css(sel) => {
            let s = serde_json::to_string(&sel).unwrap_or_else(|_| "\"\"".into());
            if single {
                format!("document.querySelector({s})")
            } else {
                format!("Array.from(document.querySelectorAll({s}))")
            }
        }
        Query::Xpath(xp) => {
            let s = serde_json::to_string(&xp).unwrap_or_else(|_| "\"\"".into());
            if single {
                format!("document.evaluate({s}, document, null, 9, null).singleNodeValue")
            } else {
                format!(
                    "(function(){{ const it=document.evaluate({s}, document, null, 7, null); \
                     const a=[]; for (let i=0;i<it.snapshotLength;i++) a.push(it.snapshotItem(i)); return a; }})()"
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::doc_query_expr;

    #[test]
    fn css_query_expr() {
        // 裸 `h1` 在 DP 语法里是“文本包含”;CSS 标签用 `css:`/`tag:` 前缀,#/. 简写也是 CSS。
        assert_eq!(
            doc_query_expr("css:h1", true),
            "document.querySelector(\"h1\")"
        );
        assert_eq!(
            doc_query_expr("#a .b", false),
            "Array.from(document.querySelectorAll(\"#a .b\"))"
        );
    }

    #[test]
    fn xpath_query_expr() {
        let s = doc_query_expr("xpath://div[@id=\"x\"]", true);
        assert!(s.starts_with("document.evaluate("));
        assert!(s.contains("singleNodeValue"));
    }
}
