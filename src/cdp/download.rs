//! CDP 后端的**下载管理** [`ChromiumDownloads`](对齐 camoufox 后端的 `Downloads`)。
//!
//! 在一次性的 [`ChromiumTab::wait_download`](crate::cdp::ChromiumTab::wait_download) 之上,提供
//! **多任务并发跟踪 + 任务列表 + 进度 + 自定义重命名**。
//!
//! **实现:CDP 原生事件**(而非 camoufox 的文件系统轮询)。Chrome 经 `Browser.setDownloadBehavior`
//! (`eventsEnabled:true`)后**可靠下发** `Page.downloadWillBegin` / `Page.downloadProgress`,故按 `guid`
//! 聚合即可得到任务列表与实时进度(比轮询更准、自带 received/total 字节)。
//!
//! 需先设下载目录:`ChromiumOptions::download_path`(启动时)或
//! [`tab.set_download_path`](crate::cdp::ChromiumTab::set_download_path)(运行时)。
//!
//! ```ignore
//! let dl = tab.downloads();
//! dl.start().await?;                                  // 务必在触发下载之前
//! tab.ele("#export").await?.click().await?;
//! let m = dl.wait_done(Duration::from_secs(30)).await?.unwrap();
//! println!("{:?} ({} 字节)", m.path, m.downloaded_bytes());
//! m.save_as("/tmp/renamed.csv").await?;              // 自定义重命名/移动
//! dl.stop().await?;
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::Mutex;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::AbortHandle;
use tokio::time::{Instant, sleep};

use crate::cdp::core::CdpCore;
use crate::protocol::Connection;
use crate::{Error, Result};

/// 一个下载任务的状态(CDP `Page.downloadProgress.state`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    /// 进行中。
    InProgress,
    /// 已完成(最终文件就绪)。
    Completed,
    /// 被取消。
    Canceled,
}

/// 一个下载任务(对齐 camoufox `DownloadMission`)。
#[derive(Debug, Clone)]
pub struct DownloadMission {
    /// CDP 分配的下载 id(同一下载的 willBegin/progress 共享)。
    pub guid: String,
    /// 下载来源 URL。
    pub url: String,
    /// 建议文件名。
    pub suggested_filename: String,
    /// 落盘路径(下载目录 + 文件名)。
    pub path: PathBuf,
    /// 状态(进行中 / 已完成 / 取消)。
    pub state: DownloadState,
    /// 已接收字节数。
    pub received_bytes: u64,
    /// 总字节数(未知时为 0)。
    pub total_bytes: u64,
}

impl DownloadMission {
    /// 是否已结束(完成或取消)。
    pub fn is_finished(&self) -> bool {
        matches!(
            self.state,
            DownloadState::Completed | DownloadState::Canceled
        )
    }

    /// 是否成功完成。
    pub fn succeeded(&self) -> bool {
        matches!(self.state, DownloadState::Completed)
    }

    /// 已下载字节数(CDP 事件直接给出,无需读文件)。
    pub fn downloaded_bytes(&self) -> u64 {
        self.received_bytes
    }

    /// 把已下载的文件移动/重命名到 `dest`(跨盘自动回退为复制+删除)。返回最终路径。
    pub async fn save_as(&self, dest: impl AsRef<Path>) -> Result<PathBuf> {
        let dest = dest.as_ref().to_path_buf();
        if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(parent).await?;
        }
        if tokio::fs::rename(&self.path, &dest).await.is_err() {
            tokio::fs::copy(&self.path, &dest).await?;
            let _ = tokio::fs::remove_file(&self.path).await;
        }
        Ok(dest)
    }
}

/// 下载跟踪共享状态(放在 [`CdpCore`])。后台任务按 `guid` 聚合事件,各 `wait_*` 据此返回并去重。
pub(crate) struct DownloadShared {
    /// 本次跟踪期间见到的任务(按首次出现顺序)。
    pub(crate) missions: Arc<Mutex<Vec<DownloadMission>>>,
    /// 已被 `wait_new` 返回过的 guid(避免重复返回)。
    pub(crate) new_returned: Arc<Mutex<HashSet<String>>>,
    /// 已被 `wait_done` 返回过的 guid(避免重复返回)。
    pub(crate) done_returned: Arc<Mutex<HashSet<String>>>,
    /// 是否在跟踪。
    pub(crate) running: bool,
    /// 后台事件聚合任务的 abort 句柄(`CdpCore` Drop 时中止)。
    pub(crate) abort: Option<AbortHandle>,
}

impl Default for DownloadShared {
    fn default() -> Self {
        Self {
            missions: Arc::new(Mutex::new(Vec::new())),
            new_returned: Arc::new(Mutex::new(HashSet::new())),
            done_returned: Arc::new(Mutex::new(HashSet::new())),
            running: false,
            abort: None,
        }
    }
}

/// `tab.downloads()` 返回的下载管理句柄(对齐 camoufox `Downloads`)。
pub struct ChromiumDownloads {
    core: Arc<CdpCore>,
}

impl ChromiumDownloads {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self { core }
    }

    /// 开始跟踪下载:允许下载 + 开事件 + 启动后台聚合任务。**务必在触发下载之前**调用。
    /// 需先用 `ChromiumOptions::download_path` 或 `tab.set_download_path` 设置下载目录。
    pub async fn start(&self) -> Result<()> {
        let dir = self.core.download_dir().ok_or_else(|| {
            Error::Other(
                "downloads(): 需先用 ChromiumOptions::download_path 或 tab.set_download_path 设置下载目录"
                    .into(),
            )
        })?;
        self.stop().await?;
        let _ = std::fs::create_dir_all(&dir);
        self.core
            .send(
                "Browser.setDownloadBehavior",
                json!({ "behavior": "allow", "downloadPath": dir.display().to_string(), "eventsEnabled": true }),
            )
            .await?;
        let missions = {
            let g = self.core.downloads.lock().await;
            g.missions.lock().await.clear();
            g.new_returned.lock().await.clear();
            g.done_returned.lock().await.clear();
            g.missions.clone()
        };
        let task = tokio::spawn(download_pump(
            self.core.conn.clone(),
            self.core.session_id.clone(),
            dir,
            missions,
        ));
        let mut g = self.core.downloads.lock().await;
        g.running = true;
        g.abort = Some(task.abort_handle());
        Ok(())
    }

    /// 是否在跟踪。
    pub async fn listening(&self) -> bool {
        self.core.downloads.lock().await.running
    }

    /// 当前所有任务快照(本次跟踪期间见到的,含实时状态/进度)。
    pub async fn missions(&self) -> Vec<DownloadMission> {
        let m = self.core.downloads.lock().await.missions.clone();
        m.lock().await.clone()
    }

    /// 等待**下一个新出现**的下载(可能仍在进行中)。超时返回 `None`。
    pub async fn wait_new(&self, timeout: Duration) -> Result<Option<DownloadMission>> {
        self.ensure_active().await?;
        let deadline = Instant::now() + timeout;
        loop {
            for m in self.missions().await {
                let seen = self.core.downloads.lock().await.new_returned.clone();
                if seen.lock().await.insert(m.guid.clone()) {
                    return Ok(Some(m));
                }
            }
            if self.expired(deadline).await {
                return Ok(None);
            }
        }
    }

    /// 等待**下一个完成**的下载(最终文件就绪)。超时返回 `None`。
    pub async fn wait_done(&self, timeout: Duration) -> Result<Option<DownloadMission>> {
        self.ensure_active().await?;
        let deadline = Instant::now() + timeout;
        loop {
            for m in self.missions().await {
                if !m.succeeded() {
                    continue;
                }
                let returned = self.core.downloads.lock().await.done_returned.clone();
                if returned.lock().await.insert(m.guid.clone()) {
                    return Ok(Some(m));
                }
            }
            if self.expired(deadline).await {
                return Ok(None);
            }
        }
    }

    /// 等待 `count` 个下载**完成**(并发场景);到点不足返回已完成的那些(不报错)。
    pub async fn wait_count_done(
        &self,
        count: usize,
        timeout: Duration,
    ) -> Result<Vec<DownloadMission>> {
        let deadline = Instant::now() + timeout;
        let mut out = Vec::with_capacity(count);
        while out.len() < count {
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                break;
            }
            match self.wait_done(remain).await? {
                Some(m) => out.push(m),
                None => break,
            }
        }
        Ok(out)
    }

    /// 停止跟踪并清空状态(中止后台任务)。
    pub async fn stop(&self) -> Result<()> {
        let (abort, missions, new_r, done_r) = {
            let mut g = self.core.downloads.lock().await;
            g.running = false;
            (
                g.abort.take(),
                g.missions.clone(),
                g.new_returned.clone(),
                g.done_returned.clone(),
            )
        };
        missions.lock().await.clear();
        new_r.lock().await.clear();
        done_r.lock().await.clear();
        if let Some(a) = abort {
            a.abort();
        }
        Ok(())
    }

    async fn ensure_active(&self) -> Result<()> {
        if self.core.downloads.lock().await.running {
            Ok(())
        } else {
            Err(Error::Other("尚未调用 downloads().start()".into()))
        }
    }

    /// 是否到截止时间(否则小睡一拍再轮询)。
    async fn expired(&self, deadline: Instant) -> bool {
        if Instant::now() >= deadline {
            return true;
        }
        sleep(Duration::from_millis(60)).await;
        false
    }
}

/// 后台任务:订阅连接事件,按 `guid` 聚合 `Page.downloadWillBegin` / `Page.downloadProgress`。
async fn download_pump(
    conn: Connection,
    session_id: String,
    dir: PathBuf,
    missions: Arc<Mutex<Vec<DownloadMission>>>,
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
        match ev.method.as_str() {
            "Page.downloadWillBegin" => {
                let guid = ev.params["guid"].as_str().unwrap_or_default().to_string();
                if guid.is_empty() {
                    continue;
                }
                let name = ev.params["suggestedFilename"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let url = ev.params["url"].as_str().unwrap_or_default().to_string();
                let mut g = missions.lock().await;
                if !g.iter().any(|m| m.guid == guid) {
                    g.push(DownloadMission {
                        path: dir.join(&name),
                        guid,
                        url,
                        suggested_filename: name,
                        state: DownloadState::InProgress,
                        received_bytes: 0,
                        total_bytes: 0,
                    });
                }
            }
            "Page.downloadProgress" => {
                let guid = ev.params["guid"].as_str().unwrap_or_default();
                if guid.is_empty() {
                    continue;
                }
                let state = map_state(ev.params["state"].as_str().unwrap_or(""));
                let received = ev.params["receivedBytes"].as_f64().unwrap_or(0.0) as u64;
                let total = ev.params["totalBytes"].as_f64().unwrap_or(0.0) as u64;
                let mut g = missions.lock().await;
                if let Some(m) = g.iter_mut().find(|m| m.guid == guid) {
                    m.received_bytes = received;
                    m.total_bytes = total;
                    m.state = state;
                }
            }
            _ => {}
        }
    }
}

/// CDP `Page.downloadProgress.state` 字符串 → [`DownloadState`]。
fn map_state(s: &str) -> DownloadState {
    match s {
        "completed" => DownloadState::Completed,
        "canceled" => DownloadState::Canceled,
        _ => DownloadState::InProgress,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_mapping() {
        assert_eq!(map_state("completed"), DownloadState::Completed);
        assert_eq!(map_state("canceled"), DownloadState::Canceled);
        assert_eq!(map_state("inProgress"), DownloadState::InProgress);
        assert_eq!(map_state(""), DownloadState::InProgress);
    }

    #[test]
    fn mission_finished_and_succeeded() {
        let mk = |state| DownloadMission {
            guid: "g".into(),
            url: "u".into(),
            suggested_filename: "f.bin".into(),
            path: PathBuf::from("/tmp/f.bin"),
            state,
            received_bytes: 10,
            total_bytes: 10,
        };
        let done = mk(DownloadState::Completed);
        assert!(done.is_finished() && done.succeeded());
        assert_eq!(done.downloaded_bytes(), 10);
        let canceled = mk(DownloadState::Canceled);
        assert!(canceled.is_finished() && !canceled.succeeded());
        let prog = mk(DownloadState::InProgress);
        assert!(!prog.is_finished() && !prog.succeeded());
    }
}
