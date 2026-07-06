//! 附着 **OOPIF(out-of-process iframe,跨域 iframe)/ worker** 子 target,把它当成可独立逆向的 tab。
//!
//! 站点隔离(site-isolation)下,跨域 iframe(典型如 Cloudflare Turnstile 的
//! `challenges.cloudflare.com`)跑在**独立进程**,主 target 的 `Runtime.evaluate` /
//! `Page.createIsolatedWorld{frameId}` 都进不去([`ChromiumFrame`](crate::cdp::ChromiumFrame)
//! 适用于**同进程**子帧)。要真正进到这种 iframe 内部逆向,必须用
//! `Target.setAutoAttach{flatten}` 拿到子 target 的独立 `sessionId`。
//!
//! 拿到 `sessionId` 后,本库的 [`CdpCore`] 完全按 session 工作,于是可以把这个 iframe **当成一个
//! 普通 [`ChromiumTab`]** 来用——在它上面 `scripts()`(dump challenge 脚本)/ `debugger()`(下断点抠
//! 解扰逻辑)/ `hook()`(偷 key/iv)/ `listen()`(抓包)/ `run_js()`。
//!
//! ```no_run
//! use drission::prelude::*;
//! use std::time::Duration;
//! # async fn f(tab: ChromiumTab) -> drission::Result<()> {
//! // 触发 Turnstile 后,等它的跨域 iframe 被附着:
//! if let Some(cf) = tab.wait_oopif("challenges.cloudflare.com", Some(Duration::from_secs(8))).await? {
//!     // 进到 iframe 内部 dump 它的 challenge 脚本:
//!     for s in cf.tab().scripts().list().await? {
//!         println!("{} ({}B)", s.url, s.length);
//!     }
//! }
//! # Ok(()) }
//! ```

use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, timeout};

use super::core::CdpCore;
use super::tab::ChromiumTab;
use crate::Result;

/// 一个被附着的子 target(跨域 iframe / worker),带一个可独立驱动 **且可逆向** 的 [`ChromiumTab`]。
pub struct ChildTarget {
    /// 子 target id。
    pub target_id: String,
    /// 附着会话 id(`flatten` 模式,与主连接复用同一条传输)。
    pub session_id: String,
    /// 类型:`iframe` / `worker` / `shared_worker` / `service_worker` / `page` 等。
    pub kind: String,
    /// 子 target 的 URL(如 `https://challenges.cloudflare.com/cdn-cgi/challenge-platform/...`)。
    /// iframe 刚附着、尚未导航时可能为空。
    pub url: String,
    tab: ChromiumTab,
}

impl ChildTarget {
    /// 该子 target 的可驱动 tab。可在其上 `scripts()` / `debugger()` / `hook()` / `listen()` /
    /// `run_js()` —— 即把这个跨域 iframe **当成一个普通标签**逆向。
    pub fn tab(&self) -> &ChromiumTab {
        &self.tab
    }

    /// 取走 tab(消费 `self`)。
    pub fn into_tab(self) -> ChromiumTab {
        self.tab
    }

    /// URL 是否包含子串(过滤目标 iframe 用)。
    pub fn url_contains(&self, s: &str) -> bool {
        self.url.contains(s)
    }

    /// 是否是 iframe 类型(过滤掉 worker 等)。
    pub fn is_iframe(&self) -> bool {
        self.kind == "iframe"
    }
}

impl ChromiumTab {
    /// 开启对子 target(OOPIF 跨域 iframe / 专用 worker)的**自动附着**,收集 `settle` 时间窗内
    /// 已附着的全部子 target;每个都带一个可独立逆向的 [`ChromiumTab`]。
    ///
    /// 站点隔离下,跨域 iframe(如 Cloudflare Turnstile)跑在独立进程、主 target 进不去;本方法用
    /// `Target.setAutoAttach{flatten:true}` 拿到它们的独立 `sessionId`,从而能在 iframe 内部
    /// `scripts()`(dump challenge 脚本)/ `debugger()`(下断点)/ `hook()`(偷 key)/ `listen()`(抓包)。
    ///
    /// 语义:先订阅事件再开启 auto-attach —— 开启那一刻会对**当前已存在**的子 target 补发
    /// `attachedToTarget`,故能拿到调用时刻已存在的子 target;`settle` 是等这些事件汇齐的静默窗
    /// (典型 1–2s)。**调用时机**:应在「目标 iframe 已出现」之后(如触发 Turnstile、页面渲染完)再调。
    /// 嵌套 OOPIF 直接对返回项的 `child.tab().attach_oopifs(..)` 继续下钻即可。
    pub async fn attach_oopifs(&self, settle: Duration) -> Result<Vec<ChildTarget>> {
        let mut events = self.core.conn.subscribe();
        self.enable_auto_attach().await?;
        let mut out: Vec<ChildTarget> = Vec::new();
        let deadline = Instant::now() + settle;
        loop {
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                break;
            }
            let ev = match timeout(remain, events.recv()).await {
                Ok(Ok(ev)) => ev,
                Ok(Err(RecvError::Lagged(_))) => continue,
                Ok(Err(RecvError::Closed)) => break,
                Err(_) => break, // settle 到点:正常收尾
            };
            if ev.method != "Target.attachedToTarget" {
                continue;
            }
            if let Some(child) = self.child_from_event(&ev.params) {
                if !out.iter().any(|c| c.session_id == child.session_id) {
                    out.push(child);
                }
            }
        }
        Ok(out)
    }

    /// 等待一个 URL 含 `url_substr` 的子 target 被附着(如等 Turnstile 的
    /// `challenges.cloudflare.com` iframe 出现),返回其可逆向 tab。超时返回 `None`。
    /// 会先开启 auto-attach;`timeout_dur` 为 `None` 时用本标签默认超时。
    pub async fn wait_oopif(
        &self,
        url_substr: &str,
        timeout_dur: Option<Duration>,
    ) -> Result<Option<ChildTarget>> {
        let mut events = self.core.conn.subscribe();
        self.enable_auto_attach().await?;
        let dur = timeout_dur.unwrap_or_else(|| self.core.timeout());
        let deadline = Instant::now() + dur;
        loop {
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                return Ok(None);
            }
            let ev = match timeout(remain, events.recv()).await {
                Ok(Ok(ev)) => ev,
                Ok(Err(RecvError::Lagged(_))) => continue,
                Ok(Err(RecvError::Closed)) => return Ok(None),
                Err(_) => return Ok(None),
            };
            if ev.method != "Target.attachedToTarget" {
                continue;
            }
            if let Some(child) = self.child_from_event(&ev.params) {
                if child.url.contains(url_substr) {
                    return Ok(Some(child));
                }
            }
        }
    }

    /// 在本标签 session 上开启 `flatten` 自动附着(对已存在 + 之后出现的子 target 都发
    /// `attachedToTarget`)。`waitForDebuggerOnStart:false`：不在子 target 第一行暂停。
    async fn enable_auto_attach(&self) -> Result<()> {
        self.core
            .send(
                "Target.setAutoAttach",
                json!({
                    "autoAttach": true,
                    "waitForDebuggerOnStart": false,
                    "flatten": true,
                }),
            )
            .await?;
        Ok(())
    }

    /// 从 `Target.attachedToTarget` 事件参数造一个 [`ChildTarget`](含可驱动 tab)。
    /// **不**预先开任何 CDP 域(`scripts()`/`debugger()` 自己开 `Debugger`、`listen()` 自己开
    /// `Network`),从而对 iframe 与 worker 都通用(worker 不支持 `Page` 域)。
    fn child_from_event(&self, params: &Value) -> Option<ChildTarget> {
        let session_id = params["sessionId"].as_str()?.to_string();
        let ti = &params["targetInfo"];
        let target_id = ti["targetId"].as_str().unwrap_or_default().to_string();
        let kind = ti["type"].as_str().unwrap_or_default().to_string();
        let url = ti["url"].as_str().unwrap_or_default().to_string();
        let core = CdpCore::new(
            self.core.conn.clone(),
            session_id.clone(),
            target_id.clone(),
            self.core.download_dir(),
            self.core.browser_context_id.clone(),
        );
        Some(ChildTarget {
            target_id,
            session_id,
            kind,
            url,
            tab: ChromiumTab::new(core),
        })
    }
}
