//! Juggler 线消息类型。
//!
//! 区分三类消息:
//! - **请求**(Client → Browser):带 `id` + `method` + `params`,page 会话还带 `sessionId`。
//! - **响应**(Browser → Client):带 `id` + (`result` 或 `error`)。
//! - **事件**(Browser → Client,非请求触发):带 `method` + `params`,无 `id`。
//!
//! 关键约束(来自协议规范):
//! - root 会话的消息**不能**带 `sessionId` 字段(多余字段会被服务端校验拒绝)。
//! - Optional 字段必须用 `skip_serializing_if` 跳过,绝不能序列化成 `null`。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `Browser.close` 使用的保留消息 id,响应到达后客户端静默丢弃。
pub const BROWSER_CLOSE_MESSAGE_ID: i64 = -9999;

/// 发往浏览器的请求帧。
#[derive(Debug, Clone, Serialize)]
pub struct OutgoingMessage {
    pub id: i64,
    pub method: String,
    pub params: Value,
    /// page 会话带 UUID;root 会话必须为 `None`(从而不出现在 JSON 中)。
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl OutgoingMessage {
    pub fn new(id: i64, method: impl Into<String>, params: Value, session_id: Option<String>) -> Self {
        Self {
            id,
            method: method.into(),
            params,
            session_id,
        }
    }

    /// 序列化为不含尾部分隔符的 JSON 字节。
    pub fn to_json_bytes(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec(self)
    }
}

/// 服务端返回的协议错误体(`error` 字段)。客户端只关心 `message`。
#[derive(Debug, Clone, Deserialize)]
pub struct ProtocolErrorBody {
    pub message: String,
    #[serde(default)]
    pub data: Option<String>,
}

/// 从浏览器收到的任意一帧(响应或事件)。
///
/// 通过 `id`/`method` 的有无来判别类型(见 [`IncomingMessage::kind`])。
#[derive(Debug, Clone, Deserialize)]
pub struct IncomingMessage {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default, rename = "sessionId")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<ProtocolErrorBody>,
}

/// 一帧入站消息的语义分类。
#[derive(Debug)]
pub enum MessageKind {
    /// 成功响应,携带 `result`。
    Response { id: i64, result: Value },
    /// 失败响应,携带错误信息。
    Error { id: i64, message: String },
    /// 非请求触发的事件。
    Event {
        method: String,
        params: Value,
        session_id: Option<String>,
    },
    /// 无法识别的帧(既不是有效响应也不是事件),通常应被忽略。
    Unknown,
}

impl IncomingMessage {
    /// 从 JSON 字节解析一帧。
    pub fn from_json_bytes(bytes: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice(bytes)
    }

    /// 判别消息类型。判别规则与 Playwright 一致:有 `id` 即响应,否则若有 `method` 即事件。
    pub fn kind(self) -> MessageKind {
        match (self.id, self.method) {
            (Some(id), _) => {
                if let Some(err) = self.error {
                    MessageKind::Error {
                        id,
                        message: err.message,
                    }
                } else {
                    MessageKind::Response {
                        id,
                        result: self.result.unwrap_or(Value::Null),
                    }
                }
            }
            (None, Some(method)) => MessageKind::Event {
                method,
                params: self.params.unwrap_or(Value::Null),
                session_id: self.session_id,
            },
            (None, None) => MessageKind::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn root_message_omits_session_id() {
        let m = OutgoingMessage::new(1, "Browser.enable", json!({}), None);
        let s = String::from_utf8(m.to_json_bytes().unwrap()).unwrap();
        assert!(!s.contains("sessionId"), "root 会话不应出现 sessionId: {s}");
        assert_eq!(s, r#"{"id":1,"method":"Browser.enable","params":{}}"#);
    }

    #[test]
    fn page_message_includes_session_id() {
        let m = OutgoingMessage::new(
            2,
            "Page.navigate",
            json!({"url":"https://example.com","frameId":"main"}),
            Some("abc123".into()),
        );
        let s = String::from_utf8(m.to_json_bytes().unwrap()).unwrap();
        assert!(s.contains(r#""sessionId":"abc123""#), "{s}");
    }

    #[test]
    fn parse_success_response() {
        let raw = br#"{"id":1,"result":{"ok":true}}"#;
        match IncomingMessage::from_json_bytes(raw).unwrap().kind() {
            MessageKind::Response { id, result } => {
                assert_eq!(id, 1);
                assert_eq!(result, json!({"ok":true}));
            }
            other => panic!("期望 Response,得到 {other:?}"),
        }
    }

    #[test]
    fn parse_error_response() {
        let raw = br#"{"id":3,"error":{"message":"boom","data":"stack"}}"#;
        match IncomingMessage::from_json_bytes(raw).unwrap().kind() {
            MessageKind::Error { id, message } => {
                assert_eq!(id, 3);
                assert_eq!(message, "boom");
            }
            other => panic!("期望 Error,得到 {other:?}"),
        }
    }

    #[test]
    fn parse_event() {
        let raw = br#"{"method":"Page.load","params":{},"sessionId":"s1"}"#;
        match IncomingMessage::from_json_bytes(raw).unwrap().kind() {
            MessageKind::Event {
                method, session_id, ..
            } => {
                assert_eq!(method, "Page.load");
                assert_eq!(session_id.as_deref(), Some("s1"));
            }
            other => panic!("期望 Event,得到 {other:?}"),
        }
    }
}
