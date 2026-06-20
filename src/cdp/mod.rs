//! **Chromium 后端**(Chrome / Edge / Brave / Electron 应用),经 **CDP**(Chrome DevTools Protocol)。
//!
//! 与默认的 Camoufox/Juggler 后端并行:用于**驱动或接管 Chromium 系浏览器**,以及**控制 Electron 桌面
//! 应用**(它们都内置 CDP)。CDP 线消息格式(`id`/`method`/`params`/`sessionId` + `result`/`error`/事件)
//! 与 Juggler 高度一致,故**复用** [`crate::protocol::Connection`] 的请求/响应/事件机制,只换方法名与目标管理。
//!
//! ```no_run
//! use drission::prelude::*;
//! # async fn f() -> drission::Result<()> {
//! // 启动 Chrome(headless),或 connect("http://127.0.0.1:9222") 接管已开的 Chrome / Electron
//! let browser = ChromiumBrowser::launch(true).await?;
//! let tab = browser.new_tab("https://example.com").await?;
//! println!("{}", tab.title().await?);
//! let v = tab.run_js("1+2").await?;          // Runtime.evaluate
//! browser.quit().await?;
//! # Ok(()) }
//! ```
//!
//! 现状:MVP——启动/接管、新标签、导航、`run_js`、标题/URL、截图、点击/输入(JS 合成)。
//! 后续补:原生 `Input.dispatch*` 拟人输入、元素句柄、网络监听/拦截、下载等(对齐 Juggler 后端能力)。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::sleep;

use crate::protocol::Connection;
use crate::util::base64_decode;
use crate::{Error, Result};

/// 一个 Chromium 浏览器(自启动或接管)。
pub struct ChromiumBrowser {
    conn: Connection,
    child: Option<tokio::process::Child>,
    user_data_dir: Option<PathBuf>,
}

impl ChromiumBrowser {
    /// 启动本机 Chrome/Edge(`headless=true` 无头),开 CDP 调试端口并连接。
    /// 浏览器可执行文件:`CHROME_BIN` 环境变量优先,否则探测常见安装路径。
    pub async fn launch(headless: bool) -> Result<Self> {
        let exe = chrome_path()?;
        let dir = std::env::temp_dir().join(format!("drission-cdp-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).map_err(|e| Error::msg(format!("CDP: 建 user-data-dir 失败: {e}")))?;
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", dir.display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-background-networking")
            .arg("--disable-features=Translate,OptimizationHints")
            .arg("about:blank");
        if headless {
            cmd.arg("--headless=new").arg("--disable-gpu");
        }
        cmd.stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
        let child = cmd.spawn().map_err(|e| Error::msg(format!("CDP: 启动浏览器失败({}): {e}", exe.display())))?;

        // Chrome 启动调试服务后把端口写到 user-data-dir/DevToolsActivePort(首行)。
        let port = wait_for_devtools_port(&dir.join("DevToolsActivePort")).await?;
        let ws_url = browser_ws_url(&format!("http://127.0.0.1:{port}")).await?;
        let ws = crate::transport::ws_connect(&ws_url).await?;
        Ok(Self { conn: Connection::from_ws(ws), child: Some(child), user_data_dir: Some(dir) })
    }

    /// 接管一个已开启 CDP 调试端口的浏览器 / Electron 应用。
    /// `debug_http_url` 形如 `http://127.0.0.1:9222`(对方需以 `--remote-debugging-port=9222` 启动)。
    pub async fn connect(debug_http_url: &str) -> Result<Self> {
        let ws_url = browser_ws_url(debug_http_url.trim_end_matches('/')).await?;
        let ws = crate::transport::ws_connect(&ws_url).await?;
        Ok(Self { conn: Connection::from_ws(ws), child: None, user_data_dir: None })
    }

    /// 新建一个标签页并附着,返回可驱动的 [`ChromiumTab`]。
    pub async fn new_tab(&self, url: &str) -> Result<ChromiumTab> {
        let r = self
            .conn
            .send("Target.createTarget", json!({ "url": url }), None)
            .await?;
        let target_id = r["targetId"].as_str().ok_or_else(|| Error::msg("CDP: 创建标签无 targetId"))?.to_string();
        self.attach(target_id).await
    }

    /// 附着到已存在的 page target(接管现有标签用)。返回首个 page 类型的标签。
    pub async fn latest_tab(&self) -> Result<ChromiumTab> {
        let r = self.conn.send("Target.getTargets", json!({}), None).await?;
        let targets = r["targetInfos"].as_array().cloned().unwrap_or_default();
        let page = targets
            .iter()
            .rev()
            .find(|t| t["type"].as_str() == Some("page"))
            .and_then(|t| t["targetId"].as_str())
            .ok_or_else(|| Error::msg("CDP: 没有可附着的 page 标签"))?
            .to_string();
        self.attach(page).await
    }

    async fn attach(&self, target_id: String) -> Result<ChromiumTab> {
        let a = self
            .conn
            .send("Target.attachToTarget", json!({ "targetId": target_id, "flatten": true }), None)
            .await?;
        let session_id = a["sessionId"].as_str().ok_or_else(|| Error::msg("CDP: 附着无 sessionId"))?.to_string();
        let tab = ChromiumTab { conn: self.conn.clone(), session_id, target_id };
        // 开启页面/运行时事件域(忽略已开启的错误)。
        let _ = tab.conn.send("Page.enable", json!({}), Some(&tab.session_id)).await;
        let _ = tab.conn.send("Runtime.enable", json!({}), Some(&tab.session_id)).await;
        Ok(tab)
    }

    /// 优雅关闭:`Browser.close` + 杀子进程(自启动时)+ 清临时 profile。
    pub async fn quit(mut self) -> Result<()> {
        let _ = self.conn.send("Browser.close", json!({}), None).await;
        if let Some(mut c) = self.child.take() {
            let _ = c.kill().await;
        }
        if let Some(d) = self.user_data_dir.take() {
            let _ = std::fs::remove_dir_all(&d);
        }
        Ok(())
    }
}

/// 一个 Chromium 标签页(或附着的 Electron 窗口)。
pub struct ChromiumTab {
    conn: Connection,
    session_id: String,
    #[allow(dead_code)]
    target_id: String,
}

impl ChromiumTab {
    /// 导航到 `url` 并等待 `load` 事件(最多 30s)。
    pub async fn get(&self, url: &str) -> Result<()> {
        let mut events = self.conn.subscribe();
        self.conn.send("Page.navigate", json!({ "url": url }), Some(&self.session_id)).await?;
        let sid = self.session_id.clone();
        let _ = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match events.recv().await {
                    Ok(ev) if ev.method == "Page.loadEventFired" && ev.session_id.as_deref() == Some(&sid) => break,
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
        })
        .await;
        Ok(())
    }

    /// 在页面执行 JS 表达式,返回结果值(`Runtime.evaluate`,自动 await Promise)。
    pub async fn run_js(&self, expression: &str) -> Result<Value> {
        let r = self
            .conn
            .send(
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
                Some(&self.session_id),
            )
            .await?;
        if let Some(exc) = r.get("exceptionDetails") {
            let msg = exc["exception"]["description"].as_str().or_else(|| exc["text"].as_str()).unwrap_or("JS 异常");
            return Err(Error::msg(format!("CDP JS 异常: {msg}")));
        }
        Ok(r["result"]["value"].clone())
    }

    /// 页面标题。
    pub async fn title(&self) -> Result<String> {
        Ok(self.run_js("document.title").await?.as_str().unwrap_or("").to_string())
    }

    /// 当前 URL。
    pub async fn url(&self) -> Result<String> {
        Ok(self.run_js("location.href").await?.as_str().unwrap_or("").to_string())
    }

    /// 元素文本(CSS 选择器;MVP 走 JS,后续补元素句柄)。`None`=未找到。
    pub async fn ele_text(&self, css: &str) -> Result<Option<String>> {
        let v = self
            .run_js(&format!(
                "(function(){{var e=document.querySelector({}); return e?e.textContent:null;}})()",
                json!(css)
            ))
            .await?;
        Ok(v.as_str().map(|s| s.to_string()))
    }

    /// 点击元素(MVP:JS `.click()`;后续补 `Input.dispatchMouseEvent` 可信点击)。
    pub async fn click(&self, css: &str) -> Result<bool> {
        let v = self
            .run_js(&format!(
                "(function(){{var e=document.querySelector({}); if(!e)return false; e.click(); return true;}})()",
                json!(css)
            ))
            .await?;
        Ok(v.as_bool().unwrap_or(false))
    }

    /// 给输入框填值(React 兼容:原生 value setter + 派发 input/change)。
    pub async fn input(&self, css: &str, text: &str) -> Result<bool> {
        let v = self
            .run_js(&format!(
                r#"(function(){{var e=document.querySelector({sel}); if(!e)return false;
  var p=e.tagName==='TEXTAREA'?window.HTMLTextAreaElement.prototype:window.HTMLInputElement.prototype;
  var set=Object.getOwnPropertyDescriptor(p,'value').set; set.call(e,{val});
  e.dispatchEvent(new Event('input',{{bubbles:true}})); e.dispatchEvent(new Event('change',{{bubbles:true}}));
  return true;}})()"#,
                sel = json!(css),
                val = json!(text)
            ))
            .await?;
        Ok(v.as_bool().unwrap_or(false))
    }

    /// 整页可视区截图(PNG 字节)。
    pub async fn screenshot_bytes(&self) -> Result<Vec<u8>> {
        let r = self
            .conn
            .send("Page.captureScreenshot", json!({ "format": "png" }), Some(&self.session_id))
            .await?;
        let data = r["data"].as_str().ok_or_else(|| Error::msg("CDP: 无截图数据"))?;
        base64_decode(data).ok_or_else(|| Error::msg("CDP: 截图 base64 解码失败"))
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 定位 Chrome/Edge/Brave/Chromium 可执行文件:`CHROME_BIN` 优先,否则按平台探测常见路径。
fn chrome_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("CHROME_BIN") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Ok(pb);
        }
    }
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ]
    } else if cfg!(target_os = "windows") {
        &[
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        ]
    } else {
        &[
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/usr/bin/microsoft-edge",
        ]
    };
    for c in candidates {
        let p = Path::new(c);
        if p.exists() {
            return Ok(p.to_path_buf());
        }
    }
    Err(Error::msg("CDP: 未找到 Chrome/Edge,可设 CHROME_BIN 指定可执行文件路径"))
}

/// 轮询读取 `DevToolsActivePort` 文件首行的端口号(Chrome 启动调试服务后写入)。
async fn wait_for_devtools_port(file: &Path) -> Result<u16> {
    for _ in 0..100 {
        if let Ok(s) = std::fs::read_to_string(file) {
            if let Some(line) = s.lines().next() {
                if let Ok(port) = line.trim().parse::<u16>() {
                    return Ok(port);
                }
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
    Err(Error::msg("CDP: 等待 DevToolsActivePort 超时(浏览器未就绪)"))
}

/// 查询 `{http}/json/version` 拿浏览器级 WebSocket 调试端点。
async fn browser_ws_url(http: &str) -> Result<String> {
    let body: Value = reqwest::get(format!("{http}/json/version"))
        .await
        .map_err(|e| Error::msg(format!("CDP: 访问 {http}/json/version 失败: {e}")))?
        .json()
        .await
        .map_err(|e| Error::msg(format!("CDP: 解析 /json/version 失败: {e}")))?;
    body["webSocketDebuggerUrl"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| Error::msg("CDP: /json/version 无 webSocketDebuggerUrl"))
}
