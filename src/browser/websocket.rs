//! WebSocket 帧监听(drission-rs 增强;DrissionPage 无对应原生 API)。
//!
//! 实现方式:订阅 Camoufox/Juggler 的**原生** `Page.webSocket*` 事件(与控制台监听同机制:
//! 不 hook 页面 `WebSocket` 对象,因此不污染页面、对反检测更友好)。`websocket().start()` 起一个
//! 后台任务持续把本会话的帧搬进缓冲,`wait/messages/steps` 取回。
//!
//! Juggler 事件(均在 page 会话上自动下发,无需 enable):
//! - `Page.webSocketCreated {frameId, wsid, requestURL}`:连接创建。
//! - `Page.webSocketOpened {frameId, requestId, wsid, effectiveURL}`:握手完成(URL 更准确)。
//! - `Page.webSocketFrameSent {frameId, wsid, opcode, data}`:发出一帧。
//! - `Page.webSocketFrameReceived {frameId, wsid, opcode, data}`:收到一帧。
//! - `Page.webSocketClosed {frameId, wsid, error}`:连接关闭。
//!
//! **数据编码**(关键):Juggler 对 `data` 的处理是 `opcode === 1 ? payload : btoa(payload)`——
//! 即**文本帧(opcode 1)是原始文本**,**其余帧(二进制 2 / 控制 8/9/10)是 base64**。
//! 故 [`WsMessage::data`] 原样保留;取文本用 [`text`](WsMessage::text)、取字节用
//! [`bytes`](WsMessage::bytes)、取 JSON 用 [`json`](WsMessage::json)。
//!
//! ```ignore
//! let ws = tab.websocket();
//! ws.start().await?;                                  // 开始监听(在建立连接之前)
//! tab.run_js("new WebSocket('wss://host/path')").await?;
//! let msg = ws.wait(None).await?.unwrap();
//! if msg.is_text() { println!("{}", msg.text().unwrap()); }
//! ws.stop().await?;
//! ```

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::browser::tab::Tab;
use crate::protocol::Event;
use crate::util::base64_decode;
use crate::{Error, Result};

/// 缓冲上限:超过则丢弃最旧的(WebSocket 帧可能高频,避免长会话内存无界增长)。
const MAX_BUFFERED: usize = 2000;

/// 帧方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsDirection {
    /// 页面发出(client → server)。
    Sent,
    /// 页面收到(server → client)。
    Received,
}

impl WsDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            WsDirection::Sent => "sent",
            WsDirection::Received => "received",
        }
    }
}

/// 一个被监听到的 WebSocket 帧。
#[derive(Debug, Clone)]
pub struct WsMessage {
    /// 方向(发出 / 收到)。
    pub direction: WsDirection,
    /// 该连接的 URL(由 `webSocketCreated`/`webSocketOpened` 回填;监听开始前已建立的连接可能为空)。
    pub url: String,
    /// 连接唯一标识(`frameId---wsid`),用于把同一连接的帧归组。
    pub socket_id: String,
    /// 所在 frame 的 id。
    pub frame_id: String,
    /// 连接序号 id(Juggler 的 `wsid`)。
    pub wsid: String,
    /// 操作码:`1`=文本,`2`=二进制,`8`=关闭,`9`=ping,`10`=pong,`0`=续帧。
    pub opcode: i64,
    /// 原始数据:**文本帧(opcode 1)为文本**,**其余帧为 base64**(Juggler 原样)。
    pub data: String,
}

impl WsMessage {
    /// 是否文本帧(opcode 1)。
    pub fn is_text(&self) -> bool {
        self.opcode == 1
    }

    /// 是否二进制帧(opcode 2)。
    pub fn is_binary(&self) -> bool {
        self.opcode == 2
    }

    /// 是否控制帧(close=8 / ping=9 / pong=10)。
    pub fn is_control(&self) -> bool {
        matches!(self.opcode, 8..=10)
    }

    /// opcode 的可读名(`text`/`binary`/`close`/`ping`/`pong`/`continuation`/`opcode(N)`)。
    pub fn opcode_name(&self) -> String {
        match self.opcode {
            0 => "continuation".into(),
            1 => "text".into(),
            2 => "binary".into(),
            8 => "close".into(),
            9 => "ping".into(),
            10 => "pong".into(),
            n => format!("opcode({n})"),
        }
    }

    /// 文本内容:文本帧返回原文;非文本帧返回 `None`(用 [`bytes`](Self::bytes) / [`text_lossy`](Self::text_lossy))。
    pub fn text(&self) -> Option<String> {
        self.is_text().then(|| self.data.clone())
    }

    /// 解码后的字节:文本帧为其 UTF-8 字节;其余帧 base64 解码(失败为空)。
    pub fn bytes(&self) -> Vec<u8> {
        if self.is_text() {
            self.data.clone().into_bytes()
        } else {
            base64_decode(&self.data).unwrap_or_default()
        }
    }

    /// 尽力得到文本:文本帧原文;二进制/控制帧按 UTF-8 有损解码其字节。
    pub fn text_lossy(&self) -> String {
        if self.is_text() {
            self.data.clone()
        } else {
            String::from_utf8_lossy(&self.bytes()).into_owned()
        }
    }

    /// 把内容按 JSON 解析(文本帧解析其文本;二进制帧解析其字节);非 JSON 返回 `None`。
    pub fn json(&self) -> Option<Value> {
        if self.is_text() {
            serde_json::from_str(&self.data).ok()
        } else {
            serde_json::from_slice(&self.bytes()).ok()
        }
    }
}

/// 一个 WebSocket 连接的状态快照(由监听任务跟踪)。
#[derive(Debug, Clone, Default)]
pub struct WsSocket {
    /// 连接唯一标识(`frameId---wsid`)。
    pub socket_id: String,
    /// 连接 URL。
    pub url: String,
    /// 是否已完成握手(收到 `webSocketOpened`)。
    pub opened: bool,
    /// 是否已关闭(收到 `webSocketClosed`)。
    pub closed: bool,
    /// 关闭时的错误信息(正常关闭为空)。
    pub error: String,
}

/// WebSocket 帧监听过滤条件(默认:双向、按 URL 不过滤、**不含控制帧**)。
#[derive(Debug, Clone, Default)]
pub struct WsFilter {
    /// 连接 URL 子串集合;为空表示匹配所有连接。
    pub url_keywords: Vec<String>,
    /// 仅保留某方向;`None` 表示双向都收。
    pub direction: Option<WsDirection>,
    /// 是否包含 ping/pong/close 控制帧(默认 `false`,只收 text/binary 数据帧)。
    pub include_control: bool,
}

impl WsFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一个连接 URL 必含子串(任一命中即保留)。
    pub fn url_contains(mut self, needle: &str) -> Self {
        self.url_keywords.push(needle.to_string());
        self
    }

    /// 只收页面**发出**的帧。
    pub fn sent_only(mut self) -> Self {
        self.direction = Some(WsDirection::Sent);
        self
    }

    /// 只收页面**收到**的帧。
    pub fn received_only(mut self) -> Self {
        self.direction = Some(WsDirection::Received);
        self
    }

    /// 同时包含控制帧(ping/pong/close)。
    pub fn with_control(mut self) -> Self {
        self.include_control = true;
        self
    }

    fn url_matches(&self, url: &str) -> bool {
        self.url_keywords.is_empty() || self.url_keywords.iter().any(|k| url.contains(k))
    }

    fn matches(&self, direction: WsDirection, opcode: i64, url: &str) -> bool {
        if let Some(d) = self.direction
            && d != direction
        {
            return false;
        }
        if !self.include_control && matches!(opcode, 8..=10) {
            return false;
        }
        self.url_matches(url)
    }
}

/// WebSocket 监听共享状态(放在 `TabCore`,由监听任务写、句柄读)。
pub(crate) struct WsShared {
    pub buf: Mutex<VecDeque<WsMessage>>,
    pub sockets: Mutex<HashMap<String, WsSocket>>,
    pub active: AtomicBool,
}

impl WsShared {
    pub(crate) fn new() -> Self {
        Self {
            buf: Mutex::new(VecDeque::new()),
            sockets: Mutex::new(HashMap::new()),
            active: AtomicBool::new(false),
        }
    }
}

/// `tab.websocket()` 返回的 WebSocket 帧监听句柄。
///
/// 即用即弃,持有一个 [`Tab`] 克隆(共享内核)。`start` 与 `wait` 即使来自不同 `websocket()` 句柄,
/// 也共享同一缓冲。
pub struct WsListener {
    tab: Tab,
}

impl WsListener {
    pub(crate) fn new(tab: Tab) -> Self {
        Self { tab }
    }

    /// 开始监听 WebSocket 帧。幂等:已在监听时直接返回。务必在建立连接(导航 / `new WebSocket`)**之前**调用。
    pub async fn start(&self) -> Result<()> {
        self.start_with(WsFilter::default()).await
    }

    /// 开始监听并指定过滤条件(只收某 URL / 某方向 / 是否含控制帧)。
    pub async fn start_with(&self, filter: WsFilter) -> Result<()> {
        let shared = self.tab.core.ws.clone();
        if shared.active.swap(true, Ordering::SeqCst) {
            return Ok(()); // 已在监听:幂等返回
        }
        shared.buf.lock().await.clear();
        shared.sockets.lock().await.clear();

        let events = self.tab.core.conn.subscribe();
        let session = self.tab.core.session_id.clone();
        let task = tokio::spawn(ws_loop(events, session, shared, filter));
        *self.tab.core.ws_task.lock().await = Some(task);
        Ok(())
    }

    /// 是否正在监听。同步读取。
    pub fn listening(&self) -> bool {
        self.tab.core.ws.active.load(Ordering::SeqCst)
    }

    /// 等待一帧。`timeout` 为 `None` 表示无限等待(直到来帧或 `stop`);否则超时返回 `Ok(None)`。
    pub async fn wait(&self, timeout: Option<Duration>) -> Result<Option<WsMessage>> {
        let shared = &self.tab.core.ws;
        if !shared.active.load(Ordering::SeqCst) {
            return Err(Error::Other("尚未调用 websocket().start()".into()));
        }
        let deadline = timeout.map(|d| Instant::now() + d);
        loop {
            if let Some(m) = shared.buf.lock().await.pop_front() {
                return Ok(Some(m));
            }
            if !shared.active.load(Ordering::SeqCst) {
                return Ok(None); // 监听已停止
            }
            if let Some(dl) = deadline
                && Instant::now() >= dl
            {
                return Ok(None);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// 等待 `count` 帧(总超时 `None`=默认操作超时)。到点不足返回已抓到的(不报错)。
    pub async fn wait_count(
        &self,
        count: usize,
        timeout: Option<Duration>,
    ) -> Result<Vec<WsMessage>> {
        let total = timeout.unwrap_or_else(|| self.tab.core.timeout());
        let deadline = Instant::now() + total;
        let mut out = Vec::with_capacity(count);
        while out.len() < count {
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                break;
            }
            match self.wait(Some(remain)).await? {
                Some(m) => out.push(m),
                None => break,
            }
        }
        Ok(out)
    }

    /// 取走当前已缓冲的所有帧并清空。
    pub async fn messages(&self) -> Vec<WsMessage> {
        self.tab.core.ws.buf.lock().await.drain(..).collect()
    }

    /// 当前已知连接的状态快照(URL / 是否打开 / 是否关闭)。
    pub async fn sockets(&self) -> Vec<WsSocket> {
        self.tab
            .core
            .ws
            .sockets
            .lock()
            .await
            .values()
            .cloned()
            .collect()
    }

    /// 清空已获取但未返回的帧。
    pub async fn clear(&self) {
        self.tab.core.ws.buf.lock().await.clear();
    }

    /// 返回流式句柄,可循环逐帧获取。
    pub fn steps(&self) -> WsSteps {
        WsSteps {
            tab: self.tab.clone(),
        }
    }

    /// 停止监听并清空帧缓冲与连接表。
    pub async fn stop(&self) -> Result<()> {
        self.tab.core.ws.active.store(false, Ordering::SeqCst);
        if let Some(h) = self.tab.core.ws_task.lock().await.take() {
            h.abort();
        }
        self.tab.core.ws.buf.lock().await.clear();
        self.tab.core.ws.sockets.lock().await.clear();
        Ok(())
    }
}

/// `websocket().steps()` 返回的流式句柄:每次 [`next`](Self::next) 取下一帧。
pub struct WsSteps {
    tab: Tab,
}

impl WsSteps {
    /// 取下一帧(`timeout` 为 `None` 无限等待;超时返回 `None` 即可结束循环)。
    pub async fn next(&self, timeout: Option<Duration>) -> Result<Option<WsMessage>> {
        WsListener::new(self.tab.clone()).wait(timeout).await
    }
}

/// 监听任务主循环:消费本会话的 `Page.webSocket*` 事件,维护连接表并把帧入缓冲。
async fn ws_loop(
    mut events: tokio::sync::broadcast::Receiver<Event>,
    session: String,
    shared: Arc<WsShared>,
    filter: WsFilter,
) {
    loop {
        let ev = match events.recv().await {
            Ok(ev) => ev,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "WebSocket 监听落后,跳过部分事件");
                continue;
            }
            Err(_) => break,
        };
        if !shared.active.load(Ordering::SeqCst) {
            break;
        }
        if ev.session_id.as_deref() != Some(&session) {
            continue;
        }
        match ev.method.as_str() {
            "Page.webSocketCreated" => {
                let id = socket_id(&ev.params);
                let url = ev.params["requestURL"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let mut socks = shared.sockets.lock().await;
                let s = socks.entry(id.clone()).or_default();
                s.socket_id = id;
                if !url.is_empty() {
                    s.url = url;
                }
            }
            "Page.webSocketOpened" => {
                let id = socket_id(&ev.params);
                let url = ev.params["effectiveURL"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let mut socks = shared.sockets.lock().await;
                let s = socks.entry(id.clone()).or_default();
                s.socket_id = id;
                s.opened = true;
                if !url.is_empty() {
                    s.url = url;
                }
            }
            "Page.webSocketClosed" => {
                let id = socket_id(&ev.params);
                let err = ev.params["error"].as_str().unwrap_or_default().to_string();
                let mut socks = shared.sockets.lock().await;
                let s = socks.entry(id.clone()).or_default();
                s.socket_id = id;
                s.closed = true;
                s.error = err;
            }
            "Page.webSocketFrameSent" => {
                push_frame(&shared, &filter, WsDirection::Sent, &ev.params).await;
            }
            "Page.webSocketFrameReceived" => {
                push_frame(&shared, &filter, WsDirection::Received, &ev.params).await;
            }
            _ => {}
        }
    }
    tracing::debug!(%session, "WebSocket 监听任务结束");
}

/// 构造一帧并(经过滤后)入缓冲。
async fn push_frame(
    shared: &Arc<WsShared>,
    filter: &WsFilter,
    direction: WsDirection,
    params: &Value,
) {
    let id = socket_id(params);
    let opcode = params["opcode"].as_i64().unwrap_or(-1);
    let url = shared
        .sockets
        .lock()
        .await
        .get(&id)
        .map(|s| s.url.clone())
        .unwrap_or_default();
    if !filter.matches(direction, opcode, &url) {
        return;
    }
    let msg = WsMessage {
        direction,
        url,
        socket_id: id,
        frame_id: params["frameId"].as_str().unwrap_or_default().to_string(),
        wsid: params["wsid"].as_str().unwrap_or_default().to_string(),
        opcode,
        data: params["data"].as_str().unwrap_or_default().to_string(),
    };
    let mut buf = shared.buf.lock().await;
    if buf.len() >= MAX_BUFFERED {
        buf.pop_front();
    }
    buf.push_back(msg);
}

/// 连接唯一标识:`frameId---wsid`(与 Playwright 的 `webSocketId` 一致)。
fn socket_id(params: &Value) -> String {
    let frame = params["frameId"].as_str().unwrap_or_default();
    let wsid = params["wsid"].as_str().unwrap_or_default();
    format!("{frame}---{wsid}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_frame_helpers() {
        let m = WsMessage {
            direction: WsDirection::Received,
            url: "wss://x/path".into(),
            socket_id: "f---1".into(),
            frame_id: "f".into(),
            wsid: "1".into(),
            opcode: 1,
            data: r#"{"a":1,"b":[2,3]}"#.into(),
        };
        assert!(m.is_text());
        assert!(!m.is_binary());
        assert!(!m.is_control());
        assert_eq!(m.opcode_name(), "text");
        assert_eq!(m.text().as_deref(), Some(r#"{"a":1,"b":[2,3]}"#));
        assert_eq!(m.bytes(), br#"{"a":1,"b":[2,3]}"#.to_vec());
        let j = m.json().unwrap();
        assert_eq!(j["b"][1], 3);
    }

    #[test]
    fn binary_frame_is_base64() {
        // [1,2,3,4] 的 base64 是 "AQIDBA=="。
        let m = WsMessage {
            direction: WsDirection::Sent,
            url: String::new(),
            socket_id: "f---2".into(),
            frame_id: "f".into(),
            wsid: "2".into(),
            opcode: 2,
            data: "AQIDBA==".into(),
        };
        assert!(m.is_binary());
        assert!(m.text().is_none());
        assert_eq!(m.bytes(), vec![1, 2, 3, 4]);
        assert_eq!(m.opcode_name(), "binary");
    }

    #[test]
    fn binary_json_decodes_then_parses() {
        // base64 of {"k":42}
        let m = WsMessage {
            direction: WsDirection::Received,
            url: String::new(),
            socket_id: "f---3".into(),
            frame_id: "f".into(),
            wsid: "3".into(),
            opcode: 2,
            data: crate::util::base64_encode(br#"{"k":42}"#),
        };
        assert_eq!(m.json().unwrap()["k"], 42);
        assert_eq!(m.text_lossy(), r#"{"k":42}"#);
    }

    #[test]
    fn filter_direction_and_control_and_url() {
        // 默认:双向、不含控制帧、不过滤 URL。
        let f = WsFilter::default();
        assert!(f.matches(WsDirection::Sent, 1, "wss://a"));
        assert!(f.matches(WsDirection::Received, 2, "wss://a"));
        assert!(!f.matches(WsDirection::Sent, 9, "wss://a")); // ping 控制帧默认丢弃

        // 含控制帧。
        let f = WsFilter::new().with_control();
        assert!(f.matches(WsDirection::Sent, 9, "wss://a"));

        // 只收发出方向。
        let f = WsFilter::new().sent_only();
        assert!(f.matches(WsDirection::Sent, 1, "x"));
        assert!(!f.matches(WsDirection::Received, 1, "x"));

        // URL 过滤。
        let f = WsFilter::new().url_contains("/live/");
        assert!(f.matches(WsDirection::Sent, 1, "wss://h/live/room"));
        assert!(!f.matches(WsDirection::Sent, 1, "wss://h/other"));
    }

    #[test]
    fn socket_id_combines_frame_and_wsid() {
        assert_eq!(socket_id(&json!({"frameId":"abc","wsid":"7"})), "abc---7");
    }
}
