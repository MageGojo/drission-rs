//! Juggler 协议层。
//!
//! - [`message`][]:线消息类型(请求 / 响应 / 事件)。
//! - [`connection`][]:基于 fd3/4 传输的连接,负责请求/响应配对与事件分发。
//!
//! 会话路由(root 无 `sessionId`;page 带 UUID)直接通过 [`connection::Connection::send`]
//! 的 `session_id` 参数携带,由各 `Tab` 持有自身 `sessionId`,无需独立的 session 模块。

pub mod connection;
pub mod message;

pub use connection::{Connection, DEFAULT_TIMEOUT, Event};
pub use message::{
    BROWSER_CLOSE_MESSAGE_ID, IncomingMessage, MessageKind, OutgoingMessage, ProtocolErrorBody,
};
