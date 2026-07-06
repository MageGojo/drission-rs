use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::engine::BrowserState;
use crate::protocol::{AxFormat, BackendKind, EngineCommand, JsonResponse};

#[derive(Clone)]
pub struct DrsMcp {
    state: Arc<Mutex<BrowserState>>,
    tool_router: ToolRouter<Self>,
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for DrsMcp {}

#[tool_router(router = tool_router)]
impl DrsMcp {
    pub async fn new(
        backend: BackendKind,
        headless: bool,
        user_data_dir: Option<PathBuf>,
    ) -> Result<Self> {
        Ok(Self {
            state: Arc::new(Mutex::new(
                BrowserState::launch(backend, headless, user_data_dir).await?,
            )),
            tool_router: Self::tool_router(),
        })
    }

    #[tool(name = "browser_open", description = "Open a URL in a new browser tab")]
    async fn browser_open(&self, Parameters(req): Parameters<OpenParams>) -> CallToolResult {
        self.exec(EngineCommand::Open { url: req.url }).await
    }

    #[tool(name = "browser_tabs", description = "List browser tabs")]
    async fn browser_tabs(&self) -> CallToolResult {
        self.exec(EngineCommand::Tabs).await
    }

    #[tool(
        name = "browser_use_tab",
        description = "Switch active tab by drs tab id"
    )]
    async fn browser_use_tab(&self, Parameters(req): Parameters<TabIdParams>) -> CallToolResult {
        self.exec(EngineCommand::UseTab { tab_id: req.tab_id })
            .await
    }

    #[tool(
        name = "browser_ax",
        description = "Get the active page accessibility snapshot"
    )]
    async fn browser_ax(&self, Parameters(req): Parameters<AxParams>) -> CallToolResult {
        self.exec(EngineCommand::Ax {
            format: if req.json.unwrap_or(false) {
                AxFormat::Json
            } else {
                AxFormat::Outline
            },
        })
        .await
    }

    #[tool(name = "browser_html", description = "Get the active page HTML")]
    async fn browser_html(&self) -> CallToolResult {
        self.exec(EngineCommand::Html).await
    }

    #[tool(
        name = "browser_text",
        description = "Get visible text from the active page or selector"
    )]
    async fn browser_text(&self, Parameters(req): Parameters<TextParams>) -> CallToolResult {
        self.exec(EngineCommand::Text {
            selector: req.selector,
        })
        .await
    }

    #[tool(
        name = "browser_eval",
        description = "Evaluate JavaScript in the active tab"
    )]
    async fn browser_eval(&self, Parameters(req): Parameters<EvalParams>) -> CallToolResult {
        self.exec(EngineCommand::Eval { js: req.js }).await
    }

    #[tool(
        name = "browser_click",
        description = "Click an element in the active tab"
    )]
    async fn browser_click(&self, Parameters(req): Parameters<SelectorParams>) -> CallToolResult {
        self.exec(EngineCommand::Click {
            selector: req.selector,
        })
        .await
    }

    #[tool(
        name = "browser_type",
        description = "Type text into an element in the active tab"
    )]
    async fn browser_type(&self, Parameters(req): Parameters<TypeParams>) -> CallToolResult {
        self.exec(EngineCommand::Type {
            selector: req.selector,
            text: req.text,
        })
        .await
    }

    #[tool(
        name = "browser_wait",
        description = "Wait for an element to be displayed"
    )]
    async fn browser_wait(&self, Parameters(req): Parameters<WaitParams>) -> CallToolResult {
        self.exec(EngineCommand::Wait {
            selector: req.selector,
            timeout_ms: req.timeout_ms,
        })
        .await
    }

    #[tool(
        name = "browser_screenshot",
        description = "Save or return a screenshot"
    )]
    async fn browser_screenshot(
        &self,
        Parameters(req): Parameters<ScreenshotParams>,
    ) -> CallToolResult {
        let result = self
            .exec_response(EngineCommand::Screenshot {
                out: req.out,
                full: req.full.unwrap_or(false),
                inline: req.inline.unwrap_or(false),
            })
            .await;
        let mut tool = result_to_tool(result);
        if req.inline.unwrap_or(false) {
            if let Some(b64) = tool
                .structured_content
                .as_ref()
                .and_then(|data| data.get("data"))
                .and_then(|data| data.get("base64"))
                .and_then(Value::as_str)
            {
                tool.content
                    .push(ContentBlock::image(b64.to_string(), "image/png"));
            }
        }
        tool
    }

    #[tool(name = "network_listen_start", description = "Start network listening")]
    async fn network_listen_start(
        &self,
        Parameters(req): Parameters<ListenStartParams>,
    ) -> CallToolResult {
        self.exec(EngineCommand::ListenStart {
            keywords: req.keywords.unwrap_or_default(),
            xhr_only: req.xhr_only.unwrap_or(false),
        })
        .await
    }

    #[tool(
        name = "network_listen_wait",
        description = "Wait for captured network packets"
    )]
    async fn network_listen_wait(
        &self,
        Parameters(req): Parameters<ListenWaitParams>,
    ) -> CallToolResult {
        self.exec(EngineCommand::ListenWait {
            count: req.count.unwrap_or(1),
            timeout_ms: req.timeout_ms,
        })
        .await
    }

    #[tool(name = "network_listen_stop", description = "Stop network listening")]
    async fn network_listen_stop(&self) -> CallToolResult {
        self.exec(EngineCommand::ListenStop).await
    }

    #[tool(
        name = "browser_pass_cf",
        description = "Try to pass a Cloudflare challenge"
    )]
    async fn browser_pass_cf(&self, Parameters(req): Parameters<TimeoutParams>) -> CallToolResult {
        self.exec(EngineCommand::PassCf {
            timeout_ms: req.timeout_ms,
        })
        .await
    }
}

impl DrsMcp {
    async fn exec(&self, command: EngineCommand) -> CallToolResult {
        result_to_tool(self.exec_response(command).await)
    }

    async fn exec_response(&self, command: EngineCommand) -> JsonResponse {
        let mut state = self.state.lock().await;
        match state.execute(command).await {
            Ok(result) => JsonResponse::ok(result.data),
            Err(e) => JsonResponse::err("command_failed", e.to_string(), None),
        }
    }
}

pub async fn run_mcp(
    backend: BackendKind,
    headless: bool,
    user_data_dir: Option<PathBuf>,
) -> Result<()> {
    let service = DrsMcp::new(backend, headless, user_data_dir)
        .await?
        .serve(stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}

fn result_to_tool(response: JsonResponse) -> CallToolResult {
    let value = response.into_value();
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        CallToolResult::structured(value)
    } else {
        CallToolResult::structured_error(value)
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct OpenParams {
    url: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct TabIdParams {
    tab_id: u64,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct AxParams {
    json: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct TextParams {
    selector: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct EvalParams {
    js: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct SelectorParams {
    selector: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct TypeParams {
    selector: String,
    text: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct WaitParams {
    selector: String,
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct ScreenshotParams {
    out: Option<PathBuf>,
    full: Option<bool>,
    inline: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct ListenStartParams {
    keywords: Option<Vec<String>>,
    xhr_only: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct ListenWaitParams {
    count: Option<usize>,
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct TimeoutParams {
    timeout_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn tool_names_are_stable() {
        const MCP_TOOL_NAMES: &[&str] = &[
            "browser_open",
            "browser_tabs",
            "browser_use_tab",
            "browser_ax",
            "browser_html",
            "browser_text",
            "browser_eval",
            "browser_click",
            "browser_type",
            "browser_wait",
            "browser_screenshot",
            "network_listen_start",
            "network_listen_wait",
            "network_listen_stop",
            "browser_pass_cf",
        ];
        assert_eq!(
            MCP_TOOL_NAMES,
            &[
                "browser_open",
                "browser_tabs",
                "browser_use_tab",
                "browser_ax",
                "browser_html",
                "browser_text",
                "browser_eval",
                "browser_click",
                "browser_type",
                "browser_wait",
                "browser_screenshot",
                "network_listen_start",
                "network_listen_wait",
                "network_listen_stop",
                "browser_pass_cf",
            ]
        );
    }
}
