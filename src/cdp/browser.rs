//! [`ChromiumBrowser`]:启动本机 Chrome/Edge/Brave/Chromium(无头或有头),或**接管**已开启
//! CDP 调试端口的浏览器 / Electron 应用。标签创建后返回 [`ChromiumTab`],由 [`CdpCore`] 驱动。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::sleep;

use crate::cdp::core::CdpCore;
use crate::cdp::locate;
use crate::cdp::tab::ChromiumTab;
use crate::protocol::Connection;
use crate::{Error, Result};

/// 一个 Chromium 浏览器(自启动或接管)。
pub struct ChromiumBrowser {
    conn: Connection,
    child: Option<tokio::process::Child>,
    user_data_dir: Option<PathBuf>,
}

impl ChromiumBrowser {
    /// 启动本机浏览器(`headless=true` 无头),开 CDP 调试端口并连接。
    ///
    /// 浏览器**默认 Google Chrome**;自动探测可执行文件(见 [`locate::chrome_path`]):
    /// `CHROME_BIN`/`DRISSION_CHROME` 环境变量 → 常见安装路径(Windows 含用户级 `%LOCALAPPDATA%`)
    /// → Windows 注册表 `App Paths` → 系统 `PATH`。要指定具体浏览器用 [`Self::launch_with`]。
    pub async fn launch(headless: bool) -> Result<Self> {
        Self::launch_with(locate::chrome_path()?, headless).await
    }

    /// 定位本机 Chrome/Edge/Brave/Chromium 可执行文件(不启动)。便于诊断“为何没找到浏览器”。
    pub fn find_chrome() -> Result<PathBuf> {
        locate::chrome_path()
    }

    /// 用**指定的可执行文件**启动浏览器(`headless=true` 无头),开 CDP 调试端口并连接。
    /// 当自动探测找不到、或要强制使用某个浏览器(Chrome/Edge/Brave/Chromium)时用它。
    pub async fn launch_with(exe: impl AsRef<Path>, headless: bool) -> Result<Self> {
        let exe = exe.as_ref();
        let dir =
            std::env::temp_dir().join(format!("drission-cdp-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir)
            .map_err(|e| Error::msg(format!("CDP: 建 user-data-dir 失败: {e}")))?;
        let mut cmd = tokio::process::Command::new(exe);
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
        cmd.stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let child = cmd
            .spawn()
            .map_err(|e| Error::msg(format!("CDP: 启动浏览器失败({}): {e}", exe.display())))?;

        // Chrome 启动调试服务后把端口写到 user-data-dir/DevToolsActivePort(首行)。
        let port = wait_for_devtools_port(&dir.join("DevToolsActivePort")).await?;
        let ws_url = browser_ws_url(&format!("http://127.0.0.1:{port}")).await?;
        let ws = crate::transport::ws_connect(&ws_url).await?;
        Ok(Self {
            conn: Connection::from_ws(ws),
            child: Some(child),
            user_data_dir: Some(dir),
        })
    }

    /// 接管一个已开启 CDP 调试端口的浏览器 / Electron 应用。
    /// `debug_http_url` 形如 `http://127.0.0.1:9222`(对方需以 `--remote-debugging-port=9222` 启动)。
    pub async fn connect(debug_http_url: &str) -> Result<Self> {
        let ws_url = browser_ws_url(debug_http_url.trim_end_matches('/')).await?;
        let ws = crate::transport::ws_connect(&ws_url).await?;
        Ok(Self {
            conn: Connection::from_ws(ws),
            child: None,
            user_data_dir: None,
        })
    }

    /// 新建一个标签页并附着,返回可驱动的 [`ChromiumTab`]。
    pub async fn new_tab(&self, url: &str) -> Result<ChromiumTab> {
        let r = self
            .conn
            .send("Target.createTarget", json!({ "url": url }), None)
            .await?;
        let target_id = r["targetId"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: 创建标签无 targetId"))?
            .to_string();
        self.attach(target_id).await
    }

    /// 附着到最近一个 page target(接管现有标签用)。
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
            .send(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                None,
            )
            .await?;
        let session_id = a["sessionId"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: 附着无 sessionId"))?
            .to_string();
        let core = CdpCore::new(self.conn.clone(), session_id, target_id);
        // 开启页面/运行时事件域(忽略已开启的错误)。
        let _ = core.send("Page.enable", json!({})).await;
        let _ = core.send("Runtime.enable", json!({})).await;
        Ok(ChromiumTab::new(core))
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

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
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
    Err(Error::msg(
        "CDP: 等待 DevToolsActivePort 超时(浏览器未就绪)",
    ))
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
