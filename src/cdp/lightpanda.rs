//! Lightpanda 二进制定位 / 自动下载 / `serve` 启动。
//!
//! Lightpanda 讲 CDP,上层仍是 [`ChromiumBrowser`](super::ChromiumBrowser)。本模块只负责
//! 「找到或下载 `lightpanda` → `serve --host 127.0.0.1 --port N` → 拿到 WS 端点」。
//! 详见 `docs/Lightpanda.md`。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::sleep;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::cdp::fetch::{self, cache_root};
use crate::cdp::options::ChromiumOptions;
use crate::{Error, Result};

type CdpWs = WebSocketStream<MaybeTlsStream<TcpStream>>;

const GITHUB_RELEASE: &str = "https://github.com/lightpanda-io/browser/releases/download";
const DEFAULT_TAG: &str = "nightly";

/// `ensure_lightpanda` 下载 / 缓存用的 release tag(`nightly` 或 `0.3.7`)。
pub fn release_tag() -> String {
    std::env::var("DRISSION_LIGHTPANDA_TAG").unwrap_or_else(|_| DEFAULT_TAG.to_string())
}

/// 当前平台对应的 GitHub 资产后缀:`aarch64-macos` / `x86_64-macos` / `x86_64-linux` / `aarch64-linux`。
///
/// Windows 无原生二进制,返回 [`Error::UnsupportedPlatform`]。
pub fn lightpanda_platform() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("aarch64-macos"),
        ("macos", "x86_64") => Ok("x86_64-macos"),
        ("linux", "x86_64") => Ok("x86_64-linux"),
        ("linux", "aarch64") => Ok("aarch64-linux"),
        ("windows", _) => Err(Error::UnsupportedPlatform(
            "Lightpanda 无原生 Windows 二进制。请在 WSL2 安装,或改用 Chromium 后端;已开的 WSL serve 可用 connect(\"http://127.0.0.1:9222\")".into(),
        )),
        (os, arch) => Err(Error::UnsupportedPlatform(format!(
            "Lightpanda 无 {os}/{arch} 资产"
        ))),
    }
}

/// 只定位、不下载。优先级:环境变量 → **可执行文件旁边**(dist 内置) → PATH / 常见路径 → 缓存。
pub fn locate_lightpanda() -> Result<PathBuf> {
    for var in ["LIGHTPANDA_BIN", "DRISSION_LIGHTPANDA"] {
        if let Some(v) = std::env::var_os(var) {
            let pb = PathBuf::from(v);
            if pb.is_file() {
                return Ok(pb);
            }
        }
    }
    if let Some(p) = sidecar_lightpanda() {
        return Ok(p);
    }
    if let Some(p) = first_existing(install_candidates()) {
        return Ok(p);
    }
    if let Some(p) = from_path_env() {
        return Ok(p);
    }
    if let Ok(p) = cached_exe() {
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(Error::BrowserNotFound(
        "未找到 Lightpanda。设置 LIGHTPANDA_BIN / DRISSION_LIGHTPANDA,或让 ensure_lightpanda() 自动下载。".into(),
    ))
}

/// 确保本机有 `lightpanda`:先定位,没有再下载当前平台资产。
pub async fn ensure_lightpanda() -> Result<PathBuf> {
    if let Ok(p) = locate_lightpanda() {
        tracing::debug!(path = %p.display(), "使用已定位的 Lightpanda");
        return Ok(p);
    }
    download_lightpanda().await
}

/// 下载当前平台 Lightpanda 到缓存并返回可执行文件路径。
pub async fn download_lightpanda() -> Result<PathBuf> {
    let platform = lightpanda_platform()?;
    let tag = release_tag();
    let dest = cached_exe_for(&tag, platform);
    if dest.is_file() {
        tracing::debug!(path = %dest.display(), "复用缓存中的 Lightpanda");
        return Ok(dest);
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let url = format!("{GITHUB_RELEASE}/{tag}/lightpanda-{platform}");
    tracing::info!(%url, %tag, %platform, "缓存未命中,开始下载 Lightpanda …");
    let tmp = dest.with_extension("download");
    fetch::download(&url, &tmp).await.map_err(|e| {
        Error::BrowserNotFound(format!("下载 Lightpanda 失败({url}): {e}"))
    })?;
    tokio::fs::rename(&tmp, &dest).await?;
    #[cfg(unix)]
    ensure_executable_bit(&dest);
    Ok(dest)
}

/// 已 spawn 的 Lightpanda CDP 服务(子进程 + 已握手的 WS)。
pub(crate) struct SpawnedServe {
    pub child: tokio::process::Child,
    pub ws: CdpWs,
}

/// `lightpanda serve --host 127.0.0.1 --port <idle>` ,等到 WS 可连。
pub(crate) async fn spawn_serve(exe: &Path, opts: &ChromiumOptions) -> Result<SpawnedServe> {
    let port = pick_free_port()?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("serve")
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .env("LIGHTPANDA_DISABLE_TELEMETRY", "true")
        .kill_on_drop(true)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    if let Some(proxy) = &opts.proxy {
        cmd.arg("--http-proxy").arg(proxy);
    }
    if let Some(dir) = &opts.user_data_dir {
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::msg(format!("Lightpanda: 建 http-cache-dir 失败: {e}")))?;
        cmd.arg("--http-cache-dir").arg(dir);
    }
    if let Some(ua) = &opts.user_agent {
        if ua.contains("Mozilla") {
            tracing::warn!(
                "Lightpanda 拒绝含 Mozilla 的 --user-agent,已跳过(官方要求勿伪装其它浏览器)"
            );
        } else {
            cmd.arg("--user-agent").arg(ua);
        }
    }
    if let Some(tz) = &opts.timezone {
        cmd.env("TZ", tz);
    }
    for a in &opts.args {
        cmd.arg(a);
    }

    let mut child = cmd.spawn().map_err(|e| {
        Error::msg(format!(
            "Lightpanda: 启动失败({}): {e}",
            exe.display()
        ))
    })?;

    let stderr_buf = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
    if let Some(err) = child.stderr.take() {
        let buf = stderr_buf.clone();
        tokio::spawn(async move {
            let mut r = BufReader::new(err);
            let mut s = String::new();
            let _ = r.read_to_string(&mut s).await;
            *buf.lock().await = s;
        });
    }

    let ws = wait_until_ready(&mut child, port, Duration::from_secs(20), &stderr_buf).await?;
    Ok(SpawnedServe { child, ws })
}

async fn wait_until_ready(
    child: &mut tokio::process::Child,
    port: u16,
    timeout: Duration,
    stderr: &std::sync::Arc<tokio::sync::Mutex<String>>,
) -> Result<CdpWs> {
    let deadline = std::time::Instant::now() + timeout;
    let http = format!("http://127.0.0.1:{port}");
    let direct = format!("ws://127.0.0.1:{port}");
    loop {
        if let Some(status) = child.try_wait().ok().flatten() {
            let err = stderr.lock().await.clone();
            return Err(Error::msg(format!(
                "Lightpanda: 进程提前退出({status});stderr: {}",
                trim_err(&err)
            )));
        }
        let candidate = match super::browser::browser_ws_url(&http).await {
            Ok(url) => url,
            Err(_) => direct.clone(),
        };
        let last = match crate::transport::ws_connect(&candidate).await {
            Ok(ws) => return Ok(ws),
            Err(e) => e.to_string(),
        };
        if std::time::Instant::now() >= deadline {
            let err = stderr.lock().await.clone();
            return Err(Error::msg(format!(
                "Lightpanda: 等待 CDP 端点就绪超时(端口 {port});最后错误: {last};stderr: {}",
                trim_err(&err)
            )));
        }
        sleep(Duration::from_millis(50)).await;
    }
}

fn trim_err(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() {
        "(空)".into()
    } else {
        t.chars().take(800).collect()
    }
}

fn cached_exe() -> Result<PathBuf> {
    Ok(cached_exe_for(&release_tag(), lightpanda_platform()?))
}

fn cached_exe_for(tag: &str, platform: &str) -> PathBuf {
    cache_root()
        .join("lightpanda")
        .join(tag)
        .join(platform)
        .join("lightpanda")
}

/// 和当前进程放在同一目录(或 `bin/`)的 `lightpanda`，给 `dist/` 打包用。
fn sidecar_lightpanda() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if cfg!(windows) {
        "lightpanda.exe"
    } else {
        "lightpanda"
    };
    first_existing(vec![dir.join(name), dir.join("bin").join(name)])
}

fn install_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = dirs::home_dir() {
        v.push(home.join(".local/bin/lightpanda"));
        v.push(home.join(".lightpanda/bin/lightpanda"));
    }
    v.push(PathBuf::from("/opt/homebrew/bin/lightpanda"));
    v.push(PathBuf::from("/usr/local/bin/lightpanda"));
    v.push(PathBuf::from("/usr/bin/lightpanda"));
    v
}

fn first_existing(cands: Vec<PathBuf>) -> Option<PathBuf> {
    cands.into_iter().find(|p| p.is_file())
}

fn from_path_env() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "lightpanda.exe"
    } else {
        "lightpanda"
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn pick_free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| Error::msg(format!("Lightpanda: 选空闲端口失败: {e}")))?;
    Ok(listener
        .local_addr()
        .map_err(|e| Error::msg(format!("Lightpanda: 读空闲端口失败: {e}")))?
        .port())
}

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
    fn platform_matches_host_or_windows_is_unsupported() {
        match lightpanda_platform() {
            Ok(p) => {
                assert!(
                    [
                        "aarch64-macos",
                        "x86_64-macos",
                        "x86_64-linux",
                        "aarch64-linux"
                    ]
                    .contains(&p)
                );
            }
            Err(Error::UnsupportedPlatform(msg)) => {
                assert!(
                    cfg!(windows) || msg.contains("无"),
                    "非 Windows 不应走 UnsupportedPlatform: {msg}"
                );
            }
            Err(e) => panic!("意外错误: {e}"),
        }
    }

    #[test]
    fn cache_path_nests_tag_and_platform() {
        let p = cached_exe_for("nightly", "aarch64-macos");
        let s = p.to_string_lossy();
        assert!(s.contains("lightpanda"));
        assert!(s.contains("nightly"));
        assert!(s.contains("aarch64-macos"));
        assert!(s.ends_with("lightpanda"));
    }

    #[test]
    fn default_tag_is_nightly_unless_overridden() {
        if std::env::var_os("DRISSION_LIGHTPANDA_TAG").is_none() {
            assert_eq!(release_tag(), "nightly");
        }
    }

    #[test]
    fn sidecar_looks_next_to_current_exe() {
        let exe = std::env::current_exe().expect("current_exe");
        let dir = exe.parent().expect("exe dir");
        let name = if cfg!(windows) {
            "lightpanda.exe"
        } else {
            "lightpanda"
        };
        let expected = dir.join(name);
        if expected.is_file() {
            assert_eq!(sidecar_lightpanda().as_deref(), Some(expected.as_path()));
        } else {
            assert!(sidecar_lightpanda().is_none() || sidecar_lightpanda() == Some(dir.join("bin").join(name)));
        }
    }
}
