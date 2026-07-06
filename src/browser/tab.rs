//! 标签页 [`Tab`]:对应 DrissionPage 的 tab/page 对象。
//!
//! 每个 `Tab` 绑定一个**独立的 BrowserContext**(从而 cookie 天然隔离)+ 一个 page 会话。
//! 内部用一个"事件泵"任务跟踪导航后变化的主世界 `executionContextId`,并驱动网络监听。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout_at};

use crate::browser::actions::Actions;
use crate::browser::console::{Console, ConsoleShared};
use crate::browser::download::{DownloadShared, Downloads};
use crate::browser::element::Element;
use crate::browser::frame::Frame;
use crate::browser::handles::{Intercept, Listen, Scroll, SetTab, Wait};
use crate::browser::interceptor::{Decision, InterceptedRequest, InterceptorState};
use crate::browser::listener::{
    DRAIN_JS, DataPacket, ListenBuffer, ListenFilter, UNINSTALL_JS, hook_script, parse_packets,
};
use crate::browser::screencast::{Screencast, ScreencastShared};
use crate::browser::static_element::StaticElement;
use crate::browser::websocket::{WsListener, WsShared};
use crate::launcher::{BrowserOptions, Geolocation, OsType, Proxy};
use crate::locator::{self, Query};
use crate::protocol::{Connection, Event};
use crate::util::{base64_decode, base64_encode};
use crate::{Error, Result};

/// 页面加载等待模式(对应 DrissionPage 的 `load_mode`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoadMode {
    /// 等待 `load` 事件(整页资源加载完成)。DP 默认。
    #[default]
    Normal,
    /// 等待 `DOMContentLoaded`(DOM 就绪即可,不等图片等子资源)。
    Eager,
    /// 不等待,`Page.navigate` 下发后立即返回。
    None,
}

impl LoadMode {
    /// 该模式要等待的 `Page.eventFired` 事件名;`None` 模式不等待。
    fn event_name(self) -> Option<&'static str> {
        match self {
            LoadMode::Normal => Some("load"),
            LoadMode::Eager => Some("DOMContentLoaded"),
            LoadMode::None => Option::None,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            LoadMode::Normal => 0,
            LoadMode::Eager => 1,
            LoadMode::None => 2,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => LoadMode::Eager,
            2 => LoadMode::None,
            _ => LoadMode::Normal,
        }
    }
}

/// [`Tab::get`] / [`Tab::get_with`] 的可选参数(对应 DP `get` 的 `retry`/`interval`/`timeout`/`load_mode`)。
#[derive(Debug, Clone)]
pub struct GetOptions {
    /// 失败重试次数(总尝试 = `retry + 1`)。默认 0。
    pub retry: u32,
    /// 重试间隔。默认 1s。
    pub interval: Duration,
    /// 本次导航超时;`None` 用标签默认超时。
    pub timeout: Option<Duration>,
    /// 本次加载模式;`None` 用标签当前默认(见 [`SetTab::load_mode`])。
    pub load_mode: Option<LoadMode>,
    /// 可选 referer。
    pub referer: Option<String>,
}

impl Default for GetOptions {
    fn default() -> Self {
        Self {
            retry: 0,
            interval: Duration::from_secs(1),
            timeout: None,
            load_mode: None,
            referer: None,
        }
    }
}

impl GetOptions {
    pub fn new() -> Self {
        Self::default()
    }
    /// 设置失败重试次数。
    pub fn retry(mut self, n: u32) -> Self {
        self.retry = n;
        self
    }
    /// 设置重试间隔(秒)。
    pub fn interval(mut self, secs: f64) -> Self {
        self.interval = Duration::from_secs_f64(secs.max(0.0));
        self
    }
    /// 设置本次导航超时。
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }
    /// 设置本次加载模式。
    pub fn load_mode(mut self, m: LoadMode) -> Self {
        self.load_mode = Some(m);
        self
    }
    /// 设置 referer。
    pub fn referer(mut self, r: impl Into<String>) -> Self {
        self.referer = Some(r.into());
        self
    }
}

/// **每标签(BrowserContext)级别的覆盖项**:在 [`Browser::new_tab_with`](crate::browser::Browser::new_tab_with)
/// 打开标签时,叠加到浏览器基线选项之上,实现"同一浏览器进程内、每个标签不同代理 / 指纹"。
///
/// 这些都是 Juggler **context 级**可下发的字段(`Browser.set*Override{browserContextId}` /
/// `Browser.setContextProxy`),因此能 per-Tab 轮换。深指纹(canvas/webgl/screen/humanize)是
/// **进程级**的,无法经此覆盖——需要时给不同浏览器 worker 配不同 [`BrowserOptions`]。
///
/// 仅"设置了的项"会覆盖基线(`None` 沿用基线),便于只换代理或只换 UA。
#[derive(Debug, Clone, Default)]
pub struct ContextOverride {
    /// 覆盖代理(`Browser.setContextProxy`)。
    pub proxy: Option<Proxy>,
    /// 覆盖 User-Agent。
    pub user_agent: Option<String>,
    /// 覆盖语言 locale(如 `zh-CN`)。
    pub locale: Option<String>,
    /// 覆盖时区(如 `Asia/Shanghai`)。
    pub timezone_id: Option<String>,
    /// 覆盖 `navigator.platform`。
    pub platform: Option<String>,
    /// 覆盖目标 OS 指纹(影响 platform 推断等)。
    pub os: Option<OsType>,
    /// 覆盖地理位置。
    pub geolocation: Option<Geolocation>,
    /// 覆盖视口大小。
    pub window_size: Option<(u32, u32)>,
}

impl ContextOverride {
    pub fn new() -> Self {
        Self::default()
    }
    /// 覆盖代理。
    pub fn proxy(mut self, p: Proxy) -> Self {
        self.proxy = Some(p);
        self
    }
    /// 覆盖 User-Agent。
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }
    /// 覆盖语言 locale。
    pub fn locale(mut self, locale: impl Into<String>) -> Self {
        self.locale = Some(locale.into());
        self
    }
    /// 覆盖时区。
    pub fn timezone(mut self, tz: impl Into<String>) -> Self {
        self.timezone_id = Some(tz.into());
        self
    }
    /// 覆盖 `navigator.platform`。
    pub fn platform(mut self, platform: impl Into<String>) -> Self {
        self.platform = Some(platform.into());
        self
    }
    /// 覆盖目标 OS 指纹。
    pub fn os(mut self, os: OsType) -> Self {
        self.os = Some(os);
        self
    }
    /// 覆盖地理位置。
    pub fn geolocation(mut self, latitude: f64, longitude: f64) -> Self {
        self.geolocation = Some(Geolocation {
            latitude,
            longitude,
            accuracy: None,
        });
        self
    }
    /// 覆盖视口大小。
    pub fn window_size(mut self, width: u32, height: u32) -> Self {
        self.window_size = Some((width, height));
        self
    }

    /// 是否没有任何覆盖项(全 `None`)。
    pub fn is_empty(&self) -> bool {
        self.proxy.is_none()
            && self.user_agent.is_none()
            && self.locale.is_none()
            && self.timezone_id.is_none()
            && self.platform.is_none()
            && self.os.is_none()
            && self.geolocation.is_none()
            && self.window_size.is_none()
    }

    /// 把覆盖项叠加到 `base` 上,产出用于开标签的合并选项(只覆盖设置了的项)。
    pub(crate) fn merge_into(&self, mut base: BrowserOptions) -> BrowserOptions {
        if let Some(p) = &self.proxy {
            base.proxy = Some(p.clone());
        }
        if let Some(ua) = &self.user_agent {
            base.fingerprint.user_agent = Some(ua.clone());
        }
        if let Some(l) = &self.locale {
            base.fingerprint.locale = Some(l.clone());
        }
        if let Some(tz) = &self.timezone_id {
            base.fingerprint.timezone_id = Some(tz.clone());
        }
        if let Some(p) = &self.platform {
            base.fingerprint.platform = Some(p.clone());
        }
        if let Some(os) = self.os {
            base.fingerprint.os = Some(os);
        }
        if let Some(g) = self.geolocation {
            base.fingerprint.geolocation = Some(g);
        }
        if let Some(ws) = self.window_size {
            base.window_size = Some(ws);
        }
        base
    }
}

/// 一个 JS 对话框(alert/confirm/prompt/beforeunload)的信息。
#[derive(Debug, Clone)]
pub struct DialogInfo {
    /// 类型:`alert` / `confirm` / `prompt` / `beforeunload`。
    pub dialog_type: String,
    /// 对话框文本。
    pub message: String,
    /// `prompt` 的默认值(其它类型为 `None`)。
    pub default_value: Option<String>,
}

/// 页面尺寸信息(一次 JS 取回)。对应 DP 的 `rect`/`size` 概念。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageRect {
    /// 可视视口宽(`innerWidth`)。
    pub window_width: f64,
    /// 可视视口高(`innerHeight`)。
    pub window_height: f64,
    /// 整个文档内容宽(`scrollWidth`)。
    pub page_width: f64,
    /// 整个文档内容高(`scrollHeight`)。
    pub page_height: f64,
    /// 横向滚动位置(`scrollX`)。
    pub scroll_x: f64,
    /// 纵向滚动位置(`scrollY`)。
    pub scroll_y: f64,
    /// 设备像素比(`devicePixelRatio`)。
    pub device_pixel_ratio: f64,
}

/// 截图图片格式。Camoufox 的 `Page.screenshot` 实测仅支持 PNG / JPEG(不支持 webp)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageFormat {
    /// PNG(无损,默认)。
    #[default]
    Png,
    /// JPEG(有损,可配合 `quality`,文件更小)。
    Jpeg,
}

impl ImageFormat {
    /// 对应 `Page.screenshot` 的 `mimeType`。
    pub(crate) fn mime(self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
        }
    }

    /// 按文件扩展名推断格式(`.jpg`/`.jpeg`→JPEG,其余→PNG)。
    pub fn from_path(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("jpg") | Some("jpeg") => ImageFormat::Jpeg,
            _ => ImageFormat::Png,
        }
    }
}

/// [`Tab::screenshot`] 的可选参数(对应 DP `get_screenshot` 的 `full_page`/`left_top`/`right_bottom` 等)。
///
/// 默认截**当前视口**、PNG。`region` 一旦设置则优先于 `full_page`。
#[derive(Debug, Clone, Default)]
pub struct ShotOpts {
    /// 是否整页截图(`true` 截整个文档,`false` 截可视视口)。
    pub full_page: bool,
    /// 指定矩形区域:`((left, top), (right, bottom))`,页面坐标(含滚动偏移)。设置后忽略 `full_page`。
    pub region: Option<((f64, f64), (f64, f64))>,
    /// 图片格式(默认 PNG)。
    pub format: ImageFormat,
    /// JPEG 质量 0–100(仅 `Jpeg` 有效;`None` 用浏览器默认)。
    pub quality: Option<u8>,
}

impl ShotOpts {
    pub fn new() -> Self {
        Self::default()
    }
    /// 整页截图。
    pub fn full_page(mut self, yes: bool) -> Self {
        self.full_page = yes;
        self
    }
    /// 指定矩形区域 `((left, top), (right, bottom))`(页面坐标)。
    pub fn region(mut self, left_top: (f64, f64), right_bottom: (f64, f64)) -> Self {
        self.region = Some((left_top, right_bottom));
        self
    }
    /// 图片格式。
    pub fn format(mut self, format: ImageFormat) -> Self {
        self.format = format;
        self
    }
    /// JPEG 质量(0–100)。
    pub fn quality(mut self, q: u8) -> Self {
        self.quality = Some(q);
        self
    }

    /// 解析出 `Page.screenshot` 用的 `clip`(region 优先,否则按视口/整页由调用方算)。
    fn region_clip(&self) -> Option<Value> {
        self.region.map(|((l, t), (r, b))| {
            json!({ "x": l, "y": t, "width": (r - l).max(1.0), "height": (b - t).max(1.0) })
        })
    }
}

/// 一条 cookie(读取结果)。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: f64,
    pub http_only: bool,
    pub secure: bool,
}

/// 设置 cookie 的参数。至少需要 `name`/`value`,以及 `url` 或 `domain` 其一。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CookieParam {
    pub name: String,
    pub value: String,
    pub url: Option<String>,
    pub domain: Option<String>,
    pub path: Option<String>,
    pub secure: Option<bool>,
    pub http_only: Option<bool>,
    pub expires: Option<f64>,
}

/// 一次下载的信息(由 [`Tab::wait_download`] 返回)。
#[derive(Debug, Clone)]
pub struct DownloadInfo {
    /// 下载来源 URL。
    pub url: String,
    /// 浏览器建议的文件名。
    pub suggested_filename: String,
    /// 最终落盘路径(`download_path` 目录 + 文件名);未设下载目录时仅为文件名。
    pub path: std::path::PathBuf,
    /// 是否成功(未取消且无错误)。
    pub success: bool,
    /// 失败原因(若有)。
    pub error: Option<String>,
}

impl CookieParam {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            ..Default::default()
        }
    }
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }
}

/// Tab 的共享内核,被 [`Tab`] 与 [`Element`] 共同持有。
pub(crate) struct TabCore {
    pub conn: Connection,
    pub session_id: String,
    pub target_id: String,
    pub browser_context_id: String,
    pub main_frame_id: String,
    pub exec_ctx: watch::Receiver<Option<String>>,
    /// 每个 frame(含 iframe)的主世界 executionContextId(`frameId → ctxId`),由事件泵维护。
    pub frame_ctxs: Arc<Mutex<HashMap<String, String>>>,
    pub listen_active: Mutex<bool>,
    pub listen_buf: Mutex<ListenBuffer>,
    /// 长监听:是否已开启后台抽取任务(开启后 `drain_into_buffer` 变 no-op,避免与之竞争）。
    pub bg_active: AtomicBool,
    /// 长监听:后台抽取任务句柄(`listen_stop` 时 abort)。
    pub listen_bg: Mutex<Option<JoinHandle<()>>>,
    /// listen 安装的 fetch/XHR hook 脚本(单独存:便于与通用 init 脚本合并下发、`listen_stop` 精确移除)。
    pub listen_script: Mutex<Option<String>>,
    /// 通用【导航前注入】脚本(每个新文档最早期执行);与 listen hook 合并后一起 `setInitScripts`。
    pub init_scripts: Mutex<Vec<String>>,
    pub interceptor: Arc<Mutex<Option<InterceptorState>>>,
    pub intercept_rx: Mutex<Option<mpsc::UnboundedReceiver<InterceptedRequest>>>,
    /// 控制台监听共享状态(缓冲 + 是否在监听)。
    pub console: Arc<ConsoleShared>,
    /// 控制台监听后台任务句柄(`console.stop()` 时 abort)。
    pub console_task: Mutex<Option<JoinHandle<()>>>,
    /// WebSocket 帧监听共享状态(帧缓冲 + 连接表 + 是否在监听)。
    pub ws: Arc<WsShared>,
    /// WebSocket 帧监听后台任务句柄(`websocket().stop()` 时 abort)。
    pub ws_task: Mutex<Option<JoinHandle<()>>>,
    /// 文件选择器拦截"自然上传"(`set_upload_files`→click→`wait_upload_paths_inputted`)共享状态。
    pub upload: Arc<UploadShared>,
    /// 默认操作超时(毫秒);可经 [`SetTab::timeout`] 运行时修改。
    pub timeout_ms: AtomicU64,
    /// 默认加载模式(见 [`LoadMode`]);可经 [`SetTab::load_mode`] 运行时修改。
    pub load_mode: AtomicU8,
    /// 最近一次 `get` 是否成功(供 [`Tab::url_available`] 读取)。
    pub last_load_ok: AtomicBool,
    /// 录像(`tab.screencast()`)共享状态:配置 + 是否在录 + 后台任务句柄 + 停止信号。
    pub screencast: Arc<ScreencastShared>,
    /// 下载跟踪共享状态(`tab.downloads()`:基线 + 已返回集合 + 是否在跟踪;文件系统轮询)。
    pub downloads: Arc<DownloadShared>,
    /// 下载目录(来自 [`BrowserOptions::download_path`]);`wait_download` 据此拼最终文件路径。
    pub download_path: Option<std::path::PathBuf>,
    /// 事件泵任务的 abort 句柄。最后一个 `Tab` 句柄析构时 abort,断开事件泵对 [`Connection`] 的强引用。
    /// 否则事件泵会"自我维持"(持 conn → Inner 存活 → 事件通道不关闭 → 泵永不退出),
    /// 导致 attach(ws)模式下 drop 浏览器后底层 ws 永不关闭、服务端单客户端槽位无法释放。
    pub pump_abort: tokio::task::AbortHandle,
}

impl Drop for TabCore {
    /// 最后一个 `Tab` 句柄析构时:abort 事件泵,解开它与 [`Connection`] 的循环引用。
    /// 这样 attach 模式 drop 浏览器即可让 ws 自然关闭;launch 模式下浏览器整体退出,abort 亦无副作用。
    fn drop(&mut self) {
        self.pump_abort.abort();
    }
}

/// 文件选择器拦截"自然上传"的共享状态(`arm_upload` 写入待上传文件,`wait_upload` 读取后填入)。
#[derive(Default)]
pub(crate) struct UploadShared {
    /// 待上传文件的绝对路径列表;由 [`TabCore::arm_upload`] 写入、[`TabCore::wait_upload`] 读取。
    pub files: Mutex<Option<Vec<String>>>,
}

impl TabCore {
    /// 当前默认操作超时。
    pub(crate) fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms.load(Ordering::Relaxed))
    }

    /// 运行时设置默认操作超时。
    pub(crate) fn set_timeout(&self, d: Duration) {
        self.timeout_ms
            .store(d.as_millis() as u64, Ordering::Relaxed);
    }

    /// 当前默认加载模式。
    pub(crate) fn current_load_mode(&self) -> LoadMode {
        LoadMode::from_u8(self.load_mode.load(Ordering::Relaxed))
    }

    /// 运行时设置默认加载模式。
    pub(crate) fn set_load_mode(&self, m: LoadMode) {
        self.load_mode.store(m.as_u8(), Ordering::Relaxed);
    }

    /// 轮询直到 `document.readyState === 'complete'`;超时返回 `false`。
    pub(crate) async fn poll_ready_complete(&self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            let rs = self
                .evaluate("document.readyState", true)
                .await
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            if rs == "complete" {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// 武装"自然上传":记下待上传文件,并在主文档注入一个**一次性 hook**——它在捕获阶段拦截
    /// `<input type=file>` 的 click,`preventDefault` 掉原生文件选择框,并把被点的 input 记到
    /// `window.__drission_upload_el`,随后由 [`wait_upload`](Self::wait_upload) 用 `setFileInputFiles` 填入。
    ///
    /// **必须在触发文件框的点击之前调用。** 之所以走 JS hook 而非 Juggler 原生 `fileChooserOpened`:
    /// 实测当前 Camoufox(headless)下原生 `setInterceptFileChooserDialog`+`fileChooserOpened` 不触发
    /// (跨进程/无头),hook 方案对"直接点 input"和"点按钮→`input.click()`"两种写法都稳定可用。
    pub(crate) async fn arm_upload(&self, files: Vec<String>) -> Result<()> {
        *self.upload.files.lock().await = Some(files);
        self.evaluate(UPLOAD_HOOK_JS, true).await?;
        Ok(())
    }

    /// 等待"自然上传"的文件被填入:轮询 `window.__drission_upload_el`,一旦捕获到被点的 file input
    /// 就用 `setFileInputFiles` 把武装时记下的文件填进去。超时返回 `false`(不报错)。
    pub(crate) async fn wait_upload(&self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            let captured = self
                .evaluate("window.__drission_upload_el || null", false)
                .await?;
            if let Some(oid) = captured.get("objectId").and_then(|v| v.as_str()) {
                let files = self.upload.files.lock().await.clone().unwrap_or_default();
                self.send_page(
                    "Page.setFileInputFiles",
                    json!({ "frameId": self.main_frame_id, "objectId": oid, "files": files }),
                )
                .await?;
                // 清掉标记,避免下次 wait 误命中同一个元素。
                let _ = self
                    .evaluate("window.__drission_upload_el = null", true)
                    .await;
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// 低层:向视口坐标 `(x,y)` 派发一个鼠标事件(左键)。`buttons`:0=未按下,1=左键按住。
    /// `mousedown`/`mouseup` 自动带 `clickCount=1`,`mousemove` 为 0。
    pub(crate) async fn dispatch_mouse(
        &self,
        ty: &str,
        x: f64,
        y: f64,
        buttons: i64,
    ) -> Result<()> {
        let click_count = i64::from(ty == "mousedown" || ty == "mouseup");
        self.dispatch_mouse_ex(ty, x, y, 0, buttons, click_count)
            .await
    }

    /// 低层:派发鼠标事件(完整参数)。`button`:0=左/1=中/2=右;`buttons`:当前按下位掩码
    /// (左=1/右=2/中=4);`click_count`:点击计数(双击=2)。供 [`Actions`](crate::browser::Actions) 用。
    pub(crate) async fn dispatch_mouse_ex(
        &self,
        ty: &str,
        x: f64,
        y: f64,
        button: i64,
        buttons: i64,
        click_count: i64,
    ) -> Result<()> {
        self.send_page(
            "Page.dispatchMouseEvent",
            json!({
                "type": ty,
                "button": button,
                "buttons": buttons,
                "x": x,
                "y": y,
                "modifiers": 0,
                "clickCount": click_count,
            }),
        )
        .await?;
        Ok(())
    }

    /// 低层:**不等待响应**地派发一个鼠标事件(用于拟人轨迹的密集移动)。
    ///
    /// 与 [`dispatch_mouse`](Self::dispatch_mouse) 的差别仅在**不做往返等待**——把"事件间隔"
    /// 完全交给调用方的 `sleep` 控制,从而能达到真人级 ~10ms 采样(`send` 路径受单次往返 ~20ms
    /// 限制,轨迹会偏稀疏且节奏规整,是滑块风控的破绽)。仅适合 move/down/up 这类无需返回值的输入。
    pub(crate) fn dispatch_mouse_fire(&self, ty: &str, x: f64, y: f64, buttons: i64) -> Result<()> {
        let click_count = i64::from(ty == "mousedown" || ty == "mouseup");
        self.conn.fire_session(
            "Page.dispatchMouseEvent",
            json!({
                "type": ty,
                "button": 0,
                "buttons": buttons,
                "x": x,
                "y": y,
                "modifiers": 0,
                "clickCount": click_count,
            }),
            Some(&self.session_id),
        )
    }

    /// 取指定 frame 的主世界 executionContextId。主帧走 watch(快);子帧轮询 `frame_ctxs`。
    pub(crate) async fn exec_ctx_for_frame(&self, frame_id: &str) -> Result<String> {
        if frame_id == self.main_frame_id {
            return self.exec_ctx_id().await;
        }
        let timeout = self.timeout();
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(ctx) = self.frame_ctxs.lock().await.get(frame_id).cloned() {
                return Ok(ctx);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(timeout));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// 把一个"节点数组"RemoteObject(其 `objectId`)展开为其中各节点的 `objectId` 列表。
    ///
    /// 在 `frame_id` 的执行上下文里 `Runtime.getObjectProperties` 枚举,只取**数字下标**且带
    /// `objectId` 的项(即数组里的节点元素)。供 [`Tab::eles`]/[`Frame::eles`](crate::browser::Frame::eles)/
    /// 元素相对定位 / shadow 查找共用,避免各处重复展开逻辑。
    pub(crate) async fn node_array_object_ids(
        &self,
        frame_id: &str,
        array_object_id: &str,
    ) -> Result<Vec<String>> {
        let ctx = self.exec_ctx_for_frame(frame_id).await?;
        let props = self
            .send_page(
                "Runtime.getObjectProperties",
                json!({ "executionContextId": ctx, "objectId": array_object_id }),
            )
            .await?;
        let mut out = Vec::new();
        if let Some(list) = props["properties"].as_array() {
            for p in list {
                if p["name"].as_str().map(is_index).unwrap_or(false)
                    && let Some(oid) = p["value"]["objectId"].as_str()
                {
                    out.push(oid.to_string());
                }
            }
        }
        Ok(out)
    }

    /// 在指定 frame 的执行上下文里 `Runtime.evaluate`。
    pub(crate) async fn evaluate_in(
        &self,
        frame_id: &str,
        expression: &str,
        return_by_value: bool,
    ) -> Result<Value> {
        let mut base = serde_json::Map::new();
        base.insert("expression".into(), json!(expression));
        base.insert("returnByValue".into(), json!(return_by_value));
        self.runtime_call_in(frame_id, "Runtime.evaluate", base)
            .await
    }

    /// 在指定 frame 的执行上下文里 `Runtime.callFunction`。
    pub(crate) async fn call_function_in(
        &self,
        frame_id: &str,
        declaration: &str,
        args: Vec<Value>,
        return_by_value: bool,
    ) -> Result<Value> {
        let mut base = serde_json::Map::new();
        base.insert("functionDeclaration".into(), json!(declaration));
        base.insert("args".into(), json!(args));
        base.insert("returnByValue".into(), json!(return_by_value));
        self.runtime_call_in(frame_id, "Runtime.callFunction", base)
            .await
    }

    /// 指定 frame 的 Runtime 调用 + 上下文失效自动重试(子帧用 `frame_ctxs`,主帧复用 watch)。
    async fn runtime_call_in(
        &self,
        frame_id: &str,
        method: &str,
        base: serde_json::Map<String, Value>,
    ) -> Result<Value> {
        for _ in 0..4 {
            let ctx = self.exec_ctx_for_frame(frame_id).await?;
            let mut params = base.clone();
            params.insert("executionContextId".into(), json!(ctx));
            match self.send_page(method, Value::Object(params)).await {
                Ok(r) => return extract_runtime_result(r),
                Err(Error::Protocol(msg)) if is_stale_context(&msg) => {
                    // 该 frame 上下文可能已重建:清掉旧缓存,稍候重试。
                    if frame_id != self.main_frame_id {
                        self.frame_ctxs.lock().await.remove(frame_id);
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        Err(Error::Other("多次重试后仍无有效执行上下文".into()))
    }

    /// 取当前主世界 executionContextId;若暂时为 None(如刚导航完),等待其就绪。
    pub(crate) async fn exec_ctx_id(&self) -> Result<String> {
        if let Some(id) = self.exec_ctx.borrow().clone() {
            return Ok(id);
        }
        let mut rx = self.exec_ctx.clone();
        let timeout = self.timeout();
        let deadline = Instant::now() + timeout;
        loop {
            if timeout_at(deadline, rx.changed()).await.is_err() {
                return Err(Error::Timeout(timeout));
            }
            if let Some(id) = rx.borrow().clone() {
                return Ok(id);
            }
        }
    }

    /// 在 page 会话上发一个请求。
    pub(crate) async fn send_page(&self, method: &str, params: Value) -> Result<Value> {
        self.conn.send(method, params, Some(&self.session_id)).await
    }

    /// 截图底层:对给定裁剪区 `clip` 截图,按 `format`(+ JPEG `quality`)返回图片字节。
    /// `Page.screenshot` 的 `clip` 必填(`{x, y, width, height}`,页面坐标)。
    pub(crate) async fn capture(
        &self,
        clip: Value,
        format: ImageFormat,
        quality: Option<u8>,
    ) -> Result<Vec<u8>> {
        let mut params = json!({ "mimeType": format.mime(), "clip": clip });
        if let (ImageFormat::Jpeg, Some(q)) = (format, quality) {
            params["quality"] = json!(q.min(100));
        }
        let r = self.send_page("Page.screenshot", params).await?;
        let data = r["data"]
            .as_str()
            .ok_or_else(|| Error::Protocol("screenshot 未返回 data".into()))?;
        base64_decode(data).ok_or_else(|| Error::Protocol("screenshot data 非法 base64".into()))
    }

    /// 计算整页 / 视口截图的裁剪区(`{x, y, width, height}`,页面坐标)。
    pub(crate) async fn page_clip(&self, full_page: bool) -> Result<Value> {
        let js = if full_page {
            "(() => { const d = document.documentElement, b = document.body; \
             const w = Math.max(d.scrollWidth, b ? b.scrollWidth : 0, d.clientWidth); \
             const h = Math.max(d.scrollHeight, b ? b.scrollHeight : 0, d.clientHeight); \
             return [0, 0, w, h]; })()"
        } else {
            "[window.scrollX, window.scrollY, window.innerWidth, window.innerHeight]"
        };
        let v = self.evaluate(js, true).await?;
        let x = v.get(0).and_then(Value::as_f64).unwrap_or(0.0);
        let y = v.get(1).and_then(Value::as_f64).unwrap_or(0.0);
        let w = v.get(2).and_then(Value::as_f64).unwrap_or(0.0).max(1.0);
        let h = v.get(3).and_then(Value::as_f64).unwrap_or(0.0).max(1.0);
        Ok(json!({ "x": x, "y": y, "width": w, "height": h }))
    }

    /// 设置内容视口大小(`Page.setViewportSize`)。**有头**会连带把浏览器窗口缩放到该尺寸,
    /// **无头**仅设视口。这是 Camoufox/Firefox(Juggler)下唯一可靠的"窗口尺寸"手段。
    pub(crate) async fn set_viewport_size(&self, width: u32, height: u32) -> Result<()> {
        self.send_page(
            "Page.setViewportSize",
            json!({ "viewportSize": { "width": width.max(1), "height": height.max(1) } }),
        )
        .await?;
        Ok(())
    }

    /// 按下并释放单个键(供 [`Tab::press_key`] 与 [`Element::input_keys`] 复用)。
    pub(crate) async fn press_key(&self, key: &str) -> Result<()> {
        let (norm, code, key_code, text) = key_descriptor(key);
        self.send_page(
            "Page.dispatchKeyEvent",
            json!({
                "type": "keydown", "key": norm, "code": code,
                "keyCode": key_code, "location": 0, "repeat": false, "text": text,
            }),
        )
        .await?;
        self.send_page(
            "Page.dispatchKeyEvent",
            json!({
                "type": "keyup", "key": norm, "code": code,
                "keyCode": key_code, "location": 0, "repeat": false,
            }),
        )
        .await?;
        Ok(())
    }

    /// 把 listen hook(若有)与所有通用 init 脚本合并,一次性下发为页面 init scripts。
    pub(crate) async fn rebuild_init_scripts(&self) -> Result<()> {
        let mut scripts: Vec<Value> = Vec::new();
        if let Some(s) = self.listen_script.lock().await.as_ref() {
            scripts.push(json!({ "script": s }));
        }
        for s in self.init_scripts.lock().await.iter() {
            scripts.push(json!({ "script": s }));
        }
        self.send_page("Page.setInitScripts", json!({ "scripts": scripts }))
            .await?;
        Ok(())
    }

    /// `Runtime.evaluate`,返回结果 RemoteObject(可能含 objectId 或 value)。
    pub(crate) async fn evaluate(&self, expression: &str, return_by_value: bool) -> Result<Value> {
        let mut base = serde_json::Map::new();
        base.insert("expression".into(), json!(expression));
        base.insert("returnByValue".into(), json!(return_by_value));
        self.runtime_call("Runtime.evaluate", base).await
    }

    /// `Runtime.callFunction`,在给定参数上调用函数声明。
    pub(crate) async fn call_function(
        &self,
        declaration: &str,
        args: Vec<Value>,
        return_by_value: bool,
    ) -> Result<Value> {
        let mut base = serde_json::Map::new();
        base.insert("functionDeclaration".into(), json!(declaration));
        base.insert("args".into(), json!(args));
        base.insert("returnByValue".into(), json!(return_by_value));
        self.runtime_call("Runtime.callFunction", base).await
    }

    /// 统一的 Runtime 调用:注入当前 executionContextId,并在"上下文失效"时
    /// 等待新上下文后自动重试(导航瞬间 / 初始上下文重建常见)。
    async fn runtime_call(
        &self,
        method: &str,
        base: serde_json::Map<String, Value>,
    ) -> Result<Value> {
        for _ in 0..4 {
            let ctx = self.exec_ctx_id().await?;
            let mut params = base.clone();
            params.insert("executionContextId".into(), json!(ctx));
            match self.send_page(method, Value::Object(params)).await {
                Ok(r) => return extract_runtime_result(r),
                Err(Error::Protocol(msg)) if is_stale_context(&msg) => {
                    tracing::debug!("执行上下文失效,等待新上下文后重试");
                    let mut rx = self.exec_ctx.clone();
                    let _ = tokio::time::timeout(Duration::from_secs(3), rx.changed()).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        Err(Error::Other("多次重试后仍无有效执行上下文".into()))
    }
}

/// 判断协议错误是否为"执行上下文失效"。
fn is_stale_context(msg: &str) -> bool {
    msg.contains("execution context") || msg.contains("findExecutionContext")
}

/// 把字节写入文件,父目录不存在则自动创建。
pub(crate) async fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, bytes).await?;
    Ok(())
}

/// 一个浏览器标签页句柄。克隆代价低(共享内核)。
#[derive(Clone)]
pub struct Tab {
    pub(crate) core: Arc<TabCore>,
}

impl Tab {
    /// 新建一个独立 BrowserContext + page,并完成会话/主帧/执行上下文的绑定。
    pub(crate) async fn open(conn: Connection, opts: &BrowserOptions) -> Result<Self> {
        let timeout = opts.launch_timeout.min(Duration::from_secs(60));
        // 在创建页面之前就订阅事件,避免漏掉 attach/frame/ctx 事件。
        let mut events = conn.subscribe();

        let r = conn
            .send(
                "Browser.createBrowserContext",
                json!({ "removeOnDetach": true }),
                None,
            )
            .await?;
        let browser_context_id = r["browserContextId"]
            .as_str()
            .ok_or_else(|| Error::Protocol("createBrowserContext 未返回 browserContextId".into()))?
            .to_string();

        apply_context_overrides(&conn, &browser_context_id, opts).await;

        let r = conn
            .send(
                "Browser.newPage",
                json!({ "browserContextId": browser_context_id }),
                None,
            )
            .await?;
        let target_id = r["targetId"]
            .as_str()
            .ok_or_else(|| Error::Protocol("newPage 未返回 targetId".into()))?
            .to_string();

        let session_id = wait_attached(&mut events, &target_id, timeout).await?;
        let (main_frame_id, exec_ctx_id) =
            wait_frame_and_ctx(&mut events, &session_id, timeout).await?;

        let frame_ctxs: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
        frame_ctxs
            .lock()
            .await
            .insert(main_frame_id.clone(), exec_ctx_id.clone());
        let (ctx_tx, ctx_rx) = watch::channel(Some(exec_ctx_id));
        let interceptor: Arc<Mutex<Option<InterceptorState>>> = Arc::new(Mutex::new(None));

        // 启动事件泵:**复用同一个订阅**(不重新 subscribe,避免引导与泵之间丢事件,
        // 例如初始 about:blank 执行上下文被重建的 destroy/create)。
        let pump_handle = tokio::spawn(pump(
            events,
            conn.clone(),
            session_id.clone(),
            main_frame_id.clone(),
            ctx_tx,
            frame_ctxs.clone(),
            interceptor.clone(),
        ));
        let pump_abort = pump_handle.abort_handle();

        let core = Arc::new(TabCore {
            conn,
            session_id,
            target_id,
            browser_context_id,
            main_frame_id,
            exec_ctx: ctx_rx,
            frame_ctxs,
            listen_active: Mutex::new(false),
            listen_buf: Mutex::new(VecDeque::new()),
            bg_active: AtomicBool::new(false),
            listen_bg: Mutex::new(None),
            listen_script: Mutex::new(None),
            init_scripts: Mutex::new(Vec::new()),
            interceptor,
            intercept_rx: Mutex::new(None),
            console: Arc::new(ConsoleShared::new()),
            console_task: Mutex::new(None),
            ws: Arc::new(WsShared::new()),
            ws_task: Mutex::new(None),
            upload: Arc::new(UploadShared::default()),
            timeout_ms: AtomicU64::new(30_000),
            load_mode: AtomicU8::new(LoadMode::Normal.as_u8()),
            last_load_ok: AtomicBool::new(true),
            screencast: Arc::new(ScreencastShared::new()),
            downloads: Arc::new(DownloadShared::new()),
            download_path: opts.download_path.clone(),
            pump_abort,
        });

        Ok(Tab { core })
    }

    /// 从一个**已 attach 的 page**(如点击打开的弹窗 / 新标签)装配出 `Tab`。
    ///
    /// 复用调用方已有的事件订阅 `events`(里面应已含/即将含该 page 的 frame/ctx 事件),
    /// 等到主帧与执行上下文后建内核 + 启动事件泵。`browserContextId` 沿用打开者所在上下文
    /// (弹窗与打开者同上下文,故指纹/代理/cookie 与打开者一致)。
    /// 弹窗 / 点击打开新标签的装配入口(由 [`Wait::new_tab`](crate::browser::Wait::new_tab) 接线)。
    pub(crate) async fn from_attached(
        conn: Connection,
        target_id: String,
        browser_context_id: String,
        session_id: String,
        mut events: tokio::sync::broadcast::Receiver<Event>,
        download_path: Option<std::path::PathBuf>,
        timeout: Duration,
    ) -> Result<Self> {
        let (main_frame_id, exec_ctx_id) =
            wait_frame_and_ctx(&mut events, &session_id, timeout).await?;

        let frame_ctxs: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
        frame_ctxs
            .lock()
            .await
            .insert(main_frame_id.clone(), exec_ctx_id.clone());
        let (ctx_tx, ctx_rx) = watch::channel(Some(exec_ctx_id));
        let interceptor: Arc<Mutex<Option<InterceptorState>>> = Arc::new(Mutex::new(None));

        let pump_handle = tokio::spawn(pump(
            events,
            conn.clone(),
            session_id.clone(),
            main_frame_id.clone(),
            ctx_tx,
            frame_ctxs.clone(),
            interceptor.clone(),
        ));
        let pump_abort = pump_handle.abort_handle();

        let core = Arc::new(TabCore {
            conn,
            session_id,
            target_id,
            browser_context_id,
            main_frame_id,
            exec_ctx: ctx_rx,
            frame_ctxs,
            listen_active: Mutex::new(false),
            listen_buf: Mutex::new(VecDeque::new()),
            bg_active: AtomicBool::new(false),
            listen_bg: Mutex::new(None),
            listen_script: Mutex::new(None),
            init_scripts: Mutex::new(Vec::new()),
            interceptor,
            intercept_rx: Mutex::new(None),
            console: Arc::new(ConsoleShared::new()),
            console_task: Mutex::new(None),
            ws: Arc::new(WsShared::new()),
            ws_task: Mutex::new(None),
            upload: Arc::new(UploadShared::default()),
            timeout_ms: AtomicU64::new(30_000),
            load_mode: AtomicU8::new(LoadMode::Normal.as_u8()),
            last_load_ok: AtomicBool::new(true),
            screencast: Arc::new(ScreencastShared::new()),
            downloads: Arc::new(DownloadShared::new()),
            download_path,
            pump_abort,
        });

        Ok(Tab { core })
    }

    /// 该标签的 page 会话 id。
    pub fn session_id(&self) -> &str {
        &self.core.session_id
    }

    /// 该标签独立 BrowserContext 的 id。
    pub fn browser_context_id(&self) -> &str {
        &self.core.browser_context_id
    }

    /// 该标签对应的 targetId。
    pub fn target_id(&self) -> &str {
        &self.core.target_id
    }

    /// 访问网址。返回是否加载成功(DP 语义:`bool`)。
    ///
    /// 默认按标签当前加载模式(默认 [`LoadMode::Normal`],等 `load`)等待;失败不报错,返回 `false`。
    /// 需要重试 / 自定义超时 / 加载模式时用 [`get_with`](Self::get_with)。
    pub async fn get(&self, url: &str) -> Result<bool> {
        self.get_with(url, &GetOptions::default()).await
    }

    /// 访问网址(带选项:重试 / 间隔 / 超时 / 加载模式 / referer)。返回是否加载成功。
    pub async fn get_with(&self, url: &str, opts: &GetOptions) -> Result<bool> {
        let timeout = opts.timeout.unwrap_or_else(|| self.core.timeout());
        let mode = opts
            .load_mode
            .unwrap_or_else(|| self.core.current_load_mode());
        let attempts = opts.retry.saturating_add(1);
        let mut ok = false;
        for attempt in 0..attempts {
            match self
                .navigate_once(url, mode, timeout, opts.referer.as_deref())
                .await
            {
                Ok(true) => {
                    ok = true;
                    break;
                }
                Ok(false) => {
                    tracing::warn!(attempt, %url, "导航未达到目标加载状态");
                }
                Err(e) => {
                    tracing::warn!(attempt, %url, error = %e, "导航出错");
                }
            }
            if attempt + 1 < attempts {
                tokio::time::sleep(opts.interval).await;
            }
        }
        self.core.last_load_ok.store(ok, Ordering::Relaxed);
        Ok(ok)
    }

    /// 单次导航:下发 `Page.navigate` 并按加载模式等待。返回是否达到目标加载状态。
    async fn navigate_once(
        &self,
        url: &str,
        mode: LoadMode,
        timeout: Duration,
        referer: Option<&str>,
    ) -> Result<bool> {
        let mut events = self.core.conn.subscribe();
        let mut params = json!({ "frameId": self.core.main_frame_id, "url": url });
        if let Some(r) = referer {
            params["referer"] = json!(r);
        }
        self.core.send_page("Page.navigate", params).await?;

        let Some(target) = mode.event_name() else {
            return Ok(true); // LoadMode::None:不等待
        };

        let deadline = Instant::now() + timeout;
        loop {
            match timeout_at(deadline, events.recv()).await {
                Ok(Ok(ev)) => {
                    if ev.session_id.as_deref() == Some(&self.core.session_id)
                        && ev.method == "Page.eventFired"
                        && ev.params["frameId"].as_str() == Some(&self.core.main_frame_id)
                        && ev.params["name"].as_str() == Some(target)
                    {
                        return Ok(true);
                    }
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(_)) => return Err(Error::Transport("连接已关闭".into())),
                Err(_) => {
                    // 等事件超时:退而求其次看 readyState 是否已达标(避免漏掉早于订阅的事件)。
                    let rs = self.ready_state().await.unwrap_or_default();
                    let reached = match mode {
                        LoadMode::Normal => rs == "complete",
                        LoadMode::Eager => rs == "interactive" || rs == "complete",
                        LoadMode::None => true,
                    };
                    if !reached {
                        tracing::warn!(%url, ready_state = %rs, "等待加载超时");
                    }
                    return Ok(reached);
                }
            }
        }
    }

    /// 当前 URL。
    pub async fn url(&self) -> Result<String> {
        Ok(self
            .core
            .evaluate("location.href", true)
            .await?
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    /// 页面标题。
    pub async fn title(&self) -> Result<String> {
        Ok(self
            .core
            .evaluate("document.title", true)
            .await?
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    /// 当前 DOM 的 HTML。
    pub async fn html(&self) -> Result<String> {
        Ok(self
            .core
            .evaluate("document.documentElement.outerHTML", true)
            .await?
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    /// 执行 JS 表达式,返回其值(JSON)。
    pub async fn run_js(&self, script: &str) -> Result<Value> {
        self.core.evaluate(script, true).await
    }

    /// **DOM 派生**的无障碍快照(注入 [`AX_SNAPSHOT_JS`](crate::a11y::AX_SNAPSHOT_JS) 按 ARIA 规则算):
    /// 把页面压成 `role "name"` 语义树,用于抗改版断言或喂 LLM。与 CDP 后端 `ax_snapshot()` 同实现、
    /// 结果一致(Camoufox 无 CDP 的 `Accessibility` 域,故不提供 `ax_tree()` 原生版)。
    pub async fn ax_snapshot(&self) -> Result<crate::a11y::AxTree> {
        let v = self
            .core
            .evaluate(crate::a11y::AX_SNAPSHOT_JS, true)
            .await?;
        Ok(crate::a11y::build_from_snapshot(&v))
    }

    /// 直接把页面文档内容设为 `html`(对标 Playwright/Puppeteer `set_content`;Juggler 无原生命令,走 JS)。
    pub async fn set_content(&self, html: &str) -> Result<()> {
        let lit = serde_json::to_string(html).unwrap_or_else(|_| "\"\"".into());
        let js =
            format!("(function(h){{document.open();document.write(h);document.close();}})({lit})");
        self.core.evaluate(&js, true).await?;
        Ok(())
    }

    /// 当前页面的 `navigator.userAgent`。
    pub async fn user_agent(&self) -> Result<String> {
        Ok(self
            .core
            .evaluate("navigator.userAgent", true)
            .await?
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    /// 当前 `document.readyState`(`loading`/`interactive`/`complete`)。
    pub async fn ready_state(&self) -> Result<String> {
        Ok(self
            .core
            .evaluate("document.readyState", true)
            .await?
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    /// 最近一次 [`get`](Self::get) 是否成功(DP 的 `url_available`)。同步读取,不发协议请求。
    pub fn url_available(&self) -> bool {
        self.core.last_load_ok.load(Ordering::Relaxed)
    }

    /// 停止当前页面加载(等价 JS `window.stop()`;Juggler 无独立 stopLoading 方法)。
    pub async fn stop_loading(&self) -> Result<()> {
        let _ = self.core.evaluate("window.stop()", true).await;
        Ok(())
    }

    /// 等待文档加载完成(`readyState === 'complete'`)。超时返回 `false`(不报错)。
    pub async fn wait_loaded(&self) -> Result<bool> {
        self.core.poll_ready_complete(self.core.timeout()).await
    }

    /// 等待并处理**下一个** JS 对话框(alert/confirm/prompt/beforeunload)。
    ///
    /// `accept`:确定/取消;`prompt_text`:`prompt` 时填入的文本。返回该对话框信息。
    /// 若 JS(如 `confirm()`)会阻塞页面,请与触发动作**并发**调用(见 `examples/page_extras`)。
    pub async fn handle_next_dialog(
        &self,
        accept: bool,
        prompt_text: Option<&str>,
    ) -> Result<DialogInfo> {
        let mut events = self.core.conn.subscribe();
        let timeout = self.core.timeout();
        let deadline = Instant::now() + timeout;
        loop {
            match timeout_at(deadline, events.recv()).await {
                Ok(Ok(ev)) => {
                    if ev.session_id.as_deref() == Some(&self.core.session_id)
                        && ev.method == "Page.dialogOpened"
                    {
                        let dialog_id = ev.params["dialogId"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        let info = DialogInfo {
                            dialog_type: ev.params["type"].as_str().unwrap_or_default().to_string(),
                            message: ev.params["message"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            default_value: ev.params["defaultValue"].as_str().map(str::to_string),
                        };
                        let mut p = json!({ "dialogId": dialog_id, "accept": accept });
                        if let Some(t) = prompt_text {
                            p["promptText"] = json!(t);
                        }
                        self.core.send_page("Page.handleDialog", p).await?;
                        return Ok(info);
                    }
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(_)) => return Err(Error::Transport("连接已关闭".into())),
                Err(_) => return Err(Error::Timeout(timeout)),
            }
        }
    }

    /// 设置"自然上传"要用的文件并**武装文件选择器拦截**(对应 DP `tab.set.upload_files`)。
    ///
    /// 调用后,**下一次**任何会弹出文件选择框的点击都不再弹原生窗,而是把这些文件直接填进去。
    /// 典型用法:`set_upload_files` → 点击触发按钮 → [`wait_upload_paths_inputted`]。
    /// `paths` 为本地文件**绝对路径**(多文件需被触发的 `<input>` 带 `multiple`)。
    ///
    /// 与 [`Element::set_files`] 的区别:`set_files` 直接对已知的 `<input type=file>` 赋值;本方法处理
    /// "点按钮→系统文件框"这种**没有可直接定位的 input**(或 input 隐藏、由 JS `input.click()` 唤起)的场景。
    ///
    /// [`wait_upload_paths_inputted`]: Self::wait_upload_paths_inputted
    pub async fn set_upload_files(&self, paths: &[&str]) -> Result<()> {
        if paths.is_empty() {
            return Err(Error::Other("set_upload_files: 文件列表为空".into()));
        }
        let files = paths.iter().map(|p| p.to_string()).collect();
        self.core.arm_upload(files).await
    }

    /// 等待"自然上传"的文件路径被填入(对应 DP `tab.wait.upload_paths_inputted`)。
    ///
    /// 配合 [`set_upload_files`](Self::set_upload_files) + 点击触发使用。`timeout=None` 用默认超时;
    /// 超时返回 `false`(不报错)。
    pub async fn wait_upload_paths_inputted(&self, timeout: Option<Duration>) -> Result<bool> {
        let d = timeout.unwrap_or_else(|| self.core.timeout());
        self.core.wait_upload(d).await
    }

    /// 重新加载。
    pub async fn reload(&self) -> Result<()> {
        self.core.send_page("Page.reload", json!({})).await?;
        Ok(())
    }

    /// 后退。
    pub async fn back(&self) -> Result<()> {
        self.core
            .send_page("Page.goBack", json!({ "frameId": self.core.main_frame_id }))
            .await?;
        Ok(())
    }

    /// 前进。
    pub async fn forward(&self) -> Result<()> {
        self.core
            .send_page(
                "Page.goForward",
                json!({ "frameId": self.core.main_frame_id }),
            )
            .await?;
        Ok(())
    }

    /// 查找单个元素(DP 定位语法)。在超时内轮询等待出现。
    pub async fn ele(&self, selector: &str) -> Result<Element> {
        let deadline = Instant::now() + self.core.timeout();
        loop {
            if let Some(el) = self.find_once(selector).await? {
                return Ok(el);
            }
            if Instant::now() >= deadline {
                return Err(Error::ElementNotFound(selector.to_string()));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// 便捷:**找到并点击**(= `ele(selector).await?.click().await`)。把最常见的「定位 + 点击」合一,
    /// 少写一次 `.await?`。`ele` 自带超时内等待,故元素稍后才出现也能点到。
    pub async fn click(&self, selector: &str) -> Result<()> {
        self.ele(selector).await?.click().await
    }

    /// 便捷:**找到并输入**(= `ele(selector).await?.input(text).await`)。
    /// 需要逐字符拟人输入用 `ele(selector).await?.input_human(text)`;输入前清空用 `ele().clear()`。
    pub async fn input(&self, selector: &str, text: &str) -> Result<()> {
        self.ele(selector).await?.input(text).await
    }

    /// 元素是否存在(**立即判定、不等待**)。要"等它出现"用 [`ele`](Self::ele) 或
    /// [`wait().ele_displayed`](crate::browser::Wait);要静态 HTML 判定用 [`s_ele`](Self::s_ele)。
    pub async fn exists(&self, selector: &str) -> Result<bool> {
        Ok(self.find_once(selector).await?.is_some())
    }

    /// 单次查找(不等待):命中返回 `Some(Element)`,否则 `None`。供 `ele` 与 [`Wait`] 复用。
    pub(crate) async fn find_once(&self, selector: &str) -> Result<Option<Element>> {
        let query = locator::parse(selector);
        let expr = single_query_expr(&query);
        let result = self.core.evaluate(&expr, false).await?;
        Ok(result
            .get("objectId")
            .and_then(|v| v.as_str())
            .map(|oid| Element::new(self.core.clone(), oid.to_string())))
    }

    /// 解析当前页面 HTML,返回**静态根元素**(离线解析,不再与浏览器通信)。
    pub async fn s_root(&self) -> Result<StaticElement> {
        let html = self.html().await?;
        StaticElement::parse(&html)
    }

    /// 解析当前页面 HTML,取第一个匹配的**静态元素**(DP `s_ele`)。
    ///
    /// 适合"抓到页面后批量离线解析"。CSS/`@attr`/`text:` 及 `xpath:`(内置子集)均支持。
    pub async fn s_ele(&self, selector: &str) -> Result<StaticElement> {
        let html = self.html().await?;
        StaticElement::parse(&html)?.ele(selector)
    }

    /// 解析当前页面 HTML,取全部匹配的**静态元素**(DP `s_eles`)。
    pub async fn s_eles(&self, selector: &str) -> Result<Vec<StaticElement>> {
        let html = self.html().await?;
        StaticElement::parse(&html)?.eles(selector)
    }

    /// 按定位语法找到一个 `<iframe>`/`<frame>` 元素,返回其内容 [`Frame`](DP `tab.get_frame`)。
    pub async fn get_frame(&self, selector: &str) -> Result<Frame> {
        self.ele(selector).await?.content_frame().await
    }

    /// 查找多个元素(立即返回,不等待)。
    pub async fn eles(&self, selector: &str) -> Result<Vec<Element>> {
        let query = locator::parse(selector);
        let expr = multi_query_expr(&query);
        let result = self.core.evaluate(&expr, false).await?;
        let Some(array_object_id) = result.get("objectId").and_then(|v| v.as_str()) else {
            return Ok(Vec::new());
        };
        let oids = self
            .core
            .node_array_object_ids(&self.core.main_frame_id, array_object_id)
            .await?;
        Ok(oids
            .into_iter()
            .map(|oid| Element::new(self.core.clone(), oid))
            .collect())
    }

    /// **翻页采集**:对每一页调用 `f` 收集结果,然后点击 `next_selector` 翻到下一页,直到
    /// 下一页按钮不存在 / 不可点击,或达到 `max_pages`。返回每页结果(按页序)。
    ///
    /// `f(page_index)` 在**当前页**就绪后被调用(第 0 页即首屏)。点下一页后会等一小段让新页渲染;
    /// 若站点是 AJAX 局部刷新、需要更精确的"等新内容"逻辑,可在 `f` 内部自行 `wait` 后再抓。
    /// 闭包通常捕获一个 `tab.clone()` 在内部读元素(避免与本方法的 `&self` 借用冲突)。
    ///
    /// ```ignore
    /// let t = tab.clone();
    /// let pages = tab.paginate("css:.next:not(.disabled)", 10, move |i| {
    ///     let t = t.clone();
    ///     async move { t.ele("tag:table").await?.table().await }
    /// }).await?;
    /// ```
    pub async fn paginate<F, Fut, T>(
        &self,
        next_selector: &str,
        max_pages: usize,
        mut f: F,
    ) -> Result<Vec<T>>
    where
        F: FnMut(usize) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut out = Vec::new();
        for page in 0..max_pages.max(1) {
            out.push(f(page).await?);
            // 找下一页按钮:不存在或不可点击 → 到末页,结束。
            let next = match self.find_once(next_selector).await? {
                Some(e) if e.is_clickable().await.unwrap_or(false) => e,
                _ => break,
            };
            next.click().await?;
            // 等新页渲染(站点差异大,这里给一个通用的小等待;精确等待可在 f 内做)。
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
        Ok(out)
    }

    /// 读取本标签(其 BrowserContext)的所有 cookie。
    pub async fn cookies(&self) -> Result<Vec<Cookie>> {
        let r = self
            .core
            .conn
            .send(
                "Browser.getCookies",
                json!({ "browserContextId": self.core.browser_context_id }),
                None,
            )
            .await?;
        let mut out = Vec::new();
        if let Some(arr) = r["cookies"].as_array() {
            for c in arr {
                out.push(Cookie {
                    name: c["name"].as_str().unwrap_or_default().to_string(),
                    value: c["value"].as_str().unwrap_or_default().to_string(),
                    domain: c["domain"].as_str().unwrap_or_default().to_string(),
                    path: c["path"].as_str().unwrap_or_default().to_string(),
                    expires: c["expires"].as_f64().unwrap_or(-1.0),
                    http_only: c["httpOnly"].as_bool().unwrap_or(false),
                    secure: c["secure"].as_bool().unwrap_or(false),
                });
            }
        }
        Ok(out)
    }

    /// 为本标签(其 BrowserContext)设置 cookie。
    pub async fn set_cookies(&self, cookies: Vec<CookieParam>) -> Result<()> {
        let arr: Vec<Value> = cookies
            .into_iter()
            .map(|c| {
                let mut o = serde_json::Map::new();
                o.insert("name".into(), json!(c.name));
                o.insert("value".into(), json!(c.value));
                if let Some(v) = c.url {
                    o.insert("url".into(), json!(v));
                }
                if let Some(v) = c.domain {
                    o.insert("domain".into(), json!(v));
                }
                if let Some(v) = c.path {
                    o.insert("path".into(), json!(v));
                }
                if let Some(v) = c.secure {
                    o.insert("secure".into(), json!(v));
                }
                if let Some(v) = c.http_only {
                    o.insert("httpOnly".into(), json!(v));
                }
                if let Some(v) = c.expires {
                    o.insert("expires".into(), json!(v));
                }
                Value::Object(o)
            })
            .collect();
        self.core
            .conn
            .send(
                "Browser.setCookies",
                json!({ "browserContextId": self.core.browser_context_id, "cookies": arr }),
                None,
            )
            .await?;
        Ok(())
    }

    /// 等待**本页触发的一次下载**完成,返回 [`DownloadInfo`](含建议文件名与最终落盘路径)。
    /// 需先用 [`BrowserOptions::download_path`](crate::launcher::BrowserOptions::download_path) 设下载目录。
    ///
    /// **实现:文件系统跟踪**。实测当前 Camoufox/Juggler 构建**不下发** `Browser.download*` 事件,
    /// 但文件会可靠落盘(下载中写 `<name>.part`、完成后改名)。本方法在调用时记录目录基线,然后轮询
    /// 等待**新出现且写入稳定**的文件。因走文件系统,[`DownloadInfo::url`] 为空(文件名/路径可用)。
    /// 调用顺序:**先触发动作、再等**(基线在本方法内记录,故 `click` 在前也可):
    /// ```ignore
    /// link.click().await?;
    /// let info = tab.wait_download(Duration::from_secs(30)).await?;  // info.path 即下载到的文件
    /// ```
    /// 多文件 / 任务列表 / 进度 / 重命名请用句柄式 [`downloads()`](Self::downloads)。
    pub async fn wait_download(&self, timeout: Duration) -> Result<DownloadInfo> {
        use crate::browser::download::{
            DownloadState, list_dir_files, scan_new_files, size_stable,
        };
        let dir = self.core.download_path.clone().ok_or_else(|| {
            Error::Other("wait_download 需先用 BrowserOptions::download_path 设置下载目录".into())
        })?;
        let baseline = list_dir_files(&dir).await;
        let deadline = Instant::now() + timeout;
        loop {
            for m in scan_new_files(&dir, &baseline).await {
                if m.state == DownloadState::Finished && size_stable(&m.path).await {
                    return Ok(DownloadInfo {
                        url: String::new(),
                        suggested_filename: m.suggested_filename,
                        path: m.path,
                        success: true,
                        error: None,
                    });
                }
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(timeout));
            }
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
    }

    /// 开始网络监听。`keywords` 为 URL 子串过滤(空=全部)。
    ///
    /// 实现:在页面注入 fetch/XHR hook(当前文档 + 后续导航),匹配的请求连同**响应体**入队,
    /// 由 [`listen_wait`](Self::listen_wait)/[`listen_next`](Self::listen_next) 取回。
    /// 务必在 [`get`](Self::get) **之前**调用,以免漏掉早期请求。
    pub async fn listen_start(&self, keywords: &[&str]) -> Result<()> {
        self.listen_start_filter(ListenFilter {
            url_keywords: keywords.iter().map(|s| s.to_string()).collect(),
            xhr_only: false,
        })
        .await
    }

    /// 监听(关键词过滤)。hook 天然只覆盖 fetch/XHR,与 [`listen_start`](Self::listen_start) 等价。
    pub async fn listen_xhr(&self, keywords: &[&str]) -> Result<()> {
        self.listen_start(keywords).await
    }

    async fn listen_start_filter(&self, filter: ListenFilter) -> Result<()> {
        let script = hook_script(&filter);
        // 记录为 listen hook,与通用 init 脚本合并后作为 init script 注入(覆盖后续导航 / 子帧)。
        *self.core.listen_script.lock().await = Some(script.clone());
        self.core.rebuild_init_scripts().await?;
        // 当前已加载的文档:立即注入一次。
        let _ = self.core.evaluate(&script, true).await;
        *self.core.listen_active.lock().await = true;
        self.core.listen_buf.lock().await.clear();
        Ok(())
    }

    /// 从页面队列取回新包并入本地缓冲。
    ///
    /// 长监听开启(`bg_active`)时变为 no-op:由后台抽取任务独占 `DRAIN_JS`,避免并发瓜分队列。
    async fn drain_into_buffer(&self) -> Result<()> {
        if self.core.bg_active.load(Ordering::SeqCst) {
            return Ok(());
        }
        let v = self.core.evaluate(DRAIN_JS, true).await?;
        let mut buf = self.core.listen_buf.lock().await;
        for p in parse_packets(&v) {
            buf.push_back(p);
        }
        Ok(())
    }

    /// 等待下一个匹配的数据包(带默认超时)。
    pub async fn listen_wait(&self) -> Result<DataPacket> {
        self.listen_wait_timeout(self.core.timeout()).await
    }

    /// 等待下一个匹配的数据包(自定义超时)。供 [`listen_wait`](Self::listen_wait) 与
    /// [`ListenStream`] 复用。单次模式下会按需 drain;长监听模式下仅从缓冲 pop(后台任务负责 drain)。
    pub async fn listen_wait_timeout(&self, timeout: Duration) -> Result<DataPacket> {
        if !*self.core.listen_active.lock().await {
            return Err(Error::Other("尚未调用 listen_start".into()));
        }
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(p) = self.core.listen_buf.lock().await.pop_front() {
                return Ok(p);
            }
            let _ = self.drain_into_buffer().await;
            if let Some(p) = self.core.listen_buf.lock().await.pop_front() {
                return Ok(p);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(timeout));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// 取下一个数据包(带默认超时;超时返回 `None`)。
    pub async fn listen_next(&self) -> Result<Option<DataPacket>> {
        match self.listen_wait().await {
            Ok(p) => Ok(Some(p)),
            Err(Error::Timeout(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// 开启**长监听**:启动后台抽取任务,持续把页面队列搬进本地缓冲(不丢包)。
    ///
    /// 需先调用 [`listen_start`](Self::listen_start)/[`listen_xhr`](Self::listen_xhr) 安装 hook。
    /// 幂等:重复调用不会重复 spawn。与单次抓取共用同一缓冲,互不冲突。
    pub async fn listen_forever(&self) -> Result<()> {
        if !*self.core.listen_active.lock().await {
            return Err(Error::Other(
                "请先调用 listen_start/listen_xhr 再开启长监听".into(),
            ));
        }
        let mut bg = self.core.listen_bg.lock().await;
        if bg.is_some() {
            return Ok(());
        }
        self.core.bg_active.store(true, Ordering::SeqCst);
        *bg = Some(tokio::spawn(bg_drain_loop(self.core.clone())));
        Ok(())
    }

    /// 开启长监听并返回一个流式句柄,适合"边滑边持续抓"的场景。
    ///
    /// ```ignore
    /// tab.listen_xhr(&["aweme/v1/web/aweme/detail"]).await?;
    /// tab.get(url).await?;
    /// let stream = tab.listen_stream().await?;
    /// while let Some(p) = stream.next_timeout(Duration::from_secs(15)).await? {
    ///     // 处理 p ...
    ///     tab.press_key("ArrowDown").await?; // 滑到下一个视频
    /// }
    /// ```
    pub async fn listen_stream(&self) -> Result<ListenStream> {
        self.listen_forever().await?;
        Ok(ListenStream { tab: self.clone() })
    }

    /// 停止监听并清空状态(同时停掉长监听后台任务)。保留通过 [`add_init_script`](Self::add_init_script)
    /// 注入的通用脚本(只移除 listen 自己的 hook)。
    pub async fn listen_stop(&self) -> Result<()> {
        // 先置假,让后台循环在下一拍退出;再 abort 兜底。
        *self.core.listen_active.lock().await = false;
        self.core.bg_active.store(false, Ordering::SeqCst);
        if let Some(h) = self.core.listen_bg.lock().await.take() {
            h.abort();
        }
        // 移除 listen hook,但保留用户通过 add_init_script 注入的通用脚本。
        *self.core.listen_script.lock().await = None;
        let _ = self.core.rebuild_init_scripts().await;
        let _ = self.core.evaluate(UNINSTALL_JS, true).await;
        self.core.listen_buf.lock().await.clear();
        Ok(())
    }

    /// 添加一个【导航前注入】脚本:在每个新文档最早期(优先于页面自身脚本)执行,并立即在当前
    /// 文档执行一次。适合补环境 / 吐环境探针 / 反检测注入。与 listen 的 hook 共存(合并下发)。
    ///
    /// 务必在 [`get`](Self::get) **之前**调用,才能保证新文档里目标脚本运行前探针已就位。
    pub async fn add_init_script(&self, script: &str) -> Result<()> {
        self.core.init_scripts.lock().await.push(script.to_string());
        self.core.rebuild_init_scripts().await?;
        let _ = self.core.evaluate(script, true).await;
        Ok(())
    }

    /// 注入「输入指纹」反检测:把**合成 PointerEvent 的空 `pointerType`** 修补成 `"mouse"`。
    ///
    /// 背景:经 `Page.dispatchMouseEvent` 派发的鼠标事件虽是可信事件(`isTrusted=true`)、也能正确
    /// 衍生 `movementX/Y`、`screenX/Y`、`pressure`,但 Firefox/Juggler 从合成鼠标事件衍生的
    /// **PointerEvent 的 `pointerType` 为空串 `""`**(真实鼠标应为 `"mouse"`)——这是滑块/行为风控
    /// (如 GeeTest)用来识别自动化输入的一个破绽。本方法用一个**轻量 getter 补丁**把空值改回
    /// `"mouse"`,只改这一处、不动 `Function.prototype.toString`(不引入更大的可扫描面)。
    ///
    /// 必须在**导航前**调用(经 [`add_init_script`](Self::add_init_script) 在每个新文档脚本运行前生效)。
    pub async fn apply_pointer_stealth(&self) -> Result<()> {
        const POINTER_STEALTH_JS: &str = r#"(function(){
  try {
    var proto = window.PointerEvent && window.PointerEvent.prototype;
    if (!proto) return;
    var d = Object.getOwnPropertyDescriptor(proto, 'pointerType');
    if (!d || !d.get) return;
    var orig = d.get;
    Object.defineProperty(proto, 'pointerType', {
      configurable: true, enumerable: d.enumerable,
      get: function(){ var v = orig.call(this); return (v === '' || v == null) ? 'mouse' : v; }
    });
  } catch(e){}
})()"#;
        self.add_init_script(POINTER_STEALTH_JS).await
    }

    /// 通用吐环境入口:链式指定目标参数(query/header/cookie)与范围后 `start()` 注入探针。
    /// 详见 [`dump_env`](crate::browser::dump_env) 模块。
    ///
    /// ```ignore
    /// let mut probe = tab.dump_env().target_query("a_bogus")
    ///     .match_url("aweme/v1/web/aweme/detail").start().await?;
    /// tab.get(url).await?;
    /// let dump = probe.collect().await?;
    /// dump.write_to("./dump-env")?;
    /// ```
    pub fn dump_env(&self) -> crate::browser::dump_env::EnvDumper {
        crate::browser::dump_env::EnvDumper::new(self.clone())
    }

    /// 模拟按键(keydown+keyup)。内置常用导航键映射;抖音切下一个视频用 `press_key("ArrowDown")`。
    pub async fn press_key(&self, key: &str) -> Result<()> {
        self.core.press_key(key).await
    }

    /// 在视口中心派发鼠标滚轮(`deltaY>0` 向下)。抖音切视频的备选触发方式。
    pub async fn wheel(&self, delta_x: f64, delta_y: f64) -> Result<()> {
        let c = self
            .core
            .evaluate(
                "[Math.floor(innerWidth/2), Math.floor(innerHeight/2)]",
                true,
            )
            .await
            .unwrap_or(Value::Null);
        let x = c.get(0).and_then(Value::as_f64).unwrap_or(200.0);
        let y = c.get(1).and_then(Value::as_f64).unwrap_or(200.0);
        self.wheel_at(x, y, delta_x, delta_y).await
    }

    /// 在指定坐标派发鼠标滚轮。
    pub async fn wheel_at(&self, x: f64, y: f64, delta_x: f64, delta_y: f64) -> Result<()> {
        self.core
            .send_page(
                "Page.dispatchWheelEvent",
                json!({
                    "x": x.floor(), "y": y.floor(),
                    "deltaX": delta_x, "deltaY": delta_y, "deltaZ": 0, "modifiers": 0,
                }),
            )
            .await?;
        Ok(())
    }

    /// 页面级滚动(`window.scrollBy`)。普通页面通用;抖音 feed 不一定切视频。
    pub async fn scroll_by(&self, x: f64, y: f64) -> Result<()> {
        self.core
            .evaluate(&format!("window.scrollBy({x},{y})"), true)
            .await?;
        Ok(())
    }

    /// 低层鼠标:移动到视口坐标 `(x,y)`(左键未按下)。
    pub async fn mouse_move(&self, x: f64, y: f64) -> Result<()> {
        self.core.dispatch_mouse("mousemove", x, y, 0).await
    }

    /// 低层鼠标:在 `(x,y)` 按下左键。
    pub async fn mouse_down(&self, x: f64, y: f64) -> Result<()> {
        self.core.dispatch_mouse("mousedown", x, y, 1).await
    }

    /// 低层鼠标:**按住左键**移动到 `(x,y)`(拖拽过程中的 move,`buttons=1`)。
    pub async fn mouse_drag(&self, x: f64, y: f64) -> Result<()> {
        self.core.dispatch_mouse("mousemove", x, y, 1).await
    }

    /// 低层鼠标:在 `(x,y)` 松开左键。
    pub async fn mouse_up(&self, x: f64, y: f64) -> Result<()> {
        self.core.dispatch_mouse("mouseup", x, y, 0).await
    }

    /// 低层鼠标(**不等待往返**):移动到 `(x,y)`(左键未按下)。
    ///
    /// 节奏由调用方 `sleep` 控制,可达真人级 ~10ms 采样;适合拟人轨迹中**不需要读返回值**的密集移动。
    /// 需要边移边读位置(如滑块对齐纠偏)时,仍用会等待的 [`mouse_move`](Self::mouse_move) 等。
    pub fn mouse_move_fast(&self, x: f64, y: f64) -> Result<()> {
        self.core.dispatch_mouse_fire("mousemove", x, y, 0)
    }

    /// 低层鼠标(**不等待往返**):按住左键移动到 `(x,y)`(拖拽中的 move,`buttons=1`)。
    pub fn mouse_drag_fast(&self, x: f64, y: f64) -> Result<()> {
        self.core.dispatch_mouse_fire("mousemove", x, y, 1)
    }

    /// 等待相关操作的句柄(对应 DP `tab.wait`):`doc_loaded` / `ele_displayed` / `ele_loaded` / …。
    pub fn wait(&self) -> Wait {
        Wait::new(self.clone())
    }

    /// 滚动相关操作的句柄(对应 DP `tab.scroll`):`to_top` / `to_bottom` / `to_location` / …。
    pub fn scroll(&self) -> Scroll {
        Scroll::new(self.clone())
    }

    /// 设置相关操作的句柄(对应 DP `tab.set`):`cookies` / `timeout` / `load_mode` / `user_agent`。
    pub fn set(&self) -> SetTab {
        SetTab::new(self.clone())
    }

    /// 网络监听句柄(对应 DP `tab.listen`):`start` / `wait` / `wait_count` / `steps` / `stop`。
    ///
    /// 与扁平的 `listen_start`/`listen_wait`/`listen_stream`/`listen_stop` 等价(底层复用),
    /// 提供更贴近 DrissionPage 的句柄式写法。
    pub fn listen(&self) -> Listen {
        Listen::new(self.clone())
    }

    /// 控制台监听句柄(对应 DP `tab.console`):`start` / `wait` / `messages` / `steps` / `stop`。
    ///
    /// 基于 Juggler 原生 `Runtime.console` 事件(不 hook 页面 `console`,对反检测更友好)。
    /// `console.log()` 等输出到控制台的内容才能获取。
    pub fn console(&self) -> Console {
        Console::new(self.clone())
    }

    /// WebSocket 帧监听句柄(drission-rs 增强):`start` / `wait` / `wait_count` / `messages` /
    /// `sockets` / `steps` / `stop`。
    ///
    /// 基于 Juggler 原生 `Page.webSocket*` 事件(不 hook 页面 `WebSocket`,对反检测更友好):
    /// 抓取页面 WebSocket 收发的每一帧(文本 / 二进制)。务必在建立连接**之前** `start()`。
    ///
    /// ```ignore
    /// let ws = tab.websocket();
    /// ws.start_with(WsFilter::new().url_contains("/im/")).await?;
    /// tab.get(url).await?;
    /// while let Some(m) = ws.wait(Some(Duration::from_secs(10))).await? {
    ///     if m.is_text() { println!("{} {}", m.direction.as_str(), m.text().unwrap()); }
    /// }
    /// ```
    pub fn websocket(&self) -> WsListener {
        WsListener::new(self.clone())
    }

    /// 动作链(对应 DP `tab.actions` / Selenium ActionChains):把鼠标/键盘动作链式串起来,
    /// `perform().await` 一次执行。最典型用途是拖放:
    ///
    /// ```ignore
    /// tab.actions().move_to_ele(&src).hold().move_to_ele(&dst).release().perform().await?;
    /// ```
    pub fn actions(&self) -> Actions {
        Actions::new(self.clone())
    }

    /// 请求拦截句柄(对应 DP 风格;与 `listen()`/`console()`/`websocket()` 句柄风格统一)。
    ///
    /// 把扁平的 `intercept_start`/`intercept_xhr`/`intercept_next`/`intercept_stop` 收敛成句柄式
    /// (底层完全复用,不改变既有行为):
    ///
    /// ```ignore
    /// tab.intercept().start(&["/api/"]).await?;   // 务必在 get 之前
    /// tab.get(url).await?;
    /// let req = tab.intercept().next().await?;     // 取一个被拦请求
    /// req.fulfill(200, vec![], "{\"ok\":true}").await?;  // 伪造响应(或 resume/abort/resume_with)
    /// tab.intercept().stop().await?;
    /// ```
    pub fn intercept(&self) -> Intercept {
        Intercept::new(self.clone())
    }

    /// 下载管理句柄(对标 DP DownloadManager):`start`/`wait_done`/`wait_count_done`/`missions`/`stop`。
    ///
    /// 后台收集本标签触发的多个下载,支持任务列表、自定义重命名([`DownloadMission::save_as`])、
    /// best-effort 进度([`DownloadMission::downloaded_bytes`])。需先用
    /// [`BrowserOptions::download_path`](crate::launcher::BrowserOptions::download_path) 设下载目录。
    /// 一次性等单个下载用 [`wait_download`](Self::wait_download) 即可。
    pub fn downloads(&self) -> Downloads {
        Downloads::new(self.clone())
    }

    /// 录像句柄(对应 DP `tab.screencast`):`set_save_path` / `set_mode` / `set_fps` / `start` / `stop`。
    ///
    /// 大道至简实现 = 后台按帧间隔反复截视口存帧;`Video` 模式停止时用 ffmpeg 合成 mp4。
    ///
    /// ```ignore
    /// let cast = tab.screencast();
    /// cast.set_save_path("video").set_mode(ScreencastMode::Imgs).set_fps(10.0);
    /// cast.start(None).await?;
    /// tab.wait().secs(3.0).await;
    /// let out = cast.stop().await?;   // 帧目录(imgs)或 mp4(video)
    /// ```
    pub fn screencast(&self) -> Screencast {
        Screencast::new(self.clone())
    }

    /// 截图,返回 PNG 字节。`full_page=true` 截整页(按文档滚动尺寸),否则截当前视口。
    ///
    /// 要 JPEG / 指定区域 / 调质量,用 [`screenshot`](Self::screenshot) + [`ShotOpts`]。
    pub async fn screenshot_bytes(&self, full_page: bool) -> Result<Vec<u8>> {
        let clip = self.core.page_clip(full_page).await?;
        self.core.capture(clip, ImageFormat::Png, None).await
    }

    /// 截图,返回 base64 字符串(对应 DP `get_screenshot(as_base64=...)`)。默认 PNG。
    pub async fn screenshot_base64(&self, full_page: bool) -> Result<String> {
        Ok(base64_encode(&self.screenshot_bytes(full_page).await?))
    }

    /// 按 [`ShotOpts`] 截图,返回图片字节(支持整页 / 指定区域 / 格式 / JPEG 质量)。
    ///
    /// ```ignore
    /// let png = tab.screenshot(&ShotOpts::new().full_page(true)).await?;
    /// let jpg = tab.screenshot(&ShotOpts::new().format(ImageFormat::Jpeg).quality(80)).await?;
    /// let area = tab.screenshot(&ShotOpts::new().region((0.0, 0.0), (300.0, 200.0))).await?;
    /// ```
    pub async fn screenshot(&self, opts: &ShotOpts) -> Result<Vec<u8>> {
        let clip = match opts.region_clip() {
            Some(c) => c,
            None => self.core.page_clip(opts.full_page).await?,
        };
        self.core.capture(clip, opts.format, opts.quality).await
    }

    /// 截图并保存到 `path`。格式按 `path` 后缀自动选(`.jpg`/`.jpeg`→JPEG,其余 PNG)。
    /// 返回写入的路径;父目录不存在会自动创建。
    pub async fn get_screenshot(&self, path: impl AsRef<Path>, full_page: bool) -> Result<PathBuf> {
        let path = path.as_ref().to_path_buf();
        let format = ImageFormat::from_path(&path);
        let clip = self.core.page_clip(full_page).await?;
        let bytes = self.core.capture(clip, format, None).await?;
        write_file(&path, &bytes).await?;
        Ok(path)
    }

    /// 可视视口尺寸 `(innerWidth, innerHeight)`。
    pub async fn size(&self) -> Result<(f64, f64)> {
        let v = self
            .run_js("[window.innerWidth, window.innerHeight]")
            .await?;
        Ok((
            v.get(0).and_then(Value::as_f64).unwrap_or(0.0),
            v.get(1).and_then(Value::as_f64).unwrap_or(0.0),
        ))
    }

    /// 整个文档内容尺寸 `(scrollWidth, scrollHeight)`。
    pub async fn page_size(&self) -> Result<(f64, f64)> {
        let v = self
            .run_js(
                "(() => { const d = document.documentElement, b = document.body; \
                 return [Math.max(d.scrollWidth, b ? b.scrollWidth : 0), \
                 Math.max(d.scrollHeight, b ? b.scrollHeight : 0)]; })()",
            )
            .await?;
        Ok((
            v.get(0).and_then(Value::as_f64).unwrap_or(0.0),
            v.get(1).and_then(Value::as_f64).unwrap_or(0.0),
        ))
    }

    /// 一次取回页面尺寸/滚动/DPR 信息(见 [`PageRect`])。
    pub async fn rect(&self) -> Result<PageRect> {
        let v = self
            .run_js(
                "(() => { const d = document.documentElement, b = document.body; return { \
                 ww: window.innerWidth, wh: window.innerHeight, \
                 pw: Math.max(d.scrollWidth, b ? b.scrollWidth : 0), \
                 ph: Math.max(d.scrollHeight, b ? b.scrollHeight : 0), \
                 sx: window.scrollX, sy: window.scrollY, dpr: window.devicePixelRatio }; })()",
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

    /// 开始请求拦截。`keywords` 为 URL 子串过滤(空=拦截所有请求)。
    ///
    /// **匹配过滤**的请求通过 [`intercept_next`](Self::intercept_next) 交给你决策
    /// (放行 / 改写 / 伪造 / 中止);**不匹配**的请求由库自动放行,避免页面卡死。
    pub async fn intercept_start(&self, keywords: &[&str]) -> Result<()> {
        self.intercept_start_filter(ListenFilter {
            url_keywords: keywords.iter().map(|s| s.to_string()).collect(),
            xhr_only: false,
        })
        .await
    }

    /// 仅拦截 XHR/fetch 类请求(其余自动放行)。
    pub async fn intercept_xhr(&self, keywords: &[&str]) -> Result<()> {
        self.intercept_start_filter(ListenFilter {
            url_keywords: keywords.iter().map(|s| s.to_string()).collect(),
            xhr_only: true,
        })
        .await
    }

    async fn intercept_start_filter(&self, filter: ListenFilter) -> Result<()> {
        self.core
            .send_page("Network.setRequestInterception", json!({ "enabled": true }))
            .await?;
        let (state, rx) =
            InterceptorState::new(filter, self.core.conn.clone(), self.core.session_id.clone());
        *self.core.interceptor.lock().await = Some(state);
        *self.core.intercept_rx.lock().await = Some(rx);
        Ok(())
    }

    /// 等待下一个被拦截的请求(带默认超时)。拿到后必须调用其
    /// `resume` / `resume_with` / `fulfill` / `abort` 之一放行。
    pub async fn intercept_next(&self) -> Result<InterceptedRequest> {
        let mut guard = self.core.intercept_rx.lock().await;
        let rx = guard
            .as_mut()
            .ok_or_else(|| Error::Other("尚未调用 intercept_start".into()))?;
        let timeout = self.core.timeout();
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(r)) => Ok(r),
            Ok(None) => Err(Error::Transport("拦截通道已关闭".into())),
            Err(_) => Err(Error::Timeout(timeout)),
        }
    }

    /// 等待下一个被拦截的请求(自定义超时;超时返回 `None`,不报错)。
    pub async fn intercept_next_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<InterceptedRequest>> {
        let mut guard = self.core.intercept_rx.lock().await;
        let rx = guard
            .as_mut()
            .ok_or_else(|| Error::Other("尚未调用 intercept_start".into()))?;
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(r)) => Ok(Some(r)),
            Ok(None) => Err(Error::Transport("拦截通道已关闭".into())),
            Err(_) => Ok(None),
        }
    }

    /// 停止拦截并清空状态。
    pub async fn intercept_stop(&self) -> Result<()> {
        let _ = self
            .core
            .send_page(
                "Network.setRequestInterception",
                json!({ "enabled": false }),
            )
            .await;
        *self.core.interceptor.lock().await = None;
        *self.core.intercept_rx.lock().await = None;
        Ok(())
    }

    /// 关闭该标签页(同时移除其 BrowserContext)。
    pub async fn close(&self) -> Result<()> {
        let _ = self.core.send_page("Page.close", json!({})).await;
        let _ = self
            .core
            .conn
            .send(
                "Browser.removeBrowserContext",
                json!({ "browserContextId": self.core.browser_context_id }),
                None,
            )
            .await;
        Ok(())
    }
}

/// 一次性"自然上传" hook(由 [`TabCore::arm_upload`] 在触发点击前注入到主文档)。
///
/// 在**捕获阶段**监听整个文档的 `click`:一旦命中 `<input type=file>`(无论是直接点它,还是按钮
/// 里 `input.click()` 程序化唤起),就 `preventDefault` 掉原生系统文件框,把该 input 暂存到
/// `window.__drission_upload_el`,供 [`TabCore::wait_upload`] 轮询到后用 `Page.setFileInputFiles`
/// 填入文件。命中即自摘监听(一次武装对应一次上传);重复武装时先移除上一枚未触发的 hook 避免叠加。
///
/// 走 JS hook 而非 Juggler 原生 `Page.setInterceptFileChooserDialog`+`fileChooserOpened`:实测当前
/// Camoufox(headless)下原生事件不触发(跨进程/无头),hook 方案对两种点击写法都稳定可用。
const UPLOAD_HOOK_JS: &str = r#"(() => {
  if (window.__drission_upload_hook) {
    document.removeEventListener('click', window.__drission_upload_hook, true);
  }
  window.__drission_upload_el = null;
  const hook = (e) => {
    const t = e.target;
    if (t && t.tagName === 'INPUT' && String(t.type).toLowerCase() === 'file') {
      e.preventDefault();
      e.stopImmediatePropagation();
      window.__drission_upload_el = t;
      document.removeEventListener('click', hook, true);
      window.__drission_upload_hook = null;
    }
  };
  window.__drission_upload_hook = hook;
  document.addEventListener('click', hook, true);
})();"#;

/// 事件泵:持续消费本会话事件,更新 exec ctx 并驱动网络监听 / 拦截放行。
async fn pump(
    mut events: tokio::sync::broadcast::Receiver<Event>,
    conn: Connection,
    session_id: String,
    main_frame_id: String,
    ctx_tx: watch::Sender<Option<String>>,
    frame_ctxs: Arc<Mutex<HashMap<String, String>>>,
    interceptor: Arc<Mutex<Option<InterceptorState>>>,
) {
    loop {
        let ev = match events.recv().await {
            Ok(ev) => ev,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "事件泵落后,跳过部分事件");
                continue;
            }
            Err(_) => break,
        };
        if ev.session_id.as_deref() != Some(&session_id) {
            continue;
        }
        match ev.method.as_str() {
            "Runtime.executionContextCreated" => {
                let aux = &ev.params["auxData"];
                let name = aux["name"].as_str().unwrap_or("");
                let frame = aux["frameId"].as_str();
                // 只跟踪各 frame 的**主世界**(name 为空)。
                if name.is_empty() {
                    if let (Some(fid), Some(id)) = (frame, ev.params["executionContextId"].as_str())
                    {
                        frame_ctxs
                            .lock()
                            .await
                            .insert(fid.to_string(), id.to_string());
                        // 主帧另用 watch 通道(快路径 + 失效感知)。
                        if fid == main_frame_id {
                            let _ = ctx_tx.send(Some(id.to_string()));
                        }
                    }
                }
            }
            "Runtime.executionContextDestroyed" => {
                let gone = ev.params["executionContextId"].as_str();
                if let Some(gone) = gone {
                    frame_ctxs.lock().await.retain(|_, v| v != gone);
                }
                if ctx_tx.borrow().as_deref() == gone {
                    let _ = ctx_tx.send(None);
                }
            }
            "Network.requestWillBeSent" => {
                let decision = match interceptor.lock().await.as_ref() {
                    Some(s) => s.on_request_will_be_sent(&ev.params),
                    None => Decision::Ignore,
                };
                // 不匹配过滤的被拦请求:异步自动放行,避免阻塞事件泵与页面。
                if let Decision::AutoResume(rid) = decision {
                    let conn2 = conn.clone();
                    let session2 = session_id.clone();
                    tokio::spawn(async move {
                        let _ = conn2
                            .send(
                                "Network.resumeInterceptedRequest",
                                json!({ "requestId": rid }),
                                Some(&session2),
                            )
                            .await;
                    });
                }
            }
            _ => {}
        }
    }
    tracing::debug!(%session_id, "事件泵结束");
}

/// 长监听流式句柄:由 [`Tab::listen_stream`] 返回。内部复用同一缓冲,与单次抓取不冲突。
#[derive(Clone)]
pub struct ListenStream {
    tab: Tab,
}

impl ListenStream {
    /// 取下一个数据包(默认超时;超时返回 [`Error::Timeout`])。
    pub async fn next(&self) -> Result<DataPacket> {
        self.tab.listen_wait().await
    }

    /// 取下一个数据包(自定义超时;超时返回 `None`,便于"滑一下再等")。
    pub async fn next_timeout(&self, timeout: Duration) -> Result<Option<DataPacket>> {
        match self.tab.listen_wait_timeout(timeout).await {
            Ok(p) => Ok(Some(p)),
            Err(Error::Timeout(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// **非阻塞**取走当前已缓冲的所有数据包(适合"等一会儿,再把这段时间到达的都拿走"的批处理)。
    pub async fn drain_ready(&self) -> Vec<DataPacket> {
        self.tab.core.listen_buf.lock().await.drain(..).collect()
    }
}

/// 长监听后台抽取循环:持续把页面队列搬进 `listen_buf`,直到监听停止或连接关闭。
async fn bg_drain_loop(core: Arc<TabCore>) {
    loop {
        if !*core.listen_active.lock().await {
            break;
        }
        match core.evaluate(DRAIN_JS, true).await {
            Ok(v) => {
                let pkts = parse_packets(&v);
                if !pkts.is_empty() {
                    let mut buf = core.listen_buf.lock().await;
                    for p in pkts {
                        buf.push_back(p);
                    }
                }
            }
            // 连接断开则退出;其余(如导航瞬间上下文失效)忽略,下一拍重试。
            Err(Error::Transport(_)) => break,
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    tracing::debug!("长监听后台抽取结束");
}

/// 常用按键描述:返回 `(规范 key, code, keyCode, text)`。未知键只发 key 名(text 留空)。
fn key_descriptor(key: &str) -> (&str, &str, i64, &str) {
    match key {
        "ArrowDown" | "Down" => ("ArrowDown", "ArrowDown", 40, ""),
        "ArrowUp" | "Up" => ("ArrowUp", "ArrowUp", 38, ""),
        "ArrowLeft" | "Left" => ("ArrowLeft", "ArrowLeft", 37, ""),
        "ArrowRight" | "Right" => ("ArrowRight", "ArrowRight", 39, ""),
        "Enter" => ("Enter", "Enter", 13, ""),
        "Escape" | "Esc" => ("Escape", "Escape", 27, ""),
        "Tab" => ("Tab", "Tab", 9, ""),
        "Backspace" => ("Backspace", "Backspace", 8, ""),
        "Delete" | "Del" => ("Delete", "Delete", 46, ""),
        "Insert" => ("Insert", "Insert", 45, ""),
        "PageDown" => ("PageDown", "PageDown", 34, ""),
        "PageUp" => ("PageUp", "PageUp", 33, ""),
        "Home" => ("Home", "Home", 36, ""),
        "End" => ("End", "End", 35, ""),
        " " | "Space" => (" ", "Space", 32, " "),
        "Control" | "Ctrl" => ("Control", "ControlLeft", 17, ""),
        "Shift" => ("Shift", "ShiftLeft", 16, ""),
        "Alt" => ("Alt", "AltLeft", 18, ""),
        "Meta" | "Command" | "Cmd" => ("Meta", "MetaLeft", 91, ""),
        other => (
            other,
            "",
            0,
            if other.chars().count() == 1 {
                other
            } else {
                ""
            },
        ),
    }
}

/// 应用上下文级覆盖(指纹/视口/代理等)。逐项 best-effort,单项失败只告警。
async fn apply_context_overrides(conn: &Connection, bctx: &str, opts: &BrowserOptions) {
    let fp = &opts.fingerprint;

    macro_rules! best_effort {
        ($method:expr, $params:expr) => {
            if let Err(e) = conn.send($method, $params, None).await {
                tracing::warn!(method = $method, error = %e, "上下文覆盖失败(已忽略)");
            }
        };
    }

    if let Some(ua) = &fp.user_agent {
        best_effort!(
            "Browser.setUserAgentOverride",
            json!({ "browserContextId": bctx, "userAgent": ua })
        );
    }
    if let Some(locale) = &fp.locale {
        best_effort!(
            "Browser.setLocaleOverride",
            json!({ "browserContextId": bctx, "locale": locale })
        );
    }
    if let Some(tz) = &fp.timezone_id {
        best_effort!(
            "Browser.setTimezoneOverride",
            json!({ "browserContextId": bctx, "timezoneId": tz })
        );
    }
    let platform = fp.platform.clone().or_else(|| fp.os.map(platform_for_os));
    if let Some(platform) = platform {
        best_effort!(
            "Browser.setPlatformOverride",
            json!({ "browserContextId": bctx, "platform": platform })
        );
    }
    if let Some(geo) = &fp.geolocation {
        let mut g = json!({ "latitude": geo.latitude, "longitude": geo.longitude });
        if let Some(acc) = geo.accuracy {
            g["accuracy"] = json!(acc);
        }
        best_effort!(
            "Browser.setGeolocationOverride",
            json!({ "browserContextId": bctx, "geolocation": g })
        );
    }
    if let Some((w, h)) = opts.window_size {
        best_effort!(
            "Browser.setDefaultViewport",
            json!({ "browserContextId": bctx, "viewport": { "viewportSize": { "width": w, "height": h } } })
        );
    }
    if opts.bypass_csp {
        best_effort!(
            "Browser.setBypassCSP",
            json!({ "browserContextId": bctx, "bypassCSP": true })
        );
    }
    if opts.ignore_https_errors {
        best_effort!(
            "Browser.setIgnoreHTTPSErrors",
            json!({ "browserContextId": bctx, "ignoreHTTPSErrors": true })
        );
    }
    if let Some(proxy) = &opts.proxy {
        if let Some(params) = proxy_to_params(bctx, proxy) {
            best_effort!("Browser.setContextProxy", params);
        }
    }
}

fn platform_for_os(os: OsType) -> String {
    match os {
        OsType::Windows => "Win32",
        OsType::MacOS => "MacIntel",
        OsType::Linux => "Linux x86_64",
    }
    .to_string()
}

/// 把 `Proxy` 解析为 `Browser.setContextProxy` 的参数。
fn proxy_to_params(bctx: &str, proxy: &crate::launcher::Proxy) -> Option<Value> {
    let s = proxy.server.trim();
    let (scheme, rest) = s.split_once("://").unwrap_or(("http", s));
    let ty = match scheme.to_ascii_lowercase().as_str() {
        "http" => "http",
        "https" => "https",
        "socks5" | "socks" | "socks5h" => "socks",
        "socks4" => "socks4",
        _ => "http",
    };
    let (host, port_str) = rest.rsplit_once(':')?;
    let port: u32 = port_str.parse().ok()?;
    let mut o = json!({
        "browserContextId": bctx,
        "type": ty,
        "host": host,
        "port": port,
        "bypass": proxy.bypass,
    });
    if let Some(u) = &proxy.username {
        o["username"] = json!(u);
    }
    if let Some(p) = &proxy.password {
        o["password"] = json!(p);
    }
    Some(o)
}

/// 等待目标 attach,返回其 page 会话 id。
async fn wait_attached(
    events: &mut tokio::sync::broadcast::Receiver<Event>,
    target_id: &str,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    loop {
        match timeout_at(deadline, events.recv()).await {
            Ok(Ok(ev)) => {
                if ev.method == "Browser.attachedToTarget"
                    && ev.params["targetInfo"]["targetId"].as_str() == Some(target_id)
                {
                    if let Some(sid) = ev.params["sessionId"].as_str() {
                        return Ok(sid.to_string());
                    }
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => return Err(Error::Transport("连接已关闭".into())),
            Err(_) => return Err(Error::Timeout(timeout)),
        }
    }
}

/// 在 page 会话上等待主帧 frameId 与主世界 executionContextId。
async fn wait_frame_and_ctx(
    events: &mut tokio::sync::broadcast::Receiver<Event>,
    session_id: &str,
    timeout: Duration,
) -> Result<(String, String)> {
    let deadline = Instant::now() + timeout;
    let mut main_frame: Option<String> = None;
    let mut exec_ctx: Option<String> = None;
    loop {
        match timeout_at(deadline, events.recv()).await {
            Ok(Ok(ev)) => {
                if ev.session_id.as_deref() != Some(session_id) {
                    continue;
                }
                match ev.method.as_str() {
                    "Page.frameAttached" => {
                        if ev.params["parentFrameId"].as_str().is_none() {
                            main_frame = ev.params["frameId"].as_str().map(str::to_string);
                        }
                    }
                    "Runtime.executionContextCreated" => {
                        let aux = &ev.params["auxData"];
                        if aux["name"].as_str().unwrap_or("").is_empty() {
                            exec_ctx = ev.params["executionContextId"].as_str().map(str::to_string);
                            if main_frame.is_none() {
                                main_frame = aux["frameId"].as_str().map(str::to_string);
                            }
                        }
                    }
                    _ => {}
                }
                if let (Some(f), Some(c)) = (&main_frame, &exec_ctx) {
                    return Ok((f.clone(), c.clone()));
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => return Err(Error::Transport("连接已关闭".into())),
            Err(_) => return Err(Error::Timeout(timeout)),
        }
    }
}

/// 从 Runtime.evaluate/callFunction 的返回里取出有用部分。
///
/// - 有异常:返回协议错误;
/// - returnByValue=true:返回 `result.value`;
/// - returnByValue=false:返回 `result`(含 `objectId`/`subtype` 等)。
pub(crate) fn extract_runtime_result(r: Value) -> Result<Value> {
    if let Some(ex) = r.get("exceptionDetails") {
        if !ex.is_null() {
            let text = ex["text"]
                .as_str()
                .or_else(|| ex["value"].as_str())
                .unwrap_or("JS 执行异常");
            return Err(Error::Protocol(text.to_string()));
        }
    }
    let result = r.get("result").cloned().unwrap_or(Value::Null);
    if let Some(v) = result.get("value") {
        return Ok(v.clone());
    }
    Ok(result)
}

/// 生成"查单个元素"的 JS 表达式,结果为节点或 null。
pub(crate) fn single_query_expr(query: &Query) -> String {
    match query {
        Query::Css(sel) => format!("document.querySelector({})", js_string(sel)),
        Query::Xpath(xp) => format!(
            "document.evaluate({}, document, null, 9, null).singleNodeValue",
            js_string(xp)
        ),
    }
}

/// 生成"查多个元素"的 JS 表达式,结果为节点数组。
pub(crate) fn multi_query_expr(query: &Query) -> String {
    match query {
        Query::Css(sel) => format!("Array.from(document.querySelectorAll({}))", js_string(sel)),
        Query::Xpath(xp) => format!(
            "(() => {{ const r = document.evaluate({}, document, null, 7, null); const a = []; \
             for (let i = 0; i < r.snapshotLength; i++) a.push(r.snapshotItem(i)); return a; }})()",
            js_string(xp)
        ),
    }
}

/// 把字符串安全编码成 JS 字面量(用 JSON 编码即可)。
fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

pub(crate) fn is_index(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::{ImageFormat, ShotOpts, UPLOAD_HOOK_JS};
    use std::path::Path;

    #[test]
    fn image_format_from_path_and_mime() {
        assert_eq!(ImageFormat::from_path(Path::new("a.png")), ImageFormat::Png);
        assert_eq!(
            ImageFormat::from_path(Path::new("a.jpg")),
            ImageFormat::Jpeg
        );
        assert_eq!(
            ImageFormat::from_path(Path::new("a.JPEG")),
            ImageFormat::Jpeg
        );
        // 未知/无后缀回退 PNG。
        assert_eq!(
            ImageFormat::from_path(Path::new("a.webp")),
            ImageFormat::Png
        );
        assert_eq!(ImageFormat::from_path(Path::new("noext")), ImageFormat::Png);
        assert_eq!(ImageFormat::Png.mime(), "image/png");
        assert_eq!(ImageFormat::Jpeg.mime(), "image/jpeg");
    }

    #[test]
    fn shot_opts_region_clip() {
        // region 设置后产出 {x,y,width,height},宽高为右下减左上。
        let clip = ShotOpts::new()
            .region((10.0, 20.0), (110.0, 220.0))
            .region_clip()
            .expect("region 应产出 clip");
        assert_eq!(clip["x"], 10.0);
        assert_eq!(clip["y"], 20.0);
        assert_eq!(clip["width"], 100.0);
        assert_eq!(clip["height"], 200.0);
        // 未设 region 时无 clip(走视口/整页)。
        assert!(ShotOpts::new().region_clip().is_none());
    }

    /// "自然上传" hook 与 [`TabCore::wait_upload`] 靠 `window.__drission_upload_el` 这个全局对接,
    /// 任一侧改名都会断链。本测试把契约钉死:hook 必须写入该全局、按捕获阶段拦 click、
    /// `preventDefault` 掉原生文件框、并按 `<input type=file>` 命中。
    #[test]
    fn upload_hook_keeps_wait_contract() {
        // wait_upload 轮询的全局名(改名必同步)。
        assert!(
            UPLOAD_HOOK_JS.contains("window.__drission_upload_el"),
            "hook 必须写入 wait_upload 轮询的 window.__drission_upload_el"
        );
        // 捕获阶段(第三参 true)才能在按钮代理 input.click() 时先于页面拿到事件。
        assert!(
            UPLOAD_HOOK_JS.contains("addEventListener('click'")
                && UPLOAD_HOOK_JS.contains(", true)"),
            "hook 必须在捕获阶段监听 click"
        );
        // 必须拦掉原生系统文件框,否则会真的弹框卡住。
        assert!(UPLOAD_HOOK_JS.contains("preventDefault"));
        // 只对文件输入框生效。
        assert!(UPLOAD_HOOK_JS.contains("'file'") && UPLOAD_HOOK_JS.contains("INPUT"));
    }
}
