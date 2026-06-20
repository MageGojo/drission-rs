//! 浏览器 **WS 服务端**:启动 Camoufox(走 fd3/fd4 管道),把**原始 Juggler 协议**桥接成一个
//! WebSocket 端点,供 [`Browser::connect`](crate::browser::Browser::connect) 或其它进程/机器接入。
//!
//! 背景:当前 Camoufox 二进制已移除浏览器内置的 `-juggler <port>`(ws)模式,只支持 `-juggler-pipe`;
//! 而 Camoufox 自带的 `python -m camoufox server` 暴露的是 Playwright 的 RPC 协议(我们的 Juggler 客户端
//! 无法直接讲)。因此我们**自建中转**:进程内同时持有管道与一个 ws 监听,做"消息级"双向转发。
//!
//! 转发模型(单浏览器实例,可顺序/并发接入多个 ws 客户端;客户端各自管理自增 id):
//! - **一个管道写任务**:收 ws 客户端发来的原始 JSON → 追加 `\0` → 写 fd3。
//! - **一个管道读任务**:读 fd4 → 按 `\0` 拆帧 → `broadcast` 给所有当前 ws 客户端。
//! - **每个 ws 客户端**两个泵:ws→管道、管道→ws。
//!
//! 浏览器在服务存活期间一直在线;客户端断开不影响浏览器,可再次接入(对标 DrissionPage 接管已开浏览器)。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{Response as HttpResponse, StatusCode};

use crate::codec::{FrameDecoder, encode_frame};
use crate::launcher::{self, BrowserOptions, Launched};
use crate::transport::Child;
use crate::{Error, Result};

/// 每个 ws 客户端独立的入站帧广播缓冲。
const FRAME_CHANNEL_CAPACITY: usize = 4096;

/// 一个对外暴露 Juggler-over-WebSocket 端点的浏览器服务。
///
/// `launch` 后浏览器持续运行,`ws_endpoint()` 即可分发给客户端;`stop()`(或 drop)关闭浏览器并清理。
pub struct BrowserServer {
    endpoint: String,
    port: u16,
    token: String,
    child: Mutex<Option<Child>>,
    profile_dir: PathBuf,
    profile_is_temp: bool,
    accept_task: JoinHandle<()>,
}

impl BrowserServer {
    /// 启动浏览器并在 `127.0.0.1` 的**随机端口**、随机 token 路径上开放 ws 端点。
    pub async fn launch(opts: BrowserOptions) -> Result<Self> {
        Self::launch_on(opts, "127.0.0.1", 0, None).await
    }

    /// 指定监听地址 / 端口(0 = 自动)/ ws 路径(`None` = 随机 token)启动。
    pub async fn launch_on(
        opts: BrowserOptions,
        host: &str,
        port: u16,
        ws_path: Option<&str>,
    ) -> Result<Self> {
        let Launched {
            child,
            writer,
            reader,
            profile_dir,
            profile_is_temp,
        } = launcher::launch(&opts).await?;

        // 管道写任务(原始 JSON → 成帧 → fd3)与读任务(fd4 → 拆帧 → 广播)。
        let (pipe_tx, pipe_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        tokio::spawn(pipe_writer(writer, pipe_rx));
        let (frame_tx, _) = broadcast::channel::<Vec<u8>>(FRAME_CHANNEL_CAPACITY);
        tokio::spawn(pipe_reader(reader, frame_tx.clone()));

        let listener = TcpListener::bind((host, port))
            .await
            .map_err(|e| Error::Transport(format!("ws 服务端绑定 {host}:{port} 失败: {e}")))?;
        let addr = listener
            .local_addr()
            .map_err(|e| Error::Transport(format!("读取 ws 监听地址失败: {e}")))?;
        let token = ws_path
            .map(|s| s.trim_start_matches('/').to_string())
            .unwrap_or_else(random_token);

        let want_path = format!("/{token}");
        let accept_task = tokio::spawn(accept_loop(listener, want_path, pipe_tx, frame_tx));

        let endpoint = format!("ws://{}:{}/{}", addr.ip(), addr.port(), token);
        tracing::info!(%endpoint, "BrowserServer 已就绪");

        Ok(Self {
            endpoint,
            port: addr.port(),
            token,
            child: Mutex::new(Some(child)),
            profile_dir,
            profile_is_temp,
            accept_task,
        })
    }

    /// 可分发给客户端的 ws 端点:`ws://host:port/token`。
    pub fn ws_endpoint(&self) -> &str {
        &self.endpoint
    }

    /// 实际监听端口。
    pub fn port(&self) -> u16 {
        self.port
    }

    /// ws 路径里的 token。
    pub fn token(&self) -> &str {
        &self.token
    }

    /// 停止服务:关闭 accept 循环、杀掉浏览器、清理临时 profile。
    pub async fn stop(&self) -> Result<()> {
        self.accept_task.abort();
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
        }
        if self.profile_is_temp {
            let _ = tokio::fs::remove_dir_all(&self.profile_dir).await;
        }
        Ok(())
    }
}

impl Drop for BrowserServer {
    fn drop(&mut self) {
        self.accept_task.abort();
        if let Ok(mut g) = self.child.try_lock()
            && let Some(mut c) = g.take()
        {
            let _ = c.start_kill();
        }
        if self.profile_is_temp {
            let _ = std::fs::remove_dir_all(&self.profile_dir);
        }
    }
}

/// 生成一个非加密、足够唯一的 token(本地 dev 端点防误连即可)。
fn random_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}{:x}{:x}", nanos, std::process::id(), n)
}

/// 管道写任务:原始 JSON → 追加 `\0` → 写 fd3。所有 `pipe_tx` 丢弃后结束。
async fn pipe_writer<W>(mut writer: W, mut rx: mpsc::UnboundedReceiver<Vec<u8>>)
where
    W: AsyncWrite + Unpin,
{
    while let Some(json) = rx.recv().await {
        let frame = encode_frame(&json);
        if writer.write_all(&frame).await.is_err() {
            break;
        }
    }
}

/// 管道读任务:fd4 → 按 `\0` 拆帧 → 广播原始 JSON 给所有 ws 客户端。
async fn pipe_reader<R>(mut reader: R, frame_tx: broadcast::Sender<Vec<u8>>)
where
    R: AsyncRead + Unpin,
{
    let mut decoder = FrameDecoder::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        decoder.push(&buf[..n]);
        while let Some(frame) = decoder.next_frame() {
            // 没有任何客户端时 `send` 返回 Err,直接忽略(浏览器空闲事件无需保留)。
            let _ = frame_tx.send(frame);
        }
    }
}

/// 监听循环:每个新 TCP 连接做 ws 握手(校验 token 路径)后开一对转发泵。
///
/// **单活动客户端**:原始 Juggler 是单逻辑连接(单自增 id 空间),多个客户端复用同一管道会导致
/// id 串台。故同一时刻只接纳一个 ws 客户端;客户端断开后槽位释放,可被再次接管(顺序复用)。
// 握手回调的 Err 类型由 tungstenite 固定为 `http::Response<Option<String>>`(较大),无法变更。
#[allow(clippy::result_large_err)]
async fn accept_loop(
    listener: TcpListener,
    want_path: String,
    pipe_tx: mpsc::UnboundedSender<Vec<u8>>,
    frame_tx: broadcast::Sender<Vec<u8>>,
) {
    let active = Arc::new(AtomicBool::new(false));
    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!(error = %e, "ws accept 失败,服务端监听退出");
                break;
            }
        };
        let pipe_tx = pipe_tx.clone();
        let frame_tx = frame_tx.clone();
        let want = want_path.clone();
        let active = active.clone();
        tokio::spawn(async move {
            let check = move |req: &Request,
                              resp: Response|
                  -> std::result::Result<Response, ErrorResponse> {
                if req.uri().path() == want {
                    Ok(resp)
                } else {
                    let err = HttpResponse::builder()
                        .status(StatusCode::FORBIDDEN)
                        .body(Some("invalid juggler ws path".to_string()))
                        .expect("build error response");
                    Err(err)
                }
            };
            let ws = match accept_hdr_async(stream, check).await {
                Ok(ws) => ws,
                Err(e) => {
                    tracing::debug!(error = %e, "ws 握手失败(可能 token 不匹配)");
                    return;
                }
            };
            // 抢占单客户端槽位;已被占用则关闭新连接(拒绝)。
            if active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                tracing::warn!("已有活动 ws 客户端,拒绝新连接(单实例)");
                let (mut sink, _stream) = ws.split();
                let _ = sink.close().await;
                return;
            }
            let frame_rx = frame_tx.subscribe();
            bridge_client(ws, pipe_tx, frame_rx).await;
            active.store(false, Ordering::Release);
        });
    }
}

/// 单个 ws 客户端的双向转发:ws→管道(发命令)、管道→ws(回响应/事件)。任一方向断开即收尾。
async fn bridge_client<S>(
    ws: WebSocketStream<S>,
    pipe_tx: mpsc::UnboundedSender<Vec<u8>>,
    mut frame_rx: broadcast::Receiver<Vec<u8>>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut stream) = ws.split();

    // 管道 → ws:把广播来的每条 JSON 作为文本消息发给客户端。
    let to_ws = tokio::spawn(async move {
        loop {
            match frame_rx.recv().await {
                Ok(json) => {
                    let text = match String::from_utf8(json) {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    if sink.send(Message::text(text)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        let _ = sink.close().await;
    });

    // ws → 管道:把客户端发来的每条命令 JSON 投递给管道写任务。
    while let Some(item) = stream.next().await {
        match item {
            Ok(Message::Text(t)) => {
                if pipe_tx.send(t.as_bytes().to_vec()).is_err() {
                    break;
                }
            }
            Ok(Message::Binary(b)) => {
                if pipe_tx.send(b.to_vec()).is_err() {
                    break;
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }

    to_ws.abort();
}
