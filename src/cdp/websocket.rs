//! CDP 后端的 **WebSocket 帧监听** [`ChromiumWsListener`](对齐 camoufox `WsListener`)。
//!
//! 基于 CDP `Network.webSocketFrameSent`/`Received` 事件(需 `Network.enable`,与网络监听同域,
//! **不**涉及 `Runtime.enable`,反检测友好)。文本帧(opcode 1)`data` 为原文,其余帧为 base64。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, sleep};

use crate::Result;
use crate::cdp::core::{CdpCore, EventBuf};
use crate::protocol::Connection;

/// 帧方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsDirection {
    Sent,
    Received,
}

/// 一个 WebSocket 帧(对齐 camoufox `WsMessage`)。
#[derive(Debug, Clone)]
pub struct WsMessage {
    pub direction: WsDirection,
    pub url: String,
    pub opcode: u8,
    /// 文本帧为原文;其余帧为 base64。
    pub data: String,
}

impl WsMessage {
    pub fn is_text(&self) -> bool {
        self.opcode == 1
    }
    pub fn is_binary(&self) -> bool {
        self.opcode == 2
    }
    /// 文本(文本帧原样;其余帧尝试 base64 解码为 UTF-8)。
    pub fn text(&self) -> String {
        if self.is_text() {
            self.data.clone()
        } else {
            crate::util::base64_decode(&self.data)
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_default()
        }
    }
    /// 原始字节(文本帧=UTF-8 字节;其余=base64 解码)。
    pub fn bytes(&self) -> Vec<u8> {
        if self.is_text() {
            self.data.clone().into_bytes()
        } else {
            crate::util::base64_decode(&self.data).unwrap_or_default()
        }
    }
    /// 把负载当 JSON 解析。
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.text()).unwrap_or(Value::Null)
    }
}

/// WS 监听过滤(对齐 camoufox `WsFilter`)。
#[derive(Debug, Clone, Default)]
pub struct WsFilter {
    pub url_contains: Option<String>,
    pub direction: Option<WsDirection>,
    /// 是否保留 ping/pong/close 等控制帧(默认 false:只留 text/binary 数据帧)。
    pub with_control: bool,
}

impl WsFilter {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn url_contains(mut self, s: impl Into<String>) -> Self {
        self.url_contains = Some(s.into());
        self
    }
    pub fn direction(mut self, d: WsDirection) -> Self {
        self.direction = Some(d);
        self
    }
    pub fn with_control(mut self) -> Self {
        self.with_control = true;
        self
    }
    fn matches(&self, m: &WsMessage) -> bool {
        if !(self.with_control || m.opcode == 1 || m.opcode == 2) {
            return false;
        }
        if let Some(s) = &self.url_contains {
            if !m.url.contains(s) {
                return false;
            }
        }
        if let Some(d) = self.direction {
            if m.direction != d {
                return false;
            }
        }
        true
    }
}

const BUFFER_CAP: usize = 500;

/// WebSocket 帧监听句柄(`tab.websocket()` 返回)。
pub struct ChromiumWsListener {
    core: Arc<CdpCore>,
}

impl ChromiumWsListener {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self { core }
    }

    /// 开始监听(默认只留 text/binary 数据帧)。
    pub async fn start(&self) -> Result<()> {
        self.start_with(WsFilter::default()).await
    }

    /// 带过滤开始监听。
    pub async fn start_with(&self, filter: WsFilter) -> Result<()> {
        self.stop().await?;
        self.core.send("Network.enable", json!({})).await?;
        let buf = self.core.ws.lock().await.buf.clone();
        let task = tokio::spawn(ws_pump(
            self.core.conn.clone(),
            self.core.session_id.clone(),
            filter,
            buf,
        ));
        let mut g = self.core.ws.lock().await;
        g.running = true;
        g.abort = Some(task.abort_handle());
        Ok(())
    }

    /// 是否正在监听。
    pub async fn listening(&self) -> bool {
        self.core.ws.lock().await.running
    }

    /// 等待一个帧(超时返回 `None`)。
    pub async fn wait(&self, timeout: Option<Duration>) -> Result<Option<WsMessage>> {
        let buf = self.core.ws.lock().await.buf.clone();
        let deadline = Instant::now() + timeout.unwrap_or_else(|| self.core.timeout());
        loop {
            if let Some(m) = buf.lock().await.pop_front() {
                return Ok(Some(m));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            sleep(Duration::from_millis(50)).await;
        }
    }

    /// 在总超时内尽量收集 `n` 个帧。
    pub async fn wait_count(&self, n: usize, timeout: Option<Duration>) -> Result<Vec<WsMessage>> {
        let buf = self.core.ws.lock().await.buf.clone();
        let deadline = Instant::now() + timeout.unwrap_or_else(|| self.core.timeout());
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            if let Some(m) = buf.lock().await.pop_front() {
                out.push(m);
                continue;
            }
            if Instant::now() >= deadline {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
        Ok(out)
    }

    /// 取出当前缓冲全部帧并清空。
    pub async fn messages(&self) -> Vec<WsMessage> {
        let buf = self.core.ws.lock().await.buf.clone();
        let mut g = buf.lock().await;
        g.drain(..).collect()
    }

    /// 停止监听。
    pub async fn stop(&self) -> Result<()> {
        let (abort, buf) = {
            let mut g = self.core.ws.lock().await;
            g.running = false;
            (g.abort.take(), g.buf.clone())
        };
        buf.lock().await.clear();
        if let Some(a) = abort {
            a.abort();
        }
        Ok(())
    }
}

async fn ws_pump(
    conn: Connection,
    session_id: String,
    filter: WsFilter,
    buf: Arc<Mutex<VecDeque<WsMessage>>>,
) {
    let mut events = conn.subscribe();
    let mut urls: HashMap<String, String> = HashMap::new();
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
            "Network.webSocketCreated" => {
                if let (Some(id), Some(url)) =
                    (ev.params["requestId"].as_str(), ev.params["url"].as_str())
                {
                    urls.insert(id.to_string(), url.to_string());
                }
            }
            "Network.webSocketFrameSent" | "Network.webSocketFrameReceived" => {
                let dir = if ev.method.ends_with("Sent") {
                    WsDirection::Sent
                } else {
                    WsDirection::Received
                };
                let id = ev.params["requestId"].as_str().unwrap_or_default();
                let url = urls.get(id).cloned().unwrap_or_default();
                let resp = &ev.params["response"];
                let m = WsMessage {
                    direction: dir,
                    url,
                    opcode: resp["opcode"].as_u64().unwrap_or(0) as u8,
                    data: resp["payloadData"].as_str().unwrap_or_default().to_string(),
                };
                if filter.matches(&m) {
                    let mut g = buf.lock().await;
                    if g.len() >= BUFFER_CAP {
                        g.pop_front();
                    }
                    g.push_back(m);
                }
            }
            _ => {}
        }
    }
}

/// WS 监听共享状态(放 [`CdpCore`])。
pub(crate) type WsShared = EventBuf<WsMessage>;
