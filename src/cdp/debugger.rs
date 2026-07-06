//! CDP **调试器** [`ChromiumDebugger`](`tab.debugger()`):逆向「主动定位」的重武器。
//!
//! 此前库几乎全压在「补环境 / 纯算复现」一环,**最前面的「主动定位」基本空白**(`Debugger.*` /
//! `DOMDebugger` 全库 0 命中)。本模块补上逆向工程师的核心动作:**下断点 → 抓调用栈 → 读私有变量**。
//!
//! ```no_run
//! use drission::prelude::*;
//! # async fn f(tab: ChromiumTab) -> drission::Result<()> {
//! let dbg = tab.debugger();
//! // 一键在「URL 含 /drama/page 的 XHR/fetch 发出处」断下:
//! dbg.break_on_xhr("/drama/page").await?;
//! // ... 触发该请求(点击 / 导航 / 调 JS)...
//! if let Some(stack) = dbg.wait_paused(None).await? {
//!     // 调用栈:函数名 + 脚本 + 行列 —— 一眼看出 x-ca-sign 在哪一行生成
//!     for (i, f) in stack.frames().iter().enumerate() {
//!         println!("#{i} {} @ {}:{}:{}", f.function_name, f.url, f.line, f.column);
//!     }
//!     // 在断住的闭包里直接读私有变量(看 key/iv 神器):
//!     println!("key = {}", stack.eval(0, "key").await?);
//!     for (name, val) in stack.locals(0).await? { println!("  {name} = {val}"); }
//!     stack.resume().await?; // 用完务必放行(drop 也会兜底放行)
//! }
//! # Ok(()) }
//! ```
//!
//! **反检测代价**:`Debugger.enable` 会被部分站点的「CDP 探测」识别,故按需调用、用完即停;极致反
//! 检测(过盾)那条线继续走「不开 Runtime/Debugger」的既有路径。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, timeout_at};

use super::core::CdpCore;
use crate::{Error, Result};

/// 调试器句柄(`tab.debugger()`)。轻量包裹 [`CdpCore`]。
pub struct ChromiumDebugger {
    core: Arc<CdpCore>,
}

impl ChromiumDebugger {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self { core }
    }

    /// 开启 `Debugger` 域(其余断点方法会自动先调本方法)。
    pub async fn enable(&self) -> Result<()> {
        self.core.send("Debugger.enable", json!({})).await?;
        Ok(())
    }

    /// 关闭 `Debugger` 域(连带清掉所有断点;反检测收尾用)。
    pub async fn disable(&self) -> Result<()> {
        let _ = self.core.send("Debugger.disable", json!({})).await;
        Ok(())
    }

    /// **XHR/fetch 断点**:当请求 URL **包含** `url_substr` 时,在「发起处」断下
    /// (`DOMDebugger.setXHRBreakpoint`)。空串 = 拦所有 XHR/fetch。这是定位「签名在哪生成」的首选:
    /// 断在请求发出的那一刻,调用栈回溯即可看到签名函数。
    pub async fn break_on_xhr(&self, url_substr: &str) -> Result<()> {
        self.enable().await?;
        self.core
            .send("DOMDebugger.setXHRBreakpoint", json!({ "url": url_substr }))
            .await?;
        Ok(())
    }

    /// 拦所有 XHR/fetch(等价 `break_on_xhr("")`)。
    pub async fn break_on_xhr_any(&self) -> Result<()> {
        self.break_on_xhr("").await
    }

    /// 移除某 XHR 断点(`url_substr` 须与设置时一致)。
    pub async fn clear_xhr_breakpoint(&self, url_substr: &str) -> Result<()> {
        let _ = self
            .core
            .send(
                "DOMDebugger.removeXHRBreakpoint",
                json!({ "url": url_substr }),
            )
            .await;
        Ok(())
    }

    /// **事件监听器断点**:当某类 DOM 事件触发时断下(`DOMDebugger.setEventListenerBreakpoint`)。
    /// `event_name` 如 `"click"`/`"submit"`/`"keydown"`——定位「点了按钮后哪段 JS 在跑」。
    pub async fn break_on_event(&self, event_name: &str) -> Result<()> {
        self.enable().await?;
        self.core
            .send(
                "DOMDebugger.setEventListenerBreakpoint",
                json!({ "eventName": event_name }),
            )
            .await?;
        Ok(())
    }

    /// 移除事件监听器断点。
    pub async fn clear_event_breakpoint(&self, event_name: &str) -> Result<()> {
        let _ = self
            .core
            .send(
                "DOMDebugger.removeEventListenerBreakpoint",
                json!({ "eventName": event_name }),
            )
            .await;
        Ok(())
    }

    /// 按「脚本 URL(正则)+ 行号」下断点(`Debugger.setBreakpointByUrl`)。`condition` 为可选条件
    /// 表达式(非空才在为真时断)。返回 breakpointId。
    pub async fn break_at(
        &self,
        url_regex: &str,
        line: u32,
        column: Option<u32>,
        condition: Option<&str>,
    ) -> Result<String> {
        self.enable().await?;
        let mut p = json!({ "lineNumber": line, "urlRegex": url_regex });
        if let Some(c) = column {
            p["columnNumber"] = json!(c);
        }
        if let Some(c) = condition.filter(|s| !s.is_empty()) {
            p["condition"] = json!(c);
        }
        let r = self.core.send("Debugger.setBreakpointByUrl", p).await?;
        Ok(r["breakpointId"].as_str().unwrap_or_default().to_string())
    }

    /// 移除按 id 的断点。
    pub async fn remove_breakpoint(&self, breakpoint_id: &str) -> Result<()> {
        let _ = self
            .core
            .send(
                "Debugger.removeBreakpoint",
                json!({ "breakpointId": breakpoint_id }),
            )
            .await;
        Ok(())
    }

    /// 启用/停用全部断点(`Debugger.setBreakpointsActive`)。
    pub async fn set_breakpoints_active(&self, active: bool) -> Result<()> {
        self.core
            .send("Debugger.setBreakpointsActive", json!({ "active": active }))
            .await?;
        Ok(())
    }

    /// **黑盒**(blackbox)若干脚本:命中这些正则的脚本里**不停断、调用栈折叠**
    /// (`Debugger.setBlackboxPatterns`)。配合 ④ 反「无限 debugger」:把反调试脚本拉黑即可。
    pub async fn blackbox(&self, patterns: &[&str]) -> Result<()> {
        self.enable().await?;
        self.core
            .send(
                "Debugger.setBlackboxPatterns",
                json!({ "patterns": patterns }),
            )
            .await?;
        Ok(())
    }

    /// 立刻在下一条 JS 语句处主动断下(`Debugger.pause`)——配合 `wait_paused` 抓当前栈。
    pub async fn pause(&self) -> Result<()> {
        self.enable().await?;
        self.core.send("Debugger.pause", json!({})).await?;
        Ok(())
    }

    /// **CDP 原生通杀反调试**(`Debugger.setSkipAllPauses`):`skip=true` 时调试器**对一切暂停视而不见**
    /// ——`debugger` 语句、计时型反调试、间接/`eval` 生成的 `debugger`、甚至你自己的断点**全部跳过、永不暂停**。
    /// 这是对付「无限 debugger / checkPerformance 计时」最稳的一招(注入式 defuse 拦不住间接 `debugger` 时用它)。
    /// 想重新用断点定位时再 `set_skip_all_pauses(false)`。需 `Debugger` 域已开(本方法自动先开)。
    pub async fn set_skip_all_pauses(&self, skip: bool) -> Result<()> {
        self.enable().await?;
        self.core
            .send("Debugger.setSkipAllPauses", json!({ "skip": skip }))
            .await?;
        Ok(())
    }

    /// **反「无限 debugger」**:导航前注入 defuse 脚本——把 `setInterval/setTimeout(()=>{debugger})`
    /// 与 `Function("debugger")()` / `[]['constructor']['constructor']('debugger')()` 这类反调试
    /// **致残**(剔除 `debugger` 语句),使断点跟栈不被反复打断。自包含,**不依赖 `Debugger` 域开启**;
    /// 与 CDP 侧 [`blackbox`](Self::blackbox) 互补。**在导航前调用**(对当前文档也即时生效一次)。
    pub async fn anti_anti_debug(&self) -> Result<()> {
        self.core
            .send(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": ANTI_DEBUG_JS }),
            )
            .await?;
        let _ = self.core.eval_value(ANTI_DEBUG_JS).await;
        Ok(())
    }

    /// 等待**一次** `Debugger.paused`(`timeout=None` 用标签默认超时),返回断点现场 [`PausedStack`]。
    /// 超时返回 `None`。
    pub async fn wait_paused(&self, timeout: Option<Duration>) -> Result<Option<PausedStack>> {
        let mut events = self.core.conn.subscribe();
        let sid = self.core.session_id.clone();
        let deadline = Instant::now() + timeout.unwrap_or_else(|| self.core.timeout());
        let got = timeout_at(deadline, async {
            loop {
                match events.recv().await {
                    Ok(ev)
                        if ev.method == "Debugger.paused"
                            && ev.session_id.as_deref() == Some(&sid) =>
                    {
                        return Some(ev.params);
                    }
                    Ok(_) => continue,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return None,
                }
            }
        })
        .await
        .ok()
        .flatten();
        Ok(got.map(|params| PausedStack::from_event(self.core.clone(), &params)))
    }

    /// **量化反调试强度**:在 `window` 时间窗内统计 `Debugger.paused` 触发次数(站点「无限
    /// debugger」会疯狂触发)。`auto_resume=true` 时每次暂停**立即非阻塞放行**(页面不冻结)。
    /// 返回 `(次数, 首个暂停的调用栈摘要)`——次数高即反调试凶,调用栈直指那段反调试代码。
    ///
    /// 用法:对照 [`anti_anti_debug`](super::ChromiumTab::anti_anti_debug) 前后跑一遍,次数应从「几十」降到「~0」。
    /// 单订阅、无竞态(不像循环调 `wait_paused` 会在两次订阅间漏事件而卡死)。
    pub async fn count_pauses(
        &self,
        window: Duration,
        auto_resume: bool,
    ) -> Result<(usize, String)> {
        let mut events = self.core.conn.subscribe();
        let sid = self.core.session_id.clone();
        let deadline = Instant::now() + window;
        let mut count = 0usize;
        let mut first_bt = String::new();
        loop {
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                break;
            }
            match tokio::time::timeout(remain, events.recv()).await {
                Ok(Ok(ev))
                    if ev.method == "Debugger.paused" && ev.session_id.as_deref() == Some(&sid) =>
                {
                    count += 1;
                    if first_bt.is_empty() {
                        first_bt = backtrace_from_params(&ev.params);
                    }
                    if auto_resume {
                        let _ =
                            self.core
                                .conn
                                .fire_session("Debugger.resume", json!({}), Some(&sid));
                    }
                }
                Ok(Ok(_)) => continue,
                Ok(Err(RecvError::Lagged(_))) => continue,
                Ok(Err(RecvError::Closed)) => break,
                Err(_) => break,
            }
        }
        Ok((count, first_bt))
    }

    /// 便捷:按「URL 正则 + 行/列」下断点 → **先订阅** → 跑 `trigger`(你在里面触发命中)→ 等断下。
    /// 一步到位,且**杜绝竞态**:断点先 arm、事件先订阅,再触发,故即使触发后**立即**命中也不漏事件
    /// (避免页面在断点处冻结而 `wait_paused` 永远收不到 → 死锁)。适合「连续触发」的场景
    /// (如视频自适应流每个分片 URL 都过同一解扰函数)。
    ///
    /// **`trigger` 必须是 fire-and-forget**(`tab.run_js("setTimeout(()=>{…},300);1")` 这类立即返回的),
    /// **切勿**在 `trigger` 里 await 一个会命中断点的 promise(会卡死)。
    #[allow(clippy::too_many_arguments)]
    pub async fn break_at_then<F, Fut>(
        &self,
        url_regex: &str,
        line: u32,
        column: Option<u32>,
        condition: Option<&str>,
        trigger: F,
        timeout: Option<Duration>,
    ) -> Result<Option<PausedStack>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        self.break_at(url_regex, line, column, condition).await?;
        let mut events = self.core.conn.subscribe(); // 先订阅,避免触发后立即命中而漏事件
        trigger().await?;
        let sid = self.core.session_id.clone();
        let deadline = Instant::now() + timeout.unwrap_or_else(|| self.core.timeout());
        let got = timeout_at(deadline, async {
            loop {
                match events.recv().await {
                    Ok(ev)
                        if ev.method == "Debugger.paused"
                            && ev.session_id.as_deref() == Some(&sid) =>
                    {
                        return Some(ev.params);
                    }
                    Ok(_) => continue,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return None,
                }
            }
        })
        .await
        .ok()
        .flatten();
        Ok(got.map(|params| PausedStack::from_event(self.core.clone(), &params)))
    }

    /// 便捷:设 XHR 断点 → 跑 `trigger`(你在里面触发该请求)→ 等断下,一步到位。
    ///
    /// **`trigger` 必须是 fire-and-forget**:用可信点击 [`click`](super::ChromiumElement::click)、
    /// 导航,或 `tab.run_js("setTimeout(()=>fetch(...),0);1")` 这类**立即返回**的触发——**切勿**用
    /// `tab.run_js("fetch(...)")`(它 `awaitPromise`,会卡在断点处永不返回 → 死锁)。
    pub async fn break_on_xhr_then<F, Fut>(
        &self,
        url_substr: &str,
        trigger: F,
        timeout: Option<Duration>,
    ) -> Result<Option<PausedStack>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        self.break_on_xhr(url_substr).await?;
        let mut events = self.core.conn.subscribe(); // 先订阅,避免 trigger 太快漏掉事件
        trigger().await?;
        let sid = self.core.session_id.clone();
        let deadline = Instant::now() + timeout.unwrap_or_else(|| self.core.timeout());
        let got = timeout_at(deadline, async {
            loop {
                match events.recv().await {
                    Ok(ev)
                        if ev.method == "Debugger.paused"
                            && ev.session_id.as_deref() == Some(&sid) =>
                    {
                        return Some(ev.params);
                    }
                    Ok(_) => continue,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return None,
                }
            }
        })
        .await
        .ok()
        .flatten();
        Ok(got.map(|params| PausedStack::from_event(self.core.clone(), &params)))
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 断点现场:调用栈 + 求值 + 放行
// ════════════════════════════════════════════════════════════════════════════

/// 一帧调用栈(`Debugger.paused` 的 `callFrames[i]`)。
#[derive(Debug, Clone)]
pub struct CallFrame {
    /// 帧 id(供 `eval` 在该帧上下文里求值)。
    pub call_frame_id: String,
    /// 函数名(匿名函数为空串)。
    pub function_name: String,
    /// 脚本 URL(内联脚本可能为空)。
    pub url: String,
    /// 脚本 id(供 `tab.scripts().source(id)` 取源码)。
    pub script_id: String,
    /// 断点行(0 基)。
    pub line: u32,
    /// 断点列(0 基)。
    pub column: u32,
    /// 该帧「local」作用域对象的 objectId(供 `locals` 展开;无则空)。
    local_scope_object_id: String,
}

/// 断点现场([`ChromiumDebugger::wait_paused`] 返回)。**用完务必 `resume()`**,否则页面一直冻结;
/// 若忘了,`drop` 时也会**尽力**发一次 `Debugger.resume` 兜底放行。
pub struct PausedStack {
    core: Arc<CdpCore>,
    reason: String,
    hit_breakpoints: Vec<String>,
    frames: Vec<CallFrame>,
    resumed: AtomicBool,
}

impl PausedStack {
    fn from_event(core: Arc<CdpCore>, params: &Value) -> Self {
        let frames = params["callFrames"]
            .as_array()
            .map(|a| a.iter().map(parse_call_frame).collect())
            .unwrap_or_default();
        let hit_breakpoints = params["hitBreakpoints"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            core,
            reason: params["reason"].as_str().unwrap_or_default().to_string(),
            hit_breakpoints,
            frames,
            resumed: AtomicBool::new(false),
        }
    }

    /// 暂停原因(`XHR`/`EventListener`/`debuggerStatement`/`other`…)。
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// 命中的断点 id 列表(按 id 断点时有值)。
    pub fn hit_breakpoints(&self) -> &[String] {
        &self.hit_breakpoints
    }

    /// 调用栈(`frames[0]` 是最内层/当前帧)。
    pub fn frames(&self) -> &[CallFrame] {
        &self.frames
    }

    /// 调用栈摘要(每行 `#i 函数名 @ url:line:col`),打印定位用。
    pub fn backtrace(&self) -> String {
        self.frames
            .iter()
            .enumerate()
            .map(|(i, f)| format_frame(i, f))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 在第 `frame_idx` 帧的上下文里求值(`Debugger.evaluateOnCallFrame`)——在断住的闭包里直接读
    /// `key`/`iv`/中间变量,**看私有变量神器**。返回 JSON 值。
    pub async fn eval(&self, frame_idx: usize, expression: &str) -> Result<Value> {
        let frame = self
            .frames
            .get(frame_idx)
            .ok_or_else(|| Error::Other(format!("调用栈无第 {frame_idx} 帧")))?;
        let r = self
            .core
            .send(
                "Debugger.evaluateOnCallFrame",
                json!({
                    "callFrameId": frame.call_frame_id,
                    "expression": expression,
                    "returnByValue": true,
                    "silent": true,
                }),
            )
            .await?;
        if let Some(exc) = r.get("exceptionDetails") {
            let msg = exc["exception"]["description"]
                .as_str()
                .or_else(|| exc["text"].as_str())
                .unwrap_or("求值异常");
            return Err(Error::Protocol(format!("evaluateOnCallFrame: {msg}")));
        }
        Ok(r["result"]["value"].clone())
    }

    /// 同 [`eval`](Self::eval),但把结果转成字符串(对象给 `description`)。
    pub async fn eval_str(&self, frame_idx: usize, expression: &str) -> Result<String> {
        let v = self.eval(frame_idx, expression).await?;
        Ok(match v {
            Value::String(s) => s,
            Value::Null => String::new(),
            other => other.to_string(),
        })
    }

    /// **把断点帧里(常为闭包私有)的表达式提升为页面全局 oracle**:在第 `frame_idx` 帧上下文求值
    /// `expr`、挂到 `window[global_name]`,使其在 [`resume`](Self::resume) **之后**仍能从任意上下文
    /// (`tab.run_js`/`Runtime.evaluate`)调用。
    ///
    /// 这是逆向最常用的一招:站点的签名器/解扰器几乎总藏在模块闭包里(全局取不到),但在断点帧里
    /// 闭包变量可见——把它(或一个调用它的小函数)挂到 `window` 上,就得到一个**可复用的纯函数 oracle**,
    /// 无需每次都靠断点。返回挂载后 `typeof window[global_name]`(成功多为 `"function"`/`"object"`;
    /// 失败为 `"ERR:<异常>"`,本方法不抛错以便调用方据返回值判断)。
    ///
    /// 例:在 YouTube cs6 断点处把模块别名 `g` 提升,得到随处可调的 n 参数解扰 oracle:
    /// ```no_run
    /// # use drission::prelude::*;
    /// # async fn f(stack: PausedStack, tab: ChromiumTab) -> drission::Result<()> {
    /// stack.expose_as_global(0, "__ytNsig", "function(u){return (new g.g7(u,!0)).get('n')}").await?;
    /// stack.resume().await?;
    /// let n = tab.run_js("window.__ytNsig('https://x/n/SCRAMBLED')").await?; // 解扰后的 n
    /// # let _ = n; Ok(()) }
    /// ```
    pub async fn expose_as_global(
        &self,
        frame_idx: usize,
        global_name: &str,
        expr: &str,
    ) -> Result<String> {
        let js = build_expose_js(global_name, expr);
        self.eval_str(frame_idx, &js).await
    }

    /// 展开第 `frame_idx` 帧的**局部变量**(`local` 作用域 → `Runtime.getProperties`)。
    /// 返回 `(变量名, 值)`;值为基本类型时是其值,对象则给 `{type, description}` 概要。
    pub async fn locals(&self, frame_idx: usize) -> Result<Vec<(String, Value)>> {
        let frame = self
            .frames
            .get(frame_idx)
            .ok_or_else(|| Error::Other(format!("调用栈无第 {frame_idx} 帧")))?;
        if frame.local_scope_object_id.is_empty() {
            return Ok(Vec::new());
        }
        let r = self
            .core
            .send(
                "Runtime.getProperties",
                json!({
                    "objectId": frame.local_scope_object_id,
                    "ownProperties": true,
                    "generatePreview": false,
                }),
            )
            .await?;
        let mut out = Vec::new();
        if let Some(list) = r["result"].as_array() {
            for p in list {
                let name = p["name"].as_str().unwrap_or_default().to_string();
                if name.is_empty() {
                    continue;
                }
                out.push((name, remote_object_to_value(&p["value"])));
            }
        }
        Ok(out)
    }

    /// 放行(`Debugger.resume`)。消费 self;之后页面继续执行。
    pub async fn resume(self) -> Result<()> {
        self.resumed.store(true, Ordering::Relaxed);
        self.core.send("Debugger.resume", json!({})).await?;
        Ok(())
    }

    /// 单步**跨过**(`Debugger.stepOver`)并等下一次断下;运行到底(不再断)返回 `None`。
    pub async fn step_over(self) -> Result<Option<PausedStack>> {
        self.step("Debugger.stepOver").await
    }

    /// 单步**进入**(`Debugger.stepInto`)。
    pub async fn step_into(self) -> Result<Option<PausedStack>> {
        self.step("Debugger.stepInto").await
    }

    /// 单步**跳出**(`Debugger.stepOut`)。
    pub async fn step_out(self) -> Result<Option<PausedStack>> {
        self.step("Debugger.stepOut").await
    }

    async fn step(self, method: &str) -> Result<Option<PausedStack>> {
        self.resumed.store(true, Ordering::Relaxed);
        let mut events = self.core.conn.subscribe();
        let sid = self.core.session_id.clone();
        self.core.send(method, json!({})).await?;
        let deadline = Instant::now() + self.core.timeout();
        let got = timeout_at(deadline, async {
            loop {
                match events.recv().await {
                    Ok(ev)
                        if ev.method == "Debugger.paused"
                            && ev.session_id.as_deref() == Some(&sid) =>
                    {
                        return Some(ev.params);
                    }
                    Ok(_) => continue,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return None,
                }
            }
        })
        .await
        .ok()
        .flatten();
        Ok(got.map(|params| PausedStack::from_event(self.core.clone(), &params)))
    }
}

impl Drop for PausedStack {
    fn drop(&mut self) {
        // 用完没显式 resume/step → 尽力发一次非阻塞放行,避免页面永久冻结。
        if !self.resumed.load(Ordering::Relaxed) {
            let _ = self.core.conn.fire_session(
                "Debugger.resume",
                json!({}),
                Some(&self.core.session_id),
            );
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 纯函数辅助(可单测,不触网)
// ════════════════════════════════════════════════════════════════════════════

/// 生成 [`expose_as_global`](PausedStack::expose_as_global) 用的 JS:在当前(断点帧)作用域里求值
/// `expr`、挂到 `window[name]`,返回挂载后的 `typeof`(异常吞掉返回 `"ERR:..."`,绝不弄崩页面)。
/// `name` 经 JSON 转义防注入;`expr` 原样嵌入(它本就是要在闭包作用域执行的表达式)。
fn build_expose_js(global_name: &str, expr: &str) -> String {
    let name = serde_json::to_string(global_name).unwrap_or_else(|_| "\"__oracle\"".to_string());
    format!(
        "(function(){{try{{window[{name}]=({expr});return typeof window[{name}];}}catch(e){{return 'ERR:'+e;}}}})()"
    )
}

/// 一帧的展示行 `#i 函数名 @ url:line:col`(匿名函数标 `(anonymous)`)。
fn format_frame(i: usize, f: &CallFrame) -> String {
    let name = if f.function_name.is_empty() {
        "(anonymous)"
    } else {
        &f.function_name
    };
    format!("#{i} {name} @ {}:{}:{}", f.url, f.line, f.column)
}

/// 直接从 `Debugger.paused` 事件参数生成调用栈摘要(供 `count_pauses` 用,不建 `PausedStack`)。
fn backtrace_from_params(params: &Value) -> String {
    params["callFrames"]
        .as_array()
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, cf)| format_frame(i, &parse_call_frame(cf)))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// 解析一帧 `callFrames[i]`。
fn parse_call_frame(cf: &Value) -> CallFrame {
    let loc = &cf["location"];
    // local 作用域对象 id(供展开局部变量)。
    let local_scope_object_id = cf["scopeChain"]
        .as_array()
        .and_then(|scopes| {
            scopes
                .iter()
                .find(|s| s["type"].as_str() == Some("local"))
                .and_then(|s| s["object"]["objectId"].as_str())
        })
        .unwrap_or_default()
        .to_string();
    CallFrame {
        call_frame_id: cf["callFrameId"].as_str().unwrap_or_default().to_string(),
        function_name: cf["functionName"].as_str().unwrap_or_default().to_string(),
        url: cf["url"].as_str().unwrap_or_default().to_string(),
        script_id: loc["scriptId"].as_str().unwrap_or_default().to_string(),
        line: loc["lineNumber"].as_u64().unwrap_or(0) as u32,
        column: loc["columnNumber"].as_u64().unwrap_or(0) as u32,
        local_scope_object_id,
    }
}

/// 把一个 CDP `RemoteObject` 转成展示用 JSON:基本类型取 `value`,对象/函数给 `{type, description}`。
fn remote_object_to_value(ro: &Value) -> Value {
    if let Some(v) = ro.get("value") {
        if !v.is_null() {
            return v.clone();
        }
    }
    if ro["type"].as_str() == Some("undefined") {
        return Value::String("undefined".into());
    }
    let ty = ro["subtype"]
        .as_str()
        .or_else(|| ro["type"].as_str())
        .unwrap_or("object");
    let desc = ro["description"]
        .as_str()
        .or_else(|| ro["className"].as_str())
        .unwrap_or(ty);
    json!({ "type": ty, "description": desc })
}

/// 反「无限 debugger」defuse 脚本(导航前注入)。三招:
/// 1. 包 `setInterval`/`setTimeout`:回调源码含 `debugger` 则换成空函数(治 `setInterval(()=>{debugger},...)`);
/// 2. 致残 `Function` 构造器:函数体里的 `debugger` 语句剔除(治 `Function("debugger")()`);
/// 3. 改写 `Function.prototype.constructor` 指向致残版(治 `[]['constructor']['constructor']('debugger')()`)。
///
/// 全程 try/catch,只剔除 `\bdebugger\b`、不动其余逻辑,尽量不误伤正常 `new Function`。
pub(crate) const ANTI_DEBUG_JS: &str = r#"(function(){
  try{
    var _si=window.setInterval, _st=window.setTimeout;
    function clean(fn){
      try{
        if(typeof fn==='function'){
          var s=Function.prototype.toString.call(fn);
          if(/\bdebugger\b/.test(s)) return function(){};
        } else if(typeof fn==='string' && /\bdebugger\b/.test(fn)){
          return fn.replace(/\bdebugger\b/g,'');
        }
      }catch(e){}
      return fn;
    }
    try{ window.setInterval=function(fn){ var a=Array.prototype.slice.call(arguments); a[0]=clean(fn); return _si.apply(this,a); }; }catch(e){}
    try{ window.setTimeout=function(fn){ var a=Array.prototype.slice.call(arguments); a[0]=clean(fn); return _st.apply(this,a); }; }catch(e){}
    try{
      var _F=window.Function;
      var FN=function(){
        var a=Array.prototype.slice.call(arguments);
        if(a.length){ var last=a[a.length-1]; if(typeof last==='string') a[a.length-1]=last.replace(/\bdebugger\b/g,''); }
        return _F.apply(this,a);
      };
      FN.prototype=_F.prototype;
      try{ FN.toString=function(){ return _F.toString(); }; }catch(e){}
      window.Function=FN;
      try{ Object.defineProperty(_F.prototype,'constructor',{value:FN,configurable:true,writable:true}); }catch(e){}
    }catch(e){}
    try{
      var _eval=window.eval;
      var EV=function(c){ if(typeof c==='string') c=c.replace(/\bdebugger\b/g,''); return _eval(c); };
      try{ EV.toString=function(){ return _eval.toString(); }; }catch(e){}
      window.eval=EV;
    }catch(e){}
  }catch(e){}
})();"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anti_debug_js_targets_known_patterns() {
        // 覆盖三类反调试:定时器 / Function 构造器 / 原型 constructor。
        assert!(ANTI_DEBUG_JS.contains("setInterval"));
        assert!(ANTI_DEBUG_JS.contains("setTimeout"));
        assert!(ANTI_DEBUG_JS.contains("window.Function"));
        assert!(ANTI_DEBUG_JS.contains("window.eval"));
        assert!(ANTI_DEBUG_JS.contains("constructor"));
        // 用词边界正则剔除,避免误伤含 debugger 子串的标识符。
        assert!(ANTI_DEBUG_JS.contains(r"\bdebugger\b"));
        // 必须 IIFE 自执行且 try 包裹(绝不弄坏页面)。
        assert!(ANTI_DEBUG_JS.trim_start().starts_with("(function()"));
    }

    #[test]
    fn build_expose_js_wraps_and_escapes() {
        let js = build_expose_js("__ytNsig", "function(u){return (new g.g7(u,!0)).get('n')}");
        // 名字 JSON 转义后嵌入(带引号),expr 原样,挂到 window 并返回 typeof。
        assert!(js.contains("window[\"__ytNsig\"]=("));
        assert!(js.contains("new g.g7(u,!0)"));
        assert!(js.contains("return typeof window[\"__ytNsig\"]"));
        // 必须 try/catch 吞异常(返回 ERR:),绝不让注入表达式崩掉页面。
        assert!(js.contains("catch(e)") && js.contains("'ERR:'+e"));
        // 含引号的名字也安全转义,不会破坏 JS 字符串。
        let js2 = build_expose_js("a\"b", "1");
        assert!(js2.contains("window[\"a\\\"b\"]=(1)"));
    }

    #[test]
    fn parse_call_frame_extracts_fields_and_local_scope() {
        let cf = json!({
            "callFrameId": "cf1",
            "functionName": "sign",
            "url": "https://api.x.com/app.js",
            "location": { "scriptId": "42", "lineNumber": 120, "columnNumber": 8 },
            "scopeChain": [
                { "type": "local", "object": { "objectId": "obj-local" } },
                { "type": "closure", "object": { "objectId": "obj-closure" } }
            ]
        });
        let f = parse_call_frame(&cf);
        assert_eq!(f.call_frame_id, "cf1");
        assert_eq!(f.function_name, "sign");
        assert_eq!(f.script_id, "42");
        assert_eq!((f.line, f.column), (120, 8));
        assert_eq!(f.local_scope_object_id, "obj-local");
    }

    #[test]
    fn paused_stack_backtrace_and_anon() {
        let params = json!({
            "reason": "XHR",
            "hitBreakpoints": ["1:0:https://x/app.js"],
            "callFrames": [
                { "callFrameId":"a", "functionName":"", "url":"https://x/app.js",
                  "location": { "scriptId":"1", "lineNumber":10, "columnNumber":2 }, "scopeChain": [] },
                { "callFrameId":"b", "functionName":"send", "url":"https://x/app.js",
                  "location": { "scriptId":"1", "lineNumber":99, "columnNumber":4 }, "scopeChain": [] }
            ]
        });
        // 不触网:只验证解析(用一个假的 core 不现实,故只测纯解析路径)。
        let frames: Vec<CallFrame> = params["callFrames"]
            .as_array()
            .unwrap()
            .iter()
            .map(parse_call_frame)
            .collect();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].function_name, "");
        assert_eq!(frames[1].function_name, "send");
        assert_eq!(frames[1].line, 99);
    }

    #[test]
    fn backtrace_from_params_formats_and_marks_anon() {
        let params = json!({
            "callFrames": [
                { "functionName":"", "url":"https://x/app.js",
                  "location": { "scriptId":"1", "lineNumber":10, "columnNumber":2 }, "scopeChain": [] },
                { "functionName":"sign", "url":"https://x/app.js",
                  "location": { "scriptId":"1", "lineNumber":99, "columnNumber":4 }, "scopeChain": [] }
            ]
        });
        let bt = backtrace_from_params(&params);
        assert!(bt.contains("#0 (anonymous) @ https://x/app.js:10:2"));
        assert!(bt.contains("#1 sign @ https://x/app.js:99:4"));
    }

    #[test]
    fn remote_object_primitive_and_object() {
        // 基本类型取 value。
        assert_eq!(
            remote_object_to_value(&json!({ "type": "string", "value": "abc" })),
            json!("abc")
        );
        assert_eq!(
            remote_object_to_value(&json!({ "type": "number", "value": 42 })),
            json!(42)
        );
        // undefined 特例。
        assert_eq!(
            remote_object_to_value(&json!({ "type": "undefined" })),
            json!("undefined")
        );
        // 对象给概要。
        let v = remote_object_to_value(
            &json!({ "type": "object", "subtype": "array", "description": "Array(3)" }),
        );
        assert_eq!(v["type"], json!("array"));
        assert_eq!(v["description"], json!("Array(3)"));
    }
}
