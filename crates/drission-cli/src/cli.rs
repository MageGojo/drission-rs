use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::protocol::{
    BackendKind, EngineCommand, IdentityDriftMatchMode, IdentityGate, IdentityGatePreset,
    IdentityLifecycleBaselinePolicy,
};
use crate::setup::{SetupScope, SetupTarget};

#[derive(Debug, Parser)]
#[command(name = "drs", version, about = "drission CLI and MCP runtime")]
pub struct Cli {
    /// Emit stable machine-readable JSON.
    #[arg(long)]
    pub json: bool,

    #[command(flatten)]
    pub ensure: EnsureServeArgs,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Args, Clone, Default)]
pub struct EnsureServeArgs {
    /// Auto-start `drs serve` before daemon-backed commands when no healthy daemon exists.
    #[arg(long, global = true)]
    pub ensure_serve: bool,
    /// Backend used by `--ensure-serve`.
    #[arg(long = "ensure-backend", value_enum, default_value_t = BackendKind::Cdp, global = true)]
    pub ensure_backend: BackendKind,
    /// Run browser headless when `--ensure-serve` starts the daemon.
    #[arg(long = "ensure-headless", global = true)]
    pub ensure_headless: bool,
    /// Persistent profile directory when `--ensure-serve` starts the daemon.
    #[arg(long = "ensure-user-data-dir", global = true)]
    pub ensure_user_data_dir: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the local browser daemon.
    Serve(RunBrowserArgs),
    /// Start the daemon in the background if it is not already healthy.
    #[command(name = "ensure-serve")]
    EnsureServe(RunBrowserArgs),
    /// Start the stdio MCP server (attaches to the persistent daemon browser).
    Mcp(McpArgs),
    /// Auto-configure the `drs` MCP server for Cursor and/or Codex.
    Setup(SetupArgs),
    /// Show daemon status.
    Status,
    /// Stop the daemon and browser.
    Stop,
    /// Open a URL in a new tab and make it active.
    Open { url: String },
    /// List daemon tabs.
    Tabs,
    /// Switch active tab by drs tab id.
    Use { tab_id: u64 },
    /// Close a tab, defaulting to the active tab.
    Close { tab_id: Option<u64> },
    /// Print an accessibility snapshot.
    Ax {
        /// Print outline text.
        #[arg(long, conflicts_with = "tree_json")]
        outline: bool,
        /// Return the full accessibility tree JSON.
        #[arg(long = "json", conflicts_with = "outline")]
        tree_json: bool,
    },
    /// Print current page HTML.
    Html,
    /// Print the active tab title.
    Title,
    /// Print the active tab URL.
    Url,
    /// Open a URL and return a page content bundle for agents.
    Extract {
        /// URL to open. Omit to extract the active tab.
        url: Option<String>,
        /// Wait for this selector before extracting.
        #[arg(long)]
        wait_selector: Option<String>,
        #[arg(long)]
        timeout_ms: Option<u64>,
        /// Try to pass Cloudflare before extracting.
        #[arg(long)]
        pass_cf: bool,
        /// Include raw HTML in the bundle.
        #[arg(long)]
        include_html: bool,
        /// Include full accessibility tree JSON.
        #[arg(long = "include-ax-json")]
        include_ax_json: bool,
        /// Truncate text/html to this many Unicode scalars.
        #[arg(long)]
        max_text_chars: Option<usize>,
        /// Save a screenshot while extracting.
        #[arg(long)]
        screenshot_out: Option<PathBuf>,
        #[arg(long)]
        full: bool,
        /// Write the JSON bundle to this file (always machine-readable JSON).
        #[arg(long)]
        save_out: Option<PathBuf>,
    },
    /// Print page text, or text of a selector.
    Text { selector: Option<String> },
    /// Evaluate JavaScript in the active tab.
    Eval { js: String },
    /// Save a screenshot.
    Screenshot {
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        inline: bool,
    },
    /// Click an element.
    Click { selector: String },
    /// Type text into an element.
    #[command(name = "type")]
    Type { selector: String, text: String },
    /// Press a key, optionally scoped to an element.
    Press {
        key: String,
        #[arg(long)]
        selector: Option<String>,
    },
    /// Wait for an element to be displayed.
    Wait {
        selector: String,
        #[arg(long)]
        timeout_ms: Option<u64>,
    },
    /// Network listener commands.
    Listen {
        #[command(subcommand)]
        command: ListenCommand,
    },
    /// Pass Cloudflare challenge in the active tab.
    PassCf {
        #[arg(long)]
        timeout_ms: Option<u64>,
    },
    /// Diagnose active browser identity or all tabs as an identity pool.
    Identity {
        /// Analyze all daemon tabs as an identity pool.
        #[arg(long)]
        pool: bool,
        /// Write the collected fingerprint snapshots to a JSON file.
        #[arg(long)]
        snapshots_out: Option<PathBuf>,
        /// Append snapshots as NDJSON instead of overwriting a JSON array.
        #[arg(long, requires = "snapshots_out")]
        append_snapshots: bool,
        #[command(flatten)]
        gate: IdentityGateArgs,
    },
    /// Analyze saved fingerprint snapshots without starting a browser daemon.
    #[command(name = "identity-pool")]
    IdentityPool {
        /// JSON file containing snapshots, {snapshots:[...]}, or drs identity output.
        snapshots: PathBuf,
        /// JSON governance policy used as defaults for identity gates.
        #[arg(long)]
        policy: Option<PathBuf>,
        /// Compare candidates against an existing baseline pool.
        #[arg(long)]
        against: Option<PathBuf>,
        /// Write accepted candidate snapshots to a JSON file.
        #[arg(long)]
        accept_out: Option<PathBuf>,
        /// Write quarantined candidate snapshots to a JSON file.
        #[arg(long)]
        quarantine_out: Option<PathBuf>,
        /// Write updated baseline = existing baseline + accepted candidates.
        #[arg(long)]
        baseline_out: Option<PathBuf>,
        /// Write candidate-level admission ledger to a JSON file.
        #[arg(long)]
        ledger_out: Option<PathBuf>,
        /// Write flattened pool admission/remediation actions to a JSON file.
        #[arg(long)]
        actions_out: Option<PathBuf>,
        /// Append split outputs as NDJSON instead of overwriting JSON arrays.
        #[arg(long)]
        append_split: bool,
        /// Append ledger entries as NDJSON instead of overwriting a JSON report.
        #[arg(long, requires = "ledger_out")]
        append_ledger: bool,
        /// Append pool actions as NDJSON instead of overwriting a JSON report.
        #[arg(long, requires = "actions_out")]
        append_actions: bool,
        #[command(flatten)]
        gate: IdentityGateArgs,
    },
    /// Compare two aligned snapshot files and report same-profile fingerprint drift.
    #[command(name = "identity-drift")]
    IdentityDrift {
        /// Previous baseline snapshots.
        before: PathBuf,
        /// Current snapshots sampled from the same account/profile order.
        after: PathBuf,
        /// Maximum acceptable drift score for every aligned snapshot pair.
        #[arg(long)]
        max_drift_score: Option<u8>,
        /// Fail the gate if any aligned pair has high-risk drift.
        #[arg(long)]
        fail_on_high_risk_drift: bool,
        /// Match mode: auto uses labels when present, otherwise index order.
        #[arg(long = "match-by", value_enum)]
        match_by: Option<IdentityDriftMatchMode>,
        /// JSON governance policy used as defaults for drift gates.
        #[arg(long)]
        policy: Option<PathBuf>,
        /// Write flattened drift remediation actions to a JSON file.
        #[arg(long)]
        actions_out: Option<PathBuf>,
        /// Append drift remediation actions as NDJSON instead of overwriting a JSON report.
        #[arg(long, requires = "actions_out")]
        append_actions: bool,
    },
    /// Classify profiles across baseline/current snapshots into lifecycle states.
    #[command(name = "identity-lifecycle")]
    IdentityLifecycle {
        /// Previous accepted baseline snapshots.
        baseline: PathBuf,
        /// Current snapshots sampled from running profiles.
        current: PathBuf,
        /// Maximum acceptable drift score before a matched profile is quarantined.
        #[arg(long)]
        max_drift_score: Option<u8>,
        /// Fail the gate if any matched profile has high-risk drift.
        #[arg(long)]
        fail_on_high_risk_drift: bool,
        /// Fail the gate if any baseline profile is missing from the current sample.
        #[arg(long)]
        fail_on_missing_current: bool,
        /// Fail the gate if any current profile is absent from the baseline.
        #[arg(long)]
        fail_on_new_current: bool,
        /// Match mode: auto uses labels when present, otherwise index order.
        #[arg(long = "match-by", value_enum)]
        match_by: Option<IdentityDriftMatchMode>,
        /// JSON governance policy used as defaults for lifecycle gates.
        #[arg(long)]
        policy: Option<PathBuf>,
        /// Write lifecycle state ledger to a JSON file.
        #[arg(long)]
        ledger_out: Option<PathBuf>,
        /// Write lifecycle baseline/current delta report to a JSON file.
        #[arg(long)]
        delta_out: Option<PathBuf>,
        /// Write a compact lifecycle audit run record to a JSON file.
        #[arg(long)]
        journal_out: Option<PathBuf>,
        /// Write per-state lifecycle snapshot files into this directory.
        #[arg(long)]
        state_out_dir: Option<PathBuf>,
        /// Append lifecycle ledger entries as NDJSON instead of overwriting a JSON report.
        #[arg(long, requires = "ledger_out")]
        append_ledger: bool,
        /// Append lifecycle audit run record as NDJSON instead of overwriting JSON.
        #[arg(long, requires = "journal_out")]
        append_journal: bool,
        /// Write the next readable baseline after applying the lifecycle policy.
        #[arg(long)]
        next_baseline_out: Option<PathBuf>,
        /// Policy used for --next-baseline-out.
        #[arg(long = "next-baseline-policy", value_enum)]
        next_baseline_policy: Option<IdentityLifecycleBaselinePolicy>,
        /// Write flattened lifecycle actions to a JSON file.
        #[arg(long)]
        actions_out: Option<PathBuf>,
        /// Append lifecycle actions as NDJSON instead of overwriting a JSON report.
        #[arg(long, requires = "actions_out")]
        append_actions: bool,
    },
    /// Apply or dry-run identity action queues against local profile assets.
    #[command(name = "identity-apply")]
    IdentityApply {
        /// JSON/NDJSON action queue file from identity-pool, identity-drift, or identity-lifecycle.
        actions: PathBuf,
        /// Root directory containing profile directories named by label or identity id.
        #[arg(long)]
        profile_root: Option<PathBuf>,
        /// Optional JSON map from label/identity id to profile directory.
        #[arg(long)]
        profile_map: Option<PathBuf>,
        /// Directory that receives quarantined profiles. Defaults to PROFILE_ROOT/_quarantine.
        #[arg(long)]
        quarantine_dir: Option<PathBuf>,
        /// Actually move profile directories. Without this flag, only a dry-run plan is emitted.
        #[arg(long)]
        execute: bool,
        /// Write the apply journal to a JSON file.
        #[arg(long)]
        journal_out: Option<PathBuf>,
        /// Append apply operations as NDJSON instead of overwriting a JSON report.
        #[arg(long, requires = "journal_out")]
        append_journal: bool,
        /// Write profile asset state patches inferred from the action queue.
        #[arg(long)]
        asset_state_out: Option<PathBuf>,
        /// Append profile asset state patches as NDJSON instead of overwriting JSON.
        #[arg(long, requires = "asset_state_out")]
        append_asset_state: bool,
    },
    /// Build a unified identity governance plan from reports, actions, and asset patches.
    #[command(name = "identity-plan")]
    IdentityPlan {
        /// JSON/NDJSON files from identity-pool, identity-drift, identity-lifecycle, or identity-apply.
        #[arg(required = true, num_args = 1..)]
        inputs: Vec<PathBuf>,
        /// Report title used in JSON and HTML output.
        #[arg(long)]
        title: Option<String>,
        /// Write the unified plan JSON report to this file.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Write a standalone HTML audit report to this file.
        #[arg(long)]
        html_out: Option<PathBuf>,
        /// Profile asset manifest to patch with assetPatches.
        #[arg(long)]
        asset_manifest: Option<PathBuf>,
        /// Write the patched profile asset manifest to this file.
        #[arg(long, requires = "asset_manifest")]
        asset_manifest_out: Option<PathBuf>,
        /// Write scheduler-ready dispatch work items to this file.
        #[arg(long)]
        dispatch_out: Option<PathBuf>,
        /// Append dispatch work items as NDJSON instead of overwriting JSON.
        #[arg(long, requires = "dispatch_out")]
        append_dispatch: bool,
    },
    /// Claim scheduler-ready identity dispatch work items with leases.
    #[command(name = "identity-dispatch")]
    IdentityDispatch {
        /// JSON/NDJSON dispatch queue from identity-plan --dispatch-out or full plan output.
        dispatch: PathBuf,
        /// Worker id recorded on claimed leases.
        #[arg(long)]
        worker: Option<String>,
        /// Maximum number of dispatch items to claim.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Lease duration in seconds.
        #[arg(long, default_value_t = 900)]
        lease_seconds: u64,
        /// Claim items marked blockedByGate as well.
        #[arg(long)]
        include_blocked: bool,
        /// Existing claim ledger used to skip still-active leases.
        #[arg(long)]
        claim_ledger: Option<PathBuf>,
        /// Claim items even when their dedupeKey has an active lease in --claim-ledger.
        #[arg(long)]
        include_leased: bool,
        /// Existing completion ledger used to skip terminal completed work.
        #[arg(long)]
        completion_ledger: Option<PathBuf>,
        /// Claim items even when their dedupeKey has a terminal completion in --completion-ledger.
        #[arg(long)]
        include_completed: bool,
        /// Write claimed lease records to this file.
        #[arg(long)]
        claim_out: Option<PathBuf>,
        /// Append claimed lease records as NDJSON instead of overwriting JSON.
        #[arg(long, requires = "claim_out")]
        append_claim: bool,
    },
    /// Renew leases for already claimed identity dispatch work.
    #[command(name = "identity-dispatch-renew")]
    IdentityDispatchRenew {
        /// JSON/NDJSON claim report or claim items from identity-dispatch --claim-out.
        claims: PathBuf,
        /// Only renew claims leased by this worker.
        #[arg(long)]
        worker: Option<String>,
        /// Only renew items from this claim id.
        #[arg(long)]
        claim_id: Option<String>,
        /// Only renew these dedupe keys; may be repeated.
        #[arg(long)]
        dedupe_key: Vec<String>,
        /// New lease duration in seconds from now.
        #[arg(long, default_value_t = 900)]
        lease_seconds: u64,
        /// Renew expired claim items as well.
        #[arg(long)]
        include_expired: bool,
        /// Write renewed lease records to this file.
        #[arg(long)]
        claim_out: Option<PathBuf>,
        /// Append renewed lease records as NDJSON instead of overwriting JSON.
        #[arg(long, requires = "claim_out")]
        append_claim: bool,
    },
    /// Reconcile dispatch ledgers back into a profile asset manifest.
    #[command(name = "identity-dispatch-reconcile")]
    IdentityDispatchReconcile {
        /// Profile asset manifest to update with dispatch state.
        asset_manifest: PathBuf,
        /// Claim ledger used to mark active leases.
        #[arg(long)]
        claim_ledger: Option<PathBuf>,
        /// Completion ledger used to mark succeeded, failed, retry, or cancelled work.
        #[arg(long)]
        completion_ledger: Option<PathBuf>,
        /// Write the reconciled profile asset manifest to this file.
        #[arg(long)]
        asset_manifest_out: Option<PathBuf>,
    },
    /// Validate profile asset manifest structure, states, duplicate keys, and runtime fields.
    #[command(name = "identity-assets-validate")]
    IdentityAssetsValidate {
        /// Profile asset manifest to validate.
        asset_manifest: PathBuf,
        /// Exit with code 2 when validation has errors.
        #[arg(long)]
        strict: bool,
        /// Write the validation report to this file.
        #[arg(long)]
        validate_out: Option<PathBuf>,
    },
    /// Reconcile runtime release ledgers back into a profile asset manifest.
    #[command(name = "identity-assets-reconcile-runtime")]
    IdentityAssetsReconcileRuntime {
        /// Profile asset manifest to update with runtime release results.
        asset_manifest: PathBuf,
        /// Runtime release ledger from identity-assets-release --append-release; repeatable.
        #[arg(long = "release-ledger")]
        release_ledger: Vec<PathBuf>,
        /// Write the reconciled profile asset manifest to this file.
        #[arg(long)]
        asset_manifest_out: Option<PathBuf>,
    },
    /// Score profile asset health from runtime release ledgers and optionally quarantine bad assets.
    #[command(name = "identity-assets-health")]
    IdentityAssetsHealth {
        /// Profile asset manifest to score.
        asset_manifest: PathBuf,
        /// JSON governance policy used as defaults for runtime health thresholds.
        #[arg(long)]
        policy: Option<PathBuf>,
        /// Runtime release ledger from identity-assets-release; repeatable.
        #[arg(long = "release-ledger")]
        release_ledger: Vec<PathBuf>,
        /// Only include release events newer than now - window seconds.
        #[arg(long)]
        window_seconds: Option<u64>,
        /// Consecutive unsuccessful runtime events before marking repair.
        #[arg(long)]
        repair_threshold: Option<usize>,
        /// Consecutive unsuccessful runtime events before marking quarantine.
        #[arg(long)]
        quarantine_threshold: Option<usize>,
        /// Cooldown seconds to apply when repair/quarantine action is written.
        #[arg(long)]
        cooldown_seconds: Option<u64>,
        /// Write the manifest with repair/quarantine actions applied.
        #[arg(long)]
        asset_manifest_out: Option<PathBuf>,
        /// Write the health report to this file.
        #[arg(long)]
        health_out: Option<PathBuf>,
    },
    /// Forecast when blocked profile assets become runnable again.
    #[command(name = "identity-assets-forecast")]
    IdentityAssetsForecast {
        /// Runtime profile asset manifest.
        asset_manifest: PathBuf,
        /// Allowed asset state; repeat to allow more than active.
        #[arg(long = "allow-state")]
        allow_state: Vec<String>,
        /// Desired business concurrency used to calculate recovery time.
        #[arg(long)]
        desired_concurrency: Option<usize>,
        /// Only count recoveries within this many seconds as predicted capacity.
        #[arg(long)]
        horizon_seconds: Option<u64>,
        /// Allow assets with active dispatch leases.
        #[arg(long)]
        include_dispatch_leased: bool,
        /// Allow assets currently waiting for dispatch retry cooldown.
        #[arg(long)]
        include_retry: bool,
        /// Allow assets whose latest dispatch failed.
        #[arg(long)]
        include_failed: bool,
        /// Allow assets whose latest dispatch was cancelled.
        #[arg(long)]
        include_cancelled: bool,
        /// Allow assets with active runtime leases.
        #[arg(long)]
        include_runtime_leased: bool,
        /// Allow assets without profileDir/profilePath/userDataDir.
        #[arg(long)]
        include_missing_profile_dir: bool,
        /// Write the forecast report to this file.
        #[arg(long)]
        forecast_out: Option<PathBuf>,
    },
    /// Gate business startup on current or soon-recovering profile asset capacity.
    #[command(name = "identity-assets-gate")]
    IdentityAssetsGate {
        /// Runtime profile asset manifest.
        asset_manifest: PathBuf,
        /// Desired business concurrency that must be satisfied.
        #[arg(long)]
        desired_concurrency: usize,
        /// Maximum seconds the scheduler is allowed to wait for recoverable capacity.
        #[arg(long)]
        max_wait_seconds: Option<u64>,
        /// Treat a recoverable wait decision as a passing gate.
        #[arg(long)]
        allow_wait: bool,
        /// Allowed asset state; repeat to allow more than active.
        #[arg(long = "allow-state")]
        allow_state: Vec<String>,
        /// Allow assets with active dispatch leases.
        #[arg(long)]
        include_dispatch_leased: bool,
        /// Allow assets currently waiting for dispatch retry cooldown.
        #[arg(long)]
        include_retry: bool,
        /// Allow assets whose latest dispatch failed.
        #[arg(long)]
        include_failed: bool,
        /// Allow assets whose latest dispatch was cancelled.
        #[arg(long)]
        include_cancelled: bool,
        /// Allow assets with active runtime leases.
        #[arg(long)]
        include_runtime_leased: bool,
        /// Allow assets without profileDir/profilePath/userDataDir.
        #[arg(long)]
        include_missing_profile_dir: bool,
        /// Write the gate report to this file.
        #[arg(long)]
        gate_out: Option<PathBuf>,
    },
    /// Report runnable capacity and block reasons for a profile asset manifest.
    #[command(name = "identity-assets-status")]
    IdentityAssetsStatus {
        /// Runtime profile asset manifest.
        asset_manifest: PathBuf,
        /// Allowed asset state; repeat to allow more than active.
        #[arg(long = "allow-state")]
        allow_state: Vec<String>,
        /// Desired business concurrency used to calculate shortage.
        #[arg(long)]
        desired_concurrency: Option<usize>,
        /// Allow assets with active dispatch leases.
        #[arg(long)]
        include_dispatch_leased: bool,
        /// Allow assets currently waiting for dispatch retry cooldown.
        #[arg(long)]
        include_retry: bool,
        /// Allow assets whose latest dispatch failed.
        #[arg(long)]
        include_failed: bool,
        /// Allow assets whose latest dispatch was cancelled.
        #[arg(long)]
        include_cancelled: bool,
        /// Allow assets with active runtime leases.
        #[arg(long)]
        include_runtime_leased: bool,
        /// Allow assets without profileDir/profilePath/userDataDir.
        #[arg(long)]
        include_missing_profile_dir: bool,
        /// Write the status report to this file.
        #[arg(long)]
        status_out: Option<PathBuf>,
    },
    /// Select runnable profile assets before starting business automation.
    #[command(name = "identity-assets-select")]
    IdentityAssetsSelect {
        /// Runtime profile asset manifest.
        asset_manifest: PathBuf,
        /// Maximum number of runnable assets to select.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Allowed asset state; repeat to allow more than active.
        #[arg(long = "allow-state")]
        allow_state: Vec<String>,
        /// Worker id used when writing runtime leases.
        #[arg(long)]
        worker: Option<String>,
        /// Business job id recorded on runtime leases and reports.
        #[arg(long)]
        job: Option<String>,
        /// Runtime lease duration in seconds when --asset-manifest-out is used.
        #[arg(long, default_value_t = 900)]
        lease_seconds: u64,
        /// Allow assets with active dispatch leases.
        #[arg(long)]
        include_dispatch_leased: bool,
        /// Allow assets currently waiting for dispatch retry cooldown.
        #[arg(long)]
        include_retry: bool,
        /// Allow assets whose latest dispatch failed.
        #[arg(long)]
        include_failed: bool,
        /// Allow assets whose latest dispatch was cancelled.
        #[arg(long)]
        include_cancelled: bool,
        /// Allow assets with active runtime leases.
        #[arg(long)]
        include_runtime_leased: bool,
        /// Allow assets without profileDir/profilePath/userDataDir.
        #[arg(long)]
        include_missing_profile_dir: bool,
        /// Write a manifest copy with selected assets marked by runtimeLease* fields.
        #[arg(long)]
        asset_manifest_out: Option<PathBuf>,
        /// Write the selection report to this file.
        #[arg(long)]
        selection_out: Option<PathBuf>,
    },
    /// Release runtime leases after business automation finishes.
    #[command(name = "identity-assets-release")]
    IdentityAssetsRelease {
        /// Runtime profile asset manifest containing runtimeLease* fields.
        asset_manifest: PathBuf,
        /// Release status: succeeded, failed, or cancelled.
        #[arg(long, default_value = "succeeded")]
        status: String,
        /// Only release assets leased by this worker.
        #[arg(long)]
        worker: Option<String>,
        /// Only release assets leased for this business job.
        #[arg(long)]
        job: Option<String>,
        /// Only release these runtime lease ids; may be repeated.
        #[arg(long = "lease-id")]
        lease_id: Vec<String>,
        /// Only release these account ids; may be repeated.
        #[arg(long = "account-id")]
        account_id: Vec<String>,
        /// Only release these profile ids; may be repeated.
        #[arg(long = "profile-id")]
        profile_id: Vec<String>,
        /// Only release these identity ids; may be repeated.
        #[arg(long = "identity-id")]
        identity_id: Vec<String>,
        /// Only release these labels; may be repeated.
        #[arg(long)]
        label: Vec<String>,
        /// Set a cooldown in seconds before this asset may be selected again.
        #[arg(long)]
        cooldown_seconds: Option<u64>,
        /// Optional next asset state to write after release.
        #[arg(long)]
        next_state: Option<String>,
        /// Optional release message recorded on matching assets.
        #[arg(long)]
        message: Option<String>,
        /// Optional JSON object/value with business result metadata.
        #[arg(long)]
        result_json: Option<String>,
        /// Write the released profile asset manifest to this file.
        #[arg(long)]
        asset_manifest_out: Option<PathBuf>,
        /// Write the release report to this file.
        #[arg(long)]
        release_out: Option<PathBuf>,
        /// Append released assets as NDJSON ledger items instead of overwriting a JSON report.
        #[arg(long, requires = "release_out")]
        append_release: bool,
    },
    /// Sweep expired runtime leases, dispatch leases, and cooldowns from a profile asset manifest.
    #[command(name = "identity-assets-sweep")]
    IdentityAssetsSweep {
        /// Runtime profile asset manifest to sweep.
        asset_manifest: PathBuf,
        /// Grace period before an expired runtime lease is marked expired.
        #[arg(long, default_value_t = 0)]
        runtime_grace_seconds: u64,
        /// Grace period before an expired dispatch lease is marked expired.
        #[arg(long, default_value_t = 0)]
        dispatch_grace_seconds: u64,
        /// Grace period before an expired cooldown is cleared.
        #[arg(long, default_value_t = 0)]
        cooldown_grace_seconds: u64,
        /// Write the swept profile asset manifest to this file.
        #[arg(long)]
        asset_manifest_out: Option<PathBuf>,
        /// Write the sweep report to this file.
        #[arg(long)]
        sweep_out: Option<PathBuf>,
    },
    /// Run business commands under account/profile runtime governance.
    #[command(name = "identity-job")]
    IdentityJob {
        #[command(subcommand)]
        command: IdentityJobCommand,
    },
    /// Query identity runtime release/risk ledgers for audit and scheduling.
    #[command(name = "identity-ledger")]
    IdentityLedger {
        #[command(subcommand)]
        command: IdentityLedgerCommand,
    },
    /// Record completion results for claimed identity dispatch work.
    #[command(name = "identity-dispatch-complete")]
    IdentityDispatchComplete {
        /// JSON/NDJSON claim report or claim items from identity-dispatch --claim-out.
        claims: PathBuf,
        /// Completion status: succeeded, failed, retry, or cancelled.
        #[arg(long, default_value = "succeeded")]
        status: String,
        /// Only complete claims leased by this worker; also records this worker as completer.
        #[arg(long)]
        worker: Option<String>,
        /// Only complete items from this claim id.
        #[arg(long)]
        claim_id: Option<String>,
        /// Only complete these dedupe keys; may be repeated.
        #[arg(long)]
        dedupe_key: Vec<String>,
        /// Mark failed items as retryable. retry status is always retryable.
        #[arg(long)]
        retryable: bool,
        /// Earliest retry delay in seconds for retryable completions.
        #[arg(long)]
        retry_after_seconds: Option<u64>,
        /// Optional human/machine message recorded on every completion item.
        #[arg(long)]
        message: Option<String>,
        /// Optional JSON object/value with worker result metadata.
        #[arg(long)]
        result_json: Option<String>,
        /// Write completion records to this file.
        #[arg(long)]
        complete_out: Option<PathBuf>,
        /// Append completion records as NDJSON instead of overwriting a JSON report.
        #[arg(long, requires = "complete_out")]
        append_complete: bool,
    },
    /// OCR helpers.
    #[cfg(feature = "ocr")]
    Ocr {
        #[command(subcommand)]
        command: OcrCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum IdentityLedgerCommand {
    /// Build a human-readable runtime governance dashboard from release/risk ledgers.
    Dashboard {
        /// Runtime release ledger from identity-assets-release --append-release; repeatable.
        #[arg(long = "release-ledger")]
        release_ledger: Vec<PathBuf>,
        /// Runtime risk ledger from identity-job run --runtime-risk-out; repeatable.
        #[arg(long = "runtime-risk-ledger")]
        runtime_risk_ledger: Vec<PathBuf>,
        /// Only include events newer than now - window seconds; active risk suppressions are kept.
        #[arg(long)]
        window_seconds: Option<u64>,
        /// Only include this business job id.
        #[arg(long)]
        job: Option<String>,
        /// Only include this worker id.
        #[arg(long)]
        worker: Option<String>,
        /// Only include this failure reason.
        #[arg(long)]
        reason: Option<String>,
        /// Number of latest release/risk evidence items to retain.
        #[arg(long, default_value_t = 50)]
        retain_recent: usize,
        /// Number of top reasons/assets/jobs/workers to return.
        #[arg(long, default_value_t = 20)]
        top: usize,
        /// Previous compact/dashboard JSON whose sourceCheckpoints should be used as byte offsets.
        #[arg(long)]
        checkpoint_in: Option<PathBuf>,
        /// Write source byte checkpoints for the next incremental dashboard/compact run.
        #[arg(long)]
        checkpoint_out: Option<PathBuf>,
        /// Write the dashboard JSON report to this file.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Write a standalone HTML dashboard to this file.
        #[arg(long)]
        html_out: Option<PathBuf>,
    },
    /// Compact runtime release/risk ledgers into a durable scheduling summary.
    Compact {
        /// Runtime release ledger from identity-assets-release --append-release; repeatable.
        #[arg(long = "release-ledger")]
        release_ledger: Vec<PathBuf>,
        /// Runtime risk ledger from identity-job run --runtime-risk-out; repeatable.
        #[arg(long = "runtime-risk-ledger")]
        runtime_risk_ledger: Vec<PathBuf>,
        /// Only include events newer than now - window seconds; active risk suppressions are kept.
        #[arg(long)]
        window_seconds: Option<u64>,
        /// Only compact this business job id.
        #[arg(long)]
        job: Option<String>,
        /// Only compact this worker id.
        #[arg(long)]
        worker: Option<String>,
        /// Only compact this failure reason.
        #[arg(long)]
        reason: Option<String>,
        /// Number of latest release/risk evidence items to retain.
        #[arg(long, default_value_t = 50)]
        retain_recent: usize,
        /// Number of top reasons/assets/jobs/workers to return.
        #[arg(long, default_value_t = 20)]
        top: usize,
        /// Previous compact/dashboard JSON whose sourceCheckpoints should be used as byte offsets.
        #[arg(long)]
        checkpoint_in: Option<PathBuf>,
        /// Write source byte checkpoints for the next incremental compact/dashboard run.
        #[arg(long)]
        checkpoint_out: Option<PathBuf>,
        /// Write the compacted ledger summary to this file.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Aggregate runtime release and runtime-risk ledgers.
    Query {
        /// Runtime release ledger from identity-assets-release --append-release; repeatable.
        #[arg(long = "release-ledger")]
        release_ledger: Vec<PathBuf>,
        /// Runtime risk ledger from identity-job run --runtime-risk-out; repeatable.
        #[arg(long = "runtime-risk-ledger")]
        runtime_risk_ledger: Vec<PathBuf>,
        /// Only include events newer than now - window seconds; active risk suppressions are kept.
        #[arg(long)]
        window_seconds: Option<u64>,
        /// Only include this business job id.
        #[arg(long)]
        job: Option<String>,
        /// Only include this worker id.
        #[arg(long)]
        worker: Option<String>,
        /// Only include this failure reason.
        #[arg(long)]
        reason: Option<String>,
        /// Number of top reasons/assets/jobs/workers to return.
        #[arg(long, default_value_t = 10)]
        top: usize,
        /// Write the query report to this file.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Explain why a job/account/profile should run, wait, or stay suppressed.
    Explain {
        /// Runtime release ledger from identity-assets-release --append-release; repeatable.
        #[arg(long = "release-ledger")]
        release_ledger: Vec<PathBuf>,
        /// Runtime risk ledger from identity-job run --runtime-risk-out; repeatable.
        #[arg(long = "runtime-risk-ledger")]
        runtime_risk_ledger: Vec<PathBuf>,
        /// Only include release/risk events newer than now - window seconds; active risk suppressions are kept.
        #[arg(long)]
        window_seconds: Option<u64>,
        /// Explain only this business job id.
        #[arg(long)]
        job: Option<String>,
        /// Explain only this worker id.
        #[arg(long)]
        worker: Option<String>,
        /// Explain only this failure reason.
        #[arg(long)]
        reason: Option<String>,
        /// Explain this account id.
        #[arg(long)]
        account_id: Option<String>,
        /// Explain this profile id.
        #[arg(long)]
        profile_id: Option<String>,
        /// Explain this identity id.
        #[arg(long)]
        identity_id: Option<String>,
        /// Explain this asset label.
        #[arg(long)]
        label: Option<String>,
        /// Explain this profile directory.
        #[arg(long)]
        profile_dir: Option<PathBuf>,
        /// Explain this runtime lease id.
        #[arg(long)]
        lease_id: Option<String>,
        /// Maximum latest release/risk evidence items to include.
        #[arg(long, default_value_t = 20)]
        evidence_limit: usize,
        /// Write the explanation report to this file.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum IdentityJobCommand {
    /// Sweep, validate, gate, select, run a command, then release runtime leases.
    Run {
        /// Runtime profile asset manifest.
        asset_manifest: PathBuf,
        /// JSON governance policy used as defaults for job runtime behavior.
        #[arg(long)]
        policy: Option<PathBuf>,
        /// Built-in job governance preset: publish_conservative, login_sensitive, or scrape_aggressive.
        #[arg(long)]
        job_preset: Option<String>,
        /// Desired business concurrency that must be available before running.
        #[arg(long)]
        desired_concurrency: Option<usize>,
        /// Maximum number of runnable assets to lease for this command.
        #[arg(long)]
        limit: Option<usize>,
        /// Worker id recorded on runtime leases; defaults to identity-job-<pid>.
        #[arg(long)]
        worker: Option<String>,
        /// Business job id recorded on runtime leases; defaults to identity-job.
        #[arg(long)]
        job: Option<String>,
        /// Runtime lease duration in seconds.
        #[arg(long)]
        lease_seconds: Option<u64>,
        /// Maximum seconds the scheduler is allowed to wait for recoverable capacity.
        #[arg(long)]
        max_wait_seconds: Option<u64>,
        /// Treat a recoverable wait decision as a passing gate.
        #[arg(long)]
        allow_wait: bool,
        /// Run the wrapped command once for every selected asset.
        #[arg(long)]
        per_asset: bool,
        /// Max child processes to run at once in --per-asset mode.
        #[arg(long)]
        child_concurrency: Option<usize>,
        /// Renew selected runtime leases while the wrapped command is still running.
        #[arg(long)]
        runtime_renew_interval_seconds: Option<u64>,
        /// Kill the wrapped child command if it runs longer than this many seconds.
        #[arg(long)]
        child_timeout_seconds: Option<u64>,
        /// Directory where child commands may write JSON result files for release decisions.
        #[arg(long)]
        child_result_dir: Option<PathBuf>,
        /// Stop launching new per-asset children after this many failed assets.
        #[arg(long)]
        max_failed_assets: Option<usize>,
        /// Stop launching new per-asset children after the same failure reason reaches this count.
        #[arg(long)]
        max_failed_assets_per_reason: Option<usize>,
        /// Allowed asset state; repeat to allow more than active.
        #[arg(long = "allow-state")]
        allow_state: Vec<String>,
        /// Allow assets with active dispatch leases.
        #[arg(long)]
        include_dispatch_leased: bool,
        /// Allow assets currently waiting for dispatch retry cooldown.
        #[arg(long)]
        include_retry: bool,
        /// Allow assets whose latest dispatch failed.
        #[arg(long)]
        include_failed: bool,
        /// Allow assets whose latest dispatch was cancelled.
        #[arg(long)]
        include_cancelled: bool,
        /// Allow assets with active runtime leases.
        #[arg(long)]
        include_runtime_leased: bool,
        /// Allow assets without profileDir/profilePath/userDataDir.
        #[arg(long)]
        include_missing_profile_dir: bool,
        /// Skip expired runtime/dispatch/cooldown cleanup before gating.
        #[arg(long)]
        skip_sweep: bool,
        /// Skip profile asset manifest validation before gating.
        #[arg(long)]
        skip_validate: bool,
        /// Grace period before an expired runtime lease is marked expired.
        #[arg(long)]
        runtime_grace_seconds: Option<u64>,
        /// Grace period before an expired dispatch lease is marked expired.
        #[arg(long)]
        dispatch_grace_seconds: Option<u64>,
        /// Grace period before an expired cooldown is cleared.
        #[arg(long)]
        cooldown_grace_seconds: Option<u64>,
        /// Cooldown seconds to apply when the wrapped command fails.
        #[arg(long)]
        failure_cooldown_seconds: Option<u64>,
        /// Next asset state to write when the wrapped command fails, e.g. repair.
        #[arg(long)]
        failure_next_state: Option<String>,
        /// Write the working manifest with runtime leases and release results.
        #[arg(long)]
        asset_manifest_out: Option<PathBuf>,
        /// Write sweep report to this file.
        #[arg(long)]
        sweep_out: Option<PathBuf>,
        /// Write validation report to this file.
        #[arg(long)]
        validate_out: Option<PathBuf>,
        /// Write gate report to this file.
        #[arg(long)]
        gate_out: Option<PathBuf>,
        /// Write selection report to this file.
        #[arg(long)]
        selection_out: Option<PathBuf>,
        /// Write release report or ledger to this file.
        #[arg(long)]
        release_out: Option<PathBuf>,
        /// Append released assets as NDJSON ledger items instead of overwriting a JSON report.
        #[arg(long, requires = "release_out")]
        append_release: bool,
        /// Read previous runtime risk events before leasing assets; repeatable.
        #[arg(long = "runtime-risk-ledger")]
        runtime_risk_ledger: Vec<PathBuf>,
        /// Only consider runtime risk events newer than now - seconds.
        #[arg(long)]
        runtime_risk_window_seconds: Option<u64>,
        /// Write compact runtime risk advice to this file.
        #[arg(long)]
        runtime_risk_out: Option<PathBuf>,
        /// Append runtime risk advice as NDJSON instead of overwriting a JSON report.
        #[arg(long, requires = "runtime_risk_out")]
        append_runtime_risk: bool,
        /// Write a compact machine-readable explanation for stage and asset decisions.
        #[arg(long)]
        explain_out: Option<PathBuf>,
        /// Write the wrapper report to this file.
        #[arg(long)]
        job_out: Option<PathBuf>,
        /// Business command to execute after `--`.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
}

#[derive(Debug, Args, Clone)]
pub struct RunBrowserArgs {
    /// Browser backend to run.
    #[arg(long, value_enum, default_value_t = BackendKind::Cdp)]
    pub backend: BackendKind,
    /// Run browser headless.
    #[arg(long)]
    pub headless: bool,
    /// Persistent browser profile directory.
    #[arg(long)]
    pub user_data_dir: Option<PathBuf>,
}

#[derive(Debug, Args, Clone)]
pub struct SetupArgs {
    /// Which client(s) to configure.
    #[arg(long, value_enum, default_value = "both")]
    pub target: SetupTarget,
    /// Cursor config scope: project-local `.cursor/mcp.json` or global `~/.cursor/mcp.json`.
    #[arg(long, value_enum, default_value = "project")]
    pub scope: SetupScope,
    /// Project directory for project-scoped Cursor config. Defaults to the current directory.
    #[arg(long)]
    pub dir: Option<PathBuf>,
    /// Browser backend written into the MCP server entry.
    #[arg(long, value_enum, default_value_t = BackendKind::Cdp)]
    pub backend: BackendKind,
    /// Do not add `--headless` to the generated server command.
    #[arg(long)]
    pub no_headless: bool,
    /// MCP server key/name to write.
    #[arg(long, default_value = "drs")]
    pub name: String,
    /// Print the planned changes without writing any files.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args, Clone, Default)]
pub struct McpArgs {
    /// Browser backend to run.
    #[arg(long, value_enum, default_value_t = BackendKind::Cdp)]
    pub backend: BackendKind,
    /// Run browser headless.
    #[arg(long)]
    pub headless: bool,
    /// Persistent browser profile directory. Defaults to a stable per-user
    /// profile so that logged-in sessions survive restarts.
    #[arg(long)]
    pub user_data_dir: Option<PathBuf>,
    /// Hold a browser inside this MCP process instead of attaching to the
    /// shared persistent `drs serve` daemon.
    #[arg(long)]
    pub standalone: bool,
}

#[derive(Debug, Args, Clone, Default)]
pub struct IdentityGateArgs {
    /// Named identity admission policy: lenient, balanced, or strict.
    #[arg(long = "gate-preset", value_enum)]
    pub gate_preset: Option<IdentityGatePreset>,
    /// Minimum acceptable identity score for every checked tab.
    #[arg(long)]
    pub min_score: Option<u8>,
    /// Maximum acceptable pairwise linkability score.
    #[arg(long)]
    pub max_linkability: Option<u8>,
    /// Maximum acceptable largest stable-signal bucket ratio, e.g. 0.8.
    #[arg(long)]
    pub max_concentration_ratio: Option<f64>,
    /// Maximum number of stable signals that may repeat inside the pool.
    #[arg(long)]
    pub max_concentrated_signals: Option<usize>,
    /// Minimum acceptable identity entropy score for the pool.
    #[arg(long)]
    pub min_entropy_score: Option<u8>,
    /// Minimum acceptable effective identity count for the pool.
    #[arg(long)]
    pub min_effective_identities: Option<f64>,
    /// Maximum acceptable nominal/effective identity ratio for the pool.
    #[arg(long)]
    pub max_nominal_to_effective_ratio: Option<f64>,
    /// Treat any high-risk identity issue as a failed gate.
    #[arg(long)]
    pub fail_on_high_risk: bool,
    /// Treat any risky pool pair as a failed gate.
    #[arg(long)]
    pub fail_on_risky_pairs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentitySnapshotExport {
    pub path: PathBuf,
    pub append: bool,
}

impl IdentityGateArgs {
    pub fn is_active(&self) -> bool {
        self.gate_preset.is_some()
            || self.min_score.is_some()
            || self.max_linkability.is_some()
            || self.max_concentration_ratio.is_some()
            || self.max_concentrated_signals.is_some()
            || self.min_entropy_score.is_some()
            || self.min_effective_identities.is_some()
            || self.max_nominal_to_effective_ratio.is_some()
            || self.fail_on_high_risk
            || self.fail_on_risky_pairs
    }
}

impl From<IdentityGateArgs> for IdentityGate {
    fn from(args: IdentityGateArgs) -> Self {
        Self {
            preset: args.gate_preset,
            min_score: args.min_score,
            max_linkability: args.max_linkability,
            max_concentration_ratio: args.max_concentration_ratio,
            max_concentrated_signals: args.max_concentrated_signals,
            min_entropy_score: args.min_entropy_score,
            min_effective_identity_count: args.min_effective_identities,
            max_nominal_to_effective_ratio: args.max_nominal_to_effective_ratio,
            fail_on_high_risk: args.fail_on_high_risk,
            fail_on_risky_pairs: args.fail_on_risky_pairs,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum ListenCommand {
    /// Start network listening.
    Start {
        keywords: Vec<String>,
        #[arg(long)]
        xhr_only: bool,
    },
    /// Wait for network packets.
    Wait {
        #[arg(long, default_value_t = 1)]
        count: usize,
        #[arg(long)]
        timeout_ms: Option<u64>,
    },
    /// Stop network listening.
    Stop,
}

#[cfg(feature = "ocr")]
#[derive(Debug, Subcommand)]
pub enum OcrCommand {
    /// Solve click-word coordinates from an image and target text.
    Clickword { image: PathBuf, targets: String },
}

impl Command {
    pub fn into_engine(self) -> Option<EngineCommand> {
        Some(match self {
            Command::Serve(_) | Command::Mcp(_) | Command::EnsureServe(_) | Command::Setup(_) => {
                return None;
            }
            #[cfg(feature = "ocr")]
            Command::Ocr { .. } => return None,
            Command::Status => EngineCommand::Status,
            Command::Stop => EngineCommand::Stop,
            Command::Open { url } => EngineCommand::Open { url },
            Command::Tabs => EngineCommand::Tabs,
            Command::Use { tab_id } => EngineCommand::UseTab { tab_id },
            Command::Close { tab_id } => EngineCommand::Close { tab_id },
            Command::Ax { outline, tree_json } => {
                let format = if tree_json && !outline {
                    crate::protocol::AxFormat::Json
                } else {
                    crate::protocol::AxFormat::Outline
                };
                EngineCommand::Ax { format }
            }
            Command::Html => EngineCommand::Html,
            Command::Title => EngineCommand::Title,
            Command::Url => EngineCommand::Url,
            Command::Extract {
                url,
                wait_selector,
                timeout_ms,
                pass_cf,
                include_html,
                include_ax_json,
                max_text_chars,
                screenshot_out,
                full,
                save_out: _,
            } => EngineCommand::Extract {
                url,
                wait_selector,
                timeout_ms,
                pass_cf,
                include_html,
                include_ax_json,
                max_text_chars,
                screenshot_out,
                full_screenshot: full,
            },
            Command::Text { selector } => EngineCommand::Text { selector },
            Command::Eval { js } => EngineCommand::Eval { js },
            Command::Screenshot { out, full, inline } => {
                EngineCommand::Screenshot { out, full, inline }
            }
            Command::Click { selector } => EngineCommand::Click { selector },
            Command::Type { selector, text } => EngineCommand::Type { selector, text },
            Command::Press { key, selector } => EngineCommand::Press { key, selector },
            Command::Wait {
                selector,
                timeout_ms,
            } => EngineCommand::Wait {
                selector,
                timeout_ms,
            },
            Command::Listen { command } => match command {
                ListenCommand::Start { keywords, xhr_only } => {
                    EngineCommand::ListenStart { keywords, xhr_only }
                }
                ListenCommand::Wait { count, timeout_ms } => {
                    EngineCommand::ListenWait { count, timeout_ms }
                }
                ListenCommand::Stop => EngineCommand::ListenStop,
            },
            Command::PassCf { timeout_ms } => EngineCommand::PassCf { timeout_ms },
            Command::Identity { pool, gate, .. } => EngineCommand::Identity {
                pool,
                gate: gate.into(),
            },
            Command::IdentityPool { .. }
            | Command::IdentityDrift { .. }
            | Command::IdentityLifecycle { .. }
            | Command::IdentityApply { .. }
            | Command::IdentityPlan { .. }
            | Command::IdentityDispatch { .. }
            | Command::IdentityDispatchRenew { .. }
            | Command::IdentityDispatchReconcile { .. }
            | Command::IdentityAssetsValidate { .. }
            | Command::IdentityAssetsReconcileRuntime { .. }
            | Command::IdentityAssetsHealth { .. }
            | Command::IdentityAssetsForecast { .. }
            | Command::IdentityAssetsGate { .. }
            | Command::IdentityAssetsStatus { .. }
            | Command::IdentityAssetsSelect { .. }
            | Command::IdentityAssetsRelease { .. }
            | Command::IdentityAssetsSweep { .. }
            | Command::IdentityJob { .. }
            | Command::IdentityLedger { .. }
            | Command::IdentityDispatchComplete { .. } => return None,
        })
    }

    pub fn identity_gate_is_active(&self) -> bool {
        match self {
            Command::Identity { gate, .. } | Command::IdentityPool { gate, .. } => gate.is_active(),
            Command::IdentityDrift {
                max_drift_score,
                fail_on_high_risk_drift,
                policy,
                ..
            } => max_drift_score.is_some() || *fail_on_high_risk_drift || policy.is_some(),
            Command::IdentityLifecycle {
                max_drift_score,
                fail_on_high_risk_drift,
                fail_on_missing_current,
                fail_on_new_current,
                policy,
                ..
            } => {
                max_drift_score.is_some()
                    || *fail_on_high_risk_drift
                    || *fail_on_missing_current
                    || *fail_on_new_current
                    || policy.is_some()
            }
            _ => false,
        }
    }

    pub fn identity_snapshot_export(&self) -> Option<IdentitySnapshotExport> {
        match self {
            Command::Identity {
                snapshots_out,
                append_snapshots,
                ..
            } => snapshots_out.as_ref().map(|path| IdentitySnapshotExport {
                path: path.clone(),
                append: *append_snapshots,
            }),
            _ => None,
        }
    }

    pub fn extract_save_out(&self) -> Option<PathBuf> {
        match self {
            Command::Extract { save_out, .. } => save_out.clone(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn parses_global_json_open() {
        let cli = Cli::try_parse_from(["drs", "--json", "open", "https://example.com"]).unwrap();
        assert!(cli.json);
        match cli.command.into_engine().unwrap() {
            EngineCommand::Open { url } => assert_eq!(url, "https://example.com"),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn mcp_defaults_to_attached_daemon() {
        let cli = Cli::try_parse_from(["drs", "mcp", "--headless"]).unwrap();
        match cli.command {
            Command::Mcp(args) => {
                assert!(args.headless);
                assert!(
                    !args.standalone,
                    "MCP should attach to the daemon by default"
                );
                assert_eq!(args.user_data_dir, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn mcp_standalone_flag_parses() {
        let cli = Cli::try_parse_from(["drs", "mcp", "--standalone"]).unwrap();
        match cli.command {
            Command::Mcp(args) => assert!(args.standalone),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_type_command() {
        let cli = Cli::try_parse_from(["drs", "type", "#kw", "hello"]).unwrap();
        match cli.command.into_engine().unwrap() {
            EngineCommand::Type { selector, text } => {
                assert_eq!(selector, "#kw");
                assert_eq!(text, "hello");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_extract_command() {
        let cli = Cli::try_parse_from([
            "drs",
            "--json",
            "extract",
            "https://example.com",
            "--pass-cf",
            "--save-out",
            "/tmp/page.json",
        ])
        .unwrap();
        assert_eq!(
            cli.command.extract_save_out().as_deref(),
            Some(std::path::Path::new("/tmp/page.json"))
        );
        match cli.command.into_engine().unwrap() {
            EngineCommand::Extract { url, pass_cf, .. } => {
                assert_eq!(url.as_deref(), Some("https://example.com"));
                assert!(pass_cf);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_global_ensure_serve() {
        let cli = Cli::try_parse_from([
            "drs",
            "--ensure-serve",
            "--ensure-headless",
            "--json",
            "status",
        ])
        .unwrap();
        assert!(cli.ensure.ensure_serve);
        assert!(cli.ensure.ensure_headless);
    }

    #[test]
    fn parses_ax_json_flag() {
        let cli = Cli::try_parse_from(["drs", "ax", "--json"]).unwrap();
        match cli.command.into_engine().unwrap() {
            EngineCommand::Ax { format } => {
                assert!(matches!(format, crate::protocol::AxFormat::Json));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_pool() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity",
            "--pool",
            "--gate-preset",
            "balanced",
            "--min-score",
            "85",
            "--max-linkability",
            "25",
            "--max-concentration-ratio",
            "0.85",
            "--max-concentrated-signals",
            "6",
            "--min-entropy-score",
            "60",
            "--min-effective-identities",
            "8.5",
            "--max-nominal-to-effective-ratio",
            "1.8",
            "--fail-on-high-risk",
            "--fail-on-risky-pairs",
            "--snapshots-out",
            "pool.json",
        ])
        .unwrap();
        assert!(cli.command.identity_gate_is_active());
        assert_eq!(
            cli.command.identity_snapshot_export(),
            Some(IdentitySnapshotExport {
                path: PathBuf::from("pool.json"),
                append: false,
            })
        );
        match cli.command.into_engine().unwrap() {
            EngineCommand::Identity { pool, gate } => {
                assert!(pool);
                assert_eq!(gate.preset, Some(IdentityGatePreset::Balanced));
                assert_eq!(gate.min_score, Some(85));
                assert_eq!(gate.max_linkability, Some(25));
                assert_eq!(gate.max_concentration_ratio, Some(0.85));
                assert_eq!(gate.max_concentrated_signals, Some(6));
                assert_eq!(gate.min_entropy_score, Some(60));
                assert_eq!(gate.min_effective_identity_count, Some(8.5));
                assert_eq!(gate.max_nominal_to_effective_ratio, Some(1.8));
                assert!(gate.fail_on_high_risk);
                assert!(gate.fail_on_risky_pairs);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_offline_identity_pool() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-pool",
            "snapshots.json",
            "--against",
            "baseline.json",
            "--accept-out",
            "accepted.json",
            "--quarantine-out",
            "quarantine.json",
            "--baseline-out",
            "next-baseline.json",
            "--ledger-out",
            "ledger.ndjson",
            "--actions-out",
            "pool-actions.ndjson",
            "--append-split",
            "--append-ledger",
            "--append-actions",
            "--gate-preset",
            "strict",
            "--max-linkability",
            "30",
            "--max-concentrated-signals",
            "3",
            "--min-entropy-score",
            "70",
            "--min-effective-identities",
            "10",
            "--max-nominal-to-effective-ratio",
            "2.5",
        ])
        .unwrap();
        assert!(cli.command.identity_gate_is_active());
        match cli.command {
            Command::IdentityPool {
                snapshots,
                policy,
                against,
                accept_out,
                quarantine_out,
                baseline_out,
                ledger_out,
                actions_out,
                append_split,
                append_ledger,
                append_actions,
                gate,
            } => {
                assert_eq!(snapshots, PathBuf::from("snapshots.json"));
                assert_eq!(policy, None);
                assert_eq!(against, Some(PathBuf::from("baseline.json")));
                assert_eq!(accept_out, Some(PathBuf::from("accepted.json")));
                assert_eq!(quarantine_out, Some(PathBuf::from("quarantine.json")));
                assert_eq!(baseline_out, Some(PathBuf::from("next-baseline.json")));
                assert_eq!(ledger_out, Some(PathBuf::from("ledger.ndjson")));
                assert_eq!(actions_out, Some(PathBuf::from("pool-actions.ndjson")));
                assert!(append_split);
                assert!(append_ledger);
                assert!(append_actions);
                assert_eq!(gate.gate_preset, Some(IdentityGatePreset::Strict));
                assert_eq!(gate.max_linkability, Some(30));
                assert_eq!(gate.max_concentrated_signals, Some(3));
                assert_eq!(gate.min_entropy_score, Some(70));
                assert_eq!(gate.min_effective_identities, Some(10.0));
                assert_eq!(gate.max_nominal_to_effective_ratio, Some(2.5));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_drift() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-drift",
            "before.json",
            "after.json",
            "--max-drift-score",
            "20",
            "--fail-on-high-risk-drift",
            "--match-by",
            "label",
            "--actions-out",
            "actions.ndjson",
            "--append-actions",
        ])
        .unwrap();
        assert!(cli.command.identity_gate_is_active());
        match cli.command {
            Command::IdentityDrift {
                before,
                after,
                max_drift_score,
                fail_on_high_risk_drift,
                match_by,
                policy,
                actions_out,
                append_actions,
            } => {
                assert_eq!(before, PathBuf::from("before.json"));
                assert_eq!(after, PathBuf::from("after.json"));
                assert_eq!(max_drift_score, Some(20));
                assert!(fail_on_high_risk_drift);
                assert_eq!(match_by, Some(IdentityDriftMatchMode::Label));
                assert_eq!(policy, None);
                assert_eq!(actions_out, Some(PathBuf::from("actions.ndjson")));
                assert!(append_actions);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_governance_policy_flags() {
        let pool = Cli::try_parse_from([
            "drs",
            "identity-pool",
            "snapshots.json",
            "--policy",
            "identity-policy.json",
        ])
        .unwrap();
        match pool.command {
            Command::IdentityPool { policy, gate, .. } => {
                assert_eq!(policy, Some(PathBuf::from("identity-policy.json")));
                assert!(gate.gate_preset.is_none());
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let drift = Cli::try_parse_from([
            "drs",
            "identity-drift",
            "before.json",
            "after.json",
            "--policy",
            "identity-policy.json",
        ])
        .unwrap();
        match drift.command {
            Command::IdentityDrift {
                policy, match_by, ..
            } => {
                assert_eq!(policy, Some(PathBuf::from("identity-policy.json")));
                assert_eq!(match_by, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let lifecycle = Cli::try_parse_from([
            "drs",
            "identity-lifecycle",
            "baseline.json",
            "current.json",
            "--policy",
            "identity-policy.json",
        ])
        .unwrap();
        match lifecycle.command {
            Command::IdentityLifecycle {
                policy,
                next_baseline_policy,
                ..
            } => {
                assert_eq!(policy, Some(PathBuf::from("identity-policy.json")));
                assert_eq!(next_baseline_policy, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let health = Cli::try_parse_from([
            "drs",
            "identity-assets-health",
            "runtime-profile-assets.json",
            "--policy",
            "identity-policy.json",
            "--release-ledger",
            "runtime-release.ndjson",
        ])
        .unwrap();
        match health.command {
            Command::IdentityAssetsHealth {
                policy,
                repair_threshold,
                quarantine_threshold,
                cooldown_seconds,
                ..
            } => {
                assert_eq!(policy, Some(PathBuf::from("identity-policy.json")));
                assert_eq!(repair_threshold, None);
                assert_eq!(quarantine_threshold, None);
                assert_eq!(cooldown_seconds, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_apply() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-apply",
            "actions.ndjson",
            "--profile-root",
            "profiles",
            "--profile-map",
            "profile-map.json",
            "--quarantine-dir",
            "quarantine",
            "--execute",
            "--journal-out",
            "apply.ndjson",
            "--append-journal",
            "--asset-state-out",
            "asset-state.ndjson",
            "--append-asset-state",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityApply {
                actions,
                profile_root,
                profile_map,
                quarantine_dir,
                execute,
                journal_out,
                append_journal,
                asset_state_out,
                append_asset_state,
            } => {
                assert_eq!(actions, PathBuf::from("actions.ndjson"));
                assert_eq!(profile_root, Some(PathBuf::from("profiles")));
                assert_eq!(profile_map, Some(PathBuf::from("profile-map.json")));
                assert_eq!(quarantine_dir, Some(PathBuf::from("quarantine")));
                assert!(execute);
                assert_eq!(journal_out, Some(PathBuf::from("apply.ndjson")));
                assert!(append_journal);
                assert_eq!(asset_state_out, Some(PathBuf::from("asset-state.ndjson")));
                assert!(append_asset_state);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_plan() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-plan",
            "pool.json",
            "actions.ndjson",
            "--title",
            "Nightly identity audit",
            "--out",
            "plan.json",
            "--html-out",
            "plan.html",
            "--asset-manifest",
            "profile-assets.json",
            "--asset-manifest-out",
            "next-profile-assets.json",
            "--dispatch-out",
            "dispatch.ndjson",
            "--append-dispatch",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityPlan {
                inputs,
                title,
                out,
                html_out,
                asset_manifest,
                asset_manifest_out,
                dispatch_out,
                append_dispatch,
            } => {
                assert_eq!(
                    inputs,
                    vec![PathBuf::from("pool.json"), PathBuf::from("actions.ndjson")]
                );
                assert_eq!(title, Some("Nightly identity audit".to_string()));
                assert_eq!(out, Some(PathBuf::from("plan.json")));
                assert_eq!(html_out, Some(PathBuf::from("plan.html")));
                assert_eq!(asset_manifest, Some(PathBuf::from("profile-assets.json")));
                assert_eq!(
                    asset_manifest_out,
                    Some(PathBuf::from("next-profile-assets.json"))
                );
                assert_eq!(dispatch_out, Some(PathBuf::from("dispatch.ndjson")));
                assert!(append_dispatch);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_dispatch() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-dispatch",
            "dispatch.ndjson",
            "--worker",
            "worker-a",
            "--limit",
            "3",
            "--lease-seconds",
            "120",
            "--include-blocked",
            "--claim-ledger",
            "existing-claims.ndjson",
            "--include-leased",
            "--completion-ledger",
            "completed.ndjson",
            "--include-completed",
            "--claim-out",
            "claims.ndjson",
            "--append-claim",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityDispatch {
                dispatch,
                worker,
                limit,
                lease_seconds,
                include_blocked,
                claim_ledger,
                include_leased,
                completion_ledger,
                include_completed,
                claim_out,
                append_claim,
            } => {
                assert_eq!(dispatch, PathBuf::from("dispatch.ndjson"));
                assert_eq!(worker, Some("worker-a".to_string()));
                assert_eq!(limit, 3);
                assert_eq!(lease_seconds, 120);
                assert!(include_blocked);
                assert_eq!(claim_ledger, Some(PathBuf::from("existing-claims.ndjson")));
                assert!(include_leased);
                assert_eq!(completion_ledger, Some(PathBuf::from("completed.ndjson")));
                assert!(include_completed);
                assert_eq!(claim_out, Some(PathBuf::from("claims.ndjson")));
                assert!(append_claim);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_dispatch_complete() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-dispatch-complete",
            "claims.ndjson",
            "--status",
            "retry",
            "--worker",
            "worker-a",
            "--claim-id",
            "claim_1",
            "--dedupe-key",
            "quarantine:acct-a",
            "--retryable",
            "--retry-after-seconds",
            "60",
            "--message",
            "proxy cooldown",
            "--result-json",
            r#"{"proxyId":"p1"}"#,
            "--complete-out",
            "completed.ndjson",
            "--append-complete",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityDispatchComplete {
                claims,
                status,
                worker,
                claim_id,
                dedupe_key,
                retryable,
                retry_after_seconds,
                message,
                result_json,
                complete_out,
                append_complete,
            } => {
                assert_eq!(claims, PathBuf::from("claims.ndjson"));
                assert_eq!(status, "retry");
                assert_eq!(worker, Some("worker-a".to_string()));
                assert_eq!(claim_id, Some("claim_1".to_string()));
                assert_eq!(dedupe_key, vec!["quarantine:acct-a".to_string()]);
                assert!(retryable);
                assert_eq!(retry_after_seconds, Some(60));
                assert_eq!(message, Some("proxy cooldown".to_string()));
                assert_eq!(result_json, Some(r#"{"proxyId":"p1"}"#.to_string()));
                assert_eq!(complete_out, Some(PathBuf::from("completed.ndjson")));
                assert!(append_complete);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_dispatch_renew() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-dispatch-renew",
            "claims.ndjson",
            "--worker",
            "worker-a",
            "--claim-id",
            "claim_1",
            "--dedupe-key",
            "quarantine:acct-a",
            "--lease-seconds",
            "300",
            "--include-expired",
            "--claim-out",
            "claims.ndjson",
            "--append-claim",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityDispatchRenew {
                claims,
                worker,
                claim_id,
                dedupe_key,
                lease_seconds,
                include_expired,
                claim_out,
                append_claim,
            } => {
                assert_eq!(claims, PathBuf::from("claims.ndjson"));
                assert_eq!(worker, Some("worker-a".to_string()));
                assert_eq!(claim_id, Some("claim_1".to_string()));
                assert_eq!(dedupe_key, vec!["quarantine:acct-a".to_string()]);
                assert_eq!(lease_seconds, 300);
                assert!(include_expired);
                assert_eq!(claim_out, Some(PathBuf::from("claims.ndjson")));
                assert!(append_claim);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_dispatch_reconcile() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-dispatch-reconcile",
            "profile-assets.json",
            "--claim-ledger",
            "claims.ndjson",
            "--completion-ledger",
            "completed.ndjson",
            "--asset-manifest-out",
            "next-profile-assets.json",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityDispatchReconcile {
                asset_manifest,
                claim_ledger,
                completion_ledger,
                asset_manifest_out,
            } => {
                assert_eq!(asset_manifest, PathBuf::from("profile-assets.json"));
                assert_eq!(claim_ledger, Some(PathBuf::from("claims.ndjson")));
                assert_eq!(completion_ledger, Some(PathBuf::from("completed.ndjson")));
                assert_eq!(
                    asset_manifest_out,
                    Some(PathBuf::from("next-profile-assets.json"))
                );
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_assets_validate() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-assets-validate",
            "runtime-profile-assets.json",
            "--strict",
            "--validate-out",
            "validate.json",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityAssetsValidate {
                asset_manifest,
                strict,
                validate_out,
            } => {
                assert_eq!(asset_manifest, PathBuf::from("runtime-profile-assets.json"));
                assert!(strict);
                assert_eq!(validate_out, Some(PathBuf::from("validate.json")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_assets_reconcile_runtime() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-assets-reconcile-runtime",
            "runtime-profile-assets.json",
            "--release-ledger",
            "runtime-a.ndjson",
            "--release-ledger",
            "runtime-b.ndjson",
            "--asset-manifest-out",
            "next-runtime-profile-assets.json",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityAssetsReconcileRuntime {
                asset_manifest,
                release_ledger,
                asset_manifest_out,
            } => {
                assert_eq!(asset_manifest, PathBuf::from("runtime-profile-assets.json"));
                assert_eq!(
                    release_ledger,
                    vec![
                        PathBuf::from("runtime-a.ndjson"),
                        PathBuf::from("runtime-b.ndjson")
                    ]
                );
                assert_eq!(
                    asset_manifest_out,
                    Some(PathBuf::from("next-runtime-profile-assets.json"))
                );
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_assets_health() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-assets-health",
            "runtime-profile-assets.json",
            "--policy",
            "identity-policy.json",
            "--release-ledger",
            "runtime-a.ndjson",
            "--release-ledger",
            "runtime-b.ndjson",
            "--window-seconds",
            "86400",
            "--repair-threshold",
            "2",
            "--quarantine-threshold",
            "4",
            "--cooldown-seconds",
            "1800",
            "--asset-manifest-out",
            "health-profile-assets.json",
            "--health-out",
            "asset-health.json",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityAssetsHealth {
                asset_manifest,
                policy,
                release_ledger,
                window_seconds,
                repair_threshold,
                quarantine_threshold,
                cooldown_seconds,
                asset_manifest_out,
                health_out,
            } => {
                assert_eq!(asset_manifest, PathBuf::from("runtime-profile-assets.json"));
                assert_eq!(policy, Some(PathBuf::from("identity-policy.json")));
                assert_eq!(
                    release_ledger,
                    vec![
                        PathBuf::from("runtime-a.ndjson"),
                        PathBuf::from("runtime-b.ndjson")
                    ]
                );
                assert_eq!(window_seconds, Some(86400));
                assert_eq!(repair_threshold, Some(2));
                assert_eq!(quarantine_threshold, Some(4));
                assert_eq!(cooldown_seconds, Some(1800));
                assert_eq!(
                    asset_manifest_out,
                    Some(PathBuf::from("health-profile-assets.json"))
                );
                assert_eq!(health_out, Some(PathBuf::from("asset-health.json")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_assets_forecast() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-assets-forecast",
            "runtime-profile-assets.json",
            "--allow-state",
            "active",
            "--desired-concurrency",
            "5",
            "--horizon-seconds",
            "3600",
            "--include-retry",
            "--forecast-out",
            "asset-forecast.json",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityAssetsForecast {
                asset_manifest,
                allow_state,
                desired_concurrency,
                horizon_seconds,
                include_retry,
                forecast_out,
                ..
            } => {
                assert_eq!(asset_manifest, PathBuf::from("runtime-profile-assets.json"));
                assert_eq!(allow_state, vec!["active".to_string()]);
                assert_eq!(desired_concurrency, Some(5));
                assert_eq!(horizon_seconds, Some(3600));
                assert!(include_retry);
                assert_eq!(forecast_out, Some(PathBuf::from("asset-forecast.json")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_assets_gate() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-assets-gate",
            "runtime-profile-assets.json",
            "--desired-concurrency",
            "5",
            "--max-wait-seconds",
            "600",
            "--allow-wait",
            "--allow-state",
            "active",
            "--gate-out",
            "asset-gate.json",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityAssetsGate {
                asset_manifest,
                desired_concurrency,
                max_wait_seconds,
                allow_wait,
                allow_state,
                gate_out,
                ..
            } => {
                assert_eq!(asset_manifest, PathBuf::from("runtime-profile-assets.json"));
                assert_eq!(desired_concurrency, 5);
                assert_eq!(max_wait_seconds, Some(600));
                assert!(allow_wait);
                assert_eq!(allow_state, vec!["active".to_string()]);
                assert_eq!(gate_out, Some(PathBuf::from("asset-gate.json")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_assets_status() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-assets-status",
            "runtime-profile-assets.json",
            "--allow-state",
            "active",
            "--allow-state",
            "repair",
            "--desired-concurrency",
            "5",
            "--include-retry",
            "--include-failed",
            "--include-cancelled",
            "--include-dispatch-leased",
            "--include-runtime-leased",
            "--include-missing-profile-dir",
            "--status-out",
            "status.json",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityAssetsStatus {
                asset_manifest,
                allow_state,
                desired_concurrency,
                include_dispatch_leased,
                include_retry,
                include_failed,
                include_cancelled,
                include_runtime_leased,
                include_missing_profile_dir,
                status_out,
            } => {
                assert_eq!(asset_manifest, PathBuf::from("runtime-profile-assets.json"));
                assert_eq!(
                    allow_state,
                    vec!["active".to_string(), "repair".to_string()]
                );
                assert_eq!(desired_concurrency, Some(5));
                assert!(include_dispatch_leased);
                assert!(include_retry);
                assert!(include_failed);
                assert!(include_cancelled);
                assert!(include_runtime_leased);
                assert!(include_missing_profile_dir);
                assert_eq!(status_out, Some(PathBuf::from("status.json")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_assets_select() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-assets-select",
            "runtime-profile-assets.json",
            "--limit",
            "2",
            "--allow-state",
            "active",
            "--allow-state",
            "repair",
            "--worker",
            "worker-a",
            "--job",
            "publish",
            "--lease-seconds",
            "300",
            "--include-retry",
            "--include-failed",
            "--include-cancelled",
            "--include-dispatch-leased",
            "--include-runtime-leased",
            "--include-missing-profile-dir",
            "--asset-manifest-out",
            "leased-profile-assets.json",
            "--selection-out",
            "selection.json",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityAssetsSelect {
                asset_manifest,
                limit,
                allow_state,
                worker,
                job,
                lease_seconds,
                include_dispatch_leased,
                include_retry,
                include_failed,
                include_cancelled,
                include_runtime_leased,
                include_missing_profile_dir,
                asset_manifest_out,
                selection_out,
            } => {
                assert_eq!(asset_manifest, PathBuf::from("runtime-profile-assets.json"));
                assert_eq!(limit, 2);
                assert_eq!(
                    allow_state,
                    vec!["active".to_string(), "repair".to_string()]
                );
                assert_eq!(worker, Some("worker-a".to_string()));
                assert_eq!(job, Some("publish".to_string()));
                assert_eq!(lease_seconds, 300);
                assert!(include_dispatch_leased);
                assert!(include_retry);
                assert!(include_failed);
                assert!(include_cancelled);
                assert!(include_runtime_leased);
                assert!(include_missing_profile_dir);
                assert_eq!(
                    asset_manifest_out,
                    Some(PathBuf::from("leased-profile-assets.json"))
                );
                assert_eq!(selection_out, Some(PathBuf::from("selection.json")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_assets_release() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-assets-release",
            "leased-profile-assets.json",
            "--status",
            "failed",
            "--worker",
            "worker-a",
            "--job",
            "publish",
            "--lease-id",
            "lease-1",
            "--account-id",
            "acct-a",
            "--profile-id",
            "profile-a",
            "--identity-id",
            "fp-a",
            "--label",
            "acct-a",
            "--cooldown-seconds",
            "600",
            "--next-state",
            "repair",
            "--message",
            "publish failed",
            "--result-json",
            r#"{"error":"captcha"}"#,
            "--asset-manifest-out",
            "released-profile-assets.json",
            "--release-out",
            "release.json",
            "--append-release",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityAssetsRelease {
                asset_manifest,
                status,
                worker,
                job,
                lease_id,
                account_id,
                profile_id,
                identity_id,
                label,
                cooldown_seconds,
                next_state,
                message,
                result_json,
                asset_manifest_out,
                release_out,
                append_release,
            } => {
                assert_eq!(asset_manifest, PathBuf::from("leased-profile-assets.json"));
                assert_eq!(status, "failed");
                assert_eq!(worker, Some("worker-a".to_string()));
                assert_eq!(job, Some("publish".to_string()));
                assert_eq!(lease_id, vec!["lease-1".to_string()]);
                assert_eq!(account_id, vec!["acct-a".to_string()]);
                assert_eq!(profile_id, vec!["profile-a".to_string()]);
                assert_eq!(identity_id, vec!["fp-a".to_string()]);
                assert_eq!(label, vec!["acct-a".to_string()]);
                assert_eq!(cooldown_seconds, Some(600));
                assert_eq!(next_state, Some("repair".to_string()));
                assert_eq!(message, Some("publish failed".to_string()));
                assert_eq!(result_json, Some(r#"{"error":"captcha"}"#.to_string()));
                assert_eq!(
                    asset_manifest_out,
                    Some(PathBuf::from("released-profile-assets.json"))
                );
                assert_eq!(release_out, Some(PathBuf::from("release.json")));
                assert!(append_release);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_assets_sweep() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-assets-sweep",
            "runtime-profile-assets.json",
            "--runtime-grace-seconds",
            "30",
            "--dispatch-grace-seconds",
            "60",
            "--cooldown-grace-seconds",
            "90",
            "--asset-manifest-out",
            "swept-profile-assets.json",
            "--sweep-out",
            "sweep.json",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityAssetsSweep {
                asset_manifest,
                runtime_grace_seconds,
                dispatch_grace_seconds,
                cooldown_grace_seconds,
                asset_manifest_out,
                sweep_out,
            } => {
                assert_eq!(asset_manifest, PathBuf::from("runtime-profile-assets.json"));
                assert_eq!(runtime_grace_seconds, 30);
                assert_eq!(dispatch_grace_seconds, 60);
                assert_eq!(cooldown_grace_seconds, 90);
                assert_eq!(
                    asset_manifest_out,
                    Some(PathBuf::from("swept-profile-assets.json"))
                );
                assert_eq!(sweep_out, Some(PathBuf::from("sweep.json")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_job_run() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-job",
            "run",
            "runtime-profile-assets.json",
            "--policy",
            "identity-policy.json",
            "--job-preset",
            "publish_conservative",
            "--desired-concurrency",
            "2",
            "--limit",
            "2",
            "--worker",
            "worker-a",
            "--job",
            "publish",
            "--lease-seconds",
            "300",
            "--max-wait-seconds",
            "60",
            "--allow-wait",
            "--per-asset",
            "--child-concurrency",
            "2",
            "--runtime-renew-interval-seconds",
            "30",
            "--child-timeout-seconds",
            "120",
            "--child-result-dir",
            "child-results",
            "--max-failed-assets",
            "1",
            "--max-failed-assets-per-reason",
            "2",
            "--allow-state",
            "repair",
            "--include-dispatch-leased",
            "--include-retry",
            "--include-failed",
            "--include-cancelled",
            "--include-runtime-leased",
            "--include-missing-profile-dir",
            "--skip-sweep",
            "--skip-validate",
            "--runtime-grace-seconds",
            "10",
            "--dispatch-grace-seconds",
            "20",
            "--cooldown-grace-seconds",
            "30",
            "--failure-cooldown-seconds",
            "600",
            "--failure-next-state",
            "repair",
            "--asset-manifest-out",
            "leased-profile-assets.json",
            "--sweep-out",
            "sweep.json",
            "--validate-out",
            "validate.json",
            "--gate-out",
            "gate.json",
            "--selection-out",
            "selection.json",
            "--release-out",
            "release.ndjson",
            "--append-release",
            "--runtime-risk-ledger",
            "runtime-risk.ndjson",
            "--runtime-risk-window-seconds",
            "900",
            "--runtime-risk-out",
            "runtime-risk.ndjson",
            "--append-runtime-risk",
            "--explain-out",
            "explain.json",
            "--job-out",
            "job.json",
            "--",
            "python",
            "publish.py",
            "--dry-run",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityJob {
                command:
                    IdentityJobCommand::Run {
                        asset_manifest,
                        policy,
                        job_preset,
                        desired_concurrency,
                        limit,
                        worker,
                        job,
                        lease_seconds,
                        max_wait_seconds,
                        allow_wait,
                        per_asset,
                        child_concurrency,
                        runtime_renew_interval_seconds,
                        child_timeout_seconds,
                        child_result_dir,
                        max_failed_assets,
                        max_failed_assets_per_reason,
                        allow_state,
                        include_dispatch_leased,
                        include_retry,
                        include_failed,
                        include_cancelled,
                        include_runtime_leased,
                        include_missing_profile_dir,
                        skip_sweep,
                        skip_validate,
                        runtime_grace_seconds,
                        dispatch_grace_seconds,
                        cooldown_grace_seconds,
                        failure_cooldown_seconds,
                        failure_next_state,
                        asset_manifest_out,
                        sweep_out,
                        validate_out,
                        gate_out,
                        selection_out,
                        release_out,
                        append_release,
                        runtime_risk_ledger,
                        runtime_risk_window_seconds,
                        runtime_risk_out,
                        append_runtime_risk,
                        explain_out,
                        job_out,
                        command,
                    },
            } => {
                assert_eq!(asset_manifest, PathBuf::from("runtime-profile-assets.json"));
                assert_eq!(policy, Some(PathBuf::from("identity-policy.json")));
                assert_eq!(job_preset, Some("publish_conservative".to_string()));
                assert_eq!(desired_concurrency, Some(2));
                assert_eq!(limit, Some(2));
                assert_eq!(worker, Some("worker-a".to_string()));
                assert_eq!(job, Some("publish".to_string()));
                assert_eq!(lease_seconds, Some(300));
                assert_eq!(max_wait_seconds, Some(60));
                assert!(allow_wait);
                assert!(per_asset);
                assert_eq!(child_concurrency, Some(2));
                assert_eq!(runtime_renew_interval_seconds, Some(30));
                assert_eq!(child_timeout_seconds, Some(120));
                assert_eq!(child_result_dir, Some(PathBuf::from("child-results")));
                assert_eq!(max_failed_assets, Some(1));
                assert_eq!(max_failed_assets_per_reason, Some(2));
                assert_eq!(allow_state, vec!["repair".to_string()]);
                assert!(include_dispatch_leased);
                assert!(include_retry);
                assert!(include_failed);
                assert!(include_cancelled);
                assert!(include_runtime_leased);
                assert!(include_missing_profile_dir);
                assert!(skip_sweep);
                assert!(skip_validate);
                assert_eq!(runtime_grace_seconds, Some(10));
                assert_eq!(dispatch_grace_seconds, Some(20));
                assert_eq!(cooldown_grace_seconds, Some(30));
                assert_eq!(failure_cooldown_seconds, Some(600));
                assert_eq!(failure_next_state, Some("repair".to_string()));
                assert_eq!(
                    asset_manifest_out,
                    Some(PathBuf::from("leased-profile-assets.json"))
                );
                assert_eq!(sweep_out, Some(PathBuf::from("sweep.json")));
                assert_eq!(validate_out, Some(PathBuf::from("validate.json")));
                assert_eq!(gate_out, Some(PathBuf::from("gate.json")));
                assert_eq!(selection_out, Some(PathBuf::from("selection.json")));
                assert_eq!(release_out, Some(PathBuf::from("release.ndjson")));
                assert!(append_release);
                assert_eq!(
                    runtime_risk_ledger,
                    vec![PathBuf::from("runtime-risk.ndjson")]
                );
                assert_eq!(runtime_risk_window_seconds, Some(900));
                assert_eq!(runtime_risk_out, Some(PathBuf::from("runtime-risk.ndjson")));
                assert!(append_runtime_risk);
                assert_eq!(explain_out, Some(PathBuf::from("explain.json")));
                assert_eq!(job_out, Some(PathBuf::from("job.json")));
                assert_eq!(
                    command,
                    vec![
                        "python".to_string(),
                        "publish.py".to_string(),
                        "--dry-run".to_string()
                    ]
                );
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_lifecycle() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-lifecycle",
            "baseline.json",
            "current.json",
            "--max-drift-score",
            "20",
            "--fail-on-high-risk-drift",
            "--fail-on-missing-current",
            "--fail-on-new-current",
            "--match-by",
            "label",
            "--ledger-out",
            "lifecycle-ledger.ndjson",
            "--delta-out",
            "lifecycle-delta.json",
            "--journal-out",
            "lifecycle-journal.ndjson",
            "--append-journal",
            "--state-out-dir",
            "lifecycle-states",
            "--append-ledger",
            "--next-baseline-out",
            "next-baseline.json",
            "--next-baseline-policy",
            "active-only",
            "--actions-out",
            "lifecycle-actions.ndjson",
            "--append-actions",
        ])
        .unwrap();
        assert!(cli.command.identity_gate_is_active());
        match cli.command {
            Command::IdentityLifecycle {
                baseline,
                current,
                max_drift_score,
                fail_on_high_risk_drift,
                fail_on_missing_current,
                fail_on_new_current,
                match_by,
                policy,
                ledger_out,
                delta_out,
                journal_out,
                state_out_dir,
                append_ledger,
                append_journal,
                next_baseline_out,
                next_baseline_policy,
                actions_out,
                append_actions,
            } => {
                assert_eq!(baseline, PathBuf::from("baseline.json"));
                assert_eq!(current, PathBuf::from("current.json"));
                assert_eq!(max_drift_score, Some(20));
                assert!(fail_on_high_risk_drift);
                assert!(fail_on_missing_current);
                assert!(fail_on_new_current);
                assert_eq!(match_by, Some(IdentityDriftMatchMode::Label));
                assert_eq!(policy, None);
                assert_eq!(ledger_out, Some(PathBuf::from("lifecycle-ledger.ndjson")));
                assert_eq!(delta_out, Some(PathBuf::from("lifecycle-delta.json")));
                assert_eq!(journal_out, Some(PathBuf::from("lifecycle-journal.ndjson")));
                assert_eq!(state_out_dir, Some(PathBuf::from("lifecycle-states")));
                assert!(append_ledger);
                assert!(append_journal);
                assert_eq!(next_baseline_out, Some(PathBuf::from("next-baseline.json")));
                assert_eq!(
                    next_baseline_policy,
                    Some(IdentityLifecycleBaselinePolicy::ActiveOnly)
                );
                assert_eq!(actions_out, Some(PathBuf::from("lifecycle-actions.ndjson")));
                assert!(append_actions);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_ledger_dashboard() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-ledger",
            "dashboard",
            "--release-ledger",
            "runtime-release.ndjson",
            "--runtime-risk-ledger",
            "runtime-risk.ndjson",
            "--window-seconds",
            "86400",
            "--job",
            "publish",
            "--worker",
            "worker-a",
            "--reason",
            "rate_limited",
            "--retain-recent",
            "12",
            "--top",
            "6",
            "--checkpoint-in",
            "ledger-checkpoint-in.json",
            "--checkpoint-out",
            "ledger-checkpoint-out.json",
            "--out",
            "ledger-dashboard.json",
            "--html-out",
            "ledger-dashboard.html",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityLedger {
                command:
                    IdentityLedgerCommand::Dashboard {
                        release_ledger,
                        runtime_risk_ledger,
                        window_seconds,
                        job,
                        worker,
                        reason,
                        retain_recent,
                        top,
                        checkpoint_in,
                        checkpoint_out,
                        out,
                        html_out,
                    },
            } => {
                assert_eq!(
                    release_ledger,
                    vec![PathBuf::from("runtime-release.ndjson")]
                );
                assert_eq!(
                    runtime_risk_ledger,
                    vec![PathBuf::from("runtime-risk.ndjson")]
                );
                assert_eq!(window_seconds, Some(86_400));
                assert_eq!(job, Some("publish".to_string()));
                assert_eq!(worker, Some("worker-a".to_string()));
                assert_eq!(reason, Some("rate_limited".to_string()));
                assert_eq!(retain_recent, 12);
                assert_eq!(top, 6);
                assert_eq!(
                    checkpoint_in,
                    Some(PathBuf::from("ledger-checkpoint-in.json"))
                );
                assert_eq!(
                    checkpoint_out,
                    Some(PathBuf::from("ledger-checkpoint-out.json"))
                );
                assert_eq!(out, Some(PathBuf::from("ledger-dashboard.json")));
                assert_eq!(html_out, Some(PathBuf::from("ledger-dashboard.html")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_ledger_compact() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-ledger",
            "compact",
            "--release-ledger",
            "runtime-release.ndjson",
            "--runtime-risk-ledger",
            "runtime-risk.ndjson",
            "--window-seconds",
            "86400",
            "--job",
            "publish",
            "--worker",
            "worker-a",
            "--reason",
            "rate_limited",
            "--retain-recent",
            "12",
            "--top",
            "6",
            "--checkpoint-in",
            "ledger-checkpoint-in.json",
            "--checkpoint-out",
            "ledger-checkpoint-out.json",
            "--out",
            "ledger-compact.json",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityLedger {
                command:
                    IdentityLedgerCommand::Compact {
                        release_ledger,
                        runtime_risk_ledger,
                        window_seconds,
                        job,
                        worker,
                        reason,
                        retain_recent,
                        top,
                        checkpoint_in,
                        checkpoint_out,
                        out,
                    },
            } => {
                assert_eq!(
                    release_ledger,
                    vec![PathBuf::from("runtime-release.ndjson")]
                );
                assert_eq!(
                    runtime_risk_ledger,
                    vec![PathBuf::from("runtime-risk.ndjson")]
                );
                assert_eq!(window_seconds, Some(86_400));
                assert_eq!(job, Some("publish".to_string()));
                assert_eq!(worker, Some("worker-a".to_string()));
                assert_eq!(reason, Some("rate_limited".to_string()));
                assert_eq!(retain_recent, 12);
                assert_eq!(top, 6);
                assert_eq!(
                    checkpoint_in,
                    Some(PathBuf::from("ledger-checkpoint-in.json"))
                );
                assert_eq!(
                    checkpoint_out,
                    Some(PathBuf::from("ledger-checkpoint-out.json"))
                );
                assert_eq!(out, Some(PathBuf::from("ledger-compact.json")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_ledger_query() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-ledger",
            "query",
            "--release-ledger",
            "runtime-release.ndjson",
            "--runtime-risk-ledger",
            "runtime-risk.ndjson",
            "--window-seconds",
            "86400",
            "--job",
            "publish",
            "--worker",
            "worker-a",
            "--reason",
            "rate_limited",
            "--top",
            "5",
            "--out",
            "ledger-query.json",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityLedger {
                command:
                    IdentityLedgerCommand::Query {
                        release_ledger,
                        runtime_risk_ledger,
                        window_seconds,
                        job,
                        worker,
                        reason,
                        top,
                        out,
                    },
            } => {
                assert_eq!(
                    release_ledger,
                    vec![PathBuf::from("runtime-release.ndjson")]
                );
                assert_eq!(
                    runtime_risk_ledger,
                    vec![PathBuf::from("runtime-risk.ndjson")]
                );
                assert_eq!(window_seconds, Some(86_400));
                assert_eq!(job, Some("publish".to_string()));
                assert_eq!(worker, Some("worker-a".to_string()));
                assert_eq!(reason, Some("rate_limited".to_string()));
                assert_eq!(top, 5);
                assert_eq!(out, Some(PathBuf::from("ledger-query.json")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_identity_ledger_explain() {
        let cli = Cli::try_parse_from([
            "drs",
            "identity-ledger",
            "explain",
            "--release-ledger",
            "runtime-release.ndjson",
            "--runtime-risk-ledger",
            "runtime-risk.ndjson",
            "--window-seconds",
            "86400",
            "--job",
            "publish",
            "--worker",
            "worker-a",
            "--reason",
            "rate_limited",
            "--account-id",
            "acct-a",
            "--profile-id",
            "profile-a",
            "--identity-id",
            "fp-a",
            "--label",
            "acct-a",
            "--profile-dir",
            "/profiles/acct-a",
            "--lease-id",
            "lease-a",
            "--evidence-limit",
            "7",
            "--out",
            "ledger-explain.json",
        ])
        .unwrap();

        match cli.command {
            Command::IdentityLedger {
                command:
                    IdentityLedgerCommand::Explain {
                        release_ledger,
                        runtime_risk_ledger,
                        window_seconds,
                        job,
                        worker,
                        reason,
                        account_id,
                        profile_id,
                        identity_id,
                        label,
                        profile_dir,
                        lease_id,
                        evidence_limit,
                        out,
                    },
            } => {
                assert_eq!(
                    release_ledger,
                    vec![PathBuf::from("runtime-release.ndjson")]
                );
                assert_eq!(
                    runtime_risk_ledger,
                    vec![PathBuf::from("runtime-risk.ndjson")]
                );
                assert_eq!(window_seconds, Some(86_400));
                assert_eq!(job, Some("publish".to_string()));
                assert_eq!(worker, Some("worker-a".to_string()));
                assert_eq!(reason, Some("rate_limited".to_string()));
                assert_eq!(account_id, Some("acct-a".to_string()));
                assert_eq!(profile_id, Some("profile-a".to_string()));
                assert_eq!(identity_id, Some("fp-a".to_string()));
                assert_eq!(label, Some("acct-a".to_string()));
                assert_eq!(profile_dir, Some(PathBuf::from("/profiles/acct-a")));
                assert_eq!(lease_id, Some("lease-a".to_string()));
                assert_eq!(evidence_limit, 7);
                assert_eq!(out, Some(PathBuf::from("ledger-explain.json")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
