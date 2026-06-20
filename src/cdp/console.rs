//! CDP 后端的**控制台监听** [`ChromiumConsole`](对齐 camoufox `Console`)。
//!
//! 基于 CDP `Runtime.consoleAPICalled` 事件。**注意取舍**:该事件需要 `Runtime.enable`,而本库为
//! 反检测默认**不开** `Runtime.enable`(经典 CF 探测点)。因此 `console().start()` 会**按需开启**
//! `Runtime.enable`(可能被强反爬站点探测到);只在确实需要读控制台时用,过盾场景别开。

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, sleep};

use crate::Result;
use crate::cdp::core::{CdpCore, EventBuf};
use crate::protocol::Connection;

/// 一条控制台消息(对齐 camoufox `ConsoleData`)。
#[derive(Debug, Clone, Default)]
pub struct ConsoleData {
    /// 级别(log/info/warning/error/debug…)。
    pub level: String,
    /// 拼接后的文本。
    pub text: String,
    /// 原始参数(RemoteObject 的 value/description)。
    pub args: Vec<Value>,
    /// 来源 URL。
    pub url: String,
    /// 行号(0 基)。
    pub line: u32,
}

impl ConsoleData {
    /// 把 `text` 当 JSON 解析(失败返回 `Null`)。
    pub fn body(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }
}

/// 控制台监听过滤(对齐 camoufox `ConsoleFilter`)。
#[derive(Debug, Clone, Default)]
pub struct ConsoleFilter {
    /// 只收该级别(`None` 全收)。
    pub level: Option<String>,
    /// text 必须包含此子串(`None` 不过滤)。
    pub text_contains: Option<String>,
}

impl ConsoleFilter {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn level(mut self, level: impl Into<String>) -> Self {
        self.level = Some(level.into());
        self
    }
    pub fn text(mut self, sub: impl Into<String>) -> Self {
        self.text_contains = Some(sub.into());
        self
    }
    fn matches(&self, d: &ConsoleData) -> bool {
        if let Some(l) = &self.level {
            if &d.level != l {
                return false;
            }
        }
        if let Some(s) = &self.text_contains {
            if !d.text.contains(s) {
                return false;
            }
        }
        true
    }
}

const BUFFER_CAP: usize = 500;

/// 控制台监听句柄(`tab.console()` 返回)。
pub struct ChromiumConsole {
    core: Arc<CdpCore>,
}

impl ChromiumConsole {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self { core }
    }

    /// 开始监听(全收)。**会开启 `Runtime.enable`**(见模块文档的反检测取舍)。
    pub async fn start(&self) -> Result<()> {
        self.start_with(ConsoleFilter::default()).await
    }

    /// 带过滤开始监听。
    pub async fn start_with(&self, filter: ConsoleFilter) -> Result<()> {
        self.stop().await?;
        self.core.send("Runtime.enable", json!({})).await?;
        let buf = self.core.console.lock().await.buf.clone();
        let task = tokio::spawn(console_pump(
            self.core.conn.clone(),
            self.core.session_id.clone(),
            filter,
            buf,
        ));
        let mut g = self.core.console.lock().await;
        g.running = true;
        g.abort = Some(task.abort_handle());
        Ok(())
    }

    /// 是否正在监听。
    pub async fn listening(&self) -> bool {
        self.core.console.lock().await.running
    }

    /// 等待一条消息(`timeout=None` 用标签默认超时);超时返回 `None`。
    pub async fn wait(&self, timeout: Option<Duration>) -> Result<Option<ConsoleData>> {
        let buf = self.core.console.lock().await.buf.clone();
        let deadline = Instant::now() + timeout.unwrap_or_else(|| self.core.timeout());
        loop {
            if let Some(d) = buf.lock().await.pop_front() {
                return Ok(Some(d));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            sleep(Duration::from_millis(50)).await;
        }
    }

    /// 取出当前已缓冲的全部消息并清空。
    pub async fn messages(&self) -> Vec<ConsoleData> {
        let buf = self.core.console.lock().await.buf.clone();
        let mut g = buf.lock().await;
        g.drain(..).collect()
    }

    /// 清空缓冲。
    pub async fn clear(&self) {
        let buf = self.core.console.lock().await.buf.clone();
        buf.lock().await.clear();
    }

    /// 停止监听(中止后台任务、清缓冲)。
    pub async fn stop(&self) -> Result<()> {
        let (abort, buf) = {
            let mut g = self.core.console.lock().await;
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

/// 后台任务:订阅连接事件,把本会话的 `Runtime.consoleAPICalled` 聚合成 [`ConsoleData`]。
async fn console_pump(
    conn: Connection,
    session_id: String,
    filter: ConsoleFilter,
    buf: Arc<Mutex<VecDeque<ConsoleData>>>,
) {
    let mut events = conn.subscribe();
    loop {
        let ev = match events.recv().await {
            Ok(ev) => ev,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        };
        if ev.session_id.as_deref() != Some(session_id.as_str())
            || ev.method != "Runtime.consoleAPICalled"
        {
            continue;
        }
        let d = build_console(&ev.params);
        if filter.matches(&d) {
            let mut g = buf.lock().await;
            if g.len() >= BUFFER_CAP {
                g.pop_front();
            }
            g.push_back(d);
        }
    }
}

/// 把 `Runtime.consoleAPICalled` 参数转成 [`ConsoleData`]。
fn build_console(p: &Value) -> ConsoleData {
    let ty = p["type"].as_str().unwrap_or("log");
    let level = match ty {
        "warning" => "warning",
        other => other,
    }
    .to_string();
    let args = p["args"].as_array().cloned().unwrap_or_default();
    // 文本:原始类型取 value、对象取 description(无 preview 序列化的简化版)。
    let parts: Vec<String> = args
        .iter()
        .map(|a| {
            if let Some(v) = a.get("value") {
                match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                }
            } else if let Some(desc) = a["description"].as_str() {
                desc.to_string()
            } else {
                a["type"].as_str().unwrap_or("").to_string()
            }
        })
        .collect();
    let frame = &p["stackTrace"]["callFrames"][0];
    ConsoleData {
        level,
        text: parts.join(" "),
        args,
        url: frame["url"].as_str().unwrap_or_default().to_string(),
        line: frame["lineNumber"].as_u64().unwrap_or(0) as u32,
    }
}

/// 控制台监听共享状态(放 [`CdpCore`])。
pub(crate) type ConsoleShared = EventBuf<ConsoleData>;
