//! 控制台监听(对应 DrissionPage 的 `tab.console`)。
//!
//! 实现方式:订阅 Camoufox/Juggler 的**原生** `Runtime.console` 事件(不像网络监听那样 hook 页面
//! `console`,因此不污染页面对象、对反检测更友好)。`console.start()` 起一个后台任务持续把本会话的
//! 控制台消息搬进缓冲,`wait/messages/steps` 取回。
//!
//! 文本还原:事件里每个参数是一个 RemoteObject。**原始类型**(字符串/数字/布尔/NaN…)直接带
//! `value`,在 Rust 侧零开销拼成 `text`;**对象/数组/Error/DOM 节点**只给 `objectId`,此时回调一次
//! `Runtime.callFunction` 在页面里把全部参数序列化(字符串原样、其余 `JSON.stringify`/`String`)再拼接,
//! 从而拿到与浏览器一致的可读文本。
//!
//! ```ignore
//! let console = tab.console();
//! console.start().await?;                       // 开始监听(在触发日志之前)
//! tab.run_js("console.log('DrissionPage')").await?;
//! let data = console.wait(None).await?.unwrap();
//! assert_eq!(data.text, "DrissionPage");
//! console.stop().await?;
//! ```

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::browser::tab::{Tab, extract_runtime_result};
use crate::protocol::{Connection, Event};
use crate::{Error, Result};

/// 缓冲上限:超过则丢弃最旧的(避免长会话内存无界增长)。
const MAX_BUFFERED: usize = 1000;

/// 页面侧把任意参数序列化成可读文本的函数(字符串原样;对象走 JSON;Error/节点特殊处理)。
const CONSOLE_JOIN_FN: &str = r#"function(){
  function s(x){
    if (typeof x === 'string') return x;
    if (x === null) return 'null';
    var t = typeof x;
    if (t === 'undefined') return 'undefined';
    if (t === 'number' || t === 'boolean' || t === 'bigint') return String(x);
    if (t === 'symbol') { try { return x.toString(); } catch(e){ return 'Symbol()'; } }
    if (t === 'function') { return 'function ' + (x.name || '') + '()'; }
    try { if (x instanceof Error) return (x.stack || (x.name + ': ' + x.message)); } catch(e){}
    try { if (typeof Node !== 'undefined' && x instanceof Node) return (x.outerHTML || x.nodeName || '[node]'); } catch(e){}
    try { return JSON.stringify(x); } catch(e){}
    try { return String(x); } catch(e){ return '[object]'; }
  }
  return Array.prototype.slice.call(arguments).map(s).join(' ');
}"#;

/// 一条控制台消息(对应 DrissionPage 的 `ConsoleData`)。
#[derive(Debug, Clone)]
pub struct ConsoleData {
    /// 来源(Juggler 下统一为 `console-api`,best-effort)。
    pub source: String,
    /// 级别 / 类型:`log` / `info` / `warning` / `error` / `debug` / `dir` / `trace` …(取自事件 `type`)。
    pub level: String,
    /// 内容文本(各参数序列化后以空格拼接)。
    pub text: String,
    /// 各参数的原始值(原始类型为其值,对象类参数为 `null`;需要结构化时用)。
    pub args: Vec<Value>,
    /// 产生该日志的脚本 URL。
    pub url: String,
    /// 行号(0 基)。
    pub line: i64,
    /// 列号(0 基)。
    pub column: i64,
}

impl ConsoleData {
    /// 把 [`text`](Self::text) 当作 JSON 解析;非 JSON 返回 `None`(对应 DP `ConsoleData.body`)。
    pub fn body(&self) -> Option<Value> {
        serde_json::from_str(&self.text).ok()
    }
}

/// 控制台监听的过滤条件(DP `console.start()` 无过滤;这里是 drission 的增强,默认全收)。
#[derive(Debug, Clone, Default)]
pub struct ConsoleFilter {
    /// 仅保留这些级别(大小写不敏感);为空表示所有级别。
    pub levels: Vec<String>,
    /// 仅保留 `text` 含其一的消息(子串);为空表示不按文本过滤。
    pub contains: Vec<String>,
}

impl ConsoleFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一个要保留的级别(如 `log` / `error`)。
    pub fn level(mut self, level: &str) -> Self {
        self.levels.push(level.to_string());
        self
    }

    /// 追加一个 `text` 必含子串(任一命中即保留)。
    pub fn contains(mut self, needle: &str) -> Self {
        self.contains.push(needle.to_string());
        self
    }

    fn matches(&self, d: &ConsoleData) -> bool {
        let level_ok =
            self.levels.is_empty() || self.levels.iter().any(|l| l.eq_ignore_ascii_case(&d.level));
        let text_ok = self.contains.is_empty() || self.contains.iter().any(|s| d.text.contains(s));
        level_ok && text_ok
    }
}

/// 控制台监听共享状态(放在 `TabCore`,由监听任务写、句柄读)。
pub(crate) struct ConsoleShared {
    pub buf: Mutex<VecDeque<ConsoleData>>,
    pub active: AtomicBool,
}

impl ConsoleShared {
    pub(crate) fn new() -> Self {
        Self {
            buf: Mutex::new(VecDeque::new()),
            active: AtomicBool::new(false),
        }
    }
}

/// `tab.console()` 返回的控制台监听句柄(对应 DP `tab.console`)。
///
/// 即用即弃,持有一个 [`Tab`] 克隆(共享内核)。`start` 与 `wait` 即使来自不同 `console()` 句柄,
/// 也共享同一缓冲。
pub struct Console {
    tab: Tab,
}

impl Console {
    pub(crate) fn new(tab: Tab) -> Self {
        Self { tab }
    }

    /// 开始监听控制台(对应 DP `console.start()`)。幂等:已在监听时直接返回。
    pub async fn start(&self) -> Result<()> {
        self.start_with(ConsoleFilter::default()).await
    }

    /// 开始监听并指定过滤条件(drission 增强:只收指定级别 / 含指定子串的消息)。
    pub async fn start_with(&self, filter: ConsoleFilter) -> Result<()> {
        let shared = self.tab.core.console.clone();
        // 已在监听:幂等返回(不重复 spawn)。
        if shared.active.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        shared.buf.lock().await.clear();

        let events = self.tab.core.conn.subscribe();
        let conn = self.tab.core.conn.clone();
        let session = self.tab.core.session_id.clone();
        let task = tokio::spawn(console_loop(events, conn, session, shared, filter));
        *self.tab.core.console_task.lock().await = Some(task);
        Ok(())
    }

    /// 是否正在监听(对应 DP `console.listening`)。同步读取。
    pub fn listening(&self) -> bool {
        self.tab.core.console.active.load(Ordering::SeqCst)
    }

    /// 等待一条控制台消息(对应 DP `console.wait()`)。
    ///
    /// `timeout` 为 `None` 表示无限等待(直到来消息或 `stop`);否则超时返回 `Ok(None)`。
    pub async fn wait(&self, timeout: Option<Duration>) -> Result<Option<ConsoleData>> {
        let shared = &self.tab.core.console;
        if !shared.active.load(Ordering::SeqCst) {
            return Err(Error::Other("尚未调用 console.start()".into()));
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

    /// 取走当前已缓冲的所有消息并清空(对应 DP `console.messages`)。
    pub async fn messages(&self) -> Vec<ConsoleData> {
        self.tab.core.console.buf.lock().await.drain(..).collect()
    }

    /// 清空已获取但未返回的消息(对应 DP `console.clear()`)。
    pub async fn clear(&self) {
        self.tab.core.console.buf.lock().await.clear();
    }

    /// 返回一个流式句柄,可循环逐条获取(对应 DP `console.steps()`)。
    pub fn steps(&self) -> ConsoleSteps {
        ConsoleSteps {
            tab: self.tab.clone(),
        }
    }

    /// 停止监听并清空消息列表(对应 DP `console.stop()`)。
    pub async fn stop(&self) -> Result<()> {
        self.tab.core.console.active.store(false, Ordering::SeqCst);
        if let Some(h) = self.tab.core.console_task.lock().await.take() {
            h.abort();
        }
        self.tab.core.console.buf.lock().await.clear();
        Ok(())
    }
}

/// `console.steps()` 返回的流式句柄:每次 [`next`](Self::next) 取下一条消息。
pub struct ConsoleSteps {
    tab: Tab,
}

impl ConsoleSteps {
    /// 取下一条消息(`timeout` 为 `None` 无限等待;超时返回 `None` 即可结束循环)。
    pub async fn next(&self, timeout: Option<Duration>) -> Result<Option<ConsoleData>> {
        Console::new(self.tab.clone()).wait(timeout).await
    }
}

/// 监听任务主循环:消费本会话的 `Runtime.console` 事件,构造 [`ConsoleData`] 入缓冲。
async fn console_loop(
    mut events: tokio::sync::broadcast::Receiver<Event>,
    conn: Connection,
    session: String,
    shared: Arc<ConsoleShared>,
    filter: ConsoleFilter,
) {
    loop {
        let ev = match events.recv().await {
            Ok(ev) => ev,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "控制台监听落后,跳过部分事件");
                continue;
            }
            Err(_) => break,
        };
        if !shared.active.load(Ordering::SeqCst) {
            break;
        }
        if ev.session_id.as_deref() != Some(&session) || ev.method != "Runtime.console" {
            continue;
        }
        let data = build_console_data(&conn, &session, &ev.params).await;
        if !filter.matches(&data) {
            continue;
        }
        let mut buf = shared.buf.lock().await;
        if buf.len() >= MAX_BUFFERED {
            buf.pop_front();
        }
        buf.push_back(data);
    }
    tracing::debug!(%session, "控制台监听任务结束");
}

/// 从一个 `Runtime.console` 事件的 `params` 构造 [`ConsoleData`]。
async fn build_console_data(conn: &Connection, session: &str, params: &Value) -> ConsoleData {
    let level = params
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("log")
        .to_string();
    let loc = params.get("location");
    let url = loc
        .and_then(|l| l.get("url"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let line = loc
        .and_then(|l| l.get("lineNumber"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let column = loc
        .and_then(|l| l.get("columnNumber"))
        .and_then(Value::as_i64)
        .unwrap_or(0);

    let args: Vec<Value> = params
        .get("args")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let arg_values: Vec<Value> = args
        .iter()
        .map(|a| a.get("value").cloned().unwrap_or(Value::Null))
        .collect();

    // 任一参数是对象(只有 objectId)→ 回页面序列化;否则纯 Rust 拼接(零往返)。
    let need_resolve = args.iter().any(|a| a.get("objectId").is_some());
    let text = if need_resolve {
        resolve_text(conn, session, params)
            .await
            .unwrap_or_else(|| fallback_text(&args))
    } else {
        args.iter()
            .map(primitive_text)
            .collect::<Vec<_>>()
            .join(" ")
    };

    ConsoleData {
        source: "console-api".to_string(),
        level,
        text,
        args: arg_values,
        url,
        line,
        column,
    }
}

/// 把一个**非对象**(无 objectId)参数转成文本:字符串原样,`null`/`undefined` 显式,其余用紧凑 JSON。
fn primitive_text(arg: &Value) -> String {
    if let Some(u) = arg.get("unserializableValue").and_then(Value::as_str) {
        return u.to_string();
    }
    match arg.get("value") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) => "null".to_string(),
        Some(v) => v.to_string(),
        None => "undefined".to_string(),
    }
}

/// 回页面序列化失败时的兜底:对象参数用 `[subtype]`/`[type]` 占位,其余按原始类型拼。
fn fallback_text(args: &[Value]) -> String {
    args.iter()
        .map(|a| {
            if a.get("objectId").is_some() {
                let label = a
                    .get("subtype")
                    .and_then(Value::as_str)
                    .or_else(|| a.get("type").and_then(Value::as_str))
                    .unwrap_or("object");
                format!("[{label}]")
            } else {
                primitive_text(a)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 回调 `Runtime.callFunction`,在页面里把全部参数序列化并拼接成文本。
async fn resolve_text(conn: &Connection, session: &str, params: &Value) -> Option<String> {
    let ctx = params.get("executionContextId").and_then(Value::as_str)?;
    let args: Vec<Value> = params
        .get("args")
        .and_then(Value::as_array)?
        .iter()
        .map(clean_arg)
        .collect();
    let r = conn
        .send(
            "Runtime.callFunction",
            json!({
                "executionContextId": ctx,
                "functionDeclaration": CONSOLE_JOIN_FN,
                "returnByValue": true,
                "args": args,
            }),
            Some(session),
        )
        .await
        .ok()?;
    extract_runtime_result(r).ok()?.as_str().map(str::to_string)
}

/// 把事件里的 RemoteObject 削成 `CallFunctionArgument`(只保留 objectId / unserializableValue / value)。
fn clean_arg(arg: &Value) -> Value {
    if let Some(o) = arg.get("objectId") {
        json!({ "objectId": o })
    } else if let Some(u) = arg.get("unserializableValue") {
        json!({ "unserializableValue": u })
    } else if let Some(v) = arg.get("value") {
        json!({ "value": v })
    } else {
        json!({}) // undefined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_text_variants() {
        assert_eq!(
            primitive_text(&json!({ "value": "hello", "type": "string" })),
            "hello"
        );
        assert_eq!(
            primitive_text(&json!({ "value": 42, "type": "number" })),
            "42"
        );
        assert_eq!(primitive_text(&json!({ "value": true })), "true");
        assert_eq!(
            primitive_text(&json!({ "value": null, "subtype": "null" })),
            "null"
        );
        assert_eq!(
            primitive_text(&json!({ "unserializableValue": "NaN" })),
            "NaN"
        );
        assert_eq!(primitive_text(&json!({})), "undefined");
    }

    #[test]
    fn fallback_uses_subtype_label() {
        let args = vec![
            json!({ "value": "x", "type": "string" }),
            json!({ "objectId": "id-1", "type": "object", "subtype": "array" }),
            json!({ "objectId": "id-2", "type": "object" }),
        ];
        assert_eq!(fallback_text(&args), "x [array] [object]");
    }

    #[test]
    fn clean_arg_keeps_only_call_fields() {
        assert_eq!(
            clean_arg(&json!({ "objectId": "id-9", "type": "object", "subtype": "array" })),
            json!({ "objectId": "id-9" })
        );
        assert_eq!(
            clean_arg(&json!({ "value": "s", "type": "string" })),
            json!({ "value": "s" })
        );
        assert_eq!(
            clean_arg(&json!({ "unserializableValue": "Infinity" })),
            json!({ "unserializableValue": "Infinity" })
        );
        assert_eq!(clean_arg(&json!({ "type": "undefined" })), json!({}));
    }

    #[test]
    fn filter_matches_level_and_text() {
        let d = ConsoleData {
            source: "console-api".into(),
            level: "error".into(),
            text: "boom at line".into(),
            args: vec![],
            url: String::new(),
            line: 0,
            column: 0,
        };
        assert!(ConsoleFilter::default().matches(&d));
        assert!(ConsoleFilter::new().level("ERROR").matches(&d));
        assert!(!ConsoleFilter::new().level("log").matches(&d));
        assert!(ConsoleFilter::new().contains("boom").matches(&d));
        assert!(!ConsoleFilter::new().contains("nope").matches(&d));
        assert!(
            ConsoleFilter::new()
                .level("error")
                .contains("boom")
                .matches(&d)
        );
    }

    #[test]
    fn console_data_body_parses_json() {
        let d = ConsoleData {
            source: "console-api".into(),
            level: "log".into(),
            text: r#"{"a":1,"b":[2,3]}"#.into(),
            args: vec![],
            url: String::new(),
            line: 0,
            column: 0,
        };
        let body = d.body().unwrap();
        assert_eq!(body["a"], 1);
        assert_eq!(body["b"][1], 3);
        let plain = ConsoleData {
            text: "not json".into(),
            ..d
        };
        assert!(plain.body().is_none());
    }
}
