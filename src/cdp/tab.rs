//! [`ChromiumTab`]:CDP 后端的标签页对象,对标 Camoufox 后端的 `Tab`。
//!
//! 高层能力:导航 / `run_js` / 标题·URL / **元素句柄查找**(`ele`/`eles`)/ 便捷点击输入 /
//! 原生可信低层鼠标 / 键盘 / 截图 / **网络监听**([`listen`](Self::listen))/ **请求拦截**
//! ([`intercept`](Self::intercept))。所有句柄共享同一 [`CdpCore`]。

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::{Instant, sleep, timeout_at};

use crate::cdp::core::CdpCore;
use crate::cdp::element::ChromiumElement;
use crate::cdp::interceptor::CdpIntercept;
use crate::cdp::listener::CdpListen;
use crate::cdp::types::{
    Cookie, CookieParam, DialogInfo, DownloadInfo, GetOptions, ImageFormat, LoadMode, PageRect,
    ShotOpts,
};
use crate::keys::KeyInput;
use crate::locator::{self, Query};
use crate::{Error, Result};

/// 一个 Chromium 标签页(或附着的 Electron 窗口)。克隆共享同一底层标签。
#[derive(Clone)]
pub struct ChromiumTab {
    pub(crate) core: Arc<CdpCore>,
}

impl ChromiumTab {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self { core }
    }

    /// 设置默认超时(影响 `ele` 等待 / 监听 / 拦截)。
    pub fn set_timeout(&self, d: Duration) {
        self.core.set_timeout(d);
    }

    /// 通用吐环境入口(对齐 camoufox `Tab::dump_env`):链式指定目标参数(query/header/cookie)与范围后
    /// `start()` 注入探针。详见 [`crate::envkit`]。
    ///
    /// ```ignore
    /// let mut probe = tab.dump_env().target_query("a_bogus")
    ///     .match_url("aweme/v1/web/aweme/detail").start().await?;
    /// tab.get(url).await?;
    /// let dump = probe.collect().await?;
    /// dump.write_to("./dump-env")?;
    /// ```
    pub fn dump_env(&self) -> crate::cdp::ChromiumEnvDumper {
        crate::envkit::EnvDumper::new(self.clone())
    }

    /// 关闭本标签(`Target.closeTarget`);若是 `new_tab_with` 带代理建的独立 BrowserContext,
    /// 一并销毁该上下文(对齐 camoufox `Tab::close`,池任务结束回收用)。
    pub async fn close(&self) -> Result<()> {
        let _ = self
            .core
            .conn
            .send(
                "Target.closeTarget",
                json!({ "targetId": self.core.target_id }),
                None,
            )
            .await;
        if let Some(ctx) = &self.core.browser_context_id {
            let _ = self
                .core
                .conn
                .send(
                    "Target.disposeBrowserContext",
                    json!({ "browserContextId": ctx }),
                    None,
                )
                .await;
        }
        Ok(())
    }

    /// 当前默认超时。
    pub fn timeout(&self) -> Duration {
        self.core.timeout()
    }

    /// 导航到 `url` 并等待 `load` 事件(最多默认超时)。返回是否在超时内加载完成
    /// (对齐 camoufox `Tab::get` 的 `Result<bool>`;超时返回 `false`,不报错)。
    pub async fn get(&self, url: &str) -> Result<bool> {
        let ok = self
            .navigate_once(url, self.core.timeout(), LoadMode::Normal, None)
            .await?;
        self.core.set_load_ok(ok);
        Ok(ok)
    }

    /// 带选项导航(retry/interval/timeout/load_mode/referer),对齐 camoufox `Tab::get_with`。
    pub async fn get_with(&self, url: &str, opts: &GetOptions) -> Result<bool> {
        let timeout = opts.timeout.unwrap_or_else(|| self.core.timeout());
        let mode = opts.load_mode.unwrap_or(LoadMode::Normal);
        let attempts = opts.retry + 1;
        let mut ok = false;
        for i in 0..attempts {
            ok = self
                .navigate_once(url, timeout, mode, opts.referer.as_deref())
                .await
                .unwrap_or(false);
            if ok || mode == LoadMode::None {
                break;
            }
            if i + 1 < attempts {
                sleep(opts.interval).await;
            }
        }
        self.core.set_load_ok(ok);
        Ok(ok)
    }

    /// 一次导航实现:按 `mode` 等待对应事件(`None` 模式不等)。
    async fn navigate_once(
        &self,
        url: &str,
        timeout: Duration,
        mode: LoadMode,
        referer: Option<&str>,
    ) -> Result<bool> {
        let mut events = self.core.conn.subscribe();
        let mut params = json!({ "url": url });
        if let Some(r) = referer {
            params["referrer"] = json!(r);
        }
        self.core.send("Page.navigate", params).await?;
        let Some(ev_name) = mode.cdp_event() else {
            return Ok(true); // None 模式:下发即返回
        };
        let sid = self.core.session_id.clone();
        let deadline = Instant::now() + timeout;
        let loaded = timeout_at(deadline, async {
            loop {
                match events.recv().await {
                    Ok(ev) if ev.method == ev_name && ev.session_id.as_deref() == Some(&sid) => {
                        break;
                    }
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        })
        .await
        .is_ok();
        Ok(loaded)
    }

    /// 重新加载当前页(等到 `complete` 或超时)。对齐 camoufox `Tab::reload`。
    pub async fn reload(&self) -> Result<()> {
        self.core.send("Page.reload", json!({})).await?;
        let _ = self.wait_loaded().await;
        Ok(())
    }

    /// 后退一步历史(对齐 camoufox `Tab::back`)。
    pub async fn back(&self) -> Result<()> {
        self.history_go(-1).await
    }

    /// 前进一步历史(对齐 camoufox `Tab::forward`)。
    pub async fn forward(&self) -> Result<()> {
        self.history_go(1).await
    }

    /// 历史跳转 `delta`(`-1` 后退 / `+1` 前进),越界忽略。
    async fn history_go(&self, delta: i64) -> Result<()> {
        let h = self
            .core
            .send("Page.getNavigationHistory", json!({}))
            .await?;
        let idx = h["currentIndex"].as_i64().unwrap_or(0);
        let entries = h["entries"].as_array().cloned().unwrap_or_default();
        let target = idx + delta;
        if target >= 0 && (target as usize) < entries.len() {
            if let Some(id) = entries[target as usize]["id"].as_i64() {
                self.core
                    .send("Page.navigateToHistoryEntry", json!({ "entryId": id }))
                    .await?;
                let _ = self.wait_loaded().await;
            }
        }
        Ok(())
    }

    /// 停止加载(对齐 camoufox `Tab::stop_loading`)。
    pub async fn stop_loading(&self) -> Result<()> {
        // 优先原生 Page.stopLoading;失败回退 window.stop()。
        if self.core.send("Page.stopLoading", json!({})).await.is_err() {
            let _ = self.run_js("window.stop()").await;
        }
        Ok(())
    }

    /// 当前 `document.readyState`(`loading`/`interactive`/`complete`)。
    pub async fn ready_state(&self) -> Result<String> {
        Ok(self
            .run_js("document.readyState")
            .await?
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    /// 在默认超时内等待 `document.readyState === 'complete'`;返回是否就绪(超时 `false`,不报错)。
    pub async fn wait_loaded(&self) -> Result<bool> {
        let deadline = Instant::now() + self.core.timeout();
        loop {
            if self.ready_state().await.unwrap_or_default() == "complete" {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(Duration::from_millis(80)).await;
        }
    }

    /// 最近一次 [`get`](Self::get) 是否加载成功(同步读,对齐 camoufox `Tab::url_available`)。
    pub fn url_available(&self) -> bool {
        self.core.load_ok()
    }

    /// 页面 User-Agent(`navigator.userAgent`)。
    pub async fn user_agent(&self) -> Result<String> {
        Ok(self
            .run_js("navigator.userAgent")
            .await?
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    /// 接管下一个 JS 对话框(alert/confirm/prompt),`accept` 是否确认、`prompt_text` 为 prompt 输入。
    /// 对话框会阻塞页面 JS,**必须与触发动作并发**(`tokio::join!` 触发 + 本方法)。对齐 camoufox。
    pub async fn handle_next_dialog(
        &self,
        accept: bool,
        prompt_text: Option<&str>,
    ) -> Result<DialogInfo> {
        let mut events = self.core.conn.subscribe();
        let sid = self.core.session_id.clone();
        let deadline = Instant::now() + self.core.timeout();
        let info = timeout_at(deadline, async {
            loop {
                match events.recv().await {
                    Ok(ev)
                        if ev.method == "Page.javascriptDialogOpening"
                            && ev.session_id.as_deref() == Some(&sid) =>
                    {
                        return Some(DialogInfo {
                            message: ev.params["message"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            dialog_type: ev.params["type"].as_str().unwrap_or_default().to_string(),
                            default_prompt: ev.params["defaultPrompt"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                        });
                    }
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return None,
                }
            }
        })
        .await
        .ok()
        .flatten()
        .ok_or_else(|| Error::msg("CDP: 未捕获到对话框(需与触发动作并发)"))?;
        let mut p = json!({ "accept": accept });
        if let Some(t) = prompt_text {
            p["promptText"] = json!(t);
        }
        self.core.send("Page.handleJavaScriptDialog", p).await?;
        Ok(info)
    }

    // ── 下载 ────────────────────────────────────────────────────────────────

    /// 设下载目录并允许下载(`Browser.setDownloadBehavior`,对齐 camoufox 下载落盘)。
    /// 同时记录到内核,使后续 [`downloads()`](Self::downloads) 的 `start()` 可直接读取。
    pub async fn set_download_path(&self, dir: impl AsRef<std::path::Path>) -> Result<()> {
        let dir = dir.as_ref();
        let _ = std::fs::create_dir_all(dir);
        self.core
            .send(
                "Browser.setDownloadBehavior",
                json!({ "behavior": "allow", "downloadPath": dir.display().to_string(), "eventsEnabled": true }),
            )
            .await?;
        self.core.set_download_dir(dir.to_path_buf());
        Ok(())
    }

    /// 下载管理句柄(对标 DP `tab.downloads` / 对齐 camoufox `Tab::downloads`):多任务并发跟踪 +
    /// 任务列表 + 进度 + 重命名。基于 CDP 原生下载事件。需先设下载目录
    /// (`ChromiumOptions::download_path` 或 [`set_download_path`](Self::set_download_path))。
    pub fn downloads(&self) -> crate::cdp::ChromiumDownloads {
        crate::cdp::ChromiumDownloads::new(self.core.clone())
    }

    /// 等待一次下载开始并完成(需先 [`set_download_path`](Self::set_download_path)),返回信息。
    /// **必须与触发下载的动作并发**。对齐 camoufox `Tab::wait_download`。
    pub async fn wait_download(&self, timeout: Duration) -> Result<DownloadInfo> {
        let mut events = self.core.conn.subscribe();
        let sid = self.core.session_id.clone();
        let deadline = Instant::now() + timeout;
        let mut info = DownloadInfo::default();
        let done = timeout_at(deadline, async {
            loop {
                match events.recv().await {
                    Ok(ev) if ev.session_id.as_deref() == Some(&sid) => match ev.method.as_str() {
                        "Page.downloadWillBegin" => {
                            info.url = ev.params["url"].as_str().unwrap_or_default().to_string();
                            info.suggested_filename = ev.params["suggestedFilename"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string();
                            info.state = "inProgress".into();
                        }
                        "Page.downloadProgress" => {
                            if ev.params["state"].as_str() == Some("completed") {
                                info.state = "completed".into();
                                return true;
                            } else if ev.params["state"].as_str() == Some("canceled") {
                                info.state = "canceled".into();
                                return true;
                            }
                        }
                        _ => {}
                    },
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return false,
                }
            }
        })
        .await
        .unwrap_or(false);
        if !done && info.state.is_empty() {
            return Err(Error::msg("CDP: 等待下载超时(未捕获下载事件)"));
        }
        Ok(info)
    }

    // ── 登录态快照(storage_state)────────────────────────────────────────────

    /// 导出当前登录态:全部 cookie + 当前源的 localStorage(JSON)。对齐 camoufox `storage_state`。
    pub async fn storage_state(&self) -> Result<Value> {
        let cookies = self.core.send("Storage.getCookies", json!({})).await?["cookies"].clone();
        let ls = self
            .run_js(
                "(function(){ try { const o={}; for (let i=0;i<localStorage.length;i++){ const k=localStorage.key(i); o[k]=localStorage.getItem(k);} return JSON.stringify({origin: location.origin, localStorage: o}); } catch(e){ return '{}'; } })()",
            )
            .await?;
        let origin_ls: Value = ls
            .as_str()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(json!({}));
        Ok(json!({ "cookies": cookies, "origins": [origin_ls] }))
    }

    /// 导入登录态(cookie + 当前源 localStorage)。对齐 camoufox `apply_storage_state`。
    pub async fn apply_storage_state(&self, state: &Value) -> Result<()> {
        if let Some(cookies) = state["cookies"].as_array() {
            self.core
                .send("Storage.setCookies", json!({ "cookies": cookies }))
                .await?;
        }
        if let Some(origins) = state["origins"].as_array() {
            for o in origins {
                if let Some(ls) = o["localStorage"].as_object() {
                    let pairs = serde_json::to_string(ls).unwrap_or_else(|_| "{}".into());
                    let js = format!(
                        "(function(d){{ try {{ for (const k in d) localStorage.setItem(k, d[k]); }} catch(e){{}} }})({pairs})"
                    );
                    let _ = self.run_js(&js).await;
                }
            }
        }
        Ok(())
    }

    /// 在页面执行 JS 表达式,返回结果值(`Runtime.evaluate`,自动 await Promise)。
    pub async fn run_js(&self, expression: &str) -> Result<Value> {
        self.core.eval_value(expression).await
    }

    /// 页面标题。
    pub async fn title(&self) -> Result<String> {
        Ok(self
            .run_js("document.title")
            .await?
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    /// 当前 URL。
    pub async fn url(&self) -> Result<String> {
        Ok(self
            .run_js("location.href")
            .await?
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    /// 页面 HTML(`documentElement.outerHTML`)。
    pub async fn html(&self) -> Result<String> {
        Ok(self
            .run_js("document.documentElement.outerHTML")
            .await?
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    // ── 元素句柄 ──────────────────────────────────────────────────────────

    /// 查找第一个匹配元素(DP 定位语法);未找到立即返回 [`Error::ElementNotFound`]。
    pub async fn ele(&self, selector: &str) -> Result<ChromiumElement> {
        match self
            .core
            .eval_handle(&doc_query_expr(selector, true))
            .await?
        {
            Some(oid) => Ok(ChromiumElement::new(self.core.clone(), oid)),
            None => Err(Error::ElementNotFound(selector.to_string())),
        }
    }

    /// 查找第一个匹配元素,在默认超时内**轮询等待**它出现;超时返回 [`Error::ElementNotFound`]。
    pub async fn wait_ele(
        &self,
        selector: &str,
        timeout: Option<Duration>,
    ) -> Result<ChromiumElement> {
        let deadline = Instant::now() + timeout.unwrap_or_else(|| self.core.timeout());
        loop {
            if let Some(oid) = self
                .core
                .eval_handle(&doc_query_expr(selector, true))
                .await?
            {
                return Ok(ChromiumElement::new(self.core.clone(), oid));
            }
            if Instant::now() >= deadline {
                return Err(Error::ElementNotFound(selector.to_string()));
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    /// 查找所有匹配元素。
    pub async fn eles(&self, selector: &str) -> Result<Vec<ChromiumElement>> {
        let Some(arr) = self
            .core
            .eval_handle(&doc_query_expr(selector, false))
            .await?
        else {
            return Ok(Vec::new());
        };
        let oids = self.core.array_object_ids(&arr).await?;
        Ok(oids
            .into_iter()
            .map(|oid| ChromiumElement::new(self.core.clone(), oid))
            .collect())
    }

    /// 元素可见文本(CSS/xpath 定位);未找到返回 `None`。便捷封装。
    pub async fn ele_text(&self, selector: &str) -> Result<Option<String>> {
        match self.ele(selector).await {
            Ok(el) => Ok(Some(el.text().await?)),
            Err(Error::ElementNotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    // ── 静态(离线)元素:解析当前 HTML 快照,后续读取不再与浏览器通信 ──────────

    /// 解析当前页面 HTML 为静态根元素(对齐 camoufox `Tab::s_root`)。
    pub async fn s_root(&self) -> Result<crate::static_element::StaticElement> {
        crate::static_element::StaticElement::parse(&self.html().await?)
    }

    /// 静态查找第一个匹配元素(离线解析,极快;对齐 camoufox `Tab::s_ele`)。
    pub async fn s_ele(&self, selector: &str) -> Result<crate::static_element::StaticElement> {
        crate::static_element::StaticElement::parse(&self.html().await?)?.ele(selector)
    }

    /// 静态查找所有匹配元素(对齐 camoufox `Tab::s_eles`)。
    pub async fn s_eles(
        &self,
        selector: &str,
    ) -> Result<Vec<crate::static_element::StaticElement>> {
        crate::static_element::StaticElement::parse(&self.html().await?)?.eles(selector)
    }

    /// 取 `selector` 命中的 `<iframe>` 的内容帧用于帧内查找(对齐 camoufox `Tab::get_frame`)。
    pub async fn get_frame(&self, selector: &str) -> Result<crate::cdp::frame::ChromiumFrame> {
        self.ele(selector).await?.content_frame().await
    }

    /// 元素是否存在(轻量:只查一次、不等待)。对标 Camoufox 后端 `Tab::exists`。
    pub async fn exists(&self, selector: &str) -> Result<bool> {
        Ok(self
            .core
            .eval_handle(&doc_query_expr(selector, true))
            .await?
            .is_some())
    }

    /// **可信点击**定位到的元素(底层走元素句柄的原生点击)。未找到返回
    /// [`Error::ElementNotFound`](对齐 camoufox `Tab::click` 的 `Result<()>`)。
    pub async fn click(&self, selector: &str) -> Result<()> {
        self.ele(selector).await?.click().await
    }

    /// 给输入框填值(一次性插入)。未找到返回 [`Error::ElementNotFound`]
    /// (对齐 camoufox `Tab::input` 的 `Result<()>`)。
    pub async fn input(&self, selector: &str, text: &str) -> Result<()> {
        self.ele(selector).await?.input(text).await
    }

    // ── 低层输入(原生可信)──────────────────────────────────────────────

    /// 移动鼠标到视口坐标 `(x, y)`(未按下)。
    pub async fn mouse_move(&self, x: f64, y: f64) -> Result<()> {
        self.core
            .dispatch_mouse("mouseMoved", x, y, "none", 0, 0)
            .await
    }

    /// 在 `(x, y)` 按下左键。
    pub async fn mouse_down(&self, x: f64, y: f64) -> Result<()> {
        self.core
            .dispatch_mouse("mousePressed", x, y, "left", 1, 1)
            .await
    }

    /// 在 `(x, y)` 松开左键。
    pub async fn mouse_up(&self, x: f64, y: f64) -> Result<()> {
        self.core
            .dispatch_mouse("mouseReleased", x, y, "left", 0, 1)
            .await
    }

    /// 按住左键移动到 `(x, y)`(拖拽中的 move,`buttons=1`)。
    pub async fn mouse_drag(&self, x: f64, y: f64) -> Result<()> {
        self.core
            .dispatch_mouse("mouseMoved", x, y, "none", 1, 0)
            .await
    }

    /// **不等往返**移动鼠标(拟人密集采样用,节奏交给调用方 `sleep`)。对齐 camoufox `mouse_move_fast`。
    pub fn mouse_move_fast(&self, x: f64, y: f64) -> Result<()> {
        self.core
            .dispatch_mouse_fire("mouseMoved", x, y, "none", 0, 0)
    }

    /// **不等往返**按住左键移动(拖拽密集采样)。对齐 camoufox `mouse_drag_fast`。
    pub fn mouse_drag_fast(&self, x: f64, y: f64) -> Result<()> {
        self.core
            .dispatch_mouse_fire("mouseMoved", x, y, "none", 1, 0)
    }

    /// 在视口坐标 `(x, y)` 派发滚轮事件(可信 `mouseWheel`)。对齐 camoufox `wheel_at`。
    pub async fn wheel_at(&self, x: f64, y: f64, delta_x: f64, delta_y: f64) -> Result<()> {
        self.core
            .send(
                "Input.dispatchMouseEvent",
                json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": delta_x, "deltaY": delta_y }),
            )
            .await?;
        Ok(())
    }

    /// 在视口一个固定可见点派发滚轮事件(`wheel_at` 的便捷版)。对齐 camoufox `wheel`。
    pub async fn wheel(&self, delta_x: f64, delta_y: f64) -> Result<()> {
        self.wheel_at(10.0, 10.0, delta_x, delta_y).await
    }

    /// 直接滚动文档 `window.scrollBy(x, y)`(对齐 camoufox `scroll_by`)。
    pub async fn scroll_by(&self, x: f64, y: f64) -> Result<()> {
        self.run_js(&format!("window.scrollBy({x},{y})")).await?;
        Ok(())
    }

    /// 敲一个键(普通字符或特殊键名,见 [`Keys`](crate::keys::Keys))。
    pub async fn press_key(&self, key: &str) -> Result<()> {
        self.core.press_key(key).await
    }

    /// **修饰组合键 / 热键**:最后一项为主键,其余为修饰键(`Control`/`Ctrl`、`Shift`、`Alt`、
    /// `Meta`/`Cmd`)。CDP 原生 `modifiers` 位掩码,页面读得到 `e.ctrlKey`/`metaKey` 等为 `true`。
    /// 作用于**当前焦点**(先 `ele.click()`/`focus()` 选中目标);元素级用 [`ChromiumElement::shortcut`]。
    ///
    /// ```ignore
    /// use drission::prelude::*;
    /// tab.key_combo(&[Keys::CONTROL, "a"]).await?; // 全选
    /// tab.key_combo(&[Keys::CONTROL, Keys::SHIFT, "z"]).await?; // 重做
    /// ```
    pub async fn key_combo(&self, keys: &[&str]) -> Result<()> {
        self.core.key_combo(keys).await
    }

    /// 按**序列**输入(文本片段直接插入、特殊键派发按键)。需先聚焦目标(如先 `ele.click()`)。
    pub async fn type_keys(&self, parts: &[KeyInput]) -> Result<()> {
        for p in parts {
            match p {
                KeyInput::Text(t) => self.core.insert_text(t).await?,
                KeyInput::Key(k) => self.core.press_key(k).await?,
            }
        }
        Ok(())
    }

    // ── 截图 ──────────────────────────────────────────────────────────────

    /// 可视区截图(PNG 字节)。
    pub async fn screenshot_bytes(&self) -> Result<Vec<u8>> {
        let r = self
            .core
            .send("Page.captureScreenshot", json!({ "format": "png" }))
            .await?;
        let data = r["data"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: 无截图数据"))?;
        crate::util::base64_decode(data).ok_or_else(|| Error::msg("CDP: 截图 base64 解码失败"))
    }

    /// 整页截图(PNG 字节,`captureBeyondViewport`)。
    pub async fn screenshot_full_bytes(&self) -> Result<Vec<u8>> {
        let r = self
            .core
            .send(
                "Page.captureScreenshot",
                json!({ "format": "png", "captureBeyondViewport": true }),
            )
            .await?;
        let data = r["data"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: 无整页截图数据"))?;
        crate::util::base64_decode(data).ok_or_else(|| Error::msg("CDP: 整页截图 base64 解码失败"))
    }

    /// 截图存盘(`full_page=true` 整页);自动建父目录。对标 Camoufox 后端 `Tab::get_screenshot`。
    pub async fn get_screenshot(
        &self,
        path: impl AsRef<std::path::Path>,
        full_page: bool,
    ) -> Result<std::path::PathBuf> {
        let path = path.as_ref().to_path_buf();
        let bytes = if full_page {
            self.screenshot_full_bytes().await?
        } else {
            self.screenshot_bytes().await?
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&path, &bytes)?;
        Ok(path)
    }

    /// 全能力截图(区域 / 整页 / 格式 / 质量)。对齐 camoufox `Tab::screenshot(&ShotOpts)`。
    pub async fn screenshot(&self, opts: &ShotOpts) -> Result<Vec<u8>> {
        let mut params = json!({ "format": opts.format.cdp_format() });
        if let (Some(q), ImageFormat::Jpeg) = (opts.quality, opts.format) {
            params["quality"] = json!(q);
        }
        if let Some(((l, t), (r, b))) = opts.region {
            params["clip"] = json!({ "x": l, "y": t, "width": (r - l).max(1.0), "height": (b - t).max(1.0), "scale": 1 });
            params["captureBeyondViewport"] = json!(true);
        } else if opts.full_page {
            params["captureBeyondViewport"] = json!(true);
        }
        let resp = self.core.send("Page.captureScreenshot", params).await?;
        let data = resp["data"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: 无截图数据"))?;
        crate::util::base64_decode(data).ok_or_else(|| Error::msg("CDP: 截图 base64 解码失败"))
    }

    /// 截图返回 base64 字符串(`full_page` 整页)。对齐 camoufox `Tab::screenshot_base64`。
    pub async fn screenshot_base64(&self, full_page: bool) -> Result<String> {
        let mut params = json!({ "format": "png" });
        if full_page {
            params["captureBeyondViewport"] = json!(true);
        }
        let resp = self.core.send("Page.captureScreenshot", params).await?;
        Ok(resp["data"].as_str().unwrap_or("").to_string())
    }

    // ── 页面几何 ────────────────────────────────────────────────────────────

    /// 可视视口尺寸 `(innerWidth, innerHeight)`。对齐 camoufox `Tab::size`。
    pub async fn size(&self) -> Result<(f64, f64)> {
        let v = self.run_js("[innerWidth, innerHeight]").await?;
        let f = |i: usize| v.get(i).and_then(Value::as_f64).unwrap_or(0.0);
        Ok((f(0), f(1)))
    }

    /// 整页内容尺寸 `(scrollWidth, scrollHeight)`。对齐 camoufox `Tab::page_size`。
    pub async fn page_size(&self) -> Result<(f64, f64)> {
        let v = self
            .run_js("[document.documentElement.scrollWidth, document.documentElement.scrollHeight]")
            .await?;
        let f = |i: usize| v.get(i).and_then(Value::as_f64).unwrap_or(0.0);
        Ok((f(0), f(1)))
    }

    /// 页面尺寸 + 滚动 + dpr 综合信息。对齐 camoufox `Tab::rect`。
    pub async fn rect(&self) -> Result<PageRect> {
        let v = self
            .run_js(
                "({ww:innerWidth,wh:innerHeight,pw:document.documentElement.scrollWidth,\
                 ph:document.documentElement.scrollHeight,sx:scrollX,sy:scrollY,dpr:devicePixelRatio})",
            )
            .await?;
        let f = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        Ok(PageRect {
            window_width: f("ww"),
            window_height: f("wh"),
            page_width: f("pw"),
            page_height: f("ph"),
            scroll_x: f("sx"),
            scroll_y: f("sy"),
            device_pixel_ratio: f("dpr"),
        })
    }

    // ── cookie(浏览器 → Session 交接 / 存盘)──────────────────────────────

    /// 取**全部** cookie(原始 CDP `Storage.getCookies` 结果,**含 httpOnly**)。
    ///
    /// 每项含 `name`/`value`/`domain`/`path`/`expires`/`httpOnly`/`secure` 等字段。
    /// 用于把登录态交接给 [`SessionPage`](crate::session::SessionPage)(纯 HTTP 接力)或存盘复用。
    pub async fn get_cookies(&self) -> Result<Vec<Value>> {
        let r = self.core.send("Storage.getCookies", json!({})).await?;
        Ok(r["cookies"].as_array().cloned().unwrap_or_default())
    }

    /// 取全部 cookie 为**类型化** [`Cookie`](对齐 camoufox `Tab::cookies`)。
    pub async fn cookies(&self) -> Result<Vec<Cookie>> {
        let r = self.core.send("Storage.getCookies", json!({})).await?;
        let s = |c: &Value, k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Ok(r["cookies"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|c| Cookie {
                name: s(c, "name"),
                value: s(c, "value"),
                domain: s(c, "domain"),
                path: s(c, "path"),
                expires: c.get("expires").and_then(Value::as_f64).unwrap_or(-1.0),
                http_only: c.get("httpOnly").and_then(Value::as_bool).unwrap_or(false),
                secure: c.get("secure").and_then(Value::as_bool).unwrap_or(false),
            })
            .collect())
    }

    /// 批量设置 cookie(对齐 camoufox `Tab::set_cookies`)。每项至少 `name`/`value` + `url` 或 `domain`。
    pub async fn set_cookies(&self, cookies: Vec<CookieParam>) -> Result<()> {
        let arr: Vec<Value> = cookies
            .iter()
            .map(|c| {
                let mut o = json!({ "name": c.name, "value": c.value });
                if let Some(u) = &c.url {
                    o["url"] = json!(u);
                }
                if let Some(d) = &c.domain {
                    o["domain"] = json!(d);
                }
                if let Some(p) = &c.path {
                    o["path"] = json!(p);
                }
                if let Some(v) = c.secure {
                    o["secure"] = json!(v);
                }
                if let Some(v) = c.http_only {
                    o["httpOnly"] = json!(v);
                }
                if let Some(v) = c.expires {
                    o["expires"] = json!(v);
                }
                o
            })
            .collect();
        self.core
            .send("Storage.setCookies", json!({ "cookies": arr }))
            .await?;
        Ok(())
    }

    /// 识别图片验证码:`<img>` 的 data:URL 直接解码、否则元素截图,再走 OCR。需 `--features ocr`。
    /// 对齐 camoufox `Tab::ocr_image`(进程内懒加载共享模型)。
    #[cfg(feature = "ocr")]
    pub async fn ocr_image(&self, selector: &str) -> Result<String> {
        use tokio::sync::OnceCell;
        static OCR: OnceCell<crate::ocr::Ocr> = OnceCell::const_new();
        let ocr = OCR.get_or_try_init(crate::ocr::Ocr::new).await?;
        let el = self.ele(selector).await?;
        let src = el.attr("src").await.ok().flatten().unwrap_or_default();
        let bytes = if src.starts_with("data:") {
            let b64 = src.split_once(',').map(|x| x.1).unwrap_or("");
            crate::util::base64_decode(b64).unwrap_or_default()
        } else {
            el.screenshot_bytes().await?
        };
        ocr.recognize(&bytes)
    }

    // ── 监听 / 拦截句柄 ──────────────────────────────────────────────────

    /// 动作链句柄(对标 DP `tab.actions`):链式串鼠标/键盘动作,`perform().await` 执行。
    pub fn actions(&self) -> crate::cdp::ChromiumActions {
        crate::cdp::ChromiumActions::new(self.core.clone())
    }

    /// 滚动句柄(对标 DP `tab.scroll`):`to_top`/`to_bottom`/`by`/`to_location`/`to_see`。
    pub fn scroll(&self) -> crate::cdp::ChromiumScroll {
        crate::cdp::ChromiumScroll::new(self.core.clone())
    }

    /// 设置句柄(对标 DP `tab.set`):`timeout`/`user_agent`/`cookies`/`window()`。
    pub fn set(&self) -> crate::cdp::ChromiumSetTab {
        crate::cdp::ChromiumSetTab::new(self.core.clone())
    }

    /// 等待句柄(对标 DP `tab.wait`):`doc_loaded`/`ele_displayed`/`ele_deleted`/`title_contains`/`url_contains`。
    pub fn wait(&self) -> crate::cdp::ChromiumWait {
        crate::cdp::ChromiumWait::new(self.core.clone())
    }

    /// 翻页采集(对齐 camoufox `Tab::paginate`):每页回调 `f` 收集结果,点 `next_selector`
    /// 直到按钮不存在/不可点/达 `max_pages`。
    pub async fn paginate<F, Fut, T>(
        &self,
        next_selector: &str,
        max_pages: usize,
        mut f: F,
    ) -> Result<Vec<T>>
    where
        F: FnMut(&ChromiumTab) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut out = Vec::new();
        for _ in 0..max_pages {
            out.push(f(self).await?);
            match self.ele(next_selector).await {
                Ok(btn) => {
                    if !btn.is_clickable().await.unwrap_or(false) {
                        break;
                    }
                    btn.click().await?;
                    let _ = self.wait_loaded().await;
                    sleep(Duration::from_millis(300)).await;
                }
                Err(_) => break,
            }
        }
        Ok(out)
    }

    /// 控制台监听句柄(对标 DP `tab.console`)。**注意**:`start()` 会开 `Runtime.enable`(反检测取舍)。
    pub fn console(&self) -> crate::cdp::ChromiumConsole {
        crate::cdp::ChromiumConsole::new(self.core.clone())
    }

    /// WebSocket 帧监听句柄(对标 DP 增强):基于 `Network` 域,不涉及 `Runtime.enable`。
    pub fn websocket(&self) -> crate::cdp::ChromiumWsListener {
        crate::cdp::ChromiumWsListener::new(self.core.clone())
    }

    /// 录像/连拍句柄(对标 DP `tab.screencast`):后台按帧间隔截图存 PNG 帧。
    pub fn screencast(&self) -> crate::cdp::ChromiumScreencast {
        crate::cdp::ChromiumScreencast::new(self.core.clone())
    }

    /// 网络监听句柄(对标 DP `tab.listen`):`start`/`wait`/`wait_count`/`stop`。
    pub fn listen(&self) -> CdpListen {
        CdpListen::new(self.core.clone())
    }

    /// 请求拦截句柄(对标 DP 拦截增强):`start`/`next`/`stop`,请求级 `resume`/`fulfill`/`abort`。
    pub fn intercept(&self) -> CdpIntercept {
        CdpIntercept::new(self.core.clone())
    }
}

/// 在 `document` 上查元素的 JS 表达式(CSS 走 `querySelector(All)`、xpath 走 `document.evaluate`)。
pub(crate) fn doc_query_expr(selector: &str, single: bool) -> String {
    match locator::parse(selector) {
        Query::Css(sel) => {
            let s = serde_json::to_string(&sel).unwrap_or_else(|_| "\"\"".into());
            if single {
                format!("document.querySelector({s})")
            } else {
                format!("Array.from(document.querySelectorAll({s}))")
            }
        }
        Query::Xpath(xp) => {
            let s = serde_json::to_string(&xp).unwrap_or_else(|_| "\"\"".into());
            if single {
                format!("document.evaluate({s}, document, null, 9, null).singleNodeValue")
            } else {
                format!(
                    "(function(){{ const it=document.evaluate({s}, document, null, 7, null); \
                     const a=[]; for (let i=0;i<it.snapshotLength;i++) a.push(it.snapshotItem(i)); return a; }})()"
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::doc_query_expr;

    #[test]
    fn css_query_expr() {
        // 裸 `h1` 在 DP 语法里是“文本包含”;CSS 标签用 `css:`/`tag:` 前缀,#/. 简写也是 CSS。
        assert_eq!(
            doc_query_expr("css:h1", true),
            "document.querySelector(\"h1\")"
        );
        assert_eq!(
            doc_query_expr("#a .b", false),
            "Array.from(document.querySelectorAll(\"#a .b\"))"
        );
    }

    #[test]
    fn xpath_query_expr() {
        let s = doc_query_expr("xpath://div[@id=\"x\"]", true);
        assert!(s.starts_with("document.evaluate("));
        assert!(s.contains("singleNodeValue"));
    }
}
