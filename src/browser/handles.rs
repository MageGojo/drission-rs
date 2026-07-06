//! DrissionPage 风格的"句柄对象":把一组相关操作收敛到 `tab.wait.*` / `tab.scroll.*` / `tab.set.*`。
//!
//! 这些句柄都只持有一个轻量 [`Tab`] 克隆(共享内核),即用即弃:
//!
//! ```ignore
//! tab.wait().doc_loaded(None).await?;                 // 等文档加载完成
//! tab.wait().ele_displayed("#login", None).await?;    // 等元素出现且可见
//! tab.scroll().to_bottom().await?;                    // 滚到底
//! tab.set().timeout(Duration::from_secs(10));         // 改默认超时
//! tab.set().load_mode(LoadMode::Eager);               // 改默认加载模式
//! ```

use std::time::Duration;

use serde_json::json;
use tokio::time::Instant;

use crate::browser::interceptor::InterceptedRequest;
use crate::browser::listener::DataPacket;
use crate::browser::tab::{CookieParam, ListenStream, LoadMode, Tab};
use crate::{Error, Result};

/// `tab.wait()` 返回的等待句柄(对应 DP `tab.wait`)。
pub struct Wait {
    tab: Tab,
}

impl Wait {
    pub(crate) fn new(tab: Tab) -> Self {
        Self { tab }
    }

    fn timeout_or_default(&self, t: Option<Duration>) -> Duration {
        t.unwrap_or_else(|| self.tab.core.timeout())
    }

    /// 单纯睡眠 `secs` 秒(对应 DP `tab.wait(2)`)。
    pub async fn secs(&self, secs: f64) {
        tokio::time::sleep(Duration::from_secs_f64(secs.max(0.0))).await;
    }

    /// 等待文档加载完成(`readyState === 'complete'`)。超时返回 `false`。
    pub async fn doc_loaded(&self, timeout: Option<Duration>) -> Result<bool> {
        let d = self.timeout_or_default(timeout);
        self.tab.core.poll_ready_complete(d).await
    }

    /// 等待元素出现于 DOM(不要求可见)。超时返回 `false`。
    pub async fn ele_loaded(&self, selector: &str, timeout: Option<Duration>) -> Result<bool> {
        let deadline = Instant::now() + self.timeout_or_default(timeout);
        loop {
            if self.tab.find_once(selector).await?.is_some() {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// 等待元素出现**且可见**。超时返回 `false`。
    pub async fn ele_displayed(&self, selector: &str, timeout: Option<Duration>) -> Result<bool> {
        let deadline = Instant::now() + self.timeout_or_default(timeout);
        loop {
            if let Some(el) = self.tab.find_once(selector).await?
                && el.is_displayed().await.unwrap_or(false)
            {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// 等待元素从 DOM 中消失。超时返回 `false`。
    pub async fn ele_deleted(&self, selector: &str, timeout: Option<Duration>) -> Result<bool> {
        let deadline = Instant::now() + self.timeout_or_default(timeout);
        loop {
            if self.tab.find_once(selector).await?.is_none() {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// 等待"自然上传"的文件路径被填入(对应 DP `tab.wait.upload_paths_inputted`)。
    ///
    /// 先 [`tab.set().upload_files(..)`](SetTab::upload_files)、再点击触发文件框的按钮,最后用本方法
    /// 等待填入完成。`timeout=None` 用默认超时;超时返回 `false`(不报错)。
    pub async fn upload_paths_inputted(&self, timeout: Option<Duration>) -> Result<bool> {
        self.tab.wait_upload_paths_inputted(timeout).await
    }

    /// 等待标题包含子串(与 cdp 端 `wait().title_contains` 对齐)。超时返回 `false`。
    pub async fn title_contains(&self, sub: &str, timeout: Option<Duration>) -> Result<bool> {
        let deadline = Instant::now() + self.timeout_or_default(timeout);
        loop {
            if self.tab.title().await.unwrap_or_default().contains(sub) {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// 等待 URL 包含子串(与 cdp 端 `wait().url_contains` 对齐)。超时返回 `false`。
    pub async fn url_contains(&self, sub: &str, timeout: Option<Duration>) -> Result<bool> {
        let deadline = Instant::now() + self.timeout_or_default(timeout);
        loop {
            if self.tab.url().await.unwrap_or_default().contains(sub) {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// 等待本标签弹出的**新标签 / 弹窗**(`window.open` / `target=_blank` / `Ctrl+点击`),返回可驱动的
    /// 新 [`Tab`]。超时返回 `None`(对标 Playwright `expect_popup` / DP `tab.wait.new_tab`,与 cdp 端
    /// `wait().new_tab` 对齐)。
    ///
    /// **用法**:先调用本方法(或与触发动作 `tokio::join!` 并发),**再**触发弹窗——内部在等待前即订阅
    /// 事件,触发太早会漏掉。只认与打开者**同一 BrowserContext** 新出现的 page(弹窗与打开者同上下文),
    /// 故不会误抓其它标签。
    pub async fn new_tab(&self, timeout: Option<Duration>) -> Result<Option<Tab>> {
        let conn = self.tab.core.conn.clone();
        let opener = self.tab.core.target_id.clone();
        let ctx = self.tab.core.browser_context_id.clone();
        let dl = self.tab.core.download_path.clone();
        let dur = self.timeout_or_default(timeout);
        let deadline = Instant::now() + dur;
        let mut events = conn.subscribe();
        loop {
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                return Ok(None);
            }
            let ev = match tokio::time::timeout(remain, events.recv()).await {
                Ok(Ok(ev)) => ev,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => return Ok(None),
                Err(_) => return Ok(None),
            };
            if ev.method != "Browser.attachedToTarget" {
                continue;
            }
            let ti = &ev.params["targetInfo"];
            if ti["type"].as_str() != Some("page") {
                continue;
            }
            let target_id = ti["targetId"].as_str().unwrap_or_default().to_string();
            if target_id.is_empty() || target_id == opener {
                continue;
            }
            // 只认与打开者同 BrowserContext 的新 page(本标签的弹窗);别的标签是别的 context。
            if ti["browserContextId"].as_str() != Some(ctx.as_str()) {
                continue;
            }
            let Some(session_id) = ev.params["sessionId"].as_str().map(str::to_string) else {
                continue;
            };
            let tab = Tab::from_attached(conn, target_id, ctx, session_id, events, dl, dur).await?;
            return Ok(Some(tab));
        }
    }
}

/// `tab.scroll()` 返回的滚动句柄(对应 DP `tab.scroll`)。
pub struct Scroll {
    tab: Tab,
}

impl Scroll {
    pub(crate) fn new(tab: Tab) -> Self {
        Self { tab }
    }

    /// 滚到顶部。
    pub async fn to_top(&self) -> Result<()> {
        self.run("window.scrollTo(window.scrollX, 0)").await
    }

    /// 滚到底部。
    pub async fn to_bottom(&self) -> Result<()> {
        self.run(
            "window.scrollTo(window.scrollX, \
             Math.max(document.documentElement.scrollHeight, \
             document.body ? document.body.scrollHeight : 0))",
        )
        .await
    }

    /// 滚到最左。
    pub async fn to_left(&self) -> Result<()> {
        self.run("window.scrollTo(0, window.scrollY)").await
    }

    /// 滚到最右。
    pub async fn to_right(&self) -> Result<()> {
        self.run(
            "window.scrollTo(Math.max(document.documentElement.scrollWidth, \
             document.body ? document.body.scrollWidth : 0), window.scrollY)",
        )
        .await
    }

    /// 垂直滚到中部。
    pub async fn to_half(&self) -> Result<()> {
        self.run(
            "window.scrollTo(window.scrollX, \
             Math.max(document.documentElement.scrollHeight, \
             document.body ? document.body.scrollHeight : 0) / 2)",
        )
        .await
    }

    /// 滚到绝对位置 `(x, y)`。
    pub async fn to_location(&self, x: f64, y: f64) -> Result<()> {
        self.run(&format!("window.scrollTo({x}, {y})")).await
    }

    /// 向上滚 `pixel` 像素。
    pub async fn up(&self, pixel: f64) -> Result<()> {
        self.tab.scroll_by(0.0, -pixel).await
    }

    /// 向下滚 `pixel` 像素。
    pub async fn down(&self, pixel: f64) -> Result<()> {
        self.tab.scroll_by(0.0, pixel).await
    }

    /// 向左滚 `pixel` 像素。
    pub async fn left(&self, pixel: f64) -> Result<()> {
        self.tab.scroll_by(-pixel, 0.0).await
    }

    /// 向右滚 `pixel` 像素。
    pub async fn right(&self, pixel: f64) -> Result<()> {
        self.tab.scroll_by(pixel, 0.0).await
    }

    async fn run(&self, js: &str) -> Result<()> {
        self.tab.run_js(js).await?;
        Ok(())
    }
}

/// `tab.set()` 返回的设置句柄(对应 DP `tab.set`)。
pub struct SetTab {
    tab: Tab,
}

impl SetTab {
    pub(crate) fn new(tab: Tab) -> Self {
        Self { tab }
    }

    /// 为本标签(其 BrowserContext)设置 cookie。
    pub async fn cookies(&self, cookies: Vec<CookieParam>) -> Result<()> {
        self.tab.set_cookies(cookies).await
    }

    /// 运行时修改本标签的默认操作超时(影响 `ele`/`get`/`listen_wait` 等)。
    pub fn timeout(&self, d: Duration) {
        self.tab.core.set_timeout(d);
    }

    /// 运行时修改本标签的默认加载模式(影响后续 `get`)。
    pub fn load_mode(&self, m: LoadMode) {
        self.tab.core.set_load_mode(m);
    }

    /// 覆盖本标签的 User-Agent(对后续导航生效)。
    pub async fn user_agent(&self, ua: &str) -> Result<()> {
        self.tab
            .core
            .conn
            .send(
                "Browser.setUserAgentOverride",
                json!({
                    "browserContextId": self.tab.core.browser_context_id,
                    "userAgent": ua,
                }),
                None,
            )
            .await?;
        Ok(())
    }

    /// 设置"自然上传"要用的文件并武装文件选择器拦截(对应 DP `tab.set.upload_files`)。
    ///
    /// 之后点击会弹出文件选择框的按钮 → 用 [`Wait::upload_paths_inputted`] 等待填入完成。
    /// 详见 [`Tab::set_upload_files`](crate::browser::Tab::set_upload_files)。
    pub async fn upload_files(&self, paths: &[&str]) -> Result<()> {
        self.tab.set_upload_files(paths).await
    }

    /// 写 `localStorage`(便捷,经 JS;与 cdp 端 `set().local_storage_set` 对齐)。
    pub async fn local_storage_set(&self, key: &str, value: &str) -> Result<()> {
        self.storage_set("localStorage", key, value).await
    }
    /// 读 `localStorage`(不存在返回 `None`)。
    pub async fn local_storage_get(&self, key: &str) -> Result<Option<String>> {
        self.storage_get("localStorage", key).await
    }
    /// 删 `localStorage` 项。
    pub async fn local_storage_remove(&self, key: &str) -> Result<()> {
        self.storage_remove("localStorage", key).await
    }
    /// 清空 `localStorage`。
    pub async fn local_storage_clear(&self) -> Result<()> {
        self.tab.run_js("localStorage.clear()").await?;
        Ok(())
    }
    /// 写 `sessionStorage`。
    pub async fn session_storage_set(&self, key: &str, value: &str) -> Result<()> {
        self.storage_set("sessionStorage", key, value).await
    }
    /// 读 `sessionStorage`。
    pub async fn session_storage_get(&self, key: &str) -> Result<Option<String>> {
        self.storage_get("sessionStorage", key).await
    }
    /// 删 `sessionStorage` 项。
    pub async fn session_storage_remove(&self, key: &str) -> Result<()> {
        self.storage_remove("sessionStorage", key).await
    }
    /// 清空 `sessionStorage`。
    pub async fn session_storage_clear(&self) -> Result<()> {
        self.tab.run_js("sessionStorage.clear()").await?;
        Ok(())
    }

    async fn storage_set(&self, store: &str, key: &str, value: &str) -> Result<()> {
        let js = format!("{store}.setItem({}, {})", js_str(key), js_str(value));
        self.tab.run_js(&js).await?;
        Ok(())
    }
    async fn storage_get(&self, store: &str, key: &str) -> Result<Option<String>> {
        let js = format!("{store}.getItem({})", js_str(key));
        Ok(self.tab.run_js(&js).await?.as_str().map(str::to_string))
    }
    async fn storage_remove(&self, store: &str, key: &str) -> Result<()> {
        let js = format!("{store}.removeItem({})", js_str(key));
        self.tab.run_js(&js).await?;
        Ok(())
    }

    /// 窗口句柄(对应 DP `tab.set.window`)。
    ///
    /// **注意平台限制**:Camoufox(Firefox/Juggler)**不支持**最小化 / 全屏 / 移动主窗口
    /// (Firefox 自 7 起禁止脚本移动/缩放主窗口,Juggler 也无 `setWindowBounds`),故只提供
    /// **可靠生效**的尺寸控制,见 [`Window`]。
    pub fn window(&self) -> Window {
        Window::new(self.tab.clone())
    }
}

/// JS 字符串字面量(用 `serde_json` 转义,安全嵌入表达式)。
fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// `tab.set().window()` 返回的窗口句柄(对应 DP `tab.set.window`)。
///
/// **平台限制(重要)**:Camoufox 基于 Firefox、自动化走 Juggler 协议——Firefox 自 7 起**禁止脚本
/// 移动 / 缩放主窗口**(`dom.disable_window_move_resize` 也只对 `window.open` 弹窗有效),Juggler 又
/// **没有** `setWindowBounds` 这类 OS 窗口方法。因此 **最小化 / 全屏切换 / 移动主窗口在本内核上做不到**
/// (属平台限制,非库缺陷)。本句柄只提供**可靠生效**的尺寸控制:`Page.setViewportSize`——**有头**会
/// 连带把窗口缩放到该尺寸、**无头**仅设内容视口。需要固定**初始** OS 窗口大小请在启动时用
/// [`BrowserOptions::window_size`](crate::launcher::BrowserOptions::window_size)。
pub struct Window {
    tab: Tab,
}

impl Window {
    pub(crate) fn new(tab: Tab) -> Self {
        Self { tab }
    }

    /// 设置视口/窗口大小(有头:窗口随之缩放;无头:设内容视口)。对应 DP `tab.set.window.size`。
    pub async fn size(&self, width: u32, height: u32) -> Result<()> {
        self.tab.core.set_viewport_size(width, height).await
    }

    /// `size` 的别名(语义上即"设置内容视口")。
    pub async fn viewport(&self, width: u32, height: u32) -> Result<()> {
        self.tab.core.set_viewport_size(width, height).await
    }

    /// 近似最大化:读 `screen.availWidth/availHeight` 后铺满可用屏幕(对应 DP `tab.set.window.max`)。
    ///
    /// 注:这是用尺寸近似("内容铺满可用屏幕"),并非 OS 级"最大化窗口状态"(Firefox/Juggler 无此能力)。
    pub async fn max(&self) -> Result<()> {
        let v = self
            .tab
            .run_js("[screen.availWidth, screen.availHeight]")
            .await?;
        let w = v.get(0).and_then(|x| x.as_f64()).unwrap_or(1280.0) as u32;
        let h = v.get(1).and_then(|x| x.as_f64()).unwrap_or(800.0) as u32;
        self.tab.core.set_viewport_size(w, h).await
    }
}

/// `tab.intercept()` 返回的请求拦截句柄(对应 DP 风格;统一 `listen()`/`console()` 的句柄风格)。
///
/// 把扁平的 `intercept_start`/`intercept_xhr`/`intercept_next`/`intercept_stop` 收敛成句柄式
/// (底层完全复用,不改变既有行为)。每个被拦请求必须用 `resume`/`resume_with`/`fulfill`/`abort`
/// 之一放行(见 [`InterceptedRequest`])。
pub struct Intercept {
    tab: Tab,
}

impl Intercept {
    pub(crate) fn new(tab: Tab) -> Self {
        Self { tab }
    }

    /// 开始拦截:`targets` 为 URL 子串过滤(空切片=拦截所有请求)。务必在 `get` **之前**调用。
    pub async fn start(&self, targets: &[&str]) -> Result<()> {
        self.tab.intercept_start(targets).await
    }

    /// 仅拦截 XHR/fetch 类请求(其余自动放行)。
    pub async fn start_xhr(&self, targets: &[&str]) -> Result<()> {
        self.tab.intercept_xhr(targets).await
    }

    /// 是否正在拦截。
    pub async fn is_intercepting(&self) -> bool {
        self.tab.core.interceptor.lock().await.is_some()
    }

    /// 等待下一个被拦请求(默认超时;超时返回 [`Error::Timeout`])。
    pub async fn next(&self) -> Result<InterceptedRequest> {
        self.tab.intercept_next().await
    }

    /// 等待下一个被拦请求(自定义超时;超时返回 `None`)。
    pub async fn next_timeout(&self, timeout: Duration) -> Result<Option<InterceptedRequest>> {
        self.tab.intercept_next_timeout(timeout).await
    }

    /// 停止拦截并清空状态。
    pub async fn stop(&self) -> Result<()> {
        self.tab.intercept_stop().await
    }
}

/// `tab.listen()` 返回的网络监听句柄(对应 DP `tab.listen`)。
///
/// 把扁平的 `listen_start` / `listen_wait` / `listen_stream` / `listen_stop` 收敛成 DP 风格的一组
/// 操作(底层完全复用,不改变长监听/单次抓取的既有行为):
///
/// ```ignore
/// tab.listen().start(&["aweme/v1/web/aweme/detail"]).await?;  // 装 hook(务必在 get 之前)
/// tab.get(url).await?;
/// let pkt  = tab.listen().wait().await?;                       // 等 1 个包(默认超时)
/// let pkts = tab.listen().wait_count(3, None).await?;          // 等 3 个包
/// let stream = tab.listen().steps().await?;                    // 长监听流式句柄(后台不丢包)
/// tab.listen().stop().await?;                                  // 停止并清理
/// ```
pub struct Listen {
    tab: Tab,
}

impl Listen {
    pub(crate) fn new(tab: Tab) -> Self {
        Self { tab }
    }

    /// 开始监听:`targets` 为 URL 子串过滤(空切片=全部 fetch/XHR)。务必在 `get` **之前**调用。
    pub async fn start(&self, targets: &[&str]) -> Result<()> {
        self.tab.listen_start(targets).await
    }

    /// 是否正在监听(对应 DP `tab.listen.listening`)。
    pub async fn is_listening(&self) -> bool {
        *self.tab.core.listen_active.lock().await
    }

    /// 等待下一个匹配数据包(默认超时;超时返回 [`Error::Timeout`])。
    pub async fn wait(&self) -> Result<DataPacket> {
        self.tab.listen_wait().await
    }

    /// 等待下一个匹配数据包(自定义超时;超时返回 `None`)。
    pub async fn wait_timeout(&self, timeout: Duration) -> Result<Option<DataPacket>> {
        match self.tab.listen_wait_timeout(timeout).await {
            Ok(p) => Ok(Some(p)),
            Err(Error::Timeout(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// 取下一个数据包(默认超时;超时返回 `None`,对应 DP 不抛异常的用法)。
    pub async fn next(&self) -> Result<Option<DataPacket>> {
        self.tab.listen_next().await
    }

    /// 等待 `count` 个数据包(对应 DP `tab.listen.wait(count=N)`)。
    ///
    /// `timeout` 为抓满 `count` 个的**总超时**(`None`=默认超时);若到点仍不足,返回已抓到的那些
    /// (可能少于 `count`),不报错——由调用方按 `len()` 判断是否凑齐。
    pub async fn wait_count(
        &self,
        count: usize,
        timeout: Option<Duration>,
    ) -> Result<Vec<DataPacket>> {
        let total = timeout.unwrap_or_else(|| self.tab.core.timeout());
        let deadline = Instant::now() + total;
        let mut out = Vec::with_capacity(count);
        while out.len() < count {
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                break;
            }
            match self.tab.listen_wait_timeout(remain).await {
                Ok(p) => out.push(p),
                Err(Error::Timeout(_)) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    /// 开启**长监听**并返回流式句柄(对应 DP `tab.listen.steps()`)。后台抽取不丢包,适合"边滑边抓"。
    pub async fn steps(&self) -> Result<ListenStream> {
        self.tab.listen_stream().await
    }

    /// 停止监听并清空状态(同时停掉长监听后台任务);保留 `add_init_script` 注入的通用脚本。
    pub async fn stop(&self) -> Result<()> {
        self.tab.listen_stop().await
    }
}
