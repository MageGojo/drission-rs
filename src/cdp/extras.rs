//! CDP 后端「标配补齐」:对标 Playwright / Puppeteer / DrissionPage 的通用浏览器能力。
//!
//! 见 [`docs/标配补齐.md`](https://github.com/MageGojo/drission-rs/blob/main/docs/标配补齐.md)。
//! 这些能力**绝大多数是 Chromium / CDP 专有**(Firefox/Juggler 无对应),故只在 CDP 后端提供:
//!
//! - 顶层 [`ChromiumTab`]:PDF 导出 / `set_content` / 保存 MHTML / `expose_function` / HAR 录制。
//! - `tab.set()` 句柄([`ChromiumSetTab`]):媒体模拟 / 网络条件 / CPU 节流 / 权限 / 设备 / 触摸 / web storage。
//! - `tab.wait()` 句柄([`ChromiumWait`]):`new_tab` / `download_begin` / `network_idle` / `ele_loaded`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::AbortHandle;
use tokio::time::{Instant, sleep};

use super::ChromiumTab;
use super::core::CdpCore;
use super::handles::{ChromiumSetTab, ChromiumWait};
use super::tab::doc_query_expr;
use crate::{Error, Result};

// ════════════════════════════════════════════════════════════════════════════
// 值类型
// ════════════════════════════════════════════════════════════════════════════

/// PDF 导出选项(`Page.printToPDF`)。默认 A4 纵向、打印背景、保留 CSS 页尺寸。
#[derive(Debug, Clone)]
pub struct PdfOptions {
    /// 横向。
    pub landscape: bool,
    /// 打印背景图/色。
    pub print_background: bool,
    /// 缩放(1.0 = 100%)。
    pub scale: f64,
    /// 纸宽(英寸,默认 A4 = 8.27)。
    pub paper_width: f64,
    /// 纸高(英寸,默认 A4 = 11.69)。
    pub paper_height: f64,
    /// 优先使用页面 CSS 的 `@page` 尺寸。
    pub prefer_css_page_size: bool,
}

impl Default for PdfOptions {
    fn default() -> Self {
        Self {
            landscape: false,
            print_background: true,
            scale: 1.0,
            paper_width: 8.27,
            paper_height: 11.69,
            prefer_css_page_size: true,
        }
    }
}

impl PdfOptions {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn landscape(mut self, yes: bool) -> Self {
        self.landscape = yes;
        self
    }
    pub fn print_background(mut self, yes: bool) -> Self {
        self.print_background = yes;
        self
    }
    pub fn scale(mut self, scale: f64) -> Self {
        self.scale = scale;
        self
    }
    /// 设纸张尺寸(英寸)。
    pub fn paper(mut self, width_in: f64, height_in: f64) -> Self {
        self.paper_width = width_in;
        self.paper_height = height_in;
        self
    }
    fn to_params(&self) -> Value {
        json!({
            "landscape": self.landscape,
            "printBackground": self.print_background,
            "scale": self.scale,
            "paperWidth": self.paper_width,
            "paperHeight": self.paper_height,
            "preferCSSPageSize": self.prefer_css_page_size,
            "transferMode": "ReturnAsBase64",
        })
    }
}

/// 网络条件(`Network.emulateNetworkConditions`)。吞吐单位为**字节/秒**(`-1` = 不限)。
#[derive(Debug, Clone, Copy)]
pub struct NetworkConditions {
    pub offline: bool,
    /// 额外往返延迟(毫秒)。
    pub latency_ms: f64,
    /// 下行吞吐(字节/秒,`-1` = 不限)。
    pub download_bps: f64,
    /// 上行吞吐(字节/秒,`-1` = 不限)。
    pub upload_bps: f64,
}

impl NetworkConditions {
    pub fn new(latency_ms: f64, download_bps: f64, upload_bps: f64) -> Self {
        Self {
            offline: false,
            latency_ms,
            download_bps,
            upload_bps,
        }
    }
    /// 离线。
    pub fn offline() -> Self {
        Self {
            offline: true,
            latency_ms: 0.0,
            download_bps: -1.0,
            upload_bps: -1.0,
        }
    }
    /// 慢速 3G 预设(对齐 Chrome DevTools)。
    pub fn slow_3g() -> Self {
        Self {
            offline: false,
            latency_ms: 400.0,
            download_bps: 400.0 * 1024.0 / 8.0,
            upload_bps: 400.0 * 1024.0 / 8.0,
        }
    }
    /// 快速 3G 预设(对齐 Chrome DevTools)。
    pub fn fast_3g() -> Self {
        Self {
            offline: false,
            latency_ms: 150.0,
            download_bps: 1.6 * 1024.0 * 1024.0 / 8.0,
            upload_bps: 750.0 * 1024.0 / 8.0,
        }
    }
}

/// 设备描述(移动端模拟):UA + 视口 + DPR + mobile + 触摸,一把梭。
#[derive(Debug, Clone)]
pub struct Device {
    pub name: String,
    pub user_agent: String,
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub mobile: bool,
    pub has_touch: bool,
}

impl Device {
    pub fn new(
        name: impl Into<String>,
        user_agent: impl Into<String>,
        width: u32,
        height: u32,
        device_scale_factor: f64,
        mobile: bool,
        has_touch: bool,
    ) -> Self {
        Self {
            name: name.into(),
            user_agent: user_agent.into(),
            width,
            height,
            device_scale_factor,
            mobile,
            has_touch,
        }
    }

    /// iPhone 13 / 14(390×844 @3x)。
    pub fn iphone_13() -> Self {
        Self::new(
            "iPhone 13",
            "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) AppleWebKit/605.1.15 \
             (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1",
            390,
            844,
            3.0,
            true,
            true,
        )
    }

    /// iPhone SE(375×667 @2x)。
    pub fn iphone_se() -> Self {
        Self::new(
            "iPhone SE",
            "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) AppleWebKit/605.1.15 \
             (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1",
            375,
            667,
            2.0,
            true,
            true,
        )
    }

    /// iPad(820×1180 @2x)。
    pub fn ipad() -> Self {
        Self::new(
            "iPad",
            "Mozilla/5.0 (iPad; CPU OS 16_0 like Mac OS X) AppleWebKit/605.1.15 \
             (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1",
            820,
            1180,
            2.0,
            true,
            true,
        )
    }

    /// Google Pixel 7(412×915 @2.625x)。
    pub fn pixel_7() -> Self {
        Self::new(
            "Pixel 7",
            "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36",
            412,
            915,
            2.625,
            true,
            true,
        )
    }

    /// Samsung Galaxy S9+(360×740 @4x)。
    pub fn galaxy_s9() -> Self {
        Self::new(
            "Galaxy S9+",
            "Mozilla/5.0 (Linux; Android 10; SM-G965F) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36",
            360,
            740,
            4.0,
            true,
            true,
        )
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 顶层 ChromiumTab:PDF / set_content / MHTML / expose / HAR
// ════════════════════════════════════════════════════════════════════════════

impl ChromiumTab {
    /// 把当前页打印为 PDF 字节(`Page.printToPDF`,对标 Playwright/Puppeteer `page.pdf()`)。
    /// **需无头 Chrome**(有头模式 Chrome 不一定返回数据)。
    pub async fn print_to_pdf(&self, opts: &PdfOptions) -> Result<Vec<u8>> {
        let r = self.core.send("Page.printToPDF", opts.to_params()).await?;
        let b64 = r["data"]
            .as_str()
            .ok_or_else(|| Error::Protocol("printToPDF 无 data(请用无头 Chrome)".into()))?;
        crate::util::base64_decode(b64).ok_or_else(|| Error::Other("PDF base64 解码失败".into()))
    }

    /// 把当前页打印为 PDF 并保存到 `path`(自动创建父目录)。返回最终路径。
    pub async fn save_pdf(&self, path: impl AsRef<Path>, opts: &PdfOptions) -> Result<PathBuf> {
        let bytes = self.print_to_pdf(opts).await?;
        let path = path.as_ref().to_path_buf();
        ensure_parent(&path).await?;
        tokio::fs::write(&path, &bytes).await?;
        Ok(path)
    }

    /// 直接把页面文档内容设为 `html`(对标 Playwright/Puppeteer `page.set_content`)。
    /// 优先 `Page.setDocumentContent`,失败回退 `document.open/write/close`。
    pub async fn set_content(&self, html: &str) -> Result<()> {
        if let Ok(frame_id) = self.main_frame_id().await {
            if !frame_id.is_empty()
                && self
                    .core
                    .send(
                        "Page.setDocumentContent",
                        json!({ "frameId": frame_id, "html": html }),
                    )
                    .await
                    .is_ok()
            {
                return Ok(());
            }
        }
        let js = format!(
            "(function(h){{document.open();document.write(h);document.close();}})({})",
            jstr(html)
        );
        self.core.eval_value(&js).await?;
        Ok(())
    }

    /// 抓取当前页的 MHTML 单文件快照字符串(`Page.captureSnapshot`,对标 DP `tab.save()`)。
    pub async fn mhtml(&self) -> Result<String> {
        let r = self
            .core
            .send("Page.captureSnapshot", json!({ "format": "mhtml" }))
            .await?;
        Ok(r["data"].as_str().unwrap_or_default().to_string())
    }

    /// 抓取 MHTML 并保存到 `path`(`.mhtml`)。返回最终路径。
    pub async fn save_mhtml(&self, path: impl AsRef<Path>) -> Result<PathBuf> {
        let data = self.mhtml().await?;
        let path = path.as_ref().to_path_buf();
        ensure_parent(&path).await?;
        tokio::fs::write(&path, data.as_bytes()).await?;
        Ok(path)
    }

    /// 主框架 frameId(`Page.getFrameTree`)。
    pub(crate) async fn main_frame_id(&self) -> Result<String> {
        let r = self.core.send("Page.getFrameTree", json!({})).await?;
        Ok(r["frameTree"]["frame"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    /// 把一个 Rust 闭包暴露成页面里的全局异步函数 `window.<name>(...args)`(对标
    /// Playwright/Puppeteer `expose_function`)。页面 `await window.name(a,b)` 会回调 `handler`,
    /// 其返回值(JSON)作为 Promise 结果回传页面。
    ///
    /// **反检测取舍**:为收到 `Runtime.bindingCalled` 事件需开 `Runtime.enable`(同 `console()`),
    /// 故对极致反检测场景慎用。返回的 [`ExposedFunction`] 守卫 drop 即停止回调。
    pub async fn expose_function<F>(&self, name: &str, handler: F) -> Result<ExposedFunction>
    where
        F: Fn(Vec<Value>) -> Result<Value> + Send + Sync + 'static,
    {
        let safe: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let binding = format!("__drission_bind_{safe}");
        let cb_obj = format!("__drission_cb_{safe}");

        self.core.send("Runtime.enable", json!({})).await?;
        self.core
            .send("Runtime.addBinding", json!({ "name": binding }))
            .await?;

        // 注入 stub:把 window.<name> 包成「JSON 编码 args → 调原始 binding → 返回 Promise」。
        let stub = format!(
            "(()=>{{window.{cb_obj}=window.{cb_obj}||{{}};window.__drission_seq=window.__drission_seq||0;\
             window.{name}=function(){{var a=Array.prototype.slice.call(arguments);\
             return new Promise(function(res,rej){{var s=++window.__drission_seq;\
             window.{cb_obj}[s]={{res:res,rej:rej}};\
             window.{binding}(JSON.stringify({{seq:s,args:a}}));}});}};}})()"
        );
        self.core
            .send(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": stub }),
            )
            .await?;
        let _ = self.core.eval_value(&stub).await; // 当前文档也即时生效

        let handler: Arc<dyn Fn(Vec<Value>) -> Result<Value> + Send + Sync> = Arc::new(handler);
        let task = tokio::spawn(binding_pump(
            self.core.clone(),
            binding.clone(),
            cb_obj,
            handler,
        ));
        Ok(ExposedFunction {
            core: self.core.clone(),
            binding,
            abort: task.abort_handle(),
        })
    }

    /// 开始录制 HAR(`HTTP Archive`,含响应体)。**在导航前调用**;`stop()` 拿 [`HarLog`] 落盘。
    /// 对标 Playwright `record_har`。
    pub async fn har_record(&self) -> Result<HarRecorder> {
        self.har_record_with(true).await
    }

    /// 同 [`har_record`](Self::har_record),`capture_bodies=false` 则不抓响应体(更省、更快)。
    pub async fn har_record_with(&self, capture_bodies: bool) -> Result<HarRecorder> {
        self.core.send("Network.enable", json!({})).await?;
        let state = Arc::new(Mutex::new(HarState {
            entries: Vec::new(),
            pending: HashMap::new(),
        }));
        let task = tokio::spawn(har_pump(self.core.clone(), capture_bodies, state.clone()));
        Ok(HarRecorder {
            state,
            abort: task.abort_handle(),
        })
    }
}

// ════════════════════════════════════════════════════════════════════════════
// tab.set() 句柄:媒体 / 网络 / CPU / 权限 / web storage / 设备 / 触摸
// ════════════════════════════════════════════════════════════════════════════

impl ChromiumSetTab {
    /// 模拟媒体类型与特性(`Emulation.setEmulatedMedia`)。`media` 如 `"print"`/`"screen"`;
    /// `features` 如 `[("prefers-color-scheme","dark")]`。
    pub async fn emulate_media(&self, media: Option<&str>, features: &[(&str, &str)]) -> Result<()> {
        let feats: Vec<Value> = features
            .iter()
            .map(|(n, v)| json!({ "name": n, "value": v }))
            .collect();
        let mut p = json!({ "features": feats });
        if let Some(m) = media {
            p["media"] = json!(m);
        }
        self.core.send("Emulation.setEmulatedMedia", p).await?;
        Ok(())
    }

    /// 模拟深色 / 浅色模式(`prefers-color-scheme`)。
    pub async fn emulate_dark(&self, dark: bool) -> Result<()> {
        let v = if dark { "dark" } else { "light" };
        self.emulate_media(None, &[("prefers-color-scheme", v)]).await
    }

    async fn ensure_network(&self) -> Result<()> {
        self.core.send("Network.enable", json!({})).await?;
        Ok(())
    }

    /// 设置离线 / 在线(`Network.emulateNetworkConditions`)。
    pub async fn offline(&self, on: bool) -> Result<()> {
        self.ensure_network().await?;
        self.core
            .send(
                "Network.emulateNetworkConditions",
                json!({ "offline": on, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1 }),
            )
            .await?;
        Ok(())
    }

    /// 模拟网络条件(离线 / 限速)。
    pub async fn network_conditions(&self, nc: &NetworkConditions) -> Result<()> {
        self.ensure_network().await?;
        self.core
            .send(
                "Network.emulateNetworkConditions",
                json!({
                    "offline": nc.offline,
                    "latency": nc.latency_ms,
                    "downloadThroughput": nc.download_bps,
                    "uploadThroughput": nc.upload_bps,
                }),
            )
            .await?;
        Ok(())
    }

    /// CPU 节流(`Emulation.setCPUThrottlingRate`)。`rate=4.0` 即降速到 1/4。
    pub async fn cpu_throttling(&self, rate: f64) -> Result<()> {
        self.core
            .send("Emulation.setCPUThrottlingRate", json!({ "rate": rate }))
            .await?;
        Ok(())
    }

    /// 给某 origin 授予权限(`Browser.grantPermissions`),如
    /// `["geolocation","clipboard-read","notifications","camera","microphone"]`。
    pub async fn grant_permissions(&self, origin: &str, permissions: &[&str]) -> Result<()> {
        let mut p = json!({ "origin": origin, "permissions": permissions });
        if let Some(ctx) = &self.core.browser_context_id {
            p["browserContextId"] = json!(ctx);
        }
        self.core
            .conn
            .send("Browser.grantPermissions", p, None)
            .await?;
        Ok(())
    }

    /// 重置(撤销)所有此前授予的权限(`Browser.resetPermissions`)。
    pub async fn reset_permissions(&self) -> Result<()> {
        let mut p = json!({});
        if let Some(ctx) = &self.core.browser_context_id {
            p["browserContextId"] = json!(ctx);
        }
        self.core
            .conn
            .send("Browser.resetPermissions", p, None)
            .await?;
        Ok(())
    }

    /// 开/关触摸事件模拟(`Emulation.setTouchEmulationEnabled`)。
    pub async fn touch(&self, on: bool) -> Result<()> {
        self.core
            .send(
                "Emulation.setTouchEmulationEnabled",
                json!({ "enabled": on, "maxTouchPoints": if on { 5 } else { 1 } }),
            )
            .await?;
        Ok(())
    }

    /// 一把梭模拟设备(UA + 视口 + DPR + mobile + 触摸)。见 [`Device`] 预设。
    pub async fn device(&self, d: &Device) -> Result<()> {
        self.core
            .send(
                "Emulation.setDeviceMetricsOverride",
                json!({
                    "width": d.width,
                    "height": d.height,
                    "deviceScaleFactor": d.device_scale_factor,
                    "mobile": d.mobile,
                }),
            )
            .await?;
        self.touch(d.has_touch).await?;
        if !d.user_agent.is_empty() {
            self.core
                .send(
                    "Emulation.setUserAgentOverride",
                    json!({ "userAgent": d.user_agent }),
                )
                .await?;
        }
        Ok(())
    }

    /// 清除设备模拟,恢复真实视口(`Emulation.clearDeviceMetricsOverride`)。
    pub async fn clear_device(&self) -> Result<()> {
        let _ = self
            .core
            .send("Emulation.clearDeviceMetricsOverride", json!({}))
            .await;
        self.touch(false).await
    }

    /// 写 `localStorage`(便捷,内部走 `Runtime.evaluate`)。
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
        self.core.eval_value("localStorage.clear()").await?;
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
        self.core.eval_value("sessionStorage.clear()").await?;
        Ok(())
    }

    async fn storage_set(&self, store: &str, key: &str, value: &str) -> Result<()> {
        let js = format!("{store}.setItem({}, {})", jstr(key), jstr(value));
        self.core.eval_value(&js).await?;
        Ok(())
    }
    async fn storage_get(&self, store: &str, key: &str) -> Result<Option<String>> {
        let js = format!("{store}.getItem({})", jstr(key));
        let v = self.core.eval_value(&js).await?;
        Ok(v.as_str().map(str::to_string))
    }
    async fn storage_remove(&self, store: &str, key: &str) -> Result<()> {
        let js = format!("{store}.removeItem({})", jstr(key));
        self.core.eval_value(&js).await?;
        Ok(())
    }
}

// ════════════════════════════════════════════════════════════════════════════
// tab.wait() 句柄:new_tab / download_begin / network_idle / ele_loaded
// ════════════════════════════════════════════════════════════════════════════

impl ChromiumWait {
    fn deadline_at(&self, timeout: Option<Duration>) -> Instant {
        Instant::now() + timeout.unwrap_or_else(|| self.core.timeout())
    }

    /// 等元素出现于 DOM(查到即返回 `true`,超时 `false`)。补齐 cdp 端(camoufox 已有)。
    pub async fn ele_loaded(&self, selector: &str, timeout: Option<Duration>) -> Result<bool> {
        let deadline = self.deadline_at(timeout);
        loop {
            if self
                .core
                .eval_handle(&doc_query_expr(selector, true))
                .await?
                .is_some()
            {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(Duration::from_millis(80)).await;
        }
    }

    /// 等本标签弹出的**新标签 / 弹窗**(`window.open` / `target=_blank`),返回可驱动的新 [`ChromiumTab`]。
    /// 超时返回 `None`。对标 Playwright `expect_popup` / DP `wait.new_tab`。
    pub async fn new_tab(&self, timeout: Option<Duration>) -> Result<Option<ChromiumTab>> {
        let _ = self
            .core
            .conn
            .send(
                "Target.setDiscoverTargets",
                json!({ "discover": true }),
                None,
            )
            .await;
        let mut events = self.core.conn.subscribe();
        let deadline = self.deadline_at(timeout);
        let opener = self.core.target_id.clone();
        loop {
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                return Ok(None);
            }
            let ev = match tokio::time::timeout(remain, events.recv()).await {
                Ok(Ok(ev)) => ev,
                Ok(Err(RecvError::Lagged(_))) => continue,
                Ok(Err(RecvError::Closed)) => return Ok(None),
                Err(_) => return Ok(None),
            };
            if ev.method != "Target.targetCreated" {
                continue;
            }
            let ti = &ev.params["targetInfo"];
            if ti["type"].as_str() != Some("page") {
                continue;
            }
            // 优先认本标签开出的弹窗(openerId 命中);openerId 缺失时也接受(兼容)。
            if let Some(op) = ti["openerId"].as_str() {
                if op != opener {
                    continue;
                }
            }
            let target_id = ti["targetId"].as_str().unwrap_or_default().to_string();
            if target_id.is_empty() || target_id == opener {
                continue;
            }
            return Ok(Some(self.attach_popup(target_id).await?));
        }
    }

    async fn attach_popup(&self, target_id: String) -> Result<ChromiumTab> {
        let a = self
            .core
            .conn
            .send(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                None,
            )
            .await?;
        let session_id = a["sessionId"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: 附着弹窗无 sessionId"))?
            .to_string();
        let core = CdpCore::new(
            self.core.conn.clone(),
            session_id,
            target_id,
            self.core.download_dir(),
            self.core.browser_context_id.clone(),
        );
        let _ = core.send("Page.enable", json!({})).await;
        Ok(ChromiumTab::new(core))
    }

    /// 等下载**开始**(`Page.downloadWillBegin`)。超时 `false`。会自动允许下载(若未配下载目录则
    /// 用 `allowAndName`,文件落到浏览器默认下载目录)。
    pub async fn download_begin(&self, timeout: Option<Duration>) -> Result<bool> {
        if let Some(dir) = self.core.download_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = self
                .core
                .send(
                    "Browser.setDownloadBehavior",
                    json!({ "behavior": "allow", "downloadPath": dir.display().to_string(), "eventsEnabled": true }),
                )
                .await;
        } else {
            let _ = self
                .core
                .send(
                    "Browser.setDownloadBehavior",
                    json!({ "behavior": "allowAndName", "eventsEnabled": true }),
                )
                .await;
        }
        let mut events = self.core.conn.subscribe();
        let sid = self.core.session_id.clone();
        let deadline = self.deadline_at(timeout);
        loop {
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                return Ok(false);
            }
            let ev = match tokio::time::timeout(remain, events.recv()).await {
                Ok(Ok(ev)) => ev,
                Ok(Err(RecvError::Lagged(_))) => continue,
                Ok(Err(RecvError::Closed)) => return Ok(false),
                Err(_) => return Ok(false),
            };
            if ev.session_id.as_deref() != Some(sid.as_str()) {
                continue;
            }
            if ev.method == "Page.downloadWillBegin" {
                return Ok(true);
            }
        }
    }

    /// 等网络空闲:在途请求数降到 0 并持续 `idle_secs` 秒(对标 Playwright `networkidle`)。
    /// 超时返回 `false`。
    pub async fn network_idle(&self, idle_secs: f64, timeout: Option<Duration>) -> Result<bool> {
        self.core.send("Network.enable", json!({})).await?;
        let mut events = self.core.conn.subscribe();
        let sid = self.core.session_id.clone();
        let deadline = self.deadline_at(timeout);
        let idle = Duration::from_secs_f64(idle_secs.max(0.05));
        let mut inflight: i64 = 0;
        let mut last_change = Instant::now();
        loop {
            if inflight <= 0 && last_change.elapsed() >= idle {
                return Ok(true);
            }
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                return Ok(false);
            }
            let wait_for = remain.min(Duration::from_millis(100));
            let ev = match tokio::time::timeout(wait_for, events.recv()).await {
                Ok(Ok(ev)) => ev,
                Ok(Err(RecvError::Lagged(_))) => continue,
                Ok(Err(RecvError::Closed)) => return Ok(false),
                Err(_) => continue, // 周期性醒来检查空闲窗口
            };
            if ev.session_id.as_deref() != Some(sid.as_str()) {
                continue;
            }
            match ev.method.as_str() {
                "Network.requestWillBeSent" => {
                    inflight += 1;
                    last_change = Instant::now();
                }
                "Network.loadingFinished" | "Network.loadingFailed" => {
                    if inflight > 0 {
                        inflight -= 1;
                    }
                    last_change = Instant::now();
                }
                _ => {}
            }
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// expose_function 守卫 + 后台回调泵
// ════════════════════════════════════════════════════════════════════════════

/// [`ChromiumTab::expose_function`] 返回的守卫:drop 即停止回调泵。
pub struct ExposedFunction {
    core: Arc<CdpCore>,
    binding: String,
    abort: AbortHandle,
}

impl ExposedFunction {
    /// 显式移除该暴露函数(停止泵 + `Runtime.removeBinding`)。
    pub async fn remove(self) -> Result<()> {
        self.abort.abort();
        let _ = self
            .core
            .send("Runtime.removeBinding", json!({ "name": self.binding }))
            .await;
        Ok(())
    }
}

impl Drop for ExposedFunction {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

async fn binding_pump(
    core: Arc<CdpCore>,
    binding: String,
    cb_obj: String,
    handler: Arc<dyn Fn(Vec<Value>) -> Result<Value> + Send + Sync>,
) {
    let mut events = core.conn.subscribe();
    let sid = core.session_id.clone();
    loop {
        let ev = match events.recv().await {
            Ok(ev) => ev,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        };
        if ev.session_id.as_deref() != Some(sid.as_str()) {
            continue;
        }
        if ev.method != "Runtime.bindingCalled" {
            continue;
        }
        if ev.params["name"].as_str() != Some(binding.as_str()) {
            continue;
        }
        let payload = ev.params["payload"].as_str().unwrap_or_default();
        let parsed: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
        let seq_s = parsed["seq"].to_string();
        let args = parsed["args"].as_array().cloned().unwrap_or_default();
        let js = match handler(args) {
            Ok(v) => {
                let val = v.to_string();
                format!(
                    "(function(){{var c=window.{cb_obj}&&window.{cb_obj}[{seq_s}];\
                     if(c){{c.res({val});delete window.{cb_obj}[{seq_s}];}}}})()"
                )
            }
            Err(e) => {
                let emsg = jstr(&e.to_string());
                format!(
                    "(function(){{var c=window.{cb_obj}&&window.{cb_obj}[{seq_s}];\
                     if(c){{c.rej(new Error({emsg}));delete window.{cb_obj}[{seq_s}];}}}})()"
                )
            }
        };
        let _ = core.eval_value(&js).await;
    }
}

// ════════════════════════════════════════════════════════════════════════════
// HAR 录制
// ════════════════════════════════════════════════════════════════════════════

/// HAR 录制句柄([`ChromiumTab::har_record`])。`stop()` 拿 [`HarLog`];drop 即停止。
pub struct HarRecorder {
    state: Arc<Mutex<HarState>>,
    abort: AbortHandle,
}

impl HarRecorder {
    /// 当前已收集的完成条目数。
    pub async fn entry_count(&self) -> usize {
        self.state.lock().await.entries.len()
    }
    /// 停止录制并产出 [`HarLog`]。
    pub async fn stop(&self) -> Result<HarLog> {
        self.abort.abort();
        let entries = self.state.lock().await.entries.clone();
        Ok(HarLog::new(entries))
    }
}

impl Drop for HarRecorder {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

/// 一份 HAR(HTTP Archive 1.2)日志。
pub struct HarLog {
    value: Value,
}

impl HarLog {
    fn new(entries: Vec<Value>) -> Self {
        Self {
            value: json!({
                "log": {
                    "version": "1.2",
                    "creator": { "name": "drission", "version": env!("CARGO_PKG_VERSION") },
                    "entries": entries,
                }
            }),
        }
    }
    /// 条目数。
    pub fn entry_count(&self) -> usize {
        self.value["log"]["entries"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0)
    }
    /// 序列化为美化 JSON。
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.value).unwrap_or_default()
    }
    /// 原始 JSON 值。
    pub fn value(&self) -> &Value {
        &self.value
    }
    /// 保存为 `.har` 文件。
    pub async fn save(&self, path: impl AsRef<Path>) -> Result<PathBuf> {
        let path = path.as_ref().to_path_buf();
        ensure_parent(&path).await?;
        tokio::fs::write(&path, self.to_json()).await?;
        Ok(path)
    }
}

struct HarState {
    entries: Vec<Value>,
    pending: HashMap<String, PendingReq>,
}

struct PendingReq {
    request: Value,
    wall_time: f64,
    ts_start: f64,
    response: Value,
    status: i64,
    status_text: String,
    mime: String,
}

async fn har_pump(core: Arc<CdpCore>, capture_bodies: bool, state: Arc<Mutex<HarState>>) {
    let mut events = core.conn.subscribe();
    let sid = core.session_id.clone();
    loop {
        let ev = match events.recv().await {
            Ok(ev) => ev,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        };
        if ev.session_id.as_deref() != Some(sid.as_str()) {
            continue;
        }
        match ev.method.as_str() {
            "Network.requestWillBeSent" => {
                let id = ev.params["requestId"].as_str().unwrap_or_default().to_string();
                if id.is_empty() {
                    continue;
                }
                let req = ev.params["request"].clone();
                let wall = ev.params["wallTime"].as_f64().unwrap_or(0.0);
                let ts = ev.params["timestamp"].as_f64().unwrap_or(0.0);
                state.lock().await.pending.insert(
                    id,
                    PendingReq {
                        request: req,
                        wall_time: wall,
                        ts_start: ts,
                        response: Value::Null,
                        status: 0,
                        status_text: String::new(),
                        mime: String::new(),
                    },
                );
            }
            "Network.responseReceived" => {
                let id = ev.params["requestId"].as_str().unwrap_or_default();
                let resp = ev.params["response"].clone();
                let mut g = state.lock().await;
                if let Some(p) = g.pending.get_mut(id) {
                    p.status = resp["status"].as_i64().unwrap_or(0);
                    p.status_text = resp["statusText"].as_str().unwrap_or_default().to_string();
                    p.mime = resp["mimeType"].as_str().unwrap_or_default().to_string();
                    p.response = resp;
                }
            }
            "Network.loadingFinished" | "Network.loadingFailed" => {
                let id = ev.params["requestId"].as_str().unwrap_or_default().to_string();
                if id.is_empty() {
                    continue;
                }
                let body = if capture_bodies && ev.method == "Network.loadingFinished" {
                    core.send("Network.getResponseBody", json!({ "requestId": id }))
                        .await
                        .ok()
                } else {
                    None
                };
                let pending = state.lock().await.pending.remove(&id);
                if let Some(p) = pending {
                    let ts_end = ev.params["timestamp"].as_f64().unwrap_or(p.ts_start);
                    let entry = build_har_entry(&p, body, ts_end);
                    state.lock().await.entries.push(entry);
                }
            }
            _ => {}
        }
    }
}

fn build_har_entry(p: &PendingReq, body: Option<Value>, ts_end: f64) -> Value {
    let url = p.request["url"].as_str().unwrap_or_default();
    let method = p.request["method"].as_str().unwrap_or("GET");
    let req_headers = headers_to_har(&p.request["headers"]);
    let resp_headers = headers_to_har(&p.response["headers"]);

    let mut content = serde_json::Map::new();
    content.insert("mimeType".into(), json!(p.mime));
    let mut content_size: i64 = -1;
    if let Some(b) = &body {
        let text = b["body"].as_str().unwrap_or_default();
        let b64 = b["base64Encoded"].as_bool().unwrap_or(false);
        content.insert("text".into(), json!(text));
        if b64 {
            content.insert("encoding".into(), json!("base64"));
        }
        content_size = text.len() as i64;
    }
    content.insert("size".into(), json!(content_size));

    let mut request = serde_json::Map::new();
    request.insert("method".into(), json!(method));
    request.insert("url".into(), json!(url));
    request.insert("httpVersion".into(), json!("HTTP/1.1"));
    request.insert("headers".into(), json!(req_headers));
    request.insert("queryString".into(), json!([]));
    request.insert("cookies".into(), json!([]));
    request.insert("headersSize".into(), json!(-1));
    let post = p.request["postData"].as_str();
    request.insert(
        "bodySize".into(),
        json!(post.map(|s| s.len() as i64).unwrap_or(-1)),
    );
    if let Some(pd) = post {
        request.insert(
            "postData".into(),
            json!({ "mimeType": "application/octet-stream", "text": pd }),
        );
    }

    let time_ms = ((ts_end - p.ts_start) * 1000.0).max(0.0);
    json!({
        "startedDateTime": epoch_to_iso8601(p.wall_time),
        "time": time_ms,
        "request": Value::Object(request),
        "response": {
            "status": p.status,
            "statusText": p.status_text,
            "httpVersion": "HTTP/1.1",
            "headers": resp_headers,
            "cookies": [],
            "content": Value::Object(content),
            "redirectURL": "",
            "headersSize": -1,
            "bodySize": -1,
        },
        "cache": {},
        "timings": { "send": 0, "wait": time_ms, "receive": 0 },
    })
}

// ════════════════════════════════════════════════════════════════════════════
// HAR 回放(routeFromHAR)
// ════════════════════════════════════════════════════════════════════════════

/// HAR 未命中时的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarNotFound {
    /// 中止请求(纯离线回放,默认)。
    Abort,
    /// 放行到真实网络(在线兜底)。
    Fallback,
}

/// HAR 回放选项(对标 Playwright `route_from_har`)。
#[derive(Debug, Clone, Copy)]
pub struct HarReplayOptions {
    /// 匹配时忽略 URL 的查询串(`?...`)。
    pub ignore_query: bool,
    /// 未命中策略。
    pub not_found: HarNotFound,
}

impl Default for HarReplayOptions {
    fn default() -> Self {
        Self {
            ignore_query: false,
            not_found: HarNotFound::Abort,
        }
    }
}

impl HarReplayOptions {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn ignore_query(mut self, yes: bool) -> Self {
        self.ignore_query = yes;
        self
    }
    pub fn not_found(mut self, nf: HarNotFound) -> Self {
        self.not_found = nf;
        self
    }
}

/// HAR 回放句柄([`ChromiumTab::route_from_har`])。`stop()` 撤销路由;drop 即停止。
pub struct HarPlayer {
    core: Arc<CdpCore>,
    abort: AbortHandle,
}

impl HarPlayer {
    /// 停止回放(中止泵 + `Fetch.disable`)。
    pub async fn stop(self) -> Result<()> {
        self.abort.abort();
        let _ = self.core.send("Fetch.disable", json!({})).await;
        Ok(())
    }
}

impl Drop for HarPlayer {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

impl HarLog {
    /// 从 `.har` 文件加载。
    pub async fn load(path: impl AsRef<Path>) -> Result<HarLog> {
        let s = tokio::fs::read_to_string(path.as_ref()).await?;
        let value: Value =
            serde_json::from_str(&s).map_err(|e| Error::Other(format!("HAR 解析失败: {e}")))?;
        Ok(HarLog { value })
    }
}

impl ChromiumTab {
    /// 用一个 `.har` 文件回放网络:命中的请求直接用 HAR 里录的响应满足,不走真实网络
    /// (对标 Playwright `route_from_har`)。**在导航前调用**。返回 [`HarPlayer`] 守卫。
    pub async fn route_from_har(
        &self,
        path: impl AsRef<Path>,
        opts: &HarReplayOptions,
    ) -> Result<HarPlayer> {
        let log = HarLog::load(path).await?;
        self.route_from_har_log(&log, opts).await
    }

    /// 用内存里的 [`HarLog`](如刚 `har_record().stop()` 拿到的)回放网络。
    pub async fn route_from_har_log(
        &self,
        log: &HarLog,
        opts: &HarReplayOptions,
    ) -> Result<HarPlayer> {
        let table = build_route_table(log.value(), opts.ignore_query);
        self.core
            .send("Fetch.enable", json!({ "patterns": [{ "urlPattern": "*" }] }))
            .await?;
        let task = tokio::spawn(har_replay_pump(
            self.core.conn.clone(),
            self.core.session_id.clone(),
            table,
            opts.ignore_query,
            opts.not_found,
        ));
        Ok(HarPlayer {
            core: self.core.clone(),
            abort: task.abort_handle(),
        })
    }
}

/// 回放响应规格。
struct RespSpec {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// HAR 回放后台泵:订阅 `Fetch.requestPaused`,命中表则伪造响应,否则按策略处理。
async fn har_replay_pump(
    conn: crate::protocol::Connection,
    session_id: String,
    table: std::collections::HashMap<String, RespSpec>,
    ignore_query: bool,
    not_found: HarNotFound,
) {
    let mut events = conn.subscribe();
    loop {
        let ev = match events.recv().await {
            Ok(ev) => ev,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        };
        if ev.session_id.as_deref() != Some(session_id.as_str()) {
            continue;
        }
        if ev.method != "Fetch.requestPaused" {
            continue;
        }
        let Some(request_id) = ev.params["requestId"].as_str() else {
            continue;
        };
        let req = &ev.params["request"];
        let url = req["url"].as_str().unwrap_or_default();
        let method = req["method"].as_str().unwrap_or("GET");
        let key = route_key(method, url, ignore_query);
        match table.get(&key) {
            Some(spec) => {
                let p = json!({
                    "requestId": request_id,
                    "responseCode": spec.status,
                    "responseHeaders": spec.headers.iter()
                        .map(|(n, v)| json!({ "name": n, "value": v }))
                        .collect::<Vec<_>>(),
                    "body": crate::util::base64_encode(&spec.body),
                });
                let _ = conn
                    .send("Fetch.fulfillRequest", p, Some(&session_id))
                    .await;
            }
            None => match not_found {
                HarNotFound::Fallback => {
                    let _ = conn
                        .send(
                            "Fetch.continueRequest",
                            json!({ "requestId": request_id }),
                            Some(&session_id),
                        )
                        .await;
                }
                HarNotFound::Abort => {
                    let _ = conn
                        .send(
                            "Fetch.failRequest",
                            json!({ "requestId": request_id, "errorReason": "BlockedByClient" }),
                            Some(&session_id),
                        )
                        .await;
                }
            },
        }
    }
}

/// 路由匹配键:`METHOD\0URL`(`ignore_query` 时去掉 `?` 之后)。
fn route_key(method: &str, url: &str, ignore_query: bool) -> String {
    let u = if ignore_query {
        url.split('?').next().unwrap_or(url)
    } else {
        url
    };
    format!("{}\u{0}{}", method.to_ascii_uppercase(), u)
}

/// 从 HAR JSON 建 `路由键 → 响应` 表(同键保留首次出现)。
fn build_route_table(har: &Value, ignore_query: bool) -> std::collections::HashMap<String, RespSpec> {
    let mut map = std::collections::HashMap::new();
    let Some(entries) = har["log"]["entries"].as_array() else {
        return map;
    };
    for e in entries {
        let method = e["request"]["method"].as_str().unwrap_or("GET");
        let url = e["request"]["url"].as_str().unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        let key = route_key(method, url, ignore_query);
        if map.contains_key(&key) {
            continue;
        }
        let resp = &e["response"];
        let status = resp["status"].as_u64().unwrap_or(200) as u16;
        let headers = har_response_headers(&resp["headers"]);
        let content = &resp["content"];
        let text = content["text"].as_str().unwrap_or_default();
        let body = if content["encoding"].as_str() == Some("base64") {
            crate::util::base64_decode(text).unwrap_or_default()
        } else {
            text.as_bytes().to_vec()
        };
        map.insert(key, RespSpec {
            status,
            headers,
            body,
        });
    }
    map
}

/// HAR 响应头数组 → 键值对,**剔除会与"已解码明文体"冲突的头**(content-encoding/length、
/// transfer-encoding、connection)与 HTTP/2 伪头(`:status` 等)。
fn har_response_headers(h: &Value) -> Vec<(String, String)> {
    h.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let name = e["name"].as_str()?.to_string();
                    let low = name.to_ascii_lowercase();
                    if low.starts_with(':')
                        || matches!(
                            low.as_str(),
                            "content-encoding"
                                | "content-length"
                                | "transfer-encoding"
                                | "connection"
                        )
                    {
                        return None;
                    }
                    Some((name, e["value"].as_str().unwrap_or_default().to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

// ════════════════════════════════════════════════════════════════════════════
// 纯函数辅助
// ════════════════════════════════════════════════════════════════════════════

/// JS 字符串字面量(用 `serde_json` 转义,安全嵌入表达式)。
fn jstr(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// CDP headers 对象 `{name:value}` → HAR headers 数组 `[{name,value}]`。
fn headers_to_har(h: &Value) -> Vec<Value> {
    h.as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| {
                    let val = v
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| v.to_string());
                    json!({ "name": k, "value": val })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 创建文件父目录(若有)。
async fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent).await?;
    }
    Ok(())
}

/// Unix 秒(可带小数)→ UTC ISO-8601(`YYYY-MM-DDThh:mm:ss.mmmZ`),不依赖 chrono。
fn epoch_to_iso8601(secs: f64) -> String {
    if secs <= 0.0 || !secs.is_finite() {
        return "1970-01-01T00:00:00.000Z".into();
    }
    let total = secs.floor() as i64;
    let millis = (((secs - total as f64) * 1000.0).round() as i64).clamp(0, 999);
    let days = total.div_euclid(86400);
    let rem = total.rem_euclid(86400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.{millis:03}Z")
}

/// 自 1970-01-01 起的天数 → `(year, month, day)`(Howard Hinnant 公历算法)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_default_is_a4_portrait() {
        let p = PdfOptions::default();
        assert!(!p.landscape && p.print_background && p.prefer_css_page_size);
        let v = p.to_params();
        assert_eq!(v["transferMode"], json!("ReturnAsBase64"));
        assert_eq!(v["paperWidth"], json!(8.27));
    }

    #[test]
    fn network_presets() {
        assert!(NetworkConditions::offline().offline);
        assert!(!NetworkConditions::slow_3g().offline);
        assert!(NetworkConditions::fast_3g().latency_ms < NetworkConditions::slow_3g().latency_ms);
    }

    #[test]
    fn device_presets_are_mobile_touch() {
        let d = Device::iphone_13();
        assert!(d.mobile && d.has_touch);
        assert_eq!(d.device_scale_factor, 3.0);
        assert_eq!((d.width, d.height), (390, 844));
        assert!(Device::pixel_7().user_agent.contains("Android"));
        assert!(Device::ipad().mobile);
    }

    #[test]
    fn jstr_quotes_and_escapes() {
        assert_eq!(jstr("a\"b"), "\"a\\\"b\"");
        assert_eq!(jstr("x"), "\"x\"");
    }

    #[test]
    fn headers_object_to_har_array() {
        let h = json!({ "Content-Type": "text/html", "X-N": 5 });
        let arr = headers_to_har(&h);
        assert_eq!(arr.len(), 2);
        assert!(arr.iter().all(|e| e.get("name").is_some() && e.get("value").is_some()));
    }

    #[test]
    fn epoch_to_iso_known_vectors() {
        assert_eq!(epoch_to_iso8601(0.0), "1970-01-01T00:00:00.000Z");
        // 2021-01-01T00:00:00Z = 1609459200
        assert_eq!(epoch_to_iso8601(1609459200.0), "2021-01-01T00:00:00.000Z");
        // 2009-02-13T23:31:30Z = 1234567890
        assert_eq!(epoch_to_iso8601(1234567890.0), "2009-02-13T23:31:30.000Z");
    }

    #[test]
    fn civil_from_days_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(31), (1970, 2, 1));
    }

    #[test]
    fn build_har_entry_shape() {
        let p = PendingReq {
            request: json!({ "url": "https://x.test/a", "method": "GET", "headers": { "Accept": "*/*" } }),
            wall_time: 1609459200.0,
            ts_start: 1.0,
            response: json!({ "headers": { "Server": "nginx" } }),
            status: 200,
            status_text: "OK".into(),
            mime: "text/html".into(),
        };
        let body = Some(json!({ "body": "hello", "base64Encoded": false }));
        let e = build_har_entry(&p, body, 1.5);
        assert_eq!(e["request"]["method"], json!("GET"));
        assert_eq!(e["response"]["status"], json!(200));
        assert_eq!(e["response"]["content"]["text"], json!("hello"));
        assert_eq!(e["response"]["content"]["size"], json!(5));
        assert_eq!(e["startedDateTime"], json!("2021-01-01T00:00:00.000Z"));
    }

    #[test]
    fn route_key_normalizes_method_and_query() {
        assert_eq!(
            route_key("get", "http://x/a?b=1", false),
            "GET\u{0}http://x/a?b=1"
        );
        assert_eq!(route_key("GET", "http://x/a?b=1", true), "GET\u{0}http://x/a");
    }

    #[test]
    fn build_route_table_and_header_filter() {
        let har = json!({"log":{"entries":[
            {"request":{"method":"GET","url":"http://x/a"},
             "response":{"status":200,"headers":[
                {"name":"Content-Type","value":"text/html"},
                {"name":"content-length","value":"5"},
                {"name":":status","value":"200"}],
              "content":{"text":"hello","mimeType":"text/html"}}}
        ]}});
        let t = build_route_table(&har, false);
        let spec = t.get(&route_key("GET", "http://x/a", false)).unwrap();
        assert_eq!(spec.status, 200);
        assert_eq!(spec.body, b"hello");
        // content-length / :status 被剔除,只剩 Content-Type
        assert_eq!(spec.headers.len(), 1);
        assert_eq!(spec.headers[0].0, "Content-Type");
    }

    #[test]
    fn har_replay_options_builder() {
        let o = HarReplayOptions::new()
            .ignore_query(true)
            .not_found(HarNotFound::Fallback);
        assert!(o.ignore_query);
        assert_eq!(o.not_found, HarNotFound::Fallback);
        assert_eq!(HarReplayOptions::default().not_found, HarNotFound::Abort);
    }
}
