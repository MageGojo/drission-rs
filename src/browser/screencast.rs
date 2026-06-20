//! 录像 [`Screencast`](`tab.screencast()`):对标 DrissionPage 的 `tab.screencast`。
//!
//! **大道至简**:不走 Juggler 原生 `Page.startScreencast`(那要处理 `screencastFrame` 事件 + 逐帧 `ack` +
//! 节流,复杂度更高),而是起一个**后台任务按帧间隔反复截视口**(复用 [`TabCore::capture`])。
//!
//! 模式([`ScreencastMode`]):
//! - `Imgs`:持续逐帧把视口存为 PNG(`frame_000000.png`、`frame_000001.png`…)。
//! - `FrugalImgs`:仅当画面**变化**(与上一帧字节不同)才存——对应 DP 的"省流"模式。
//! - `Video` / `FrugalVideo`:先取帧,停止时用 **ffmpeg** 合成 mp4(对标 DP 的 video 模式需 opencv,
//!   这里换成更通用的 ffmpeg CLI,**不引入 Rust 依赖**);未装 ffmpeg 则报错并指引改用 `Imgs`。
//!
//! ```ignore
//! let cast = tab.screencast();
//! cast.set_save_path("video").set_mode(ScreencastMode::Imgs).set_fps(10.0);
//! cast.start(None).await?;
//! tab.wait().secs(3.0).await;
//! let out = cast.stop().await?;   // imgs→帧目录;video→mp4 文件
//! ```

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

use crate::browser::tab::{ImageFormat, Tab, TabCore};
use crate::{Error, Result};

/// 录制模式(对标 DP `screencast.set_mode.*`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreencastMode {
    /// 持续逐帧保存图片(默认)。
    #[default]
    Imgs,
    /// 仅画面变化时保存图片(省流)。
    FrugalImgs,
    /// 持续取帧,停止时用 ffmpeg 合成 mp4。
    Video,
    /// 仅画面变化时取帧,停止时用 ffmpeg 合成 mp4。
    FrugalVideo,
}

impl ScreencastMode {
    fn is_video(self) -> bool {
        matches!(self, ScreencastMode::Video | ScreencastMode::FrugalVideo)
    }
    fn is_frugal(self) -> bool {
        matches!(
            self,
            ScreencastMode::FrugalImgs | ScreencastMode::FrugalVideo
        )
    }
}

/// 录像配置(可经句柄链式设置)。
#[derive(Debug, Clone)]
struct Cfg {
    save_path: Option<PathBuf>,
    mode: ScreencastMode,
    interval: Duration,
}

impl Default for Cfg {
    fn default() -> Self {
        Self {
            save_path: None,
            mode: ScreencastMode::Imgs,
            interval: Duration::from_millis(100), // 默认 ~10fps
        }
    }
}

/// 录像共享状态(挂在 `TabCore` 上,跨 `start`/`stop` 持久)。
pub(crate) struct ScreencastShared {
    running: AtomicBool,
    cfg: std::sync::Mutex<Cfg>,
    task: Mutex<Option<JoinHandle<Result<PathBuf>>>>,
    stop_tx: Mutex<Option<watch::Sender<bool>>>,
}

impl ScreencastShared {
    pub(crate) fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            cfg: std::sync::Mutex::new(Cfg::default()),
            task: Mutex::new(None),
            stop_tx: Mutex::new(None),
        }
    }
}

/// `tab.screencast()` 返回的录像句柄(对应 DP `tab.screencast`)。
pub struct Screencast {
    tab: Tab,
}

impl Screencast {
    pub(crate) fn new(tab: Tab) -> Self {
        Self { tab }
    }

    fn shared(&self) -> &Arc<ScreencastShared> {
        &self.tab.core.screencast
    }

    /// 设置保存路径(对应 DP `screencast.set_save_path`)。imgs 模式存帧到此目录,video 模式 mp4 落此目录。
    pub fn set_save_path(&self, path: impl AsRef<Path>) -> &Self {
        if let Ok(mut cfg) = self.shared().cfg.lock() {
            cfg.save_path = Some(path.as_ref().to_path_buf());
        }
        self
    }

    /// 设置录制模式(对应 DP `screencast.set_mode.*`)。
    pub fn set_mode(&self, mode: ScreencastMode) -> &Self {
        if let Ok(mut cfg) = self.shared().cfg.lock() {
            cfg.mode = mode;
        }
        self
    }

    /// 设置帧率(每秒帧数,`1.0`–`60.0`;默认 10)。
    pub fn set_fps(&self, fps: f64) -> &Self {
        let fps = fps.clamp(1.0, 60.0);
        if let Ok(mut cfg) = self.shared().cfg.lock() {
            cfg.interval = Duration::from_secs_f64(1.0 / fps);
        }
        self
    }

    /// 是否正在录制(对应 DP `screencast` 的运行态)。
    pub fn is_recording(&self) -> bool {
        self.shared().running.load(Ordering::SeqCst)
    }

    /// 开始录制(对应 DP `screencast.start`)。可顺带传保存路径;未设保存路径会报错。
    ///
    /// 后台任务每隔一帧间隔截一次视口;`FrugalImgs`/`FrugalVideo` 仅在画面变化时存帧。
    pub async fn start(&self, save_path: Option<impl AsRef<Path>>) -> Result<()> {
        let sh = self.shared().clone();
        if sh.running.swap(true, Ordering::SeqCst) {
            return Err(Error::Other("录屏已在进行中".into()));
        }
        // 取一份配置快照(顺带应用本次传入的 save_path)。
        let cfg = {
            match sh.cfg.lock() {
                Ok(mut c) => {
                    if let Some(p) = save_path {
                        c.save_path = Some(p.as_ref().to_path_buf());
                    }
                    c.clone()
                }
                Err(_) => {
                    sh.running.store(false, Ordering::SeqCst);
                    return Err(Error::Other("读取录屏配置失败".into()));
                }
            }
        };
        let Some(save_path) = cfg.save_path.clone() else {
            sh.running.store(false, Ordering::SeqCst);
            return Err(Error::Other(
                "请先 set_save_path(或 start(Some(path))) 设置保存路径".into(),
            ));
        };

        let (tx, rx) = watch::channel(false);
        *sh.stop_tx.lock().await = Some(tx);
        let core = self.tab.core.clone();
        let handle = tokio::spawn(record_loop(core, cfg, save_path, rx));
        *sh.task.lock().await = Some(handle);
        Ok(())
    }

    /// 停止录制(对应 DP `screencast.stop`)。返回结果路径:imgs 模式为帧目录,video 模式为 mp4 文件。
    pub async fn stop(&self) -> Result<PathBuf> {
        let sh = self.shared().clone();
        if !sh.running.swap(false, Ordering::SeqCst) {
            return Err(Error::Other("当前未在录屏".into()));
        }
        if let Some(tx) = sh.stop_tx.lock().await.take() {
            let _ = tx.send(true);
        }
        let handle = sh.task.lock().await.take();
        match handle {
            Some(h) => h
                .await
                .map_err(|e| Error::Other(format!("录屏任务异常退出: {e}")))?,
            None => Err(Error::Other("录屏任务句柄缺失".into())),
        }
    }
}

/// 后台录制循环:按帧间隔截视口存帧,直到收到停止信号;video 模式收尾时合成 mp4。
async fn record_loop(
    core: Arc<TabCore>,
    cfg: Cfg,
    save_path: PathBuf,
    mut stop: watch::Receiver<bool>,
) -> Result<PathBuf> {
    // video 模式把帧落到子目录,合成后清理;imgs 模式直接落到 save_path。
    let frames_dir = if cfg.mode.is_video() {
        save_path.join(".frames")
    } else {
        save_path.clone()
    };
    tokio::fs::create_dir_all(&frames_dir).await?;

    let mut n: usize = 0;
    let mut prev: Option<Vec<u8>> = None;
    loop {
        // 截当前视口(PNG)。截图失败不致命:跳过本帧继续(页面可能正在导航)。
        if let Ok(clip) = core.page_clip(false).await
            && let Ok(bytes) = core.capture(clip, ImageFormat::Png, None).await
        {
            let changed = prev.as_deref() != Some(bytes.as_slice());
            if !cfg.mode.is_frugal() || changed {
                let path = frames_dir.join(format!("frame_{n:06}.png"));
                let _ = tokio::fs::write(&path, &bytes).await;
                n += 1;
                prev = Some(bytes);
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(cfg.interval) => {}
            res = stop.changed() => {
                // 发送端在 stop() 时置 true;通道关闭也视为停止。
                if res.is_err() || *stop.borrow() {
                    break;
                }
            }
        }
    }

    if cfg.mode.is_video() {
        let fps = (1.0 / cfg.interval.as_secs_f64()).round().max(1.0) as u32;
        let out = save_path.join(format!("screencast_{}.mp4", timestamp()));
        encode_video(&frames_dir, fps, &out).await?;
        let _ = tokio::fs::remove_dir_all(&frames_dir).await;
        Ok(out)
    } else {
        Ok(frames_dir)
    }
}

/// 用 ffmpeg 把帧序列合成 mp4(H.264 + yuv420p,通用可播)。
async fn encode_video(frames_dir: &Path, fps: u32, out: &Path) -> Result<()> {
    let pattern = frames_dir.join("frame_%06d.png");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-y", "-framerate", &fps.to_string(), "-i"])
        .arg(&pattern)
        // libx264 + yuv420p 要求宽高为偶数;视口尺寸可能为奇数,补齐到偶数。
        .args([
            "-vf",
            "pad=ceil(iw/2)*2:ceil(ih/2)*2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(Error::Other(format!(
            "ffmpeg 合成失败(退出码 {:?});video 模式需要 ffmpeg,或改用 ScreencastMode::Imgs",
            s.code()
        ))),
        Err(e) => Err(Error::Other(format!(
            "无法运行 ffmpeg({e});video 模式需安装 ffmpeg,或改用 ScreencastMode::Imgs 保存帧序列"
        ))),
    }
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
