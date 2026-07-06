use std::path::PathBuf;

use clap::ValueEnum;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum BackendKind {
    Cdp,
    Camoufox,
}

impl Default for BackendKind {
    fn default() -> Self {
        Self::Cdp
    }
}

impl std::fmt::Display for BackendKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendKind::Cdp => f.write_str("cdp"),
            BackendKind::Camoufox => f.write_str("camoufox"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateFile {
    pub host: String,
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub backend: BackendKind,
}

impl StateFile {
    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonRequest {
    pub token: String,
    pub command: EngineCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum EngineCommand {
    Status,
    Stop,
    Open {
        url: String,
    },
    Tabs,
    UseTab {
        tab_id: u64,
    },
    Close {
        tab_id: Option<u64>,
    },
    Ax {
        format: AxFormat,
    },
    Html,
    Text {
        selector: Option<String>,
    },
    Eval {
        js: String,
    },
    Screenshot {
        out: Option<PathBuf>,
        full: bool,
        inline: bool,
    },
    Click {
        selector: String,
    },
    Type {
        selector: String,
        text: String,
    },
    Press {
        key: String,
        selector: Option<String>,
    },
    Wait {
        selector: String,
        timeout_ms: Option<u64>,
    },
    ListenStart {
        keywords: Vec<String>,
        xhr_only: bool,
    },
    ListenWait {
        count: usize,
        timeout_ms: Option<u64>,
    },
    ListenStop,
    PassCf {
        timeout_ms: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AxFormat {
    Outline,
    Json,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JsonResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

impl JsonResponse {
    pub fn ok(data: impl Serialize) -> Self {
        Self {
            ok: true,
            data: Some(serde_json::to_value(data).unwrap_or(Value::Null)),
            error: None,
        }
    }

    pub fn err(code: impl Into<String>, message: impl Into<String>, hint: Option<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(ResponseError {
                code: code.into(),
                message: message.into(),
                hint,
            }),
        }
    }

    pub fn into_value(self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResponseError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TabSummary {
    pub id: u64,
    pub active: bool,
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PacketSummary {
    pub url: String,
    pub method: String,
    pub resource_type: String,
    pub status: u16,
    pub status_text: String,
    pub request_headers: Vec<(String, String)>,
    pub response_headers: Vec<(String, String)>,
    pub body: String,
    pub body_base64: String,
}

impl From<drission::net::DataPacket> for PacketSummary {
    fn from(p: drission::net::DataPacket) -> Self {
        Self {
            url: p.url,
            method: p.method,
            resource_type: p.resource_type,
            status: p.response.status,
            status_text: p.response.status_text,
            request_headers: p.request.headers,
            response_headers: p.response.headers,
            body: p.response.body,
            body_base64: p.response.body_base64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_response_ok_shape() {
        let response = JsonResponse::ok(serde_json::json!({ "answer": 42 }));
        let value = response.into_value();
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["answer"], 42);
        assert!(value.get("error").is_none());
    }

    #[test]
    fn json_response_error_shape() {
        let response = JsonResponse::err("x", "failed", Some("try again".to_string()));
        let value = response.into_value();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "x");
        assert_eq!(value["error"]["hint"], "try again");
    }
}
