//! CDP 网络监听 [`CdpListen`](对标 DrissionPage 的 `tab.listen` 与 Camoufox 后端的 `Listen`)。
//!
//! 与 Juggler 后端用页面 `fetch/XHR` hook 不同,CDP 走**原生 `Network` 域事件**
//! (`requestWillBeSent`/`responseReceived`/`loadingFinished`)聚合,响应体用
//! `Network.getResponseBody`(CDP 下稳定可靠,含同源与跨域)。**不污染页面**、覆盖所有资源类型。
//!
//! 数据类型复用后端无关的 [`DataPacket`]/[`ListenFilter`],与 Juggler 后端 API 一致。

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Mutex;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::AbortHandle;
use tokio::time::{Instant, sleep};

use crate::Result;
use crate::cdp::core::CdpCore;
use crate::net::{DataPacket, ListenFilter, RequestData, ResponseData};
use crate::protocol::Connection;

/// 标签内共享的网络监听缓冲(后台任务写入、句柄读取)。
pub(crate) type SharedBuf = Arc<Mutex<VecDeque<DataPacket>>>;

/// 监听共享状态(放在 [`CdpCore`] 里,供同一标签的多个句柄共享)。
///
/// `buf` 用独立 `Arc` 而非内嵌——后台任务只持有 `buf`/`Connection`(不持 [`CdpCore`]),
/// 从而 `CdpCore` 析构时能 `abort` 任务、不形成"任务↔core"自环。
pub(crate) struct ListenShared {
    pub(crate) buf: SharedBuf,
    pub(crate) running: bool,
    pub(crate) abort: Option<AbortHandle>,
}

impl Default for ListenShared {
    fn default() -> Self {
        Self {
            buf: Arc::new(Mutex::new(VecDeque::new())),
            running: false,
            abort: None,
        }
    }
}

/// 监听缓冲上限(超出丢最旧,避免长监听撑爆内存)。
const BUFFER_CAP: usize = 300;

/// 网络监听句柄(`tab.listen()` 返回;轻量包裹 [`CdpCore`])。
pub struct CdpListen {
    core: Arc<CdpCore>,
}

impl CdpListen {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self { core }
    }

    /// 开始监听:`keywords` 为 URL 子串过滤(空=全部),覆盖所有资源类型。
    pub async fn start(&self, keywords: &[&str]) -> Result<()> {
        self.start_with(keywords, false).await
    }

    /// 开始监听,仅 XHR/fetch 类请求。
    pub async fn start_xhr(&self, keywords: &[&str]) -> Result<()> {
        self.start_with(keywords, true).await
    }

    async fn start_with(&self, keywords: &[&str], xhr_only: bool) -> Result<()> {
        let filter = ListenFilter {
            url_keywords: keywords.iter().map(|s| s.to_string()).collect(),
            xhr_only,
        };
        // 先停掉旧的(如有),再重开。
        self.stop().await?;
        self.core
            .send("Network.enable", serde_json::json!({}))
            .await?;
        let buf = self.core.listen.lock().await.buf.clone();
        let task = tokio::spawn(listen_pump(
            self.core.conn.clone(),
            self.core.session_id.clone(),
            filter,
            buf,
        ));
        let mut g = self.core.listen.lock().await;
        g.running = true;
        g.abort = Some(task.abort_handle());
        Ok(())
    }

    /// 是否正在监听。
    pub async fn is_listening(&self) -> bool {
        self.core.listen.lock().await.running
    }

    /// 等待**一个**数据包(`timeout=None` 用标签默认超时);超时返回 `None`。
    pub async fn wait(&self, timeout: Option<Duration>) -> Result<Option<DataPacket>> {
        let buf = self.core.listen.lock().await.buf.clone();
        let deadline = Instant::now() + timeout.unwrap_or_else(|| self.core.timeout());
        loop {
            if let Some(p) = buf.lock().await.pop_front() {
                return Ok(Some(p));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            sleep(Duration::from_millis(50)).await;
        }
    }

    /// 在总超时内尽量收集 `n` 个数据包(不足返回已收到的)。
    pub async fn wait_count(&self, n: usize, timeout: Option<Duration>) -> Result<Vec<DataPacket>> {
        let buf = self.core.listen.lock().await.buf.clone();
        let deadline = Instant::now() + timeout.unwrap_or_else(|| self.core.timeout());
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            if let Some(p) = buf.lock().await.pop_front() {
                out.push(p);
                continue;
            }
            if Instant::now() >= deadline {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
        Ok(out)
    }

    /// 停止监听:中止后台任务、关 `Network` 域、清空缓冲。
    pub async fn stop(&self) -> Result<()> {
        let (abort, buf) = {
            let mut g = self.core.listen.lock().await;
            g.running = false;
            (g.abort.take(), g.buf.clone())
        };
        buf.lock().await.clear();
        if let Some(a) = abort {
            a.abort();
            let _ = self
                .core
                .send("Network.disable", serde_json::json!({}))
                .await;
        }
        Ok(())
    }
}

/// 监听后台任务:订阅连接事件,聚合 `Network.*` 成 [`DataPacket`] 推入缓冲。
///
/// 只持有 `conn`/`session_id`/`buf`(不持 [`CdpCore`]),便于 `CdpCore` 析构时 `abort`。
async fn listen_pump(conn: Connection, session_id: String, filter: ListenFilter, buf: SharedBuf) {
    let mut events = conn.subscribe();
    // requestId → 累积中的请求/响应信息。
    let mut pending: HashMap<String, Partial> = HashMap::new();

    loop {
        let ev = match events.recv().await {
            Ok(ev) => ev,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        };
        if ev.session_id.as_deref() != Some(session_id.as_str()) {
            continue;
        }
        match ev.method.as_str() {
            "Network.requestWillBeSent" => {
                if let Some(id) = ev.params["requestId"].as_str() {
                    let req = &ev.params["request"];
                    pending.insert(
                        id.to_string(),
                        Partial {
                            url: req["url"].as_str().unwrap_or_default().to_string(),
                            method: req["method"].as_str().unwrap_or_default().to_string(),
                            resource_type: ev.params["type"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            req_headers: header_map_to_pairs(&req["headers"]),
                            post_data: req["postData"].as_str().map(str::to_string),
                            resp: None,
                        },
                    );
                }
            }
            "Network.responseReceived" => {
                if let Some(id) = ev.params["requestId"].as_str() {
                    if let Some(p) = pending.get_mut(id) {
                        let resp = &ev.params["response"];
                        // 资源类型以 response 事件为准更新(更准确)。
                        if let Some(t) = ev.params["type"].as_str() {
                            p.resource_type = t.to_string();
                        }
                        p.resp = Some(PartialResp {
                            status: resp["status"].as_u64().unwrap_or(0) as u16,
                            status_text: resp["statusText"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            headers: header_map_to_pairs(&resp["headers"]),
                        });
                    }
                }
            }
            "Network.loadingFinished" => {
                if let Some(id) = ev.params["requestId"].as_str() {
                    if let Some(p) = pending.remove(id) {
                        if filter.matches(&p.url, &p.resource_type) {
                            let packet = assemble(&conn, &session_id, id, p).await;
                            let mut g = buf.lock().await;
                            if g.len() >= BUFFER_CAP {
                                g.pop_front();
                            }
                            g.push_back(packet);
                        }
                    }
                }
            }
            "Network.loadingFailed" => {
                if let Some(id) = ev.params["requestId"].as_str() {
                    pending.remove(id);
                }
            }
            _ => {}
        }
    }
}

/// 累积中的请求(等响应体到齐再组装)。
struct Partial {
    url: String,
    method: String,
    resource_type: String,
    req_headers: Vec<(String, String)>,
    post_data: Option<String>,
    resp: Option<PartialResp>,
}

struct PartialResp {
    status: u16,
    status_text: String,
    headers: Vec<(String, String)>,
}

/// 取响应体并组装成 [`DataPacket`]。
async fn assemble(conn: &Connection, session_id: &str, request_id: &str, p: Partial) -> DataPacket {
    let (status, status_text, headers) = match p.resp {
        Some(r) => (r.status, r.status_text, r.headers),
        None => (0, String::new(), Vec::new()),
    };
    let (body, body_base64) = fetch_body(conn, session_id, request_id).await;
    DataPacket {
        url: p.url,
        method: p.method,
        resource_type: p.resource_type,
        request: RequestData {
            headers: p.req_headers,
            post_data: p.post_data,
        },
        response: ResponseData {
            status,
            status_text,
            headers,
            body,
            body_base64,
        },
    }
}

/// 取响应体:`Network.getResponseBody` → 文本直接用;base64 编码的留在 `body_base64`,并尽力解出文本。
async fn fetch_body(conn: &Connection, session_id: &str, request_id: &str) -> (String, String) {
    let r = match conn
        .send(
            "Network.getResponseBody",
            serde_json::json!({ "requestId": request_id }),
            Some(session_id),
        )
        .await
    {
        Ok(r) => r,
        Err(_) => return (String::new(), String::new()),
    };
    let raw = r["body"].as_str().unwrap_or_default();
    if r["base64Encoded"].as_bool().unwrap_or(false) {
        let text = crate::util::base64_decode(raw)
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default();
        (text, raw.to_string())
    } else {
        (raw.to_string(), String::new())
    }
}

/// CDP 的 `headers` 是对象 `{name: value}`(value 可能是字符串或换行拼接);转成键值对。
fn header_map_to_pairs(v: &Value) -> Vec<(String, String)> {
    v.as_object()
        .map(|o| {
            o.iter()
                .map(|(k, val)| (k.clone(), val.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn header_map_parses_object() {
        let v = json!({ "content-type": "application/json", "x-id": "42" });
        let mut pairs = header_map_to_pairs(&v);
        pairs.sort();
        assert_eq!(pairs.len(), 2);
        assert!(pairs.contains(&("content-type".to_string(), "application/json".to_string())));
        assert!(pairs.contains(&("x-id".to_string(), "42".to_string())));
    }

    #[test]
    fn header_map_non_object_is_empty() {
        assert!(header_map_to_pairs(&json!(null)).is_empty());
        assert!(header_map_to_pairs(&json!("x")).is_empty());
    }
}
