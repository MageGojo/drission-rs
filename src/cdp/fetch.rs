//! **Chrome for Testing** 自动下载与分发(CDP 后端)。
//!
//! 对标 CloakBrowser / Camoufox 的「首次运行自动下载浏览器二进制」体验,但走 Google 官方
//! **Chrome for Testing** 分发(三平台齐全):解析平台 → 查询 last-known-good JSON →
//! 选匹配资产 → 流式下载 zip → 解压(unix 保留可执行位与符号链接,mac `.app` 内有符号链接)
//! → 定位 chrome 可执行文件。与 [`super::locate`](定位系统已装 Chrome)互补。
//!
//! 解析优先级([`ensure_chrome`]):
//! 1. 环境变量 `CHROME_BIN` / `DRISSION_CHROME`(经 [`super::locate`]);
//! 2. 系统已安装的 Chrome / Edge / Brave / Chromium(经 [`super::locate`],Windows 含注册表);
//! 3. 缓存目录(`~/.cache/drission/chrome/<platform>`)中已下载的 Chrome for Testing;
//! 4. 从 Chrome for Testing 下载当前平台最新 Stable。
//!
//! 跨平台预取:[`download_chrome_for`] 可下载**任意**平台(如在 mac 上预取 `win64`),
//! 用于分发 / 打包(对应「mac 和 win 都要」)。

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use crate::{Error, Result};

/// Chrome for Testing 已知良好版本索引(含各平台下载直链)。
const CFT_ENDPOINT: &str = "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";
const USER_AGENT: &str = concat!("drission-rs/", env!("CARGO_PKG_VERSION"));

/// 默认下载渠道。
pub const DEFAULT_CHANNEL: &str = "Stable";

#[derive(Debug, Deserialize)]
struct CftIndex {
    channels: std::collections::HashMap<String, Channel>,
}

#[derive(Debug, Deserialize)]
struct Channel {
    #[serde(default)]
    version: String,
    downloads: Downloads,
}

#[derive(Debug, Deserialize)]
struct Downloads {
    #[serde(default)]
    chrome: Vec<Download>,
}

#[derive(Debug, Deserialize)]
struct Download {
    platform: String,
    url: String,
}

/// 当前平台对应的 Chrome for Testing 平台标记(资产命名 `chrome-<platform>.zip`)。
///
/// `mac-arm64` / `mac-x64` / `win64` / `win32` / `linux64`。
pub fn cft_platform() -> Result<&'static str> {
    let p = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "mac-arm64",
        ("macos", "x86_64") => "mac-x64",
        ("windows", "x86_64") => "win64",
        ("windows", "x86") => "win32",
        ("linux", "x86_64") => "linux64",
        (os, arch) => {
            return Err(Error::UnsupportedPlatform(format!(
                "Chrome for Testing 无 {os}/{arch} 资产"
            )));
        }
    };
    Ok(p)
}

/// 缓存根目录:`~/.cache/drission`(可用 `DRISSION_CACHE` 覆盖)。
pub fn cache_root() -> PathBuf {
    if let Ok(custom) = std::env::var("DRISSION_CACHE") {
        return PathBuf::from(custom);
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".cache").join("drission")
}

/// 给定平台解压后的 Chrome 可执行文件名(用于在解出目录里定位)。
fn chrome_exe_name(platform: &str) -> &'static str {
    if platform.starts_with("mac") {
        "Google Chrome for Testing"
    } else if platform.starts_with("win") {
        "chrome.exe"
    } else {
        "chrome"
    }
}

/// 确保本机有可用的 Chrome,返回其可执行文件路径。必要时自动下载 Chrome for Testing。
///
/// 优先级见[模块文档](self):环境变量 / 系统已装 → 缓存 → 下载当前平台 Stable。
pub async fn ensure_chrome() -> Result<PathBuf> {
    // 1 + 2. 环境变量与系统已安装(locate 内部已涵盖 CHROME_BIN/DRISSION_CHROME、安装路径、
    //        Windows 注册表、PATH 扫描)。优先用真实系统浏览器。
    if let Ok(p) = super::locate::chrome_path() {
        tracing::debug!(path = %p.display(), "使用系统已定位的 Chrome");
        return Ok(p);
    }
    // 3 + 4. 缓存命中或下载当前平台。
    let platform = cft_platform()?;
    download_chrome_for(platform, DEFAULT_CHANNEL).await
}

/// 确保**指定平台**的 Chrome for Testing 已下载到缓存,返回其可执行文件路径。
///
/// - 当前平台:返回的路径可直接传给 [`super::ChromiumBrowser::launch_with`] 启动;
/// - **其它平台**(跨平台预取,如在 mac 上取 `win64`):返回的可执行文件**无法在本机运行**,
///   仅用于分发 / 打包。
///
/// 缓存命中(`~/.cache/drission/chrome/<platform>` 下已有可执行文件)则直接复用、不重复下载。
pub async fn download_chrome_for(platform: &str, channel: &str) -> Result<PathBuf> {
    let chrome_root = cache_root().join("chrome");
    let dest = chrome_root.join(platform);
    let exe_name = chrome_exe_name(platform);

    // 缓存命中(已解压)。
    if let Some(found) = find_executable(&dest, exe_name) {
        tracing::debug!(path = %found.display(), "复用缓存中的 Chrome for Testing");
        return Ok(found);
    }

    tokio::fs::create_dir_all(&dest).await?;
    let zip_path = chrome_root.join(format!("chrome-{platform}.zip"));

    // 若已有预下载好的 zip(如外部下载器预置到 `~/.cache/drission/chrome/chrome-<platform>.zip`),
    // 直接解压复用,免重复网络下载;否则查 Chrome for Testing 索引并流式下载。
    if zip_path.exists() {
        tracing::info!(path = %zip_path.display(), "发现预下载的 Chrome zip,直接解压复用");
    } else {
        let (version, url) = pick_asset(platform, channel).await?;
        tracing::info!(%version, %platform, %channel, "缓存未命中,开始下载 Chrome for Testing …");
        download(&url, &zip_path).await?;
    }

    let dest_clone = dest.clone();
    let zip_clone = zip_path.clone();
    tokio::task::spawn_blocking(move || extract_zip(&zip_clone, &dest_clone))
        .await
        .map_err(|e| Error::Other(format!("解压任务 join 失败: {e}")))??;

    // 删除下载的 zip(失败不致命)。
    let _ = tokio::fs::remove_file(&zip_path).await;

    let found = find_executable(&dest, exe_name).ok_or_else(|| {
        Error::BrowserNotFound(format!(
            "解压后未找到 Chrome 可执行文件({exe_name}),目录: {}",
            dest.display()
        ))
    })?;

    // unix:确保主可执行文件有执行位(下载非本机平台时无意义但无害)。
    #[cfg(unix)]
    ensure_executable_bit(&found);

    Ok(found)
}

/// 查询 Chrome for Testing 索引,返回指定平台 / 渠道的 (version, url)。
async fn pick_asset(platform: &str, channel: &str) -> Result<(String, String)> {
    let client = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
    let index: CftIndex = client
        .get(CFT_ENDPOINT)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let ch = index.channels.get(channel).ok_or_else(|| {
        Error::BrowserNotFound(format!(
            "Chrome for Testing 无渠道 `{channel}`(可选 Stable/Beta/Dev/Canary)"
        ))
    })?;

    for dl in &ch.downloads.chrome {
        if dl.platform == platform {
            return Ok((ch.version.clone(), dl.url.clone()));
        }
    }
    Err(Error::BrowserNotFound(format!(
        "Chrome for Testing 渠道 `{channel}` 无平台 `{platform}` 的 chrome 资产"
    )))
}

/// 流式下载到文件(每 16 MiB 打一次进度日志)。
async fn download(url: &str, dest: &Path) -> Result<()> {
    let client = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
    let resp = client.get(url).send().await?.error_for_status()?;
    let total = resp.content_length().unwrap_or(0);

    let mut file = tokio::fs::File::create(dest).await?;
    let mut downloaded: u64 = 0;
    let mut last_logged: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        if downloaded - last_logged > 16 * 1024 * 1024 {
            last_logged = downloaded;
            if total > 0 {
                tracing::info!(
                    "下载进度 {:.1}% ({}/{} MiB)",
                    downloaded as f64 / total as f64 * 100.0,
                    downloaded / 1024 / 1024,
                    total / 1024 / 1024
                );
            }
        }
    }
    file.flush().await?;
    Ok(())
}

/// 在目录下(有界深度)递归查找指定名字的可执行文件。
///
/// 深度给到 8,足以覆盖 mac `.app` 的嵌套(`chrome-mac-*/...app/Contents/MacOS/<exe>`)。
fn find_executable(root: &Path, target: &str) -> Option<PathBuf> {
    fn walk(dir: &Path, target: &str, depth: usize) -> Option<PathBuf> {
        if depth == 0 {
            return None;
        }
        let entries = std::fs::read_dir(dir).ok()?;
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_file() && entry.file_name().to_string_lossy() == target {
                return Some(path);
            }
            if ft.is_dir() {
                subdirs.push(path);
            }
        }
        for sub in subdirs {
            if let Some(found) = walk(&sub, target, depth - 1) {
                return Some(found);
            }
        }
        None
    }
    walk(root, target, 8)
}

/// 同步解压 zip 到目标目录(在 unix 上保留权限与符号链接)。
fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let rel = match entry.enclosed_name() {
            Some(p) => p,
            None => continue, // 跳过不安全/非法路径
        };
        let outpath = dest.join(rel);

        let mode = entry.unix_mode();
        let is_symlink = mode.map(|m| m & 0o170000 == 0o120000).unwrap_or(false);

        if entry.is_dir() {
            std::fs::create_dir_all(&outpath)?;
            continue;
        }

        if let Some(parent) = outpath.parent() {
            std::fs::create_dir_all(parent)?;
        }

        if is_symlink {
            #[cfg(unix)]
            {
                use std::io::Read;
                let mut target = String::new();
                entry.read_to_string(&mut target)?;
                // 已存在则先删,避免 symlink 创建失败
                let _ = std::fs::remove_file(&outpath);
                std::os::unix::fs::symlink(&target, &outpath)?;
            }
            #[cfg(not(unix))]
            {
                // 非 unix 平台:把符号链接当普通文件落地
                let mut out = std::fs::File::create(&outpath)?;
                std::io::copy(&mut entry, &mut out)?;
            }
            continue;
        }

        let mut out = std::fs::File::create(&outpath)?;
        std::io::copy(&mut entry, &mut out)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(m) = mode {
                std::fs::set_permissions(&outpath, std::fs::Permissions::from_mode(m))?;
            }
        }
    }
    Ok(())
}

/// 确保 unix 下文件带执行位(`u+x`),解压若丢了执行位时兜底。
#[cfg(unix)]
fn ensure_executable_bit(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perm = meta.permissions();
        let mode = perm.mode();
        if mode & 0o111 == 0 {
            perm.set_mode(mode | 0o755);
            let _ = std::fs::set_permissions(path, perm);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cft_platform_is_known() {
        // 当前平台至少应解析成功(五个已知标记之一)。
        let p = cft_platform().expect("当前平台应被支持");
        assert!(["mac-arm64", "mac-x64", "win64", "win32", "linux64"].contains(&p));
    }

    #[test]
    fn chrome_exe_name_per_platform() {
        assert_eq!(chrome_exe_name("mac-arm64"), "Google Chrome for Testing");
        assert_eq!(chrome_exe_name("mac-x64"), "Google Chrome for Testing");
        assert_eq!(chrome_exe_name("win64"), "chrome.exe");
        assert_eq!(chrome_exe_name("win32"), "chrome.exe");
        assert_eq!(chrome_exe_name("linux64"), "chrome");
    }

    #[test]
    fn cache_root_under_home_or_override() {
        let r = cache_root();
        assert!(r.ends_with("drission") || std::env::var("DRISSION_CACHE").is_ok());
    }

    #[test]
    fn find_executable_locates_nested_file() {
        // 造一个嵌套目录里的目标文件,验证有界递归能找到。
        let base = std::env::temp_dir().join(format!(
            "drission_fetch_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let nested = base.join("chrome-x").join("a.app").join("Contents");
        std::fs::create_dir_all(&nested).expect("建嵌套目录");
        let exe = nested.join("target-exe");
        std::fs::write(&exe, b"x").expect("写目标文件");

        assert_eq!(
            find_executable(&base, "target-exe").as_deref(),
            Some(exe.as_path())
        );
        assert!(find_executable(&base, "no-such-file").is_none());

        let _ = std::fs::remove_dir_all(&base);
    }
}
