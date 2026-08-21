//! 统一媒体入口:`tab.capture()`。底层仍是现有 screenshot / PDF / MHTML / HAR。

use std::path::{Path, PathBuf};

use super::extras::{HarRecorder, PdfOptions};
use super::tab::ChromiumTab;
use crate::Result;

/// `tab.capture()` 句柄。
pub struct PageCapture {
    tab: ChromiumTab,
}

impl ChromiumTab {
    pub fn capture(&self) -> PageCapture {
        PageCapture { tab: self.clone() }
    }

    /// 下载管理(同 [`downloads`](Self::downloads))。
    pub fn download(&self) -> crate::cdp::ChromiumDownloads {
        self.downloads()
    }

    /// 对 input[type=file] 设本地路径。
    pub async fn upload(&self, selector: &str, path: &str) -> crate::Result<()> {
        self.ele(selector).await?.upload(path).await
    }
}

impl PageCapture {
    pub async fn screenshot(&self) -> Result<Vec<u8>> {
        self.tab.screenshot_bytes().await
    }

    pub async fn full_page(&self) -> Result<Vec<u8>> {
        self.tab.screenshot_full_bytes().await
    }

    pub async fn pdf(&self, path: impl AsRef<Path>) -> Result<PathBuf> {
        self.tab.save_pdf(path, &PdfOptions::default()).await
    }

    pub async fn mhtml(&self, path: impl AsRef<Path>) -> Result<PathBuf> {
        self.tab.save_mhtml(path).await
    }

    pub async fn html(&self) -> Result<String> {
        self.tab.html().await
    }

    /// 开始录 HAR。导航前调用,`stop()` 取日志。
    pub async fn har(&self) -> Result<HarRecorder> {
        self.tab.har_record().await
    }
}
