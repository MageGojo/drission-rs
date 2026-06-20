//! CDP 请求拦截 [`CdpIntercept`] + [`CdpInterceptedRequest`](对标 Juggler 后端的
//! [`Intercept`](crate::browser::Intercept) / [`InterceptedRequest`](crate::browser::InterceptedRequest))。
//!
//! 走 CDP **`Fetch` 域**:`Fetch.enable` 后所有请求在 `Fetch.requestPaused` 暂停;库把**匹配过滤**
//! 的请求投递给用户决策([`CdpInterceptedRequest`] 的 `resume`/`resume_with`/`fulfill`/`abort`),
//! **不匹配**的自动 `Fetch.continueRequest` 放行(避免页面卡死)。
//!
//! 决策可覆盖字段复用后端无关的 [`ResumeOptions`],与 Juggler 后端 API 一致。

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::Result;
use crate::browser::interceptor::ResumeOptions;
use crate::browser::listener::ListenFilter;
use crate::cdp::core::CdpCore;
use crate::protocol::Connection;
use crate::util::base64_encode;

/// 拦截共享状态(放在 [`CdpCore`] 里)。
#[derive(Default)]
pub(crate) struct InterceptShared {
    pub(crate) running: bool,
    pub(crate) abort: Option<AbortHandle>,
    pub(crate) rx: Option<mpsc::UnboundedReceiver<CdpInterceptedRequest>>,
}

/// 请求拦截句柄(`tab.intercept()` 返回)。
pub struct CdpIntercept {
    core: Arc<CdpCore>,
}

impl CdpIntercept {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self { core }
    }

    /// 开始拦截:`keywords` 为 URL 子串过滤(空=全部)。匹配的请求交用户决策,不匹配自动放行。
    pub async fn start(&self, keywords: &[&str]) -> Result<()> {
        self.start_with(keywords, false).await
    }

    /// 开始拦截,仅 XHR/fetch 类请求(其余自动放行)。
    pub async fn start_xhr(&self, keywords: &[&str]) -> Result<()> {
        self.start_with(keywords, true).await
    }

    async fn start_with(&self, keywords: &[&str], xhr_only: bool) -> Result<()> {
        let filter = ListenFilter {
            url_keywords: keywords.iter().map(|s| s.to_string()).collect(),
            xhr_only,
        };
        self.stop().await?;
        self.core
            .send(
                "Fetch.enable",
                json!({ "patterns": [{ "urlPattern": "*" }] }),
            )
            .await?;
        let (tx, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(intercept_pump(
            self.core.conn.clone(),
            self.core.session_id.clone(),
            filter,
            tx,
        ));
        let mut g = self.core.intercept.lock().await;
        g.running = true;
        g.abort = Some(task.abort_handle());
        g.rx = Some(rx);
        Ok(())
    }

    /// 是否正在拦截。
    pub async fn is_intercepting(&self) -> bool {
        self.core.intercept.lock().await.running
    }

    /// 取下一个被拦截、待决策的请求(`timeout=None` 用标签默认超时);超时返回 `None`。
    ///
    /// 拿到后必须调用其 `resume`/`resume_with`/`fulfill`/`abort` 之一放行。
    pub async fn next(
        &self,
        timeout: Option<std::time::Duration>,
    ) -> Result<Option<CdpInterceptedRequest>> {
        let d = timeout.unwrap_or_else(|| self.core.timeout());
        // 取出接收端(避免跨 await 持锁),用完放回。
        let mut rx = match self.core.intercept.lock().await.rx.take() {
            Some(rx) => rx,
            None => return Ok(None),
        };
        let got = tokio::time::timeout(d, rx.recv()).await.ok().flatten();
        self.core.intercept.lock().await.rx = Some(rx);
        Ok(got)
    }

    /// 停止拦截:中止后台任务、关 `Fetch` 域。
    pub async fn stop(&self) -> Result<()> {
        let abort = {
            let mut g = self.core.intercept.lock().await;
            g.running = false;
            g.rx = None;
            g.abort.take()
        };
        if let Some(a) = abort {
            a.abort();
            let _ = self.core.send("Fetch.disable", json!({})).await;
        }
        Ok(())
    }
}

/// 一个被拦截、等待决策的请求(CDP 后端)。决策方法消费 `self`,类型层面保证每请求只决策一次。
pub struct CdpInterceptedRequest {
    /// 请求 URL。
    pub url: String,
    /// 请求方法(GET/POST…)。
    pub method: String,
    /// 资源类型(CDP 的 `resourceType`,如 `XHR`/`Fetch`/`Document`)。
    pub resource_type: String,
    /// 请求头。
    pub headers: Vec<(String, String)>,
    /// 请求体(若有)。
    pub post_data: Option<String>,
    request_id: String,
    conn: Connection,
    session_id: String,
}

impl CdpInterceptedRequest {
    async fn fetch(&self, method: &str, params: Value) -> Result<()> {
        self.conn
            .send(method, params, Some(&self.session_id))
            .await?;
        Ok(())
    }

    /// 原样放行。
    pub async fn resume(self) -> Result<()> {
        self.fetch(
            "Fetch.continueRequest",
            json!({ "requestId": self.request_id }),
        )
        .await
    }

    /// 改写后放行(可改 `url` / `method` / `headers` / `postData`)。
    pub async fn resume_with(self, opts: ResumeOptions) -> Result<()> {
        let mut p = serde_json::Map::new();
        p.insert("requestId".into(), json!(self.request_id));
        if let Some(u) = opts.url {
            p.insert("url".into(), json!(u));
        }
        if let Some(m) = opts.method {
            p.insert("method".into(), json!(m));
        }
        if let Some(h) = opts.headers {
            p.insert("headers".into(), json!(to_header_array(&h)));
        }
        if let Some(d) = opts.post_data {
            // CDP 的 continueRequest.postData 为 base64。
            p.insert("postData".into(), json!(base64_encode(d.as_bytes())));
        }
        self.fetch("Fetch.continueRequest", Value::Object(p)).await
    }

    /// 直接用伪造响应满足请求(不真正发往服务器)。`body` 为文本响应体。
    pub async fn fulfill(
        self,
        status: u16,
        headers: Vec<(String, String)>,
        body: &str,
    ) -> Result<()> {
        let p = json!({
            "requestId": self.request_id,
            "responseCode": status,
            "responseHeaders": to_header_array(&headers),
            "body": base64_encode(body.as_bytes()),
        });
        self.fetch("Fetch.fulfillRequest", p).await
    }

    /// 中止请求。`error_code` 兼容 Juggler 风格(`failed`/`aborted`/`timedout`/`namenotresolved`
    /// /`connectionrefused`/`blockedbyclient`…),内部映射为 CDP `errorReason`。
    pub async fn abort(self, error_code: &str) -> Result<()> {
        self.fetch(
            "Fetch.failRequest",
            json!({ "requestId": self.request_id, "errorReason": fail_reason(error_code) }),
        )
        .await
    }

    /// 底层 requestId(调试用)。
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

/// 拦截后台任务:订阅事件,把匹配过滤的 `Fetch.requestPaused` 投递给用户,其余自动放行。
///
/// 只持有 `conn`/`session_id`(不持 [`CdpCore`]),便于 `CdpCore` 析构时 `abort`。
async fn intercept_pump(
    conn: Connection,
    session_id: String,
    filter: ListenFilter,
    tx: mpsc::UnboundedSender<CdpInterceptedRequest>,
) {
    let auto_continue = |request_id: &str| {
        let conn = conn.clone();
        let sid = session_id.clone();
        let id = request_id.to_string();
        async move {
            let _ = conn
                .send(
                    "Fetch.continueRequest",
                    json!({ "requestId": id }),
                    Some(&sid),
                )
                .await;
        }
    };

    let mut events = conn.subscribe();
    loop {
        let ev = match events.recv().await {
            Ok(ev) => ev,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        };
        if ev.session_id.as_deref() != Some(session_id.as_str()) {
            continue;
        }
        if ev.method != "Fetch.requestPaused" {
            continue;
        }
        let Some(request_id) = ev.params["requestId"].as_str() else {
            continue;
        };
        let req = &ev.params["request"];
        let url = req["url"].as_str().unwrap_or_default().to_string();
        let resource_type = ev.params["resourceType"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // 不匹配:自动放行,别让请求悬着。
        if !filter.matches(&url, &resource_type) {
            auto_continue(request_id).await;
            continue;
        }

        let intercepted = CdpInterceptedRequest {
            url,
            method: req["method"].as_str().unwrap_or_default().to_string(),
            resource_type,
            headers: header_map_to_pairs(&req["headers"]),
            post_data: req["postData"].as_str().map(str::to_string),
            request_id: request_id.to_string(),
            conn: conn.clone(),
            session_id: session_id.clone(),
        };
        // 用户没在等(接收端已丢弃):自动放行。
        if tx.send(intercepted).is_err() {
            auto_continue(request_id).await;
        }
    }
}

/// `[(name,value)]` → CDP 的 `[{name,value}]` 头数组。
fn to_header_array(headers: &[(String, String)]) -> Vec<Value> {
    headers
        .iter()
        .map(|(n, v)| json!({ "name": n, "value": v }))
        .collect()
}

/// CDP 的请求头是对象 `{name: value}`;转成键值对。
fn header_map_to_pairs(v: &Value) -> Vec<(String, String)> {
    v.as_object()
        .map(|o| {
            o.iter()
                .map(|(k, val)| (k.clone(), val.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Juggler 风格错误码 → CDP `Network.ErrorReason` 枚举(未知归 `Failed`)。
fn fail_reason(code: &str) -> &'static str {
    match code.to_ascii_lowercase().as_str() {
        "aborted" => "Aborted",
        "timedout" | "timeout" => "TimedOut",
        "accessdenied" => "AccessDenied",
        "connectionclosed" => "ConnectionClosed",
        "connectionreset" => "ConnectionReset",
        "connectionrefused" => "ConnectionRefused",
        "namenotresolved" => "NameNotResolved",
        "internetdisconnected" => "InternetDisconnected",
        "addressunreachable" => "AddressUnreachable",
        "blockedbyclient" => "BlockedByClient",
        "blockedbyresponse" => "BlockedByResponse",
        _ => "Failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fail_reason_maps_known_and_unknown() {
        assert_eq!(fail_reason("aborted"), "Aborted");
        assert_eq!(fail_reason("BlockedByClient"), "BlockedByClient");
        assert_eq!(fail_reason("namenotresolved"), "NameNotResolved");
        assert_eq!(fail_reason("whatever"), "Failed");
    }

    #[test]
    fn header_array_shape() {
        let h = vec![("Content-Type".to_string(), "text/html".to_string())];
        let arr = to_header_array(&h);
        assert_eq!(arr[0]["name"], "Content-Type");
        assert_eq!(arr[0]["value"], "text/html");
    }
}
