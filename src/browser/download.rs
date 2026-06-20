//! 下载管理([`Downloads`],对标 DrissionPage 的 DownloadManager)。
//!
//! 在一次性的 [`Tab::wait_download`](crate::browser::Tab::wait_download) 之上,提供**多任务并发跟踪 +
//! 任务列表 + 自定义重命名 + 进度**。
//!
//! **实现:文件系统跟踪(而非 Juggler 事件)**。实测当前 Camoufox/Juggler 构建**不下发**
//! `Browser.downloadCreated`/`downloadFinished` 事件,但只要设了下载目录,文件会**可靠落盘**:
//! Firefox 下载中写入 `<name>.part`、完成后原子改名为 `<name>`。因此本模块通过**轮询下载目录**
//! 跟踪下载——比事件更稳,且"进度"可直接由 `.part` 文件大小得到。
//!
//! 必须先用 [`BrowserOptions::download_path`](crate::launcher::BrowserOptions::download_path) 设下载目录;
//! `start()` 会记录目录现有文件作为基线(不计入),之后出现的新文件即本次下载。
//!
//! ```ignore
//! let dl = tab.downloads();
//! dl.start().await?;                                  // 务必在触发下载之前(记录基线)
//! tab.ele("#export").await?.click().await?;
//! let m = dl.wait_done(Duration::from_secs(30)).await?.unwrap();
//! println!("{:?} ({} 字节)", m.path, m.downloaded_bytes().await);
//! m.save_as("/tmp/renamed.csv").await?;               // 自定义重命名/移动
//! dl.stop().await?;
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::browser::tab::Tab;
use crate::{Error, Result};

/// 一个下载任务的状态(文件系统跟踪能区分的两态)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    /// 进行中(存在 `<name>.part` 临时文件)。
    Downloading,
    /// 已完成(最终文件就绪,无 `.part`)。
    Finished,
}

/// 一个下载任务(对应下载目录里的一个新文件)。
#[derive(Debug, Clone)]
pub struct DownloadMission {
    /// 文件名(不含 `.part`)。
    pub suggested_filename: String,
    /// 落盘路径(下载目录 + 文件名)。
    pub path: PathBuf,
    /// 状态(进行中 / 已完成)。
    pub state: DownloadState,
}

impl DownloadMission {
    /// 是否已完成。
    pub fn is_finished(&self) -> bool {
        matches!(self.state, DownloadState::Finished)
    }

    /// 是否成功完成(文件系统跟踪下等价于 [`is_finished`](Self::is_finished))。
    pub fn succeeded(&self) -> bool {
        self.is_finished()
    }

    /// 当前已下载字节数:完成文件读 `path`,进行中读同目录 `<name>.part`;读不到返回 0。
    ///
    /// 可用于进度展示;无 `content-length` 时无法换算百分比(总大小未知)。
    pub async fn downloaded_bytes(&self) -> u64 {
        if let Ok(m) = tokio::fs::metadata(&self.path).await {
            return m.len();
        }
        let part = self
            .path
            .with_file_name(format!("{}.part", self.suggested_filename));
        tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0)
    }

    /// 把已下载的文件移动/重命名到 `dest`(自定义重命名;跨盘自动回退为复制+删除)。返回最终路径。
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

/// 下载跟踪共享状态(放在 `TabCore`;`start` 记录基线,各 `wait_*` 据此识别新文件并去重返回)。
pub(crate) struct DownloadShared {
    pub active: AtomicBool,
    /// `start()` 时目录里已有的文件名(不计入本次下载)。
    pub baseline: Mutex<HashSet<String>>,
    /// 已被 `wait_done` 返回过的完成文件名(避免重复返回)。
    pub done_returned: Mutex<HashSet<String>>,
    /// 已被 `wait_new` 返回过的新文件名(避免重复返回)。
    pub new_returned: Mutex<HashSet<String>>,
}

impl DownloadShared {
    pub(crate) fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            baseline: Mutex::new(HashSet::new()),
            done_returned: Mutex::new(HashSet::new()),
            new_returned: Mutex::new(HashSet::new()),
        }
    }
}

/// `tab.downloads()` 返回的下载管理句柄(对标 DP DownloadManager)。
///
/// 即用即弃,持有一个 [`Tab`] 克隆(共享内核)。不同 `downloads()` 句柄共享同一跟踪状态。
pub struct Downloads {
    tab: Tab,
}

impl Downloads {
    pub(crate) fn new(tab: Tab) -> Self {
        Self { tab }
    }

    fn dir(&self) -> Option<PathBuf> {
        self.tab.core.download_path.clone()
    }

    /// 开始跟踪下载:记录下载目录现有文件为基线。幂等地把状态置为"在跟踪"。
    /// **务必在触发下载之前**调用。需先用
    /// [`BrowserOptions::download_path`](crate::launcher::BrowserOptions::download_path) 设下载目录。
    pub async fn start(&self) -> Result<()> {
        let dir = self.dir().ok_or_else(|| {
            Error::Other("downloads(): 需先用 BrowserOptions::download_path 设置下载目录".into())
        })?;
        let shared = &self.tab.core.downloads;
        *shared.baseline.lock().await = list_dir_files(&dir).await;
        shared.done_returned.lock().await.clear();
        shared.new_returned.lock().await.clear();
        shared.active.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// 是否在跟踪。
    pub fn listening(&self) -> bool {
        self.tab.core.downloads.active.load(Ordering::SeqCst)
    }

    /// 扫描下载目录,返回本次新增的任务(排除基线;`.part` → 进行中,最终文件 → 已完成)。
    async fn scan(&self) -> Vec<DownloadMission> {
        let Some(dir) = self.dir() else {
            return Vec::new();
        };
        let baseline = self.tab.core.downloads.baseline.lock().await.clone();
        scan_new_files(&dir, &baseline).await
    }

    /// 等待**下一个新出现**的下载(可能仍在进行中)。超时返回 `None`。
    pub async fn wait_new(&self, timeout: Duration) -> Result<Option<DownloadMission>> {
        self.ensure_active()?;
        let deadline = Instant::now() + timeout;
        loop {
            for m in self.scan().await {
                let mut seen = self.tab.core.downloads.new_returned.lock().await;
                if seen.insert(m.suggested_filename.clone()) {
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
        self.ensure_active()?;
        let deadline = Instant::now() + timeout;
        loop {
            for m in self.scan().await {
                if m.state != DownloadState::Finished {
                    continue;
                }
                {
                    let seen = self.tab.core.downloads.done_returned.lock().await;
                    if seen.contains(&m.suggested_filename) {
                        continue;
                    }
                }
                // 稳定性兜底(应对极少数不经 .part 直接写入的情况):大小连续两次一致才算完成。
                if !size_stable(&m.path).await {
                    continue;
                }
                self.tab
                    .core
                    .downloads
                    .done_returned
                    .lock()
                    .await
                    .insert(m.suggested_filename.clone());
                return Ok(Some(m));
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

    /// 当前所有任务快照(本次新增的文件,含状态)。
    pub async fn missions(&self) -> Vec<DownloadMission> {
        self.scan().await
    }

    /// 停止跟踪并清空状态。
    pub async fn stop(&self) -> Result<()> {
        let shared = &self.tab.core.downloads;
        shared.active.store(false, Ordering::SeqCst);
        shared.baseline.lock().await.clear();
        shared.done_returned.lock().await.clear();
        shared.new_returned.lock().await.clear();
        Ok(())
    }

    fn ensure_active(&self) -> Result<()> {
        if self.tab.core.downloads.active.load(Ordering::SeqCst) {
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
        tokio::time::sleep(Duration::from_millis(80)).await;
        false
    }
}

/// 扫描下载目录,返回不在 `baseline` 里的新文件任务(`.part` → 进行中,最终文件 → 已完成)。
/// 供 [`Downloads`] 与一次性的 [`Tab::wait_download`](crate::browser::Tab::wait_download) 共用。
pub(crate) async fn scan_new_files(dir: &Path, baseline: &HashSet<String>) -> Vec<DownloadMission> {
    let mut finals: HashSet<String> = HashSet::new();
    let mut parts: HashSet<String> = HashSet::new();
    if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let name = e.file_name().to_string_lossy().to_string();
            match name.strip_suffix(".part") {
                Some(stripped) => {
                    parts.insert(stripped.to_string());
                }
                None => {
                    finals.insert(name);
                }
            }
        }
    }
    let mut out = Vec::new();
    for p in &parts {
        if baseline.contains(p) {
            continue;
        }
        out.push(DownloadMission {
            suggested_filename: p.clone(),
            path: dir.join(p),
            state: DownloadState::Downloading,
        });
    }
    for f in &finals {
        if baseline.contains(f) || parts.contains(f) {
            continue;
        }
        out.push(DownloadMission {
            suggested_filename: f.clone(),
            path: dir.join(f),
            state: DownloadState::Finished,
        });
    }
    out
}

/// 列出目录里的文件名集合(供记录基线)。读不到目录返回空集。
pub(crate) async fn list_dir_files(dir: &Path) -> HashSet<String> {
    let mut set = HashSet::new();
    if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            set.insert(e.file_name().to_string_lossy().to_string());
        }
    }
    set
}

/// 文件大小连续两次(间隔 120ms)一致即视为写入稳定。
pub(crate) async fn size_stable(path: &Path) -> bool {
    let Ok(a) = tokio::fs::metadata(path).await.map(|m| m.len()) else {
        return false;
    };
    tokio::time::sleep(Duration::from_millis(120)).await;
    matches!(tokio::fs::metadata(path).await.map(|m| m.len()), Ok(b) if b == a)
}
