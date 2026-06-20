//! Camoufox 自动下载与分发。
//!
//! 解析当前平台/架构 → 查询 GitHub releases → 选择匹配资产 → 流式下载 zip →
//! 解压(在 unix 上保留可执行位与 `.app` 内的符号链接)→ 定位可执行文件。
//!
//! 解析优先级:
//! 1. 显式传入的 `binary_path`;
//! 2. 环境变量 `CAMOUFOX_BIN`;
//! 3. 缓存目录(`~/.cache/camoufox`)中已安装的版本;
//! 4. 从 GitHub 下载最新匹配版本。

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use crate::{Error, Result};

const GITHUB_RELEASES_API: &str = "https://api.github.com/repos/daijro/camoufox/releases";
const USER_AGENT: &str = concat!("drission-rs/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Deserialize)]
struct Release {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// 返回 (os 标记, arch 标记),对应 Camoufox 资产命名 `camoufox-<ver>-<os>.<arch>.zip`。
pub fn platform_tag() -> Result<(&'static str, &'static str)> {
    let os = match std::env::consts::OS {
        "macos" => "mac",
        "linux" => "lin",
        "windows" => "win",
        other => return Err(Error::UnsupportedPlatform(format!("os={other}"))),
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        "x86" => "i686",
        other => return Err(Error::UnsupportedPlatform(format!("arch={other}"))),
    };
    Ok((os, arch))
}

/// 缓存根目录:`~/.cache/camoufox`(与官方 Python 库保持一致)。
pub fn cache_root() -> PathBuf {
    if let Ok(custom) = std::env::var("CAMOUFOX_CACHE") {
        return PathBuf::from(custom);
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".cache").join("camoufox")
}

/// 当前平台可执行文件名。
fn exe_name() -> &'static str {
    if cfg!(windows) {
        "camoufox.exe"
    } else {
        "camoufox"
    }
}

/// 在目录下(有界深度)递归查找 Camoufox 可执行文件。
fn find_executable(root: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, target: &str, depth: usize) -> Option<PathBuf> {
        if depth == 0 {
            return None;
        }
        let entries = std::fs::read_dir(dir).ok()?;
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let ft = entry.file_type().ok()?;
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
    walk(root, exe_name(), 6)
}

/// 确保本机有可用的 Camoufox 可执行文件,返回其路径。必要时自动下载。
pub async fn ensure_camoufox(binary_path: Option<&Path>) -> Result<PathBuf> {
    // 1. 显式路径
    if let Some(p) = binary_path {
        if p.exists() {
            return Ok(p.to_path_buf());
        }
        return Err(Error::BrowserNotFound(format!(
            "指定的 binary_path 不存在: {}",
            p.display()
        )));
    }

    // 2. 环境变量
    if let Ok(env_bin) = std::env::var("CAMOUFOX_BIN") {
        let p = PathBuf::from(env_bin);
        if p.exists() {
            return Ok(p);
        }
    }

    let root = cache_root();
    let browsers_dir = root.join("browsers");

    // 3. 缓存命中
    if let Some(found) = find_executable(&browsers_dir) {
        tracing::debug!(path = %found.display(), "复用缓存中的 Camoufox");
        return Ok(found);
    }

    // 4. 下载最新版本
    tracing::info!("缓存未命中,开始从 GitHub 下载 Camoufox …");
    let (os, arch) = platform_tag()?;
    let (asset_name, url) = pick_asset(os, arch).await?;
    tracing::info!(asset = %asset_name, "选定资产");

    tokio::fs::create_dir_all(&browsers_dir).await?;
    let zip_path = root.join(&asset_name);
    download(&url, &zip_path).await?;

    let stem = asset_name.trim_end_matches(".zip");
    let dest = browsers_dir.join(stem);
    tokio::fs::create_dir_all(&dest).await?;

    let dest_clone = dest.clone();
    let zip_clone = zip_path.clone();
    tokio::task::spawn_blocking(move || extract_zip(&zip_clone, &dest_clone))
        .await
        .map_err(|e| Error::Other(format!("解压任务 join 失败: {e}")))??;

    // 删除下载的 zip(失败不致命)
    let _ = tokio::fs::remove_file(&zip_path).await;

    find_executable(&dest).ok_or_else(|| {
        Error::BrowserNotFound(format!("解压后未找到可执行文件,目录: {}", dest.display()))
    })
}

/// 查询 releases,返回第一个匹配 `-<os>.<arch>.zip` 的资产 (name, url)。
async fn pick_asset(os: &str, arch: &str) -> Result<(String, String)> {
    let suffix = format!("-{os}.{arch}.zip");
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()?;
    let releases: Vec<Release> = client
        .get(GITHUB_RELEASES_API)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    for rel in &releases {
        for asset in &rel.assets {
            if asset.name.ends_with(&suffix) {
                tracing::debug!(tag = %rel.tag_name, "命中 release");
                return Ok((asset.name.clone(), asset.browser_download_url.clone()));
            }
        }
    }
    Err(Error::BrowserNotFound(format!(
        "GitHub releases 中没有匹配 `{suffix}` 的 Camoufox 资产"
    )))
}

/// 流式下载到文件。
async fn download(url: &str, dest: &Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_tag_is_known() {
        // 在 CI/本机上至少应解析成功(三大平台之一)。
        let (os, arch) = platform_tag().expect("当前平台应被支持");
        assert!(["mac", "lin", "win"].contains(&os));
        assert!(["arm64", "x86_64", "i686"].contains(&arch));
    }

    #[test]
    fn cache_root_under_home() {
        let r = cache_root();
        assert!(r.ends_with("camoufox") || std::env::var("CAMOUFOX_CACHE").is_ok());
    }
}
