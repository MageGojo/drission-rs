//! 链式等待:`tab.wait().element("#login").visible().timeout(...)`。
//! 超时返回 `false`,不报错。不要用 `sleep` 代替。

use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::time::Duration;

use tokio::time::{Instant, sleep};

use super::handles::ChromiumWait;
use crate::Result;

impl ChromiumWait {
    /// 等元素出现;接 `.visible()` / `.timeout()` 后 `.await`。
    pub fn element(self, selector: impl Into<String>) -> WaitElement {
        WaitElement {
            wait: self,
            selector: selector.into(),
            visible: false,
            gone: false,
            timeout: None,
        }
    }

    pub fn url(self, needle: impl Into<String>) -> WaitText {
        WaitText {
            wait: self,
            kind: WaitKind::Url,
            needle: needle.into(),
            timeout: None,
        }
    }

    pub fn title(self, needle: impl Into<String>) -> WaitText {
        WaitText {
            wait: self,
            kind: WaitKind::Title,
            needle: needle.into(),
            timeout: None,
        }
    }

    pub fn text(self, needle: impl Into<String>) -> WaitText {
        WaitText {
            wait: self,
            kind: WaitKind::Text,
            needle: needle.into(),
            timeout: None,
        }
    }

    pub fn function(self, js: impl Into<String>) -> WaitText {
        WaitText {
            wait: self,
            kind: WaitKind::Function,
            needle: js.into(),
            timeout: None,
        }
    }

    /// 等本页弹出的新标签 / `window.open`。
    pub fn popup(self) -> WaitPopup {
        WaitPopup {
            wait: self,
            timeout: None,
        }
    }

    /// 等网络空闲,或等某个请求 URL。
    pub fn network(self) -> WaitNetwork {
        WaitNetwork {
            wait: self,
            idle_secs: 0.5,
            url: None,
            timeout: None,
        }
    }

    /// 等下载开始。
    pub fn download(self) -> WaitDownload {
        WaitDownload {
            wait: self,
            timeout: None,
        }
    }

    /// `document.readyState === complete`。
    pub async fn idle(&self, timeout: Option<Duration>) -> Result<bool> {
        self.doc_loaded(timeout).await
    }

    /// 正文 `innerText` 含子串。
    pub async fn text_contains(&self, sub: &str, timeout: Option<Duration>) -> Result<bool> {
        poll(&self.core, timeout, |core| async move {
            let t = core
                .eval_value("document.body ? document.body.innerText : ''")
                .await
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            Ok(t.contains(sub))
        })
        .await
    }

    /// 等表达式为真。
    pub async fn js_true(&self, js: &str, timeout: Option<Duration>) -> Result<bool> {
        poll(&self.core, timeout, |core| async move {
            Ok(core
                .eval_value(js)
                .await
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false))
        })
        .await
    }
}

/// `wait().element(...)` 构建器。
pub struct WaitElement {
    wait: ChromiumWait,
    selector: String,
    visible: bool,
    gone: bool,
    timeout: Option<Duration>,
}

impl WaitElement {
    pub fn visible(mut self) -> Self {
        self.visible = true;
        self
    }
    pub fn deleted(mut self) -> Self {
        self.gone = true;
        self
    }
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }
    pub async fn run(self) -> Result<bool> {
        if self.gone {
            self.wait.ele_deleted(&self.selector, self.timeout).await
        } else if self.visible {
            self.wait.ele_displayed(&self.selector, self.timeout).await
        } else {
            self.wait.ele_exists(&self.selector, self.timeout).await
        }
    }
}

impl IntoFuture for WaitElement {
    type Output = Result<bool>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<bool>> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

#[derive(Clone, Copy)]
enum WaitKind {
    Url,
    Title,
    Text,
    Function,
}

/// `wait().url/title/text/function(...)` 构建器。
pub struct WaitText {
    wait: ChromiumWait,
    kind: WaitKind,
    needle: String,
    timeout: Option<Duration>,
}

impl WaitText {
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }
    pub async fn run(self) -> Result<bool> {
        match self.kind {
            WaitKind::Url => self.wait.url_contains(&self.needle, self.timeout).await,
            WaitKind::Title => self.wait.title_contains(&self.needle, self.timeout).await,
            WaitKind::Text => self.wait.text_contains(&self.needle, self.timeout).await,
            WaitKind::Function => self.wait.js_true(&self.needle, self.timeout).await,
        }
    }
}

impl IntoFuture for WaitText {
    type Output = Result<bool>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<bool>> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

/// `wait().popup()`。
pub struct WaitPopup {
    wait: ChromiumWait,
    timeout: Option<Duration>,
}

impl WaitPopup {
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }
    pub async fn run(self) -> Result<Option<crate::cdp::ChromiumTab>> {
        self.wait.new_tab(self.timeout).await
    }
}

impl IntoFuture for WaitPopup {
    type Output = Result<Option<crate::cdp::ChromiumTab>>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<Option<crate::cdp::ChromiumTab>>> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

/// `wait().network()`。默认等空闲;`.url("/api/")` 改等请求。
pub struct WaitNetwork {
    wait: ChromiumWait,
    idle_secs: f64,
    url: Option<String>,
    timeout: Option<Duration>,
}

impl WaitNetwork {
    pub fn idle(mut self) -> Self {
        self.url = None;
        self
    }
    pub fn quiet(mut self, secs: f64) -> Self {
        self.idle_secs = secs;
        self
    }
    pub fn url(mut self, needle: impl Into<String>) -> Self {
        self.url = Some(needle.into());
        self
    }
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }
    pub async fn run(self) -> Result<bool> {
        if let Some(needle) = &self.url {
            self.wait.request_url_contains(needle, self.timeout).await
        } else {
            self.wait.network_idle(self.idle_secs, self.timeout).await
        }
    }
}

impl IntoFuture for WaitNetwork {
    type Output = Result<bool>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<bool>> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

/// `wait().download()`。
pub struct WaitDownload {
    wait: ChromiumWait,
    timeout: Option<Duration>,
}

impl WaitDownload {
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }
    pub async fn run(self) -> Result<bool> {
        self.wait.download_begin(self.timeout).await
    }
}

impl IntoFuture for WaitDownload {
    type Output = Result<bool>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<bool>> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

impl ChromiumWait {
    /// 等出现 URL 含子串的网络请求。
    pub async fn request_url_contains(&self, sub: &str, timeout: Option<Duration>) -> Result<bool> {
        use serde_json::json;
        use tokio::sync::broadcast::error::RecvError;
        let lit = json!(sub);
        let seen_js = format!(
            "performance.getEntries().some(function(e){{return String(e.name).includes({lit});}})"
        );
        let already = self
            .core
            .eval_value(&seen_js)
            .await
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if already {
            return Ok(true);
        }
        self.core.send("Network.enable", json!({})).await?;
        let mut events = self.core.conn.subscribe();
        let sid = self.core.session_id.clone();
        let deadline = Instant::now() + timeout.unwrap_or_else(|| self.core.timeout());
        loop {
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                return Ok(false);
            }
            if self
                .core
                .eval_value(&seen_js)
                .await
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                return Ok(true);
            }
            let ev =
                match tokio::time::timeout(remain.min(Duration::from_millis(120)), events.recv())
                    .await
                {
                    Ok(Ok(ev)) => ev,
                    Ok(Err(RecvError::Lagged(_))) => continue,
                    Ok(Err(RecvError::Closed)) => return Ok(false),
                    Err(_) => continue,
                };
            if ev.session_id.as_deref() != Some(sid.as_str()) {
                continue;
            }
            if ev.method != "Network.requestWillBeSent" {
                continue;
            }
            let url = ev.params["request"]["url"].as_str().unwrap_or_default();
            if url.contains(sub) {
                return Ok(true);
            }
        }
    }

    pub async fn ele_exists(&self, selector: &str, timeout: Option<Duration>) -> Result<bool> {
        use crate::cdp::element::ChromiumElement;
        use crate::cdp::tab::doc_query_expr;
        let deadline = Instant::now() + timeout.unwrap_or_else(|| self.core.timeout());
        loop {
            if self
                .core
                .eval_handle(&doc_query_expr(selector, true))
                .await?
                .map(|oid| ChromiumElement::new(self.core.clone(), oid))
                .is_some()
            {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(Duration::from_millis(80)).await;
        }
    }
}

async fn poll<F, Fut>(
    core: &std::sync::Arc<crate::cdp::core::CdpCore>,
    timeout: Option<Duration>,
    mut check: F,
) -> Result<bool>
where
    F: FnMut(std::sync::Arc<crate::cdp::core::CdpCore>) -> Fut,
    Fut: Future<Output = Result<bool>>,
{
    let deadline = Instant::now() + timeout.unwrap_or_else(|| core.timeout());
    loop {
        if check(core.clone()).await? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        sleep(Duration::from_millis(80)).await;
    }
}
