//! 子进程启动 + 就绪检测。
//!
//! 流程:
//! 1. [`ensure_camoufox`] 解析/下载得到可执行文件;
//! 2. 准备 profile 目录(用户指定或临时目录);
//! 3. 组装 Camoufox 启动参数(`-no-remote -headless|-foreground -profile <dir> -juggler-pipe`);
//! 4. [`crate::transport::spawn`] 拉起子进程并接好 fd3/fd4 管道;
//! 5. 读 stderr 等待 `"Juggler listening to the pipe"`(带总超时),就绪后把 stderr
//!    交给后台任务持续排空(防止管道写满阻塞子进程),并按行打到 `tracing`。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout_at};

use crate::launcher::{BrowserOptions, ensure_camoufox};
use crate::transport::{self, Child, PipeReader, PipeWriter, Spawned};
use crate::{Error, Result};

/// 就绪标志:Camoufox/Firefox 在 Juggler 管道就绪时打印的行(子串匹配,stdout/stderr 均扫描)。
const READY_MARKER: &str = "Juggler listening";

/// 已就绪的浏览器:子进程 + 异步管道端 + profile 信息。
pub struct Launched {
    pub child: Child,
    pub writer: PipeWriter,
    pub reader: PipeReader,
    /// 使用的 profile 目录。
    pub profile_dir: PathBuf,
    /// 是否为库创建的临时目录(退出时应清理)。
    pub profile_is_temp: bool,
}

/// 启动 Camoufox 并等待 Juggler 就绪。
pub async fn launch(opts: &BrowserOptions) -> Result<Launched> {
    opts.validate()?;

    let exe = ensure_camoufox(opts.binary_path.as_deref()).await?;
    tracing::info!(path = %exe.display(), "使用 Camoufox 可执行文件");

    let (profile_dir, profile_is_temp) = prepare_profile(opts)?;
    if let Some(dir) = &opts.download_path {
        let _ = std::fs::create_dir_all(dir);
    }
    let args = build_args(opts, &profile_dir);
    let envs = build_envs(opts);
    tracing::debug!(?args, "Camoufox 启动参数");

    let Spawned {
        child,
        writer,
        reader,
        stdout,
        stderr,
    } = transport::spawn(&exe, &args, &envs).await?;

    wait_for_ready(stdout, stderr, opts.launch_timeout).await?;
    tracing::info!("Camoufox 已就绪(Juggler 管道在线)");

    Ok(Launched {
        child,
        writer,
        reader,
        profile_dir,
        profile_is_temp,
    })
}

/// 准备 profile 目录,返回 (路径, 是否为临时目录)。
fn prepare_profile(opts: &BrowserOptions) -> Result<(PathBuf, bool)> {
    if let Some(dir) = &opts.user_data_dir {
        std::fs::create_dir_all(dir)?;
        return Ok((dir.clone(), false));
    }
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "drission-camoufox-{}-{}-{}",
        std::process::id(),
        nanos,
        n
    ));
    std::fs::create_dir_all(&dir)?;
    Ok((dir, true))
}

/// 组装命令行参数。`-profile`/`-juggler-pipe` 由库内部管理(选项校验已禁止用户覆盖)。
fn build_args(opts: &BrowserOptions, profile_dir: &std::path::Path) -> Vec<String> {
    let mut args: Vec<String> = vec!["-no-remote".into()];
    if opts.headless {
        args.push("-headless".into());
    } else {
        // 对标 Playwright 的 headed 配方:`-wait-for-browser -foreground`(顺序固定)。
        // **Windows 必需**:有头/前台启动时,我们 spawn 的进程会再 fork 出真正的浏览器进程
        // 后自己退出;缺了 `-wait-for-browser`,原进程一退出,继承来的 fd3/fd4 Juggler 管道
        // 写端随之关闭 → 父进程读端立刻 EOF → 首个命令(`Browser.enable`)就报「连接已关闭」。
        // 该标志让前台进程**等真正的浏览器进程**、不提前退出,管道因此保持在线。
        // macOS/Linux 无 launcher 进程,此标志为良性(Playwright 同样跨平台传它)。
        args.push("-wait-for-browser".into());
        args.push("-foreground".into());
    }
    args.push("-profile".into());
    args.push(profile_dir.display().to_string());
    args.push("-juggler-pipe".into());
    args.extend(opts.args.iter().cloned());
    args
}

/// 组装环境变量:把 Camoufox 指纹配置(拟人化光标 + 屏幕一致性 + 自定义透传)序列化后
/// **按字符分块**写入 `CAMOU_CONFIG_1..n`(浏览器侧 `MaskConfig` 读取 `CAMOU_CONFIG_1..n` 拼接再解析)。
/// UA/语言/时区等仍走 Juggler 覆盖;WebRTC 阻断走 Firefox user prefs。
fn build_envs(opts: &BrowserOptions) -> Vec<(String, String)> {
    let cfg = opts.build_camou_config();
    if cfg.is_empty() {
        return Vec::new();
    }
    let json = serde_json::Value::Object(cfg).to_string();
    chunk_camou_config(&json)
}

/// 把 Camoufox 配置 JSON 按 **字符边界** 切成 `CAMOU_CONFIG_1..n`(浏览器侧按序拼接后再 JSON.parse,
/// 故任意切点都安全)。单块上限 2000 字符,避免单个环境变量过长(Windows 限制尤紧)。
fn chunk_camou_config(json: &str) -> Vec<(String, String)> {
    const MAX: usize = 2000;
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut idx = 1;
    for ch in json.chars() {
        if !buf.is_empty() && buf.len() + ch.len_utf8() > MAX {
            out.push((format!("CAMOU_CONFIG_{idx}"), std::mem::take(&mut buf)));
            idx += 1;
        }
        buf.push(ch);
    }
    if !buf.is_empty() {
        out.push((format!("CAMOU_CONFIG_{idx}"), buf));
    }
    out
}

/// 同时扫描 stdout 与 stderr 直到任一出现就绪标志;两条流都会持续被排空(防写满阻塞)。
///
/// 流类型按平台不同(unix `ChildStdout`/`ChildStderr` / windows `NamedPipeServer`),
/// 故对 `AsyncRead` 泛型化。
async fn wait_for_ready<O, E>(stdout: O, stderr: E, timeout: std::time::Duration) -> Result<()>
where
    O: AsyncRead + Unpin + Send + 'static,
    E: AsyncRead + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<()>(2);
    tokio::spawn(scan_stream(stdout, "out", tx.clone()));
    tokio::spawn(scan_stream(stderr, "err", tx));

    match timeout_at(Instant::now() + timeout, rx.recv()).await {
        Ok(Some(())) => Ok(()),
        // 两条流都结束仍未见标志:浏览器可能启动失败。
        Ok(None) => Err(Error::Transport(
            "子进程输出在就绪前已结束(浏览器可能启动失败)".into(),
        )),
        Err(_) => Err(Error::Timeout(timeout)),
    }
}

/// 按行读取一条流,记录到 tracing;遇到就绪标志通知一次。读到 EOF 自然结束。
async fn scan_stream<R>(stream: R, tag: &'static str, tx: mpsc::Sender<()>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "camoufox", "[{tag}] {line}");
        if line.contains(READY_MARKER) {
            let _ = tx.send(()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 分块后按序拼接必须等于原文(浏览器侧就是这样还原的),且每块不超过上限。
    #[test]
    fn chunk_roundtrips_and_bounds() {
        let long = format!("{{\"k\":\"{}\",\"含中文也安全\":true}}", "a".repeat(5000));
        let chunks = chunk_camou_config(&long);
        assert!(chunks.len() >= 3, "5000+ 字符应被切成多块");
        for (i, (name, part)) in chunks.iter().enumerate() {
            assert_eq!(name, &format!("CAMOU_CONFIG_{}", i + 1));
            assert!(part.chars().count() <= 2000 || part.len() <= 2000 + 4);
        }
        let joined: String = chunks.into_iter().map(|(_, p)| p).collect();
        assert_eq!(joined, long);
    }

    /// 默认选项至少下发拟人化 + 屏幕一致性,且首块名为 CAMOU_CONFIG_1。
    #[test]
    fn default_envs_include_humanize_and_screen() {
        let envs = build_envs(&BrowserOptions::default());
        assert_eq!(envs[0].0, "CAMOU_CONFIG_1");
        let joined: String = envs.into_iter().map(|(_, v)| v).collect();
        assert!(joined.contains("humanize"));
        assert!(joined.contains("screen.width"));
    }
}
