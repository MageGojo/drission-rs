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

use crate::daemon;
use crate::engine::BrowserState;
use crate::identity_cmd;
use crate::protocol::{
    AxFormat, BackendKind, EngineCommand, IdentityGate, IdentityGatePreset, JsonResponse,
};

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
    tool_router: ToolRouter<Self>,
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for DrsMcp {}

#[tool_router(router = tool_router)]
impl DrsMcp {
    /// Attach to the shared persistent daemon browser (default MCP mode).
    fn attached(config: DaemonConfig) -> Self {
        Self {
            backend: DrsBackend::Daemon(config),
            tool_router: Self::tool_router(),
        }
    }

    /// Launch a browser inside this process (`--standalone`).
    async fn standalone(
        backend: BackendKind,
        headless: bool,
        user_data_dir: Option<PathBuf>,
    ) -> Result<Self> {
        Ok(Self {
            backend: DrsBackend::Local(Arc::new(Mutex::new(
                BrowserState::launch(backend, headless, user_data_dir).await?,
            ))),
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
            max_text_chars: req.max_text_chars,
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
        let result_json = req.result_json.as_ref().map(Value::to_string);
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
                result_json.as_deref(),
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
        result_to_tool(self.exec_response(command).await)
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
) -> Result<()> {
    let mcp = if standalone {
        DrsMcp::standalone(backend, headless, user_data_dir).await?
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
        DrsMcp::attached(config)
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
    max_text_chars: Option<usize>,
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
    result_json: Option<Value>,
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

#[cfg(test)]
mod tests {
    #[test]
    fn tool_names_are_stable() {
        const MCP_TOOL_NAMES: &[&str] = &[
            "browser_open",
            "browser_tabs",
            "browser_use_tab",
            "browser_close",
            "browser_ax",
            "browser_html",
            "browser_title",
            "browser_url",
            "browser_extract",
            "browser_text",
            "browser_eval",
            "browser_click",
            "browser_type",
            "browser_wait",
            "browser_screenshot",
            "network_listen_start",
            "network_listen_wait",
            "network_listen_stop",
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
        assert_eq!(
            MCP_TOOL_NAMES,
            &[
                "browser_open",
                "browser_tabs",
                "browser_use_tab",
                "browser_close",
                "browser_ax",
                "browser_html",
                "browser_title",
                "browser_url",
                "browser_extract",
                "browser_text",
                "browser_eval",
                "browser_click",
                "browser_type",
                "browser_wait",
                "browser_screenshot",
                "network_listen_start",
                "network_listen_wait",
                "network_listen_stop",
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
            ]
        );
    }
}
