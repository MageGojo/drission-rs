//! 传输层:启动 Camoufox 子进程,并建立 Juggler 的 fd3/fd4 异步管道。
//!
//! Juggler 通过 `-juggler-pipe` 在子进程的 **fd3(读命令)/ fd4(写响应)** 上通信:
//! - 父进程把命令写入一个管道,其读端是子进程的 **fd3**;
//! - 子进程把响应写入另一个管道,其写端是子进程的 **fd4**,父进程读其读端。
//!
//! 帧格式由 [`crate::codec`] 负责(UTF-8 JSON + 单个 `\0`)。本层只负责字节进出 +
//! 子进程生命周期 + stderr 捕获(就绪检测与日志由 [`crate::launcher::process`] 使用)。
//!
//! 跨平台:
//! - **unix(macOS / Linux)**:`os_pipe` 匿名管道 + `command-fds` 映射子进程 fd3/fd4,
//!   父端用 tokio `unix::pipe` 异步读写。
//! - **windows**:命名管道(父端 tokio `NamedPipeServer` 异步,子端同步可继承句柄),
//!   句柄经 **CRT `lpReserved2` 块**注入子进程 fd3/fd4(等价 libuv/Node 的做法,
//!   Camoufox/Firefox 的 `wmain`/`LauncherProcessWin` 用 `_get_osfhandle(3/4)` 取用)。
//!
//! 上层([`crate::protocol::Connection::from_pipe`] / Camoufox 后端的 `BrowserServer`)
//! 对 `writer`/`reader` 只要求 `AsyncWrite`/`AsyncRead`,故两平台的具体类型经下方别名统一。

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::{Spawned, spawn};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{Spawned, spawn};
// Windows 进程树兜底杀手(Job Object,KILL_ON_JOB_CLOSE);CDP 后端也用它绑 Chrome 进程树。
// 该 re-export 仅在 `feature="cdp"` 时被 cdp/browser.rs 经 `crate::transport::JobHandle` 消费;
// 纯 camoufox(无 cdp)等组合下无 crate 内消费方(camoufox 的 transport/windows.rs 用同模块内的
// `JobHandle` 而非此 re-export)→ 豁免 unused_imports,避免 windows clippy `-D warnings` 误报。
#[cfg(windows)]
#[allow(unused_imports)]
pub(crate) use windows::JobHandle;

// ── 跨平台传输类型别名 ────────────────────────────────────────────────────────
// 子进程句柄:unix 直接用 tokio 的 `Child`;windows 用自管理的 `WinChild`(同名方法
// `wait().await` / `kill().await` / `start_kill()`,故消费侧代码两平台一致)。
#[cfg(unix)]
pub use tokio::process::Child;
#[cfg(windows)]
pub use windows::WinChild as Child;

// Juggler 命令写端(父→子 fd3):unix 管道 `Sender` / windows 命名管道 `NamedPipeServer`。
#[cfg(unix)]
pub use tokio::net::unix::pipe::Sender as PipeWriter;
#[cfg(windows)]
pub use tokio::net::windows::named_pipe::NamedPipeServer as PipeWriter;

// Juggler 响应读端(子 fd4→父):unix 管道 `Receiver` / windows 命名管道 `NamedPipeServer`。
#[cfg(unix)]
pub use tokio::net::unix::pipe::Receiver as PipeReader;
#[cfg(windows)]
pub use tokio::net::windows::named_pipe::NamedPipeServer as PipeReader;

use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use crate::{Error, Result};

/// 连接到一个 **Juggler WebSocket 端点**(`ws://host:port/token`),返回已就绪的 ws 流。
///
/// 端点必须讲**原始 Juggler 协议**(由 Camoufox 后端的 `BrowserServer` 暴露),
/// 而不是 Camoufox `python -m camoufox server` 那种 Playwright RPC 协议(二者不兼容)。
pub async fn ws_connect(url: &str) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
    let (ws, _resp) = connect_async(url)
        .await
        .map_err(|e| Error::Transport(format!("ws 连接 {url} 失败: {e}")))?;
    Ok(ws)
}
