//! Juggler 连接:在 fd3/fd4 异步管道之上,做请求↔响应配对与事件分发。
//!
//! 模型:
//! - **一个写任务**:从 `mpsc` 取出已成帧的字节,顺序写入命令管道(避免对写端加锁跨 await)。
//! - **一个读任务**:循环读响应管道 → [`crate::codec`] 成帧 → 解析 →
//!   按 `id` 唤醒等待者(`oneshot`),或把事件广播出去(`broadcast`)。
//! - [`Connection::send`] 分配自增 `id`,登记 `oneshot`,发送后带超时等待。
//!
//! 事件用 `broadcast`:导航类事件低频;网络监听虽可能高频,但订阅者各自过滤,
//! 落后(`Lagged`)时跳过即可(监听器侧会处理)。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use crate::codec::{FrameDecoder, encode_frame};
use crate::protocol::message::{IncomingMessage, MessageKind, OutgoingMessage};
use crate::{Error, Result};

/// 默认请求超时。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// 事件广播缓冲容量(每个订阅者独立队列)。
const EVENT_CHANNEL_CAPACITY: usize = 4096;

/// 一条已分类的浏览器事件。
#[derive(Debug, Clone)]
pub struct Event {
    pub method: String,
    pub params: Value,
    /// page 会话事件带 `sessionId`;root 会话事件为 `None`。
    pub session_id: Option<String>,
}

/// `oneshot` 里传回的结果:`Ok(result)` 或 `Err(protocol message)`。
type ReplyResult = std::result::Result<Value, String>;
type PendingMap = Arc<Mutex<HashMap<i64, oneshot::Sender<ReplyResult>>>>;

/// 一个 Juggler 连接。克隆代价低(内部全是 `Arc`/句柄)。
#[derive(Clone)]
pub struct Connection {
    inner: Arc<Inner>,
}

struct Inner {
    next_id: AtomicI64,
    pending: PendingMap,
    cmd_tx: mpsc::UnboundedSender<Vec<u8>>,
    event_tx: broadcast::Sender<Event>,
}

impl Connection {
    /// 从一对 Juggler **fd3/fd4 管道**端建立连接(本地启动浏览器场景)。
    ///
    /// `writer`/`reader` 的具体类型按平台不同(unix 匿名管道 / windows 命名管道),
    /// 这里只要求实现 `AsyncWrite`/`AsyncRead`(见 [`crate::transport::PipeWriter`] /
    /// [`crate::transport::PipeReader`])。帧格式为 `<JSON>\0`(见 [`crate::codec`]),
    /// 由内部写/读任务负责加/去分隔符。
    pub fn from_pipe<W, R>(writer: W, reader: R) -> Self
    where
        W: AsyncWrite + Unpin + Send + 'static,
        R: AsyncRead + Unpin + Send + 'static,
    {
        let (inner, cmd_rx, pending, event_tx) = Self::scaffold();
        tokio::spawn(pipe_write_loop(writer, cmd_rx));
        tokio::spawn(pipe_read_loop(reader, pending, event_tx));
        Self { inner }
    }

    /// 从一个 **WebSocket** 流建立连接(连接到已在运行的浏览器 / 我们自己的 ws 服务端场景)。
    ///
    /// 与管道不同:ws 自带消息边界,**一条 ws 文本消息 = 一条完整 Juggler JSON**,无需 `\0`。
    /// 上层 [`send`](Self::send) / 事件分发逻辑与管道完全一致(复用同一 `Inner`/`dispatch`)。
    pub fn from_ws<S>(ws: WebSocketStream<S>) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (inner, cmd_rx, pending, event_tx) = Self::scaffold();
        let (sink, stream) = ws.split();
        tokio::spawn(ws_write_loop(sink, cmd_rx));
        tokio::spawn(ws_read_loop(stream, pending, event_tx));
        Self { inner }
    }

    /// 构建共享状态(pending 表 / 事件广播 / 命令队列)。命令队列里流动的是 **未成帧的原始 JSON**,
    /// 具体成帧(管道加 `\0` / ws 包成消息)交由各传输的写任务处理。
    fn scaffold() -> (
        Arc<Inner>,
        mpsc::UnboundedReceiver<Vec<u8>>,
        PendingMap,
        broadcast::Sender<Event>,
    ) {
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let inner = Arc::new(Inner {
            next_id: AtomicI64::new(1),
            pending: pending.clone(),
            cmd_tx,
            event_tx: event_tx.clone(),
        });
        (inner, cmd_rx, pending, event_tx)
    }

    /// 订阅事件流。返回的接收端只会收到订阅之后到达的事件。
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.event_tx.subscribe()
    }

    /// 发送一个请求并用默认超时等待响应。
    pub async fn send(
        &self,
        method: impl Into<String>,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value> {
        self.send_timeout(method, params, session_id, DEFAULT_TIMEOUT)
            .await
    }

    /// 发送一个请求并用指定超时等待响应。
    pub async fn send_timeout(
        &self,
        method: impl Into<String>,
        params: Value,
        session_id: Option<&str>,
        timeout: Duration,
    ) -> Result<Value> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(id, tx);

        let msg = OutgoingMessage::new(id, method, params, session_id.map(str::to_string));
        let json = msg.to_json_bytes()?;
        if self.inner.cmd_tx.send(json).is_err() {
            self.inner.pending.lock().await.remove(&id);
            return Err(Error::Transport(
                "命令写通道已关闭(子进程可能已退出)".into(),
            ));
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(message))) => Err(Error::Protocol(message)),
            Ok(Err(_)) => Err(Error::Transport("连接已关闭,响应通道被丢弃".into())),
            Err(_) => {
                self.inner.pending.lock().await.remove(&id);
                Err(Error::Timeout(timeout))
            }
        }
    }

    /// 发送一条不等待响应的消息(用于 `Browser.close` 等)。
    pub fn fire(&self, id: i64, method: impl Into<String>, params: Value) -> Result<()> {
        let msg = OutgoingMessage::new(id, method, params, None);
        let json = msg.to_json_bytes()?;
        self.inner
            .cmd_tx
            .send(json)
            .map_err(|_| Error::Transport("命令写通道已关闭".into()))
    }

    /// 发送一条**不等待响应**、但可指定 `session_id` 的消息(用于需要紧凑时序、不在意返回值的
    /// 场景,如拟人鼠标轨迹的密集 `Page.dispatchMouseEvent`)。
    ///
    /// 自增 `id` 从内部计数器分配(避免与等待响应的请求串台);回包到达时 `pending` 里查无此
    /// `id`,在 `dispatch` 处被安全丢弃。相比 `send`,省掉每事件一次往返(实测可把鼠标采样从
    /// ~60ms/点 提升到由调用方自定的 ~10ms/点,贴近真人 60~120Hz)。
    pub fn fire_session(
        &self,
        method: impl Into<String>,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<()> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = OutgoingMessage::new(id, method, params, session_id.map(str::to_string));
        let json = msg.to_json_bytes()?;
        self.inner
            .cmd_tx
            .send(json)
            .map_err(|_| Error::Transport("命令写通道已关闭".into()))
    }
}

/// 管道写任务:从队列取出原始 JSON,**追加 `\0` 成帧**后顺序写入命令管道。
/// 所有写端句柄丢弃后(`Connection` 析构)自动结束。
async fn pipe_write_loop<W>(mut writer: W, mut rx: mpsc::UnboundedReceiver<Vec<u8>>)
where
    W: AsyncWrite + Unpin,
{
    while let Some(json) = rx.recv().await {
        let frame = encode_frame(&json);
        if let Err(e) = writer.write_all(&frame).await {
            tracing::error!(error = %e, "写入命令管道失败,写任务退出");
            break;
        }
    }
    tracing::debug!("命令写任务(pipe)结束");
}

/// 管道读任务:循环读响应管道,按 `\0` 成帧、解析、分发。EOF 时唤醒所有等待者为错误。
async fn pipe_read_loop<R>(mut reader: R, pending: PendingMap, event_tx: broadcast::Sender<Event>)
where
    R: AsyncRead + Unpin,
{
    let mut decoder = FrameDecoder::new();
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) => {
                tracing::debug!("响应管道 EOF,子进程已退出");
                break;
            }
            Ok(n) => n,
            Err(e) => {
                tracing::error!(error = %e, "读取响应管道失败,读任务退出");
                break;
            }
        };
        decoder.push(&buf[..n]);
        while let Some(frame) = decoder.next_frame() {
            dispatch_json(&frame, &pending, &event_tx).await;
        }
    }

    fail_all_pending(&pending).await;
}

/// ws 写任务:从队列取出原始 JSON,作为**一条 ws 文本消息**发出(ws 自带消息边界,无需 `\0`)。
async fn ws_write_loop<S>(
    mut sink: SplitSink<WebSocketStream<S>, Message>,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    while let Some(json) = rx.recv().await {
        // serde_json 产出的一定是合法 UTF-8;极端情况下跳过该帧而不是 panic。
        let text = match String::from_utf8(json) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "命令 JSON 非 UTF-8,已跳过");
                continue;
            }
        };
        if let Err(e) = sink.send(Message::text(text)).await {
            tracing::error!(error = %e, "写入 ws 失败,写任务退出");
            break;
        }
    }
    let _ = sink.close().await;
    tracing::debug!("命令写任务(ws)结束");
}

/// ws 读任务:每条文本/二进制消息即一条完整 Juggler JSON,解析、分发。关闭/出错时唤醒等待者。
async fn ws_read_loop<S>(
    mut stream: SplitStream<WebSocketStream<S>>,
    pending: PendingMap,
    event_tx: broadcast::Sender<Event>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    while let Some(item) = stream.next().await {
        match item {
            Ok(Message::Text(t)) => dispatch_json(t.as_bytes(), &pending, &event_tx).await,
            Ok(Message::Binary(b)) => dispatch_json(&b, &pending, &event_tx).await,
            Ok(Message::Close(_)) => {
                tracing::debug!("ws 收到 Close,读任务退出");
                break;
            }
            // Ping/Pong/Frame 由 tungstenite 内部处理,忽略即可。
            Ok(_) => {}
            Err(e) => {
                tracing::error!(error = %e, "读取 ws 失败,读任务退出");
                break;
            }
        }
    }
    fail_all_pending(&pending).await;
}

/// 解析一段原始 JSON 字节并分发(响应唤醒等待者 / 事件广播)。
async fn dispatch_json(bytes: &[u8], pending: &PendingMap, event_tx: &broadcast::Sender<Event>) {
    match IncomingMessage::from_json_bytes(bytes) {
        Ok(msg) => dispatch(msg, pending, event_tx).await,
        Err(e) => tracing::warn!(error = %e, "解析入站帧失败,已跳过"),
    }
}

/// 断连兜底:把所有在途请求唤醒为错误,避免调用方永久挂起。
async fn fail_all_pending(pending: &PendingMap) {
    let mut map = pending.lock().await;
    for (_, tx) in map.drain() {
        let _ = tx.send(Err("连接已关闭".to_string()));
    }
}

async fn dispatch(msg: IncomingMessage, pending: &PendingMap, event_tx: &broadcast::Sender<Event>) {
    match msg.kind() {
        MessageKind::Response { id, result } => {
            if let Some(tx) = pending.lock().await.remove(&id) {
                let _ = tx.send(Ok(result));
            }
        }
        MessageKind::Error { id, message } => {
            if let Some(tx) = pending.lock().await.remove(&id) {
                let _ = tx.send(Err(message));
            }
        }
        MessageKind::Event {
            method,
            params,
            session_id,
        } => {
            // 没有订阅者时 `send` 会返回 Err,直接忽略。
            let _ = event_tx.send(Event {
                method,
                params,
                session_id,
            });
        }
        MessageKind::Unknown => {}
    }
}
