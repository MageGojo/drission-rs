//! CDP 后端的**录制器** [`ChromiumRecorder`](`tab.recorder()`):录一遍页面操作 → 生成可运行 Rust。
//!
//! 机制(同 [`expose_function`](crate::cdp::ChromiumTab::expose_function) / `console()`):
//! - `Runtime.addBinding("__drission_record")` 让页面能回调宿主;
//! - `Page.addScriptToEvaluateOnNewDocument([RECORDER_JS])` 让录制脚本每次导航自动就位;
//! - 后台泵订阅 `Runtime.bindingCalled`(动作)+ `Page.frameNavigated`(主框架导航)→ 收进
//!   [`RecordedScript`](crate::codegen::RecordedScript)。
//!
//! **反检测取舍**:`start()` 会开 `Runtime.enable`(经典 CF 探测点)。录制是**开发期**行为、不在过盾
//! 生产链路用,故可接受。产物经 [`RecordedScript::to_rust`](crate::codegen::RecordedScript::to_rust)
//! 拿可运行 Rust。

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::AbortHandle;

use super::ChromiumTab;
use super::core::CdpCore;
use crate::Result;
use crate::codegen::{RecordedAction, RecordedScript};

/// 录制脚本(注入页面;事件钩子 + DP 风格选择器计算 → 回调 `__drission_record`)。
pub const RECORDER_JS: &str = include_str!("assets/recorder.js");

/// 宿主侧 binding 名(页面 `window.__drission_record(JSON)` 回传动作)。
const BINDING: &str = "__drission_record";

/// 录制器句柄(`tab.recorder()`)。`start()` 起录、`stop()` 收尾拿 [`RecordedScript`];drop 自动停。
pub struct ChromiumRecorder {
    core: Arc<CdpCore>,
    state: Arc<Mutex<RecordedScript>>,
    abort: Arc<StdMutex<Option<AbortHandle>>>,
    /// `addScriptToEvaluateOnNewDocument` 的标识(stop 时移除注入脚本)。
    script_id: Arc<StdMutex<Option<String>>>,
}

impl ChromiumRecorder {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self {
            core,
            state: Arc::new(Mutex::new(RecordedScript::new())),
            abort: Arc::new(StdMutex::new(None)),
            script_id: Arc::new(StdMutex::new(None)),
        }
    }

    /// 开始录制。**在导航前调用**(让注入脚本随后续 `tab.get(..)` 自动就位,并捕获该次导航)。
    pub async fn start(&self) -> Result<()> {
        self.stop().await?; // 幂等:重复 start 先清旧
        // 收页面动作需 Runtime;捕获导航需 Page(attach 时已 enable,这里再保险)。
        let _ = self.core.send("Page.enable", json!({})).await;
        self.core.send("Runtime.enable", json!({})).await?;
        self.core
            .send("Runtime.addBinding", json!({ "name": BINDING }))
            .await?;
        let r = self
            .core
            .send(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": RECORDER_JS }),
            )
            .await?;
        if let Some(id) = r["identifier"].as_str() {
            *self.script_id.lock().unwrap() = Some(id.to_string());
        }
        let _ = self.core.eval_value(RECORDER_JS).await; // 当前文档也即时就位

        // 开启目标发现:捕获本标签打开的弹窗 / 新标签(多标签录制)。
        let _ = self
            .core
            .conn
            .send("Target.setDiscoverTargets", json!({ "discover": true }), None)
            .await;

        let task = tokio::spawn(recorder_pump(self.core.clone(), self.state.clone()));
        *self.abort.lock().unwrap() = Some(task.abort_handle());
        Ok(())
    }

    /// 是否正在录制。
    pub fn is_recording(&self) -> bool {
        self.abort.lock().unwrap().is_some()
    }

    /// 当前已录动作数。
    pub async fn len(&self) -> usize {
        self.state.lock().await.len()
    }

    /// 当前是否还没录到动作。
    pub async fn is_empty(&self) -> bool {
        self.state.lock().await.is_empty()
    }

    /// 拿到当前已录脚本的**快照**(不停止录制)。
    pub async fn script(&self) -> RecordedScript {
        self.state.lock().await.clone()
    }

    /// 便捷:当前脚本生成的可运行 Rust 代码。
    pub async fn to_rust(&self) -> String {
        self.state.lock().await.to_rust()
    }

    /// 停止录制并返回最终 [`RecordedScript`](中止泵 + 移除 binding / 注入脚本)。
    pub async fn stop(&self) -> Result<RecordedScript> {
        if let Some(a) = self.abort.lock().unwrap().take() {
            a.abort();
        }
        // 先取出再 await(不要跨 await 持有同步锁)。
        let script_id = self.script_id.lock().unwrap().take();
        if let Some(id) = script_id {
            let _ = self
                .core
                .send(
                    "Page.removeScriptToEvaluateOnNewDocument",
                    json!({ "identifier": id }),
                )
                .await;
        }
        let _ = self
            .core
            .send("Runtime.removeBinding", json!({ "name": BINDING }))
            .await;
        Ok(self.state.lock().await.clone())
    }
}

impl Drop for ChromiumRecorder {
    fn drop(&mut self) {
        if let Some(a) = self.abort.lock().unwrap().take() {
            a.abort();
        }
    }
}

impl ChromiumTab {
    /// 录制器句柄(对标 Playwright `codegen`):录一遍页面操作 → 生成可运行 Rust 代码。
    ///
    /// ```ignore
    /// let rec = tab.recorder();
    /// rec.start().await?;          // 导航前起录
    /// tab.get("https://example.com").await?;
    /// // ... 人工(或程序)操作页面 ...
    /// let script = rec.stop().await?;
    /// println!("{}", script.to_rust());   // 可运行 Rust(DP 风格选择器)
    /// ```
    pub fn recorder(&self) -> ChromiumRecorder {
        ChromiumRecorder::new(self.core.clone())
    }
}

/// 后台泵:聚合**所有已知会话**(主标签 + 录制期间打开的弹窗 / 新标签)的
/// `Runtime.bindingCalled`(动作)与主框架 `Page.frameNavigated`(导航)进 [`RecordedScript`];
/// 监听 `Target.targetCreated` 自动附着本标签打开的弹窗、注入录制脚本并记一个 `NewTab`。
async fn recorder_pump(core: Arc<CdpCore>, state: Arc<Mutex<RecordedScript>>) {
    let mut events = core.conn.subscribe();
    // 已知会话(收其动作/导航)与已知目标(判定弹窗 openerId、去重)。
    let mut sessions: HashSet<String> = HashSet::new();
    sessions.insert(core.session_id.clone());
    let mut targets: HashSet<String> = HashSet::new();
    targets.insert(core.target_id.clone());

    loop {
        let ev = match events.recv().await {
            Ok(ev) => ev,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        };
        match ev.method.as_str() {
            // 浏览器级事件(无 session):新目标 → 若是本标签开的弹窗则附着 + 注入 + 记 NewTab。
            "Target.targetCreated" => {
                let ti = &ev.params["targetInfo"];
                if ti["type"].as_str() != Some("page") {
                    continue;
                }
                let target_id = ti["targetId"].as_str().unwrap_or_default().to_string();
                if target_id.is_empty() || targets.contains(&target_id) {
                    continue;
                }
                // 只接管"本标签(或已接管的弹窗)打开的"弹窗,避开用户其它无关标签。
                let opener = ti["openerId"].as_str().unwrap_or_default();
                if opener.is_empty() || !targets.contains(opener) {
                    continue;
                }
                if let Some(sid) = attach_and_arm(&core, &target_id).await {
                    targets.insert(target_id);
                    sessions.insert(sid);
                    state.lock().await.push(RecordedAction::NewTab);
                }
            }
            "Runtime.bindingCalled" => {
                if !session_known(&ev.session_id, &sessions)
                    || ev.params["name"].as_str() != Some(BINDING)
                {
                    continue;
                }
                let payload = ev.params["payload"].as_str().unwrap_or_default();
                let parsed: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
                if let Some(action) = RecordedAction::from_event(&parsed) {
                    state.lock().await.push(action);
                }
            }
            "Page.frameNavigated" => {
                if !session_known(&ev.session_id, &sessions) {
                    continue;
                }
                let frame = &ev.params["frame"];
                if frame.get("parentId").and_then(|v| v.as_str()).is_some() {
                    continue; // 仅主框架
                }
                let url = frame["url"].as_str().unwrap_or_default();
                if is_recordable_url(url) {
                    state
                        .lock()
                        .await
                        .push(RecordedAction::Navigate { url: url.to_string() });
                }
            }
            _ => {}
        }
    }
}

/// 事件会话是否属于已知会话集。
fn session_known(sid: &Option<String>, sessions: &HashSet<String>) -> bool {
    sid.as_deref().map(|s| sessions.contains(s)).unwrap_or(false)
}

/// 附着到弹窗目标并武装录制(Page/Runtime enable + addBinding + 注入脚本)。返回其 sessionId。
async fn attach_and_arm(core: &Arc<CdpCore>, target_id: &str) -> Option<String> {
    let a = core
        .conn
        .send(
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
            None,
        )
        .await
        .ok()?;
    let sid = a["sessionId"].as_str()?.to_string();
    let send = |method: &'static str, params: Value| {
        let conn = core.conn.clone();
        let sid = sid.clone();
        async move { conn.send(method, params, Some(&sid)).await }
    };
    let _ = send("Page.enable", json!({})).await;
    let _ = send("Runtime.enable", json!({})).await;
    let _ = send("Runtime.addBinding", json!({ "name": BINDING })).await;
    let _ = send(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": RECORDER_JS }),
    )
    .await;
    // 弹窗当前文档也即时注入(可能已加载完)。
    let _ = send(
        "Runtime.evaluate",
        json!({ "expression": RECORDER_JS, "returnByValue": true }),
    )
    .await;
    Some(sid)
}

/// 只录真实页面导航:http/https/file;跳过 `about:blank`/`data:`/`chrome:` 等。
fn is_recordable_url(url: &str) -> bool {
    (url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://"))
        && url != "about:blank"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recordable_url_filter() {
        assert!(is_recordable_url("https://example.com/"));
        assert!(is_recordable_url("http://127.0.0.1:8080/a"));
        assert!(is_recordable_url("file:///tmp/x.html"));
        assert!(!is_recordable_url("about:blank"));
        assert!(!is_recordable_url("data:text/html,<p>hi"));
        assert!(!is_recordable_url("chrome://newtab/"));
        assert!(!is_recordable_url(""));
    }

    #[test]
    fn recorder_js_embedded() {
        assert!(RECORDER_JS.contains("__drission_record"));
        assert!(RECORDER_JS.contains("addEventListener"));
    }
}
