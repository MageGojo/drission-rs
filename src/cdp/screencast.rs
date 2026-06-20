//! CDP 后端的**录像/连拍** [`ChromiumScreencast`](对齐 camoufox `Screencast`,大道至简)。
//!
//! 实现 = 后台任务按帧间隔反复 `Page.captureScreenshot` 截视口存为 `frame_<n>.png`(Imgs 模式),
//! 与主线程操作经连接多路复用、互不阻塞。`stop()` 返回帧目录。要合成 mp4 自行用 ffmpeg。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::task::AbortHandle;
use tokio::time::sleep;

use crate::cdp::core::CdpCore;
use crate::protocol::Connection;
use crate::{Error, Result};

/// 录像共享状态(放 [`CdpCore`])。
#[derive(Default)]
pub(crate) struct ScreencastShared {
    pub(crate) running: bool,
    pub(crate) abort: Option<AbortHandle>,
    pub(crate) dir: Option<PathBuf>,
}

/// 录像句柄(`tab.screencast()`)。
pub struct ChromiumScreencast {
    core: Arc<CdpCore>,
}

impl ChromiumScreencast {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self { core }
    }

    /// 开始连拍到 `save_dir`(每秒 `fps` 帧,Imgs 模式存 PNG 帧)。
    pub async fn start(&self, save_dir: impl Into<PathBuf>, fps: u32) -> Result<()> {
        self.stop().await?;
        let dir = save_dir.into();
        std::fs::create_dir_all(&dir)
            .map_err(|e| Error::msg(format!("CDP: 建录像目录失败: {e}")))?;
        let interval = Duration::from_secs_f64(1.0 / (fps.max(1) as f64));
        let task = tokio::spawn(cast_pump(
            self.core.conn.clone(),
            self.core.session_id.clone(),
            dir.clone(),
            interval,
        ));
        let mut g = self.core.screencast.lock().await;
        g.running = true;
        g.abort = Some(task.abort_handle());
        g.dir = Some(dir);
        Ok(())
    }

    /// 是否正在录制。
    pub async fn is_recording(&self) -> bool {
        self.core.screencast.lock().await.running
    }

    /// 停止录制,返回帧目录(未开始则返回空路径)。
    pub async fn stop(&self) -> Result<PathBuf> {
        let mut g = self.core.screencast.lock().await;
        g.running = false;
        if let Some(a) = g.abort.take() {
            a.abort();
        }
        Ok(g.dir.clone().unwrap_or_default())
    }
}

/// 后台连拍任务:按间隔截视口写 `frame_<n>.png`。
async fn cast_pump(conn: Connection, session_id: String, dir: PathBuf, interval: Duration) {
    let mut n: u64 = 0;
    loop {
        if let Ok(r) = conn
            .send(
                "Page.captureScreenshot",
                json!({ "format": "png" }),
                Some(&session_id),
            )
            .await
        {
            if let Some(data) = r["data"].as_str() {
                if let Some(bytes) = crate::util::base64_decode(data) {
                    let path = dir.join(format!("frame_{n:05}.png"));
                    let _ = std::fs::write(&path, &bytes);
                    n += 1;
                }
            }
        }
        sleep(interval).await;
    }
}
