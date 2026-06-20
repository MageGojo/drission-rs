//! 请求拦截(对 `tab.listen` 监听之外的增强:放行 / 改写 / 伪造 / 中止)。
//!
//! Juggler 模型:在 page 会话上 `Network.setRequestInterception{enabled:true}` 开启后,
//! 该页的**所有**请求都会在 `Network.requestWillBeSent` 中带 `isIntercepted:true` 并暂停,
//! 必须对每个被拦请求**恰好调用一次** `resume` / `fulfill` / `abort`。
//!
//! 本模块策略:**匹配过滤条件的请求**投递给用户决策([`InterceptedRequest`]);
//! **不匹配的**由库自动放行(`resume`),避免开了拦截后页面整体卡死。

use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::Result;
use crate::browser::listener::parse_headers;
use crate::net::ListenFilter;
use crate::protocol::Connection;
use crate::util::base64_encode;

/// 改写放行的可选覆盖字段 [`ResumeOptions`] 统一定义在 [`crate::net`];此处再导出,保持
/// `browser::interceptor::ResumeOptions` 老路径可用。
pub use crate::net::ResumeOptions;

/// 一个被拦截、等待决策的请求。
///
/// 必须调用 [`resume`](Self::resume) / [`resume_with`](Self::resume_with) /
/// [`fulfill`](Self::fulfill) / [`abort`](Self::abort) 之一来放行(方法消费 `self`,
/// 类型层面保证每个请求只决策一次)。
pub struct InterceptedRequest {
    /// 请求 URL。
    pub url: String,
    /// 请求方法(GET/POST…)。
    pub method: String,
    /// 资源类型(Juggler 的 cause,如 `xhr`/`fetch`/`document`/`TYPE_*`)。
    pub resource_type: String,
    /// 请求头。
    pub headers: Vec<(String, String)>,
    /// 请求体(若有)。
    pub post_data: Option<String>,
    request_id: String,
    conn: Connection,
    session_id: String,
}

impl InterceptedRequest {
    /// 原样放行。
    pub async fn resume(self) -> Result<()> {
        self.conn
            .send(
                "Network.resumeInterceptedRequest",
                json!({ "requestId": self.request_id }),
                Some(&self.session_id),
            )
            .await?;
        Ok(())
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
            p.insert("postData".into(), json!(d));
        }
        self.conn
            .send(
                "Network.resumeInterceptedRequest",
                Value::Object(p),
                Some(&self.session_id),
            )
            .await?;
        Ok(())
    }

    /// 直接用伪造的响应满足请求(不真正发往服务器)。`body` 为文本响应体。
    pub async fn fulfill(
        self,
        status: u16,
        headers: Vec<(String, String)>,
        body: &str,
    ) -> Result<()> {
        let p = json!({
            "requestId": self.request_id,
            "status": status,
            "statusText": status_text(status),
            "headers": to_header_array(&headers),
            "base64body": base64_encode(body.as_bytes()),
        });
        self.conn
            .send(
                "Network.fulfillInterceptedRequest",
                p,
                Some(&self.session_id),
            )
            .await?;
        Ok(())
    }

    /// 中止请求。`error_code` 如 `failed`/`aborted`/`accessdenied`/`connectionrefused`/
    /// `connectionreset`/`namenotresolved`/`timedout`/`blockedbyclient` 等。
    pub async fn abort(self, error_code: &str) -> Result<()> {
        self.conn
            .send(
                "Network.abortInterceptedRequest",
                json!({ "requestId": self.request_id, "errorCode": error_code }),
                Some(&self.session_id),
            )
            .await?;
        Ok(())
    }

    /// 底层 requestId(调试用)。
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

/// pump 对一次 `requestWillBeSent` 的处置结论。
pub(crate) enum Decision {
    /// 已投递给用户决策。
    Delivered,
    /// 不匹配过滤,需库自动放行该 requestId。
    AutoResume(String),
    /// 非拦截请求(`isIntercepted=false`)或未开启拦截,忽略。
    Ignore,
}

/// 拦截器内部状态(放在 `TabCore` 的 `Mutex` 中)。
pub(crate) struct InterceptorState {
    filter: ListenFilter,
    tx: mpsc::UnboundedSender<InterceptedRequest>,
    conn: Connection,
    session_id: String,
}

impl InterceptorState {
    pub(crate) fn new(
        filter: ListenFilter,
        conn: Connection,
        session_id: String,
    ) -> (Self, mpsc::UnboundedReceiver<InterceptedRequest>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                filter,
                tx,
                conn,
                session_id,
            },
            rx,
        )
    }

    /// 处理一条 `Network.requestWillBeSent`。
    pub(crate) fn on_request_will_be_sent(&self, params: &Value) -> Decision {
        if !params["isIntercepted"].as_bool().unwrap_or(false) {
            return Decision::Ignore;
        }
        let Some(request_id) = params["requestId"].as_str() else {
            return Decision::Ignore;
        };
        let url = params["url"].as_str().unwrap_or_default().to_string();
        let cause = params["cause"].as_str().unwrap_or_default();
        let internal = params["internalCause"].as_str().unwrap_or_default();
        let resource_type = if !cause.is_empty() { cause } else { internal }.to_string();

        // 不匹配过滤:交回库自动放行,避免页面卡死。
        if !self.filter.matches(&url, &resource_type) {
            return Decision::AutoResume(request_id.to_string());
        }

        let req = InterceptedRequest {
            url,
            method: params["method"].as_str().unwrap_or_default().to_string(),
            resource_type,
            headers: parse_headers(&params["headers"]),
            post_data: params["postData"].as_str().map(str::to_string),
            request_id: request_id.to_string(),
            conn: self.conn.clone(),
            session_id: self.session_id.clone(),
        };
        // 接收端已丢弃(用户没在等):自动放行,别让请求悬着。
        if self.tx.send(req).is_err() {
            return Decision::AutoResume(request_id.to_string());
        }
        Decision::Delivered
    }
}

/// `[(name,value)]` → Juggler 的 `[{name,value}]` 头数组。
fn to_header_array(headers: &[(String, String)]) -> Vec<Value> {
    headers
        .iter()
        .map(|(n, v)| json!({ "name": n, "value": v }))
        .collect()
}

/// 常见状态码的标准 reason phrase;未知返回空串(协议只要求是字符串)。
fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_array_shape() {
        let h = vec![("Content-Type".to_string(), "text/html".to_string())];
        let arr = to_header_array(&h);
        assert_eq!(arr[0]["name"], "Content-Type");
        assert_eq!(arr[0]["value"], "text/html");
    }

    #[test]
    fn status_text_known() {
        assert_eq!(status_text(200), "OK");
        assert_eq!(status_text(404), "Not Found");
        assert_eq!(status_text(599), "");
    }
}
