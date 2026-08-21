use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::daemon;
use crate::engine::BrowserState;
use crate::identity_cmd;
use crate::paths;
use crate::protocol::{
    AxFormat, BackendKind, EngineCommand, IdentityGate, IdentityGatePreset, JsonResponse,
    McpToolset,
};

/// Core browse loop exposed by default (`DRS_MCP_TOOLS=core`).
const CORE_TOOLS: &[&str] = &[
    "browser_open",
    "browser_tabs",
    "browser_use_tab",
    "browser_close",
    "browser_snapshot",
    "browser_extract",
    "browser_click",
    "browser_type",
    "browser_press",
    "browser_wait",
    "browser_screenshot",
    "browser_network",
    "browser_console",
    "browser_download",
];

/// Extended browser/network tools (included in `browser` + `all`).
const BROWSER_EXTRA_TOOLS: &[&str] = &[
    "browser_ax",
    "browser_markdown",
    "browser_html",
    "browser_title",
    "browser_url",
    "browser_text",
    "browser_eval",
    "browser_save_state",
    "browser_load_state",
    "browser_ocr",
    "browser_solve_slider",
    "network_listen_start",
    "network_listen_wait",
    "network_listen_stop",
    "network_block",
    "network_mock",
    "network_rewrite",
    "network_record",
    "network_stop",
    "browser_pdf",
    "browser_inspect",
    "browser_record",
    "browser_context_list",
    "browser_context_use",
    "browser_context_close",
    "profile_list",
    "profile_show",
    "browser_pass_cf",
    "browser_identity",
    "browser_identity_pool",
];

/// Identity asset governance tools (included in `identity` + `all`).
const IDENTITY_TOOLS: &[&str] = &[
    "browser_identity",
    "browser_identity_pool",
    "profile_list",
    "profile_show",
    "identity_assets_validate",
    "identity_assets_status",
    "identity_assets_forecast",
    "identity_assets_gate",
    "identity_assets_select",
    "identity_assets_release",
    "identity_assets_reconcile_runtime",
    "identity_assets_health",
    "identity_assets_sweep",
];

const ALL_MCP_TOOLS: &[&str] = &[
    "browser_open",
    "browser_tabs",
    "browser_use_tab",
    "browser_close",
    "browser_ax",
    "browser_snapshot",
    "browser_markdown",
    "browser_html",
    "browser_title",
    "browser_url",
    "browser_extract",
    "browser_text",
    "browser_eval",
    "browser_click",
    "browser_type",
    "browser_press",
    "browser_wait",
    "browser_save_state",
    "browser_load_state",
    "browser_ocr",
    "browser_solve_slider",
    "browser_screenshot",
    "network_listen_start",
    "network_listen_wait",
    "network_listen_stop",
    "network_block",
    "network_mock",
    "network_rewrite",
    "network_record",
    "network_stop",
    "browser_network",
    "browser_console",
    "browser_download",
    "browser_pdf",
    "browser_inspect",
    "browser_record",
    "browser_context_list",
    "browser_context_use",
    "browser_context_close",
    "profile_list",
    "profile_show",
    "browser_identity",
    "browser_identity_pool",
    "browser_pass_cf",
    "identity_assets_validate",
    "identity_assets_status",
    "identity_assets_forecast",
    "identity_assets_gate",
    "identity_assets_select",
    "identity_assets_release",
    "identity_assets_reconcile_runtime",
    "identity_assets_health",
    "identity_assets_sweep",
];

/// How to (re)launch the shared daemon when it is missing.
#[derive(Clone)]
struct DaemonConfig {
    backend: BackendKind,
    headless: bool,
    user_data_dir: Option<PathBuf>,
}

/// Where the MCP browser commands actually run.
#[derive(Clone)]
enum DrsBackend {
    /// Forward every browser command to the shared persistent `drs serve`
    /// daemon. The browser lives in the long-running daemon process, so tabs
    /// and login state survive MCP server restarts (the default).
    Daemon(DaemonConfig),
    /// Hold a browser inside this MCP process (`drs mcp --standalone`).
    Local(Arc<Mutex<BrowserState>>),
}

#[derive(Clone)]
pub struct DrsMcp {
    backend: DrsBackend,
    toolset: McpToolset,
    tool_router: ToolRouter<Self>,
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for DrsMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("drs", env!("CARGO_PKG_VERSION"))
                    .with_title("drs browser")
                    .with_description(
                        "Persistent browser MCP. Cursor agent id is usually `user-drs` (config key `drs`).",
                    ),
            )
            .with_instructions(mcp_instructions(self.toolset))
    }
}

#[tool_router(router = tool_router)]
impl DrsMcp {
    /// Attach to the shared persistent daemon browser (default MCP mode).
    fn attached(config: DaemonConfig, toolset: McpToolset) -> Self {
        Self {
            backend: DrsBackend::Daemon(config),
            toolset,
            tool_router: Self::router_for(toolset),
        }
    }

    /// Launch a browser inside this process (`--standalone`).
    async fn standalone(
        backend: BackendKind,
        headless: bool,
        user_data_dir: Option<PathBuf>,
        toolset: McpToolset,
    ) -> Result<Self> {
        Ok(Self {
            backend: DrsBackend::Local(Arc::new(Mutex::new(
                BrowserState::launch(backend, headless, user_data_dir).await?,
            ))),
            toolset,
            tool_router: Self::router_for(toolset),
        })
    }

    fn router_for(toolset: McpToolset) -> ToolRouter<Self> {
        let mut router = Self::tool_router();
        let allowed = allowed_tools(toolset);
        for name in ALL_MCP_TOOLS {
            if !allowed.contains(*name) {
                router.remove_route(name);
            }
        }
        router
    }

    #[tool(name = "browser_open", description = "Open a URL in a new browser tab")]
    async fn browser_open(&self, Parameters(req): Parameters<OpenParams>) -> CallToolResult {
        self.exec(EngineCommand::Open {
            url: req.url,
            context: req.context,
        })
        .await
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

    #[tool(
        name = "browser_snapshot",
        description = "AI-readable snapshot of the active page: title/url, interesting controls with refs (e1..), HTML→Markdown body, and short text. Prefer this over browser_html/browser_ax."
    )]
    async fn browser_snapshot(
        &self,
        Parameters(req): Parameters<SnapshotParams>,
    ) -> CallToolResult {
        self.exec(EngineCommand::Snapshot {
            budget_ms: req.budget_ms,
            max_items: req.max_items,
            max_text_chars: req.max_text_chars,
            max_markdown_chars: req.max_markdown_chars,
        })
        .await
    }

    #[tool(
        name = "browser_markdown",
        description = "Convert the active page body HTML to Markdown (htmd). Prefer browser_snapshot for interactive refs + markdown together."
    )]
    async fn browser_markdown(
        &self,
        Parameters(req): Parameters<MarkdownParams>,
    ) -> CallToolResult {
        self.exec(EngineCommand::Markdown {
            max_chars: req.max_chars,
        })
        .await
    }

    #[tool(
        name = "browser_html",
        description = "Get the active page HTML (truncated by default; prefer browser_snapshot for agents)"
    )]
    async fn browser_html(&self, Parameters(req): Parameters<HtmlParams>) -> CallToolResult {
        self.exec(EngineCommand::Html {
            max_chars: req.max_chars,
        })
        .await
    }

    #[tool(name = "browser_title", description = "Get the active tab title")]
    async fn browser_title(&self) -> CallToolResult {
        self.exec(EngineCommand::Title).await
    }

    #[tool(name = "browser_url", description = "Get the active tab URL")]
    async fn browser_url(&self) -> CallToolResult {
        self.exec(EngineCommand::Url).await
    }

    #[tool(
        name = "browser_extract",
        description = "Open a URL (or reuse active tab) and return title/url/text/outline for agents"
    )]
    async fn browser_extract(&self, Parameters(req): Parameters<ExtractParams>) -> CallToolResult {
        self.exec(EngineCommand::Extract {
            url: req.url,
            wait_selector: req.wait_selector,
            timeout_ms: req.timeout_ms,
            pass_cf: req.pass_cf.unwrap_or(false),
            include_html: req.include_html.unwrap_or(false),
            include_ax_json: req.include_ax_json.unwrap_or(false),
            include_markdown: req.include_markdown.unwrap_or(true),
            max_text_chars: req.max_text_chars,
            max_markdown_chars: req.max_markdown_chars,
            screenshot_out: req.screenshot_out,
            full_screenshot: req.full.unwrap_or(false),
        })
        .await
    }

    #[tool(
        name = "browser_close",
        description = "Close a tab, defaulting to the active tab"
    )]
    async fn browser_close(&self, Parameters(req): Parameters<CloseTabParams>) -> CallToolResult {
        self.exec(EngineCommand::Close { tab_id: req.tab_id }).await
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
        description = "Click an element in the active tab. Prefer ref from browser_snapshot (e1 / ref:e1). Pass return_snapshot=true to attach a lightweight page snapshot after success."
    )]
    async fn browser_click(&self, Parameters(req): Parameters<ClickParams>) -> CallToolResult {
        let selector = match (req.ref_id, req.selector) {
            (Some(r), _) => normalize_ref_selector(r),
            (None, Some(s)) => s,
            (None, None) => {
                return result_to_tool(JsonResponse::err(
                    "invalid_args",
                    "browser_click requires selector or ref",
                    None,
                ));
            }
        };
        self.exec_action(
            EngineCommand::Click {
                selector: selector.clone(),
            },
            Some(&selector),
            req.return_snapshot.unwrap_or(false),
        )
        .await
    }

    #[tool(
        name = "browser_type",
        description = "Type text into an element in the active tab. Prefer ref from browser_snapshot (e1 / ref:e1). Pass return_snapshot=true to attach a lightweight page snapshot after success."
    )]
    async fn browser_type(&self, Parameters(req): Parameters<TypeParams>) -> CallToolResult {
        let selector = match (req.ref_id, req.selector) {
            (Some(r), _) => normalize_ref_selector(r),
            (None, Some(s)) => s,
            (None, None) => {
                return result_to_tool(JsonResponse::err(
                    "invalid_args",
                    "browser_type requires selector or ref",
                    None,
                ));
            }
        };
        self.exec_action(
            EngineCommand::Type {
                selector: selector.clone(),
                text: req.text,
            },
            Some(&selector),
            req.return_snapshot.unwrap_or(false),
        )
        .await
    }

    #[tool(
        name = "browser_press",
        description = "Press a key in the active tab, optionally scoped to an element (e.g. Enter, Tab, Escape, ArrowDown). Pass return_snapshot=true to attach a lightweight page snapshot after success."
    )]
    async fn browser_press(&self, Parameters(req): Parameters<PressParams>) -> CallToolResult {
        let selector = req.selector.map(normalize_ref_selector);
        self.exec_action(
            EngineCommand::Press {
                key: req.key,
                selector: selector.clone(),
            },
            selector.as_deref(),
            req.return_snapshot.unwrap_or(false),
        )
        .await
    }

    #[tool(
        name = "browser_wait",
        description = "Wait for an element to be displayed. Pass return_snapshot=true to attach a lightweight page snapshot after success."
    )]
    async fn browser_wait(&self, Parameters(req): Parameters<WaitParams>) -> CallToolResult {
        let selector = normalize_ref_selector(req.selector);
        self.exec_action(
            EngineCommand::Wait {
                selector: selector.clone(),
                timeout_ms: req.timeout_ms,
            },
            Some(&selector),
            req.return_snapshot.unwrap_or(false),
        )
        .await
    }

    #[tool(
        name = "browser_save_state",
        description = "Save login state (cookies + localStorage/sessionStorage) of the active tab to a JSON file"
    )]
    async fn browser_save_state(&self, Parameters(req): Parameters<PathParams>) -> CallToolResult {
        self.exec(EngineCommand::SaveState { path: req.path }).await
    }

    #[tool(
        name = "browser_load_state",
        description = "Load login state from a JSON file into the active tab (navigate to the site first, then reload after)"
    )]
    async fn browser_load_state(&self, Parameters(req): Parameters<PathParams>) -> CallToolResult {
        self.exec(EngineCommand::LoadState { path: req.path }).await
    }

    #[tool(
        name = "browser_ocr",
        description = "Recognize a captcha image at a CSS/XPath selector and return the text (drs must be built with the ocr feature)"
    )]
    async fn browser_ocr(&self, Parameters(req): Parameters<SelectorParams>) -> CallToolResult {
        self.exec(EngineCommand::Ocr {
            selector: req.selector,
        })
        .await
    }

    #[tool(
        name = "browser_solve_slider",
        description = "Solve a slider captcha by preset ('geetest' or 'dingxiang'); returns whether it passed"
    )]
    async fn browser_solve_slider(
        &self,
        Parameters(req): Parameters<SolveSliderParams>,
    ) -> CallToolResult {
        self.exec(EngineCommand::SolveSlider {
            preset: req.preset,
            index: req.index,
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
            methods: req.methods.unwrap_or_default(),
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
        name = "network_block",
        description = "Abort matching requests on the active tab"
    )]
    async fn network_block(
        &self,
        Parameters(req): Parameters<NetworkFilterParams>,
    ) -> CallToolResult {
        self.exec(EngineCommand::NetworkBlock {
            keywords: req.keywords.unwrap_or_default(),
            methods: req.methods.unwrap_or_default(),
        })
        .await
    }

    #[tool(
        name = "network_mock",
        description = "Fulfill matching requests with a fake response"
    )]
    async fn network_mock(&self, Parameters(req): Parameters<NetworkMockParams>) -> CallToolResult {
        self.exec(EngineCommand::NetworkMock {
            keywords: req.keywords.unwrap_or_default(),
            methods: req.methods.unwrap_or_default(),
            status: req.status.unwrap_or(200),
            body: req.body.unwrap_or_default(),
            headers: req.headers.unwrap_or_default(),
        })
        .await
    }

    #[tool(
        name = "network_rewrite",
        description = "Rewrite matching requests then let them continue"
    )]
    async fn network_rewrite(
        &self,
        Parameters(req): Parameters<NetworkRewriteParams>,
    ) -> CallToolResult {
        self.exec(EngineCommand::NetworkRewrite {
            keywords: req.keywords.unwrap_or_default(),
            methods: req.methods.unwrap_or_default(),
            set_headers: req.set_headers.unwrap_or_default(),
            url: req.url,
            method: req.method,
            post_data: req.post_data,
        })
        .await
    }

    #[tool(
        name = "network_record",
        description = "Start HAR recording on the active tab"
    )]
    async fn network_record(
        &self,
        Parameters(req): Parameters<NetworkRecordParams>,
    ) -> CallToolResult {
        self.exec(EngineCommand::NetworkRecord { out: req.out })
            .await
    }

    #[tool(
        name = "network_stop",
        description = "Stop block/mock/rewrite/record and the old listener on the active tab"
    )]
    async fn network_stop(&self) -> CallToolResult {
        self.exec(EngineCommand::NetworkStop).await
    }

    #[tool(
        name = "browser_network",
        description = "Semantic network tool. action=inspect|listen|wait|block|mock|rewrite|record|replay|stop. Prefer this over network_*."
    )]
    async fn browser_network(
        &self,
        Parameters(req): Parameters<BrowserNetworkParams>,
    ) -> CallToolResult {
        let action = req.action.trim().to_ascii_lowercase();
        match action.as_str() {
            "inspect" => {
                self.exec(EngineCommand::NetworkInspect { limit: req.limit })
                    .await
            }
            "listen" => {
                self.exec(EngineCommand::ListenStart {
                    keywords: req.keywords.unwrap_or_default(),
                    xhr_only: req.xhr_only.unwrap_or(false),
                    methods: req.methods.unwrap_or_default(),
                })
                .await
            }
            "wait" => {
                self.exec(EngineCommand::ListenWait {
                    count: req.count.unwrap_or(1),
                    timeout_ms: req.timeout_ms,
                })
                .await
            }
            "block" => {
                self.exec(EngineCommand::NetworkBlock {
                    keywords: req.keywords.unwrap_or_default(),
                    methods: req.methods.unwrap_or_default(),
                })
                .await
            }
            "mock" => {
                self.exec(EngineCommand::NetworkMock {
                    keywords: req.keywords.unwrap_or_default(),
                    methods: req.methods.unwrap_or_default(),
                    status: req.status.unwrap_or(200),
                    body: req.body.unwrap_or_default(),
                    headers: req.headers.unwrap_or_default(),
                })
                .await
            }
            "rewrite" => {
                self.exec(EngineCommand::NetworkRewrite {
                    keywords: req.keywords.unwrap_or_default(),
                    methods: req.methods.unwrap_or_default(),
                    set_headers: req.set_headers.unwrap_or_default(),
                    url: req.url,
                    method: req.method,
                    post_data: req.post_data,
                })
                .await
            }
            "record" => {
                self.exec(EngineCommand::NetworkRecord { out: req.out })
                    .await
            }
            "replay" => {
                let Some(path) = req.har.or(req.path) else {
                    return result_to_tool(JsonResponse::err(
                        "command_failed",
                        "browser_network action=replay 需要 har 或 path",
                        None,
                    ));
                };
                self.exec(EngineCommand::NetworkReplay {
                    path,
                    ignore_query: req.ignore_query.unwrap_or(false),
                    fallback: req.fallback.unwrap_or(false),
                })
                .await
            }
            "stop" => self.exec(EngineCommand::NetworkStop).await,
            other => result_to_tool(JsonResponse::err(
                "command_failed",
                format!(
                    "unknown browser_network action `{other}`; use inspect|listen|wait|block|mock|rewrite|record|replay|stop"
                ),
                None,
            )),
        }
    }

    #[tool(
        name = "browser_console",
        description = "Semantic console tool. action=start|wait|list|stop. start enables Runtime.enable on CDP."
    )]
    async fn browser_console(
        &self,
        Parameters(req): Parameters<BrowserConsoleParams>,
    ) -> CallToolResult {
        match req.action.trim().to_ascii_lowercase().as_str() {
            "start" => {
                self.exec(EngineCommand::ConsoleStart {
                    level: req.level,
                    text: req.text,
                })
                .await
            }
            "wait" => {
                self.exec(EngineCommand::ConsoleWait {
                    timeout_ms: req.timeout_ms,
                })
                .await
            }
            "list" => self.exec(EngineCommand::ConsoleList).await,
            "stop" => self.exec(EngineCommand::ConsoleStop).await,
            other => result_to_tool(JsonResponse::err(
                "command_failed",
                format!("unknown browser_console action `{other}`; use start|wait|list|stop"),
                None,
            )),
        }
    }

    #[tool(
        name = "browser_download",
        description = "Semantic download tracker. action=start|list|wait|stop. Call start before triggering a download."
    )]
    async fn browser_download(
        &self,
        Parameters(req): Parameters<BrowserDownloadParams>,
    ) -> CallToolResult {
        match req.action.trim().to_ascii_lowercase().as_str() {
            "start" => {
                self.exec(EngineCommand::DownloadStart { dir: req.dir })
                    .await
            }
            "list" => self.exec(EngineCommand::DownloadList).await,
            "wait" => {
                self.exec(EngineCommand::DownloadWait {
                    timeout_ms: req.timeout_ms,
                    count: req.count,
                })
                .await
            }
            "stop" => self.exec(EngineCommand::DownloadStop).await,
            other => result_to_tool(JsonResponse::err(
                "command_failed",
                format!("unknown browser_download action `{other}`; use start|list|wait|stop"),
                None,
            )),
        }
    }

    #[tool(
        name = "browser_pdf",
        description = "Print the active page to PDF (headless Chrome / CDP)"
    )]
    async fn browser_pdf(&self, Parameters(req): Parameters<PdfParams>) -> CallToolResult {
        self.exec(EngineCommand::Pdf {
            out: req.out,
            landscape: req.landscape.unwrap_or(false),
            print_background: req.print_background.unwrap_or(true),
        })
        .await
    }

    #[tool(
        name = "browser_inspect",
        description = "Combined page debug dump: snapshot + optional network/console peek. Prefer snapshot for ordinary reading."
    )]
    async fn browser_inspect(&self, Parameters(req): Parameters<InspectParams>) -> CallToolResult {
        self.exec(EngineCommand::Inspect {
            include_network: req.include_network.unwrap_or(true),
            include_console: req.include_console.unwrap_or(true),
            max_packets: req.max_packets,
        })
        .await
    }

    #[tool(
        name = "browser_record",
        description = "Codegen recorder. action=start|stop|status. stop writes runnable Rust."
    )]
    async fn browser_record(
        &self,
        Parameters(req): Parameters<BrowserRecordParams>,
    ) -> CallToolResult {
        match req.action.trim().to_ascii_lowercase().as_str() {
            "start" => self.exec(EngineCommand::RecordStart).await,
            "stop" => self.exec(EngineCommand::RecordStop { out: req.out }).await,
            "status" => self.exec(EngineCommand::RecordStatus).await,
            other => result_to_tool(JsonResponse::err(
                "command_failed",
                format!("unknown browser_record action `{other}`; use start|stop|status"),
                None,
            )),
        }
    }

    #[tool(
        name = "browser_context_list",
        description = "List named CDP contexts in the current browser"
    )]
    async fn browser_context_list(&self) -> CallToolResult {
        self.exec(EngineCommand::ContextList).await
    }

    #[tool(
        name = "browser_context_use",
        description = "Create or reuse a named CDP context; later browser_open can inherit it"
    )]
    async fn browser_context_use(
        &self,
        Parameters(req): Parameters<ContextUseParams>,
    ) -> CallToolResult {
        self.exec(EngineCommand::ContextUse {
            name: req.name,
            proxy: req.proxy,
            user_agent: req.user_agent,
            locale: req.locale,
            timezone: req.timezone,
            viewport: req.viewport,
        })
        .await
    }

    #[tool(
        name = "browser_context_close",
        description = "Dispose a named CDP context and its tabs"
    )]
    async fn browser_context_close(
        &self,
        Parameters(req): Parameters<ContextNameParams>,
    ) -> CallToolResult {
        self.exec(EngineCommand::ContextClose { name: req.name })
            .await
    }

    #[tool(name = "profile_list", description = "List disk profile ids")]
    async fn profile_list(&self, Parameters(req): Parameters<ProfileRootParams>) -> CallToolResult {
        result_to_tool(profile_list_response(req.root))
    }

    #[tool(name = "profile_show", description = "Show one disk profile manifest")]
    async fn profile_show(&self, Parameters(req): Parameters<ProfileShowParams>) -> CallToolResult {
        result_to_tool(profile_show_response(req.id, req.root))
    }

    #[tool(
        name = "browser_identity",
        description = "Diagnose the active tab browser identity and fingerprint consistency"
    )]
    async fn browser_identity(
        &self,
        Parameters(req): Parameters<IdentityGateParams>,
    ) -> CallToolResult {
        self.exec(EngineCommand::Identity {
            pool: false,
            gate: req.into_gate(),
        })
        .await
    }

    #[tool(
        name = "browser_identity_pool",
        description = "Analyze all tabs as an identity pool and report linkability risks"
    )]
    async fn browser_identity_pool(
        &self,
        Parameters(req): Parameters<IdentityGateParams>,
    ) -> CallToolResult {
        self.exec(EngineCommand::Identity {
            pool: true,
            gate: req.into_gate(),
        })
        .await
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

    #[tool(
        name = "identity_assets_validate",
        description = "Validate a profile asset manifest before schedulers or workers use it"
    )]
    async fn identity_assets_validate(
        &self,
        Parameters(req): Parameters<IdentityAssetsValidateParams>,
    ) -> CallToolResult {
        result_to_tool_result(
            identity_cmd::validate_identity_assets(
                &req.asset_manifest,
                req.strict.unwrap_or(false),
                req.validate_out.as_deref(),
            )
            .await,
        )
    }

    #[tool(
        name = "identity_assets_status",
        description = "Report runnable account/profile capacity and block reasons"
    )]
    async fn identity_assets_status(
        &self,
        Parameters(req): Parameters<IdentityAssetsStatusParams>,
    ) -> CallToolResult {
        result_to_tool_result(
            identity_cmd::status_identity_assets(
                &req.asset_manifest,
                req.allow_state.as_deref().unwrap_or(&[]),
                req.desired_concurrency,
                req.include_dispatch_leased.unwrap_or(false),
                req.include_retry.unwrap_or(false),
                req.include_failed.unwrap_or(false),
                req.include_cancelled.unwrap_or(false),
                req.include_runtime_leased.unwrap_or(false),
                req.include_missing_profile_dir.unwrap_or(false),
                req.status_out.as_deref(),
            )
            .await,
        )
    }

    #[tool(
        name = "identity_assets_forecast",
        description = "Forecast when blocked account/profile assets become runnable again"
    )]
    async fn identity_assets_forecast(
        &self,
        Parameters(req): Parameters<IdentityAssetsForecastParams>,
    ) -> CallToolResult {
        result_to_tool_result(
            identity_cmd::forecast_identity_assets(
                &req.asset_manifest,
                req.allow_state.as_deref().unwrap_or(&[]),
                req.desired_concurrency,
                req.horizon_seconds,
                req.include_dispatch_leased.unwrap_or(false),
                req.include_retry.unwrap_or(false),
                req.include_failed.unwrap_or(false),
                req.include_cancelled.unwrap_or(false),
                req.include_runtime_leased.unwrap_or(false),
                req.include_missing_profile_dir.unwrap_or(false),
                req.forecast_out.as_deref(),
            )
            .await,
        )
    }

    #[tool(
        name = "identity_assets_gate",
        description = "Gate business startup on current or soon-recovering account/profile capacity"
    )]
    async fn identity_assets_gate(
        &self,
        Parameters(req): Parameters<IdentityAssetsGateParams>,
    ) -> CallToolResult {
        result_to_tool_result(
            identity_cmd::gate_identity_assets(
                &req.asset_manifest,
                req.desired_concurrency,
                req.max_wait_seconds,
                req.allow_wait.unwrap_or(false),
                req.allow_state.as_deref().unwrap_or(&[]),
                req.include_dispatch_leased.unwrap_or(false),
                req.include_retry.unwrap_or(false),
                req.include_failed.unwrap_or(false),
                req.include_cancelled.unwrap_or(false),
                req.include_runtime_leased.unwrap_or(false),
                req.include_missing_profile_dir.unwrap_or(false),
                req.gate_out.as_deref(),
            )
            .await,
        )
    }

    #[tool(
        name = "identity_assets_select",
        description = "Select runnable profile assets and optionally reserve runtime leases"
    )]
    async fn identity_assets_select(
        &self,
        Parameters(req): Parameters<IdentityAssetsSelectParams>,
    ) -> CallToolResult {
        result_to_tool_result(
            identity_cmd::select_identity_assets(
                &req.asset_manifest,
                req.limit.unwrap_or(10),
                req.allow_state.as_deref().unwrap_or(&[]),
                req.worker.as_deref(),
                req.job.as_deref(),
                req.lease_seconds.unwrap_or(900),
                req.include_dispatch_leased.unwrap_or(false),
                req.include_retry.unwrap_or(false),
                req.include_failed.unwrap_or(false),
                req.include_cancelled.unwrap_or(false),
                req.include_runtime_leased.unwrap_or(false),
                req.include_missing_profile_dir.unwrap_or(false),
                req.asset_manifest_out.as_deref(),
                req.selection_out.as_deref(),
            )
            .await,
        )
    }

    #[tool(
        name = "identity_assets_release",
        description = "Release runtime leases after business automation finishes"
    )]
    async fn identity_assets_release(
        &self,
        Parameters(req): Parameters<IdentityAssetsReleaseParams>,
    ) -> CallToolResult {
        result_to_tool_result(
            identity_cmd::release_identity_assets(
                &req.asset_manifest,
                req.status.as_deref().unwrap_or("succeeded"),
                req.worker.as_deref(),
                req.job.as_deref(),
                req.lease_id.as_deref().unwrap_or(&[]),
                req.account_id.as_deref().unwrap_or(&[]),
                req.profile_id.as_deref().unwrap_or(&[]),
                req.identity_id.as_deref().unwrap_or(&[]),
                req.label.as_deref().unwrap_or(&[]),
                req.cooldown_seconds,
                req.next_state.as_deref(),
                req.message.as_deref(),
                req.result_json.as_deref(),
                req.asset_manifest_out.as_deref(),
                req.release_out.as_deref(),
                req.append_release.unwrap_or(false),
            )
            .await,
        )
    }

    #[tool(
        name = "identity_assets_reconcile_runtime",
        description = "Replay runtime release ledgers into a central profile asset manifest"
    )]
    async fn identity_assets_reconcile_runtime(
        &self,
        Parameters(req): Parameters<IdentityAssetsReconcileRuntimeParams>,
    ) -> CallToolResult {
        result_to_tool_result(
            identity_cmd::reconcile_identity_asset_runtime_manifest(
                &req.asset_manifest,
                &req.release_ledger,
                req.asset_manifest_out.as_deref(),
            )
            .await,
        )
    }

    #[tool(
        name = "identity_assets_health",
        description = "Score runtime health from release ledgers and optionally mark bad assets"
    )]
    async fn identity_assets_health(
        &self,
        Parameters(req): Parameters<IdentityAssetsHealthParams>,
    ) -> CallToolResult {
        let policy = match identity_cmd::load_identity_policy(req.policy.as_deref()).await {
            Ok(policy) => policy,
            Err(error) => {
                return result_to_tool(JsonResponse::err(
                    "command_failed",
                    error.to_string(),
                    None,
                ));
            }
        };
        let resolved = policy.as_ref().map_or_else(
            || identity_cmd::ResolvedHealthPolicy {
                window_seconds: req.window_seconds,
                repair_threshold: req.repair_threshold.unwrap_or(3),
                quarantine_threshold: req.quarantine_threshold.unwrap_or(5),
                cooldown_seconds: req.cooldown_seconds.unwrap_or(900),
            },
            |policy| {
                policy.merge_health(
                    req.window_seconds,
                    req.repair_threshold,
                    req.quarantine_threshold,
                    req.cooldown_seconds,
                )
            },
        );
        let mut response = match identity_cmd::health_identity_assets(
            &req.asset_manifest,
            &req.release_ledger,
            resolved.window_seconds,
            resolved.repair_threshold,
            resolved.quarantine_threshold,
            resolved.cooldown_seconds,
            req.asset_manifest_out.as_deref(),
            req.health_out.as_deref(),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => {
                return result_to_tool(JsonResponse::err(
                    "command_failed",
                    error.to_string(),
                    None,
                ));
            }
        };
        identity_cmd::attach_identity_policy(&mut response, policy.as_ref());
        result_to_tool(response)
    }

    #[tool(
        name = "identity_assets_sweep",
        description = "Sweep expired runtime leases, dispatch leases, and cooldowns"
    )]
    async fn identity_assets_sweep(
        &self,
        Parameters(req): Parameters<IdentityAssetsSweepParams>,
    ) -> CallToolResult {
        result_to_tool_result(
            identity_cmd::sweep_identity_assets(
                &req.asset_manifest,
                req.runtime_grace_seconds.unwrap_or(0),
                req.dispatch_grace_seconds.unwrap_or(0),
                req.cooldown_grace_seconds.unwrap_or(0),
                req.asset_manifest_out.as_deref(),
                req.sweep_out.as_deref(),
            )
            .await,
        )
    }
}

impl DrsMcp {
    async fn exec(&self, command: EngineCommand) -> CallToolResult {
        self.exec_action(command, None, false).await
    }

    /// Run a browser action; optionally attach a lightweight snapshot afterwards.
    async fn exec_action(
        &self,
        command: EngineCommand,
        selector_hint: Option<&str>,
        return_snapshot: bool,
    ) -> CallToolResult {
        let timeout_ms = mcp_command_timeout_ms();
        let response = match tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            self.exec_response(command),
        )
        .await
        {
            Ok(response) => annotate_command_error(response, selector_hint),
            Err(_) => JsonResponse::err(
                "timeout",
                format!("MCP browser command timed out after {timeout_ms}ms"),
                Some(
                    "page may be automation-hostile; try browser_snapshot, or raise DRS_MCP_TIMEOUT_MS"
                        .to_string(),
                ),
            ),
        };

        if !response.ok || !return_snapshot {
            return result_to_tool(response);
        }

        let mut data = response.data.unwrap_or_else(|| json!({}));
        let snap = match tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms.min(15_000)),
            self.exec_response(EngineCommand::Snapshot {
                budget_ms: Some(1_500),
                max_items: Some(80),
                max_text_chars: Some(2_000),
                max_markdown_chars: Some(0),
            }),
        )
        .await
        {
            Ok(snap) => snap,
            Err(_) => JsonResponse::err(
                "timeout",
                "lightweight snapshot after action timed out",
                Some("retry browser_snapshot separately".to_string()),
            ),
        };
        if snap.ok {
            if let Some(page) = snap.data {
                data["page"] = page;
            }
        } else if let Some(error) = snap.error {
            data["pageError"] = json!({
                "code": error.code,
                "message": error.message,
                "hint": error.hint,
            });
        }
        result_to_tool(JsonResponse::ok(data))
    }

    async fn exec_response(&self, command: EngineCommand) -> JsonResponse {
        match &self.backend {
            DrsBackend::Daemon(config) => self.exec_daemon(config, command).await,
            DrsBackend::Local(state) => {
                let mut state = state.lock().await;
                match state.execute(command).await {
                    Ok(result) => JsonResponse::ok(result.data),
                    Err(e) => JsonResponse::err("command_failed", e.to_string(), None),
                }
            }
        }
    }

    /// Send a command to the shared daemon, transparently restarting it if it
    /// has exited since MCP started (so the AI never sees a dead browser).
    async fn exec_daemon(&self, config: &DaemonConfig, command: EngineCommand) -> JsonResponse {
        if !daemon::daemon_is_healthy().await
            && daemon::ensure_daemon(
                config.backend,
                config.headless,
                config.user_data_dir.clone(),
            )
            .await
            .is_err()
        {
            return JsonResponse::err(
                "daemon_unavailable",
                "drs daemon is not running and could not be started",
                Some("run `drs serve` manually to inspect the failure".to_string()),
            );
        }
        match daemon::send_to_daemon(command).await {
            Ok(response) => response,
            Err(e) => JsonResponse::err(
                "daemon_error",
                format!("failed to reach drs daemon: {e}"),
                Some("the daemon may have exited; retry the request".to_string()),
            ),
        }
    }
}

pub async fn run_mcp(
    backend: BackendKind,
    headless: bool,
    user_data_dir: Option<PathBuf>,
    standalone: bool,
    tools: McpToolset,
) -> Result<()> {
    let mcp = if standalone {
        DrsMcp::standalone(backend, headless, user_data_dir, tools).await?
    } else {
        // Bring up (or reuse) the shared persistent daemon before serving, so
        // that MCP browser commands attach to the long-lived daemon browser
        // instead of a per-process one. `ensure_daemon` writes nothing to our
        // stdout, keeping the stdio JSON-RPC channel clean.
        let config = DaemonConfig {
            backend,
            headless,
            user_data_dir,
        };
        daemon::ensure_daemon(
            config.backend,
            config.headless,
            config.user_data_dir.clone(),
        )
        .await?;
        DrsMcp::attached(config, tools)
    };
    let service = mcp.serve(stdio()).await?;
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

fn result_to_tool_result(result: Result<JsonResponse>) -> CallToolResult {
    match result {
        Ok(response) => result_to_tool(response),
        Err(error) => result_to_tool(JsonResponse::err("command_failed", error.to_string(), None)),
    }
}

fn profile_root_or_default(root: Option<PathBuf>) -> Result<PathBuf> {
    Ok(root.unwrap_or(paths::default_profiles_root()?))
}

fn profile_list_response(root: Option<PathBuf>) -> JsonResponse {
    match profile_list_inner(root) {
        Ok(data) => JsonResponse::ok(data),
        Err(e) => JsonResponse::err("command_failed", e.to_string(), None),
    }
}

fn profile_show_response(id: String, root: Option<PathBuf>) -> JsonResponse {
    match profile_show_inner(id, root) {
        Ok(data) => JsonResponse::ok(data),
        Err(e) => JsonResponse::err("command_failed", e.to_string(), None),
    }
}

#[cfg(feature = "cdp")]
fn profile_list_inner(root: Option<PathBuf>) -> Result<Value> {
    let root = profile_root_or_default(root)?;
    let mgr = drission::cdp::ProfileManager::open(&root)?;
    Ok(json!({ "root": root, "profiles": mgr.list()? }))
}

#[cfg(not(feature = "cdp"))]
fn profile_list_inner(_root: Option<PathBuf>) -> Result<Value> {
    Err(anyhow::Error::msg("profile 只支持 CDP 后端"))
}

#[cfg(feature = "cdp")]
fn profile_show_inner(id: String, root: Option<PathBuf>) -> Result<Value> {
    let root = profile_root_or_default(root)?;
    let mgr = drission::cdp::ProfileManager::open(&root)?;
    let profile = mgr.get(&id)?;
    Ok(json!({
        "id": profile.id,
        "dir": profile.dir,
        "chromeDir": profile.chrome_dir,
        "manifest": profile.manifest,
    }))
}

#[cfg(not(feature = "cdp"))]
fn profile_show_inner(_id: String, _root: Option<PathBuf>) -> Result<Value> {
    Err(anyhow::Error::msg("profile 只支持 CDP 后端"))
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct OpenParams {
    url: String,
    context: Option<String>,
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
struct SnapshotParams {
    /// JS time budget in ms (default 2000).
    budget_ms: Option<u64>,
    /// Max outline items (default 250).
    max_items: Option<usize>,
    /// Truncate body text (default 8000).
    max_text_chars: Option<usize>,
    /// Truncate HTML→Markdown (default 50000). Pass 0 to skip markdown.
    max_markdown_chars: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct MarkdownParams {
    /// Truncate markdown to this many Unicode scalars (default 50000).
    max_chars: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct HtmlParams {
    /// Truncate HTML to this many Unicode scalars (default 200000).
    max_chars: Option<usize>,
}

fn mcp_command_timeout_ms() -> u64 {
    std::env::var("DRS_MCP_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60_000)
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct CloseTabParams {
    tab_id: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct ExtractParams {
    url: Option<String>,
    wait_selector: Option<String>,
    timeout_ms: Option<u64>,
    pass_cf: Option<bool>,
    include_html: Option<bool>,
    include_ax_json: Option<bool>,
    /// Include HTML→Markdown (default true). Prefer this over include_html for agents.
    include_markdown: Option<bool>,
    max_text_chars: Option<usize>,
    max_markdown_chars: Option<usize>,
    screenshot_out: Option<PathBuf>,
    full: Option<bool>,
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
struct ClickParams {
    /// CSS/DP selector. Prefer `ref` from browser_snapshot when available.
    selector: Option<String>,
    /// Interactive ref from browser_snapshot (`e1` or `ref:e1`).
    #[serde(rename = "ref")]
    ref_id: Option<String>,
    /// After a successful click, attach a lightweight `page` snapshot (refs + short text; no markdown).
    return_snapshot: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct TypeParams {
    /// CSS/DP selector. Prefer `ref` from browser_snapshot when available.
    selector: Option<String>,
    /// Interactive ref from browser_snapshot (`e1` or `ref:e1`).
    #[serde(rename = "ref")]
    ref_id: Option<String>,
    text: String,
    /// After successful typing, attach a lightweight `page` snapshot (refs + short text; no markdown).
    return_snapshot: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct PressParams {
    /// Key name to press, e.g. `Enter`, `Tab`, `Escape`, `ArrowDown`, `a`.
    key: String,
    /// Optional element selector to focus before pressing; omit to press on the active element.
    selector: Option<String>,
    /// After a successful key press, attach a lightweight `page` snapshot (refs + short text; no markdown).
    return_snapshot: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct PathParams {
    /// File path for the login-state JSON.
    path: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct SolveSliderParams {
    /// Slider preset: `geetest` (default) or `dingxiang`.
    preset: String,
    /// Instance index for the `dingxiang` preset (ignored otherwise).
    index: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct WaitParams {
    selector: String,
    timeout_ms: Option<u64>,
    /// After the element appears, attach a lightweight `page` snapshot (refs + short text; no markdown).
    return_snapshot: Option<bool>,
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
    methods: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct NetworkFilterParams {
    keywords: Option<Vec<String>>,
    methods: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct NetworkMockParams {
    keywords: Option<Vec<String>>,
    methods: Option<Vec<String>>,
    status: Option<u16>,
    body: Option<String>,
    headers: Option<Vec<(String, String)>>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct NetworkRewriteParams {
    keywords: Option<Vec<String>>,
    methods: Option<Vec<String>>,
    set_headers: Option<Vec<(String, String)>>,
    url: Option<String>,
    method: Option<String>,
    post_data: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct NetworkRecordParams {
    out: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct BrowserNetworkParams {
    /// inspect | listen | wait | block | mock | rewrite | record | replay | stop
    action: String,
    keywords: Option<Vec<String>>,
    methods: Option<Vec<String>>,
    xhr_only: Option<bool>,
    count: Option<usize>,
    timeout_ms: Option<u64>,
    status: Option<u16>,
    body: Option<String>,
    headers: Option<Vec<(String, String)>>,
    set_headers: Option<Vec<(String, String)>>,
    url: Option<String>,
    method: Option<String>,
    post_data: Option<String>,
    out: Option<String>,
    har: Option<String>,
    path: Option<String>,
    ignore_query: Option<bool>,
    fallback: Option<bool>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct BrowserConsoleParams {
    /// start | wait | list | stop
    action: String,
    level: Option<String>,
    text: Option<String>,
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct BrowserDownloadParams {
    /// start | list | wait | stop
    action: String,
    dir: Option<String>,
    timeout_ms: Option<u64>,
    count: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct PdfParams {
    out: Option<PathBuf>,
    landscape: Option<bool>,
    print_background: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct InspectParams {
    include_network: Option<bool>,
    include_console: Option<bool>,
    max_packets: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct BrowserRecordParams {
    /// start | stop | status
    action: String,
    out: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct ContextUseParams {
    name: String,
    proxy: Option<String>,
    user_agent: Option<String>,
    locale: Option<String>,
    timezone: Option<String>,
    viewport: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct ContextNameParams {
    name: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct ProfileRootParams {
    root: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct ProfileShowParams {
    id: String,
    root: Option<PathBuf>,
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

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct IdentityGateParams {
    gate_preset: Option<IdentityGatePreset>,
    min_score: Option<u8>,
    max_linkability: Option<u8>,
    max_concentration_ratio: Option<f64>,
    max_concentrated_signals: Option<usize>,
    min_entropy_score: Option<u8>,
    min_effective_identities: Option<f64>,
    max_nominal_to_effective_ratio: Option<f64>,
    fail_on_high_risk: Option<bool>,
    fail_on_risky_pairs: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct IdentityAssetsValidateParams {
    asset_manifest: PathBuf,
    strict: Option<bool>,
    validate_out: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct IdentityAssetsStatusParams {
    asset_manifest: PathBuf,
    allow_state: Option<Vec<String>>,
    desired_concurrency: Option<usize>,
    include_dispatch_leased: Option<bool>,
    include_retry: Option<bool>,
    include_failed: Option<bool>,
    include_cancelled: Option<bool>,
    include_runtime_leased: Option<bool>,
    include_missing_profile_dir: Option<bool>,
    status_out: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct IdentityAssetsForecastParams {
    asset_manifest: PathBuf,
    allow_state: Option<Vec<String>>,
    desired_concurrency: Option<usize>,
    horizon_seconds: Option<u64>,
    include_dispatch_leased: Option<bool>,
    include_retry: Option<bool>,
    include_failed: Option<bool>,
    include_cancelled: Option<bool>,
    include_runtime_leased: Option<bool>,
    include_missing_profile_dir: Option<bool>,
    forecast_out: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct IdentityAssetsGateParams {
    asset_manifest: PathBuf,
    desired_concurrency: usize,
    max_wait_seconds: Option<u64>,
    allow_wait: Option<bool>,
    allow_state: Option<Vec<String>>,
    include_dispatch_leased: Option<bool>,
    include_retry: Option<bool>,
    include_failed: Option<bool>,
    include_cancelled: Option<bool>,
    include_runtime_leased: Option<bool>,
    include_missing_profile_dir: Option<bool>,
    gate_out: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct IdentityAssetsSelectParams {
    asset_manifest: PathBuf,
    limit: Option<usize>,
    allow_state: Option<Vec<String>>,
    worker: Option<String>,
    job: Option<String>,
    lease_seconds: Option<u64>,
    include_dispatch_leased: Option<bool>,
    include_retry: Option<bool>,
    include_failed: Option<bool>,
    include_cancelled: Option<bool>,
    include_runtime_leased: Option<bool>,
    include_missing_profile_dir: Option<bool>,
    asset_manifest_out: Option<PathBuf>,
    selection_out: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct IdentityAssetsReleaseParams {
    asset_manifest: PathBuf,
    status: Option<String>,
    worker: Option<String>,
    job: Option<String>,
    lease_id: Option<Vec<String>>,
    account_id: Option<Vec<String>>,
    profile_id: Option<Vec<String>>,
    identity_id: Option<Vec<String>>,
    label: Option<Vec<String>>,
    cooldown_seconds: Option<u64>,
    next_state: Option<String>,
    message: Option<String>,
    /// JSON object/array as a string (Claude Code rejects schemars `Value` → schema `true`).
    result_json: Option<String>,
    asset_manifest_out: Option<PathBuf>,
    release_out: Option<PathBuf>,
    append_release: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct IdentityAssetsReconcileRuntimeParams {
    asset_manifest: PathBuf,
    release_ledger: Vec<PathBuf>,
    asset_manifest_out: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct IdentityAssetsHealthParams {
    asset_manifest: PathBuf,
    policy: Option<PathBuf>,
    release_ledger: Vec<PathBuf>,
    window_seconds: Option<u64>,
    repair_threshold: Option<usize>,
    quarantine_threshold: Option<usize>,
    cooldown_seconds: Option<u64>,
    asset_manifest_out: Option<PathBuf>,
    health_out: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct IdentityAssetsSweepParams {
    asset_manifest: PathBuf,
    runtime_grace_seconds: Option<u64>,
    dispatch_grace_seconds: Option<u64>,
    cooldown_grace_seconds: Option<u64>,
    asset_manifest_out: Option<PathBuf>,
    sweep_out: Option<PathBuf>,
}

impl IdentityGateParams {
    fn into_gate(self) -> IdentityGate {
        IdentityGate {
            preset: self.gate_preset,
            min_score: self.min_score,
            max_linkability: self.max_linkability,
            max_concentration_ratio: self.max_concentration_ratio,
            max_concentrated_signals: self.max_concentrated_signals,
            min_entropy_score: self.min_entropy_score,
            min_effective_identity_count: self.min_effective_identities,
            max_nominal_to_effective_ratio: self.max_nominal_to_effective_ratio,
            fail_on_high_risk: self.fail_on_high_risk.unwrap_or(false),
            fail_on_risky_pairs: self.fail_on_risky_pairs.unwrap_or(false),
        }
    }
}

fn normalize_ref_selector(selector: String) -> String {
    if selector.starts_with("ref:") || !is_bare_ref_id(&selector) {
        selector
    } else {
        format!("ref:{selector}")
    }
}

fn is_bare_ref_id(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some('e'))
        && chars.next().is_some_and(|c| c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_digit())
}

fn allowed_tools(toolset: McpToolset) -> HashSet<&'static str> {
    let mut set = HashSet::new();
    match toolset {
        McpToolset::Core => set.extend(CORE_TOOLS.iter().copied()),
        McpToolset::Browser => {
            set.extend(CORE_TOOLS.iter().copied());
            set.extend(BROWSER_EXTRA_TOOLS.iter().copied());
        }
        McpToolset::Identity => set.extend(IDENTITY_TOOLS.iter().copied()),
        McpToolset::All => set.extend(ALL_MCP_TOOLS.iter().copied()),
    }
    set
}

fn looks_like_stale_ref(message: &str, selector_hint: Option<&str>) -> bool {
    let hint = selector_hint.unwrap_or("");
    let has_ref_marker = message.contains("data-drs-ref")
        || message.contains("ref:")
        || hint.starts_with("ref:")
        || is_bare_ref_id(hint);
    if !has_ref_marker {
        return false;
    }
    let lower = message.to_ascii_lowercase();
    message.contains("未找到元素")
        || message.contains("已失效")
        || lower.contains("element not found")
        || lower.contains("stale element")
        || lower.contains("no node")
        || lower.contains("could not find")
}

fn annotate_command_error(response: JsonResponse, selector_hint: Option<&str>) -> JsonResponse {
    if response.ok {
        return response;
    }
    let Some(error) = response.error else {
        return response;
    };
    if looks_like_stale_ref(&error.message, selector_hint) {
        return JsonResponse::err(
            "stale_ref",
            error.message,
            Some(
                "stale_ref → call browser_snapshot again and use a fresh ref (page changed or prior ref expired)"
                    .to_string(),
            ),
        );
    }
    JsonResponse::err(error.code, error.message, error.hint)
}

fn mcp_instructions(toolset: McpToolset) -> String {
    let profile = match toolset {
        McpToolset::Core => "core",
        McpToolset::Browser => "browser",
        McpToolset::Identity => "identity",
        McpToolset::All => "all",
    };
    format!(
        r#"drs persistent browser MCP (toolset={profile}).
Cursor agent server id is usually `user-drs` (mcp.json key `drs`).

Preferred loop: browser_snapshot → browser_click/type/press/wait with ref → optional return_snapshot=true.
Do NOT use browser_html/browser_ax on heavy pages; prefer browser_snapshot / browser_extract.
Capture XHR with browser_network action=listen then wait/inspect — already in core, do not require DRS_MCP_TOOLS=browser.
console start opens Runtime.enable — skip on Cloudflare/anti-bot pages. Start browser_download before clicking download.

Stable core schemas (session-cacheable; after one GetMcpTools for this server, reuse these args):
- browser_open {{url}}
- browser_snapshot {{budget_ms?, max_items?, max_text_chars?, max_markdown_chars?}}
- browser_extract {{url?, wait_selector?, pass_cf?, include_markdown?}}
- browser_click {{ref?|selector?, return_snapshot?}}
- browser_type {{ref?|selector?, text, return_snapshot?}}
- browser_press {{key, selector?, return_snapshot?}}
- browser_wait {{selector, timeout_ms?, return_snapshot?}}
- browser_screenshot {{out?, full?, inline?}}
- browser_tabs / browser_use_tab {{tab_id}} / browser_close {{tab_id?}}
- browser_network {{action: inspect|listen|wait|block|mock|rewrite|record|replay|stop}}
- browser_console {{action: start|wait|list|stop}}
- browser_download {{action: start|list|wait|stop}}

Errors: code `stale_ref` means the snapshot ref is gone — call browser_snapshot and retry with a new ref.
Need identity_assets_* or PDF/inspect/record? Restart with DRS_MCP_TOOLS=browser|identity|all (default core)."#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_stable() {
        assert_eq!(ALL_MCP_TOOLS.len(), 53);
        assert!(ALL_MCP_TOOLS.contains(&"browser_snapshot"));
        assert!(ALL_MCP_TOOLS.contains(&"browser_network"));
        assert!(ALL_MCP_TOOLS.contains(&"browser_console"));
        assert!(ALL_MCP_TOOLS.contains(&"browser_download"));
        assert!(ALL_MCP_TOOLS.contains(&"identity_assets_sweep"));
    }

    #[test]
    fn core_toolset_is_small_and_excludes_identity_assets() {
        let core = allowed_tools(McpToolset::Core);
        assert_eq!(core.len(), CORE_TOOLS.len());
        assert!(core.contains("browser_snapshot"));
        assert!(core.contains("browser_click"));
        assert!(core.contains("browser_network"));
        assert!(core.contains("browser_console"));
        assert!(core.contains("browser_download"));
        assert!(!core.contains("identity_assets_gate"));
        assert!(!core.contains("network_listen_start"));
        assert!(!core.contains("browser_html"));
        assert!(!core.contains("browser_pdf"));
    }

    #[test]
    fn browser_toolset_keeps_network_but_not_identity_assets() {
        let browser = allowed_tools(McpToolset::Browser);
        assert!(browser.contains("network_listen_wait"));
        assert!(browser.contains("browser_pass_cf"));
        assert!(!browser.contains("identity_assets_select"));
    }

    #[test]
    fn stale_ref_detection_uses_resolved_selector_markers() {
        assert!(looks_like_stale_ref(
            "未找到元素: css:[data-drs-ref=\"e3\"]",
            Some("ref:e3")
        ));
        assert!(looks_like_stale_ref(
            "Element not found: css:[data-drs-ref=\"e1\"]",
            None
        ));
        assert!(!looks_like_stale_ref(
            "未找到元素: css:#submit",
            Some("css:#submit")
        ));
    }

    #[test]
    fn annotate_promotes_stale_ref_code() {
        let response = annotate_command_error(
            JsonResponse::err(
                "command_failed",
                "未找到元素: css:[data-drs-ref=\"e9\"]",
                None,
            ),
            Some("ref:e9"),
        );
        let err = response.error.expect("error");
        assert_eq!(err.code, "stale_ref");
        assert!(err.hint.unwrap().contains("browser_snapshot"));
    }

    #[test]
    fn normalize_ref_accepts_bare_ids() {
        assert_eq!(normalize_ref_selector("e12".into()), "ref:e12");
        assert_eq!(normalize_ref_selector("ref:e12".into()), "ref:e12");
        assert_eq!(normalize_ref_selector("css:#x".into()), "css:#x");
    }
}
