use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::io::{ErrorKind, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use drission::fingerprint::{
    FingerprintSnapshot, IdentityAdmissionAction, IdentityCapacityPlan,
    IdentityDriftRemediationPlan, IdentityDriftRemediationTarget, IdentityDriftReport,
    IdentityDriftSeverity, IdentityDriftSignal, IdentityFixPriority, IdentityPoolRemediationPlan,
    IdentityPoolRemediationTarget, IdentityPoolReport, IdentityReport, LinkabilityReport,
    LinkabilitySignal,
};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::process::Command as TokioCommand;
use tokio::time::{Duration, Instant, sleep};

use crate::protocol::{
    IdentityDriftMatchMode, IdentityGate, IdentityGateReport, IdentityLifecycleBaselinePolicy,
    JsonResponse,
};

#[derive(Debug, Clone)]
pub struct ResolvedDriftPolicy {
    pub max_drift_score: Option<u8>,
    pub fail_on_high_risk_drift: bool,
    pub match_by: IdentityDriftMatchMode,
}

#[derive(Debug, Clone)]
pub struct ResolvedLifecyclePolicy {
    pub max_drift_score: Option<u8>,
    pub fail_on_high_risk_drift: bool,
    pub fail_on_missing_current: bool,
    pub fail_on_new_current: bool,
    pub match_by: IdentityDriftMatchMode,
    pub next_baseline_policy: IdentityLifecycleBaselinePolicy,
}

#[derive(Debug, Clone)]
pub struct ResolvedHealthPolicy {
    pub window_seconds: Option<u64>,
    pub repair_threshold: usize,
    pub quarantine_threshold: usize,
    pub cooldown_seconds: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IdentityJobFailureReasonRule {
    #[serde(rename = "cooldownSeconds", alias = "cooldown_seconds")]
    pub cooldown_seconds: Option<u64>,
    #[serde(rename = "nextState", alias = "next_state")]
    pub next_state: Option<String>,
    #[serde(
        rename = "recommendedAction",
        alias = "recommended_action",
        alias = "runtimeRiskAction",
        alias = "runtime_risk_action"
    )]
    pub recommended_action: Option<String>,
    #[serde(rename = "runtimeRiskSeverity", alias = "runtime_risk_severity")]
    pub runtime_risk_severity: Option<String>,
    #[serde(rename = "nextSuggestedLimit", alias = "next_suggested_limit")]
    pub next_suggested_limit: Option<usize>,
    #[serde(
        rename = "nextSuggestedDesiredConcurrency",
        alias = "next_suggested_desired_concurrency"
    )]
    pub next_suggested_desired_concurrency: Option<usize>,
    #[serde(rename = "runtimeRiskMessage", alias = "runtime_risk_message")]
    pub runtime_risk_message: Option<String>,
    #[serde(
        rename = "runtimeRiskCooldownSeconds",
        alias = "runtime_risk_cooldown_seconds",
        alias = "suppressSeconds",
        alias = "suppress_seconds"
    )]
    pub runtime_risk_cooldown_seconds: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct IdentityJobRunOptions {
    pub asset_manifest: PathBuf,
    pub policy: Option<PathBuf>,
    pub job_preset: Option<String>,
    pub desired_concurrency: Option<usize>,
    pub limit: Option<usize>,
    pub worker: Option<String>,
    pub job: Option<String>,
    pub lease_seconds: Option<u64>,
    pub max_wait_seconds: Option<u64>,
    pub allow_wait: bool,
    pub per_asset: bool,
    pub child_concurrency: Option<usize>,
    pub runtime_renew_interval_seconds: Option<u64>,
    pub child_timeout_seconds: Option<u64>,
    pub child_result_dir: Option<PathBuf>,
    pub max_failed_assets: Option<usize>,
    pub max_failed_assets_per_reason: Option<usize>,
    pub allow_states: Vec<String>,
    pub include_dispatch_leased: bool,
    pub include_retry: bool,
    pub include_failed: bool,
    pub include_cancelled: bool,
    pub include_runtime_leased: bool,
    pub include_missing_profile_dir: bool,
    pub skip_sweep: bool,
    pub skip_validate: bool,
    pub runtime_grace_seconds: Option<u64>,
    pub dispatch_grace_seconds: Option<u64>,
    pub cooldown_grace_seconds: Option<u64>,
    pub failure_cooldown_seconds: Option<u64>,
    pub failure_next_state: Option<String>,
    pub failure_reason_rules: BTreeMap<String, IdentityJobFailureReasonRule>,
    pub asset_manifest_out: Option<PathBuf>,
    pub sweep_out: Option<PathBuf>,
    pub validate_out: Option<PathBuf>,
    pub gate_out: Option<PathBuf>,
    pub selection_out: Option<PathBuf>,
    pub release_out: Option<PathBuf>,
    pub append_release: bool,
    pub runtime_risk_ledgers: Vec<PathBuf>,
    pub runtime_risk_window_seconds: Option<u64>,
    pub runtime_risk_out: Option<PathBuf>,
    pub append_runtime_risk: bool,
    pub explain_out: Option<PathBuf>,
    pub job_out: Option<PathBuf>,
    pub command: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LoadedIdentityPolicy {
    path: PathBuf,
    spec: IdentityPolicySpec,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct IdentityPolicySpec {
    #[serde(flatten)]
    root: IdentityPolicyRoot,
    gate: Option<IdentityGatePolicy>,
    drift: Option<IdentityDriftPolicy>,
    lifecycle: Option<IdentityLifecyclePolicy>,
    health: Option<IdentityHealthPolicy>,
    job: Option<IdentityJobPolicy>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct IdentityPolicyRoot {
    #[serde(flatten)]
    gate: IdentityGatePolicy,
    #[serde(flatten)]
    drift: IdentityDriftPolicy,
    #[serde(flatten)]
    health: IdentityHealthPolicy,
    #[serde(flatten)]
    job: IdentityJobPolicy,
    #[serde(rename = "failOnMissingCurrent", alias = "fail_on_missing_current")]
    fail_on_missing_current: Option<bool>,
    #[serde(rename = "failOnNewCurrent", alias = "fail_on_new_current")]
    fail_on_new_current: Option<bool>,
    #[serde(rename = "nextBaselinePolicy", alias = "next_baseline_policy")]
    next_baseline_policy: Option<IdentityLifecycleBaselinePolicy>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct IdentityGatePolicy {
    #[serde(rename = "gatePreset", alias = "gate_preset")]
    gate_preset: Option<crate::protocol::IdentityGatePreset>,
    #[serde(rename = "minScore", alias = "min_score")]
    min_score: Option<u8>,
    #[serde(rename = "maxLinkability", alias = "max_linkability")]
    max_linkability: Option<u8>,
    #[serde(rename = "maxConcentrationRatio", alias = "max_concentration_ratio")]
    max_concentration_ratio: Option<f64>,
    #[serde(rename = "maxConcentratedSignals", alias = "max_concentrated_signals")]
    max_concentrated_signals: Option<usize>,
    #[serde(rename = "minEntropyScore", alias = "min_entropy_score")]
    min_entropy_score: Option<u8>,
    #[serde(
        rename = "minEffectiveIdentities",
        alias = "min_effective_identities",
        alias = "minEffectiveIdentityCount",
        alias = "min_effective_identity_count"
    )]
    min_effective_identity_count: Option<f64>,
    #[serde(
        rename = "maxNominalToEffectiveRatio",
        alias = "max_nominal_to_effective_ratio"
    )]
    max_nominal_to_effective_ratio: Option<f64>,
    #[serde(rename = "failOnHighRisk", alias = "fail_on_high_risk")]
    fail_on_high_risk: Option<bool>,
    #[serde(rename = "failOnRiskyPairs", alias = "fail_on_risky_pairs")]
    fail_on_risky_pairs: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct IdentityDriftPolicy {
    #[serde(rename = "maxDriftScore", alias = "max_drift_score")]
    max_drift_score: Option<u8>,
    #[serde(rename = "failOnHighRiskDrift", alias = "fail_on_high_risk_drift")]
    fail_on_high_risk_drift: Option<bool>,
    #[serde(rename = "matchBy", alias = "match_by")]
    match_by: Option<IdentityDriftMatchMode>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct IdentityLifecyclePolicy {
    #[serde(flatten)]
    drift: IdentityDriftPolicy,
    #[serde(rename = "failOnMissingCurrent", alias = "fail_on_missing_current")]
    fail_on_missing_current: Option<bool>,
    #[serde(rename = "failOnNewCurrent", alias = "fail_on_new_current")]
    fail_on_new_current: Option<bool>,
    #[serde(rename = "nextBaselinePolicy", alias = "next_baseline_policy")]
    next_baseline_policy: Option<IdentityLifecycleBaselinePolicy>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct IdentityHealthPolicy {
    #[serde(
        rename = "windowSeconds",
        alias = "window_seconds",
        alias = "healthWindowSeconds",
        alias = "health_window_seconds"
    )]
    window_seconds: Option<u64>,
    #[serde(
        rename = "repairThreshold",
        alias = "repair_threshold",
        alias = "healthRepairThreshold",
        alias = "health_repair_threshold"
    )]
    repair_threshold: Option<usize>,
    #[serde(
        rename = "quarantineThreshold",
        alias = "quarantine_threshold",
        alias = "healthQuarantineThreshold",
        alias = "health_quarantine_threshold"
    )]
    quarantine_threshold: Option<usize>,
    #[serde(
        rename = "cooldownSeconds",
        alias = "cooldown_seconds",
        alias = "healthCooldownSeconds",
        alias = "health_cooldown_seconds"
    )]
    cooldown_seconds: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct IdentityJobPolicy {
    #[serde(
        rename = "preset",
        alias = "jobPreset",
        alias = "job_preset",
        alias = "policyPreset",
        alias = "policy_preset"
    )]
    preset: Option<String>,
    #[serde(rename = "desiredConcurrency", alias = "desired_concurrency")]
    desired_concurrency: Option<usize>,
    limit: Option<usize>,
    #[serde(rename = "leaseSeconds", alias = "lease_seconds")]
    lease_seconds: Option<u64>,
    #[serde(rename = "maxWaitSeconds", alias = "max_wait_seconds")]
    max_wait_seconds: Option<u64>,
    #[serde(rename = "allowWait", alias = "allow_wait")]
    allow_wait: Option<bool>,
    #[serde(rename = "perAsset", alias = "per_asset")]
    per_asset: Option<bool>,
    #[serde(rename = "childConcurrency", alias = "child_concurrency")]
    child_concurrency: Option<usize>,
    #[serde(
        rename = "runtimeRenewIntervalSeconds",
        alias = "runtime_renew_interval_seconds",
        alias = "renewIntervalSeconds",
        alias = "renew_interval_seconds"
    )]
    runtime_renew_interval_seconds: Option<u64>,
    #[serde(
        rename = "childTimeoutSeconds",
        alias = "child_timeout_seconds",
        alias = "timeoutSeconds",
        alias = "timeout_seconds"
    )]
    child_timeout_seconds: Option<u64>,
    #[serde(rename = "childResultDir", alias = "child_result_dir")]
    child_result_dir: Option<PathBuf>,
    #[serde(rename = "maxFailedAssets", alias = "max_failed_assets")]
    max_failed_assets: Option<usize>,
    #[serde(
        rename = "maxFailedAssetsPerReason",
        alias = "max_failed_assets_per_reason"
    )]
    max_failed_assets_per_reason: Option<usize>,
    #[serde(rename = "allowState", alias = "allow_state")]
    allow_states: Option<Vec<String>>,
    #[serde(rename = "includeDispatchLeased", alias = "include_dispatch_leased")]
    include_dispatch_leased: Option<bool>,
    #[serde(rename = "includeRetry", alias = "include_retry")]
    include_retry: Option<bool>,
    #[serde(rename = "includeFailed", alias = "include_failed")]
    include_failed: Option<bool>,
    #[serde(rename = "includeCancelled", alias = "include_cancelled")]
    include_cancelled: Option<bool>,
    #[serde(rename = "includeRuntimeLeased", alias = "include_runtime_leased")]
    include_runtime_leased: Option<bool>,
    #[serde(
        rename = "includeMissingProfileDir",
        alias = "include_missing_profile_dir"
    )]
    include_missing_profile_dir: Option<bool>,
    #[serde(rename = "skipSweep", alias = "skip_sweep")]
    skip_sweep: Option<bool>,
    #[serde(rename = "skipValidate", alias = "skip_validate")]
    skip_validate: Option<bool>,
    #[serde(rename = "runtimeGraceSeconds", alias = "runtime_grace_seconds")]
    runtime_grace_seconds: Option<u64>,
    #[serde(rename = "dispatchGraceSeconds", alias = "dispatch_grace_seconds")]
    dispatch_grace_seconds: Option<u64>,
    #[serde(rename = "cooldownGraceSeconds", alias = "cooldown_grace_seconds")]
    cooldown_grace_seconds: Option<u64>,
    #[serde(rename = "failureCooldownSeconds", alias = "failure_cooldown_seconds")]
    failure_cooldown_seconds: Option<u64>,
    #[serde(rename = "failureNextState", alias = "failure_next_state")]
    failure_next_state: Option<String>,
    #[serde(
        rename = "failureReasonRules",
        alias = "failure_reason_rules",
        alias = "failureReasons",
        alias = "failure_reasons"
    )]
    failure_reason_rules: BTreeMap<String, IdentityJobFailureReasonRule>,
    #[serde(rename = "runtimeRiskLedgers", alias = "runtime_risk_ledgers")]
    runtime_risk_ledgers: Option<Vec<PathBuf>>,
    #[serde(
        rename = "runtimeRiskWindowSeconds",
        alias = "runtime_risk_window_seconds"
    )]
    runtime_risk_window_seconds: Option<u64>,
    #[serde(rename = "runtimeRiskOut", alias = "runtime_risk_out")]
    runtime_risk_out: Option<PathBuf>,
    #[serde(rename = "appendRuntimeRisk", alias = "append_runtime_risk")]
    append_runtime_risk: Option<bool>,
    #[serde(rename = "explainOut", alias = "explain_out")]
    explain_out: Option<PathBuf>,
}

pub async fn load_identity_policy(path: Option<&Path>) -> Result<Option<LoadedIdentityPolicy>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let text = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read identity policy {}", path.display()))?;
    let spec = parse_identity_policy(&text)
        .with_context(|| format!("failed to parse identity policy {}", path.display()))?;
    Ok(Some(LoadedIdentityPolicy {
        path: path.to_path_buf(),
        spec,
    }))
}

pub fn attach_identity_policy(response: &mut JsonResponse, policy: Option<&LoadedIdentityPolicy>) {
    let Some(policy) = policy else {
        return;
    };
    if let Some(data) = response.data.as_mut() {
        data["policy"] = policy.summary();
    }
}

impl LoadedIdentityPolicy {
    pub fn merge_gate(&self, cli: IdentityGate) -> IdentityGate {
        let mut gate = IdentityGate::default();
        self.spec.root.gate.apply_to(&mut gate);
        if let Some(section) = &self.spec.gate {
            section.apply_to(&mut gate);
        }
        merge_cli_gate(&mut gate, &cli);
        gate
    }

    pub fn merge_drift(
        &self,
        cli_max_drift_score: Option<u8>,
        cli_fail_on_high_risk_drift: bool,
        cli_match_by: Option<IdentityDriftMatchMode>,
    ) -> ResolvedDriftPolicy {
        let mut policy = self.spec.root.drift.clone();
        if let Some(section) = &self.spec.drift {
            section.apply_to(&mut policy);
        }
        ResolvedDriftPolicy {
            max_drift_score: cli_max_drift_score.or(policy.max_drift_score),
            fail_on_high_risk_drift: cli_fail_on_high_risk_drift
                || policy.fail_on_high_risk_drift.unwrap_or(false),
            match_by: cli_match_by
                .or(policy.match_by)
                .unwrap_or(IdentityDriftMatchMode::Auto),
        }
    }

    pub fn merge_lifecycle(
        &self,
        cli_max_drift_score: Option<u8>,
        cli_fail_on_high_risk_drift: bool,
        cli_fail_on_missing_current: bool,
        cli_fail_on_new_current: bool,
        cli_match_by: Option<IdentityDriftMatchMode>,
        cli_next_baseline_policy: Option<IdentityLifecycleBaselinePolicy>,
    ) -> ResolvedLifecyclePolicy {
        let mut drift = self.spec.root.drift.clone();
        let mut fail_on_missing_current = self.spec.root.fail_on_missing_current;
        let mut fail_on_new_current = self.spec.root.fail_on_new_current;
        let mut next_baseline_policy = self.spec.root.next_baseline_policy;
        if let Some(section) = &self.spec.drift {
            section.apply_to(&mut drift);
        }
        if let Some(section) = &self.spec.lifecycle {
            section.drift.apply_to(&mut drift);
            if section.fail_on_missing_current.is_some() {
                fail_on_missing_current = section.fail_on_missing_current;
            }
            if section.fail_on_new_current.is_some() {
                fail_on_new_current = section.fail_on_new_current;
            }
            if section.next_baseline_policy.is_some() {
                next_baseline_policy = section.next_baseline_policy;
            }
        }

        ResolvedLifecyclePolicy {
            max_drift_score: cli_max_drift_score.or(drift.max_drift_score),
            fail_on_high_risk_drift: cli_fail_on_high_risk_drift
                || drift.fail_on_high_risk_drift.unwrap_or(false),
            fail_on_missing_current: cli_fail_on_missing_current
                || fail_on_missing_current.unwrap_or(false),
            fail_on_new_current: cli_fail_on_new_current || fail_on_new_current.unwrap_or(false),
            match_by: cli_match_by
                .or(drift.match_by)
                .unwrap_or(IdentityDriftMatchMode::Auto),
            next_baseline_policy: cli_next_baseline_policy
                .or(next_baseline_policy)
                .unwrap_or_default(),
        }
    }

    pub fn merge_health(
        &self,
        cli_window_seconds: Option<u64>,
        cli_repair_threshold: Option<usize>,
        cli_quarantine_threshold: Option<usize>,
        cli_cooldown_seconds: Option<u64>,
    ) -> ResolvedHealthPolicy {
        let mut policy = self.spec.root.health.clone();
        if let Some(section) = &self.spec.health {
            section.apply_to(&mut policy);
        }
        ResolvedHealthPolicy {
            window_seconds: cli_window_seconds.or(policy.window_seconds),
            repair_threshold: cli_repair_threshold
                .or(policy.repair_threshold)
                .unwrap_or(3),
            quarantine_threshold: cli_quarantine_threshold
                .or(policy.quarantine_threshold)
                .unwrap_or(5),
            cooldown_seconds: cli_cooldown_seconds
                .or(policy.cooldown_seconds)
                .unwrap_or(900),
        }
    }

    pub fn merge_job_run(&self, cli: IdentityJobRunOptions) -> IdentityJobRunOptions {
        let mut policy = self.spec.root.job.clone();
        apply_identity_job_policy_preset_defaults(&mut policy);
        if let Some(section) = &self.spec.job {
            let mut section = section.clone();
            apply_identity_job_policy_preset_defaults(&mut section);
            section.apply_to(&mut policy);
        }
        apply_identity_job_cli_preset(&mut policy, cli.job_preset.as_deref());
        merge_identity_job_policy_with_cli(policy, cli)
    }

    fn summary(&self) -> Value {
        json!({
            "path": self.path.display().to_string(),
            "format": "json",
            "rules": self.spec,
        })
    }
}

fn merge_identity_job_without_loaded_policy(cli: IdentityJobRunOptions) -> IdentityJobRunOptions {
    let mut policy = IdentityJobPolicy::default();
    apply_identity_job_cli_preset(&mut policy, cli.job_preset.as_deref());
    merge_identity_job_policy_with_cli(policy, cli)
}

fn apply_identity_job_cli_preset(policy: &mut IdentityJobPolicy, cli_preset: Option<&str>) {
    if let Some(cli_preset) = cli_preset {
        if let Some(preset) = identity_job_preset_policy(cli_preset) {
            preset.apply_to(policy);
        } else {
            policy.preset = Some(cli_preset.to_string());
        }
    }
}

fn merge_identity_job_policy_with_cli(
    policy: IdentityJobPolicy,
    cli: IdentityJobRunOptions,
) -> IdentityJobRunOptions {
    let mut failure_reason_rules =
        normalize_identity_job_failure_reason_rules(&policy.failure_reason_rules);
    failure_reason_rules.extend(normalize_identity_job_failure_reason_rules(
        &cli.failure_reason_rules,
    ));
    let job_preset = cli
        .job_preset
        .as_deref()
        .and_then(identity_job_preset_canonical)
        .map(str::to_string)
        .or(cli.job_preset)
        .or_else(|| {
            policy
                .preset
                .as_deref()
                .and_then(identity_job_preset_canonical)
                .map(str::to_string)
        })
        .or(policy.preset);
    IdentityJobRunOptions {
        asset_manifest: cli.asset_manifest,
        policy: cli.policy,
        job_preset,
        desired_concurrency: cli.desired_concurrency.or(policy.desired_concurrency),
        limit: cli.limit.or(policy.limit),
        worker: cli.worker,
        job: cli.job,
        lease_seconds: cli.lease_seconds.or(policy.lease_seconds),
        max_wait_seconds: cli.max_wait_seconds.or(policy.max_wait_seconds),
        allow_wait: cli.allow_wait || policy.allow_wait.unwrap_or(false),
        per_asset: cli.per_asset || policy.per_asset.unwrap_or(false),
        child_concurrency: cli.child_concurrency.or(policy.child_concurrency),
        runtime_renew_interval_seconds: cli
            .runtime_renew_interval_seconds
            .or(policy.runtime_renew_interval_seconds),
        child_timeout_seconds: cli.child_timeout_seconds.or(policy.child_timeout_seconds),
        child_result_dir: cli.child_result_dir.or(policy.child_result_dir),
        max_failed_assets: cli.max_failed_assets.or(policy.max_failed_assets),
        max_failed_assets_per_reason: cli
            .max_failed_assets_per_reason
            .or(policy.max_failed_assets_per_reason),
        allow_states: if cli.allow_states.is_empty() {
            policy.allow_states.unwrap_or_default()
        } else {
            cli.allow_states
        },
        include_dispatch_leased: cli.include_dispatch_leased
            || policy.include_dispatch_leased.unwrap_or(false),
        include_retry: cli.include_retry || policy.include_retry.unwrap_or(false),
        include_failed: cli.include_failed || policy.include_failed.unwrap_or(false),
        include_cancelled: cli.include_cancelled || policy.include_cancelled.unwrap_or(false),
        include_runtime_leased: cli.include_runtime_leased
            || policy.include_runtime_leased.unwrap_or(false),
        include_missing_profile_dir: cli.include_missing_profile_dir
            || policy.include_missing_profile_dir.unwrap_or(false),
        skip_sweep: cli.skip_sweep || policy.skip_sweep.unwrap_or(false),
        skip_validate: cli.skip_validate || policy.skip_validate.unwrap_or(false),
        runtime_grace_seconds: cli.runtime_grace_seconds.or(policy.runtime_grace_seconds),
        dispatch_grace_seconds: cli.dispatch_grace_seconds.or(policy.dispatch_grace_seconds),
        cooldown_grace_seconds: cli.cooldown_grace_seconds.or(policy.cooldown_grace_seconds),
        failure_cooldown_seconds: cli
            .failure_cooldown_seconds
            .or(policy.failure_cooldown_seconds),
        failure_next_state: cli.failure_next_state.or(policy.failure_next_state),
        failure_reason_rules,
        asset_manifest_out: cli.asset_manifest_out,
        sweep_out: cli.sweep_out,
        validate_out: cli.validate_out,
        gate_out: cli.gate_out,
        selection_out: cli.selection_out,
        release_out: cli.release_out,
        append_release: cli.append_release,
        runtime_risk_ledgers: if cli.runtime_risk_ledgers.is_empty() {
            policy.runtime_risk_ledgers.unwrap_or_default()
        } else {
            cli.runtime_risk_ledgers
        },
        runtime_risk_window_seconds: cli
            .runtime_risk_window_seconds
            .or(policy.runtime_risk_window_seconds),
        runtime_risk_out: cli.runtime_risk_out.or(policy.runtime_risk_out),
        append_runtime_risk: cli.append_runtime_risk || policy.append_runtime_risk.unwrap_or(false),
        explain_out: cli.explain_out.or(policy.explain_out),
        job_out: cli.job_out,
        command: cli.command,
    }
}

fn apply_identity_job_policy_preset_defaults(policy: &mut IdentityJobPolicy) {
    let Some(preset_name) = policy.preset.clone() else {
        return;
    };
    let Some(mut defaults) = identity_job_preset_policy(&preset_name) else {
        return;
    };
    policy.apply_to(&mut defaults);
    *policy = defaults;
}

fn identity_job_preset_canonical(name: &str) -> Option<&'static str> {
    match normalize_identity_job_preset_name(name).as_str() {
        "publish" | "publish_conservative" | "conservative_publish" => Some("publish_conservative"),
        "login" | "login_sensitive" | "sensitive_login" => Some("login_sensitive"),
        "scrape" | "scrape_aggressive" | "aggressive_scrape" => Some("scrape_aggressive"),
        _ => None,
    }
}

fn normalize_identity_job_preset_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace(['-', ' '], "_")
}

fn identity_job_preset_policy(name: &str) -> Option<IdentityJobPolicy> {
    let canonical = identity_job_preset_canonical(name)?;
    let mut policy = IdentityJobPolicy {
        preset: Some(canonical.to_string()),
        ..Default::default()
    };
    match canonical {
        "publish_conservative" => {
            policy.per_asset = Some(true);
            policy.child_concurrency = Some(1);
            policy.runtime_renew_interval_seconds = Some(300);
            policy.child_timeout_seconds = Some(1800);
            policy.max_failed_assets = Some(1);
            policy.max_failed_assets_per_reason = Some(2);
            policy.allow_states = Some(vec!["active".to_string()]);
            policy.failure_cooldown_seconds = Some(600);
            policy.failure_next_state = Some("repair".to_string());
            policy.runtime_risk_window_seconds = Some(900);
            policy.failure_reason_rules = BTreeMap::from([
                (
                    "rate_limited".to_string(),
                    identity_job_preset_reason_rule(
                        Some(900),
                        Some("repair"),
                        Some("pause_failure_reason"),
                        Some("critical"),
                        Some(0),
                        Some(0),
                        Some("pause rate-limited publish jobs"),
                        Some(1800),
                    ),
                ),
                (
                    "risk_control".to_string(),
                    identity_job_preset_reason_rule(
                        Some(3600),
                        Some("quarantine"),
                        Some("pause_pool"),
                        Some("critical"),
                        Some(0),
                        Some(0),
                        Some("pause publish jobs after platform risk control"),
                        Some(3600),
                    ),
                ),
                (
                    "captcha".to_string(),
                    identity_job_preset_reason_rule(
                        Some(900),
                        Some("repair"),
                        Some("reduce_concurrency"),
                        Some("high"),
                        Some(1),
                        Some(1),
                        Some("reduce publish concurrency after captcha pressure"),
                        Some(900),
                    ),
                ),
                (
                    "login_required".to_string(),
                    identity_job_preset_reason_rule(
                        Some(1800),
                        Some("repair"),
                        Some("pause_failure_reason"),
                        Some("critical"),
                        Some(0),
                        Some(0),
                        Some("pause publish jobs for accounts requiring login"),
                        Some(1800),
                    ),
                ),
            ]);
        }
        "login_sensitive" => {
            policy.per_asset = Some(true);
            policy.child_concurrency = Some(1);
            policy.runtime_renew_interval_seconds = Some(120);
            policy.child_timeout_seconds = Some(900);
            policy.max_failed_assets = Some(1);
            policy.max_failed_assets_per_reason = Some(1);
            policy.allow_states = Some(vec!["active".to_string()]);
            policy.failure_cooldown_seconds = Some(1800);
            policy.failure_next_state = Some("repair".to_string());
            policy.runtime_risk_window_seconds = Some(1800);
            policy.failure_reason_rules = BTreeMap::from([
                (
                    "login_required".to_string(),
                    identity_job_preset_reason_rule(
                        Some(3600),
                        Some("repair"),
                        Some("pause_pool"),
                        Some("critical"),
                        Some(0),
                        Some(0),
                        Some("pause login-sensitive jobs after login_required"),
                        Some(3600),
                    ),
                ),
                (
                    "risk_control".to_string(),
                    identity_job_preset_reason_rule(
                        Some(7200),
                        Some("quarantine"),
                        Some("pause_pool"),
                        Some("critical"),
                        Some(0),
                        Some(0),
                        Some("pause login-sensitive jobs after risk control"),
                        Some(7200),
                    ),
                ),
                (
                    "captcha".to_string(),
                    identity_job_preset_reason_rule(
                        Some(1800),
                        Some("repair"),
                        Some("pause_failure_reason"),
                        Some("high"),
                        Some(0),
                        Some(0),
                        Some("pause login-sensitive jobs after captcha pressure"),
                        Some(1800),
                    ),
                ),
                (
                    "password_or_2fa_required".to_string(),
                    identity_job_preset_reason_rule(
                        Some(7200),
                        Some("repair"),
                        Some("pause_pool"),
                        Some("critical"),
                        Some(0),
                        Some(0),
                        Some("pause login jobs requiring manual credential or 2fa handling"),
                        Some(7200),
                    ),
                ),
            ]);
        }
        "scrape_aggressive" => {
            policy.per_asset = Some(true);
            policy.child_concurrency = Some(5);
            policy.runtime_renew_interval_seconds = Some(300);
            policy.child_timeout_seconds = Some(1200);
            policy.max_failed_assets = Some(5);
            policy.max_failed_assets_per_reason = Some(3);
            policy.allow_states = Some(vec!["active".to_string()]);
            policy.failure_cooldown_seconds = Some(300);
            policy.runtime_risk_window_seconds = Some(600);
            policy.failure_reason_rules = BTreeMap::from([
                (
                    "rate_limited".to_string(),
                    identity_job_preset_reason_rule(
                        Some(300),
                        Some("active"),
                        Some("reduce_concurrency"),
                        Some("high"),
                        Some(1),
                        Some(1),
                        Some("reduce scrape concurrency after rate limit"),
                        Some(600),
                    ),
                ),
                (
                    "network_error".to_string(),
                    identity_job_preset_reason_rule(
                        Some(60),
                        Some("active"),
                        Some("continue_current"),
                        Some("elevated"),
                        None,
                        None,
                        Some("short cooldown after transient network error"),
                        None,
                    ),
                ),
                (
                    "timeout".to_string(),
                    identity_job_preset_reason_rule(
                        Some(120),
                        Some("active"),
                        Some("reduce_concurrency"),
                        Some("elevated"),
                        Some(2),
                        Some(2),
                        Some("reduce scrape concurrency after timeouts"),
                        Some(300),
                    ),
                ),
            ]);
        }
        _ => return None,
    }
    Some(policy)
}

#[allow(clippy::too_many_arguments)]
fn identity_job_preset_reason_rule(
    cooldown_seconds: Option<u64>,
    next_state: Option<&str>,
    recommended_action: Option<&str>,
    runtime_risk_severity: Option<&str>,
    next_suggested_limit: Option<usize>,
    next_suggested_desired_concurrency: Option<usize>,
    runtime_risk_message: Option<&str>,
    runtime_risk_cooldown_seconds: Option<u64>,
) -> IdentityJobFailureReasonRule {
    IdentityJobFailureReasonRule {
        cooldown_seconds,
        next_state: next_state.map(str::to_string),
        recommended_action: recommended_action.map(str::to_string),
        runtime_risk_severity: runtime_risk_severity.map(str::to_string),
        next_suggested_limit,
        next_suggested_desired_concurrency,
        runtime_risk_message: runtime_risk_message.map(str::to_string),
        runtime_risk_cooldown_seconds,
    }
}

impl IdentityGatePolicy {
    fn apply_to(&self, gate: &mut IdentityGate) {
        if self.gate_preset.is_some() {
            gate.preset = self.gate_preset;
        }
        if self.min_score.is_some() {
            gate.min_score = self.min_score;
        }
        if self.max_linkability.is_some() {
            gate.max_linkability = self.max_linkability;
        }
        if self.max_concentration_ratio.is_some() {
            gate.max_concentration_ratio = self.max_concentration_ratio;
        }
        if self.max_concentrated_signals.is_some() {
            gate.max_concentrated_signals = self.max_concentrated_signals;
        }
        if self.min_entropy_score.is_some() {
            gate.min_entropy_score = self.min_entropy_score;
        }
        if self.min_effective_identity_count.is_some() {
            gate.min_effective_identity_count = self.min_effective_identity_count;
        }
        if self.max_nominal_to_effective_ratio.is_some() {
            gate.max_nominal_to_effective_ratio = self.max_nominal_to_effective_ratio;
        }
        if let Some(fail_on_high_risk) = self.fail_on_high_risk {
            gate.fail_on_high_risk = fail_on_high_risk;
        }
        if let Some(fail_on_risky_pairs) = self.fail_on_risky_pairs {
            gate.fail_on_risky_pairs = fail_on_risky_pairs;
        }
    }
}

impl IdentityDriftPolicy {
    fn apply_to(&self, target: &mut IdentityDriftPolicy) {
        if self.max_drift_score.is_some() {
            target.max_drift_score = self.max_drift_score;
        }
        if self.fail_on_high_risk_drift.is_some() {
            target.fail_on_high_risk_drift = self.fail_on_high_risk_drift;
        }
        if self.match_by.is_some() {
            target.match_by = self.match_by;
        }
    }
}

impl IdentityHealthPolicy {
    fn apply_to(&self, target: &mut IdentityHealthPolicy) {
        if self.window_seconds.is_some() {
            target.window_seconds = self.window_seconds;
        }
        if self.repair_threshold.is_some() {
            target.repair_threshold = self.repair_threshold;
        }
        if self.quarantine_threshold.is_some() {
            target.quarantine_threshold = self.quarantine_threshold;
        }
        if self.cooldown_seconds.is_some() {
            target.cooldown_seconds = self.cooldown_seconds;
        }
    }
}

impl IdentityJobPolicy {
    fn apply_to(&self, target: &mut IdentityJobPolicy) {
        if self.preset.is_some() {
            target.preset = self.preset.clone();
        }
        if self.desired_concurrency.is_some() {
            target.desired_concurrency = self.desired_concurrency;
        }
        if self.limit.is_some() {
            target.limit = self.limit;
        }
        if self.lease_seconds.is_some() {
            target.lease_seconds = self.lease_seconds;
        }
        if self.max_wait_seconds.is_some() {
            target.max_wait_seconds = self.max_wait_seconds;
        }
        if self.allow_wait.is_some() {
            target.allow_wait = self.allow_wait;
        }
        if self.per_asset.is_some() {
            target.per_asset = self.per_asset;
        }
        if self.child_concurrency.is_some() {
            target.child_concurrency = self.child_concurrency;
        }
        if self.runtime_renew_interval_seconds.is_some() {
            target.runtime_renew_interval_seconds = self.runtime_renew_interval_seconds;
        }
        if self.child_timeout_seconds.is_some() {
            target.child_timeout_seconds = self.child_timeout_seconds;
        }
        if self.child_result_dir.is_some() {
            target.child_result_dir = self.child_result_dir.clone();
        }
        if self.max_failed_assets.is_some() {
            target.max_failed_assets = self.max_failed_assets;
        }
        if self.max_failed_assets_per_reason.is_some() {
            target.max_failed_assets_per_reason = self.max_failed_assets_per_reason;
        }
        if self.allow_states.is_some() {
            target.allow_states = self.allow_states.clone();
        }
        if self.include_dispatch_leased.is_some() {
            target.include_dispatch_leased = self.include_dispatch_leased;
        }
        if self.include_retry.is_some() {
            target.include_retry = self.include_retry;
        }
        if self.include_failed.is_some() {
            target.include_failed = self.include_failed;
        }
        if self.include_cancelled.is_some() {
            target.include_cancelled = self.include_cancelled;
        }
        if self.include_runtime_leased.is_some() {
            target.include_runtime_leased = self.include_runtime_leased;
        }
        if self.include_missing_profile_dir.is_some() {
            target.include_missing_profile_dir = self.include_missing_profile_dir;
        }
        if self.skip_sweep.is_some() {
            target.skip_sweep = self.skip_sweep;
        }
        if self.skip_validate.is_some() {
            target.skip_validate = self.skip_validate;
        }
        if self.runtime_grace_seconds.is_some() {
            target.runtime_grace_seconds = self.runtime_grace_seconds;
        }
        if self.dispatch_grace_seconds.is_some() {
            target.dispatch_grace_seconds = self.dispatch_grace_seconds;
        }
        if self.cooldown_grace_seconds.is_some() {
            target.cooldown_grace_seconds = self.cooldown_grace_seconds;
        }
        if self.failure_cooldown_seconds.is_some() {
            target.failure_cooldown_seconds = self.failure_cooldown_seconds;
        }
        if self.failure_next_state.is_some() {
            target.failure_next_state = self.failure_next_state.clone();
        }
        if !self.failure_reason_rules.is_empty() {
            target
                .failure_reason_rules
                .extend(normalize_identity_job_failure_reason_rules(
                    &self.failure_reason_rules,
                ));
        }
        if self.runtime_risk_ledgers.is_some() {
            target.runtime_risk_ledgers = self.runtime_risk_ledgers.clone();
        }
        if self.runtime_risk_window_seconds.is_some() {
            target.runtime_risk_window_seconds = self.runtime_risk_window_seconds;
        }
        if self.runtime_risk_out.is_some() {
            target.runtime_risk_out = self.runtime_risk_out.clone();
        }
        if self.append_runtime_risk.is_some() {
            target.append_runtime_risk = self.append_runtime_risk;
        }
        if self.explain_out.is_some() {
            target.explain_out = self.explain_out.clone();
        }
    }
}

fn merge_cli_gate(gate: &mut IdentityGate, cli: &IdentityGate) {
    if cli.preset.is_some() {
        gate.preset = cli.preset;
    }
    if cli.min_score.is_some() {
        gate.min_score = cli.min_score;
    }
    if cli.max_linkability.is_some() {
        gate.max_linkability = cli.max_linkability;
    }
    if cli.max_concentration_ratio.is_some() {
        gate.max_concentration_ratio = cli.max_concentration_ratio;
    }
    if cli.max_concentrated_signals.is_some() {
        gate.max_concentrated_signals = cli.max_concentrated_signals;
    }
    if cli.min_entropy_score.is_some() {
        gate.min_entropy_score = cli.min_entropy_score;
    }
    if cli.min_effective_identity_count.is_some() {
        gate.min_effective_identity_count = cli.min_effective_identity_count;
    }
    if cli.max_nominal_to_effective_ratio.is_some() {
        gate.max_nominal_to_effective_ratio = cli.max_nominal_to_effective_ratio;
    }
    gate.fail_on_high_risk |= cli.fail_on_high_risk;
    gate.fail_on_risky_pairs |= cli.fail_on_risky_pairs;
}

fn parse_identity_policy(text: &str) -> Result<IdentityPolicySpec> {
    serde_json::from_str(text).context("identity policy must be a JSON object")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BaselineLinkabilityPair {
    candidate_index: usize,
    candidate_id: String,
    baseline_index: usize,
    baseline_id: String,
    score: u8,
    same_identity_likely: bool,
    signals: Vec<LinkabilitySignal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BaselineIdentityCluster {
    candidate_indexes: Vec<usize>,
    candidate_ids: Vec<String>,
    baseline_indexes: Vec<usize>,
    baseline_ids: Vec<String>,
    pair_count: usize,
    max_score: u8,
    strong_signal_count: usize,
    signal_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BaselineIdentityOffender {
    index: usize,
    identity_id: String,
    pair_count: usize,
    max_score: u8,
    strong_signal_count: usize,
    linked_indexes: Vec<usize>,
    linked_ids: Vec<String>,
    signal_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BaselineQuarantinePlan {
    candidate_indexes: Vec<usize>,
    candidate_ids: Vec<String>,
    covered_pair_count: usize,
    remaining_pair_count: usize,
    max_covered_score: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineAdmissionPlan {
    action: IdentityAdmissionAction,
    accept_indexes: Vec<usize>,
    accept_ids: Vec<String>,
    quarantine_indexes: Vec<usize>,
    quarantine_ids: Vec<String>,
    total_count: usize,
    accept_count: usize,
    quarantine_count: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OfflineLedgerDecision {
    Accept,
    Quarantine,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLedgerEntry {
    index: usize,
    identity_id: String,
    decision: OfflineLedgerDecision,
    accepted: bool,
    known_in_baseline: bool,
    duplicate_in_batch: bool,
    internal_linked_indexes: Vec<usize>,
    internal_linked_ids: Vec<String>,
    baseline_linked_indexes: Vec<usize>,
    baseline_linked_ids: Vec<String>,
    max_internal_linkability: u8,
    max_baseline_linkability: u8,
    signal_codes: Vec<String>,
    reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLedgerReport {
    candidate_count: usize,
    accepted_count: usize,
    quarantine_count: usize,
    known_baseline_count: usize,
    duplicate_candidate_count: usize,
    risky_internal_count: usize,
    risky_baseline_count: usize,
    entries: Vec<OfflineLedgerEntry>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OfflinePoolActionSource {
    Admission,
    Remediation,
    Capacity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflinePoolActionEntry {
    action_index: usize,
    source: OfflinePoolActionSource,
    action_code: String,
    target: IdentityPoolRemediationTarget,
    priority: IdentityFixPriority,
    title: String,
    detail: String,
    indexes: Vec<usize>,
    identity_ids: Vec<String>,
    affected_count: usize,
    pair_count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    signal_codes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    values: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    reasons: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision: Option<OfflineLedgerDecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accepted: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    internal_linked_indexes: Vec<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    internal_linked_ids: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    baseline_linked_indexes: Vec<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    baseline_linked_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_internal_linkability: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_baseline_linkability: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    estimated_gain: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflinePoolActionQueue {
    snapshot_count: usize,
    action_count: usize,
    admission_action_count: usize,
    remediation_action_count: usize,
    capacity_action_count: usize,
    high_priority_count: usize,
    quarantine_count: usize,
    affected_identity_ids: Vec<String>,
    actions: Vec<OfflinePoolActionEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BaselineCompareReport {
    candidate_count: usize,
    candidate_ids: Vec<String>,
    baseline_count: usize,
    baseline_ids: Vec<String>,
    max_linkability: u8,
    risky_pairs: Vec<BaselineLinkabilityPair>,
    clusters: Vec<BaselineIdentityCluster>,
    candidate_offenders: Vec<BaselineIdentityOffender>,
    baseline_offenders: Vec<BaselineIdentityOffender>,
    candidate_quarantine: BaselineQuarantinePlan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineDriftGateCriteria {
    max_drift_score: Option<u8>,
    fail_on_high_risk_drift: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineDriftGateReport {
    passed: bool,
    criteria: OfflineDriftGateCriteria,
    failures: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineDriftEntry {
    index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    before_index: usize,
    after_index: usize,
    before_id: String,
    after_id: String,
    before_stable_hash: String,
    after_stable_hash: String,
    stable_hash_changed: bool,
    stable: bool,
    high_risk: bool,
    score: u8,
    severity: IdentityDriftSeverity,
    changed_signal_count: usize,
    high_risk_signal_count: usize,
    signals: Vec<IdentityDriftSignal>,
    remediation: IdentityDriftRemediationPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineDriftActionEntry {
    entry_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    before_index: usize,
    after_index: usize,
    before_id: String,
    after_id: String,
    drift_score: u8,
    drift_severity: IdentityDriftSeverity,
    high_risk: bool,
    stable_hash_changed: bool,
    action_code: String,
    target: IdentityDriftRemediationTarget,
    priority: IdentityFixPriority,
    title: String,
    detail: String,
    fields: Vec<String>,
    signal_codes: Vec<String>,
    before_values: Vec<String>,
    after_values: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineDriftActionQueue {
    entry_count: usize,
    action_count: usize,
    high_priority_count: usize,
    labels: Vec<String>,
    actions: Vec<OfflineDriftActionEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OfflineLifecycleState {
    Active,
    Repair,
    Quarantine,
    MissingCurrent,
    NewCurrent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLifecycleEntry {
    index: usize,
    state: OfflineLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before_stable_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after_stable_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stable_hash_changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    high_risk: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    drift_score: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    drift_severity: Option<IdentityDriftSeverity>,
    changed_signal_count: usize,
    high_risk_signal_count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    reason_codes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    signals: Vec<IdentityDriftSignal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remediation: Option<IdentityDriftRemediationPlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLifecycleLedger {
    baseline_count: usize,
    current_count: usize,
    entry_count: usize,
    active_count: usize,
    repair_count: usize,
    quarantine_count: usize,
    missing_current_count: usize,
    new_current_count: usize,
    changed_count: usize,
    high_risk_count: usize,
    max_drift_score: u8,
    labels: Vec<String>,
    entries: Vec<OfflineLifecycleEntry>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OfflineLifecycleActionSource {
    Lifecycle,
    DriftRemediation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLifecycleActionEntry {
    action_index: usize,
    entry_index: usize,
    source: OfflineLifecycleActionSource,
    state: OfflineLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    drift_score: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    high_risk: Option<bool>,
    action_code: String,
    target: IdentityDriftRemediationTarget,
    priority: IdentityFixPriority,
    title: String,
    detail: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    fields: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    signal_codes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    before_values: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    after_values: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    reason_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLifecycleActionQueue {
    entry_count: usize,
    action_count: usize,
    lifecycle_action_count: usize,
    remediation_action_count: usize,
    high_priority_count: usize,
    labels: Vec<String>,
    actions: Vec<OfflineLifecycleActionEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLifecycleGateCriteria {
    max_drift_score: Option<u8>,
    fail_on_high_risk_drift: bool,
    fail_on_missing_current: bool,
    fail_on_new_current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLifecycleGateReport {
    passed: bool,
    criteria: OfflineLifecycleGateCriteria,
    failures: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OfflineLifecycleBaselineSource {
    Baseline,
    Current,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLifecycleBaselineEntry {
    entry_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    state: OfflineLifecycleState,
    source: OfflineLifecycleBaselineSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    before_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after_index: Option<usize>,
    identity_id: String,
    stable_hash: String,
    snapshot: FingerprintSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLifecycleNextBaseline {
    policy: IdentityLifecycleBaselinePolicy,
    count: usize,
    current_source_count: usize,
    baseline_source_count: usize,
    skipped_count: usize,
    kept_states: Vec<OfflineLifecycleState>,
    skipped_states: Vec<OfflineLifecycleState>,
    entries: Vec<OfflineLifecycleBaselineEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OfflineLifecycleDeltaChange {
    BaselineRetained,
    BaselineUpdated,
    BaselineRemoved,
    CurrentExcluded,
    NewCurrentUnadmitted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLifecycleDeltaEntry {
    change_index: usize,
    entry_index: usize,
    change: OfflineLifecycleDeltaChange,
    state: OfflineLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_stable_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_stable_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<OfflineLifecycleBaselineSource>,
    reason_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLifecycleDelta {
    baseline_count: usize,
    current_count: usize,
    next_baseline_count: usize,
    change_count: usize,
    retained_count: usize,
    updated_count: usize,
    removed_count: usize,
    current_excluded_count: usize,
    new_unadmitted_count: usize,
    affected_labels: Vec<String>,
    entries: Vec<OfflineLifecycleDeltaEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLifecycleRunSummary {
    entry_count: usize,
    active_count: usize,
    repair_count: usize,
    quarantine_count: usize,
    missing_current_count: usize,
    new_current_count: usize,
    changed_count: usize,
    high_risk_count: usize,
    max_drift_score: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLifecycleRunRecord {
    run_id: String,
    generated_at_unix_seconds: u64,
    baseline_path: String,
    current_path: String,
    requested_match_by: IdentityDriftMatchMode,
    match_by: IdentityDriftMatchMode,
    baseline_count: usize,
    current_count: usize,
    gate_passed: bool,
    gate_failures: Vec<String>,
    summary: OfflineLifecycleRunSummary,
    next_baseline_policy: IdentityLifecycleBaselinePolicy,
    next_baseline_count: usize,
    delta_change_count: usize,
    action_count: usize,
    high_priority_action_count: usize,
    affected_labels: Vec<String>,
    artifacts: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLifecycleStateExportEntry {
    entry_index: usize,
    state: OfflineLifecycleState,
    source: OfflineLifecycleBaselineSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after_index: Option<usize>,
    identity_id: String,
    stable_hash: String,
    snapshot: FingerprintSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineLifecycleStateExport {
    state: OfflineLifecycleState,
    count: usize,
    current_source_count: usize,
    baseline_source_count: usize,
    labels: Vec<String>,
    entries: Vec<OfflineLifecycleStateExportEntry>,
}

#[derive(Debug, Clone)]
struct LabeledSnapshot {
    label: Option<String>,
    snapshot: FingerprintSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IdentityApplyIntent {
    QuarantineProfile,
    ReviewProfile,
    InvestigateProfile,
    RemediationPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IdentityApplyStatus {
    Planned,
    Applied,
    Skipped,
    Unresolved,
    Failed,
}

#[derive(Debug, Clone)]
struct IdentityApplyAction {
    action_index: usize,
    action_code: String,
    priority: Option<String>,
    labels: Vec<String>,
    identity_ids: Vec<String>,
}

#[derive(Debug, Clone)]
struct IdentityApplyTarget {
    target_index: usize,
    label: Option<String>,
    identity_id: Option<String>,
    selectors: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct ProfileBindings {
    by_key: BTreeMap<String, ProfileBinding>,
}

#[derive(Debug, Clone)]
struct ProfileBinding {
    path: PathBuf,
    asset: Option<IdentityProfileAsset>,
}

#[derive(Debug, Clone)]
struct ProfileCandidate {
    path: PathBuf,
    asset: Option<IdentityProfileAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityProfileAsset {
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    profile_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    proxy_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fingerprint_seed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityApplyOperation {
    operation_index: usize,
    action_index: usize,
    target_index: usize,
    action_code: String,
    intent: IdentityApplyIntent,
    #[serde(skip_serializing_if = "Option::is_none")]
    priority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    selectors: Vec<String>,
    executable: bool,
    execute: bool,
    status: IdentityApplyStatus,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    destination_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_exists: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    asset: Option<IdentityProfileAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityApplyAssetPatch {
    patch_index: usize,
    operation_index: usize,
    action_index: usize,
    target_index: usize,
    action_code: String,
    intent: IdentityApplyIntent,
    status: IdentityApplyStatus,
    execute: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proxy_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fingerprint_seed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_state: Option<String>,
    next_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    destination_path: Option<String>,
    reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityApplyAssetStateOut {
    path: String,
    append: bool,
    count: usize,
    format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityApplyJournalOut {
    path: String,
    append: bool,
    count: usize,
    format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityApplyReport {
    scope: String,
    path: String,
    execute: bool,
    dry_run: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_map: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quarantine_dir: Option<String>,
    run_id: String,
    generated_at_unix_seconds: u64,
    action_count: usize,
    operation_count: usize,
    executable_count: usize,
    planned_count: usize,
    applied_count: usize,
    skipped_count: usize,
    unresolved_count: usize,
    failed_count: usize,
    operations: Vec<IdentityApplyOperation>,
    asset_patch_count: usize,
    asset_patches: Vec<IdentityApplyAssetPatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    asset_state_out: Option<IdentityApplyAssetStateOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    journal_out: Option<IdentityApplyJournalOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityPlanInput {
    input_index: usize,
    path: String,
    format: String,
    value_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gate_passed: Option<bool>,
    gate_failures: Vec<String>,
    action_count: usize,
    asset_patch_count: usize,
    operation_count: usize,
    failed_operation_count: usize,
    unresolved_operation_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityPlanAction {
    plan_index: usize,
    input_index: usize,
    input_path: String,
    action_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    priority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    estimated_gain: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    affected_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    signal_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityPlanAssetPatch {
    plan_index: usize,
    input_index: usize,
    input_path: String,
    #[serde(flatten)]
    patch: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityPlanRunbookStep {
    step_index: usize,
    phase: String,
    title: String,
    rationale: String,
    action_count: usize,
    asset_patch_count: usize,
    high_priority_action_count: usize,
    blocked_by_gate: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    action_indexes: Vec<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    asset_patch_indexes: Vec<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_command_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IdentityPlanDispatchKind {
    Action,
    AssetPatch,
    Command,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityPlanDispatchItem {
    dispatch_index: usize,
    step_index: usize,
    phase: String,
    kind: IdentityPlanDispatchKind,
    sort_rank: u16,
    dedupe_key: String,
    lease_key: String,
    blocked_by_gate: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    action_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    asset_patch_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    action_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    priority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    signal_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityPlanDispatchQueue {
    item_count: usize,
    action_item_count: usize,
    asset_patch_item_count: usize,
    command_item_count: usize,
    high_priority_count: usize,
    blocked_count: usize,
    phases: Vec<String>,
    items: Vec<IdentityPlanDispatchItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityPlanSummary {
    input_count: usize,
    value_count: usize,
    action_count: usize,
    high_priority_action_count: usize,
    quarantine_action_count: usize,
    remediation_action_count: usize,
    capacity_action_count: usize,
    lifecycle_action_count: usize,
    asset_patch_count: usize,
    failed_operation_count: usize,
    unresolved_operation_count: usize,
    dispatch_item_count: usize,
    gate_failed: bool,
    gate_failure_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityPlanOutput {
    path: String,
    format: String,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityPlanAssetManifestOut {
    source_path: String,
    path: String,
    format: String,
    asset_count: usize,
    updated_count: usize,
    unchanged_count: usize,
    unmatched_patch_count: usize,
    state_counts: BTreeMap<String, usize>,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityPlanDispatchOut {
    path: String,
    append: bool,
    count: usize,
    format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityDispatchClaimItem {
    claim_index: usize,
    status: String,
    worker_id: String,
    claim_id: String,
    lease_expires_unix_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    claimed_at_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    renewal_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    renewed_at_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_lease_expires_unix_seconds: Option<u64>,
    dispatch: IdentityPlanDispatchItem,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityDispatchClaimOut {
    path: String,
    append: bool,
    count: usize,
    format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityDispatchClaimReport {
    scope: String,
    path: String,
    worker_id: String,
    claim_id: String,
    generated_at_unix_seconds: u64,
    lease_seconds: u64,
    lease_expires_unix_seconds: u64,
    requested_limit: usize,
    include_blocked: bool,
    include_leased: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    claim_ledger: Option<String>,
    include_completed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_ledger: Option<String>,
    input_count: usize,
    candidate_count: usize,
    claimed_count: usize,
    skipped_blocked_count: usize,
    skipped_leased_count: usize,
    skipped_completed_count: usize,
    active_lease_count: usize,
    expired_lease_count: usize,
    completion_ledger_count: usize,
    terminal_completion_count: usize,
    retryable_completion_count: usize,
    duplicate_dedupe_key_count: usize,
    remaining_candidate_count: usize,
    claimed_phases: Vec<String>,
    items: Vec<IdentityDispatchClaimItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    claim_out: Option<IdentityDispatchClaimOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityDispatchRenewReport {
    scope: String,
    path: String,
    renewal_id: String,
    generated_at_unix_seconds: u64,
    lease_seconds: u64,
    lease_expires_unix_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    claim_id: Option<String>,
    filtered_dedupe_keys: Vec<String>,
    include_expired: bool,
    input_count: usize,
    renewed_count: usize,
    skipped_worker_count: usize,
    skipped_claim_id_count: usize,
    skipped_dedupe_key_count: usize,
    skipped_non_leased_count: usize,
    skipped_expired_count: usize,
    duplicate_dedupe_key_count: usize,
    items: Vec<IdentityDispatchClaimItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    claim_out: Option<IdentityDispatchClaimOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityDispatchReconcileManifestOut {
    source_path: String,
    path: String,
    format: String,
    asset_count: usize,
    updated_count: usize,
    unchanged_count: usize,
    unmatched_event_count: usize,
    state_counts: BTreeMap<String, usize>,
    dispatch_state_counts: BTreeMap<String, usize>,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityDispatchManifestUpdate {
    asset_index: usize,
    dispatch_state: String,
    status: String,
    dedupe_key: String,
    phase: String,
    kind: IdentityPlanDispatchKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    claim_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_id: Option<String>,
    updated_at_unix_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityDispatchReconcileReport {
    scope: String,
    asset_manifest: String,
    generated_at_unix_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    claim_ledger: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_ledger: Option<String>,
    asset_count: usize,
    claim_item_count: usize,
    completion_item_count: usize,
    reconciled_event_count: usize,
    updated_asset_count: usize,
    unmatched_event_count: usize,
    active_lease_count: usize,
    expired_lease_count: usize,
    completion_event_count: usize,
    state_counts: BTreeMap<String, usize>,
    dispatch_state_counts: BTreeMap<String, usize>,
    updates: Vec<IdentityDispatchManifestUpdate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    asset_manifest_out: Option<IdentityDispatchReconcileManifestOut>,
}

#[derive(Debug, Clone)]
struct IdentityDispatchReconcileEvent {
    dispatch_state: String,
    status: String,
    worker_id: Option<String>,
    claim_id: Option<String>,
    completion_id: Option<String>,
    lease_expires_unix_seconds: Option<u64>,
    completed_at_unix_seconds: Option<u64>,
    retry_eligible: Option<bool>,
    retry_after_unix_seconds: Option<u64>,
    updated_at_unix_seconds: u64,
    result: Option<Value>,
    dispatch: IdentityPlanDispatchItem,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetSelectionOut {
    path: String,
    format: String,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetStatusOut {
    path: String,
    format: String,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetValidateOut {
    path: String,
    format: String,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetValidationIssue {
    severity: String,
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    asset_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<Value>,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetsValidateReport {
    scope: String,
    asset_manifest: String,
    generated_at_unix_seconds: u64,
    strict: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    manifest_version: Option<String>,
    valid: bool,
    asset_count: usize,
    issue_count: usize,
    error_count: usize,
    warning_count: usize,
    info_count: usize,
    duplicate_key_count: usize,
    state_counts: BTreeMap<String, usize>,
    dispatch_state_counts: BTreeMap<String, usize>,
    runtime_lease_state_counts: BTreeMap<String, usize>,
    issue_code_counts: BTreeMap<String, usize>,
    issues: Vec<IdentityAssetValidationIssue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    validate_out: Option<IdentityAssetValidateOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetsStatusRecommendation {
    code: String,
    severity: String,
    affected_count: usize,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetsStatusReport {
    scope: String,
    asset_manifest: String,
    generated_at_unix_seconds: u64,
    allowed_states: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    desired_concurrency: Option<usize>,
    asset_count: usize,
    runnable_count: usize,
    blocked_count: usize,
    capacity_status: String,
    capacity_shortage_count: usize,
    recommended_limit: usize,
    state_counts: BTreeMap<String, usize>,
    dispatch_state_counts: BTreeMap<String, usize>,
    runtime_lease_state_counts: BTreeMap<String, usize>,
    active_runtime_lease_count: usize,
    expired_runtime_lease_count: usize,
    active_dispatch_lease_count: usize,
    expired_dispatch_lease_count: usize,
    active_cooldown_count: usize,
    expired_cooldown_count: usize,
    dispatch_retry_waiting_count: usize,
    dispatch_retry_ready_count: usize,
    missing_profile_dir_count: usize,
    block_reason_counts: BTreeMap<String, usize>,
    recommendations: Vec<IdentityAssetsStatusRecommendation>,
    runnable_assets: Vec<IdentityAssetSelectionItem>,
    blocked_assets: Vec<IdentityAssetSelectionItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_out: Option<IdentityAssetStatusOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetForecastOut {
    path: String,
    format: String,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetForecastItem {
    asset_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_dir: Option<String>,
    state: String,
    dispatch_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_lease_state: Option<String>,
    reasons: Vec<String>,
    recoverable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    available_at_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seconds_until_available: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetsForecastReport {
    scope: String,
    asset_manifest: String,
    generated_at_unix_seconds: u64,
    allowed_states: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    desired_concurrency: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    horizon_seconds: Option<u64>,
    asset_count: usize,
    current_runnable_count: usize,
    blocked_count: usize,
    recoverable_count: usize,
    recoverable_within_horizon_count: usize,
    hard_blocked_count: usize,
    predicted_runnable_count: usize,
    current_shortage_count: usize,
    predicted_shortage_count: usize,
    capacity_status: String,
    predicted_capacity_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_recovery_at_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enough_at_unix_seconds: Option<u64>,
    block_reason_counts: BTreeMap<String, usize>,
    recovery_reason_counts: BTreeMap<String, usize>,
    state_counts: BTreeMap<String, usize>,
    dispatch_state_counts: BTreeMap<String, usize>,
    runtime_lease_state_counts: BTreeMap<String, usize>,
    recovery_events: Vec<IdentityAssetForecastItem>,
    hard_blocked_assets: Vec<IdentityAssetForecastItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    forecast_out: Option<IdentityAssetForecastOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetGateOut {
    path: String,
    format: String,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetsGateReport {
    scope: String,
    asset_manifest: String,
    generated_at_unix_seconds: u64,
    desired_concurrency: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_wait_seconds: Option<u64>,
    allow_wait: bool,
    passed: bool,
    exit_code: i32,
    decision: String,
    recommended_action: String,
    message: String,
    current_runnable_count: usize,
    predicted_runnable_count: usize,
    current_shortage_count: usize,
    predicted_shortage_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    enough_at_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seconds_until_enough: Option<u64>,
    forecast: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    gate_out: Option<IdentityAssetGateOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetSelectionManifestOut {
    source_path: String,
    path: String,
    format: String,
    asset_count: usize,
    leased_count: usize,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetSelectionItem {
    asset_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proxy_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fingerprint_seed: Option<String>,
    state: String,
    dispatch_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_lease_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_lease_expires_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lease_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetsSelectReport {
    scope: String,
    asset_manifest: String,
    generated_at_unix_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    job_id: Option<String>,
    lease_seconds: u64,
    lease_expires_unix_seconds: u64,
    allowed_states: Vec<String>,
    requested_limit: usize,
    asset_count: usize,
    selected_count: usize,
    blocked_count: usize,
    overflow_count: usize,
    state_counts: BTreeMap<String, usize>,
    dispatch_state_counts: BTreeMap<String, usize>,
    block_reason_counts: BTreeMap<String, usize>,
    selected_assets: Vec<IdentityAssetSelectionItem>,
    blocked_assets: Vec<IdentityAssetSelectionItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    asset_manifest_out: Option<IdentityAssetSelectionManifestOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection_out: Option<IdentityAssetSelectionOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetReleaseManifestOut {
    source_path: String,
    path: String,
    format: String,
    asset_count: usize,
    released_count: usize,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetReleaseOut {
    path: String,
    append: bool,
    count: usize,
    format: String,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetReleaseItem {
    asset_index: usize,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lease_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cooldown_until_unix_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetsReleaseReport {
    scope: String,
    asset_manifest: String,
    generated_at_unix_seconds: u64,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    job_id: Option<String>,
    filtered_lease_ids: Vec<String>,
    filtered_account_ids: Vec<String>,
    filtered_profile_ids: Vec<String>,
    filtered_identity_ids: Vec<String>,
    filtered_labels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cooldown_until_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    asset_count: usize,
    matched_count: usize,
    released_count: usize,
    skipped_filter_count: usize,
    skipped_non_leased_count: usize,
    state_counts: BTreeMap<String, usize>,
    runtime_lease_state_counts: BTreeMap<String, usize>,
    released_assets: Vec<IdentityAssetReleaseItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    asset_manifest_out: Option<IdentityAssetReleaseManifestOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_out: Option<IdentityAssetReleaseOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetRuntimeReconcileManifestOut {
    source_path: String,
    path: String,
    format: String,
    asset_count: usize,
    updated_count: usize,
    unchanged_count: usize,
    unmatched_event_count: usize,
    state_counts: BTreeMap<String, usize>,
    runtime_lease_state_counts: BTreeMap<String, usize>,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetRuntimeReconcileUpdate {
    asset_index: usize,
    event_index: usize,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lease_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    job_id: Option<String>,
    released_at_unix_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetsRuntimeReconcileReport {
    scope: String,
    asset_manifest: String,
    generated_at_unix_seconds: u64,
    release_ledgers: Vec<String>,
    asset_count: usize,
    release_event_count: usize,
    updated_asset_count: usize,
    unmatched_event_count: usize,
    state_counts: BTreeMap<String, usize>,
    runtime_lease_state_counts: BTreeMap<String, usize>,
    updates: Vec<IdentityAssetRuntimeReconcileUpdate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    asset_manifest_out: Option<IdentityAssetRuntimeReconcileManifestOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetHealthManifestOut {
    source_path: String,
    path: String,
    format: String,
    asset_count: usize,
    updated_count: usize,
    action_counts: BTreeMap<String, usize>,
    state_counts: BTreeMap<String, usize>,
    runtime_lease_state_counts: BTreeMap<String, usize>,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetHealthOut {
    path: String,
    format: String,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetHealthItem {
    asset_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_dir: Option<String>,
    event_count: usize,
    success_count: usize,
    failed_count: usize,
    cancelled_count: usize,
    other_count: usize,
    unsuccessful_count: usize,
    consecutive_unsuccessful_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    health_score: Option<u8>,
    health_state: String,
    recommended_action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_released_at_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cooldown_until_unix_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetsHealthReport {
    scope: String,
    asset_manifest: String,
    generated_at_unix_seconds: u64,
    release_ledgers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    window_seconds: Option<u64>,
    repair_threshold: usize,
    quarantine_threshold: usize,
    cooldown_seconds: u64,
    asset_count: usize,
    release_event_count: usize,
    matched_event_count: usize,
    unmatched_event_count: usize,
    healthy_count: usize,
    watch_count: usize,
    degraded_count: usize,
    quarantine_count: usize,
    unknown_count: usize,
    updated_asset_count: usize,
    action_counts: BTreeMap<String, usize>,
    state_counts: BTreeMap<String, usize>,
    runtime_lease_state_counts: BTreeMap<String, usize>,
    assets: Vec<IdentityAssetHealthItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    asset_manifest_out: Option<IdentityAssetHealthManifestOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    health_out: Option<IdentityAssetHealthOut>,
}

#[derive(Debug, Clone)]
struct IdentityAssetRuntimeReleaseEvent {
    event_index: usize,
    source_path: String,
    generated_at_unix_seconds: u64,
    status: String,
    worker_id: Option<String>,
    job_id: Option<String>,
    cooldown_until_unix_seconds: Option<u64>,
    next_state: Option<String>,
    message: Option<String>,
    result: Option<Value>,
    item: IdentityAssetReleaseItem,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetSweepManifestOut {
    source_path: String,
    path: String,
    format: String,
    asset_count: usize,
    updated_count: usize,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetSweepOut {
    path: String,
    format: String,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetSweepItem {
    asset_index: usize,
    action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_value: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityAssetsSweepReport {
    scope: String,
    asset_manifest: String,
    generated_at_unix_seconds: u64,
    runtime_grace_seconds: u64,
    dispatch_grace_seconds: u64,
    cooldown_grace_seconds: u64,
    asset_count: usize,
    updated_asset_count: usize,
    expired_runtime_lease_count: usize,
    expired_dispatch_lease_count: usize,
    cleared_cooldown_count: usize,
    state_counts: BTreeMap<String, usize>,
    runtime_lease_state_counts: BTreeMap<String, usize>,
    dispatch_state_counts: BTreeMap<String, usize>,
    actions: Vec<IdentityAssetSweepItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    asset_manifest_out: Option<IdentityAssetSweepManifestOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sweep_out: Option<IdentityAssetSweepOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityDispatchCompletionItem {
    completion_index: usize,
    status: String,
    worker_id: String,
    claim_id: String,
    completion_id: String,
    completed_at_unix_seconds: u64,
    retry_eligible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    claim: IdentityDispatchClaimItem,
    dispatch: IdentityPlanDispatchItem,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityDispatchCompletionOut {
    path: String,
    append: bool,
    count: usize,
    format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityDispatchCompletionReport {
    scope: String,
    path: String,
    completion_id: String,
    generated_at_unix_seconds: u64,
    status: String,
    retry_eligible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    claim_id: Option<String>,
    filtered_dedupe_keys: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    input_count: usize,
    completed_count: usize,
    skipped_worker_count: usize,
    skipped_claim_id_count: usize,
    skipped_dedupe_key_count: usize,
    duplicate_dedupe_key_count: usize,
    items: Vec<IdentityDispatchCompletionItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    complete_out: Option<IdentityDispatchCompletionOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityPlanReport {
    scope: String,
    title: String,
    run_id: String,
    generated_at_unix_seconds: u64,
    summary: IdentityPlanSummary,
    inputs: Vec<IdentityPlanInput>,
    action_code_counts: BTreeMap<String, usize>,
    priority_counts: BTreeMap<String, usize>,
    state_counts: BTreeMap<String, usize>,
    recommendations: Vec<String>,
    execution_runbook: Vec<IdentityPlanRunbookStep>,
    dispatch_queue: IdentityPlanDispatchQueue,
    actions: Vec<IdentityPlanAction>,
    asset_patches: Vec<IdentityPlanAssetPatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    asset_manifest_out: Option<IdentityPlanAssetManifestOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    out: Option<IdentityPlanOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    html_out: Option<IdentityPlanOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dispatch_out: Option<IdentityPlanDispatchOut>,
}

pub async fn build_identity_plan(
    inputs: &[PathBuf],
    title: Option<&str>,
    out: Option<&Path>,
    html_out: Option<&Path>,
    asset_manifest: Option<&Path>,
    asset_manifest_out: Option<&Path>,
    dispatch_out: Option<&Path>,
    append_dispatch: bool,
) -> Result<JsonResponse> {
    if inputs.is_empty() {
        bail!("identity-plan requires at least one input file");
    }
    if asset_manifest_out.is_some() && asset_manifest.is_none() {
        bail!("--asset-manifest-out requires --asset-manifest");
    }

    let generated_at = unix_seconds();
    let run_id = format!("plan_{}_{}", generated_at, std::process::id());
    let mut input_reports = Vec::new();
    let mut actions = Vec::new();
    let mut asset_patches = Vec::new();
    let mut value_count = 0usize;
    let mut failed_operation_count = 0usize;
    let mut unresolved_operation_count = 0usize;
    let mut gate_failure_count = 0usize;

    for (input_index, path) in inputs.iter().enumerate() {
        let text = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read identity plan input {}", path.display()))?;
        let (format, values) = parse_identity_plan_values(&text)
            .with_context(|| format!("failed to parse identity plan input {}", path.display()))?;
        value_count += values.len();
        let input_path = path.display().to_string();
        let mut input_actions = Vec::new();
        let mut input_patches = Vec::new();
        let mut input_operations = Vec::new();
        let mut input_gate_failures = Vec::new();
        let mut input_gate_passed = None;

        for value in &values {
            input_actions.extend(identity_plan_action_values_from_value(value));
            input_patches.extend(identity_plan_asset_patch_values_from_value(value));
            input_operations.extend(identity_plan_operation_values_from_value(value));
            if let Some((passed, failures)) = identity_plan_gate_from_value(value) {
                input_gate_passed = Some(input_gate_passed.unwrap_or(true) && passed);
                input_gate_failures.extend(failures);
            }
        }

        let input_failed_operations = input_operations
            .iter()
            .filter(|operation| identity_plan_status(operation) == Some("failed".to_string()))
            .count();
        let input_unresolved_operations = input_operations
            .iter()
            .filter(|operation| identity_plan_status(operation) == Some("unresolved".to_string()))
            .count();
        failed_operation_count += input_failed_operations;
        unresolved_operation_count += input_unresolved_operations;
        gate_failure_count += input_gate_failures.len();

        let scope = values.iter().find_map(identity_plan_scope_from_value);
        let ok = values.iter().find_map(identity_plan_ok_from_value);
        for action in &input_actions {
            let plan_action =
                identity_plan_action_from_value(action, actions.len(), input_index, &input_path);
            actions.push(plan_action);
        }
        for patch in &input_patches {
            if let Some(patch) =
                identity_plan_patch_from_value(patch, asset_patches.len(), input_index, &input_path)
            {
                asset_patches.push(patch);
            }
        }

        input_reports.push(IdentityPlanInput {
            input_index,
            path: input_path,
            format,
            value_count: values.len(),
            scope,
            ok,
            gate_passed: input_gate_passed,
            gate_failures: input_gate_failures,
            action_count: input_actions.len(),
            asset_patch_count: input_patches.len(),
            operation_count: input_operations.len(),
            failed_operation_count: input_failed_operations,
            unresolved_operation_count: input_unresolved_operations,
        });
    }

    let action_code_counts = count_plan_action_codes(&actions);
    let priority_counts = count_plan_priorities(&actions);
    let state_counts = count_plan_states(&actions, &asset_patches);
    let mut summary = build_identity_plan_summary(
        inputs.len(),
        value_count,
        &actions,
        &asset_patches,
        failed_operation_count,
        unresolved_operation_count,
        gate_failure_count,
    );
    let recommendations = build_identity_plan_recommendations(&summary);
    let execution_runbook = build_identity_plan_runbook(&summary, &actions, &asset_patches);
    let dispatch_queue =
        build_identity_plan_dispatch_queue(&run_id, &execution_runbook, &actions, &asset_patches);
    summary.dispatch_item_count = dispatch_queue.item_count;
    let title = title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Identity Governance Plan")
        .to_string();

    let mut report = IdentityPlanReport {
        scope: "identity_plan".to_string(),
        title,
        run_id,
        generated_at_unix_seconds: generated_at,
        summary,
        inputs: input_reports,
        action_code_counts,
        priority_counts,
        state_counts,
        recommendations,
        execution_runbook,
        dispatch_queue,
        actions,
        asset_patches,
        asset_manifest_out: None,
        out: None,
        html_out: None,
        dispatch_out: None,
    };
    report.asset_manifest_out =
        write_identity_plan_asset_manifest(&report, asset_manifest, asset_manifest_out).await?;
    report.dispatch_out =
        write_identity_plan_dispatch(&report, dispatch_out, append_dispatch).await?;
    report.html_out = write_identity_plan_html(&report, html_out).await?;
    report.out = write_identity_plan_json(&report, out).await?;

    Ok(JsonResponse::ok(report))
}

pub async fn claim_identity_dispatch(
    dispatch_path: &Path,
    worker: Option<&str>,
    limit: usize,
    lease_seconds: u64,
    include_blocked: bool,
    claim_ledger: Option<&Path>,
    include_leased: bool,
    completion_ledger: Option<&Path>,
    include_completed: bool,
    claim_out: Option<&Path>,
    append_claim: bool,
) -> Result<JsonResponse> {
    if limit == 0 {
        bail!("identity-dispatch --limit must be greater than 0");
    }
    if lease_seconds == 0 {
        bail!("identity-dispatch --lease-seconds must be greater than 0");
    }

    let text = tokio::fs::read_to_string(dispatch_path)
        .await
        .with_context(|| format!("failed to read dispatch queue {}", dispatch_path.display()))?;
    let mut dispatch_items = parse_identity_dispatch_items(&text)
        .with_context(|| format!("failed to parse dispatch queue {}", dispatch_path.display()))?;
    dispatch_items.sort_by(|a, b| {
        a.sort_rank
            .cmp(&b.sort_rank)
            .then_with(|| a.dispatch_index.cmp(&b.dispatch_index))
            .then_with(|| a.dedupe_key.cmp(&b.dedupe_key))
    });

    let generated_at = unix_seconds();
    let worker_id = worker
        .map(str::trim)
        .filter(|worker| !worker.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("worker-{}", std::process::id()));
    let claim_id = format!(
        "claim_{}_{}_{}",
        generated_at,
        std::process::id(),
        worker_id
    );
    let lease_expires = generated_at.saturating_add(lease_seconds);
    let ledger_items = read_identity_dispatch_claim_ledger(claim_ledger).await?;
    let (active_lease_keys, active_lease_count, expired_lease_count) =
        active_identity_dispatch_leases(&ledger_items, generated_at);
    let completion_items = read_identity_dispatch_completion_ledger(completion_ledger).await?;
    let (terminal_completion_keys, terminal_completion_count, retryable_completion_count) =
        terminal_identity_dispatch_completions(&completion_items);
    let mut seen = BTreeSet::new();
    let mut claimed = Vec::new();
    let mut skipped_blocked_count = 0usize;
    let mut skipped_leased_count = 0usize;
    let mut skipped_completed_count = 0usize;
    let mut duplicate_dedupe_key_count = 0usize;

    for item in dispatch_items {
        if !seen.insert(item.dedupe_key.clone()) {
            duplicate_dedupe_key_count += 1;
            continue;
        }
        if item.blocked_by_gate && !include_blocked {
            skipped_blocked_count += 1;
            continue;
        }
        if active_lease_keys.contains(&item.dedupe_key) && !include_leased {
            skipped_leased_count += 1;
            continue;
        }
        if terminal_completion_keys.contains(&item.dedupe_key) && !include_completed {
            skipped_completed_count += 1;
            continue;
        }
        if claimed.len() >= limit {
            continue;
        }
        claimed.push(IdentityDispatchClaimItem {
            claim_index: claimed.len(),
            status: "leased".to_string(),
            worker_id: worker_id.clone(),
            claim_id: claim_id.clone(),
            lease_expires_unix_seconds: lease_expires,
            claimed_at_unix_seconds: Some(generated_at),
            renewal_id: None,
            renewed_at_unix_seconds: None,
            previous_lease_expires_unix_seconds: None,
            dispatch: item,
        });
    }

    let claimed_phases = identity_claim_phases(&claimed);
    let candidate_count = seen.len();
    let mut report = IdentityDispatchClaimReport {
        scope: "identity_dispatch_claim".to_string(),
        path: dispatch_path.display().to_string(),
        worker_id,
        claim_id,
        generated_at_unix_seconds: generated_at,
        lease_seconds,
        lease_expires_unix_seconds: lease_expires,
        requested_limit: limit,
        include_blocked,
        include_leased,
        claim_ledger: claim_ledger.map(|path| path.display().to_string()),
        include_completed,
        completion_ledger: completion_ledger.map(|path| path.display().to_string()),
        input_count: candidate_count + duplicate_dedupe_key_count,
        candidate_count,
        claimed_count: claimed.len(),
        skipped_blocked_count,
        skipped_leased_count,
        skipped_completed_count,
        active_lease_count,
        expired_lease_count,
        completion_ledger_count: completion_items.len(),
        terminal_completion_count,
        retryable_completion_count,
        duplicate_dedupe_key_count,
        remaining_candidate_count: candidate_count
            .saturating_sub(claimed.len())
            .saturating_sub(skipped_blocked_count)
            .saturating_sub(skipped_leased_count)
            .saturating_sub(skipped_completed_count),
        claimed_phases,
        items: claimed,
        claim_out: None,
    };
    report.claim_out = write_identity_dispatch_claim(&report, claim_out, append_claim).await?;

    Ok(JsonResponse::ok(report))
}

pub async fn renew_identity_dispatch(
    claims_path: &Path,
    worker: Option<&str>,
    claim_id: Option<&str>,
    dedupe_keys: &[String],
    lease_seconds: u64,
    include_expired: bool,
    claim_out: Option<&Path>,
    append_claim: bool,
) -> Result<JsonResponse> {
    if lease_seconds == 0 {
        bail!("identity-dispatch-renew --lease-seconds must be greater than 0");
    }

    let text = tokio::fs::read_to_string(claims_path)
        .await
        .with_context(|| format!("failed to read claim records {}", claims_path.display()))?;
    let claim_items = parse_identity_dispatch_claim_items(&text)
        .with_context(|| format!("failed to parse claim records {}", claims_path.display()))?;
    let worker_filter = worker
        .map(str::trim)
        .filter(|worker| !worker.is_empty())
        .map(ToString::to_string);
    let claim_id_filter = claim_id
        .map(str::trim)
        .filter(|claim_id| !claim_id.is_empty())
        .map(ToString::to_string);
    let dedupe_filter = dedupe_keys
        .iter()
        .map(|key| key.trim())
        .filter(|key| !key.is_empty())
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>();

    let generated_at = unix_seconds();
    let lease_expires = generated_at.saturating_add(lease_seconds);
    let renewal_id = format!(
        "renewal_{}_{}_{}",
        generated_at,
        std::process::id(),
        worker_filter.as_deref().unwrap_or("any-worker")
    );
    let mut skipped_worker_count = 0usize;
    let mut skipped_claim_id_count = 0usize;
    let mut skipped_dedupe_key_count = 0usize;
    let mut skipped_non_leased_count = 0usize;
    let mut skipped_expired_count = 0usize;
    let mut duplicate_dedupe_key_count = 0usize;
    let mut renewed_by_key = BTreeMap::<String, IdentityDispatchClaimItem>::new();

    for claim in claim_items {
        if let Some(worker) = &worker_filter {
            if claim.worker_id != *worker {
                skipped_worker_count += 1;
                continue;
            }
        }
        if let Some(claim_id) = &claim_id_filter {
            if claim.claim_id != *claim_id {
                skipped_claim_id_count += 1;
                continue;
            }
        }
        if !dedupe_filter.is_empty() && !dedupe_filter.contains(&claim.dispatch.dedupe_key) {
            skipped_dedupe_key_count += 1;
            continue;
        }
        if claim.status != "leased" {
            skipped_non_leased_count += 1;
            continue;
        }
        if claim.lease_expires_unix_seconds <= generated_at && !include_expired {
            skipped_expired_count += 1;
            continue;
        }

        let dedupe_key = claim.dispatch.dedupe_key.clone();
        let previous_lease_expires = claim.lease_expires_unix_seconds;
        let mut renewed = claim;
        renewed.claim_index = 0;
        renewed.status = "leased".to_string();
        renewed.worker_id = worker_filter
            .clone()
            .unwrap_or_else(|| renewed.worker_id.clone());
        renewed.lease_expires_unix_seconds = lease_expires;
        renewed.renewal_id = Some(renewal_id.clone());
        renewed.renewed_at_unix_seconds = Some(generated_at);
        renewed.previous_lease_expires_unix_seconds = Some(previous_lease_expires);

        if let Some(existing) = renewed_by_key.get(&dedupe_key) {
            duplicate_dedupe_key_count += 1;
            let existing_previous = existing
                .previous_lease_expires_unix_seconds
                .unwrap_or(existing.lease_expires_unix_seconds);
            if existing_previous >= previous_lease_expires {
                continue;
            }
        }
        renewed_by_key.insert(dedupe_key, renewed);
    }

    let mut items = renewed_by_key.into_values().collect::<Vec<_>>();
    items.sort_by(|a, b| {
        a.dispatch
            .sort_rank
            .cmp(&b.dispatch.sort_rank)
            .then_with(|| a.dispatch.dispatch_index.cmp(&b.dispatch.dispatch_index))
            .then_with(|| a.dispatch.dedupe_key.cmp(&b.dispatch.dedupe_key))
    });
    for (index, item) in items.iter_mut().enumerate() {
        item.claim_index = index;
    }

    let filtered_dedupe_keys = dedupe_filter.iter().cloned().collect::<Vec<_>>();
    let mut report = IdentityDispatchRenewReport {
        scope: "identity_dispatch_renewal".to_string(),
        path: claims_path.display().to_string(),
        renewal_id,
        generated_at_unix_seconds: generated_at,
        lease_seconds,
        lease_expires_unix_seconds: lease_expires,
        worker_id: worker_filter,
        claim_id: claim_id_filter,
        filtered_dedupe_keys,
        include_expired,
        input_count: items.len()
            + skipped_worker_count
            + skipped_claim_id_count
            + skipped_dedupe_key_count
            + skipped_non_leased_count
            + skipped_expired_count
            + duplicate_dedupe_key_count,
        renewed_count: items.len(),
        skipped_worker_count,
        skipped_claim_id_count,
        skipped_dedupe_key_count,
        skipped_non_leased_count,
        skipped_expired_count,
        duplicate_dedupe_key_count,
        items,
        claim_out: None,
    };
    report.claim_out = write_identity_dispatch_renewal(&report, claim_out, append_claim).await?;

    Ok(JsonResponse::ok(report))
}

pub async fn reconcile_identity_dispatch_manifest(
    asset_manifest: &Path,
    claim_ledger: Option<&Path>,
    completion_ledger: Option<&Path>,
    asset_manifest_out: Option<&Path>,
) -> Result<JsonResponse> {
    if claim_ledger.is_none() && completion_ledger.is_none() {
        bail!("identity-dispatch-reconcile requires --claim-ledger or --completion-ledger");
    }

    let text = tokio::fs::read_to_string(asset_manifest)
        .await
        .with_context(|| format!("failed to read asset manifest {}", asset_manifest.display()))?;
    let mut manifest = serde_json::from_str::<Value>(&text).with_context(|| {
        format!(
            "failed to parse asset manifest {}",
            asset_manifest.display()
        )
    })?;
    let asset_count = identity_plan_manifest_entries(&manifest)?.len();
    let generated_at = unix_seconds();
    let claim_items = read_identity_dispatch_claim_ledger(claim_ledger).await?;
    let completion_items = read_identity_dispatch_completion_ledger(completion_ledger).await?;
    let (active_lease_count, expired_lease_count) =
        identity_dispatch_active_expired_counts(&claim_items, generated_at);
    let events =
        build_identity_dispatch_reconcile_events(&claim_items, &completion_items, generated_at);
    let (updates, unmatched_event_count) =
        apply_identity_dispatch_reconcile_events(&mut manifest, &events)?;
    let state_counts = count_identity_plan_manifest_states(&manifest)?;
    let dispatch_state_counts = count_identity_dispatch_manifest_states(&manifest)?;
    let updated_asset_count = updates
        .iter()
        .map(|update| update.asset_index)
        .collect::<BTreeSet<_>>()
        .len();
    let asset_manifest_out_report = write_identity_dispatch_reconcile_manifest(
        asset_manifest,
        asset_manifest_out,
        &manifest,
        asset_count,
        updated_asset_count,
        unmatched_event_count,
        state_counts.clone(),
        dispatch_state_counts.clone(),
    )
    .await?;

    let report = IdentityDispatchReconcileReport {
        scope: "identity_dispatch_reconcile".to_string(),
        asset_manifest: asset_manifest.display().to_string(),
        generated_at_unix_seconds: generated_at,
        claim_ledger: claim_ledger.map(|path| path.display().to_string()),
        completion_ledger: completion_ledger.map(|path| path.display().to_string()),
        asset_count,
        claim_item_count: claim_items.len(),
        completion_item_count: completion_items.len(),
        reconciled_event_count: events.len(),
        updated_asset_count,
        unmatched_event_count,
        active_lease_count,
        expired_lease_count,
        completion_event_count: completion_items.len(),
        state_counts,
        dispatch_state_counts,
        updates,
        asset_manifest_out: asset_manifest_out_report,
    };

    Ok(JsonResponse::ok(report))
}

pub async fn validate_identity_assets(
    asset_manifest: &Path,
    strict: bool,
    validate_out: Option<&Path>,
) -> Result<JsonResponse> {
    let text = tokio::fs::read_to_string(asset_manifest)
        .await
        .with_context(|| format!("failed to read asset manifest {}", asset_manifest.display()))?;
    let manifest = serde_json::from_str::<Value>(&text).with_context(|| {
        format!(
            "failed to parse asset manifest {}",
            asset_manifest.display()
        )
    })?;
    let generated_at = unix_seconds();
    let entries = identity_plan_manifest_entries(&manifest)?;
    let asset_count = entries.len();
    let mut issues = Vec::new();
    let mut duplicate_keys = BTreeMap::<String, BTreeMap<String, Vec<usize>>>::new();

    for (asset_index, entry) in entries.iter().enumerate() {
        if !entry.is_object() {
            push_identity_asset_validation_issue(
                &mut issues,
                "error",
                "asset_not_object",
                Some(asset_index),
                None,
                Some(entry.clone()),
                "asset manifest entries must be JSON objects",
            );
            continue;
        }

        for field in [
            "accountId",
            "profileId",
            "identityId",
            "label",
            "profileDir",
        ] {
            if let Some(value) = identity_asset_validate_key_value(entry, field) {
                duplicate_keys
                    .entry(field.to_string())
                    .or_default()
                    .entry(value)
                    .or_default()
                    .push(asset_index);
            }
        }

        if identity_plan_manifest_entry_match_keys(entry).is_empty() {
            push_identity_asset_validation_issue(
                &mut issues,
                "error",
                "missing_match_key",
                Some(asset_index),
                None,
                None,
                "asset must include at least one stable match key: accountId, profileId, identityId, label, or profileDir",
            );
        }
        if identity_asset_profile_dir(entry).is_none() {
            push_identity_asset_validation_issue(
                &mut issues,
                "warning",
                "missing_profile_dir",
                Some(asset_index),
                Some("profileDir"),
                None,
                "asset has no profileDir/profilePath/userDataDir and cannot be selected by default",
            );
        }

        match identity_asset_field_string(entry, &["state"]) {
            Some(state) if identity_asset_known_state(&state) => {}
            Some(state) => push_identity_asset_validation_issue(
                &mut issues,
                "warning",
                "unknown_state",
                Some(asset_index),
                Some("state"),
                Some(Value::String(state)),
                "asset state is not one of the known lifecycle states",
            ),
            None => push_identity_asset_validation_issue(
                &mut issues,
                "warning",
                "missing_state",
                Some(asset_index),
                Some("state"),
                None,
                "asset has no state and will be treated as unknown by selection",
            ),
        }

        let runtime_state =
            identity_asset_field_string(entry, &["runtimeLeaseState", "runtime_lease_state"]);
        if let Some(state) = runtime_state.as_deref() {
            if !identity_asset_known_runtime_lease_state(state) {
                push_identity_asset_validation_issue(
                    &mut issues,
                    "warning",
                    "unknown_runtime_lease_state",
                    Some(asset_index),
                    Some("runtimeLeaseState"),
                    Some(Value::String(state.to_string())),
                    "runtimeLeaseState is not a known runtime lease state",
                );
            }
        }
        let runtime_expires = validate_identity_asset_timestamp(
            &mut issues,
            asset_index,
            entry,
            &[
                "runtimeLeaseExpiresUnixSeconds",
                "runtime_lease_expires_unix_seconds",
            ],
        );
        if runtime_state
            .as_deref()
            .map(|state| state.eq_ignore_ascii_case("leased"))
            .unwrap_or(false)
        {
            if identity_asset_field_string(entry, &["runtimeLeaseId", "runtime_lease_id"]).is_none()
            {
                push_identity_asset_validation_issue(
                    &mut issues,
                    "warning",
                    "runtime_lease_missing_id",
                    Some(asset_index),
                    Some("runtimeLeaseId"),
                    None,
                    "leased runtime asset has no runtimeLeaseId",
                );
            }
            match runtime_expires {
                Some(expires) if expires <= generated_at => push_identity_asset_validation_issue(
                    &mut issues,
                    "warning",
                    "runtime_lease_expired",
                    Some(asset_index),
                    Some("runtimeLeaseExpiresUnixSeconds"),
                    Some(json!(expires)),
                    "runtime lease is expired and should be swept or released",
                ),
                Some(_) => {}
                None => push_identity_asset_validation_issue(
                    &mut issues,
                    "warning",
                    "runtime_lease_missing_expires",
                    Some(asset_index),
                    Some("runtimeLeaseExpiresUnixSeconds"),
                    None,
                    "leased runtime asset has no runtime lease expiration",
                ),
            }
        }

        let dispatch_state =
            identity_asset_field_string(entry, &["dispatchState", "dispatch_state"]);
        if let Some(state) = dispatch_state.as_deref() {
            if !identity_asset_known_dispatch_state(state) {
                push_identity_asset_validation_issue(
                    &mut issues,
                    "warning",
                    "unknown_dispatch_state",
                    Some(asset_index),
                    Some("dispatchState"),
                    Some(Value::String(state.to_string())),
                    "dispatchState is not a known dispatch state",
                );
            }
        }
        let dispatch_lease_expires = validate_identity_asset_timestamp(
            &mut issues,
            asset_index,
            entry,
            &[
                "lastDispatchLeaseExpiresUnixSeconds",
                "last_dispatch_lease_expires_unix_seconds",
            ],
        );
        if dispatch_state
            .as_deref()
            .map(|state| state.eq_ignore_ascii_case("leased"))
            .unwrap_or(false)
        {
            match dispatch_lease_expires {
                Some(expires) if expires <= generated_at => push_identity_asset_validation_issue(
                    &mut issues,
                    "warning",
                    "dispatch_lease_expired",
                    Some(asset_index),
                    Some("lastDispatchLeaseExpiresUnixSeconds"),
                    Some(json!(expires)),
                    "dispatch lease is expired and should be reconciled or swept",
                ),
                Some(_) => {}
                None => push_identity_asset_validation_issue(
                    &mut issues,
                    "warning",
                    "dispatch_lease_missing_expires",
                    Some(asset_index),
                    Some("lastDispatchLeaseExpiresUnixSeconds"),
                    None,
                    "leased dispatch asset has no dispatch lease expiration",
                ),
            }
        }

        let retry_after = validate_identity_asset_timestamp(
            &mut issues,
            asset_index,
            entry,
            &[
                "lastDispatchRetryAfterUnixSeconds",
                "last_dispatch_retry_after_unix_seconds",
            ],
        );
        if dispatch_state
            .as_deref()
            .map(|state| state.eq_ignore_ascii_case("retry"))
            .unwrap_or(false)
            && retry_after.is_none()
        {
            push_identity_asset_validation_issue(
                &mut issues,
                "warning",
                "dispatch_retry_missing_after",
                Some(asset_index),
                Some("lastDispatchRetryAfterUnixSeconds"),
                None,
                "retry dispatch asset has no retry-after timestamp",
            );
        }

        if let Some(cooldown_until) = validate_identity_asset_timestamp(
            &mut issues,
            asset_index,
            entry,
            &[
                "cooldownUntilUnixSeconds",
                "cooldown_until_unix_seconds",
                "nextAvailableUnixSeconds",
                "next_available_unix_seconds",
            ],
        ) {
            if cooldown_until <= generated_at {
                push_identity_asset_validation_issue(
                    &mut issues,
                    "warning",
                    "cooldown_expired",
                    Some(asset_index),
                    Some("cooldownUntilUnixSeconds"),
                    Some(json!(cooldown_until)),
                    "cooldown is expired and can be cleared by identity-assets-sweep",
                );
            }
        }
    }

    for (field, values) in duplicate_keys {
        for (value, indexes) in values {
            if indexes.len() < 2 {
                continue;
            }
            push_identity_asset_validation_issue(
                &mut issues,
                "error",
                identity_asset_duplicate_issue_code(&field),
                None,
                Some(&field),
                Some(json!({
                    "value": value,
                    "assetIndexes": indexes,
                })),
                "asset manifest contains duplicate stable match keys",
            );
        }
    }

    let mut issue_code_counts = BTreeMap::new();
    let mut error_count = 0usize;
    let mut warning_count = 0usize;
    let mut info_count = 0usize;
    for issue in &issues {
        *issue_code_counts.entry(issue.code.clone()).or_insert(0) += 1;
        match issue.severity.as_str() {
            "error" => error_count += 1,
            "warning" => warning_count += 1,
            _ => info_count += 1,
        }
    }
    let state_counts = count_identity_plan_manifest_states(&manifest)?;
    let dispatch_state_counts = count_identity_dispatch_manifest_states(&manifest)?;
    let runtime_lease_state_counts = count_identity_asset_runtime_lease_states(&manifest)?;
    let duplicate_key_count = issues
        .iter()
        .filter(|issue| issue.code.starts_with("duplicate_"))
        .count();
    let mut report = IdentityAssetsValidateReport {
        scope: "identity_assets_validate".to_string(),
        asset_manifest: asset_manifest.display().to_string(),
        generated_at_unix_seconds: generated_at,
        strict,
        manifest_version: identity_asset_manifest_version(&manifest),
        valid: error_count == 0,
        asset_count,
        issue_count: issues.len(),
        error_count,
        warning_count,
        info_count,
        duplicate_key_count,
        state_counts,
        dispatch_state_counts,
        runtime_lease_state_counts,
        issue_code_counts,
        issues,
        validate_out: None,
    };
    report.validate_out = write_identity_asset_validate_report(&report, validate_out).await?;

    Ok(JsonResponse::ok(report))
}

pub async fn status_identity_assets(
    asset_manifest: &Path,
    allow_states: &[String],
    desired_concurrency: Option<usize>,
    include_dispatch_leased: bool,
    include_retry: bool,
    include_failed: bool,
    include_cancelled: bool,
    include_runtime_leased: bool,
    include_missing_profile_dir: bool,
    status_out: Option<&Path>,
) -> Result<JsonResponse> {
    let text = tokio::fs::read_to_string(asset_manifest)
        .await
        .with_context(|| format!("failed to read asset manifest {}", asset_manifest.display()))?;
    let manifest = serde_json::from_str::<Value>(&text).with_context(|| {
        format!(
            "failed to parse asset manifest {}",
            asset_manifest.display()
        )
    })?;
    let generated_at = unix_seconds();
    let allowed_states = normalize_identity_asset_allowed_states(allow_states);
    let entries = identity_plan_manifest_entries(&manifest)?;
    let asset_count = entries.len();
    let state_counts = count_identity_plan_manifest_states(&manifest)?;
    let dispatch_state_counts = count_identity_dispatch_manifest_states(&manifest)?;
    let runtime_lease_state_counts = count_identity_asset_runtime_lease_states(&manifest)?;
    let mut runnable_assets = Vec::new();
    let mut blocked_assets = Vec::new();
    let mut block_reason_counts = BTreeMap::new();
    let mut active_runtime_lease_count = 0usize;
    let mut expired_runtime_lease_count = 0usize;
    let mut active_dispatch_lease_count = 0usize;
    let mut expired_dispatch_lease_count = 0usize;
    let mut active_cooldown_count = 0usize;
    let mut expired_cooldown_count = 0usize;
    let mut dispatch_retry_waiting_count = 0usize;
    let mut dispatch_retry_ready_count = 0usize;
    let mut missing_profile_dir_count = 0usize;

    for (asset_index, entry) in entries.iter().enumerate() {
        if identity_asset_profile_dir(entry).is_none() {
            missing_profile_dir_count += 1;
        }
        if identity_asset_runtime_lease_active(entry, generated_at) {
            active_runtime_lease_count += 1;
        }
        if identity_asset_runtime_lease_expired(entry, generated_at) {
            expired_runtime_lease_count += 1;
        }
        if identity_asset_dispatch_lease_active_state(entry, generated_at) {
            active_dispatch_lease_count += 1;
        }
        if identity_asset_dispatch_lease_expired(entry, generated_at) {
            expired_dispatch_lease_count += 1;
        }
        if identity_asset_cooldown_active(entry, generated_at) {
            active_cooldown_count += 1;
        }
        if identity_asset_cooldown_expired(entry, generated_at) {
            expired_cooldown_count += 1;
        }
        if identity_asset_dispatch_state_is(entry, "retry") {
            if identity_asset_dispatch_retry_waiting(entry, generated_at) {
                dispatch_retry_waiting_count += 1;
            } else {
                dispatch_retry_ready_count += 1;
            }
        }

        let reasons = identity_asset_selection_block_reasons(
            entry,
            &allowed_states,
            generated_at,
            include_dispatch_leased,
            include_retry,
            include_failed,
            include_cancelled,
            include_runtime_leased,
            include_missing_profile_dir,
        );
        if reasons.is_empty() {
            runnable_assets.push(identity_asset_selection_item(
                asset_index,
                entry,
                Vec::new(),
                None,
            ));
        } else {
            for reason in &reasons {
                *block_reason_counts.entry(reason.clone()).or_insert(0) += 1;
            }
            blocked_assets.push(identity_asset_selection_item(
                asset_index,
                entry,
                reasons,
                None,
            ));
        }
    }

    let runnable_count = runnable_assets.len();
    let blocked_count = blocked_assets.len();
    let capacity_shortage_count = desired_concurrency
        .map(|desired| desired.saturating_sub(runnable_count))
        .unwrap_or(0);
    let capacity_status =
        identity_assets_capacity_status(asset_count, runnable_count, desired_concurrency);
    let recommendations = identity_assets_status_recommendations(
        capacity_shortage_count,
        &block_reason_counts,
        expired_runtime_lease_count,
        expired_dispatch_lease_count,
        expired_cooldown_count,
        active_runtime_lease_count,
        active_dispatch_lease_count,
        active_cooldown_count,
        dispatch_retry_waiting_count,
        missing_profile_dir_count,
    );

    let mut report = IdentityAssetsStatusReport {
        scope: "identity_assets_status".to_string(),
        asset_manifest: asset_manifest.display().to_string(),
        generated_at_unix_seconds: generated_at,
        allowed_states: allowed_states.iter().cloned().collect(),
        desired_concurrency,
        asset_count,
        runnable_count,
        blocked_count,
        capacity_status,
        capacity_shortage_count,
        recommended_limit: runnable_count,
        state_counts,
        dispatch_state_counts,
        runtime_lease_state_counts,
        active_runtime_lease_count,
        expired_runtime_lease_count,
        active_dispatch_lease_count,
        expired_dispatch_lease_count,
        active_cooldown_count,
        expired_cooldown_count,
        dispatch_retry_waiting_count,
        dispatch_retry_ready_count,
        missing_profile_dir_count,
        block_reason_counts,
        recommendations,
        runnable_assets,
        blocked_assets,
        status_out: None,
    };
    report.status_out = write_identity_asset_status_report(&report, status_out).await?;

    Ok(JsonResponse::ok(report))
}

#[allow(clippy::too_many_arguments)]
pub async fn forecast_identity_assets(
    asset_manifest: &Path,
    allow_states: &[String],
    desired_concurrency: Option<usize>,
    horizon_seconds: Option<u64>,
    include_dispatch_leased: bool,
    include_retry: bool,
    include_failed: bool,
    include_cancelled: bool,
    include_runtime_leased: bool,
    include_missing_profile_dir: bool,
    forecast_out: Option<&Path>,
) -> Result<JsonResponse> {
    let text = tokio::fs::read_to_string(asset_manifest)
        .await
        .with_context(|| format!("failed to read asset manifest {}", asset_manifest.display()))?;
    let manifest = serde_json::from_str::<Value>(&text).with_context(|| {
        format!(
            "failed to parse asset manifest {}",
            asset_manifest.display()
        )
    })?;
    let generated_at = unix_seconds();
    let horizon_until = horizon_seconds.map(|seconds| generated_at.saturating_add(seconds));
    let allowed_states = normalize_identity_asset_allowed_states(allow_states);
    let entries = identity_plan_manifest_entries(&manifest)?;
    let asset_count = entries.len();
    let state_counts = count_identity_plan_manifest_states(&manifest)?;
    let dispatch_state_counts = count_identity_dispatch_manifest_states(&manifest)?;
    let runtime_lease_state_counts = count_identity_asset_runtime_lease_states(&manifest)?;
    let mut current_runnable_count = 0usize;
    let mut block_reason_counts = BTreeMap::new();
    let mut recovery_reason_counts = BTreeMap::new();
    let mut recovery_events = Vec::new();
    let mut hard_blocked_assets = Vec::new();

    for (asset_index, entry) in entries.iter().enumerate() {
        let reasons = identity_asset_selection_block_reasons(
            entry,
            &allowed_states,
            generated_at,
            include_dispatch_leased,
            include_retry,
            include_failed,
            include_cancelled,
            include_runtime_leased,
            include_missing_profile_dir,
        );
        if reasons.is_empty() {
            current_runnable_count += 1;
            continue;
        }
        for reason in &reasons {
            *block_reason_counts.entry(reason.clone()).or_insert(0) += 1;
        }

        if let Some(available_at) =
            identity_asset_forecast_available_at(entry, &reasons, generated_at)
        {
            for reason in &reasons {
                *recovery_reason_counts.entry(reason.clone()).or_insert(0) += 1;
            }
            recovery_events.push(identity_asset_forecast_item(
                asset_index,
                entry,
                reasons,
                true,
                Some(available_at),
                generated_at,
            ));
        } else {
            hard_blocked_assets.push(identity_asset_forecast_item(
                asset_index,
                entry,
                reasons,
                false,
                None,
                generated_at,
            ));
        }
    }

    recovery_events.sort_by(|left, right| {
        left.available_at_unix_seconds
            .cmp(&right.available_at_unix_seconds)
            .then_with(|| left.asset_index.cmp(&right.asset_index))
    });
    hard_blocked_assets.sort_by(|left, right| left.asset_index.cmp(&right.asset_index));

    let blocked_count = asset_count.saturating_sub(current_runnable_count);
    let recoverable_count = recovery_events.len();
    let recoverable_within_horizon_count = recovery_events
        .iter()
        .filter(|event| {
            event
                .available_at_unix_seconds
                .zip(horizon_until)
                .map(|(available_at, horizon_until)| available_at <= horizon_until)
                .unwrap_or(true)
        })
        .count();
    let predicted_runnable_count = current_runnable_count + recoverable_within_horizon_count;
    let current_shortage_count = desired_concurrency
        .map(|desired| desired.saturating_sub(current_runnable_count))
        .unwrap_or(0);
    let predicted_shortage_count = desired_concurrency
        .map(|desired| desired.saturating_sub(predicted_runnable_count))
        .unwrap_or(0);
    let capacity_status =
        identity_assets_capacity_status(asset_count, current_runnable_count, desired_concurrency);
    let predicted_capacity_status =
        identity_assets_capacity_status(asset_count, predicted_runnable_count, desired_concurrency);
    let next_recovery_at_unix_seconds = recovery_events
        .first()
        .and_then(|event| event.available_at_unix_seconds);
    let enough_at_unix_seconds = identity_asset_forecast_enough_at(
        current_runnable_count,
        desired_concurrency,
        &recovery_events,
    );

    let mut report = IdentityAssetsForecastReport {
        scope: "identity_assets_forecast".to_string(),
        asset_manifest: asset_manifest.display().to_string(),
        generated_at_unix_seconds: generated_at,
        allowed_states: allowed_states.iter().cloned().collect(),
        desired_concurrency,
        horizon_seconds,
        asset_count,
        current_runnable_count,
        blocked_count,
        recoverable_count,
        recoverable_within_horizon_count,
        hard_blocked_count: hard_blocked_assets.len(),
        predicted_runnable_count,
        current_shortage_count,
        predicted_shortage_count,
        capacity_status,
        predicted_capacity_status,
        next_recovery_at_unix_seconds,
        enough_at_unix_seconds,
        block_reason_counts,
        recovery_reason_counts,
        state_counts,
        dispatch_state_counts,
        runtime_lease_state_counts,
        recovery_events,
        hard_blocked_assets,
        forecast_out: None,
    };
    report.forecast_out = write_identity_asset_forecast_report(&report, forecast_out).await?;

    Ok(JsonResponse::ok(report))
}

#[allow(clippy::too_many_arguments)]
pub async fn gate_identity_assets(
    asset_manifest: &Path,
    desired_concurrency: usize,
    max_wait_seconds: Option<u64>,
    allow_wait: bool,
    allow_states: &[String],
    include_dispatch_leased: bool,
    include_retry: bool,
    include_failed: bool,
    include_cancelled: bool,
    include_runtime_leased: bool,
    include_missing_profile_dir: bool,
    gate_out: Option<&Path>,
) -> Result<JsonResponse> {
    if desired_concurrency == 0 {
        bail!("--desired-concurrency must be greater than 0");
    }

    let forecast_response = forecast_identity_assets(
        asset_manifest,
        allow_states,
        Some(desired_concurrency),
        max_wait_seconds,
        include_dispatch_leased,
        include_retry,
        include_failed,
        include_cancelled,
        include_runtime_leased,
        include_missing_profile_dir,
        None,
    )
    .await?;
    let forecast = forecast_response.data.unwrap_or(Value::Null);
    let generated_at = identity_asset_field_u64(&forecast, &["generatedAtUnixSeconds"])
        .unwrap_or_else(unix_seconds);
    let current_runnable_count =
        identity_asset_field_u64(&forecast, &["currentRunnableCount"]).unwrap_or(0) as usize;
    let predicted_runnable_count =
        identity_asset_field_u64(&forecast, &["predictedRunnableCount"]).unwrap_or(0) as usize;
    let current_shortage_count =
        identity_asset_field_u64(&forecast, &["currentShortageCount"]).unwrap_or(0) as usize;
    let predicted_shortage_count =
        identity_asset_field_u64(&forecast, &["predictedShortageCount"]).unwrap_or(0) as usize;
    let enough_at_unix_seconds = identity_asset_field_u64(&forecast, &["enoughAtUnixSeconds"]);
    let seconds_until_enough = enough_at_unix_seconds.map(|enough_at| {
        if enough_at > generated_at {
            enough_at - generated_at
        } else {
            0
        }
    });
    let wait_is_within_limit = enough_at_unix_seconds
        .zip(max_wait_seconds)
        .map(|(enough_at, max_wait)| enough_at <= generated_at.saturating_add(max_wait))
        .unwrap_or(false);

    let (decision, recommended_action, message, passed) = if current_runnable_count
        >= desired_concurrency
    {
        (
            "run_now",
            "start_workers",
            format!(
                "当前可运行资产 {current_runnable_count} 个,已满足目标并发 {desired_concurrency}。"
            ),
            true,
        )
    } else if wait_is_within_limit {
        let wait_seconds = seconds_until_enough.unwrap_or(0);
        (
            "wait",
            "sleep_until_enough_at",
            format!(
                "当前只可运行 {current_runnable_count} 个,预计等待 {wait_seconds} 秒后可满足目标并发 {desired_concurrency}。"
            ),
            allow_wait,
        )
    } else {
        (
            "insufficient",
            "reduce_concurrency_or_add_assets",
            format!(
                "当前只可运行 {current_runnable_count} 个,预测窗口内最多 {predicted_runnable_count} 个,无法满足目标并发 {desired_concurrency}。"
            ),
            false,
        )
    };
    let exit_code = if passed { 0 } else { 2 };

    let mut report = IdentityAssetsGateReport {
        scope: "identity_assets_gate".to_string(),
        asset_manifest: asset_manifest.display().to_string(),
        generated_at_unix_seconds: generated_at,
        desired_concurrency,
        max_wait_seconds,
        allow_wait,
        passed,
        exit_code,
        decision: decision.to_string(),
        recommended_action: recommended_action.to_string(),
        message,
        current_runnable_count,
        predicted_runnable_count,
        current_shortage_count,
        predicted_shortage_count,
        enough_at_unix_seconds,
        seconds_until_enough,
        forecast,
        gate_out: None,
    };
    report.gate_out = write_identity_asset_gate_report(&report, gate_out).await?;

    Ok(JsonResponse::ok(report))
}

pub async fn select_identity_assets(
    asset_manifest: &Path,
    limit: usize,
    allow_states: &[String],
    worker: Option<&str>,
    job: Option<&str>,
    lease_seconds: u64,
    include_dispatch_leased: bool,
    include_retry: bool,
    include_failed: bool,
    include_cancelled: bool,
    include_runtime_leased: bool,
    include_missing_profile_dir: bool,
    asset_manifest_out: Option<&Path>,
    selection_out: Option<&Path>,
) -> Result<JsonResponse> {
    if limit == 0 {
        bail!("identity-assets-select --limit must be greater than 0");
    }
    if lease_seconds == 0 {
        bail!("identity-assets-select --lease-seconds must be greater than 0");
    }

    let text = tokio::fs::read_to_string(asset_manifest)
        .await
        .with_context(|| format!("failed to read asset manifest {}", asset_manifest.display()))?;
    let mut manifest = serde_json::from_str::<Value>(&text).with_context(|| {
        format!(
            "failed to parse asset manifest {}",
            asset_manifest.display()
        )
    })?;
    let generated_at = unix_seconds();
    let lease_expires = generated_at.saturating_add(lease_seconds);
    let allowed_states = normalize_identity_asset_allowed_states(allow_states);
    let worker_id = worker
        .map(str::trim)
        .filter(|worker| !worker.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            asset_manifest_out
                .is_some()
                .then(|| format!("worker-{}", std::process::id()))
        });
    let job_id = job
        .map(str::trim)
        .filter(|job| !job.is_empty())
        .map(ToString::to_string);
    let reserve = asset_manifest_out.is_some();

    let entries = identity_plan_manifest_entries(&manifest)?;
    let asset_count = entries.len();
    let state_counts = count_identity_plan_manifest_states(&manifest)?;
    let dispatch_state_counts = count_identity_dispatch_manifest_states(&manifest)?;
    let mut selected_assets = Vec::new();
    let mut blocked_assets = Vec::new();
    let mut block_reason_counts = BTreeMap::new();
    let mut overflow_count = 0usize;
    let mut leased_asset_indexes = Vec::new();

    for (asset_index, entry) in entries.iter().enumerate() {
        let mut reasons = identity_asset_selection_block_reasons(
            entry,
            &allowed_states,
            generated_at,
            include_dispatch_leased,
            include_retry,
            include_failed,
            include_cancelled,
            include_runtime_leased,
            include_missing_profile_dir,
        );
        if reasons.is_empty() && selected_assets.len() >= limit {
            reasons.push("limit_reached".to_string());
            overflow_count += 1;
        }

        if reasons.is_empty() {
            let lease_id = reserve.then(|| {
                format!(
                    "asset_lease_{}_{}_{}",
                    generated_at,
                    std::process::id(),
                    asset_index
                )
            });
            selected_assets.push(identity_asset_selection_item(
                asset_index,
                entry,
                Vec::new(),
                lease_id.clone(),
            ));
            if let Some(lease_id) = lease_id {
                leased_asset_indexes.push((asset_index, lease_id));
            }
        } else {
            for reason in &reasons {
                *block_reason_counts.entry(reason.clone()).or_insert(0) += 1;
            }
            blocked_assets.push(identity_asset_selection_item(
                asset_index,
                entry,
                reasons,
                None,
            ));
        }
    }

    if reserve {
        apply_identity_asset_runtime_leases(
            &mut manifest,
            &leased_asset_indexes,
            worker_id.as_deref(),
            job_id.as_deref(),
            generated_at,
            lease_expires,
        )?;
    }

    let asset_manifest_out_report = write_identity_asset_selection_manifest(
        asset_manifest,
        asset_manifest_out,
        &manifest,
        asset_count,
        selected_assets.len(),
    )
    .await?;

    let mut report = IdentityAssetsSelectReport {
        scope: "identity_assets_select".to_string(),
        asset_manifest: asset_manifest.display().to_string(),
        generated_at_unix_seconds: generated_at,
        worker_id,
        job_id,
        lease_seconds,
        lease_expires_unix_seconds: lease_expires,
        allowed_states: allowed_states.iter().cloned().collect(),
        requested_limit: limit,
        asset_count,
        selected_count: selected_assets.len(),
        blocked_count: blocked_assets.len(),
        overflow_count,
        state_counts,
        dispatch_state_counts,
        block_reason_counts,
        selected_assets,
        blocked_assets,
        asset_manifest_out: asset_manifest_out_report,
        selection_out: None,
    };
    report.selection_out = write_identity_asset_selection_report(&report, selection_out).await?;

    Ok(JsonResponse::ok(report))
}

pub async fn release_identity_assets(
    asset_manifest: &Path,
    status: &str,
    worker: Option<&str>,
    job: Option<&str>,
    lease_ids: &[String],
    account_ids: &[String],
    profile_ids: &[String],
    identity_ids: &[String],
    labels: &[String],
    cooldown_seconds: Option<u64>,
    next_state: Option<&str>,
    message: Option<&str>,
    result_json: Option<&str>,
    asset_manifest_out: Option<&Path>,
    release_out: Option<&Path>,
    append_release: bool,
) -> Result<JsonResponse> {
    let status = normalize_identity_asset_release_status(status)?;
    let worker_id = worker
        .map(str::trim)
        .filter(|worker| !worker.is_empty())
        .map(ToString::to_string);
    let job_id = job
        .map(str::trim)
        .filter(|job| !job.is_empty())
        .map(ToString::to_string);
    let lease_filter = normalize_identity_asset_filter_set(lease_ids);
    let account_filter = normalize_identity_asset_filter_set(account_ids);
    let profile_filter = normalize_identity_asset_filter_set(profile_ids);
    let identity_filter = normalize_identity_asset_filter_set(identity_ids);
    let label_filter = normalize_identity_asset_filter_set(labels);
    if worker_id.is_none()
        && job_id.is_none()
        && lease_filter.is_empty()
        && account_filter.is_empty()
        && profile_filter.is_empty()
        && identity_filter.is_empty()
        && label_filter.is_empty()
    {
        bail!(
            "identity-assets-release requires at least one filter: --worker, --job, --lease-id, --account-id, --profile-id, --identity-id, or --label"
        );
    }

    let text = tokio::fs::read_to_string(asset_manifest)
        .await
        .with_context(|| format!("failed to read asset manifest {}", asset_manifest.display()))?;
    let mut manifest = serde_json::from_str::<Value>(&text).with_context(|| {
        format!(
            "failed to parse asset manifest {}",
            asset_manifest.display()
        )
    })?;
    let generated_at = unix_seconds();
    let cooldown_until = cooldown_seconds.map(|seconds| generated_at.saturating_add(seconds));
    let next_state = next_state
        .map(str::trim)
        .filter(|state| !state.is_empty())
        .map(ToString::to_string);
    let message = message
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(ToString::to_string);
    let result = match result_json {
        Some(raw) => Some(
            serde_json::from_str::<Value>(raw)
                .with_context(|| format!("invalid --result-json value: {raw}"))?,
        ),
        None => None,
    };

    let entries = identity_plan_manifest_entries_mut(&mut manifest)?;
    let asset_count = entries.len();
    let mut released_assets = Vec::new();
    let mut skipped_filter_count = 0usize;
    let mut skipped_non_leased_count = 0usize;

    for (asset_index, entry) in entries.iter_mut().enumerate() {
        if !identity_asset_matches_release_filters(
            entry,
            worker_id.as_deref(),
            job_id.as_deref(),
            &lease_filter,
            &account_filter,
            &profile_filter,
            &identity_filter,
            &label_filter,
        ) {
            skipped_filter_count += 1;
            continue;
        }
        if !identity_asset_runtime_lease_present(entry) {
            skipped_non_leased_count += 1;
            continue;
        }
        let release_item = apply_identity_asset_runtime_release(
            asset_index,
            entry,
            &status,
            generated_at,
            cooldown_until,
            next_state.as_deref(),
            message.as_deref(),
            result.as_ref(),
        );
        released_assets.push(release_item);
    }

    let state_counts = count_identity_plan_manifest_states(&manifest)?;
    let runtime_lease_state_counts = count_identity_asset_runtime_lease_states(&manifest)?;
    let asset_manifest_out_report = write_identity_asset_release_manifest(
        asset_manifest,
        asset_manifest_out,
        &manifest,
        asset_count,
        released_assets.len(),
    )
    .await?;

    let mut report = IdentityAssetsReleaseReport {
        scope: "identity_assets_release".to_string(),
        asset_manifest: asset_manifest.display().to_string(),
        generated_at_unix_seconds: generated_at,
        status,
        worker_id,
        job_id,
        filtered_lease_ids: lease_filter.iter().cloned().collect(),
        filtered_account_ids: account_filter.iter().cloned().collect(),
        filtered_profile_ids: profile_filter.iter().cloned().collect(),
        filtered_identity_ids: identity_filter.iter().cloned().collect(),
        filtered_labels: label_filter.iter().cloned().collect(),
        cooldown_until_unix_seconds: cooldown_until,
        next_state,
        message,
        result,
        asset_count,
        matched_count: released_assets.len() + skipped_non_leased_count,
        released_count: released_assets.len(),
        skipped_filter_count,
        skipped_non_leased_count,
        state_counts,
        runtime_lease_state_counts,
        released_assets,
        asset_manifest_out: asset_manifest_out_report,
        release_out: None,
    };
    report.release_out =
        write_identity_asset_release_report(&report, release_out, append_release).await?;

    Ok(JsonResponse::ok(report))
}

pub async fn reconcile_identity_asset_runtime_manifest(
    asset_manifest: &Path,
    release_ledgers: &[PathBuf],
    asset_manifest_out: Option<&Path>,
) -> Result<JsonResponse> {
    if release_ledgers.is_empty() {
        bail!("identity-assets-reconcile-runtime requires at least one --release-ledger");
    }

    let text = tokio::fs::read_to_string(asset_manifest)
        .await
        .with_context(|| format!("failed to read asset manifest {}", asset_manifest.display()))?;
    let mut manifest = serde_json::from_str::<Value>(&text).with_context(|| {
        format!(
            "failed to parse asset manifest {}",
            asset_manifest.display()
        )
    })?;
    let generated_at = unix_seconds();
    let asset_count = identity_plan_manifest_entries(&manifest)?.len();
    let mut events = Vec::new();
    for path in release_ledgers {
        let text = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read release ledger {}", path.display()))?;
        let mut parsed = parse_identity_asset_runtime_release_events(&text, path)
            .with_context(|| format!("failed to parse release ledger {}", path.display()))?;
        events.append(&mut parsed);
    }
    for (event_index, event) in events.iter_mut().enumerate() {
        event.event_index = event_index;
    }
    events.sort_by(|left, right| {
        left.generated_at_unix_seconds
            .cmp(&right.generated_at_unix_seconds)
            .then_with(|| left.event_index.cmp(&right.event_index))
    });

    let (updates, unmatched_event_count) =
        apply_identity_asset_runtime_release_events(&mut manifest, &events, generated_at)?;
    let updated_asset_count = updates
        .iter()
        .map(|update| update.asset_index)
        .collect::<BTreeSet<_>>()
        .len();
    let state_counts = count_identity_plan_manifest_states(&manifest)?;
    let runtime_lease_state_counts = count_identity_asset_runtime_lease_states(&manifest)?;
    let asset_manifest_out_report = write_identity_asset_runtime_reconcile_manifest(
        asset_manifest,
        asset_manifest_out,
        &manifest,
        asset_count,
        updated_asset_count,
        unmatched_event_count,
        state_counts.clone(),
        runtime_lease_state_counts.clone(),
    )
    .await?;

    let report = IdentityAssetsRuntimeReconcileReport {
        scope: "identity_assets_reconcile_runtime".to_string(),
        asset_manifest: asset_manifest.display().to_string(),
        generated_at_unix_seconds: generated_at,
        release_ledgers: release_ledgers
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        asset_count,
        release_event_count: events.len(),
        updated_asset_count,
        unmatched_event_count,
        state_counts,
        runtime_lease_state_counts,
        updates,
        asset_manifest_out: asset_manifest_out_report,
    };

    Ok(JsonResponse::ok(report))
}

#[allow(clippy::too_many_arguments)]
pub async fn health_identity_assets(
    asset_manifest: &Path,
    release_ledgers: &[PathBuf],
    window_seconds: Option<u64>,
    repair_threshold: usize,
    quarantine_threshold: usize,
    cooldown_seconds: u64,
    asset_manifest_out: Option<&Path>,
    health_out: Option<&Path>,
) -> Result<JsonResponse> {
    if release_ledgers.is_empty() {
        bail!("identity-assets-health requires at least one --release-ledger");
    }
    if repair_threshold == 0 {
        bail!("--repair-threshold must be greater than 0");
    }
    if quarantine_threshold == 0 {
        bail!("--quarantine-threshold must be greater than 0");
    }
    if quarantine_threshold < repair_threshold {
        bail!("--quarantine-threshold must be greater than or equal to --repair-threshold");
    }

    let text = tokio::fs::read_to_string(asset_manifest)
        .await
        .with_context(|| format!("failed to read asset manifest {}", asset_manifest.display()))?;
    let mut manifest = serde_json::from_str::<Value>(&text).with_context(|| {
        format!(
            "failed to parse asset manifest {}",
            asset_manifest.display()
        )
    })?;
    let generated_at = unix_seconds();
    let asset_count = identity_plan_manifest_entries(&manifest)?.len();
    let mut events = Vec::new();
    for path in release_ledgers {
        let text = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read release ledger {}", path.display()))?;
        let mut parsed = parse_identity_asset_runtime_release_events(&text, path)
            .with_context(|| format!("failed to parse release ledger {}", path.display()))?;
        events.append(&mut parsed);
    }
    if let Some(window_seconds) = window_seconds {
        let cutoff = generated_at.saturating_sub(window_seconds);
        events.retain(|event| event.generated_at_unix_seconds >= cutoff);
    }
    for (event_index, event) in events.iter_mut().enumerate() {
        event.event_index = event_index;
    }
    events.sort_by(|left, right| {
        left.generated_at_unix_seconds
            .cmp(&right.generated_at_unix_seconds)
            .then_with(|| left.event_index.cmp(&right.event_index))
    });

    let apply_manifest_updates = asset_manifest_out.is_some();
    let health = build_identity_asset_health_report_items(
        &mut manifest,
        &events,
        generated_at,
        repair_threshold,
        quarantine_threshold,
        cooldown_seconds,
        apply_manifest_updates,
    )?;
    let state_counts = count_identity_plan_manifest_states(&manifest)?;
    let runtime_lease_state_counts = count_identity_asset_runtime_lease_states(&manifest)?;
    let asset_manifest_out_report = write_identity_asset_health_manifest(
        asset_manifest,
        asset_manifest_out,
        &manifest,
        asset_count,
        health.updated_asset_count,
        health.action_counts.clone(),
        state_counts.clone(),
        runtime_lease_state_counts.clone(),
    )
    .await?;

    let mut report = IdentityAssetsHealthReport {
        scope: "identity_assets_health".to_string(),
        asset_manifest: asset_manifest.display().to_string(),
        generated_at_unix_seconds: generated_at,
        release_ledgers: release_ledgers
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        window_seconds,
        repair_threshold,
        quarantine_threshold,
        cooldown_seconds,
        asset_count,
        release_event_count: events.len(),
        matched_event_count: health.matched_event_count,
        unmatched_event_count: health.unmatched_event_count,
        healthy_count: health.healthy_count,
        watch_count: health.watch_count,
        degraded_count: health.degraded_count,
        quarantine_count: health.quarantine_count,
        unknown_count: health.unknown_count,
        updated_asset_count: health.updated_asset_count,
        action_counts: health.action_counts,
        state_counts,
        runtime_lease_state_counts,
        assets: health.items,
        asset_manifest_out: asset_manifest_out_report,
        health_out: None,
    };
    report.health_out = write_identity_asset_health_report(&report, health_out).await?;

    Ok(JsonResponse::ok(report))
}

#[allow(clippy::too_many_arguments)]
pub async fn query_identity_ledgers(
    release_ledgers: &[PathBuf],
    runtime_risk_ledgers: &[PathBuf],
    window_seconds: Option<u64>,
    job_filter: Option<&str>,
    worker_filter: Option<&str>,
    reason_filter: Option<&str>,
    top: usize,
    out: Option<&Path>,
) -> Result<JsonResponse> {
    if release_ledgers.is_empty() && runtime_risk_ledgers.is_empty() {
        bail!(
            "identity-ledger query requires at least one --release-ledger or --runtime-risk-ledger"
        );
    }
    if top == 0 {
        bail!("identity-ledger query --top must be greater than 0");
    }

    let generated_at = unix_seconds();
    let cutoff = window_seconds.map(|seconds| generated_at.saturating_sub(seconds));
    let mut release_events = read_identity_ledger_release_events(release_ledgers).await?;
    let release_event_count = release_events.len();
    release_events.retain(|event| {
        identity_ledger_release_event_matches_filters(
            event,
            cutoff,
            job_filter,
            worker_filter,
            reason_filter,
        )
    });

    let mut runtime_risk_events =
        read_identity_ledger_runtime_risk_events(runtime_risk_ledgers).await?;
    let runtime_risk_event_count = runtime_risk_events.len();
    let mut active_suppression_count = 0usize;
    let mut expired_suppression_count = 0usize;
    runtime_risk_events.retain(|event| {
        identity_ledger_risk_event_matches_filters(
            event,
            cutoff,
            generated_at,
            job_filter,
            worker_filter,
            reason_filter,
            &mut active_suppression_count,
            &mut expired_suppression_count,
        )
    });

    let mut release_status_counts = BTreeMap::new();
    let mut release_failure_reason_counts = BTreeMap::new();
    let mut release_job_counts = BTreeMap::new();
    let mut release_worker_counts = BTreeMap::new();
    let mut release_asset_stats = BTreeMap::<String, IdentityLedgerAssetStats>::new();
    for event in &release_events {
        increment_identity_ledger_count(&mut release_status_counts, event.status.as_str());
        if let Some(job_id) = event.item.job_id.as_ref().or(event.job_id.as_ref()) {
            increment_identity_ledger_count(&mut release_job_counts, job_id);
        }
        if let Some(worker_id) = event.item.worker_id.as_ref().or(event.worker_id.as_ref()) {
            increment_identity_ledger_count(&mut release_worker_counts, worker_id);
        }
        let failure_reason = identity_ledger_release_failure_reason(event);
        if let Some(reason) = failure_reason.as_deref() {
            increment_identity_ledger_count(&mut release_failure_reason_counts, reason);
        } else if event.status != "succeeded" {
            increment_identity_ledger_count(&mut release_failure_reason_counts, "unknown");
        }

        let asset_key = identity_ledger_release_asset_key(event);
        release_asset_stats
            .entry(asset_key.clone())
            .or_insert_with(|| IdentityLedgerAssetStats::new(asset_key))
            .record_release(event, failure_reason.as_deref());
    }

    let mut risk_action_counts = BTreeMap::new();
    let mut risk_severity_counts = BTreeMap::new();
    let mut risk_failure_reason_counts = BTreeMap::new();
    let mut risk_job_counts = BTreeMap::new();
    let mut risk_worker_counts = BTreeMap::new();
    let mut active_suppressions = Vec::new();
    for event in &runtime_risk_events {
        if let Some(action) = identity_asset_field_string(event, &["recommendedAction"]) {
            increment_identity_ledger_count(&mut risk_action_counts, &action);
        }
        if let Some(severity) = identity_asset_field_string(event, &["severity"]) {
            increment_identity_ledger_count(&mut risk_severity_counts, &severity);
        }
        if let Some(job_id) = identity_asset_field_string(event, &["jobId", "job_id"]) {
            increment_identity_ledger_count(&mut risk_job_counts, &job_id);
        }
        if let Some(worker_id) = identity_asset_field_string(event, &["workerId", "worker_id"]) {
            increment_identity_ledger_count(&mut risk_worker_counts, &worker_id);
        }
        if let Some(reason) = identity_ledger_risk_failure_reason(event) {
            increment_identity_ledger_count(&mut risk_failure_reason_counts, &reason);
        }
        if identity_asset_field_u64(event, &["suppressUntilUnixSeconds"])
            .map(|suppress_until| suppress_until > generated_at)
            .unwrap_or(false)
        {
            active_suppressions.push(identity_ledger_risk_suppression_summary(event));
        }
    }

    let latest_release_event = release_events
        .last()
        .map(identity_ledger_release_event_summary);
    let latest_runtime_risk_event = runtime_risk_events.last().cloned();
    let mut report = json!({
        "scope": "identity_ledger_query",
        "generatedAtUnixSeconds": generated_at,
        "windowSeconds": window_seconds,
        "filters": {
            "job": job_filter,
            "worker": worker_filter,
            "reason": reason_filter,
            "top": top,
        },
        "releaseLedgers": release_ledgers
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "runtimeRiskLedgers": runtime_risk_ledgers
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "releaseEventCount": release_events.len(),
        "releaseEventReadCount": release_event_count,
        "runtimeRiskEventCount": runtime_risk_events.len(),
        "runtimeRiskEventReadCount": runtime_risk_event_count,
        "releaseStatusCounts": release_status_counts,
        "failureReasonCounts": release_failure_reason_counts,
        "releaseJobCounts": release_job_counts,
        "releaseWorkerCounts": release_worker_counts,
        "runtimeRiskActionCounts": risk_action_counts,
        "runtimeRiskSeverityCounts": risk_severity_counts,
        "runtimeRiskFailureReasonCounts": risk_failure_reason_counts,
        "runtimeRiskJobCounts": risk_job_counts,
        "runtimeRiskWorkerCounts": risk_worker_counts,
        "activeSuppressionCount": active_suppression_count,
        "expiredSuppressionCount": expired_suppression_count,
        "activeSuppressions": active_suppressions,
        "topFailureReasons": identity_ledger_top_counts_json(&release_failure_reason_counts, top),
        "topRuntimeRiskActions": identity_ledger_top_counts_json(&risk_action_counts, top),
        "topAssets": identity_ledger_top_asset_stats_json(&release_asset_stats, top),
        "latestReleaseEvent": latest_release_event,
        "latestRuntimeRiskEvent": latest_runtime_risk_event,
        "recommendations": identity_ledger_query_recommendations(
            &release_failure_reason_counts,
            &risk_action_counts,
            active_suppression_count,
        ),
    });

    if let Some(out) = out {
        let out_report = write_identity_ledger_query_report(out, &report).await?;
        report["out"] = out_report;
    }

    Ok(JsonResponse::ok(report))
}

#[allow(clippy::too_many_arguments)]
pub async fn compact_identity_ledgers(
    release_ledgers: &[PathBuf],
    runtime_risk_ledgers: &[PathBuf],
    window_seconds: Option<u64>,
    job_filter: Option<&str>,
    worker_filter: Option<&str>,
    reason_filter: Option<&str>,
    retain_recent: usize,
    top: usize,
    checkpoint_in: Option<&Path>,
    checkpoint_out: Option<&Path>,
    out: Option<&Path>,
) -> Result<JsonResponse> {
    if release_ledgers.is_empty() && runtime_risk_ledgers.is_empty() {
        bail!(
            "identity-ledger compact requires at least one --release-ledger or --runtime-risk-ledger"
        );
    }
    if top == 0 {
        bail!("identity-ledger compact --top must be greater than 0");
    }

    let generated_at = unix_seconds();
    let cutoff = window_seconds.map(|seconds| generated_at.saturating_sub(seconds));
    let checkpoint = read_identity_ledger_checkpoint(checkpoint_in).await?;
    let release_read =
        read_identity_ledger_release_events_incremental(release_ledgers, &checkpoint.offsets)
            .await?;
    let mut release_events = release_read.events;
    let release_event_read_count = release_events.len();
    release_events.retain(|event| {
        identity_ledger_release_event_matches_filters(
            event,
            cutoff,
            job_filter,
            worker_filter,
            reason_filter,
        )
    });

    let risk_read = read_identity_ledger_runtime_risk_events_incremental(
        runtime_risk_ledgers,
        &checkpoint.offsets,
    )
    .await?;
    let mut runtime_risk_events = risk_read.events;
    let runtime_risk_event_read_count = runtime_risk_events.len();
    let mut active_suppression_count = 0usize;
    let mut expired_suppression_count = 0usize;
    runtime_risk_events.retain(|event| {
        identity_ledger_risk_event_matches_filters(
            event,
            cutoff,
            generated_at,
            job_filter,
            worker_filter,
            reason_filter,
            &mut active_suppression_count,
            &mut expired_suppression_count,
        )
    });

    let mut release_status_counts = BTreeMap::new();
    let mut release_failure_reason_counts = BTreeMap::new();
    let mut release_job_counts = BTreeMap::new();
    let mut release_worker_counts = BTreeMap::new();
    let mut release_asset_stats = BTreeMap::<String, IdentityLedgerAssetStats>::new();
    for event in &release_events {
        increment_identity_ledger_count(&mut release_status_counts, event.status.as_str());
        if let Some(job_id) = event.item.job_id.as_ref().or(event.job_id.as_ref()) {
            increment_identity_ledger_count(&mut release_job_counts, job_id);
        }
        if let Some(worker_id) = event.item.worker_id.as_ref().or(event.worker_id.as_ref()) {
            increment_identity_ledger_count(&mut release_worker_counts, worker_id);
        }
        let failure_reason = identity_ledger_release_failure_reason(event);
        if let Some(reason) = failure_reason.as_deref() {
            increment_identity_ledger_count(&mut release_failure_reason_counts, reason);
        } else if event.status != "succeeded" {
            increment_identity_ledger_count(&mut release_failure_reason_counts, "unknown");
        }
        let asset_key = identity_ledger_release_asset_key(event);
        release_asset_stats
            .entry(asset_key.clone())
            .or_insert_with(|| IdentityLedgerAssetStats::new(asset_key))
            .record_release(event, failure_reason.as_deref());
    }

    let mut risk_action_counts = BTreeMap::new();
    let mut risk_severity_counts = BTreeMap::new();
    let mut risk_failure_reason_counts = BTreeMap::new();
    let mut risk_job_counts = BTreeMap::new();
    let mut risk_worker_counts = BTreeMap::new();
    let mut active_suppressions = Vec::new();
    for event in &runtime_risk_events {
        if let Some(action) = identity_asset_field_string(event, &["recommendedAction"]) {
            increment_identity_ledger_count(&mut risk_action_counts, &action);
        }
        if let Some(severity) = identity_asset_field_string(event, &["severity"]) {
            increment_identity_ledger_count(&mut risk_severity_counts, &severity);
        }
        if let Some(job_id) = identity_asset_field_string(event, &["jobId", "job_id"]) {
            increment_identity_ledger_count(&mut risk_job_counts, &job_id);
        }
        if let Some(worker_id) = identity_asset_field_string(event, &["workerId", "worker_id"]) {
            increment_identity_ledger_count(&mut risk_worker_counts, &worker_id);
        }
        if let Some(reason) = identity_ledger_risk_failure_reason(event) {
            increment_identity_ledger_count(&mut risk_failure_reason_counts, &reason);
        }
        if identity_asset_field_u64(
            event,
            &["suppressUntilUnixSeconds", "suppress_until_unix_seconds"],
        )
        .map(|suppress_until| suppress_until > generated_at)
        .unwrap_or(false)
        {
            active_suppressions.push(identity_ledger_risk_suppression_summary(event));
        }
    }
    let carried_active_suppression_count = carry_identity_ledger_active_suppressions(
        &mut active_suppressions,
        &checkpoint.active_suppressions,
        generated_at,
        job_filter,
        worker_filter,
        reason_filter,
        &mut risk_action_counts,
        &mut risk_severity_counts,
        &mut risk_failure_reason_counts,
    );
    active_suppression_count = active_suppressions.len();

    let retained_release_evidence = release_events
        .iter()
        .rev()
        .take(retain_recent)
        .map(identity_ledger_release_event_summary)
        .collect::<Vec<_>>();
    let retained_runtime_risk_evidence = runtime_risk_events
        .iter()
        .rev()
        .take(retain_recent)
        .map(identity_ledger_risk_event_summary)
        .collect::<Vec<_>>();
    let retained_evidence_count =
        retained_release_evidence.len() + retained_runtime_risk_evidence.len();
    let compacted_through_unix_seconds = identity_ledger_high_watermark(
        release_events
            .iter()
            .map(|event| event.generated_at_unix_seconds),
        runtime_risk_events
            .iter()
            .filter_map(|event| identity_asset_field_u64(event, &["generatedAtUnixSeconds"])),
    );
    let next_suppression_until = active_suppressions
        .iter()
        .filter_map(|event| {
            identity_asset_field_u64(
                event,
                &["suppressUntilUnixSeconds", "suppress_until_unix_seconds"],
            )
        })
        .max();
    let source_checkpoints = release_read
        .source_checkpoints
        .into_iter()
        .chain(risk_read.source_checkpoints.into_iter())
        .collect::<Vec<_>>();

    let mut report = json!({
        "scope": "identity_ledger_compact",
        "generatedAtUnixSeconds": generated_at,
        "compactedThroughUnixSeconds": compacted_through_unix_seconds,
        "checkpointIn": checkpoint_in.map(|path| path.display().to_string()),
        "incremental": checkpoint_in.is_some(),
        "windowSeconds": window_seconds,
        "filters": {
            "job": job_filter,
            "worker": worker_filter,
            "reason": reason_filter,
            "retainRecent": retain_recent,
            "top": top,
        },
        "releaseLedgers": release_ledgers
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "runtimeRiskLedgers": runtime_risk_ledgers
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "releaseEventReadCount": release_event_read_count,
        "releaseEventCount": release_events.len(),
        "runtimeRiskEventReadCount": runtime_risk_event_read_count,
        "runtimeRiskEventCount": runtime_risk_events.len(),
        "sourceEventCount": release_event_read_count + runtime_risk_event_read_count,
        "compactedEventCount": release_events.len() + runtime_risk_events.len(),
        "retainedEvidenceCount": retained_evidence_count,
        "sourceCheckpoints": source_checkpoints,
        "releaseStatusCounts": release_status_counts,
        "failureReasonCounts": release_failure_reason_counts,
        "releaseJobCounts": release_job_counts,
        "releaseWorkerCounts": release_worker_counts,
        "runtimeRiskActionCounts": risk_action_counts,
        "runtimeRiskSeverityCounts": risk_severity_counts,
        "runtimeRiskFailureReasonCounts": risk_failure_reason_counts,
        "runtimeRiskJobCounts": risk_job_counts,
        "runtimeRiskWorkerCounts": risk_worker_counts,
        "activeSuppressionCount": active_suppression_count,
        "expiredSuppressionCount": expired_suppression_count,
        "carriedActiveSuppressionCount": carried_active_suppression_count,
        "activeSuppressions": active_suppressions,
        "nextSuppressionUntilUnixSeconds": next_suppression_until,
        "assetSummaryCount": release_asset_stats.len(),
        "assetSummaries": identity_ledger_all_asset_stats_json(&release_asset_stats),
        "topAssets": identity_ledger_top_asset_stats_json(&release_asset_stats, top),
        "topFailureReasons": identity_ledger_top_counts_json(&release_failure_reason_counts, top),
        "topRuntimeRiskActions": identity_ledger_top_counts_json(&risk_action_counts, top),
        "retainedReleaseEvidence": retained_release_evidence,
        "retainedRuntimeRiskEvidence": retained_runtime_risk_evidence,
        "recommendations": identity_ledger_compact_recommendations(
            active_suppression_count,
            release_event_read_count + runtime_risk_event_read_count,
            release_events.len() + runtime_risk_events.len(),
            retained_evidence_count,
        ),
    });

    if let Some(out) = out {
        let out_report = write_identity_ledger_compact_report(out, &report).await?;
        report["out"] = out_report;
    }
    if let Some(checkpoint_out) = checkpoint_out {
        let checkpoint_out_report =
            write_identity_ledger_checkpoint_report(checkpoint_out, &report).await?;
        report["checkpointOut"] = checkpoint_out_report;
    }

    Ok(JsonResponse::ok(report))
}

#[allow(clippy::too_many_arguments)]
pub async fn dashboard_identity_ledgers(
    release_ledgers: &[PathBuf],
    runtime_risk_ledgers: &[PathBuf],
    window_seconds: Option<u64>,
    job_filter: Option<&str>,
    worker_filter: Option<&str>,
    reason_filter: Option<&str>,
    retain_recent: usize,
    top: usize,
    checkpoint_in: Option<&Path>,
    checkpoint_out: Option<&Path>,
    out: Option<&Path>,
    html_out: Option<&Path>,
) -> Result<JsonResponse> {
    let compact_response = compact_identity_ledgers(
        release_ledgers,
        runtime_risk_ledgers,
        window_seconds,
        job_filter,
        worker_filter,
        reason_filter,
        retain_recent,
        top,
        checkpoint_in,
        checkpoint_out,
        None,
    )
    .await?;
    let compact = compact_response.data.unwrap_or(Value::Null);
    let summary = identity_ledger_dashboard_summary(&compact);
    let generated_at =
        identity_ledger_value_u64(&compact, "generatedAtUnixSeconds").unwrap_or_else(unix_seconds);

    let mut report = json!({
        "scope": "identity_ledger_dashboard",
        "generatedAtUnixSeconds": generated_at,
        "windowSeconds": window_seconds,
        "filters": {
            "job": job_filter,
            "worker": worker_filter,
            "reason": reason_filter,
            "retainRecent": retain_recent,
            "top": top,
        },
        "summary": summary,
        "compact": compact,
    });

    if let Some(out) = out {
        let out_report = write_identity_ledger_dashboard_report(out, &report).await?;
        report["out"] = out_report;
    }
    if let Some(html_out) = html_out {
        let html_report = write_identity_ledger_dashboard_html(html_out, &report).await?;
        report["htmlOut"] = html_report;
    }
    if let Some(checkpoint_out) = report
        .get("compact")
        .and_then(|compact| compact.get("checkpointOut"))
        .cloned()
    {
        report["checkpointOut"] = checkpoint_out;
    }

    Ok(JsonResponse::ok(report))
}

#[allow(clippy::too_many_arguments)]
pub async fn explain_identity_ledger(
    release_ledgers: &[PathBuf],
    runtime_risk_ledgers: &[PathBuf],
    window_seconds: Option<u64>,
    job_filter: Option<&str>,
    worker_filter: Option<&str>,
    reason_filter: Option<&str>,
    account_id_filter: Option<&str>,
    profile_id_filter: Option<&str>,
    identity_id_filter: Option<&str>,
    label_filter: Option<&str>,
    profile_dir_filter: Option<&str>,
    lease_id_filter: Option<&str>,
    evidence_limit: usize,
    out: Option<&Path>,
) -> Result<JsonResponse> {
    if release_ledgers.is_empty() && runtime_risk_ledgers.is_empty() {
        bail!(
            "identity-ledger explain requires at least one --release-ledger or --runtime-risk-ledger"
        );
    }
    if evidence_limit == 0 {
        bail!("identity-ledger explain --evidence-limit must be greater than 0");
    }

    let generated_at = unix_seconds();
    let cutoff = window_seconds.map(|seconds| generated_at.saturating_sub(seconds));
    let mut release_events = read_identity_ledger_release_events(release_ledgers).await?;
    let release_event_read_count = release_events.len();
    release_events.retain(|event| {
        identity_ledger_release_event_matches_filters(
            event,
            cutoff,
            job_filter,
            worker_filter,
            reason_filter,
        ) && identity_ledger_release_event_matches_asset_filters(
            event,
            account_id_filter,
            profile_id_filter,
            identity_id_filter,
            label_filter,
            profile_dir_filter,
            lease_id_filter,
        )
    });

    let mut release_status_counts = BTreeMap::new();
    let mut release_failure_reason_counts = BTreeMap::new();
    let mut observed_failure_reasons = BTreeSet::new();
    let mut active_cooldowns = Vec::new();
    for event in &release_events {
        increment_identity_ledger_count(&mut release_status_counts, event.status.as_str());
        let failure_reason = identity_ledger_release_failure_reason(event);
        if let Some(reason) = failure_reason.as_deref() {
            observed_failure_reasons.insert(reason.to_string());
            increment_identity_ledger_count(&mut release_failure_reason_counts, reason);
        } else if event.status != "succeeded" {
            increment_identity_ledger_count(&mut release_failure_reason_counts, "unknown");
        }
        if event
            .cooldown_until_unix_seconds
            .map(|cooldown_until| cooldown_until > generated_at)
            .unwrap_or(false)
        {
            active_cooldowns.push(identity_ledger_active_cooldown_summary(event, generated_at));
        }
    }

    let mut runtime_risk_events =
        read_identity_ledger_runtime_risk_events(runtime_risk_ledgers).await?;
    let runtime_risk_event_read_count = runtime_risk_events.len();
    let mut active_suppression_count = 0usize;
    let mut expired_suppression_count = 0usize;
    runtime_risk_events.retain(|event| {
        identity_ledger_explain_risk_event_matches(
            event,
            cutoff,
            generated_at,
            job_filter,
            worker_filter,
            reason_filter,
            &observed_failure_reasons,
            &mut active_suppression_count,
            &mut expired_suppression_count,
        )
    });

    let mut risk_action_counts = BTreeMap::new();
    let mut risk_severity_counts = BTreeMap::new();
    let mut risk_failure_reason_counts = BTreeMap::new();
    let mut active_suppressions = Vec::new();
    for event in &runtime_risk_events {
        if let Some(action) = identity_asset_field_string(event, &["recommendedAction"]) {
            increment_identity_ledger_count(&mut risk_action_counts, &action);
        }
        if let Some(severity) = identity_asset_field_string(event, &["severity"]) {
            increment_identity_ledger_count(&mut risk_severity_counts, &severity);
        }
        if let Some(reason) = identity_ledger_risk_failure_reason(event) {
            increment_identity_ledger_count(&mut risk_failure_reason_counts, &reason);
        }
        if identity_asset_field_u64(
            event,
            &["suppressUntilUnixSeconds", "suppress_until_unix_seconds"],
        )
        .map(|suppress_until| suppress_until > generated_at)
        .unwrap_or(false)
        {
            active_suppressions.push(identity_ledger_risk_suppression_summary(event));
        }
    }

    let latest_release_event = release_events
        .last()
        .map(identity_ledger_release_event_summary);
    let latest_runtime_risk_event = runtime_risk_events
        .last()
        .map(identity_ledger_risk_event_summary);
    let latest_release_status = release_events.last().map(|event| event.status.clone());
    let latest_release_failure_reason = release_events
        .last()
        .and_then(identity_ledger_release_failure_reason);
    let next_suppression_until = active_suppressions
        .iter()
        .filter_map(|event| identity_asset_field_u64(event, &["suppressUntilUnixSeconds"]))
        .max();
    let next_cooldown_until = active_cooldowns
        .iter()
        .filter_map(|event| identity_asset_field_u64(event, &["cooldownUntilUnixSeconds"]))
        .max();
    let next_runnable_unix_seconds = next_suppression_until
        .into_iter()
        .chain(next_cooldown_until)
        .max();
    let blocking_reasons = identity_ledger_explain_blocking_reasons(
        !active_suppressions.is_empty(),
        !active_cooldowns.is_empty(),
    );
    let decision = identity_ledger_explain_decision(
        !active_suppressions.is_empty(),
        !active_cooldowns.is_empty(),
        latest_release_status.as_deref(),
        release_events.is_empty(),
        runtime_risk_events.is_empty(),
    );

    let release_evidence = release_events
        .iter()
        .rev()
        .take(evidence_limit)
        .map(identity_ledger_release_event_summary)
        .collect::<Vec<_>>();
    let runtime_risk_evidence = runtime_risk_events
        .iter()
        .rev()
        .take(evidence_limit)
        .map(identity_ledger_risk_event_summary)
        .collect::<Vec<_>>();

    let mut report = json!({
        "scope": "identity_ledger_explain",
        "generatedAtUnixSeconds": generated_at,
        "windowSeconds": window_seconds,
        "target": {
            "job": job_filter,
            "worker": worker_filter,
            "reason": reason_filter,
            "accountId": account_id_filter,
            "profileId": profile_id_filter,
            "identityId": identity_id_filter,
            "label": label_filter,
            "profileDir": profile_dir_filter,
            "leaseId": lease_id_filter,
        },
        "releaseLedgers": release_ledgers
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "runtimeRiskLedgers": runtime_risk_ledgers
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "decision": decision,
        "blockingReasons": blocking_reasons,
        "blockedByActiveSuppression": !active_suppressions.is_empty(),
        "blockedByCooldown": !active_cooldowns.is_empty(),
        "nextRunnableUnixSeconds": next_runnable_unix_seconds,
        "latestReleaseStatus": latest_release_status,
        "latestReleaseFailureReason": latest_release_failure_reason,
        "releaseEventCount": release_events.len(),
        "releaseEventReadCount": release_event_read_count,
        "runtimeRiskEventCount": runtime_risk_events.len(),
        "runtimeRiskEventReadCount": runtime_risk_event_read_count,
        "activeSuppressionCount": active_suppression_count,
        "expiredSuppressionCount": expired_suppression_count,
        "activeCooldownCount": active_cooldowns.len(),
        "releaseStatusCounts": release_status_counts,
        "failureReasonCounts": release_failure_reason_counts,
        "runtimeRiskActionCounts": risk_action_counts,
        "runtimeRiskSeverityCounts": risk_severity_counts,
        "runtimeRiskFailureReasonCounts": risk_failure_reason_counts,
        "observedFailureReasons": observed_failure_reasons.into_iter().collect::<Vec<_>>(),
        "activeSuppressions": active_suppressions,
        "activeCooldowns": active_cooldowns,
        "latestReleaseEvent": latest_release_event,
        "latestRuntimeRiskEvent": latest_runtime_risk_event,
        "releaseEvidence": release_evidence,
        "runtimeRiskEvidence": runtime_risk_evidence,
        "recommendations": identity_ledger_explain_recommendations(
            decision,
            next_runnable_unix_seconds,
            release_events.len(),
            runtime_risk_events.len(),
        ),
    });

    if let Some(out) = out {
        let out_report = write_identity_ledger_explain_report(out, &report).await?;
        report["out"] = out_report;
    }

    Ok(JsonResponse::ok(report))
}

pub async fn sweep_identity_assets(
    asset_manifest: &Path,
    runtime_grace_seconds: u64,
    dispatch_grace_seconds: u64,
    cooldown_grace_seconds: u64,
    asset_manifest_out: Option<&Path>,
    sweep_out: Option<&Path>,
) -> Result<JsonResponse> {
    let text = tokio::fs::read_to_string(asset_manifest)
        .await
        .with_context(|| format!("failed to read asset manifest {}", asset_manifest.display()))?;
    let mut manifest = serde_json::from_str::<Value>(&text).with_context(|| {
        format!(
            "failed to parse asset manifest {}",
            asset_manifest.display()
        )
    })?;
    let generated_at = unix_seconds();
    let entries = identity_plan_manifest_entries_mut(&mut manifest)?;
    let asset_count = entries.len();
    let mut actions = Vec::new();
    let mut updated_indexes = BTreeSet::new();
    let mut expired_runtime_lease_count = 0usize;
    let mut expired_dispatch_lease_count = 0usize;
    let mut cleared_cooldown_count = 0usize;

    for (asset_index, entry) in entries.iter_mut().enumerate() {
        if let Some(action) = sweep_identity_asset_runtime_lease(
            asset_index,
            entry,
            generated_at,
            runtime_grace_seconds,
        ) {
            expired_runtime_lease_count += 1;
            updated_indexes.insert(asset_index);
            actions.push(action);
        }
        if let Some(action) = sweep_identity_asset_dispatch_lease(
            asset_index,
            entry,
            generated_at,
            dispatch_grace_seconds,
        ) {
            expired_dispatch_lease_count += 1;
            updated_indexes.insert(asset_index);
            actions.push(action);
        }
        if let Some(action) =
            sweep_identity_asset_cooldown(asset_index, entry, generated_at, cooldown_grace_seconds)
        {
            cleared_cooldown_count += 1;
            updated_indexes.insert(asset_index);
            actions.push(action);
        }
    }

    let state_counts = count_identity_plan_manifest_states(&manifest)?;
    let runtime_lease_state_counts = count_identity_asset_runtime_lease_states(&manifest)?;
    let dispatch_state_counts = count_identity_dispatch_manifest_states(&manifest)?;
    let asset_manifest_out_report = write_identity_asset_sweep_manifest(
        asset_manifest,
        asset_manifest_out,
        &manifest,
        asset_count,
        updated_indexes.len(),
    )
    .await?;

    let mut report = IdentityAssetsSweepReport {
        scope: "identity_assets_sweep".to_string(),
        asset_manifest: asset_manifest.display().to_string(),
        generated_at_unix_seconds: generated_at,
        runtime_grace_seconds,
        dispatch_grace_seconds,
        cooldown_grace_seconds,
        asset_count,
        updated_asset_count: updated_indexes.len(),
        expired_runtime_lease_count,
        expired_dispatch_lease_count,
        cleared_cooldown_count,
        state_counts,
        runtime_lease_state_counts,
        dispatch_state_counts,
        actions,
        asset_manifest_out: asset_manifest_out_report,
        sweep_out: None,
    };
    report.sweep_out = write_identity_asset_sweep_report(&report, sweep_out).await?;

    Ok(JsonResponse::ok(report))
}

#[derive(Debug, Clone)]
struct IdentityLedgerAssetStats {
    key: String,
    event_count: usize,
    succeeded_count: usize,
    failed_count: usize,
    cancelled_count: usize,
    other_count: usize,
    failure_reason_counts: BTreeMap<String, usize>,
    account_id: Option<String>,
    profile_id: Option<String>,
    identity_id: Option<String>,
    label: Option<String>,
    profile_dir: Option<String>,
    last_status: Option<String>,
    last_failure_reason: Option<String>,
    last_message: Option<String>,
    last_released_at_unix_seconds: Option<u64>,
    last_worker_id: Option<String>,
    last_job_id: Option<String>,
}

impl IdentityLedgerAssetStats {
    fn new(key: String) -> Self {
        Self {
            key,
            event_count: 0,
            succeeded_count: 0,
            failed_count: 0,
            cancelled_count: 0,
            other_count: 0,
            failure_reason_counts: BTreeMap::new(),
            account_id: None,
            profile_id: None,
            identity_id: None,
            label: None,
            profile_dir: None,
            last_status: None,
            last_failure_reason: None,
            last_message: None,
            last_released_at_unix_seconds: None,
            last_worker_id: None,
            last_job_id: None,
        }
    }

    fn record_release(
        &mut self,
        event: &IdentityAssetRuntimeReleaseEvent,
        failure_reason: Option<&str>,
    ) {
        self.event_count += 1;
        match event.status.as_str() {
            "succeeded" => self.succeeded_count += 1,
            "failed" => self.failed_count += 1,
            "cancelled" => self.cancelled_count += 1,
            _ => self.other_count += 1,
        }
        if let Some(reason) = failure_reason {
            increment_identity_ledger_count(&mut self.failure_reason_counts, reason);
        } else if event.status != "succeeded" {
            increment_identity_ledger_count(&mut self.failure_reason_counts, "unknown");
        }

        fill_identity_ledger_option(&mut self.account_id, &event.item.account_id);
        fill_identity_ledger_option(&mut self.profile_id, &event.item.profile_id);
        fill_identity_ledger_option(&mut self.identity_id, &event.item.identity_id);
        fill_identity_ledger_option(&mut self.label, &event.item.label);
        fill_identity_ledger_option(&mut self.profile_dir, &event.item.profile_dir);

        if self
            .last_released_at_unix_seconds
            .map(|last| event.generated_at_unix_seconds >= last)
            .unwrap_or(true)
        {
            self.last_status = Some(event.status.clone());
            self.last_failure_reason = failure_reason.map(str::to_string);
            self.last_message = event.message.clone();
            self.last_released_at_unix_seconds = Some(event.generated_at_unix_seconds);
            self.last_worker_id = event
                .item
                .worker_id
                .clone()
                .or_else(|| event.worker_id.clone());
            self.last_job_id = event.item.job_id.clone().or_else(|| event.job_id.clone());
        }
    }

    fn unsuccessful_count(&self) -> usize {
        self.failed_count + self.cancelled_count + self.other_count
    }

    fn to_json(&self) -> Value {
        json!({
            "key": self.key,
            "eventCount": self.event_count,
            "succeededCount": self.succeeded_count,
            "failedCount": self.failed_count,
            "cancelledCount": self.cancelled_count,
            "otherCount": self.other_count,
            "unsuccessfulCount": self.unsuccessful_count(),
            "failureReasonCounts": self.failure_reason_counts,
            "accountId": self.account_id,
            "profileId": self.profile_id,
            "identityId": self.identity_id,
            "label": self.label,
            "profileDir": self.profile_dir,
            "lastStatus": self.last_status,
            "lastFailureReason": self.last_failure_reason,
            "lastMessage": self.last_message,
            "lastReleasedAtUnixSeconds": self.last_released_at_unix_seconds,
            "lastWorkerId": self.last_worker_id,
            "lastJobId": self.last_job_id,
        })
    }
}

fn fill_identity_ledger_option(target: &mut Option<String>, value: &Option<String>) {
    if target.is_none() {
        *target = value.clone();
    }
}

#[derive(Debug, Default)]
struct IdentityLedgerCheckpoint {
    offsets: BTreeMap<String, u64>,
    active_suppressions: Vec<Value>,
}

struct IdentityLedgerReadResult<T> {
    events: Vec<T>,
    source_checkpoints: Vec<Value>,
}

async fn read_identity_ledger_checkpoint(path: Option<&Path>) -> Result<IdentityLedgerCheckpoint> {
    let Some(path) = path else {
        return Ok(IdentityLedgerCheckpoint::default());
    };
    let text = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read ledger checkpoint {}", path.display()))?;
    let value = serde_json::from_str::<Value>(&text)
        .with_context(|| format!("failed to parse ledger checkpoint {}", path.display()))?;
    let mut checkpoint = IdentityLedgerCheckpoint::default();
    collect_identity_ledger_checkpoint_offsets(&value, &mut checkpoint.offsets);
    collect_identity_ledger_checkpoint_suppressions(&value, &mut checkpoint.active_suppressions);
    Ok(checkpoint)
}

fn collect_identity_ledger_checkpoint_offsets(value: &Value, offsets: &mut BTreeMap<String, u64>) {
    if let Some(data) = value.get("data") {
        collect_identity_ledger_checkpoint_offsets(data, offsets);
    }
    if let Some(compact) = value.get("compact") {
        collect_identity_ledger_checkpoint_offsets(compact, offsets);
    }
    let Some(items) = value.get("sourceCheckpoints").and_then(Value::as_array) else {
        return;
    };
    for item in items {
        let Some(kind) = identity_asset_field_string(item, &["kind"]) else {
            continue;
        };
        let Some(path) = identity_asset_field_string(item, &["path"]) else {
            continue;
        };
        let Some(bytes) = identity_asset_field_u64(item, &["bytes"]) else {
            continue;
        };
        offsets.insert(identity_ledger_checkpoint_key(&kind, &path), bytes);
    }
}

fn collect_identity_ledger_checkpoint_suppressions(value: &Value, suppressions: &mut Vec<Value>) {
    if let Some(data) = value.get("data") {
        collect_identity_ledger_checkpoint_suppressions(data, suppressions);
    }
    if let Some(compact) = value.get("compact") {
        collect_identity_ledger_checkpoint_suppressions(compact, suppressions);
    }
    if let Some(items) = value.get("activeSuppressions").and_then(Value::as_array) {
        suppressions.extend(items.iter().cloned());
    }
}

fn identity_ledger_checkpoint_key(kind: &str, path: &str) -> String {
    format!("{kind}:{path}")
}

async fn read_identity_ledger_text_from_checkpoint(
    path: &Path,
    kind: &str,
    offsets: &BTreeMap<String, u64>,
) -> Result<(String, Value)> {
    let source_path = path.display().to_string();
    let metadata = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat ledger {}", path.display()))?;
    let bytes = metadata.len();
    let checkpoint_key = identity_ledger_checkpoint_key(kind, &source_path);
    let previous_bytes = offsets.get(&checkpoint_key).copied().unwrap_or(0);
    let reset = previous_bytes > bytes;
    let read_from_byte = if reset { 0 } else { previous_bytes };
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open ledger {}", path.display()))?;
    file.seek(SeekFrom::Start(read_from_byte))
        .await
        .with_context(|| format!("failed to seek ledger {}", path.display()))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .await
        .with_context(|| format!("failed to read ledger {}", path.display()))?;
    let source = json!({
        "kind": kind,
        "path": source_path,
        "bytes": bytes,
        "previousBytes": previous_bytes,
        "readFromByte": read_from_byte,
        "readBytes": bytes.saturating_sub(read_from_byte),
        "reset": reset,
    });
    Ok((text, source))
}

async fn read_identity_ledger_release_events_incremental(
    release_ledgers: &[PathBuf],
    offsets: &BTreeMap<String, u64>,
) -> Result<IdentityLedgerReadResult<IdentityAssetRuntimeReleaseEvent>> {
    let mut events = Vec::new();
    let mut source_checkpoints = Vec::new();
    for path in release_ledgers {
        let (text, mut source) =
            read_identity_ledger_text_from_checkpoint(path, "release", offsets).await?;
        let mut parsed = parse_identity_asset_runtime_release_events(&text, path)
            .with_context(|| format!("failed to parse release ledger {}", path.display()))?;
        source["eventReadCount"] = json!(parsed.len());
        for event in &mut parsed {
            event.event_index = events.len();
            events.push(event.clone());
        }
        source_checkpoints.push(source);
    }
    events.sort_by(|left, right| {
        left.generated_at_unix_seconds
            .cmp(&right.generated_at_unix_seconds)
            .then_with(|| left.event_index.cmp(&right.event_index))
    });
    Ok(IdentityLedgerReadResult {
        events,
        source_checkpoints,
    })
}

async fn read_identity_ledger_runtime_risk_events_incremental(
    runtime_risk_ledgers: &[PathBuf],
    offsets: &BTreeMap<String, u64>,
) -> Result<IdentityLedgerReadResult<Value>> {
    let mut events = Vec::new();
    let mut source_checkpoints = Vec::new();
    for path in runtime_risk_ledgers {
        let (text, mut source) =
            read_identity_ledger_text_from_checkpoint(path, "runtimeRisk", offsets).await?;
        let mut parsed = parse_identity_job_runtime_risk_events(&text, path)
            .with_context(|| format!("failed to parse runtime risk ledger {}", path.display()))?;
        source["eventReadCount"] = json!(parsed.len());
        events.append(&mut parsed);
        source_checkpoints.push(source);
    }
    events.sort_by(|left, right| {
        identity_asset_field_u64(left, &["generatedAtUnixSeconds"])
            .unwrap_or(0)
            .cmp(&identity_asset_field_u64(right, &["generatedAtUnixSeconds"]).unwrap_or(0))
            .then_with(|| {
                identity_asset_field_u64(left, &["runtimeRiskLedgerLine"])
                    .unwrap_or(0)
                    .cmp(&identity_asset_field_u64(right, &["runtimeRiskLedgerLine"]).unwrap_or(0))
            })
    });
    Ok(IdentityLedgerReadResult {
        events,
        source_checkpoints,
    })
}

async fn read_identity_ledger_release_events(
    release_ledgers: &[PathBuf],
) -> Result<Vec<IdentityAssetRuntimeReleaseEvent>> {
    let mut events = Vec::new();
    for path in release_ledgers {
        let text = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read release ledger {}", path.display()))?;
        let mut parsed = parse_identity_asset_runtime_release_events(&text, path)
            .with_context(|| format!("failed to parse release ledger {}", path.display()))?;
        for event in &mut parsed {
            event.event_index = events.len();
            events.push(event.clone());
        }
    }
    events.sort_by(|left, right| {
        left.generated_at_unix_seconds
            .cmp(&right.generated_at_unix_seconds)
            .then_with(|| left.event_index.cmp(&right.event_index))
    });
    Ok(events)
}

async fn read_identity_ledger_runtime_risk_events(
    runtime_risk_ledgers: &[PathBuf],
) -> Result<Vec<Value>> {
    let mut events = Vec::new();
    for path in runtime_risk_ledgers {
        let text = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read runtime risk ledger {}", path.display()))?;
        let mut parsed = parse_identity_job_runtime_risk_events(&text, path)
            .with_context(|| format!("failed to parse runtime risk ledger {}", path.display()))?;
        events.append(&mut parsed);
    }
    events.sort_by(|left, right| {
        identity_asset_field_u64(left, &["generatedAtUnixSeconds"])
            .unwrap_or(0)
            .cmp(&identity_asset_field_u64(right, &["generatedAtUnixSeconds"]).unwrap_or(0))
            .then_with(|| {
                identity_asset_field_u64(left, &["runtimeRiskLedgerLine"])
                    .unwrap_or(0)
                    .cmp(&identity_asset_field_u64(right, &["runtimeRiskLedgerLine"]).unwrap_or(0))
            })
    });
    Ok(events)
}

fn identity_ledger_release_event_matches_filters(
    event: &IdentityAssetRuntimeReleaseEvent,
    cutoff: Option<u64>,
    job_filter: Option<&str>,
    worker_filter: Option<&str>,
    reason_filter: Option<&str>,
) -> bool {
    if cutoff
        .map(|cutoff| event.generated_at_unix_seconds < cutoff)
        .unwrap_or(false)
    {
        return false;
    }
    if let Some(job_filter) = job_filter {
        if event
            .item
            .job_id
            .as_ref()
            .or(event.job_id.as_ref())
            .map(|job_id| job_id != job_filter)
            .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(worker_filter) = worker_filter {
        if event
            .item
            .worker_id
            .as_ref()
            .or(event.worker_id.as_ref())
            .map(|worker_id| worker_id != worker_filter)
            .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(reason_filter) = reason_filter {
        if identity_ledger_release_failure_reason(event)
            .map(|reason| reason != reason_filter)
            .unwrap_or(true)
        {
            return false;
        }
    }
    true
}

fn identity_ledger_risk_event_matches_filters(
    event: &Value,
    cutoff: Option<u64>,
    now: u64,
    job_filter: Option<&str>,
    worker_filter: Option<&str>,
    reason_filter: Option<&str>,
    active_suppression_count: &mut usize,
    expired_suppression_count: &mut usize,
) -> bool {
    if let Some(job_filter) = job_filter {
        if identity_asset_field_string(event, &["jobId", "job_id"])
            .map(|job_id| job_id != job_filter)
            .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(worker_filter) = worker_filter {
        if identity_asset_field_string(event, &["workerId", "worker_id"])
            .map(|worker_id| worker_id != worker_filter)
            .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(reason_filter) = reason_filter {
        if !identity_ledger_risk_event_matches_reason(event, reason_filter) {
            return false;
        }
    }

    let suppress_until = identity_asset_field_u64(
        event,
        &["suppressUntilUnixSeconds", "suppress_until_unix_seconds"],
    );
    let active_suppression = suppress_until
        .map(|suppress_until| suppress_until > now)
        .unwrap_or(false);
    if cutoff
        .map(|cutoff| {
            identity_asset_field_u64(event, &["generatedAtUnixSeconds"]).unwrap_or(0) < cutoff
                && !active_suppression
        })
        .unwrap_or(false)
    {
        return false;
    }
    if active_suppression {
        *active_suppression_count += 1;
    } else if suppress_until.is_some() {
        *expired_suppression_count += 1;
    }
    true
}

fn identity_ledger_risk_event_matches_reason(event: &Value, reason_filter: &str) -> bool {
    identity_ledger_risk_failure_reason(event)
        .map(|reason| reason == reason_filter)
        .unwrap_or(false)
        || event
            .get("failureReasonCounts")
            .and_then(Value::as_object)
            .map(|counts| counts.contains_key(reason_filter))
            .unwrap_or(false)
}

fn identity_ledger_release_failure_reason(
    event: &IdentityAssetRuntimeReleaseEvent,
) -> Option<String> {
    let result = event.result.as_ref()?;
    identity_asset_field_string(result, &["failureReason", "failure_reason", "reason"])
        .or_else(|| {
            result.get("releaseOverride").and_then(|value| {
                identity_asset_field_string(value, &["failureReason", "failure_reason", "reason"])
            })
        })
        .or_else(|| {
            result
                .get("releaseOverride")
                .and_then(|value| value.get("result"))
                .and_then(|value| {
                    identity_asset_field_string(
                        value,
                        &["failureReason", "failure_reason", "reason"],
                    )
                })
        })
        .or_else(|| {
            result.get("result").and_then(|value| {
                identity_asset_field_string(value, &["failureReason", "failure_reason", "reason"])
            })
        })
        .or_else(|| {
            result
                .get("resultOut")
                .and_then(|value| value.get("result"))
                .and_then(|value| {
                    identity_asset_field_string(
                        value,
                        &["failureReason", "failure_reason", "reason"],
                    )
                })
        })
}

fn identity_ledger_risk_failure_reason(event: &Value) -> Option<String> {
    identity_asset_field_string(
        event,
        &[
            "dominantFailureReason",
            "dominant_failure_reason",
            "policyFailureReason",
            "policy_failure_reason",
            "failureReason",
            "failure_reason",
            "reason",
        ],
    )
    .or_else(|| {
        event
            .get("failureReasonCounts")
            .and_then(Value::as_object)
            .and_then(|counts| {
                counts
                    .iter()
                    .filter_map(|(reason, count)| count.as_u64().map(|count| (reason, count)))
                    .max_by(|(left_reason, left_count), (right_reason, right_count)| {
                        left_count
                            .cmp(right_count)
                            .then_with(|| right_reason.cmp(left_reason))
                    })
                    .map(|(reason, _)| reason.to_string())
            })
    })
}

fn identity_ledger_release_asset_key(event: &IdentityAssetRuntimeReleaseEvent) -> String {
    event
        .item
        .label
        .as_ref()
        .map(|value| format!("label:{value}"))
        .or_else(|| {
            event
                .item
                .account_id
                .as_ref()
                .map(|value| format!("account:{value}"))
        })
        .or_else(|| {
            event
                .item
                .profile_id
                .as_ref()
                .map(|value| format!("profile:{value}"))
        })
        .or_else(|| {
            event
                .item
                .identity_id
                .as_ref()
                .map(|value| format!("identity:{value}"))
        })
        .or_else(|| {
            event
                .item
                .profile_dir
                .as_ref()
                .map(|value| format!("profileDir:{value}"))
        })
        .or_else(|| {
            event
                .item
                .lease_id
                .as_ref()
                .map(|value| format!("lease:{value}"))
        })
        .unwrap_or_else(|| format!("assetIndex:{}", event.item.asset_index))
}

fn increment_identity_ledger_count(counts: &mut BTreeMap<String, usize>, key: &str) {
    *counts.entry(key.to_string()).or_default() += 1;
}

fn identity_ledger_top_counts_json(counts: &BTreeMap<String, usize>, top: usize) -> Vec<Value> {
    let mut items = counts.iter().collect::<Vec<_>>();
    items.sort_by(|(left_key, left_count), (right_key, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| left_key.cmp(right_key))
    });
    items
        .into_iter()
        .take(top)
        .map(|(key, count)| json!({ "key": key, "count": count }))
        .collect()
}

fn identity_ledger_top_asset_stats_json(
    stats: &BTreeMap<String, IdentityLedgerAssetStats>,
    top: usize,
) -> Vec<Value> {
    let mut items = stats.values().collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right
            .unsuccessful_count()
            .cmp(&left.unsuccessful_count())
            .then_with(|| right.event_count.cmp(&left.event_count))
            .then_with(|| left.key.cmp(&right.key))
    });
    items
        .into_iter()
        .take(top)
        .map(IdentityLedgerAssetStats::to_json)
        .collect()
}

fn identity_ledger_all_asset_stats_json(
    stats: &BTreeMap<String, IdentityLedgerAssetStats>,
) -> Vec<Value> {
    stats
        .values()
        .map(IdentityLedgerAssetStats::to_json)
        .collect()
}

fn identity_ledger_high_watermark(
    release_times: impl Iterator<Item = u64>,
    risk_times: impl Iterator<Item = u64>,
) -> Option<u64> {
    release_times.chain(risk_times).max()
}

fn identity_ledger_release_event_summary(event: &IdentityAssetRuntimeReleaseEvent) -> Value {
    json!({
        "generatedAtUnixSeconds": event.generated_at_unix_seconds,
        "status": event.status,
        "failureReason": identity_ledger_release_failure_reason(event),
        "message": event.message,
        "accountId": event.item.account_id,
        "profileId": event.item.profile_id,
        "identityId": event.item.identity_id,
        "label": event.item.label,
        "profileDir": event.item.profile_dir,
        "leaseId": event.item.lease_id,
        "workerId": event.item.worker_id.as_ref().or(event.worker_id.as_ref()),
        "jobId": event.item.job_id.as_ref().or(event.job_id.as_ref()),
        "sourcePath": event.source_path,
    })
}

fn identity_ledger_risk_suppression_summary(event: &Value) -> Value {
    json!({
        "generatedAtUnixSeconds": identity_asset_field_u64(event, &["generatedAtUnixSeconds"]),
        "suppressUntilUnixSeconds": identity_asset_field_u64(event, &["suppressUntilUnixSeconds", "suppress_until_unix_seconds"]),
        "jobId": identity_asset_field_string(event, &["jobId", "job_id"]),
        "workerId": identity_asset_field_string(event, &["workerId", "worker_id"]),
        "failureReason": identity_ledger_risk_failure_reason(event),
        "recommendedAction": identity_asset_field_string(event, &["recommendedAction"]),
        "severity": identity_asset_field_string(event, &["severity"]),
        "message": identity_asset_field_string(event, &["message"]),
        "runtimeRiskLedgerPath": identity_asset_field_string(event, &["runtimeRiskLedgerPath"]),
        "runtimeRiskLedgerLine": identity_asset_field_u64(event, &["runtimeRiskLedgerLine"]),
    })
}

fn identity_ledger_query_recommendations(
    failure_reason_counts: &BTreeMap<String, usize>,
    risk_action_counts: &BTreeMap<String, usize>,
    active_suppression_count: usize,
) -> Vec<Value> {
    let mut recommendations = Vec::new();
    if active_suppression_count > 0 {
        recommendations.push(json!({
            "code": "honor_active_runtime_risk_suppression",
            "severity": "critical",
            "count": active_suppression_count,
            "message": "存在仍未到期的 runtime risk suppression,下一轮调度应继续读取 runtime-risk-ledger 并在领取 profile 前拦截。",
        }));
    }
    if let Some(count) = risk_action_counts.get("pause_pool") {
        recommendations.push(json!({
            "code": "pause_pool_seen",
            "severity": "critical",
            "count": count,
            "message": "runtime risk ledger 出现 pause_pool,应暂停整池并复盘最近失败。",
        }));
    }
    if let Some(count) = risk_action_counts.get("pause_failure_reason") {
        recommendations.push(json!({
            "code": "pause_failure_reason_seen",
            "severity": "high",
            "count": count,
            "message": "runtime risk ledger 出现按失败原因暂停,调度器应按 failureReason 维度降速或阻断。",
        }));
    }
    if let Some((reason, count)) = failure_reason_counts.iter().max_by(
        |(left_reason, left_count), (right_reason, right_count)| {
            left_count
                .cmp(right_count)
                .then_with(|| right_reason.cmp(left_reason))
        },
    ) {
        recommendations.push(json!({
            "code": "review_dominant_failure_reason",
            "severity": if reason == "unknown" { "medium" } else { "high" },
            "count": count,
            "reason": reason,
            "message": "存在集中的 runtime 失败原因,应先调整 job preset/failureReasonRules 或修复对应账号。",
        }));
    }
    recommendations
}

fn identity_ledger_compact_recommendations(
    active_suppression_count: usize,
    source_event_count: usize,
    compacted_event_count: usize,
    retained_evidence_count: usize,
) -> Vec<Value> {
    let mut recommendations = Vec::new();
    if active_suppression_count > 0 {
        recommendations.push(json!({
            "code": "carry_active_suppressions_forward",
            "severity": "critical",
            "count": active_suppression_count,
            "message": "compact 结果保留了仍未到期的 runtime risk suppression,下一轮调度应继续读取 compact 或原始 risk ledger 做预启动门禁。",
        }));
    }
    if source_event_count > compacted_event_count {
        recommendations.push(json!({
            "code": "archive_source_ledgers_after_compaction",
            "severity": "medium",
            "sourceEventCount": source_event_count,
            "compactedEventCount": compacted_event_count,
            "retainedEvidenceCount": retained_evidence_count,
            "message": "部分源事件已被窗口过滤;确认 compact 文件已进入调度/审计链路后,可把旧 NDJSON 移入冷归档。",
        }));
    }
    recommendations
}

#[allow(clippy::too_many_arguments)]
fn carry_identity_ledger_active_suppressions(
    active_suppressions: &mut Vec<Value>,
    checkpoint_suppressions: &[Value],
    now: u64,
    job_filter: Option<&str>,
    worker_filter: Option<&str>,
    reason_filter: Option<&str>,
    risk_action_counts: &mut BTreeMap<String, usize>,
    risk_severity_counts: &mut BTreeMap<String, usize>,
    risk_failure_reason_counts: &mut BTreeMap<String, usize>,
) -> usize {
    let mut carried = 0usize;
    let mut seen = active_suppressions
        .iter()
        .map(identity_ledger_suppression_dedupe_key)
        .collect::<BTreeSet<_>>();
    for event in checkpoint_suppressions {
        if !identity_ledger_checkpoint_suppression_matches_filters(
            event,
            now,
            job_filter,
            worker_filter,
            reason_filter,
        ) {
            continue;
        }
        let key = identity_ledger_suppression_dedupe_key(event);
        if !seen.insert(key) {
            continue;
        }
        if let Some(action) = identity_asset_field_string(event, &["recommendedAction"]) {
            increment_identity_ledger_count(risk_action_counts, &action);
        }
        if let Some(severity) = identity_asset_field_string(event, &["severity"]) {
            increment_identity_ledger_count(risk_severity_counts, &severity);
        }
        if let Some(reason) = identity_ledger_risk_failure_reason(event) {
            increment_identity_ledger_count(risk_failure_reason_counts, &reason);
        }
        active_suppressions.push(event.clone());
        carried += 1;
    }
    carried
}

fn identity_ledger_checkpoint_suppression_matches_filters(
    event: &Value,
    now: u64,
    job_filter: Option<&str>,
    worker_filter: Option<&str>,
    reason_filter: Option<&str>,
) -> bool {
    if identity_asset_field_u64(
        event,
        &["suppressUntilUnixSeconds", "suppress_until_unix_seconds"],
    )
    .map(|suppress_until| suppress_until <= now)
    .unwrap_or(true)
    {
        return false;
    }
    if let Some(job_filter) = job_filter {
        if identity_asset_field_string(event, &["jobId", "job_id"])
            .map(|job_id| job_id != job_filter)
            .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(worker_filter) = worker_filter {
        if identity_asset_field_string(event, &["workerId", "worker_id"])
            .map(|worker_id| worker_id != worker_filter)
            .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(reason_filter) = reason_filter {
        if !identity_ledger_risk_event_matches_reason(event, reason_filter) {
            return false;
        }
    }
    true
}

fn identity_ledger_suppression_dedupe_key(event: &Value) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}",
        identity_ledger_json_text(event, "runtimeRiskLedgerPath"),
        identity_ledger_json_text(event, "runtimeRiskLedgerLine"),
        identity_ledger_json_text(event, "generatedAtUnixSeconds"),
        identity_ledger_json_text(event, "jobId"),
        identity_ledger_json_text(event, "failureReason"),
        identity_ledger_json_text(event, "suppressUntilUnixSeconds")
    )
}

fn identity_ledger_dashboard_summary(compact: &Value) -> Value {
    let release_event_count = identity_ledger_value_u64(compact, "releaseEventCount").unwrap_or(0);
    let failed_count = identity_ledger_count_at(compact, &["releaseStatusCounts"], "failed");
    let cancelled_count = identity_ledger_count_at(compact, &["releaseStatusCounts"], "cancelled");
    let unsuccessful_count = failed_count + cancelled_count;
    let failure_rate_permille = if release_event_count > 0 {
        unsuccessful_count.saturating_mul(1_000) / release_event_count
    } else {
        0
    };
    let active_suppression_count =
        identity_ledger_value_u64(compact, "activeSuppressionCount").unwrap_or(0);
    let active_asset_count = identity_ledger_value_u64(compact, "assetSummaryCount").unwrap_or(0);
    let top_failure_reason = identity_ledger_first_top_key(compact, "topFailureReasons");
    let top_runtime_risk_action = identity_ledger_first_top_key(compact, "topRuntimeRiskActions");
    let status = if active_suppression_count > 0 {
        "blocked"
    } else if top_runtime_risk_action
        .as_deref()
        .map(|action| action == "pause_pool" || action == "pause_failure_reason")
        .unwrap_or(false)
        || unsuccessful_count > 0
    {
        "degraded"
    } else {
        "healthy"
    };
    let recommended_action = if active_suppression_count > 0 {
        "honor_active_suppression"
    } else if top_runtime_risk_action.as_deref() == Some("reduce_concurrency") {
        "reduce_concurrency"
    } else if unsuccessful_count > 0 {
        "review_failed_assets"
    } else {
        "continue_current"
    };

    json!({
        "status": status,
        "recommendedAction": recommended_action,
        "releaseEventCount": release_event_count,
        "runtimeRiskEventCount": identity_ledger_value_u64(compact, "runtimeRiskEventCount").unwrap_or(0),
        "assetSummaryCount": active_asset_count,
        "failedCount": failed_count,
        "cancelledCount": cancelled_count,
        "unsuccessfulCount": unsuccessful_count,
        "failureRatePermille": failure_rate_permille,
        "activeSuppressionCount": active_suppression_count,
        "nextSuppressionUntilUnixSeconds": identity_ledger_value_u64(compact, "nextSuppressionUntilUnixSeconds"),
        "topFailureReason": top_failure_reason,
        "topRuntimeRiskAction": top_runtime_risk_action,
        "compactedThroughUnixSeconds": identity_ledger_value_u64(compact, "compactedThroughUnixSeconds"),
    })
}

fn identity_ledger_value_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(|field| {
        field
            .as_u64()
            .or_else(|| field.as_str().and_then(|text| text.parse::<u64>().ok()))
    })
}

fn identity_ledger_count_at(value: &Value, path: &[&str], key: &str) -> u64 {
    let mut current = value;
    for segment in path {
        let Some(next) = current.get(*segment) else {
            return 0;
        };
        current = next;
    }
    current.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn identity_ledger_first_top_key(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("key"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

async fn write_identity_ledger_query_report(path: &Path, report: &Value) -> Result<Value> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(report)?;
    tokio::fs::write(path, &bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(json!({
        "path": path.display().to_string(),
        "format": "json_identity_ledger_query",
        "bytes": bytes.len(),
    }))
}

async fn write_identity_ledger_dashboard_report(path: &Path, report: &Value) -> Result<Value> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(report)?;
    tokio::fs::write(path, &bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(json!({
        "path": path.display().to_string(),
        "format": "json_identity_ledger_dashboard",
        "bytes": bytes.len(),
    }))
}

async fn write_identity_ledger_checkpoint_report(path: &Path, report: &Value) -> Result<Value> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let checkpoint = json!({
        "scope": "identity_ledger_checkpoint",
        "generatedAtUnixSeconds": identity_ledger_value_u64(report, "generatedAtUnixSeconds").unwrap_or_else(unix_seconds),
        "compactedThroughUnixSeconds": identity_ledger_value_u64(report, "compactedThroughUnixSeconds"),
        "sourceCheckpoints": report
            .get("sourceCheckpoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        "activeSuppressions": report
            .get("activeSuppressions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    });
    let bytes = serde_json::to_vec_pretty(&checkpoint)?;
    tokio::fs::write(path, &bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(json!({
        "path": path.display().to_string(),
        "format": "json_identity_ledger_checkpoint",
        "bytes": bytes.len(),
    }))
}

async fn write_identity_ledger_dashboard_html(path: &Path, report: &Value) -> Result<Value> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let html = render_identity_ledger_dashboard_html(report);
    tokio::fs::write(path, html.as_bytes())
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(json!({
        "path": path.display().to_string(),
        "format": "html_identity_ledger_dashboard",
        "bytes": html.len(),
    }))
}

fn render_identity_ledger_dashboard_html(report: &Value) -> String {
    let compact = report.get("compact").unwrap_or(&Value::Null);
    let summary = report.get("summary").unwrap_or(&Value::Null);
    let mut html = String::new();
    html.push_str("<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\">");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    html.push_str("<title>drs identity ledger dashboard</title><style>");
    html.push_str("body{font-family:-apple-system,BlinkMacSystemFont,\"Segoe UI\",sans-serif;margin:0;background:#f6f7f9;color:#16181d}main{max-width:1180px;margin:0 auto;padding:28px}h1{font-size:28px;margin:0 0 6px}h2{font-size:18px;margin:28px 0 10px}.muted{color:#687383}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(165px,1fr));gap:10px;margin:18px 0}.metric{background:#fff;border:1px solid #dce2ea;border-radius:8px;padding:12px}.metric span{display:block;color:#687383}.metric b{display:block;font-size:24px;margin-top:5px}.blocked{color:#a52727}.degraded{color:#9f4b00}.healthy{color:#176b3a}table{width:100%;border-collapse:collapse;background:#fff;border:1px solid #dce2ea;border-radius:8px;overflow:hidden}th,td{padding:9px 10px;border-bottom:1px solid #edf0f3;text-align:left;font-size:13px;vertical-align:top}th{background:#eef2f6;color:#303844}tr:last-child td{border-bottom:0}code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:12px}.pill{display:inline-block;border:1px solid #cbd3dc;border-radius:999px;padding:2px 8px;font-size:12px;background:#fff}.small{font-size:12px}.empty{background:#fff;border:1px solid #dce2ea;border-radius:8px;padding:12px}");
    html.push_str("</style></head><body><main>");
    html.push_str("<h1>drs identity ledger dashboard</h1><div class=\"muted\">generated ");
    html.push_str(&escape_html(&identity_ledger_json_text(
        report,
        "generatedAtUnixSeconds",
    )));
    html.push_str(" · compacted through ");
    html.push_str(&escape_html(&identity_ledger_json_text(
        summary,
        "compactedThroughUnixSeconds",
    )));
    html.push_str("</div>");

    let status = identity_ledger_json_text(summary, "status");
    html.push_str("<section class=\"grid\">");
    identity_ledger_dashboard_metric(&mut html, "状态", &status, Some(status.as_str()));
    identity_ledger_dashboard_metric(
        &mut html,
        "建议动作",
        &identity_ledger_json_text(summary, "recommendedAction"),
        None,
    );
    identity_ledger_dashboard_metric(
        &mut html,
        "Release 事件",
        &identity_ledger_json_text(summary, "releaseEventCount"),
        None,
    );
    identity_ledger_dashboard_metric(
        &mut html,
        "Risk 事件",
        &identity_ledger_json_text(summary, "runtimeRiskEventCount"),
        None,
    );
    identity_ledger_dashboard_metric(
        &mut html,
        "失败率 permille",
        &identity_ledger_json_text(summary, "failureRatePermille"),
        None,
    );
    identity_ledger_dashboard_metric(
        &mut html,
        "Active suppression",
        &identity_ledger_json_text(summary, "activeSuppressionCount"),
        None,
    );
    identity_ledger_dashboard_metric(
        &mut html,
        "Top failure",
        &identity_ledger_json_text(summary, "topFailureReason"),
        None,
    );
    identity_ledger_dashboard_metric(
        &mut html,
        "Top risk action",
        &identity_ledger_json_text(summary, "topRuntimeRiskAction"),
        None,
    );
    html.push_str("</section>");

    identity_ledger_dashboard_recommendations(&mut html, compact);
    identity_ledger_dashboard_suppressions(&mut html, compact);
    identity_ledger_dashboard_top_assets(&mut html, compact);
    identity_ledger_dashboard_top_counts(&mut html, compact, "topFailureReasons", "失败原因排行");
    identity_ledger_dashboard_top_counts(
        &mut html,
        compact,
        "topRuntimeRiskActions",
        "Runtime risk action",
    );
    identity_ledger_dashboard_release_evidence(&mut html, compact);
    identity_ledger_dashboard_risk_evidence(&mut html, compact);

    html.push_str("</main></body></html>");
    html
}

fn identity_ledger_dashboard_metric(
    html: &mut String,
    label: &str,
    value: &str,
    class_name: Option<&str>,
) {
    html.push_str("<div class=\"metric\"><span>");
    html.push_str(&escape_html(label));
    html.push_str("</span><b");
    if let Some(class_name) = class_name {
        html.push_str(" class=\"");
        html.push_str(&escape_html(class_name));
        html.push('"');
    }
    html.push('>');
    html.push_str(&escape_html(value));
    html.push_str("</b></div>");
}

fn identity_ledger_json_text(value: &Value, key: &str) -> String {
    let Some(field) = value.get(key) else {
        return "-".to_string();
    };
    match field {
        Value::Null => "-".to_string(),
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "-".to_string()),
    }
}

fn identity_ledger_dashboard_recommendations(html: &mut String, compact: &Value) {
    html.push_str("<h2>建议</h2>");
    let Some(items) = compact.get("recommendations").and_then(Value::as_array) else {
        html.push_str("<div class=\"empty\">无建议</div>");
        return;
    };
    if items.is_empty() {
        html.push_str("<div class=\"empty\">无建议</div>");
        return;
    }
    html.push_str("<table><thead><tr><th>级别</th><th>代码</th><th>说明</th></tr></thead><tbody>");
    for item in items {
        html.push_str("<tr><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(item, "severity")));
        html.push_str("</td><td><code>");
        html.push_str(&escape_html(&identity_ledger_json_text(item, "code")));
        html.push_str("</code></td><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(item, "message")));
        html.push_str("</td></tr>");
    }
    html.push_str("</tbody></table>");
}

fn identity_ledger_dashboard_suppressions(html: &mut String, compact: &Value) {
    html.push_str("<h2>Active suppressions</h2>");
    let Some(items) = compact.get("activeSuppressions").and_then(Value::as_array) else {
        html.push_str("<div class=\"empty\">无 active suppression</div>");
        return;
    };
    if items.is_empty() {
        html.push_str("<div class=\"empty\">无 active suppression</div>");
        return;
    }
    html.push_str("<table><thead><tr><th>reason</th><th>action</th><th>severity</th><th>job</th><th>until</th><th>message</th></tr></thead><tbody>");
    for item in items {
        html.push_str("<tr><td><code>");
        html.push_str(&escape_html(&identity_ledger_json_text(
            item,
            "failureReason",
        )));
        html.push_str("</code></td><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(
            item,
            "recommendedAction",
        )));
        html.push_str("</td><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(item, "severity")));
        html.push_str("</td><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(item, "jobId")));
        html.push_str("</td><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(
            item,
            "suppressUntilUnixSeconds",
        )));
        html.push_str("</td><td class=\"small\">");
        html.push_str(&escape_html(&identity_ledger_json_text(item, "message")));
        html.push_str("</td></tr>");
    }
    html.push_str("</tbody></table>");
}

fn identity_ledger_dashboard_top_assets(html: &mut String, compact: &Value) {
    html.push_str("<h2>Top assets</h2>");
    let Some(items) = compact.get("topAssets").and_then(Value::as_array) else {
        html.push_str("<div class=\"empty\">无资产统计</div>");
        return;
    };
    if items.is_empty() {
        html.push_str("<div class=\"empty\">无资产统计</div>");
        return;
    }
    html.push_str("<table><thead><tr><th>asset</th><th>status</th><th>失败</th><th>成功</th><th>reason</th><th>message</th></tr></thead><tbody>");
    for item in items {
        let asset = identity_ledger_json_text(item, "label");
        let asset = if asset == "-" {
            identity_ledger_json_text(item, "key")
        } else {
            asset
        };
        html.push_str("<tr><td><code>");
        html.push_str(&escape_html(&asset));
        html.push_str("</code></td><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(item, "lastStatus")));
        html.push_str("</td><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(
            item,
            "failedCount",
        )));
        html.push_str("</td><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(
            item,
            "succeededCount",
        )));
        html.push_str("</td><td><code>");
        html.push_str(&escape_html(&identity_ledger_json_text(
            item,
            "lastFailureReason",
        )));
        html.push_str("</code></td><td class=\"small\">");
        html.push_str(&escape_html(&identity_ledger_json_text(
            item,
            "lastMessage",
        )));
        html.push_str("</td></tr>");
    }
    html.push_str("</tbody></table>");
}

fn identity_ledger_dashboard_top_counts(
    html: &mut String,
    compact: &Value,
    key: &str,
    title: &str,
) {
    html.push_str("<h2>");
    html.push_str(&escape_html(title));
    html.push_str("</h2>");
    let Some(items) = compact.get(key).and_then(Value::as_array) else {
        html.push_str("<div class=\"empty\">无数据</div>");
        return;
    };
    if items.is_empty() {
        html.push_str("<div class=\"empty\">无数据</div>");
        return;
    }
    html.push_str("<table><thead><tr><th>key</th><th>count</th></tr></thead><tbody>");
    for item in items {
        html.push_str("<tr><td><code>");
        html.push_str(&escape_html(&identity_ledger_json_text(item, "key")));
        html.push_str("</code></td><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(item, "count")));
        html.push_str("</td></tr>");
    }
    html.push_str("</tbody></table>");
}

fn identity_ledger_dashboard_release_evidence(html: &mut String, compact: &Value) {
    html.push_str("<h2>最近 release 证据</h2>");
    let Some(items) = compact
        .get("retainedReleaseEvidence")
        .and_then(Value::as_array)
    else {
        html.push_str("<div class=\"empty\">无 release 证据</div>");
        return;
    };
    if items.is_empty() {
        html.push_str("<div class=\"empty\">无 release 证据</div>");
        return;
    }
    html.push_str("<table><thead><tr><th>time</th><th>asset</th><th>status</th><th>reason</th><th>job/worker</th><th>message</th></tr></thead><tbody>");
    for item in items.iter().take(50) {
        html.push_str("<tr><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(
            item,
            "generatedAtUnixSeconds",
        )));
        html.push_str("</td><td><code>");
        html.push_str(&escape_html(&identity_ledger_json_text(item, "label")));
        html.push_str("</code></td><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(item, "status")));
        html.push_str("</td><td><code>");
        html.push_str(&escape_html(&identity_ledger_json_text(
            item,
            "failureReason",
        )));
        html.push_str("</code></td><td>");
        html.push_str(&escape_html(&format!(
            "{}/{}",
            identity_ledger_json_text(item, "jobId"),
            identity_ledger_json_text(item, "workerId")
        )));
        html.push_str("</td><td class=\"small\">");
        html.push_str(&escape_html(&identity_ledger_json_text(item, "message")));
        html.push_str("</td></tr>");
    }
    html.push_str("</tbody></table>");
}

fn identity_ledger_dashboard_risk_evidence(html: &mut String, compact: &Value) {
    html.push_str("<h2>最近 runtime risk 证据</h2>");
    let Some(items) = compact
        .get("retainedRuntimeRiskEvidence")
        .and_then(Value::as_array)
    else {
        html.push_str("<div class=\"empty\">无 runtime risk 证据</div>");
        return;
    };
    if items.is_empty() {
        html.push_str("<div class=\"empty\">无 runtime risk 证据</div>");
        return;
    }
    html.push_str("<table><thead><tr><th>time</th><th>reason</th><th>action</th><th>severity</th><th>until</th><th>message</th></tr></thead><tbody>");
    for item in items.iter().take(50) {
        html.push_str("<tr><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(
            item,
            "generatedAtUnixSeconds",
        )));
        html.push_str("</td><td><code>");
        html.push_str(&escape_html(&identity_ledger_json_text(
            item,
            "failureReason",
        )));
        html.push_str("</code></td><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(
            item,
            "recommendedAction",
        )));
        html.push_str("</td><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(item, "severity")));
        html.push_str("</td><td>");
        html.push_str(&escape_html(&identity_ledger_json_text(
            item,
            "suppressUntilUnixSeconds",
        )));
        html.push_str("</td><td class=\"small\">");
        html.push_str(&escape_html(&identity_ledger_json_text(item, "message")));
        html.push_str("</td></tr>");
    }
    html.push_str("</tbody></table>");
}

async fn write_identity_ledger_compact_report(path: &Path, report: &Value) -> Result<Value> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(report)?;
    tokio::fs::write(path, &bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(json!({
        "path": path.display().to_string(),
        "format": "json_identity_ledger_compact",
        "bytes": bytes.len(),
    }))
}

#[allow(clippy::too_many_arguments)]
fn identity_ledger_release_event_matches_asset_filters(
    event: &IdentityAssetRuntimeReleaseEvent,
    account_id_filter: Option<&str>,
    profile_id_filter: Option<&str>,
    identity_id_filter: Option<&str>,
    label_filter: Option<&str>,
    profile_dir_filter: Option<&str>,
    lease_id_filter: Option<&str>,
) -> bool {
    if let Some(filter) = account_id_filter {
        if event.item.account_id.as_deref() != Some(filter) {
            return false;
        }
    }
    if let Some(filter) = profile_id_filter {
        if event.item.profile_id.as_deref() != Some(filter) {
            return false;
        }
    }
    if let Some(filter) = identity_id_filter {
        if event.item.identity_id.as_deref() != Some(filter) {
            return false;
        }
    }
    if let Some(filter) = label_filter {
        if event.item.label.as_deref() != Some(filter) {
            return false;
        }
    }
    if let Some(filter) = profile_dir_filter {
        if event.item.profile_dir.as_deref() != Some(filter) {
            return false;
        }
    }
    if let Some(filter) = lease_id_filter {
        if event.item.lease_id.as_deref() != Some(filter) {
            return false;
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn identity_ledger_explain_risk_event_matches(
    event: &Value,
    cutoff: Option<u64>,
    now: u64,
    job_filter: Option<&str>,
    worker_filter: Option<&str>,
    reason_filter: Option<&str>,
    observed_failure_reasons: &BTreeSet<String>,
    active_suppression_count: &mut usize,
    expired_suppression_count: &mut usize,
) -> bool {
    if let Some(job_filter) = job_filter {
        if identity_asset_field_string(event, &["jobId", "job_id"])
            .map(|job_id| job_id != job_filter)
            .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(worker_filter) = worker_filter {
        if identity_asset_field_string(event, &["workerId", "worker_id"])
            .map(|worker_id| worker_id != worker_filter)
            .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(reason_filter) = reason_filter {
        if !identity_ledger_risk_event_matches_reason(event, reason_filter) {
            return false;
        }
    } else if !observed_failure_reasons.is_empty()
        && !identity_ledger_risk_event_matches_any_reason(event, observed_failure_reasons)
        && identity_asset_field_string(event, &["recommendedAction"]).as_deref()
            != Some("pause_pool")
    {
        return false;
    }

    let suppress_until = identity_asset_field_u64(
        event,
        &["suppressUntilUnixSeconds", "suppress_until_unix_seconds"],
    );
    let active_suppression = suppress_until
        .map(|suppress_until| suppress_until > now)
        .unwrap_or(false);
    if cutoff
        .map(|cutoff| {
            identity_asset_field_u64(event, &["generatedAtUnixSeconds"]).unwrap_or(0) < cutoff
                && !active_suppression
        })
        .unwrap_or(false)
    {
        return false;
    }
    if active_suppression {
        *active_suppression_count += 1;
    } else if suppress_until.is_some() {
        *expired_suppression_count += 1;
    }
    true
}

fn identity_ledger_risk_event_matches_any_reason(
    event: &Value,
    reasons: &BTreeSet<String>,
) -> bool {
    reasons
        .iter()
        .any(|reason| identity_ledger_risk_event_matches_reason(event, reason))
}

fn identity_ledger_active_cooldown_summary(
    event: &IdentityAssetRuntimeReleaseEvent,
    now: u64,
) -> Value {
    let cooldown_until = event.cooldown_until_unix_seconds.unwrap_or(0);
    let mut summary = identity_ledger_release_event_summary(event);
    summary["cooldownUntilUnixSeconds"] = json!(cooldown_until);
    summary["secondsRemaining"] = json!(cooldown_until.saturating_sub(now));
    summary
}

fn identity_ledger_risk_event_summary(event: &Value) -> Value {
    let suppress_until = identity_asset_field_u64(
        event,
        &["suppressUntilUnixSeconds", "suppress_until_unix_seconds"],
    );
    json!({
        "generatedAtUnixSeconds": identity_asset_field_u64(event, &["generatedAtUnixSeconds"]),
        "suppressUntilUnixSeconds": suppress_until,
        "jobId": identity_asset_field_string(event, &["jobId", "job_id"]),
        "workerId": identity_asset_field_string(event, &["workerId", "worker_id"]),
        "failureReason": identity_ledger_risk_failure_reason(event),
        "recommendedAction": identity_asset_field_string(event, &["recommendedAction"]),
        "severity": identity_asset_field_string(event, &["severity"]),
        "message": identity_asset_field_string(event, &["message"]),
        "nextSuggestedLimit": identity_asset_field_u64(event, &["nextSuggestedLimit"]),
        "nextSuggestedDesiredConcurrency": identity_asset_field_u64(event, &["nextSuggestedDesiredConcurrency"]),
        "failureRatePermille": identity_asset_field_u64(event, &["failureRatePermille"]),
        "runtimeRiskCooldownSeconds": identity_asset_field_u64(event, &["runtimeRiskCooldownSeconds"]),
        "runtimeRiskLedgerPath": identity_asset_field_string(event, &["runtimeRiskLedgerPath"]),
        "runtimeRiskLedgerLine": identity_asset_field_u64(event, &["runtimeRiskLedgerLine"]),
    })
}

fn identity_ledger_explain_blocking_reasons(
    has_active_suppression: bool,
    has_active_cooldown: bool,
) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    if has_active_suppression {
        reasons.push("active_runtime_risk_suppression");
    }
    if has_active_cooldown {
        reasons.push("active_asset_cooldown");
    }
    reasons
}

fn identity_ledger_explain_decision(
    has_active_suppression: bool,
    has_active_cooldown: bool,
    latest_release_status: Option<&str>,
    no_release_evidence: bool,
    no_risk_evidence: bool,
) -> &'static str {
    if has_active_suppression {
        return "blocked_by_runtime_risk_suppression";
    }
    if has_active_cooldown {
        return "blocked_by_asset_cooldown";
    }
    match latest_release_status {
        Some("succeeded") => "runnable_from_ledger",
        Some("failed" | "cancelled") => "review_latest_runtime_failure",
        Some(_) => "review_runtime_history",
        None if no_risk_evidence && no_release_evidence => "no_matching_ledger_evidence",
        None => "review_runtime_risk_history",
    }
}

fn identity_ledger_explain_recommendations(
    decision: &str,
    next_runnable_unix_seconds: Option<u64>,
    release_event_count: usize,
    runtime_risk_event_count: usize,
) -> Vec<Value> {
    let mut recommendations = Vec::new();
    match decision {
        "blocked_by_runtime_risk_suppression" => {
            recommendations.push(json!({
                "code": "honor_runtime_risk_suppression",
                "severity": "critical",
                "nextRunnableUnixSeconds": next_runnable_unix_seconds,
                "message": "存在仍未到期的 runtime risk suppression,下一轮调度应在领取 profile 前继续阻断或降速。",
            }));
        }
        "blocked_by_asset_cooldown" => {
            recommendations.push(json!({
                "code": "wait_for_asset_cooldown",
                "severity": "high",
                "nextRunnableUnixSeconds": next_runnable_unix_seconds,
                "message": "账号/Profile 仍在 cooldown,应等待到期或先执行修复流程。",
            }));
        }
        "review_latest_runtime_failure" => {
            recommendations.push(json!({
                "code": "review_latest_runtime_failure",
                "severity": "high",
                "message": "最近 release 仍是失败/取消,建议先查看 failureReasonRules、账号状态或站点返回原因。",
            }));
        }
        "no_matching_ledger_evidence" => {
            recommendations.push(json!({
                "code": "no_matching_ledger_evidence",
                "severity": "medium",
                "message": "没有匹配到 release/risk 证据;请检查 selector、windowSeconds 或 ledger 路径。",
            }));
        }
        _ => {}
    }
    if release_event_count == 0 && runtime_risk_event_count > 0 {
        recommendations.push(json!({
            "code": "risk_without_asset_release",
            "severity": "medium",
            "message": "只匹配到 runtime risk,没有匹配到具体资产 release;如需账号级解释,请增加 account/profile/label 或扩大窗口。",
        }));
    }
    recommendations
}

async fn write_identity_ledger_explain_report(path: &Path, report: &Value) -> Result<Value> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(report)?;
    tokio::fs::write(path, &bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(json!({
        "path": path.display().to_string(),
        "format": "json_identity_ledger_explain",
        "bytes": bytes.len(),
    }))
}

#[allow(clippy::too_many_lines)]
pub async fn run_identity_job(mut options: IdentityJobRunOptions) -> Result<JsonResponse> {
    let policy = load_identity_policy(options.policy.as_deref()).await?;
    if let Some(policy) = policy.as_ref() {
        options = policy.merge_job_run(options);
    } else {
        options = merge_identity_job_without_loaded_policy(options);
    }
    if let Some(job_preset) = options.job_preset.clone() {
        let Some(canonical) = identity_job_preset_canonical(&job_preset) else {
            bail!(
                "identity-job run job preset is unsupported: {job_preset}; supported presets are publish_conservative, login_sensitive, scrape_aggressive"
            );
        };
        options.job_preset = Some(canonical.to_string());
    }
    if options.command.is_empty() {
        bail!("identity-job run requires a command after --");
    }
    let command = options.command.clone();
    let mut limit = options.limit.unwrap_or(1);
    let lease_seconds = options.lease_seconds.unwrap_or(900);
    let child_concurrency = options.child_concurrency.unwrap_or(1);
    let runtime_renew_interval_seconds = options.runtime_renew_interval_seconds;
    let child_timeout_seconds = options.child_timeout_seconds;
    let child_result_dir = options.child_result_dir.clone();
    let max_failed_assets = options.max_failed_assets;
    let max_failed_assets_per_reason = options.max_failed_assets_per_reason;
    let failure_reason_rules = options.failure_reason_rules.clone();
    let runtime_grace_seconds = options.runtime_grace_seconds.unwrap_or(0);
    let dispatch_grace_seconds = options.dispatch_grace_seconds.unwrap_or(0);
    let cooldown_grace_seconds = options.cooldown_grace_seconds.unwrap_or(0);
    if limit == 0 {
        bail!("identity-job run --limit must be greater than 0");
    }
    if lease_seconds == 0 {
        bail!("identity-job run --lease-seconds must be greater than 0");
    }
    if child_concurrency == 0 {
        bail!("identity-job run --child-concurrency must be greater than 0");
    }
    if let Some(timeout_seconds) = child_timeout_seconds {
        if timeout_seconds == 0 {
            bail!("identity-job run --child-timeout-seconds must be greater than 0");
        }
    }
    if let Some(max_failed_assets) = max_failed_assets {
        if max_failed_assets == 0 {
            bail!("identity-job run --max-failed-assets must be greater than 0");
        }
    }
    if let Some(max_failed_assets_per_reason) = max_failed_assets_per_reason {
        if max_failed_assets_per_reason == 0 {
            bail!("identity-job run --max-failed-assets-per-reason must be greater than 0");
        }
    }
    for (reason, rule) in &failure_reason_rules {
        if rule.cooldown_seconds == Some(0) {
            bail!(
                "identity-job run failure reason rule {reason} cooldownSeconds must be greater than 0"
            );
        }
        if rule.runtime_risk_cooldown_seconds == Some(0) {
            bail!(
                "identity-job run failure reason rule {reason} runtimeRiskCooldownSeconds must be greater than 0"
            );
        }
        if let Some(action) = rule.recommended_action.as_deref() {
            if !matches!(
                action,
                "continue_current"
                    | "reduce_concurrency"
                    | "pause_pool"
                    | "pause_failure_reason"
                    | "inspect_job_report"
            ) {
                bail!(
                    "identity-job run failure reason rule {reason} recommendedAction is unsupported: {action}"
                );
            }
        }
        if let Some(severity) = rule.runtime_risk_severity.as_deref() {
            if !matches!(
                severity,
                "healthy" | "elevated" | "high" | "critical" | "blocked" | "unknown"
            ) {
                bail!(
                    "identity-job run failure reason rule {reason} runtimeRiskSeverity is unsupported: {severity}"
                );
            }
        }
    }
    if let Some(interval_seconds) = runtime_renew_interval_seconds {
        if interval_seconds == 0 {
            bail!("identity-job run --runtime-renew-interval-seconds must be greater than 0");
        }
        if interval_seconds >= lease_seconds {
            bail!(
                "identity-job run --runtime-renew-interval-seconds must be less than --lease-seconds"
            );
        }
    }
    if options.per_asset && options.release_out.is_some() && !options.append_release {
        bail!("identity-job run --per-asset requires --append-release when --release-out is used");
    }
    if options.append_runtime_risk && options.runtime_risk_out.is_none() {
        bail!("identity-job run --append-runtime-risk requires --runtime-risk-out");
    }
    if let Some(window_seconds) = options.runtime_risk_window_seconds {
        if window_seconds == 0 {
            bail!("identity-job run --runtime-risk-window-seconds must be greater than 0");
        }
    }

    let generated_at = unix_seconds();
    let run_id = format!("identity_job_{}_{}", generated_at, std::process::id());
    let worker_id = options
        .worker
        .as_deref()
        .map(str::trim)
        .filter(|worker| !worker.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("identity-job-{}", std::process::id()));
    let job_id = options
        .job
        .as_deref()
        .map(str::trim)
        .filter(|job| !job.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| "identity-job".to_string());
    let mut desired_concurrency = options.desired_concurrency.unwrap_or(limit);
    if desired_concurrency == 0 {
        bail!("identity-job run --desired-concurrency must be greater than 0");
    }
    let working_manifest = options
        .asset_manifest_out
        .clone()
        .unwrap_or_else(|| options.asset_manifest.clone());
    let mut current_manifest = options.asset_manifest.clone();

    let sweep = if options.skip_sweep {
        Value::Null
    } else {
        let response = sweep_identity_assets(
            &current_manifest,
            runtime_grace_seconds,
            dispatch_grace_seconds,
            cooldown_grace_seconds,
            Some(&working_manifest),
            options.sweep_out.as_deref(),
        )
        .await?;
        current_manifest = working_manifest.clone();
        response.data.unwrap_or(Value::Null)
    };

    let validation = if options.skip_validate {
        Value::Null
    } else {
        let response =
            validate_identity_assets(&current_manifest, false, options.validate_out.as_deref())
                .await?;
        let data = response.data.unwrap_or(Value::Null);
        if data.get("valid").and_then(Value::as_bool) == Some(false) {
            let report = identity_job_run_report(
                &run_id,
                generated_at,
                &options.asset_manifest,
                &working_manifest,
                &worker_id,
                &job_id,
                options.job_preset.as_deref(),
                desired_concurrency,
                limit,
                &command,
                "validate",
                false,
                2,
                sweep,
                data,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                &failure_reason_rules,
                Some("profile asset manifest is invalid"),
                options.runtime_risk_out.as_deref(),
                options.append_runtime_risk,
                options.explain_out.as_deref(),
                options.job_out.as_deref(),
            )
            .await?;
            return Ok(identity_job_json_response(report, policy.as_ref()));
        }
        data
    };

    let mut runtime_risk_gate = Value::Null;
    if !options.runtime_risk_ledgers.is_empty() {
        runtime_risk_gate = evaluate_identity_job_runtime_risk_gate(
            &options.runtime_risk_ledgers,
            options.runtime_risk_window_seconds.unwrap_or(900),
            generated_at,
            &job_id,
            limit,
            desired_concurrency,
        )
        .await?;
        if runtime_risk_gate
            .get("blocked")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let report = identity_job_run_report(
                &run_id,
                generated_at,
                &options.asset_manifest,
                &working_manifest,
                &worker_id,
                &job_id,
                options.job_preset.as_deref(),
                desired_concurrency,
                limit,
                &command,
                "runtime_risk_gate",
                false,
                2,
                sweep,
                validation,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                runtime_risk_gate,
                &failure_reason_rules,
                Some("identity-job stopped by runtime risk ledger before leasing assets"),
                options.runtime_risk_out.as_deref(),
                options.append_runtime_risk,
                options.explain_out.as_deref(),
                options.job_out.as_deref(),
            )
            .await?;
            return Ok(identity_job_json_response(report, policy.as_ref()));
        }
        if let Some(next_limit) =
            identity_asset_field_u64(&runtime_risk_gate, &["nextSuggestedLimit"])
                .map(|value| value as usize)
                .filter(|value| *value > 0 && *value < limit)
        {
            limit = next_limit;
        }
        if let Some(next_desired_concurrency) =
            identity_asset_field_u64(&runtime_risk_gate, &["nextSuggestedDesiredConcurrency"])
                .map(|value| value as usize)
                .filter(|value| *value > 0 && *value < desired_concurrency)
        {
            desired_concurrency = next_desired_concurrency;
        }
    }

    let gate_response = gate_identity_assets(
        &current_manifest,
        desired_concurrency,
        options.max_wait_seconds,
        options.allow_wait,
        &options.allow_states,
        options.include_dispatch_leased,
        options.include_retry,
        options.include_failed,
        options.include_cancelled,
        options.include_runtime_leased,
        options.include_missing_profile_dir,
        options.gate_out.as_deref(),
    )
    .await?;
    let gate = gate_response.data.unwrap_or(Value::Null);
    if gate.get("passed").and_then(Value::as_bool).unwrap_or(false) == false {
        let exit_code = gate
            .get("exitCode")
            .and_then(Value::as_i64)
            .unwrap_or(2)
            .clamp(1, 255) as i32;
        let report = identity_job_run_report(
            &run_id,
            generated_at,
            &options.asset_manifest,
            &working_manifest,
            &worker_id,
            &job_id,
            options.job_preset.as_deref(),
            desired_concurrency,
            limit,
            &command,
            "gate",
            false,
            exit_code,
            sweep,
            validation,
            gate,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            runtime_risk_gate,
            &failure_reason_rules,
            Some("identity asset capacity gate did not pass"),
            options.runtime_risk_out.as_deref(),
            options.append_runtime_risk,
            options.explain_out.as_deref(),
            options.job_out.as_deref(),
        )
        .await?;
        return Ok(identity_job_json_response(report, policy.as_ref()));
    }

    let selection_response = select_identity_assets(
        &current_manifest,
        limit,
        &options.allow_states,
        Some(&worker_id),
        Some(&job_id),
        lease_seconds,
        options.include_dispatch_leased,
        options.include_retry,
        options.include_failed,
        options.include_cancelled,
        options.include_runtime_leased,
        options.include_missing_profile_dir,
        Some(&working_manifest),
        options.selection_out.as_deref(),
    )
    .await?;
    let selection = selection_response.data.unwrap_or(Value::Null);
    let selected_count = selection
        .get("selectedCount")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    if selected_count < limit {
        let report = identity_job_run_report(
            &run_id,
            generated_at,
            &options.asset_manifest,
            &working_manifest,
            &worker_id,
            &job_id,
            options.job_preset.as_deref(),
            desired_concurrency,
            limit,
            &command,
            "select",
            false,
            2,
            sweep,
            validation,
            gate,
            selection,
            Value::Null,
            Value::Null,
            Value::Null,
            runtime_risk_gate,
            &failure_reason_rules,
            Some("not enough profile assets could be leased after gate"),
            options.runtime_risk_out.as_deref(),
            options.append_runtime_risk,
            options.explain_out.as_deref(),
            options.job_out.as_deref(),
        )
        .await?;
        return Ok(identity_job_json_response(report, policy.as_ref()));
    }

    if options.per_asset {
        let (per_asset_result, lease_renewal) = run_identity_job_with_runtime_renewal(
            run_identity_job_per_asset_children(
                &command,
                &run_id,
                &working_manifest,
                &worker_id,
                &job_id,
                &selection,
                options.failure_cooldown_seconds,
                options.failure_next_state.as_deref(),
                options.release_out.as_deref(),
                options.append_release,
                child_concurrency,
                child_timeout_seconds,
                child_result_dir.as_deref(),
                max_failed_assets,
                max_failed_assets_per_reason,
                &failure_reason_rules,
            ),
            runtime_renew_interval_seconds,
            &working_manifest,
            &worker_id,
            &job_id,
            &selection,
            lease_seconds,
        )
        .await;
        let (child, release, child_success, exit_code) = per_asset_result?;
        let report = identity_job_run_report(
            &run_id,
            generated_at,
            &options.asset_manifest,
            &working_manifest,
            &worker_id,
            &job_id,
            options.job_preset.as_deref(),
            desired_concurrency,
            limit,
            &command,
            "complete",
            child_success,
            exit_code,
            sweep,
            validation,
            gate,
            selection,
            child,
            release,
            lease_renewal,
            runtime_risk_gate,
            &failure_reason_rules,
            None,
            options.runtime_risk_out.as_deref(),
            options.append_runtime_risk,
            options.explain_out.as_deref(),
            options.job_out.as_deref(),
        )
        .await?;
        return Ok(identity_job_json_response(report, policy.as_ref()));
    }

    let (child, lease_renewal) = run_identity_job_with_runtime_renewal(
        run_identity_job_child(
            &command,
            &run_id,
            &working_manifest,
            &worker_id,
            &job_id,
            &selection,
            None,
            None,
            child_timeout_seconds,
            child_result_dir.as_deref(),
        ),
        runtime_renew_interval_seconds,
        &working_manifest,
        &worker_id,
        &job_id,
        &selection,
        lease_seconds,
    )
    .await;
    let child_success = child
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let release_status = identity_job_child_release_status(&child);
    let failure_reason = identity_job_child_failure_reason(&child, &release_status);
    let failure_reason_rule =
        identity_job_failure_reason_rule(&failure_reason_rules, failure_reason.as_deref());
    let release_cooldown_seconds = identity_job_child_release_cooldown(
        &child,
        options.failure_cooldown_seconds,
        failure_reason_rule,
    );
    let release_next_state = identity_job_child_release_next_state(
        &child,
        options.failure_next_state.as_deref(),
        failure_reason_rule,
    );
    let child_exit_code = child
        .get("exitCode")
        .and_then(Value::as_i64)
        .map(|code| code.clamp(0, 255) as i32);
    let exit_code = if child_success {
        0
    } else {
        child_exit_code.unwrap_or(1).clamp(1, 255)
    };
    let release_result = json!({
        "identityJobRunId": run_id,
        "command": command.clone(),
        "success": child_success,
        "exitCode": child_exit_code,
        "timedOut": child.get("timedOut").and_then(Value::as_bool).unwrap_or(false),
        "timeoutSeconds": child.get("timeoutSeconds").cloned().unwrap_or(Value::Null),
        "spawned": child.get("spawned").and_then(Value::as_bool).unwrap_or(false),
        "releaseOverride": child.get("releaseOverride").cloned().unwrap_or(Value::Null),
        "resultOut": child.get("resultOut").cloned().unwrap_or(Value::Null),
        "failureReason": failure_reason,
        "failureReasonRuleApplied": failure_reason_rule.is_some(),
    });
    let release_message = identity_job_child_release_message(&child).unwrap_or_else(|| {
        if child_success {
            format!("identity-job run {run_id} succeeded")
        } else {
            format!("identity-job run {run_id} failed")
        }
    });
    let release_response = release_identity_assets(
        &working_manifest,
        &release_status,
        Some(&worker_id),
        Some(&job_id),
        &[],
        &[],
        &[],
        &[],
        &[],
        release_cooldown_seconds,
        release_next_state.as_deref(),
        Some(&release_message),
        Some(&release_result.to_string()),
        Some(&working_manifest),
        options.release_out.as_deref(),
        options.append_release,
    )
    .await?;
    let release = release_response.data.unwrap_or(Value::Null);
    let report = identity_job_run_report(
        &run_id,
        generated_at,
        &options.asset_manifest,
        &working_manifest,
        &worker_id,
        &job_id,
        options.job_preset.as_deref(),
        desired_concurrency,
        limit,
        &command,
        "complete",
        child_success,
        exit_code,
        sweep,
        validation,
        gate,
        selection,
        child,
        release,
        lease_renewal,
        runtime_risk_gate,
        &failure_reason_rules,
        None,
        options.runtime_risk_out.as_deref(),
        options.append_runtime_risk,
        options.explain_out.as_deref(),
        options.job_out.as_deref(),
    )
    .await?;

    Ok(identity_job_json_response(report, policy.as_ref()))
}

fn identity_job_json_response(
    report: Value,
    policy: Option<&LoadedIdentityPolicy>,
) -> JsonResponse {
    let mut response = JsonResponse::ok(report);
    attach_identity_policy(&mut response, policy);
    response
}

async fn run_identity_job_with_runtime_renewal<F, T>(
    future: F,
    interval_seconds: Option<u64>,
    working_manifest: &Path,
    worker_id: &str,
    job_id: &str,
    selection: &Value,
    lease_seconds: u64,
) -> (T, Value)
where
    F: Future<Output = T>,
{
    let Some(interval_seconds) = interval_seconds else {
        return (future.await, json!({ "enabled": false }));
    };

    let started_at = unix_seconds();
    let mut tick_count = 0usize;
    let mut renewed_count = 0usize;
    let mut error_count = 0usize;
    let mut items = Vec::new();
    let next_tick = sleep(Duration::from_secs(interval_seconds));
    tokio::pin!(future);
    tokio::pin!(next_tick);

    loop {
        tokio::select! {
            output = &mut future => {
                let completed_at = unix_seconds();
                let report = json!({
                    "enabled": true,
                    "scope": "identity_job_runtime_renewal",
                    "intervalSeconds": interval_seconds,
                    "leaseSeconds": lease_seconds,
                    "startedAtUnixSeconds": started_at,
                    "completedAtUnixSeconds": completed_at,
                    "tickCount": tick_count,
                    "renewedCount": renewed_count,
                    "errorCount": error_count,
                    "items": items,
                });
                return (output, report);
            }
            _ = &mut next_tick => {
                tick_count += 1;
                match renew_identity_job_runtime_leases(
                    working_manifest,
                    worker_id,
                    job_id,
                    selection,
                    lease_seconds,
                ).await {
                    Ok(report) => {
                        renewed_count += report
                            .get("renewedCount")
                            .and_then(Value::as_u64)
                            .unwrap_or(0) as usize;
                        items.push(report);
                    }
                    Err(error) => {
                        error_count += 1;
                        items.push(json!({
                            "scope": "identity_job_runtime_renewal",
                            "generatedAtUnixSeconds": unix_seconds(),
                            "error": error.to_string(),
                        }));
                    }
                }
                next_tick
                    .as_mut()
                    .reset(Instant::now() + Duration::from_secs(interval_seconds));
            }
        }
    }
}

async fn renew_identity_job_runtime_leases(
    asset_manifest: &Path,
    worker_id: &str,
    job_id: &str,
    selection: &Value,
    lease_seconds: u64,
) -> Result<Value> {
    let selected_assets = selection
        .get("selectedAssets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let lease_ids = selected_assets
        .iter()
        .flat_map(|asset| identity_job_asset_filter_values(asset, "leaseId"))
        .collect::<Vec<_>>();
    if lease_ids.is_empty() {
        bail!("identity-job runtime renewal requires selected runtime lease ids");
    }

    let text = tokio::fs::read_to_string(asset_manifest)
        .await
        .with_context(|| format!("failed to read asset manifest {}", asset_manifest.display()))?;
    let mut manifest = serde_json::from_str::<Value>(&text).with_context(|| {
        format!(
            "failed to parse asset manifest {}",
            asset_manifest.display()
        )
    })?;
    let generated_at = unix_seconds();
    let lease_expires = generated_at.saturating_add(lease_seconds);
    let lease_filter = normalize_identity_asset_filter_set(&lease_ids);
    let empty_filter = BTreeSet::new();
    let entries = identity_plan_manifest_entries_mut(&mut manifest)?;
    let asset_count = entries.len();
    let mut renewed_assets = Vec::new();
    let mut skipped_filter_count = 0usize;
    let mut skipped_non_leased_count = 0usize;

    for (asset_index, entry) in entries.iter_mut().enumerate() {
        if !identity_asset_matches_release_filters(
            entry,
            Some(worker_id),
            Some(job_id),
            &lease_filter,
            &empty_filter,
            &empty_filter,
            &empty_filter,
            &empty_filter,
        ) {
            skipped_filter_count += 1;
            continue;
        }
        if !identity_asset_runtime_lease_present(entry) {
            skipped_non_leased_count += 1;
            continue;
        }
        renewed_assets.push(apply_identity_asset_runtime_renewal(
            asset_index,
            entry,
            generated_at,
            lease_expires,
        ));
    }

    let bytes = serde_json::to_vec_pretty(&manifest)?;
    tokio::fs::write(asset_manifest, &bytes)
        .await
        .with_context(|| format!("failed to write {}", asset_manifest.display()))?;

    Ok(json!({
        "scope": "identity_job_runtime_renewal",
        "assetManifest": asset_manifest.display().to_string(),
        "generatedAtUnixSeconds": generated_at,
        "workerId": worker_id,
        "jobId": job_id,
        "leaseSeconds": lease_seconds,
        "leaseExpiresUnixSeconds": lease_expires,
        "filteredLeaseIds": lease_filter.iter().cloned().collect::<Vec<_>>(),
        "assetCount": asset_count,
        "matchedCount": renewed_assets.len() + skipped_non_leased_count,
        "renewedCount": renewed_assets.len(),
        "skippedFilterCount": skipped_filter_count,
        "skippedNonLeasedCount": skipped_non_leased_count,
        "items": renewed_assets,
    }))
}

fn apply_identity_asset_runtime_renewal(
    asset_index: usize,
    entry: &mut Value,
    renewed_at: u64,
    lease_expires: u64,
) -> Value {
    let account_id = identity_asset_field_string(entry, &["accountId", "account_id"]);
    let profile_id = identity_asset_field_string(entry, &["profileId", "profile_id"]);
    let identity_id = identity_asset_field_string(entry, &["identityId", "identity_id"]);
    let label = identity_asset_field_string(entry, &["label", "name"]);
    let profile_dir = identity_asset_profile_dir(entry);
    let lease_id = identity_asset_field_string(entry, &["runtimeLeaseId", "runtime_lease_id"]);
    let previous_lease_expires = identity_asset_field_u64(
        entry,
        &[
            "runtimeLeaseExpiresUnixSeconds",
            "runtime_lease_expires_unix_seconds",
        ],
    );
    let previous_renewal_count = identity_asset_field_u64(
        entry,
        &["runtimeLeaseRenewalCount", "runtime_lease_renewal_count"],
    )
    .unwrap_or(0);
    let renewal_count = previous_renewal_count.saturating_add(1);

    if let Value::Object(map) = entry {
        if let Some(previous_lease_expires) = previous_lease_expires {
            map.insert(
                "runtimeLeasePreviousExpiresUnixSeconds".to_string(),
                json!(previous_lease_expires),
            );
        }
        map.insert(
            "runtimeLeaseExpiresUnixSeconds".to_string(),
            json!(lease_expires),
        );
        map.insert(
            "runtimeLeaseRenewedAtUnixSeconds".to_string(),
            json!(renewed_at),
        );
        map.insert("runtimeLeaseRenewalCount".to_string(), json!(renewal_count));
    }

    json!({
        "assetIndex": asset_index,
        "accountId": account_id,
        "profileId": profile_id,
        "identityId": identity_id,
        "label": label,
        "profileDir": profile_dir,
        "leaseId": lease_id,
        "previousLeaseExpiresUnixSeconds": previous_lease_expires,
        "leaseExpiresUnixSeconds": lease_expires,
        "renewedAtUnixSeconds": renewed_at,
        "renewalCount": renewal_count,
    })
}

async fn prepare_identity_job_child_result_out(path: &Path) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to remove stale child result {}", path.display())),
    }
}

async fn apply_identity_job_child_result_out(child: &mut Value, path: &Path) {
    let mut result_out = json!({
        "path": path.display().to_string(),
        "exists": false,
        "valid": false,
    });
    let text = match tokio::fs::read_to_string(path).await {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            child["resultOut"] = result_out;
            return;
        }
        Err(error) => {
            result_out["error"] = Value::String(error.to_string());
            child["resultOut"] = result_out;
            child["success"] = Value::Bool(false);
            child["resultProtocolError"] = Value::Bool(true);
            return;
        }
    };
    result_out["exists"] = Value::Bool(true);

    let result = match serde_json::from_str::<Value>(&text) {
        Ok(result) => result,
        Err(error) => {
            result_out["error"] = Value::String(error.to_string());
            child["resultOut"] = result_out;
            child["success"] = Value::Bool(false);
            child["resultProtocolError"] = Value::Bool(true);
            child["releaseOverride"] = json!({
                "status": "failed",
                "message": "child result JSON is invalid",
            });
            return;
        }
    };
    result_out["valid"] = Value::Bool(true);
    result_out["result"] = result.clone();
    child["resultOut"] = result_out;

    let Some(result_object) = result.as_object() else {
        child["success"] = Value::Bool(false);
        child["resultProtocolError"] = Value::Bool(true);
        child["releaseOverride"] = json!({
            "status": "failed",
            "message": "child result JSON must be an object",
            "result": result,
        });
        return;
    };

    let mut release_override = serde_json::Map::new();
    if let Some(status) = identity_asset_field_string(&result, &["status", "releaseStatus"]) {
        match normalize_identity_asset_release_status(&status) {
            Ok(status) => {
                release_override.insert("status".to_string(), Value::String(status.clone()));
                let process_success = child
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                child["success"] = Value::Bool(process_success && status == "succeeded");
            }
            Err(error) => {
                child["success"] = Value::Bool(false);
                child["resultProtocolError"] = Value::Bool(true);
                release_override.insert("status".to_string(), Value::String("failed".to_string()));
                release_override.insert("message".to_string(), Value::String(error.to_string()));
            }
        }
    }
    if let Some(cooldown_seconds) =
        identity_asset_field_u64(&result, &["cooldownSeconds", "cooldown_seconds"])
    {
        release_override.insert("cooldownSeconds".to_string(), json!(cooldown_seconds));
    }
    if let Some(next_state) = identity_asset_field_string(&result, &["nextState", "next_state"]) {
        release_override.insert("nextState".to_string(), Value::String(next_state));
    }
    if let Some(message) = identity_asset_field_string(&result, &["message"]) {
        release_override.insert("message".to_string(), Value::String(message));
    }
    if let Some(reason) =
        identity_asset_field_string(&result, &["reason", "failureReason", "failure_reason"])
    {
        release_override.insert("reason".to_string(), Value::String(reason));
    }
    if let Some(value) = result_object.get("result").cloned() {
        release_override.insert("result".to_string(), value);
    } else {
        release_override.insert("result".to_string(), result);
    }
    if !release_override.is_empty() {
        child["releaseOverride"] = Value::Object(release_override);
    }
}

fn identity_job_child_release_status(child: &Value) -> String {
    child
        .get("releaseOverride")
        .and_then(|release| release.get("status"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            if child
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "succeeded".to_string()
            } else {
                "failed".to_string()
            }
        })
}

fn identity_job_child_release_cooldown(
    child: &Value,
    fallback_failure_cooldown_seconds: Option<u64>,
    failure_reason_rule: Option<&IdentityJobFailureReasonRule>,
) -> Option<u64> {
    child
        .get("releaseOverride")
        .and_then(|release| release.get("cooldownSeconds"))
        .and_then(Value::as_u64)
        .or_else(|| {
            (identity_job_child_release_status(child) != "succeeded")
                .then(|| {
                    failure_reason_rule
                        .and_then(|rule| rule.cooldown_seconds)
                        .or(fallback_failure_cooldown_seconds)
                })
                .flatten()
        })
}

fn identity_job_child_release_next_state(
    child: &Value,
    fallback_failure_next_state: Option<&str>,
    failure_reason_rule: Option<&IdentityJobFailureReasonRule>,
) -> Option<String> {
    child
        .get("releaseOverride")
        .and_then(|release| release.get("nextState"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            (identity_job_child_release_status(child) != "succeeded").then(|| {
                failure_reason_rule
                    .and_then(|rule| rule.next_state.clone())
                    .or_else(|| fallback_failure_next_state.map(ToString::to_string))
            })?
        })
}

fn identity_job_child_release_message(child: &Value) -> Option<String> {
    child
        .get("releaseOverride")
        .and_then(|release| release.get("message"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn identity_job_child_failure_reason(child: &Value, status: &str) -> Option<String> {
    if status == "succeeded" {
        return None;
    }
    if let Some(reason) = child
        .get("releaseOverride")
        .and_then(identity_job_failure_reason_from_value)
    {
        return Some(reason);
    }
    if child
        .get("resultProtocolError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some("result_protocol_error".to_string());
    }
    if child
        .get("timedOut")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some("timeout".to_string());
    }
    if child.get("spawned").and_then(Value::as_bool) == Some(false) {
        return Some("spawn_failed".to_string());
    }
    None
}

fn identity_job_failure_reason_from_value(value: &Value) -> Option<String> {
    identity_asset_field_string(
        value,
        &[
            "reason",
            "failureReason",
            "failure_reason",
            "code",
            "errorCode",
        ],
    )
    .and_then(|reason| normalize_identity_job_failure_reason(&reason))
    .or_else(|| {
        value
            .get("releaseOverride")
            .and_then(identity_job_failure_reason_from_value)
    })
    .or_else(|| {
        value
            .get("result")
            .and_then(|result| {
                identity_asset_field_string(
                    result,
                    &[
                        "reason",
                        "failureReason",
                        "failure_reason",
                        "code",
                        "errorCode",
                    ],
                )
            })
            .and_then(|reason| normalize_identity_job_failure_reason(&reason))
    })
}

fn normalize_identity_job_failure_reason(reason: &str) -> Option<String> {
    let mut normalized = String::new();
    let mut previous_was_separator = false;
    for character in reason.trim().chars() {
        if character.is_whitespace() {
            if !normalized.is_empty() && !previous_was_separator {
                normalized.push('_');
                previous_was_separator = true;
            }
            continue;
        }
        for lowercase in character.to_lowercase() {
            normalized.push(lowercase);
        }
        previous_was_separator = false;
    }
    let normalized = normalized.trim_matches('_').to_string();
    (!normalized.is_empty()).then_some(normalized)
}

fn normalize_identity_job_failure_reason_rules(
    rules: &BTreeMap<String, IdentityJobFailureReasonRule>,
) -> BTreeMap<String, IdentityJobFailureReasonRule> {
    rules
        .iter()
        .filter_map(|(reason, rule)| {
            normalize_identity_job_failure_reason(reason).map(|reason| (reason, rule.clone()))
        })
        .collect()
}

fn identity_job_failure_reason_rule<'a>(
    rules: &'a BTreeMap<String, IdentityJobFailureReasonRule>,
    failure_reason: Option<&str>,
) -> Option<&'a IdentityJobFailureReasonRule> {
    failure_reason
        .and_then(normalize_identity_job_failure_reason)
        .and_then(|reason| rules.get(&reason))
}

async fn run_identity_job_child(
    command: &[String],
    run_id: &str,
    working_manifest: &Path,
    worker_id: &str,
    job_id: &str,
    selection: &Value,
    asset: Option<&Value>,
    child_index: Option<usize>,
    child_timeout_seconds: Option<u64>,
    child_result_dir: Option<&Path>,
) -> Value {
    let Some(program) = command.first() else {
        return json!({
            "spawned": false,
            "success": false,
            "exitCode": null,
            "error": "missing command",
        });
    };
    let selected_assets = asset
        .map(|asset| Value::Array(vec![asset.clone()]))
        .or_else(|| selection.get("selectedAssets").cloned())
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let child_result_out = child_result_dir.map(|dir| {
        if let Some(index) = child_index {
            dir.join(format!("child-{index}.json"))
        } else {
            dir.join("child-result.json")
        }
    });
    if let Some(path) = child_result_out.as_deref() {
        if let Err(error) = prepare_identity_job_child_result_out(path).await {
            return json!({
                "spawned": false,
                "success": false,
                "exitCode": null,
                "timedOut": false,
                "resultOut": {
                    "path": path.display().to_string(),
                    "exists": false,
                    "valid": false,
                    "error": error.to_string(),
                },
                "error": error.to_string(),
            });
        }
    }

    let mut child = TokioCommand::new(program);
    child.args(command.iter().skip(1));
    if child_timeout_seconds.is_some() {
        child.kill_on_drop(true);
    }
    child.env("DRS_IDENTITY_JOB_RUN_ID", run_id);
    child.env("DRS_IDENTITY_ASSET_MANIFEST", working_manifest);
    child.env("DRS_IDENTITY_WORKER", worker_id);
    child.env("DRS_IDENTITY_JOB", job_id);
    if let Some(path) = child_result_out.as_deref() {
        child.env("DRS_IDENTITY_RESULT_OUT", path);
        child.env("DRS_IDENTITY_CHILD_RESULT_OUT", path);
    }
    child.env(
        "DRS_IDENTITY_SELECTED_COUNT",
        asset
            .map(|_| 1)
            .or_else(|| selection.get("selectedCount").and_then(Value::as_u64))
            .unwrap_or(0)
            .to_string(),
    );
    child.env(
        "DRS_IDENTITY_SELECTED_ASSETS_JSON",
        selected_assets.to_string(),
    );
    if let Some(selection_out) = selection
        .get("selectionOut")
        .and_then(|out| out.get("path"))
        .and_then(Value::as_str)
    {
        child.env("DRS_IDENTITY_SELECTION_OUT", selection_out);
    }
    if let Some(index) = child_index {
        child.env("DRS_IDENTITY_CHILD_INDEX", index.to_string());
    }
    if let Some(asset) = asset {
        child.env("DRS_IDENTITY_ASSET_JSON", asset.to_string());
        set_identity_job_asset_env(&mut child, "DRS_IDENTITY_ASSET_INDEX", asset, "assetIndex");
        set_identity_job_asset_env(&mut child, "DRS_IDENTITY_ACCOUNT_ID", asset, "accountId");
        set_identity_job_asset_env(&mut child, "DRS_IDENTITY_PROFILE_ID", asset, "profileId");
        set_identity_job_asset_env(&mut child, "DRS_IDENTITY_IDENTITY_ID", asset, "identityId");
        set_identity_job_asset_env(&mut child, "DRS_IDENTITY_LABEL", asset, "label");
        set_identity_job_asset_env(&mut child, "DRS_IDENTITY_PROFILE_DIR", asset, "profileDir");
        set_identity_job_asset_env(&mut child, "DRS_IDENTITY_PROXY_ID", asset, "proxyId");
        set_identity_job_asset_env(
            &mut child,
            "DRS_IDENTITY_FINGERPRINT_SEED",
            asset,
            "fingerprintSeed",
        );
        set_identity_job_asset_env(
            &mut child,
            "DRS_IDENTITY_RUNTIME_LEASE_ID",
            asset,
            "leaseId",
        );
    }

    let output = match child_timeout_seconds {
        Some(timeout_seconds) => {
            match tokio::time::timeout(Duration::from_secs(timeout_seconds), child.output()).await {
                Ok(output) => output,
                Err(_) => {
                    return json!({
                        "spawned": true,
                        "success": false,
                        "exitCode": null,
                        "timedOut": true,
                        "timeoutSeconds": timeout_seconds,
                        "error": format!("child command timed out after {timeout_seconds} seconds"),
                    });
                }
            }
        }
        None => child.output().await,
    };

    let mut report = match output {
        Ok(output) => {
            let exit_code = output.status.code();
            json!({
                "spawned": true,
                "success": output.status.success(),
                "exitCode": exit_code,
                "timedOut": false,
                "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                "stdoutBytes": output.stdout.len(),
                "stderrBytes": output.stderr.len(),
            })
        }
        Err(error) => json!({
            "spawned": false,
            "success": false,
            "exitCode": null,
            "timedOut": false,
            "error": error.to_string(),
        }),
    };
    if let Some(path) = child_result_out.as_deref() {
        apply_identity_job_child_result_out(&mut report, path).await;
    }
    report
}

#[allow(clippy::too_many_arguments)]
async fn run_identity_job_per_asset_children(
    command: &[String],
    run_id: &str,
    working_manifest: &Path,
    worker_id: &str,
    job_id: &str,
    selection: &Value,
    failure_cooldown_seconds: Option<u64>,
    failure_next_state: Option<&str>,
    release_out: Option<&Path>,
    append_release: bool,
    child_concurrency: usize,
    child_timeout_seconds: Option<u64>,
    child_result_dir: Option<&Path>,
    max_failed_assets: Option<usize>,
    max_failed_assets_per_reason: Option<usize>,
    failure_reason_rules: &BTreeMap<String, IdentityJobFailureReasonRule>,
) -> Result<(Value, Value, bool, i32)> {
    let selected_assets = selection
        .get("selectedAssets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let child_concurrency = child_concurrency.max(1);
    let indexed_assets = selected_assets
        .iter()
        .cloned()
        .enumerate()
        .collect::<Vec<_>>();
    let mut children = Vec::new();
    let mut releases = Vec::new();
    let mut succeeded_count = 0usize;
    let mut failed_count = 0usize;
    let mut skipped_count = 0usize;
    let mut cancelled_count = 0usize;
    let mut released_count = 0usize;
    let mut first_failure_exit_code = None;
    let mut failure_reason_counts = BTreeMap::<String, usize>::new();
    let mut circuit_breaker = json!({
        "enabled": max_failed_assets.is_some() || max_failed_assets_per_reason.is_some(),
        "maxFailedAssets": max_failed_assets,
        "maxFailedAssetsPerReason": max_failed_assets_per_reason,
        "tripped": false,
        "skippedCount": 0,
        "failureReasonCounts": {},
    });
    let mut next_start = 0usize;

    while next_start < indexed_assets.len() {
        let chunk_end = (next_start + child_concurrency).min(indexed_assets.len());
        let chunk = &indexed_assets[next_start..chunk_end];
        let futures = chunk.iter().map(|(child_index, asset)| {
            let command = command.to_vec();
            let run_id = run_id.to_string();
            let working_manifest = working_manifest.to_path_buf();
            let worker_id = worker_id.to_string();
            let job_id = job_id.to_string();
            let selection = selection.clone();
            let asset = asset.clone();
            let child_index = *child_index;
            let child_result_dir = child_result_dir.map(Path::to_path_buf);
            async move {
                let child = run_identity_job_child(
                    &command,
                    &run_id,
                    &working_manifest,
                    &worker_id,
                    &job_id,
                    &selection,
                    Some(&asset),
                    Some(child_index),
                    child_timeout_seconds,
                    child_result_dir.as_deref(),
                )
                .await;
                (child_index, asset, child)
            }
        });
        let mut child_results = join_all(futures).await;
        child_results.sort_by_key(|(child_index, _, _)| *child_index);
        next_start = chunk_end;

        for (child_index, asset, child) in child_results {
            let child_success = child
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let status = identity_job_child_release_status(&child);
            let failure_reason = identity_job_child_failure_reason(&child, &status);
            let failure_reason_rule =
                identity_job_failure_reason_rule(failure_reason_rules, failure_reason.as_deref());
            let release_cooldown_seconds = identity_job_child_release_cooldown(
                &child,
                failure_cooldown_seconds,
                failure_reason_rule,
            );
            let release_next_state = identity_job_child_release_next_state(
                &child,
                failure_next_state,
                failure_reason_rule,
            );
            if child_success {
                succeeded_count += 1;
            } else {
                failed_count += 1;
                if first_failure_exit_code.is_none() {
                    first_failure_exit_code = child
                        .get("exitCode")
                        .and_then(Value::as_i64)
                        .map(|code| code.clamp(1, 255) as i32)
                        .or(Some(1));
                }
                if let Some(reason) = &failure_reason {
                    *failure_reason_counts.entry(reason.clone()).or_insert(0) += 1;
                }
            }

            let release_result = json!({
                "identityJobRunId": run_id,
                "mode": "per_asset",
                "childIndex": child_index,
                "command": command,
                "success": child_success,
                "exitCode": child.get("exitCode").cloned().unwrap_or(Value::Null),
                "timedOut": child.get("timedOut").and_then(Value::as_bool).unwrap_or(false),
                "timeoutSeconds": child.get("timeoutSeconds").cloned().unwrap_or(Value::Null),
                "spawned": child.get("spawned").and_then(Value::as_bool).unwrap_or(false),
                "releaseOverride": child.get("releaseOverride").cloned().unwrap_or(Value::Null),
                "resultOut": child.get("resultOut").cloned().unwrap_or(Value::Null),
                "failureReason": failure_reason.clone(),
                "failureReasonRuleApplied": failure_reason_rule.is_some(),
                "asset": asset,
            });
            let release_message = identity_job_child_release_message(&child).unwrap_or_else(|| {
                if child_success {
                    format!("identity-job run {run_id} child {child_index} succeeded")
                } else {
                    format!("identity-job run {run_id} child {child_index} failed")
                }
            });
            let lease_ids = identity_job_asset_filter_values(&asset, "leaseId");
            let account_ids = if lease_ids.is_empty() {
                identity_job_asset_filter_values(&asset, "accountId")
            } else {
                Vec::new()
            };
            let profile_ids = if lease_ids.is_empty() {
                identity_job_asset_filter_values(&asset, "profileId")
            } else {
                Vec::new()
            };
            let identity_ids = if lease_ids.is_empty() {
                identity_job_asset_filter_values(&asset, "identityId")
            } else {
                Vec::new()
            };
            let labels = if lease_ids.is_empty() {
                identity_job_asset_filter_values(&asset, "label")
            } else {
                Vec::new()
            };
            let release_response = release_identity_assets(
                working_manifest,
                &status,
                Some(worker_id),
                Some(job_id),
                &lease_ids,
                &account_ids,
                &profile_ids,
                &identity_ids,
                &labels,
                release_cooldown_seconds,
                release_next_state.as_deref(),
                Some(&release_message),
                Some(&release_result.to_string()),
                Some(working_manifest),
                release_out,
                append_release,
            )
            .await?;
            let release = release_response.data.unwrap_or(Value::Null);
            released_count += release
                .get("releasedCount")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            children.push(json!({
                "childIndex": child_index,
                "asset": asset,
                "failureReason": failure_reason.clone(),
                "child": child,
            }));
            releases.push(json!({
                "childIndex": child_index,
                "status": status,
                "failureReason": failure_reason.clone(),
                "asset": asset,
                "release": release,
            }));
        }

        let mut breaker_trip = None;
        if let Some(max_failed_assets) = max_failed_assets {
            if failed_count >= max_failed_assets {
                breaker_trip = Some(("failed_assets", None, None));
            }
        }
        if breaker_trip.is_none() {
            if let Some(max_failed_assets_per_reason) = max_failed_assets_per_reason {
                if let Some((reason, count)) = failure_reason_counts
                    .iter()
                    .find(|(_, count)| **count >= max_failed_assets_per_reason)
                {
                    breaker_trip = Some(("failure_reason", Some(reason.clone()), Some(*count)));
                }
            }
        }

        if let Some((trip_kind, trip_reason, trip_reason_count)) = breaker_trip {
            if next_start < indexed_assets.len() {
                let remaining_assets = &indexed_assets[next_start..];
                skipped_count = remaining_assets.len();
                let message = match (&trip_reason, trip_reason_count) {
                    (Some(reason), Some(count)) => format!(
                        "identity-job per-asset circuit breaker tripped after {count} failed assets with reason {reason}"
                    ),
                    _ => format!(
                        "identity-job per-asset circuit breaker tripped after {failed_count} failed assets"
                    ),
                };
                circuit_breaker = json!({
                    "enabled": true,
                    "maxFailedAssets": max_failed_assets,
                    "maxFailedAssetsPerReason": max_failed_assets_per_reason,
                    "tripped": true,
                    "kind": trip_kind,
                    "failedCount": failed_count,
                    "failureReason": trip_reason,
                    "failureReasonCount": trip_reason_count,
                    "failureReasonCounts": failure_reason_counts.clone(),
                    "skippedCount": skipped_count,
                    "message": message,
                });
                for (child_index, asset) in remaining_assets {
                    let release_result = json!({
                        "identityJobRunId": run_id,
                        "mode": "per_asset",
                        "childIndex": child_index,
                        "command": command,
                        "success": false,
                        "exitCode": Value::Null,
                        "spawned": false,
                        "cancelledByCircuitBreaker": true,
                        "circuitBreaker": circuit_breaker.clone(),
                        "asset": asset,
                    });
                    let release_message = format!(
                        "identity-job run {run_id} child {child_index} cancelled by circuit breaker"
                    );
                    let lease_ids = identity_job_asset_filter_values(asset, "leaseId");
                    let account_ids = if lease_ids.is_empty() {
                        identity_job_asset_filter_values(asset, "accountId")
                    } else {
                        Vec::new()
                    };
                    let profile_ids = if lease_ids.is_empty() {
                        identity_job_asset_filter_values(asset, "profileId")
                    } else {
                        Vec::new()
                    };
                    let identity_ids = if lease_ids.is_empty() {
                        identity_job_asset_filter_values(asset, "identityId")
                    } else {
                        Vec::new()
                    };
                    let labels = if lease_ids.is_empty() {
                        identity_job_asset_filter_values(asset, "label")
                    } else {
                        Vec::new()
                    };
                    let release_response = release_identity_assets(
                        working_manifest,
                        "cancelled",
                        Some(worker_id),
                        Some(job_id),
                        &lease_ids,
                        &account_ids,
                        &profile_ids,
                        &identity_ids,
                        &labels,
                        None,
                        None,
                        Some(&release_message),
                        Some(&release_result.to_string()),
                        Some(working_manifest),
                        release_out,
                        append_release,
                    )
                    .await?;
                    let release = release_response.data.unwrap_or(Value::Null);
                    released_count += release
                        .get("releasedCount")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize;
                    cancelled_count += 1;
                    releases.push(json!({
                        "childIndex": child_index,
                        "status": "cancelled",
                        "skipped": true,
                        "circuitBreaker": circuit_breaker.clone(),
                        "asset": asset,
                        "release": release,
                    }));
                }
                break;
            }
        }
    }

    let success = failed_count == 0 && cancelled_count == 0;
    let exit_code = if success {
        0
    } else {
        first_failure_exit_code.unwrap_or(1)
    };
    let child = json!({
        "mode": "per_asset",
        "childConcurrency": child_concurrency,
        "selectedCount": selected_assets.len(),
        "success": success,
        "exitCode": exit_code,
        "childCount": children.len(),
        "succeededCount": succeeded_count,
        "failedCount": failed_count,
        "failureReasonCounts": failure_reason_counts.clone(),
        "skippedCount": skipped_count,
        "cancelledCount": cancelled_count,
        "circuitBreaker": circuit_breaker,
        "children": children,
    });
    let release = json!({
        "mode": "per_asset",
        "childConcurrency": child_concurrency,
        "releasedCount": released_count,
        "succeededCount": succeeded_count,
        "failedCount": failed_count,
        "failureReasonCounts": failure_reason_counts,
        "skippedCount": skipped_count,
        "cancelledCount": cancelled_count,
        "circuitBreaker": circuit_breaker,
        "items": releases,
    });
    Ok((child, release, success, exit_code))
}

fn set_identity_job_asset_env(child: &mut TokioCommand, name: &str, asset: &Value, field: &str) {
    if let Some(value) = identity_job_asset_env_value(asset, field) {
        child.env(name, value);
    }
}

fn identity_job_asset_filter_values(asset: &Value, field: &str) -> Vec<String> {
    identity_job_asset_env_value(asset, field)
        .into_iter()
        .collect()
}

fn identity_job_asset_env_value(asset: &Value, field: &str) -> Option<String> {
    let value = asset.get(field)?;
    if let Some(text) = value.as_str() {
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    if let Some(number) = value.as_u64() {
        return Some(number.to_string());
    }
    if let Some(number) = value.as_i64() {
        return Some(number.to_string());
    }
    None
}

#[allow(clippy::too_many_arguments)]
async fn identity_job_run_report(
    run_id: &str,
    generated_at: u64,
    asset_manifest: &Path,
    working_manifest: &Path,
    worker_id: &str,
    job_id: &str,
    job_preset: Option<&str>,
    desired_concurrency: usize,
    limit: usize,
    command: &[String],
    phase: &str,
    passed: bool,
    exit_code: i32,
    sweep: Value,
    validation: Value,
    gate: Value,
    selection: Value,
    child: Value,
    release: Value,
    lease_renewal: Value,
    runtime_risk_gate: Value,
    failure_reason_rules: &BTreeMap<String, IdentityJobFailureReasonRule>,
    message: Option<&str>,
    runtime_risk_out: Option<&Path>,
    append_runtime_risk: bool,
    explain_out: Option<&Path>,
    job_out: Option<&Path>,
) -> Result<Value> {
    let runtime_risk = identity_job_runtime_risk_report(
        generated_at,
        phase,
        passed,
        desired_concurrency,
        limit,
        &selection,
        &child,
        &release,
        &runtime_risk_gate,
        failure_reason_rules,
    );
    let mut report = json!({
        "scope": "identity_job_run",
        "identityJobRunId": run_id,
        "generatedAtUnixSeconds": generated_at,
        "assetManifest": asset_manifest.display().to_string(),
        "workingAssetManifest": working_manifest.display().to_string(),
        "workerId": worker_id,
        "jobId": job_id,
        "jobPreset": job_preset,
        "desiredConcurrency": desired_concurrency,
        "limit": limit,
        "command": command,
        "phase": phase,
        "passed": passed,
        "exitCode": exit_code,
        "sweep": sweep,
        "validation": validation,
        "gate": gate,
        "selection": selection,
        "child": child,
        "release": release,
        "leaseRenewal": lease_renewal,
        "runtimeRiskGate": runtime_risk_gate,
        "runtimeRisk": runtime_risk,
    });
    let explain = identity_job_explain_report(
        run_id,
        generated_at,
        phase,
        passed,
        exit_code,
        job_preset,
        desired_concurrency,
        limit,
        &sweep,
        &validation,
        &gate,
        &selection,
        &child,
        &release,
        &lease_renewal,
        &runtime_risk_gate,
        &runtime_risk,
        message,
    );
    report["explain"] = explain.clone();
    if let Some(message) = message {
        report["message"] = Value::String(message.to_string());
    }
    if let Some(runtime_risk_out) = runtime_risk_out {
        let event = identity_job_runtime_risk_event(
            run_id,
            generated_at,
            asset_manifest,
            working_manifest,
            worker_id,
            job_id,
            desired_concurrency,
            limit,
            command,
            exit_code,
            &runtime_risk,
        );
        let out =
            write_identity_job_runtime_risk_event(&event, runtime_risk_out, append_runtime_risk)
                .await?;
        report["runtimeRiskOut"] = out;
    }
    if let Some(explain_out) = explain_out {
        let out = write_identity_job_explain_report(&explain, explain_out).await?;
        report["explainOut"] = out;
    }
    if let Some(job_out) = job_out {
        if let Some(parent) = job_out
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(&report)?;
        tokio::fs::write(job_out, &bytes)
            .await
            .with_context(|| format!("failed to write {}", job_out.display()))?;
        report["jobOut"] = json!({
            "path": job_out.display().to_string(),
            "format": "json_report",
            "bytes": bytes.len(),
        });
    }
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn identity_job_explain_report(
    run_id: &str,
    generated_at: u64,
    phase: &str,
    passed: bool,
    exit_code: i32,
    job_preset: Option<&str>,
    desired_concurrency: usize,
    limit: usize,
    sweep: &Value,
    validation: &Value,
    gate: &Value,
    selection: &Value,
    child: &Value,
    release: &Value,
    lease_renewal: &Value,
    runtime_risk_gate: &Value,
    runtime_risk: &Value,
    message: Option<&str>,
) -> Value {
    let selected_count = identity_job_explain_usize(selection, &["selectedCount"]);
    let blocked_count = identity_job_explain_usize(selection, &["blockedCount"]);
    let released_count = identity_job_explain_usize(release, &["releasedCount"]);
    let child_success = child
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let failed_count = if !child.is_null()
        && identity_job_explain_usize(child, &["childCount"]) == 0
        && !child_success
    {
        1
    } else {
        identity_job_explain_usize(child, &["failedCount"])
    };
    let cancelled_count = identity_job_explain_usize(child, &["cancelledCount"]);
    let final_decision = if passed {
        "run_completed"
    } else if phase == "complete" {
        "run_failed"
    } else {
        "blocked_before_child"
    };

    json!({
        "scope": "identity_job_explain",
        "identityJobRunId": run_id,
        "generatedAtUnixSeconds": generated_at,
        "phase": phase,
        "passed": passed,
        "exitCode": exit_code,
        "jobPreset": job_preset,
        "finalDecision": final_decision,
        "blockingStage": (!passed).then_some(phase),
        "message": message,
        "desiredConcurrency": desired_concurrency,
        "limit": limit,
        "summary": {
            "selectedCount": selected_count,
            "blockedCount": blocked_count,
            "releasedCount": released_count,
            "failedCount": failed_count,
            "cancelledCount": cancelled_count,
            "runtimeRiskRecommendedAction": identity_asset_field_string(runtime_risk, &["recommendedAction"]),
            "runtimeRiskSeverity": identity_asset_field_string(runtime_risk, &["severity"]),
            "dominantFailureReason": identity_asset_field_string(runtime_risk, &["dominantFailureReason"]),
        },
        "stageDecisions": identity_job_explain_stage_decisions(
            desired_concurrency,
            limit,
            sweep,
            validation,
            gate,
            selection,
            child,
            release,
            lease_renewal,
            runtime_risk_gate,
            runtime_risk,
        ),
        "assetDecisions": identity_job_explain_asset_decisions(selection, child, release),
    })
}

#[allow(clippy::too_many_arguments)]
fn identity_job_explain_stage_decisions(
    desired_concurrency: usize,
    limit: usize,
    sweep: &Value,
    validation: &Value,
    gate: &Value,
    selection: &Value,
    child: &Value,
    release: &Value,
    lease_renewal: &Value,
    runtime_risk_gate: &Value,
    runtime_risk: &Value,
) -> Vec<Value> {
    let mut stages = Vec::new();
    if sweep.is_null() {
        stages.push(identity_job_explain_stage(
            "sweep",
            "skipped",
            "skip_sweep",
            Value::Null,
        ));
    } else {
        stages.push(identity_job_explain_stage(
            "sweep",
            "completed",
            "cleaned_runtime_state",
            json!({
                "assetCount": identity_job_explain_usize(sweep, &["assetCount"]),
                "updatedAssetCount": identity_job_explain_usize(sweep, &["updatedAssetCount"]),
                "expiredRuntimeLeaseCount": identity_job_explain_usize(sweep, &["expiredRuntimeLeaseCount"]),
                "expiredDispatchLeaseCount": identity_job_explain_usize(sweep, &["expiredDispatchLeaseCount"]),
                "clearedCooldownCount": identity_job_explain_usize(sweep, &["clearedCooldownCount"]),
            }),
        ));
    }

    if validation.is_null() {
        stages.push(identity_job_explain_stage(
            "validate",
            "skipped",
            "skip_validate",
            Value::Null,
        ));
    } else {
        let valid = validation
            .get("valid")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        stages.push(identity_job_explain_stage(
            "validate",
            if valid { "passed" } else { "blocked" },
            if valid {
                "manifest_valid"
            } else {
                "manifest_invalid"
            },
            json!({
                "errorCount": identity_job_explain_usize(validation, &["errorCount"]),
                "warningCount": identity_job_explain_usize(validation, &["warningCount"]),
                "issueCodeCounts": validation.get("issueCodeCounts").cloned().unwrap_or(Value::Null),
            }),
        ));
    }

    if runtime_risk_gate.is_null() {
        stages.push(identity_job_explain_stage(
            "runtime_risk_gate",
            "skipped",
            "no_runtime_risk_ledger",
            Value::Null,
        ));
    } else {
        let blocked = runtime_risk_gate
            .get("blocked")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let adjusted = runtime_risk_gate
            .get("adjusted")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        stages.push(identity_job_explain_stage(
            "runtime_risk_gate",
            if blocked {
                "blocked"
            } else if adjusted {
                "adjusted"
            } else {
                "passed"
            },
            identity_asset_field_string(runtime_risk_gate, &["recommendedAction"])
                .as_deref()
                .unwrap_or("continue_current"),
            json!({
                "blockReason": runtime_risk_gate.get("blockReason").cloned().unwrap_or(Value::Null),
                "failureReason": runtime_risk_gate.get("failureReason").cloned().unwrap_or(Value::Null),
                "activeSuppressionCount": identity_job_explain_usize(runtime_risk_gate, &["activeSuppressionCount"]),
                "expiredSuppressionCount": identity_job_explain_usize(runtime_risk_gate, &["expiredSuppressionCount"]),
                "nextSuggestedLimit": runtime_risk_gate.get("nextSuggestedLimit").cloned().unwrap_or(Value::Null),
                "nextSuggestedDesiredConcurrency": runtime_risk_gate.get("nextSuggestedDesiredConcurrency").cloned().unwrap_or(Value::Null),
                "message": runtime_risk_gate.get("message").cloned().unwrap_or(Value::Null),
            }),
        ));
    }

    if gate.is_null() {
        stages.push(identity_job_explain_stage(
            "gate",
            "skipped",
            "blocked_before_capacity_gate",
            Value::Null,
        ));
    } else {
        let gate_passed = gate.get("passed").and_then(Value::as_bool).unwrap_or(false);
        stages.push(identity_job_explain_stage(
            "gate",
            if gate_passed { "passed" } else { "blocked" },
            identity_asset_field_string(gate, &["decision"])
                .as_deref()
                .unwrap_or("unknown"),
            json!({
                "desiredConcurrency": desired_concurrency,
                "currentRunnableCount": identity_job_explain_usize(gate, &["currentRunnableCount"]),
                "currentShortageCount": identity_job_explain_usize(gate, &["currentShortageCount"]),
                "predictedRunnableCount": identity_job_explain_usize(gate, &["predictedRunnableCount"]),
                "predictedShortageCount": identity_job_explain_usize(gate, &["predictedShortageCount"]),
                "recommendedAction": gate.get("recommendedAction").cloned().unwrap_or(Value::Null),
                "message": gate.get("message").cloned().unwrap_or(Value::Null),
            }),
        ));
    }

    if selection.is_null() {
        stages.push(identity_job_explain_stage(
            "select",
            "skipped",
            "blocked_before_runtime_lease",
            Value::Null,
        ));
    } else {
        let selected_count = identity_job_explain_usize(selection, &["selectedCount"]);
        stages.push(identity_job_explain_stage(
            "select",
            if selected_count >= limit {
                "passed"
            } else {
                "blocked"
            },
            if selected_count >= limit {
                "runtime_leases_acquired"
            } else {
                "not_enough_runtime_leases"
            },
            json!({
                "requestedLimit": selection.get("requestedLimit").cloned().unwrap_or(json!(limit)),
                "selectedCount": selected_count,
                "blockedCount": identity_job_explain_usize(selection, &["blockedCount"]),
                "overflowCount": identity_job_explain_usize(selection, &["overflowCount"]),
                "blockReasonCounts": selection.get("blockReasonCounts").cloned().unwrap_or(Value::Null),
            }),
        ));
    }

    if child.is_null() {
        stages.push(identity_job_explain_stage(
            "child",
            "skipped",
            "child_not_started",
            Value::Null,
        ));
    } else {
        let success = child
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| identity_job_explain_usize(child, &["failedCount"]) == 0);
        let explicit_child_count = identity_job_explain_usize(child, &["childCount"]);
        let child_count = if explicit_child_count == 0 {
            1
        } else {
            explicit_child_count
        };
        let succeeded_count = if explicit_child_count == 0 && success {
            1
        } else {
            identity_job_explain_usize(child, &["succeededCount"])
        };
        let failed_count = if explicit_child_count == 0 && !success {
            1
        } else {
            identity_job_explain_usize(child, &["failedCount"])
        };
        stages.push(identity_job_explain_stage(
            "child",
            if success { "passed" } else { "failed" },
            if success {
                "wrapped_command_succeeded"
            } else {
                "wrapped_command_failed"
            },
            json!({
                "mode": child.get("mode").cloned().unwrap_or(Value::Null),
                "childCount": child_count,
                "succeededCount": succeeded_count,
                "failedCount": failed_count,
                "cancelledCount": identity_job_explain_usize(child, &["cancelledCount"]),
                "skippedCount": identity_job_explain_usize(child, &["skippedCount"]),
                "exitCode": child.get("exitCode").cloned().unwrap_or(Value::Null),
                "failureReasonCounts": child.get("failureReasonCounts").cloned().unwrap_or(Value::Null),
                "circuitBreaker": child.get("circuitBreaker").cloned().unwrap_or(Value::Null),
            }),
        ));
    }

    if lease_renewal.is_null() {
        stages.push(identity_job_explain_stage(
            "lease_renewal",
            "skipped",
            "runtime_renewal_not_needed",
            Value::Null,
        ));
    } else {
        let enabled = lease_renewal
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        stages.push(identity_job_explain_stage(
            "lease_renewal",
            if enabled { "completed" } else { "skipped" },
            if enabled {
                "runtime_leases_renewed"
            } else {
                "runtime_renewal_disabled"
            },
            lease_renewal.clone(),
        ));
    }

    if release.is_null() {
        stages.push(identity_job_explain_stage(
            "release",
            "skipped",
            "nothing_to_release",
            Value::Null,
        ));
    } else {
        let released_count = identity_job_explain_usize(release, &["releasedCount"]);
        let release_status = identity_asset_field_string(release, &["status"]);
        let succeeded_count = if release_status.as_deref() == Some("succeeded") {
            released_count
        } else {
            identity_job_explain_usize(release, &["succeededCount"])
        };
        let failed_count = if release_status.as_deref() == Some("failed") {
            released_count
        } else {
            identity_job_explain_usize(release, &["failedCount"])
        };
        let cancelled_count = if release_status.as_deref() == Some("cancelled") {
            released_count
        } else {
            identity_job_explain_usize(release, &["cancelledCount"])
        };
        stages.push(identity_job_explain_stage(
            "release",
            "completed",
            "runtime_leases_released",
            json!({
                "releasedCount": released_count,
                "succeededCount": succeeded_count,
                "failedCount": failed_count,
                "cancelledCount": cancelled_count,
                "skippedCount": identity_job_explain_usize(release, &["skippedCount"]),
                "failureReasonCounts": release.get("failureReasonCounts").cloned().unwrap_or(Value::Null),
            }),
        ));
    }

    stages.push(identity_job_explain_stage(
        "runtime_risk",
        if runtime_risk
            .get("passed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "passed"
        } else {
            "advised"
        },
        identity_asset_field_string(runtime_risk, &["recommendedAction"])
            .as_deref()
            .unwrap_or("inspect_job_report"),
        json!({
            "severity": runtime_risk.get("severity").cloned().unwrap_or(Value::Null),
            "nextSuggestedLimit": runtime_risk.get("nextSuggestedLimit").cloned().unwrap_or(Value::Null),
            "nextSuggestedDesiredConcurrency": runtime_risk.get("nextSuggestedDesiredConcurrency").cloned().unwrap_or(Value::Null),
            "dominantFailureReason": runtime_risk.get("dominantFailureReason").cloned().unwrap_or(Value::Null),
            "runtimeRiskCooldownSeconds": runtime_risk.get("runtimeRiskCooldownSeconds").cloned().unwrap_or(Value::Null),
            "suppressUntilUnixSeconds": runtime_risk.get("suppressUntilUnixSeconds").cloned().unwrap_or(Value::Null),
            "message": runtime_risk.get("message").cloned().unwrap_or(Value::Null),
        }),
    ));

    stages
}

fn identity_job_explain_stage(stage: &str, status: &str, decision: &str, details: Value) -> Value {
    json!({
        "stage": stage,
        "status": status,
        "decision": decision,
        "details": details,
    })
}

fn identity_job_explain_asset_decisions(
    selection: &Value,
    child: &Value,
    release: &Value,
) -> Vec<Value> {
    let mut decisions = Vec::new();
    if let Some(selected_assets) = selection.get("selectedAssets").and_then(Value::as_array) {
        for asset in selected_assets {
            decisions.push(json!({
                "phase": "select",
                "decision": "selected",
                "asset": identity_job_explain_asset_ref(asset),
                "leaseId": asset.get("leaseId").cloned().unwrap_or(Value::Null),
                "reasons": ["runtime_lease_acquired"],
            }));
        }
    }
    if let Some(blocked_assets) = selection.get("blockedAssets").and_then(Value::as_array) {
        for asset in blocked_assets {
            let reasons = string_values_for_keys(asset, &["reasons"]);
            decisions.push(json!({
                "phase": "select",
                "decision": "blocked",
                "asset": identity_job_explain_asset_ref(asset),
                "reasons": reasons,
            }));
        }
    }
    if let Some(children) = child.get("children").and_then(Value::as_array) {
        for child_item in children {
            let child_report = child_item.get("child").unwrap_or(child_item);
            let success = child_report
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let failure_reason = child_item
                .get("failureReason")
                .cloned()
                .or_else(|| child_report.get("failureReason").cloned())
                .unwrap_or(Value::Null);
            decisions.push(json!({
                "phase": "child",
                "decision": if success { "succeeded" } else { "failed" },
                "asset": identity_job_explain_asset_ref(child_item.get("asset").unwrap_or(child_item)),
                "childIndex": child_item.get("childIndex").cloned().unwrap_or(Value::Null),
                "exitCode": child_report.get("exitCode").cloned().unwrap_or(Value::Null),
                "failureReason": failure_reason,
                "timedOut": child_report.get("timedOut").cloned().unwrap_or(Value::Null),
            }));
        }
    }
    if let Some(released_assets) = release.get("releasedAssets").and_then(Value::as_array) {
        for asset in released_assets {
            decisions.push(json!({
                "phase": "release",
                "decision": "released",
                "asset": identity_job_explain_asset_ref(asset),
                "status": asset.get("status").cloned().unwrap_or(Value::Null),
                "leaseId": asset.get("leaseId").cloned().unwrap_or(Value::Null),
                "cooldownUntilUnixSeconds": asset.get("cooldownUntilUnixSeconds").cloned().unwrap_or(Value::Null),
            }));
        }
    }
    if let Some(items) = release.get("items").and_then(Value::as_array) {
        for item in items {
            decisions.push(json!({
                "phase": "release",
                "decision": "released",
                "asset": identity_job_explain_asset_ref(item.get("asset").unwrap_or(item)),
                "status": item.get("status").cloned().unwrap_or(Value::Null),
                "childIndex": item.get("childIndex").cloned().unwrap_or(Value::Null),
                "failureReason": item.get("failureReason").cloned().unwrap_or(Value::Null),
            }));
        }
    }
    decisions
}

fn identity_job_explain_asset_ref(asset: &Value) -> Value {
    json!({
        "assetIndex": identity_asset_field_u64(asset, &["assetIndex", "asset_index"]),
        "accountId": identity_asset_field_string(asset, &["accountId", "account_id"]),
        "profileId": identity_asset_field_string(asset, &["profileId", "profile_id"]),
        "identityId": identity_asset_field_string(asset, &["identityId", "identity_id"]),
        "label": identity_asset_field_string(asset, &["label", "name"]),
        "profileDir": identity_asset_profile_dir(asset),
        "state": identity_asset_field_string(asset, &["state"]),
        "dispatchState": identity_asset_field_string(asset, &["dispatchState", "dispatch_state"]),
        "runtimeLeaseState": identity_asset_field_string(asset, &["runtimeLeaseState", "runtime_lease_state"]),
    })
}

fn identity_job_explain_usize(value: &Value, keys: &[&str]) -> usize {
    identity_asset_field_u64(value, keys).unwrap_or(0) as usize
}

async fn write_identity_job_explain_report(explain: &Value, path: &Path) -> Result<Value> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(explain)?;
    tokio::fs::write(path, &bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(json!({
        "path": path.display().to_string(),
        "format": "json_explain",
        "bytes": bytes.len(),
    }))
}

fn identity_job_runtime_risk_event(
    run_id: &str,
    generated_at: u64,
    asset_manifest: &Path,
    working_manifest: &Path,
    worker_id: &str,
    job_id: &str,
    desired_concurrency: usize,
    limit: usize,
    command: &[String],
    exit_code: i32,
    runtime_risk: &Value,
) -> Value {
    let mut event = runtime_risk.clone();
    if let Some(object) = event.as_object_mut() {
        let risk_scope = object.get("scope").cloned().unwrap_or(Value::Null);
        object.insert(
            "scope".to_string(),
            Value::String("identity_job_runtime_risk_event".to_string()),
        );
        object.insert("riskScope".to_string(), risk_scope);
        object.insert(
            "identityJobRunId".to_string(),
            Value::String(run_id.to_string()),
        );
        object.insert(
            "generatedAtUnixSeconds".to_string(),
            Value::Number(generated_at.into()),
        );
        object.insert(
            "assetManifest".to_string(),
            Value::String(asset_manifest.display().to_string()),
        );
        object.insert(
            "workingAssetManifest".to_string(),
            Value::String(working_manifest.display().to_string()),
        );
        object.insert("workerId".to_string(), Value::String(worker_id.to_string()));
        object.insert("jobId".to_string(), Value::String(job_id.to_string()));
        object.insert(
            "desiredConcurrency".to_string(),
            Value::Number((desired_concurrency as u64).into()),
        );
        object.insert("limit".to_string(), Value::Number((limit as u64).into()));
        object.insert("command".to_string(), json!(command));
        object.insert(
            "exitCode".to_string(),
            Value::Number((exit_code as i64).into()),
        );
        event
    } else {
        json!({
            "scope": "identity_job_runtime_risk_event",
            "identityJobRunId": run_id,
            "generatedAtUnixSeconds": generated_at,
            "assetManifest": asset_manifest.display().to_string(),
            "workingAssetManifest": working_manifest.display().to_string(),
            "workerId": worker_id,
            "jobId": job_id,
            "desiredConcurrency": desired_concurrency,
            "limit": limit,
            "command": command,
            "exitCode": exit_code,
            "runtimeRisk": runtime_risk,
        })
    }
}

async fn write_identity_job_runtime_risk_event(
    event: &Value,
    path: &Path,
    append: bool,
) -> Result<Value> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let bytes = if append {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        let text = serde_json::to_string(event)?;
        file.write_all(text.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        text.len() + 1
    } else {
        let bytes = serde_json::to_vec_pretty(event)?;
        tokio::fs::write(path, &bytes)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
        bytes.len()
    };

    Ok(json!({
        "path": path.display().to_string(),
        "append": append,
        "count": 1,
        "format": if append {
            "ndjson_runtime_risk_events"
        } else {
            "json_runtime_risk_event"
        },
        "bytes": bytes,
    }))
}

async fn evaluate_identity_job_runtime_risk_gate(
    runtime_risk_ledgers: &[PathBuf],
    window_seconds: u64,
    now: u64,
    job_id: &str,
    current_limit: usize,
    current_desired_concurrency: usize,
) -> Result<Value> {
    let mut events = Vec::new();
    for path in runtime_risk_ledgers {
        let text = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read runtime risk ledger {}", path.display()))?;
        let mut parsed = parse_identity_job_runtime_risk_events(&text, path)
            .with_context(|| format!("failed to parse runtime risk ledger {}", path.display()))?;
        events.append(&mut parsed);
    }

    let min_generated_at = now.saturating_sub(window_seconds);
    let mut considered_event_count = 0usize;
    let mut active_suppression_count = 0usize;
    let mut expired_suppression_count = 0usize;
    let mut matching_events = Vec::<(usize, Value)>::new();
    for (event_index, event) in events.iter().enumerate() {
        let generated_at =
            identity_asset_field_u64(event, &["generatedAtUnixSeconds"]).unwrap_or(0);
        let suppress_until = identity_asset_field_u64(
            event,
            &["suppressUntilUnixSeconds", "suppress_until_unix_seconds"],
        );
        let active_suppression = suppress_until
            .map(|suppress_until| suppress_until > now)
            .unwrap_or(false);
        if suppress_until.is_some() && !active_suppression {
            expired_suppression_count += 1;
            continue;
        }
        if active_suppression {
            active_suppression_count += 1;
        }
        if generated_at < min_generated_at && !active_suppression {
            continue;
        }
        considered_event_count += 1;
        if identity_job_runtime_risk_event_matches_job(event, job_id) {
            matching_events.push((event_index, event.clone()));
        }
    }
    matching_events.sort_by(|(left_index, left), (right_index, right)| {
        identity_asset_field_u64(left, &["generatedAtUnixSeconds"])
            .unwrap_or(0)
            .cmp(&identity_asset_field_u64(right, &["generatedAtUnixSeconds"]).unwrap_or(0))
            .then_with(|| left_index.cmp(right_index))
    });

    let latest_event = matching_events.last().map(|(_, event)| event.clone());
    let action = latest_event
        .as_ref()
        .and_then(|event| identity_asset_field_string(event, &["recommendedAction"]))
        .unwrap_or_else(|| "continue_current".to_string());
    let failure_reason = latest_event.as_ref().and_then(|event| {
        identity_asset_field_string(
            event,
            &[
                "circuitBreakerFailureReason",
                "dominantFailureReason",
                "failureReason",
                "reason",
            ],
        )
    });
    let event_suggested_limit = latest_event
        .as_ref()
        .and_then(|event| identity_asset_field_u64(event, &["nextSuggestedLimit"]))
        .map(|value| value as usize);
    let event_suggested_desired = latest_event
        .as_ref()
        .and_then(|event| identity_asset_field_u64(event, &["nextSuggestedDesiredConcurrency"]))
        .map(|value| value as usize);
    let mut next_limit = event_suggested_limit.unwrap_or(current_limit);
    let mut next_desired_concurrency =
        event_suggested_desired.unwrap_or(current_desired_concurrency);
    let mut blocked = false;
    let mut adjusted = false;
    let mut block_reason = None::<String>;
    let message = match action.as_str() {
        "pause_pool" => {
            blocked = true;
            next_limit = 0;
            next_desired_concurrency = 0;
            block_reason = Some("pause_pool".to_string());
            "runtime risk ledger recommends pausing this asset pool".to_string()
        }
        "pause_failure_reason" => {
            blocked = true;
            next_limit = 0;
            next_desired_concurrency = 0;
            let reason = failure_reason
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            block_reason = Some(format!("pause_failure_reason:{reason}"));
            format!("runtime risk ledger recommends pausing failure reason {reason}")
        }
        "reduce_concurrency" => {
            if next_limit > 0 && next_limit < current_limit {
                adjusted = true;
            } else {
                next_limit = current_limit;
            }
            if next_desired_concurrency > 0
                && next_desired_concurrency < current_desired_concurrency
            {
                adjusted = true;
            } else {
                next_desired_concurrency = current_desired_concurrency.min(next_limit);
            }
            if adjusted {
                format!(
                    "runtime risk ledger reduced next run from limit {current_limit} to {next_limit}"
                )
            } else {
                "runtime risk ledger recommended reducing concurrency but no lower positive limit was available".to_string()
            }
        }
        _ => "runtime risk ledger allows this job to continue".to_string(),
    };

    Ok(json!({
        "scope": "identity_job_runtime_risk_gate",
        "generatedAtUnixSeconds": now,
        "passed": !blocked,
        "blocked": blocked,
        "adjusted": adjusted,
        "recommendedAction": action,
        "blockReason": block_reason,
        "failureReason": failure_reason,
        "ledgerPaths": runtime_risk_ledgers
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "windowSeconds": window_seconds,
        "minGeneratedAtUnixSeconds": min_generated_at,
        "eventCount": events.len(),
        "consideredEventCount": considered_event_count,
        "matchingEventCount": matching_events.len(),
        "activeSuppressionCount": active_suppression_count,
        "expiredSuppressionCount": expired_suppression_count,
        "originalLimit": current_limit,
        "originalDesiredConcurrency": current_desired_concurrency,
        "nextSuggestedLimit": next_limit,
        "nextSuggestedDesiredConcurrency": next_desired_concurrency,
        "latestEvent": latest_event,
        "message": message,
    }))
}

fn parse_identity_job_runtime_risk_events(text: &str, path: &Path) -> Result<Vec<Value>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let mut events = Vec::new();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        collect_identity_job_runtime_risk_events(&value, path, None, &mut events);
    } else {
        for (line_index, line) in trimmed.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let value = serde_json::from_str::<Value>(line).with_context(|| {
                format!(
                    "failed to parse runtime risk ledger {} line {}",
                    path.display(),
                    line_index + 1
                )
            })?;
            collect_identity_job_runtime_risk_events(
                &value,
                path,
                Some(line_index + 1),
                &mut events,
            );
        }
    }
    Ok(events)
}

fn collect_identity_job_runtime_risk_events(
    value: &Value,
    path: &Path,
    line_number: Option<usize>,
    events: &mut Vec<Value>,
) {
    if let Some(data) = value.get("data") {
        collect_identity_job_runtime_risk_events(data, path, line_number, events);
        return;
    }
    if let Some(array) = value.as_array() {
        for item in array {
            collect_identity_job_runtime_risk_events(item, path, line_number, events);
        }
        return;
    }
    let Some(mut event) = normalize_identity_job_runtime_risk_event(value) else {
        return;
    };
    if let Some(object) = event.as_object_mut() {
        object.insert(
            "runtimeRiskLedgerPath".to_string(),
            Value::String(path.display().to_string()),
        );
        if let Some(line_number) = line_number {
            object.insert(
                "runtimeRiskLedgerLine".to_string(),
                Value::Number((line_number as u64).into()),
            );
        }
    }
    events.push(event);
}

fn normalize_identity_job_runtime_risk_event(value: &Value) -> Option<Value> {
    if value.get("runtimeRisk").is_some() {
        let mut event = value.get("runtimeRisk")?.clone();
        if let Some(object) = event.as_object_mut() {
            let risk_scope = object.get("scope").cloned().unwrap_or(Value::Null);
            object.insert(
                "scope".to_string(),
                Value::String("identity_job_runtime_risk_event".to_string()),
            );
            object.entry("riskScope").or_insert(risk_scope);
            for key in [
                "identityJobRunId",
                "generatedAtUnixSeconds",
                "assetManifest",
                "workingAssetManifest",
                "workerId",
                "jobId",
                "desiredConcurrency",
                "limit",
                "command",
                "exitCode",
            ] {
                if !object.contains_key(key) {
                    if let Some(context_value) = value.get(key) {
                        object.insert(key.to_string(), context_value.clone());
                    }
                }
            }
        }
        return Some(event);
    }

    if identity_asset_field_string(value, &["recommendedAction"]).is_some() {
        let mut event = value.clone();
        if let Some(object) = event.as_object_mut() {
            let risk_scope = object.get("scope").cloned().unwrap_or(Value::Null);
            object.insert(
                "scope".to_string(),
                Value::String("identity_job_runtime_risk_event".to_string()),
            );
            object.entry("riskScope").or_insert(risk_scope);
        }
        return Some(event);
    }
    None
}

fn identity_job_runtime_risk_event_matches_job(event: &Value, job_id: &str) -> bool {
    identity_asset_field_string(event, &["jobId", "job_id"])
        .map(|event_job| event_job == job_id)
        .unwrap_or(true)
}

fn identity_job_runtime_risk_report(
    generated_at: u64,
    phase: &str,
    passed: bool,
    desired_concurrency: usize,
    limit: usize,
    selection: &Value,
    child: &Value,
    release: &Value,
    runtime_risk_gate: &Value,
    failure_reason_rules: &BTreeMap<String, IdentityJobFailureReasonRule>,
) -> Value {
    let selected_count = identity_asset_field_u64(child, &["selectedCount"])
        .or_else(|| identity_asset_field_u64(selection, &["selectedCount"]))
        .unwrap_or(0) as usize;
    let child_count = identity_asset_field_u64(child, &["childCount"])
        .map(|count| count as usize)
        .or_else(|| {
            child
                .as_object()
                .filter(|object| !object.is_empty())
                .map(|_| 1usize)
        })
        .unwrap_or(0);
    let succeeded_count = identity_asset_field_u64(child, &["succeededCount"])
        .map(|count| count as usize)
        .or_else(|| {
            child
                .get("success")
                .and_then(Value::as_bool)
                .map(|success| usize::from(success))
        })
        .unwrap_or(0);
    let failed_count = identity_asset_field_u64(child, &["failedCount"])
        .map(|count| count as usize)
        .or_else(|| {
            child
                .get("success")
                .and_then(Value::as_bool)
                .map(|success| usize::from(!success))
        })
        .unwrap_or(0);
    let skipped_count = identity_asset_field_u64(child, &["skippedCount"]).unwrap_or(0) as usize;
    let cancelled_count =
        identity_asset_field_u64(child, &["cancelledCount"]).unwrap_or(0) as usize;
    let released_count =
        identity_asset_field_u64(release, &["releasedCount"]).unwrap_or(0) as usize;
    let failure_rate_permille = if child_count > 0 {
        failed_count.saturating_mul(1000) / child_count
    } else {
        0
    };
    let failure_reason_counts = identity_job_failure_reason_counts(child);
    let (dominant_failure_reason, dominant_failure_reason_count) = failure_reason_counts
        .iter()
        .max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(left.0)))
        .map(|(reason, count)| (Some(reason.clone()), *count))
        .unwrap_or((None, 0));
    let circuit_breaker = child
        .get("circuitBreaker")
        .or_else(|| release.get("circuitBreaker"));
    let circuit_breaker_tripped = circuit_breaker
        .and_then(|breaker| breaker.get("tripped"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let circuit_breaker_kind = circuit_breaker
        .and_then(|breaker| breaker.get("kind"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let circuit_failure_reason = circuit_breaker
        .and_then(|breaker| breaker.get("failureReason"))
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let mut policy_failure_reason = circuit_failure_reason
        .clone()
        .or_else(|| dominant_failure_reason.clone());
    if policy_failure_reason.is_none() {
        policy_failure_reason = identity_job_child_failure_reason(child, "failed");
    }
    let policy_failure_reason_rule =
        identity_job_failure_reason_rule(failure_reason_rules, policy_failure_reason.as_deref());

    let (
        mut severity,
        mut recommended_action,
        mut next_limit,
        mut next_desired_concurrency,
        mut message,
    ) = if phase == "runtime_risk_gate" {
        let action = identity_asset_field_string(runtime_risk_gate, &["recommendedAction"])
            .unwrap_or_else(|| "follow_runtime_risk_ledger".to_string());
        let next_limit = identity_asset_field_u64(runtime_risk_gate, &["nextSuggestedLimit"])
            .unwrap_or(0) as usize;
        let next_desired_concurrency =
            identity_asset_field_u64(runtime_risk_gate, &["nextSuggestedDesiredConcurrency"])
                .unwrap_or(0) as usize;
        let message =
            identity_asset_field_string(runtime_risk_gate, &["message"]).unwrap_or_else(|| {
                "identity-job stopped by runtime risk ledger before child execution".to_string()
            });
        (
            "blocked",
            action,
            next_limit,
            next_desired_concurrency,
            message,
        )
    } else if phase != "complete" {
        let action = match phase {
            "validate" => "fix_asset_manifest",
            "gate" => "wait_reduce_concurrency_or_add_assets",
            "select" => "retry_after_sweep_or_reduce_limit",
            _ => "inspect_job_phase",
        };
        (
            "blocked",
            action.to_string(),
            0usize,
            0usize,
            format!("identity-job stopped before child execution in {phase} phase"),
        )
    } else if circuit_breaker_tripped {
        match circuit_breaker_kind.as_deref() {
            Some("failure_reason") => {
                let reason = circuit_failure_reason
                    .clone()
                    .or_else(|| dominant_failure_reason.clone())
                    .unwrap_or_else(|| "unknown".to_string());
                (
                    "critical",
                    "pause_failure_reason".to_string(),
                    0usize,
                    0usize,
                    format!(
                        "pause jobs that produce failure reason {reason}; current run tripped the per-reason circuit breaker"
                    ),
                )
            }
            _ => (
                "critical",
                "pause_pool".to_string(),
                0usize,
                0usize,
                "pause this asset pool; current run tripped the failed-asset circuit breaker"
                    .to_string(),
            ),
        }
    } else if passed && failed_count == 0 && cancelled_count == 0 {
        (
            "healthy",
            "continue_current".to_string(),
            limit,
            desired_concurrency,
            "runtime completed without failed or cancelled assets".to_string(),
        )
    } else if child_count > 0 && failure_rate_permille >= 500 {
        let suggested = (limit / 2).max(1);
        (
            "high",
            "reduce_concurrency".to_string(),
            suggested,
            desired_concurrency.min(suggested),
            "failed asset rate is at least 50%; reduce next-run concurrency".to_string(),
        )
    } else if failed_count > 0 || cancelled_count > 0 {
        let suggested = limit.saturating_sub(failed_count + cancelled_count).max(1);
        (
            "elevated",
            "reduce_concurrency".to_string(),
            suggested,
            desired_concurrency.min(suggested),
            "some assets failed or were cancelled; reduce next-run concurrency".to_string(),
        )
    } else {
        (
            "unknown",
            "inspect_job_report".to_string(),
            limit,
            desired_concurrency,
            "runtime risk could not be classified from this report".to_string(),
        )
    };
    let mut failure_reason_rule_applied_to_runtime_risk = false;
    let mut runtime_risk_cooldown_seconds = None::<u64>;
    let mut suppress_until_unix_seconds = None::<u64>;
    if phase == "complete" {
        if let (Some(reason), Some(rule)) =
            (policy_failure_reason.clone(), policy_failure_reason_rule)
        {
            if rule.recommended_action.is_some()
                || rule.runtime_risk_severity.is_some()
                || rule.next_suggested_limit.is_some()
                || rule.next_suggested_desired_concurrency.is_some()
                || rule.runtime_risk_message.is_some()
                || rule.runtime_risk_cooldown_seconds.is_some()
            {
                failure_reason_rule_applied_to_runtime_risk = true;
                if let Some(rule_severity) = rule.runtime_risk_severity.as_deref() {
                    severity = rule_severity;
                }
                if let Some(rule_action) = rule.recommended_action.clone() {
                    recommended_action = rule_action;
                }
                if let Some(rule_limit) = rule.next_suggested_limit {
                    next_limit = rule_limit;
                }
                if let Some(rule_desired_concurrency) = rule.next_suggested_desired_concurrency {
                    next_desired_concurrency = rule_desired_concurrency;
                }
                if let Some(rule_message) = rule.runtime_risk_message.clone() {
                    message = rule_message;
                } else if rule.recommended_action.is_some() {
                    message = format!("failure reason {reason} matched policy runtime risk rule");
                }
                if let Some(cooldown_seconds) = rule.runtime_risk_cooldown_seconds {
                    runtime_risk_cooldown_seconds = Some(cooldown_seconds);
                    suppress_until_unix_seconds =
                        Some(generated_at.saturating_add(cooldown_seconds));
                }
            }
        }
    }

    json!({
        "scope": "identity_job_runtime_risk",
        "phase": phase,
        "passed": passed,
        "severity": severity,
        "recommendedAction": recommended_action,
        "nextSuggestedLimit": next_limit,
        "nextSuggestedDesiredConcurrency": next_desired_concurrency,
        "selectedCount": selected_count,
        "childCount": child_count,
        "succeededCount": succeeded_count,
        "failedCount": failed_count,
        "cancelledCount": cancelled_count,
        "skippedCount": skipped_count,
        "releasedCount": released_count,
        "failureRatePermille": failure_rate_permille,
        "failureReasonCounts": failure_reason_counts,
        "dominantFailureReason": dominant_failure_reason,
        "dominantFailureReasonCount": dominant_failure_reason_count,
        "policyFailureReason": policy_failure_reason,
        "failureReasonRuleAppliedToRuntimeRisk": failure_reason_rule_applied_to_runtime_risk,
        "runtimeRiskCooldownSeconds": runtime_risk_cooldown_seconds,
        "suppressUntilUnixSeconds": suppress_until_unix_seconds,
        "circuitBreakerTripped": circuit_breaker_tripped,
        "circuitBreakerKind": circuit_breaker_kind,
        "circuitBreakerFailureReason": circuit_failure_reason,
        "message": message,
    })
}

fn identity_job_failure_reason_counts(child: &Value) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    if let Some(object) = child.get("failureReasonCounts").and_then(Value::as_object) {
        for (reason, count) in object {
            if let Some(count) = count.as_u64() {
                counts.insert(reason.clone(), count as usize);
            }
        }
    }
    if counts.is_empty() {
        if let Some(reason) = child
            .get("failureReason")
            .and_then(Value::as_str)
            .and_then(normalize_identity_job_failure_reason)
            .or_else(|| identity_job_failure_reason_from_value(child))
        {
            counts.insert(reason, 1);
        }
    }
    if counts.is_empty() {
        if let Some(children) = child.get("children").and_then(Value::as_array) {
            for entry in children {
                if let Some(reason) = entry
                    .get("failureReason")
                    .and_then(Value::as_str)
                    .and_then(normalize_identity_job_failure_reason)
                    .or_else(|| {
                        entry
                            .get("child")
                            .and_then(identity_job_failure_reason_from_value)
                    })
                {
                    *counts.entry(reason).or_insert(0) += 1;
                }
            }
        }
    }
    counts
}

pub async fn complete_identity_dispatch(
    claims_path: &Path,
    status: &str,
    worker: Option<&str>,
    claim_id: Option<&str>,
    dedupe_keys: &[String],
    retryable: bool,
    retry_after_seconds: Option<u64>,
    message: Option<&str>,
    result_json: Option<&str>,
    complete_out: Option<&Path>,
    append_complete: bool,
) -> Result<JsonResponse> {
    let status = normalize_identity_dispatch_completion_status(status)?;
    let retry_eligible = identity_dispatch_completion_retry_eligible(&status, retryable);
    if retry_after_seconds.is_some() && !retry_eligible {
        bail!("--retry-after-seconds requires --status retry or --retryable failed completion");
    }

    let text = tokio::fs::read_to_string(claims_path)
        .await
        .with_context(|| format!("failed to read claim records {}", claims_path.display()))?;
    let claim_items = parse_identity_dispatch_claim_items(&text)
        .with_context(|| format!("failed to parse claim records {}", claims_path.display()))?;
    let worker_filter = worker
        .map(str::trim)
        .filter(|worker| !worker.is_empty())
        .map(ToString::to_string);
    let claim_id_filter = claim_id
        .map(str::trim)
        .filter(|claim_id| !claim_id.is_empty())
        .map(ToString::to_string);
    let dedupe_filter = dedupe_keys
        .iter()
        .map(|key| key.trim())
        .filter(|key| !key.is_empty())
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>();
    let result = match result_json {
        Some(raw) => Some(
            serde_json::from_str::<Value>(raw)
                .with_context(|| format!("invalid --result-json value: {raw}"))?,
        ),
        None => None,
    };

    let generated_at = unix_seconds();
    let completion_id = format!(
        "completion_{}_{}_{}",
        generated_at,
        std::process::id(),
        status
    );
    let retry_after_unix_seconds =
        retry_after_seconds.map(|seconds| generated_at.saturating_add(seconds));
    let message = message
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(ToString::to_string);
    let mut skipped_worker_count = 0usize;
    let mut skipped_claim_id_count = 0usize;
    let mut skipped_dedupe_key_count = 0usize;
    let mut duplicate_dedupe_key_count = 0usize;
    let mut selected_claims = BTreeMap::<String, IdentityDispatchClaimItem>::new();

    for claim in claim_items {
        if let Some(worker) = &worker_filter {
            if claim.worker_id != *worker {
                skipped_worker_count += 1;
                continue;
            }
        }
        if let Some(claim_id) = &claim_id_filter {
            if claim.claim_id != *claim_id {
                skipped_claim_id_count += 1;
                continue;
            }
        }
        if !dedupe_filter.is_empty() && !dedupe_filter.contains(&claim.dispatch.dedupe_key) {
            skipped_dedupe_key_count += 1;
            continue;
        }

        let dedupe_key = claim.dispatch.dedupe_key.clone();
        if let Some(existing) = selected_claims.get(&dedupe_key) {
            duplicate_dedupe_key_count += 1;
            if !identity_dispatch_claim_is_newer(&claim, existing) {
                continue;
            }
        }
        selected_claims.insert(dedupe_key, claim);
    }

    let mut selected_claims = selected_claims.into_values().collect::<Vec<_>>();
    selected_claims.sort_by(|a, b| {
        a.dispatch
            .sort_rank
            .cmp(&b.dispatch.sort_rank)
            .then_with(|| a.dispatch.dispatch_index.cmp(&b.dispatch.dispatch_index))
            .then_with(|| a.dispatch.dedupe_key.cmp(&b.dispatch.dedupe_key))
    });

    let mut completed = Vec::new();
    for claim in selected_claims {
        let worker_id = worker_filter
            .clone()
            .unwrap_or_else(|| claim.worker_id.clone());
        completed.push(IdentityDispatchCompletionItem {
            completion_index: completed.len(),
            status: status.clone(),
            worker_id,
            claim_id: claim.claim_id.clone(),
            completion_id: completion_id.clone(),
            completed_at_unix_seconds: generated_at,
            retry_eligible,
            retry_after_unix_seconds,
            message: message.clone(),
            result: result.clone(),
            dispatch: claim.dispatch.clone(),
            claim,
        });
    }

    let filtered_dedupe_keys = dedupe_filter.iter().cloned().collect::<Vec<_>>();
    let mut report = IdentityDispatchCompletionReport {
        scope: "identity_dispatch_completion".to_string(),
        path: claims_path.display().to_string(),
        completion_id,
        generated_at_unix_seconds: generated_at,
        status,
        retry_eligible,
        retry_after_unix_seconds,
        worker_id: worker_filter,
        claim_id: claim_id_filter,
        filtered_dedupe_keys,
        message,
        result,
        input_count: completed.len()
            + skipped_worker_count
            + skipped_claim_id_count
            + skipped_dedupe_key_count
            + duplicate_dedupe_key_count,
        completed_count: completed.len(),
        skipped_worker_count,
        skipped_claim_id_count,
        skipped_dedupe_key_count,
        duplicate_dedupe_key_count,
        items: completed,
        complete_out: None,
    };
    report.complete_out =
        write_identity_dispatch_completion(&report, complete_out, append_complete).await?;

    Ok(JsonResponse::ok(report))
}

fn parse_identity_dispatch_items(text: &str) -> Result<Vec<IdentityPlanDispatchItem>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("dispatch queue is empty");
    }
    let values = if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        vec![value]
    } else {
        let mut values = Vec::new();
        for (line_index, line) in trimmed.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            values.push(
                serde_json::from_str::<Value>(line)
                    .with_context(|| format!("invalid JSON at line {}", line_index + 1))?,
            );
        }
        values
    };

    let mut items = Vec::new();
    for value in &values {
        items.extend(identity_dispatch_items_from_value(value)?);
    }
    if items.is_empty() {
        bail!("no dispatch items found");
    }
    Ok(items)
}

fn identity_dispatch_items_from_value(value: &Value) -> Result<Vec<IdentityPlanDispatchItem>> {
    if let Some(data) = value.get("data") {
        return identity_dispatch_items_from_value(data);
    }
    if let Some(dispatch) = value.get("dispatch") {
        return Ok(vec![
            serde_json::from_value(dispatch.clone()).context("invalid dispatch item")?,
        ]);
    }
    if let Some(queue) = value.get("dispatchQueue").or_else(|| value.get("queue")) {
        return identity_dispatch_items_from_value(queue);
    }
    if let Some(items) = value.get("items").and_then(Value::as_array) {
        return items
            .iter()
            .map(|item| serde_json::from_value(item.clone()).context("invalid dispatch item"))
            .collect();
    }
    if value.get("dedupeKey").is_some() && value.get("leaseKey").is_some() {
        return Ok(vec![
            serde_json::from_value(value.clone()).context("invalid dispatch item")?,
        ]);
    }
    if let Some(items) = value.as_array() {
        let mut out = Vec::new();
        for item in items {
            out.extend(identity_dispatch_items_from_value(item)?);
        }
        return Ok(out);
    }
    Ok(Vec::new())
}

fn identity_claim_phases(items: &[IdentityDispatchClaimItem]) -> Vec<String> {
    let mut phases = Vec::new();
    for item in items {
        if !phases.contains(&item.dispatch.phase) {
            phases.push(item.dispatch.phase.clone());
        }
    }
    phases
}

async fn read_identity_dispatch_claim_ledger(
    path: Option<&Path>,
) -> Result<Vec<IdentityDispatchClaimItem>> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let text = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read claim ledger {}", path.display()))?;
    parse_identity_dispatch_claim_items(&text)
        .with_context(|| format!("failed to parse claim ledger {}", path.display()))
}

fn parse_identity_dispatch_claim_items(text: &str) -> Result<Vec<IdentityDispatchClaimItem>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let values = if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        vec![value]
    } else {
        let mut values = Vec::new();
        for (line_index, line) in trimmed.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            values.push(
                serde_json::from_str::<Value>(line)
                    .with_context(|| format!("invalid JSON at line {}", line_index + 1))?,
            );
        }
        values
    };

    let mut items = Vec::new();
    for value in &values {
        items.extend(identity_dispatch_claim_items_from_value(value)?);
    }
    Ok(items)
}

fn identity_dispatch_claim_items_from_value(
    value: &Value,
) -> Result<Vec<IdentityDispatchClaimItem>> {
    if let Some(data) = value.get("data") {
        return identity_dispatch_claim_items_from_value(data);
    }
    if let Some(item) = value.get("item") {
        let mut item = item.clone();
        if let (Some(generated_at), Value::Object(map)) =
            (value.get("generatedAtUnixSeconds").cloned(), &mut item)
        {
            map.entry("claimedAtUnixSeconds".to_string())
                .or_insert(generated_at);
        }
        return Ok(vec![
            serde_json::from_value(item).context("invalid claim item")?,
        ]);
    }
    if value.get("dispatch").is_some() && value.get("leaseExpiresUnixSeconds").is_some() {
        return Ok(vec![
            serde_json::from_value(value.clone()).context("invalid claim item")?,
        ]);
    }
    if let Some(items) = value.get("items").and_then(Value::as_array) {
        return items
            .iter()
            .map(|item| serde_json::from_value(item.clone()).context("invalid claim item"))
            .collect();
    }
    if let Some(values) = value.as_array() {
        let mut out = Vec::new();
        for value in values {
            out.extend(identity_dispatch_claim_items_from_value(value)?);
        }
        return Ok(out);
    }
    Ok(Vec::new())
}

fn active_identity_dispatch_leases(
    items: &[IdentityDispatchClaimItem],
    now: u64,
) -> (BTreeSet<String>, usize, usize) {
    let mut active = BTreeSet::new();
    let mut expired_count = 0usize;
    for item in items {
        if item.status != "leased" {
            continue;
        }
        if item.lease_expires_unix_seconds > now {
            active.insert(item.dispatch.dedupe_key.clone());
        } else {
            expired_count += 1;
        }
    }
    let active_count = active.len();
    (active, active_count, expired_count)
}

fn identity_dispatch_claim_is_newer(
    candidate: &IdentityDispatchClaimItem,
    existing: &IdentityDispatchClaimItem,
) -> bool {
    let candidate_key = (
        candidate.lease_expires_unix_seconds,
        candidate.renewed_at_unix_seconds.unwrap_or(0),
    );
    let existing_key = (
        existing.lease_expires_unix_seconds,
        existing.renewed_at_unix_seconds.unwrap_or(0),
    );
    candidate_key >= existing_key
}

fn identity_dispatch_active_expired_counts(
    items: &[IdentityDispatchClaimItem],
    now: u64,
) -> (usize, usize) {
    let mut active = BTreeSet::new();
    let mut expired = BTreeSet::new();
    for item in items {
        if item.status != "leased" {
            continue;
        }
        if item.lease_expires_unix_seconds > now {
            active.insert(item.dispatch.dedupe_key.clone());
        } else {
            expired.insert(item.dispatch.dedupe_key.clone());
        }
    }
    (active.len(), expired.len())
}

fn build_identity_dispatch_reconcile_events(
    claim_items: &[IdentityDispatchClaimItem],
    completion_items: &[IdentityDispatchCompletionItem],
    now: u64,
) -> Vec<IdentityDispatchReconcileEvent> {
    let mut events = BTreeMap::<String, IdentityDispatchReconcileEvent>::new();
    let mut latest_claims = BTreeMap::<String, &IdentityDispatchClaimItem>::new();
    for claim in claim_items {
        if claim.status != "leased" || claim.lease_expires_unix_seconds <= now {
            continue;
        }
        let key = claim.dispatch.dedupe_key.clone();
        let replace = latest_claims
            .get(&key)
            .map(|existing| identity_dispatch_claim_is_newer(claim, existing))
            .unwrap_or(true);
        if replace {
            latest_claims.insert(key, claim);
        }
    }
    for claim in latest_claims.into_values() {
        let event = IdentityDispatchReconcileEvent {
            dispatch_state: "leased".to_string(),
            status: claim.status.clone(),
            worker_id: Some(claim.worker_id.clone()),
            claim_id: Some(claim.claim_id.clone()),
            completion_id: None,
            lease_expires_unix_seconds: Some(claim.lease_expires_unix_seconds),
            completed_at_unix_seconds: None,
            retry_eligible: None,
            retry_after_unix_seconds: None,
            updated_at_unix_seconds: claim
                .renewed_at_unix_seconds
                .or(claim.claimed_at_unix_seconds)
                .unwrap_or(now),
            result: None,
            dispatch: claim.dispatch.clone(),
        };
        events.insert(event.dispatch.dedupe_key.clone(), event);
    }

    let mut latest_completions = BTreeMap::<String, &IdentityDispatchCompletionItem>::new();
    for completion in completion_items {
        let key = completion.dispatch.dedupe_key.clone();
        let replace = latest_completions
            .get(&key)
            .map(|existing| {
                completion.completed_at_unix_seconds >= existing.completed_at_unix_seconds
            })
            .unwrap_or(true);
        if replace {
            latest_completions.insert(key, completion);
        }
    }
    for completion in latest_completions.into_values() {
        let dispatch_state = if completion.retry_eligible || completion.status == "retry" {
            "retry".to_string()
        } else {
            completion.status.clone()
        };
        let event = IdentityDispatchReconcileEvent {
            dispatch_state,
            status: completion.status.clone(),
            worker_id: Some(completion.worker_id.clone()),
            claim_id: Some(completion.claim_id.clone()),
            completion_id: Some(completion.completion_id.clone()),
            lease_expires_unix_seconds: Some(completion.claim.lease_expires_unix_seconds),
            completed_at_unix_seconds: Some(completion.completed_at_unix_seconds),
            retry_eligible: Some(completion.retry_eligible),
            retry_after_unix_seconds: completion.retry_after_unix_seconds,
            updated_at_unix_seconds: completion.completed_at_unix_seconds,
            result: completion.result.clone(),
            dispatch: completion.dispatch.clone(),
        };
        let replace = events
            .get(&event.dispatch.dedupe_key)
            .map(|existing| event.updated_at_unix_seconds >= existing.updated_at_unix_seconds)
            .unwrap_or(true);
        if replace {
            events.insert(event.dispatch.dedupe_key.clone(), event);
        }
    }

    events.into_values().collect()
}

fn apply_identity_dispatch_reconcile_events(
    manifest: &mut Value,
    events: &[IdentityDispatchReconcileEvent],
) -> Result<(Vec<IdentityDispatchManifestUpdate>, usize)> {
    let entries = identity_plan_manifest_entries_mut(manifest)?;
    let mut updates = Vec::new();
    let mut unmatched_event_count = 0usize;

    for event in events {
        let event_keys = identity_dispatch_match_keys(&event.dispatch);
        if event_keys.is_empty() {
            unmatched_event_count += 1;
            continue;
        }
        let mut matched_index = None;
        for (index, entry) in entries.iter().enumerate() {
            let entry_keys = identity_plan_manifest_entry_match_keys(entry);
            if !entry_keys.is_disjoint(&event_keys) {
                matched_index = Some(index);
                break;
            }
        }

        if let Some(asset_index) = matched_index {
            if let Some(entry) = entries.get_mut(asset_index) {
                apply_identity_dispatch_event_to_manifest_entry(entry, event);
                updates.push(IdentityDispatchManifestUpdate {
                    asset_index,
                    dispatch_state: event.dispatch_state.clone(),
                    status: event.status.clone(),
                    dedupe_key: event.dispatch.dedupe_key.clone(),
                    phase: event.dispatch.phase.clone(),
                    kind: event.dispatch.kind.clone(),
                    worker_id: event.worker_id.clone(),
                    claim_id: event.claim_id.clone(),
                    completion_id: event.completion_id.clone(),
                    updated_at_unix_seconds: event.updated_at_unix_seconds,
                });
            }
        } else {
            unmatched_event_count += 1;
        }
    }

    Ok((updates, unmatched_event_count))
}

fn apply_identity_dispatch_event_to_manifest_entry(
    entry: &mut Value,
    event: &IdentityDispatchReconcileEvent,
) {
    let Value::Object(map) = entry else {
        return;
    };

    map.insert(
        "dispatchState".to_string(),
        Value::String(event.dispatch_state.clone()),
    );
    map.insert(
        "lastDispatchStatus".to_string(),
        Value::String(event.status.clone()),
    );
    map.insert(
        "lastDispatchDedupeKey".to_string(),
        Value::String(event.dispatch.dedupe_key.clone()),
    );
    map.insert(
        "lastDispatchLeaseKey".to_string(),
        Value::String(event.dispatch.lease_key.clone()),
    );
    map.insert(
        "lastDispatchPhase".to_string(),
        Value::String(event.dispatch.phase.clone()),
    );
    map.insert(
        "lastDispatchKind".to_string(),
        Value::String(identity_dispatch_kind_name(&event.dispatch.kind).to_string()),
    );
    if let Some(action_code) = &event.dispatch.action_code {
        map.insert(
            "lastDispatchActionCode".to_string(),
            Value::String(action_code.clone()),
        );
    }
    if let Some(worker_id) = &event.worker_id {
        map.insert(
            "lastDispatchWorkerId".to_string(),
            Value::String(worker_id.clone()),
        );
    }
    if let Some(claim_id) = &event.claim_id {
        map.insert(
            "lastDispatchClaimId".to_string(),
            Value::String(claim_id.clone()),
        );
    }
    map.insert(
        "lastDispatchUpdatedAtUnixSeconds".to_string(),
        json!(event.updated_at_unix_seconds),
    );

    if let Some(lease_expires) = event.lease_expires_unix_seconds {
        map.insert(
            "lastDispatchLeaseExpiresUnixSeconds".to_string(),
            json!(lease_expires),
        );
    } else {
        map.remove("lastDispatchLeaseExpiresUnixSeconds");
    }
    if let Some(completion_id) = &event.completion_id {
        map.insert(
            "lastDispatchCompletionId".to_string(),
            Value::String(completion_id.clone()),
        );
    } else {
        map.remove("lastDispatchCompletionId");
    }
    if let Some(completed_at) = event.completed_at_unix_seconds {
        map.insert(
            "lastDispatchCompletedAtUnixSeconds".to_string(),
            json!(completed_at),
        );
    } else {
        map.remove("lastDispatchCompletedAtUnixSeconds");
    }
    if let Some(retry_eligible) = event.retry_eligible {
        map.insert(
            "lastDispatchRetryEligible".to_string(),
            json!(retry_eligible),
        );
    } else {
        map.remove("lastDispatchRetryEligible");
    }
    if let Some(retry_after) = event.retry_after_unix_seconds {
        map.insert(
            "lastDispatchRetryAfterUnixSeconds".to_string(),
            json!(retry_after),
        );
    } else {
        map.remove("lastDispatchRetryAfterUnixSeconds");
    }

    if event.status == "succeeded" {
        if let Some(next_state) = identity_dispatch_event_next_state(event) {
            if let Some(previous_state) = map.get("state").cloned() {
                map.insert("lastDispatchPreviousState".to_string(), previous_state);
            }
            map.insert("state".to_string(), Value::String(next_state));
        }
        if let Some(profile_dir) = identity_dispatch_event_profile_dir(event) {
            map.insert("profileDir".to_string(), Value::String(profile_dir));
        }
    }
}

fn identity_dispatch_match_keys(dispatch: &IdentityPlanDispatchItem) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    push_identity_plan_string_key(&mut keys, "accountId", dispatch.account_id.as_deref());
    push_identity_plan_string_key(&mut keys, "profileId", dispatch.profile_id.as_deref());
    push_identity_plan_string_key(&mut keys, "identityId", dispatch.identity_id.as_deref());
    push_identity_plan_string_key(&mut keys, "label", dispatch.label.as_deref());
    push_identity_plan_string_key(&mut keys, "profileDir", dispatch.profile_dir.as_deref());
    keys
}

fn push_identity_plan_string_key(
    keys: &mut BTreeSet<String>,
    normalized_key: &str,
    value: Option<&str>,
) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    keys.insert(format!("{normalized_key}:{value}"));
}

fn identity_dispatch_event_next_state(event: &IdentityDispatchReconcileEvent) -> Option<String> {
    event
        .result
        .as_ref()
        .and_then(|result| {
            result
                .get("nextState")
                .or_else(|| result.get("next_state"))
                .or_else(|| result.get("state"))
                .and_then(label_value_to_string)
        })
        .or_else(|| event.dispatch.next_state.clone())
}

fn identity_dispatch_event_profile_dir(event: &IdentityDispatchReconcileEvent) -> Option<String> {
    event.result.as_ref().and_then(|result| {
        result
            .get("profileDir")
            .or_else(|| result.get("profilePath"))
            .or_else(|| result.get("destinationPath"))
            .or_else(|| result.get("profile_dir"))
            .or_else(|| result.get("profile_path"))
            .or_else(|| result.get("destination_path"))
            .and_then(label_value_to_string)
    })
}

fn identity_dispatch_kind_name(kind: &IdentityPlanDispatchKind) -> &'static str {
    match kind {
        IdentityPlanDispatchKind::Action => "action",
        IdentityPlanDispatchKind::AssetPatch => "asset_patch",
        IdentityPlanDispatchKind::Command => "command",
    }
}

fn normalize_identity_asset_allowed_states(states: &[String]) -> BTreeSet<String> {
    let mut allowed = states
        .iter()
        .map(|state| state.trim().to_ascii_lowercase())
        .filter(|state| !state.is_empty())
        .collect::<BTreeSet<_>>();
    if allowed.is_empty() {
        allowed.insert("active".to_string());
    }
    allowed
}

fn identity_asset_selection_block_reasons(
    entry: &Value,
    allowed_states: &BTreeSet<String>,
    now: u64,
    include_dispatch_leased: bool,
    include_retry: bool,
    include_failed: bool,
    include_cancelled: bool,
    include_runtime_leased: bool,
    include_missing_profile_dir: bool,
) -> Vec<String> {
    let mut reasons = Vec::new();
    let state = identity_asset_field_string(entry, &["state"])
        .unwrap_or_else(|| "unknown".to_string())
        .to_ascii_lowercase();
    if !allowed_states.contains("*") && !allowed_states.contains(&state) {
        reasons.push(format!("state_not_allowed:{state}"));
    }
    if identity_asset_profile_dir(entry).is_none() && !include_missing_profile_dir {
        reasons.push("missing_profile_dir".to_string());
    }
    if identity_asset_runtime_lease_active(entry, now) && !include_runtime_leased {
        reasons.push("runtime_lease_active".to_string());
    }
    if identity_asset_cooldown_active(entry, now) {
        reasons.push("cooldown_active".to_string());
    }

    let dispatch_state = identity_asset_field_string(entry, &["dispatchState", "dispatch_state"])
        .unwrap_or_else(|| "idle".to_string())
        .to_ascii_lowercase();
    match dispatch_state.as_str() {
        "leased" => {
            if !include_dispatch_leased && identity_asset_dispatch_lease_active(entry, now) {
                reasons.push("dispatch_lease_active".to_string());
            }
        }
        "retry" => {
            if !include_retry && identity_asset_dispatch_retry_waiting(entry, now) {
                reasons.push("dispatch_retry_waiting".to_string());
            }
        }
        "failed" => {
            if !include_failed {
                reasons.push("dispatch_failed".to_string());
            }
        }
        "cancelled" | "canceled" => {
            if !include_cancelled {
                reasons.push("dispatch_cancelled".to_string());
            }
        }
        _ => {}
    }

    reasons
}

fn identity_asset_selection_item(
    asset_index: usize,
    entry: &Value,
    reasons: Vec<String>,
    lease_id: Option<String>,
) -> IdentityAssetSelectionItem {
    IdentityAssetSelectionItem {
        asset_index,
        account_id: identity_asset_field_string(entry, &["accountId", "account_id"]),
        profile_id: identity_asset_field_string(entry, &["profileId", "profile_id"]),
        identity_id: identity_asset_field_string(entry, &["identityId", "identity_id"]),
        label: identity_asset_field_string(entry, &["label", "name"]),
        profile_dir: identity_asset_profile_dir(entry),
        proxy_id: identity_asset_field_string(entry, &["proxyId", "proxy_id"]),
        fingerprint_seed: identity_asset_field_string(
            entry,
            &["fingerprintSeed", "fingerprint_seed"],
        ),
        state: identity_asset_field_string(entry, &["state"])
            .unwrap_or_else(|| "unknown".to_string()),
        dispatch_state: identity_asset_field_string(entry, &["dispatchState", "dispatch_state"])
            .unwrap_or_else(|| "idle".to_string()),
        runtime_lease_state: identity_asset_field_string(
            entry,
            &["runtimeLeaseState", "runtime_lease_state"],
        ),
        runtime_lease_expires_unix_seconds: identity_asset_field_u64(
            entry,
            &[
                "runtimeLeaseExpiresUnixSeconds",
                "runtime_lease_expires_unix_seconds",
            ],
        ),
        lease_id,
        reasons,
    }
}

fn identity_asset_forecast_item(
    asset_index: usize,
    entry: &Value,
    reasons: Vec<String>,
    recoverable: bool,
    available_at_unix_seconds: Option<u64>,
    now: u64,
) -> IdentityAssetForecastItem {
    IdentityAssetForecastItem {
        asset_index,
        account_id: identity_asset_field_string(entry, &["accountId", "account_id"]),
        profile_id: identity_asset_field_string(entry, &["profileId", "profile_id"]),
        identity_id: identity_asset_field_string(entry, &["identityId", "identity_id"]),
        label: identity_asset_field_string(entry, &["label", "name"]),
        profile_dir: identity_asset_profile_dir(entry),
        state: identity_asset_field_string(entry, &["state"])
            .unwrap_or_else(|| "unknown".to_string()),
        dispatch_state: identity_asset_field_string(entry, &["dispatchState", "dispatch_state"])
            .unwrap_or_else(|| "idle".to_string()),
        runtime_lease_state: identity_asset_field_string(
            entry,
            &["runtimeLeaseState", "runtime_lease_state"],
        ),
        reasons,
        recoverable,
        available_at_unix_seconds,
        seconds_until_available: available_at_unix_seconds.map(|available_at| {
            if available_at > now {
                available_at - now
            } else {
                0
            }
        }),
    }
}

fn identity_asset_forecast_available_at(
    entry: &Value,
    reasons: &[String],
    now: u64,
) -> Option<u64> {
    let mut available_at = now;
    for reason in reasons {
        let reason_available_at = match reason.as_str() {
            "runtime_lease_active" => identity_asset_field_u64(
                entry,
                &[
                    "runtimeLeaseExpiresUnixSeconds",
                    "runtime_lease_expires_unix_seconds",
                ],
            ),
            "dispatch_lease_active" => identity_asset_field_u64(
                entry,
                &[
                    "lastDispatchLeaseExpiresUnixSeconds",
                    "last_dispatch_lease_expires_unix_seconds",
                ],
            ),
            "dispatch_retry_waiting" => identity_asset_field_u64(
                entry,
                &[
                    "lastDispatchRetryAfterUnixSeconds",
                    "last_dispatch_retry_after_unix_seconds",
                ],
            ),
            "cooldown_active" => identity_asset_field_u64(
                entry,
                &[
                    "cooldownUntilUnixSeconds",
                    "cooldown_until_unix_seconds",
                    "nextAvailableUnixSeconds",
                    "next_available_unix_seconds",
                ],
            ),
            _ => return None,
        }?;
        if reason_available_at <= now {
            return None;
        }
        available_at = available_at.max(reason_available_at);
    }
    Some(available_at)
}

fn identity_asset_forecast_enough_at(
    current_runnable_count: usize,
    desired_concurrency: Option<usize>,
    recovery_events: &[IdentityAssetForecastItem],
) -> Option<u64> {
    let desired = desired_concurrency?;
    if current_runnable_count >= desired {
        return None;
    }
    let mut runnable_count = current_runnable_count;
    for event in recovery_events {
        let Some(available_at) = event.available_at_unix_seconds else {
            continue;
        };
        runnable_count += 1;
        if runnable_count >= desired {
            return Some(available_at);
        }
    }
    None
}

fn identity_asset_field_string(entry: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = entry.get(*key).and_then(label_value_to_string) {
            return Some(value);
        }
    }
    None
}

fn identity_asset_profile_dir(entry: &Value) -> Option<String> {
    identity_asset_field_string(
        entry,
        &[
            "profileDir",
            "profilePath",
            "userDataDir",
            "path",
            "profile_dir",
            "profile_path",
            "user_data_dir",
        ],
    )
}

fn identity_asset_field_u64(entry: &Value, keys: &[&str]) -> Option<u64> {
    for key in keys {
        if let Some(value) = entry.get(*key) {
            if let Some(number) = value.as_u64() {
                return Some(number);
            }
            if let Some(number) = value.as_str().and_then(|text| text.trim().parse().ok()) {
                return Some(number);
            }
        }
    }
    None
}

fn identity_asset_manifest_version(manifest: &Value) -> Option<String> {
    identity_asset_field_string(
        manifest,
        &[
            "manifestVersion",
            "manifest_version",
            "profileAssetManifestVersion",
            "profile_asset_manifest_version",
        ],
    )
}

fn identity_asset_validate_key_value(entry: &Value, field: &str) -> Option<String> {
    match field {
        "accountId" => identity_asset_field_string(entry, &["accountId", "account_id"]),
        "profileId" => identity_asset_field_string(entry, &["profileId", "profile_id"]),
        "identityId" => identity_asset_field_string(entry, &["identityId", "identity_id"]),
        "label" => identity_asset_field_string(entry, &["label", "name"]),
        "profileDir" => identity_asset_profile_dir(entry),
        _ => None,
    }
}

fn identity_asset_duplicate_issue_code(field: &str) -> &'static str {
    match field {
        "accountId" => "duplicate_account_id",
        "profileId" => "duplicate_profile_id",
        "identityId" => "duplicate_identity_id",
        "label" => "duplicate_label",
        "profileDir" => "duplicate_profile_dir",
        _ => "duplicate_match_key",
    }
}

fn validate_identity_asset_timestamp(
    issues: &mut Vec<IdentityAssetValidationIssue>,
    asset_index: usize,
    entry: &Value,
    keys: &[&'static str],
) -> Option<u64> {
    let Some((field, value)) = identity_asset_timestamp_field(entry, keys) else {
        return None;
    };
    match parse_identity_asset_timestamp_value(&value) {
        Ok(timestamp) => Some(timestamp),
        Err(()) => {
            push_identity_asset_validation_issue(
                issues,
                "error",
                "invalid_timestamp",
                Some(asset_index),
                Some(field),
                Some(value),
                "timestamp fields must be unsigned integer seconds or numeric strings",
            );
            None
        }
    }
}

fn identity_asset_timestamp_field(
    entry: &Value,
    keys: &[&'static str],
) -> Option<(&'static str, Value)> {
    for key in keys {
        let Some(value) = entry.get(*key) else {
            continue;
        };
        if value.is_null() {
            return None;
        }
        return Some((*key, value.clone()));
    }
    None
}

fn parse_identity_asset_timestamp_value(value: &Value) -> std::result::Result<u64, ()> {
    if let Some(number) = value.as_u64() {
        return Ok(number);
    }
    if let Some(text) = value.as_str() {
        return text.trim().parse::<u64>().map_err(|_| ());
    }
    Err(())
}

fn identity_asset_known_state(state: &str) -> bool {
    matches!(
        normalize_identity_asset_state_name(state).as_str(),
        "active"
            | "repair"
            | "quarantine"
            | "review"
            | "investigate"
            | "missing_current"
            | "new_current"
            | "archived"
            | "disabled"
            | "pending"
    )
}

fn identity_asset_known_dispatch_state(state: &str) -> bool {
    matches!(
        normalize_identity_asset_state_name(state).as_str(),
        "idle"
            | "leased"
            | "succeeded"
            | "failed"
            | "retry"
            | "cancelled"
            | "canceled"
            | "expired"
            | "unknown"
    )
}

fn identity_asset_known_runtime_lease_state(state: &str) -> bool {
    matches!(
        normalize_identity_asset_state_name(state).as_str(),
        "none" | "idle" | "leased" | "released" | "expired"
    )
}

fn normalize_identity_asset_state_name(state: &str) -> String {
    state.trim().to_ascii_lowercase().replace('-', "_")
}

fn push_identity_asset_validation_issue(
    issues: &mut Vec<IdentityAssetValidationIssue>,
    severity: &str,
    code: &str,
    asset_index: Option<usize>,
    field: Option<&str>,
    value: Option<Value>,
    message: &str,
) {
    issues.push(IdentityAssetValidationIssue {
        severity: severity.to_string(),
        code: code.to_string(),
        asset_index,
        field: field.map(ToString::to_string),
        value,
        message: message.to_string(),
    });
}

fn identity_asset_runtime_lease_active(entry: &Value, now: u64) -> bool {
    let state = identity_asset_field_string(entry, &["runtimeLeaseState", "runtime_lease_state"])
        .map(|state| state.to_ascii_lowercase());
    if state.as_deref() != Some("leased") {
        return false;
    }
    identity_asset_field_u64(
        entry,
        &[
            "runtimeLeaseExpiresUnixSeconds",
            "runtime_lease_expires_unix_seconds",
        ],
    )
    .map(|expires| expires > now)
    .unwrap_or(true)
}

fn identity_asset_runtime_lease_expired(entry: &Value, now: u64) -> bool {
    let state = identity_asset_field_string(entry, &["runtimeLeaseState", "runtime_lease_state"])
        .map(|state| state.to_ascii_lowercase());
    if state.as_deref() != Some("leased") {
        return false;
    }
    identity_asset_field_u64(
        entry,
        &[
            "runtimeLeaseExpiresUnixSeconds",
            "runtime_lease_expires_unix_seconds",
        ],
    )
    .map(|expires| expires <= now)
    .unwrap_or(false)
}

fn identity_asset_cooldown_active(entry: &Value, now: u64) -> bool {
    identity_asset_field_u64(
        entry,
        &[
            "cooldownUntilUnixSeconds",
            "cooldown_until_unix_seconds",
            "nextAvailableUnixSeconds",
            "next_available_unix_seconds",
        ],
    )
    .map(|expires| expires > now)
    .unwrap_or(false)
}

fn identity_asset_cooldown_expired(entry: &Value, now: u64) -> bool {
    identity_asset_field_u64(
        entry,
        &[
            "cooldownUntilUnixSeconds",
            "cooldown_until_unix_seconds",
            "nextAvailableUnixSeconds",
            "next_available_unix_seconds",
        ],
    )
    .map(|expires| expires <= now)
    .unwrap_or(false)
}

fn identity_asset_dispatch_lease_active(entry: &Value, now: u64) -> bool {
    identity_asset_field_u64(
        entry,
        &[
            "lastDispatchLeaseExpiresUnixSeconds",
            "last_dispatch_lease_expires_unix_seconds",
        ],
    )
    .map(|expires| expires > now)
    .unwrap_or(true)
}

fn identity_asset_dispatch_lease_active_state(entry: &Value, now: u64) -> bool {
    identity_asset_dispatch_state_is(entry, "leased")
        && identity_asset_dispatch_lease_active(entry, now)
}

fn identity_asset_dispatch_lease_expired(entry: &Value, now: u64) -> bool {
    identity_asset_dispatch_state_is(entry, "leased")
        && identity_asset_field_u64(
            entry,
            &[
                "lastDispatchLeaseExpiresUnixSeconds",
                "last_dispatch_lease_expires_unix_seconds",
            ],
        )
        .map(|expires| expires <= now)
        .unwrap_or(false)
}

fn identity_asset_dispatch_retry_waiting(entry: &Value, now: u64) -> bool {
    identity_asset_field_u64(
        entry,
        &[
            "lastDispatchRetryAfterUnixSeconds",
            "last_dispatch_retry_after_unix_seconds",
        ],
    )
    .map(|retry_after| retry_after > now)
    .unwrap_or(true)
}

fn identity_asset_dispatch_state_is(entry: &Value, expected: &str) -> bool {
    identity_asset_field_string(entry, &["dispatchState", "dispatch_state"])
        .map(|state| state.eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

fn identity_assets_capacity_status(
    asset_count: usize,
    runnable_count: usize,
    desired_concurrency: Option<usize>,
) -> String {
    if asset_count == 0 {
        "empty".to_string()
    } else if runnable_count == 0 {
        "exhausted".to_string()
    } else if desired_concurrency
        .map(|desired| runnable_count < desired)
        .unwrap_or(false)
    {
        "shortage".to_string()
    } else {
        "ready".to_string()
    }
}

fn identity_assets_status_recommendations(
    capacity_shortage_count: usize,
    block_reason_counts: &BTreeMap<String, usize>,
    expired_runtime_lease_count: usize,
    expired_dispatch_lease_count: usize,
    expired_cooldown_count: usize,
    active_runtime_lease_count: usize,
    active_dispatch_lease_count: usize,
    active_cooldown_count: usize,
    dispatch_retry_waiting_count: usize,
    missing_profile_dir_count: usize,
) -> Vec<IdentityAssetsStatusRecommendation> {
    let mut recommendations = Vec::new();
    if capacity_shortage_count > 0 {
        push_identity_assets_status_recommendation(
            &mut recommendations,
            "capacity_shortage",
            "high",
            capacity_shortage_count,
            "当前可运行 profile 少于期望并发,应先补充可运行账号或释放/修复阻塞资产。",
        );
    }

    let expired_cleanup_count =
        expired_runtime_lease_count + expired_dispatch_lease_count + expired_cooldown_count;
    if expired_cleanup_count > 0 {
        push_identity_assets_status_recommendation(
            &mut recommendations,
            "run_assets_sweep",
            "medium",
            expired_cleanup_count,
            "存在过期 runtime lease、dispatch lease 或 cooldown,建议先运行 identity-assets-sweep 清理残留。",
        );
    }

    if active_runtime_lease_count > 0 {
        push_identity_assets_status_recommendation(
            &mut recommendations,
            "runtime_leases_in_use",
            "info",
            active_runtime_lease_count,
            "存在正在执行业务的 runtime lease,需要等待完成或用 identity-assets-release 释放。",
        );
    }
    if active_dispatch_lease_count > 0 {
        push_identity_assets_status_recommendation(
            &mut recommendations,
            "dispatch_leases_in_use",
            "info",
            active_dispatch_lease_count,
            "存在仍被调度任务占用的 dispatch lease,需要等待任务完成或对账后再放量。",
        );
    }
    if active_cooldown_count > 0 || dispatch_retry_waiting_count > 0 {
        push_identity_assets_status_recommendation(
            &mut recommendations,
            "cooldown_or_retry_waiting",
            "info",
            active_cooldown_count + dispatch_retry_waiting_count,
            "部分资产仍在业务冷却或调度 retry 等待期,可降低并发或等待下一轮。",
        );
    }
    if missing_profile_dir_count > 0 {
        push_identity_assets_status_recommendation(
            &mut recommendations,
            "missing_profile_dir",
            "medium",
            missing_profile_dir_count,
            "部分资产缺少 profileDir/profilePath/userDataDir,需要补齐 profile 目录映射。",
        );
    }
    if let Some(count) = block_reason_counts.get("dispatch_failed") {
        push_identity_assets_status_recommendation(
            &mut recommendations,
            "review_failed_dispatch",
            "high",
            *count,
            "存在最新 dispatch failed 的资产,应先复盘失败原因并决定修复、冷却或隔离。",
        );
    }
    if let Some(count) = block_reason_counts.get("dispatch_cancelled") {
        push_identity_assets_status_recommendation(
            &mut recommendations,
            "review_cancelled_dispatch",
            "medium",
            *count,
            "存在最新 dispatch cancelled 的资产,应确认是否可重新入池。",
        );
    }

    let state_blocked_count = block_reason_counts
        .iter()
        .filter(|(reason, _)| reason.starts_with("state_not_allowed:"))
        .map(|(_, count)| *count)
        .sum::<usize>();
    if state_blocked_count > 0 {
        push_identity_assets_status_recommendation(
            &mut recommendations,
            "review_asset_states",
            "info",
            state_blocked_count,
            "部分资产因 state 不在允许集合内被跳过,可修复状态或用 --allow-state 明确放行。",
        );
    }

    recommendations
}

fn push_identity_assets_status_recommendation(
    recommendations: &mut Vec<IdentityAssetsStatusRecommendation>,
    code: &str,
    severity: &str,
    affected_count: usize,
    message: &str,
) {
    recommendations.push(IdentityAssetsStatusRecommendation {
        code: code.to_string(),
        severity: severity.to_string(),
        affected_count,
        message: message.to_string(),
    });
}

fn apply_identity_asset_runtime_leases(
    manifest: &mut Value,
    leases: &[(usize, String)],
    worker_id: Option<&str>,
    job_id: Option<&str>,
    generated_at: u64,
    lease_expires: u64,
) -> Result<()> {
    let entries = identity_plan_manifest_entries_mut(manifest)?;
    for (asset_index, lease_id) in leases {
        let Some(Value::Object(map)) = entries.get_mut(*asset_index) else {
            continue;
        };
        if let Some(previous) = map.get("runtimeLeaseState").cloned() {
            map.insert("previousRuntimeLeaseState".to_string(), previous);
        }
        map.insert(
            "runtimeLeaseState".to_string(),
            Value::String("leased".to_string()),
        );
        map.insert(
            "runtimeLeaseId".to_string(),
            Value::String(lease_id.clone()),
        );
        if let Some(worker_id) = worker_id {
            map.insert(
                "runtimeLeaseWorkerId".to_string(),
                Value::String(worker_id.to_string()),
            );
        }
        if let Some(job_id) = job_id {
            map.insert(
                "runtimeLeaseJobId".to_string(),
                Value::String(job_id.to_string()),
            );
        }
        map.insert(
            "runtimeLeaseExpiresUnixSeconds".to_string(),
            json!(lease_expires),
        );
        map.insert(
            "runtimeLeaseUpdatedAtUnixSeconds".to_string(),
            json!(generated_at),
        );
    }
    Ok(())
}

fn normalize_identity_asset_release_status(status: &str) -> Result<String> {
    let normalized = status.trim().to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "succeeded" | "success" | "ok" => Ok("succeeded".to_string()),
        "failed" | "failure" | "error" => Ok("failed".to_string()),
        "cancelled" | "canceled" | "cancel" => Ok("cancelled".to_string()),
        _ => bail!(
            "unsupported asset release status {status:?}; expected succeeded, failed, or cancelled"
        ),
    }
}

fn normalize_identity_asset_filter_set(values: &[String]) -> BTreeSet<String> {
    values
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn identity_asset_matches_release_filters(
    entry: &Value,
    worker_id: Option<&str>,
    job_id: Option<&str>,
    lease_filter: &BTreeSet<String>,
    account_filter: &BTreeSet<String>,
    profile_filter: &BTreeSet<String>,
    identity_filter: &BTreeSet<String>,
    label_filter: &BTreeSet<String>,
) -> bool {
    if let Some(worker_id) = worker_id {
        if identity_asset_field_string(entry, &["runtimeLeaseWorkerId", "runtime_lease_worker_id"])
            .as_deref()
            != Some(worker_id)
        {
            return false;
        }
    }
    if let Some(job_id) = job_id {
        if identity_asset_field_string(entry, &["runtimeLeaseJobId", "runtime_lease_job_id"])
            .as_deref()
            != Some(job_id)
        {
            return false;
        }
    }
    if !lease_filter.is_empty()
        && !identity_asset_field_string(entry, &["runtimeLeaseId", "runtime_lease_id"])
            .map(|value| lease_filter.contains(&value))
            .unwrap_or(false)
    {
        return false;
    }
    if !account_filter.is_empty()
        && !identity_asset_field_string(entry, &["accountId", "account_id"])
            .map(|value| account_filter.contains(&value))
            .unwrap_or(false)
    {
        return false;
    }
    if !profile_filter.is_empty()
        && !identity_asset_field_string(entry, &["profileId", "profile_id"])
            .map(|value| profile_filter.contains(&value))
            .unwrap_or(false)
    {
        return false;
    }
    if !identity_filter.is_empty()
        && !identity_asset_field_string(entry, &["identityId", "identity_id"])
            .map(|value| identity_filter.contains(&value))
            .unwrap_or(false)
    {
        return false;
    }
    if !label_filter.is_empty()
        && !identity_asset_field_string(entry, &["label", "name"])
            .map(|value| label_filter.contains(&value))
            .unwrap_or(false)
    {
        return false;
    }
    true
}

fn identity_asset_runtime_lease_present(entry: &Value) -> bool {
    identity_asset_field_string(entry, &["runtimeLeaseState", "runtime_lease_state"])
        .map(|state| state.eq_ignore_ascii_case("leased"))
        .unwrap_or(false)
}

fn apply_identity_asset_runtime_release(
    asset_index: usize,
    entry: &mut Value,
    status: &str,
    generated_at: u64,
    cooldown_until: Option<u64>,
    next_state: Option<&str>,
    message: Option<&str>,
    result: Option<&Value>,
) -> IdentityAssetReleaseItem {
    let account_id = identity_asset_field_string(entry, &["accountId", "account_id"]);
    let profile_id = identity_asset_field_string(entry, &["profileId", "profile_id"]);
    let identity_id = identity_asset_field_string(entry, &["identityId", "identity_id"]);
    let label = identity_asset_field_string(entry, &["label", "name"]);
    let profile_dir = identity_asset_profile_dir(entry);
    let lease_id = identity_asset_field_string(entry, &["runtimeLeaseId", "runtime_lease_id"]);
    let worker_id =
        identity_asset_field_string(entry, &["runtimeLeaseWorkerId", "runtime_lease_worker_id"]);
    let job_id = identity_asset_field_string(entry, &["runtimeLeaseJobId", "runtime_lease_job_id"]);
    let lease_expires = identity_asset_field_u64(
        entry,
        &[
            "runtimeLeaseExpiresUnixSeconds",
            "runtime_lease_expires_unix_seconds",
        ],
    );

    if let Value::Object(map) = entry {
        if let Some(value) = lease_id.clone() {
            map.insert("lastRuntimeLeaseId".to_string(), Value::String(value));
        }
        if let Some(value) = worker_id.clone() {
            map.insert("lastRuntimeWorkerId".to_string(), Value::String(value));
        }
        if let Some(value) = job_id.clone() {
            map.insert("lastRuntimeJobId".to_string(), Value::String(value));
        }
        if let Some(value) = lease_expires {
            map.insert(
                "lastRuntimeLeaseExpiresUnixSeconds".to_string(),
                json!(value),
            );
        }
        map.insert(
            "runtimeLeaseState".to_string(),
            Value::String("released".to_string()),
        );
        map.insert(
            "lastRuntimeStatus".to_string(),
            Value::String(status.to_string()),
        );
        map.insert(
            "lastRuntimeReleasedAtUnixSeconds".to_string(),
            json!(generated_at),
        );
        if let Some(message) = message {
            map.insert(
                "lastRuntimeMessage".to_string(),
                Value::String(message.to_string()),
            );
        }
        if let Some(result) = result {
            map.insert("lastRuntimeResult".to_string(), result.clone());
        }
        if let Some(cooldown_until) = cooldown_until {
            map.insert(
                "cooldownUntilUnixSeconds".to_string(),
                json!(cooldown_until),
            );
        } else if status == "succeeded" {
            map.remove("cooldownUntilUnixSeconds");
            map.remove("nextAvailableUnixSeconds");
        }
        if let Some(next_state) = next_state {
            if let Some(previous_state) = map.get("state").cloned() {
                map.insert("lastRuntimePreviousState".to_string(), previous_state);
            }
            map.insert("state".to_string(), Value::String(next_state.to_string()));
        }
        map.remove("runtimeLeaseId");
        map.remove("runtimeLeaseWorkerId");
        map.remove("runtimeLeaseJobId");
        map.remove("runtimeLeaseExpiresUnixSeconds");
    }

    IdentityAssetReleaseItem {
        asset_index,
        status: status.to_string(),
        account_id,
        profile_id,
        identity_id,
        label,
        profile_dir,
        lease_id,
        worker_id,
        job_id,
        cooldown_until_unix_seconds: cooldown_until,
    }
}

fn parse_identity_asset_runtime_release_events(
    text: &str,
    source_path: &Path,
) -> Result<Vec<IdentityAssetRuntimeReleaseEvent>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let source = source_path.display().to_string();
    let mut events = Vec::new();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        identity_asset_runtime_release_events_from_value(&value, &source, &mut events)?;
    } else {
        for (line_index, line) in trimmed.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let value = serde_json::from_str::<Value>(line).with_context(|| {
                format!("invalid release ledger JSON on line {}", line_index + 1)
            })?;
            identity_asset_runtime_release_events_from_value(&value, &source, &mut events)
                .with_context(|| {
                    format!("invalid release ledger item on line {}", line_index + 1)
                })?;
        }
    }
    Ok(events)
}

fn identity_asset_runtime_release_events_from_value(
    value: &Value,
    source_path: &str,
    events: &mut Vec<IdentityAssetRuntimeReleaseEvent>,
) -> Result<()> {
    if let Some(data) = value.get("data") {
        return identity_asset_runtime_release_events_from_value(data, source_path, events);
    }
    if let Some(items) = value.as_array() {
        for item in items {
            identity_asset_runtime_release_events_from_value(item, source_path, events)?;
        }
        return Ok(());
    }
    if let Some(items) = value
        .get("releasedAssets")
        .or_else(|| value.get("released_assets"))
        .and_then(Value::as_array)
    {
        for item in items {
            let event = identity_asset_runtime_release_event_from_parts(value, item, source_path)?;
            events.push(event);
        }
        return Ok(());
    }
    if let Some(item) = value.get("item") {
        let event = identity_asset_runtime_release_event_from_parts(value, item, source_path)?;
        events.push(event);
    }
    Ok(())
}

fn identity_asset_runtime_release_event_from_parts(
    outer: &Value,
    item: &Value,
    source_path: &str,
) -> Result<IdentityAssetRuntimeReleaseEvent> {
    let item = serde_json::from_value::<IdentityAssetReleaseItem>(item.clone())
        .context("failed to parse runtime release item")?;
    let status = identity_asset_field_string(outer, &["status"])
        .unwrap_or_else(|| item.status.clone())
        .to_ascii_lowercase();
    let worker_id = identity_asset_field_string(outer, &["workerId", "worker_id"])
        .or_else(|| item.worker_id.clone());
    let job_id =
        identity_asset_field_string(outer, &["jobId", "job_id"]).or_else(|| item.job_id.clone());
    let cooldown_until_unix_seconds = identity_asset_field_u64(
        outer,
        &["cooldownUntilUnixSeconds", "cooldown_until_unix_seconds"],
    )
    .or(item.cooldown_until_unix_seconds);
    Ok(IdentityAssetRuntimeReleaseEvent {
        event_index: 0,
        source_path: source_path.to_string(),
        generated_at_unix_seconds: identity_asset_field_u64(
            outer,
            &["generatedAtUnixSeconds", "generated_at_unix_seconds"],
        )
        .unwrap_or(0),
        status,
        worker_id,
        job_id,
        cooldown_until_unix_seconds,
        next_state: identity_asset_field_string(outer, &["nextState", "next_state"]),
        message: identity_asset_field_string(outer, &["message"]),
        result: outer.get("result").cloned(),
        item,
    })
}

fn apply_identity_asset_runtime_release_events(
    manifest: &mut Value,
    events: &[IdentityAssetRuntimeReleaseEvent],
    reconciled_at: u64,
) -> Result<(Vec<IdentityAssetRuntimeReconcileUpdate>, usize)> {
    let entries = identity_plan_manifest_entries_mut(manifest)?;
    let mut updates = Vec::new();
    let mut unmatched_event_count = 0usize;

    for event in events {
        let event_keys = identity_asset_runtime_release_event_match_keys(event);
        if event_keys.is_empty() {
            unmatched_event_count += 1;
            continue;
        }
        let mut matched_index = None;
        for (index, entry) in entries.iter().enumerate() {
            let entry_keys = identity_asset_runtime_manifest_entry_match_keys(entry);
            if !entry_keys.is_disjoint(&event_keys) {
                matched_index = Some(index);
                break;
            }
        }

        let Some(asset_index) = matched_index else {
            unmatched_event_count += 1;
            continue;
        };
        if let Some(entry) = entries.get_mut(asset_index) {
            apply_identity_asset_runtime_release_event(entry, event, reconciled_at);
            updates.push(IdentityAssetRuntimeReconcileUpdate {
                asset_index,
                event_index: event.event_index,
                status: event.status.clone(),
                account_id: event.item.account_id.clone(),
                profile_id: event.item.profile_id.clone(),
                identity_id: event.item.identity_id.clone(),
                label: event.item.label.clone(),
                lease_id: event.item.lease_id.clone(),
                worker_id: event
                    .item
                    .worker_id
                    .clone()
                    .or_else(|| event.worker_id.clone()),
                job_id: event.item.job_id.clone().or_else(|| event.job_id.clone()),
                released_at_unix_seconds: event.generated_at_unix_seconds,
            });
        }
    }

    Ok((updates, unmatched_event_count))
}

fn apply_identity_asset_runtime_release_event(
    entry: &mut Value,
    event: &IdentityAssetRuntimeReleaseEvent,
    reconciled_at: u64,
) {
    let Value::Object(map) = entry else {
        return;
    };
    if let Some(lease_id) = &event.item.lease_id {
        map.insert(
            "lastRuntimeLeaseId".to_string(),
            Value::String(lease_id.clone()),
        );
    }
    if let Some(worker_id) = event.item.worker_id.as_ref().or(event.worker_id.as_ref()) {
        map.insert(
            "lastRuntimeWorkerId".to_string(),
            Value::String(worker_id.clone()),
        );
    }
    if let Some(job_id) = event.item.job_id.as_ref().or(event.job_id.as_ref()) {
        map.insert(
            "lastRuntimeJobId".to_string(),
            Value::String(job_id.clone()),
        );
    }
    map.insert(
        "runtimeLeaseState".to_string(),
        Value::String("released".to_string()),
    );
    map.insert(
        "lastRuntimeStatus".to_string(),
        Value::String(event.status.clone()),
    );
    map.insert(
        "lastRuntimeReleasedAtUnixSeconds".to_string(),
        json!(event.generated_at_unix_seconds),
    );
    map.insert(
        "lastRuntimeReconciledAtUnixSeconds".to_string(),
        json!(reconciled_at),
    );
    map.insert(
        "lastRuntimeReleaseLedgerSource".to_string(),
        Value::String(event.source_path.clone()),
    );
    if let Some(message) = &event.message {
        map.insert(
            "lastRuntimeMessage".to_string(),
            Value::String(message.clone()),
        );
    }
    if let Some(result) = &event.result {
        map.insert("lastRuntimeResult".to_string(), result.clone());
    }
    if let Some(cooldown_until) = event
        .item
        .cooldown_until_unix_seconds
        .or(event.cooldown_until_unix_seconds)
    {
        map.insert(
            "cooldownUntilUnixSeconds".to_string(),
            json!(cooldown_until),
        );
    } else if event.status == "succeeded" {
        map.remove("cooldownUntilUnixSeconds");
        map.remove("cooldown_until_unix_seconds");
        map.remove("nextAvailableUnixSeconds");
        map.remove("next_available_unix_seconds");
    }
    if let Some(next_state) = &event.next_state {
        if let Some(previous_state) = map.get("state").cloned() {
            map.insert("lastRuntimePreviousState".to_string(), previous_state);
        }
        map.insert("state".to_string(), Value::String(next_state.clone()));
    }
    map.remove("runtimeLeaseId");
    map.remove("runtimeLeaseWorkerId");
    map.remove("runtimeLeaseJobId");
    map.remove("runtimeLeaseExpiresUnixSeconds");
}

fn identity_asset_runtime_manifest_entry_match_keys(entry: &Value) -> BTreeSet<String> {
    let mut keys = identity_plan_manifest_entry_match_keys(entry);
    push_identity_plan_value_key(&mut keys, "runtimeLeaseId", entry.get("runtimeLeaseId"));
    push_identity_plan_value_key(&mut keys, "runtimeLeaseId", entry.get("runtime_lease_id"));
    push_identity_plan_value_key(&mut keys, "runtimeLeaseId", entry.get("lastRuntimeLeaseId"));
    push_identity_plan_value_key(
        &mut keys,
        "runtimeLeaseId",
        entry.get("last_runtime_lease_id"),
    );
    keys
}

fn identity_asset_runtime_release_event_match_keys(
    event: &IdentityAssetRuntimeReleaseEvent,
) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    push_identity_asset_optional_match_key(&mut keys, "accountId", &event.item.account_id);
    push_identity_asset_optional_match_key(&mut keys, "profileId", &event.item.profile_id);
    push_identity_asset_optional_match_key(&mut keys, "identityId", &event.item.identity_id);
    push_identity_asset_optional_match_key(&mut keys, "label", &event.item.label);
    push_identity_asset_optional_match_key(&mut keys, "profileDir", &event.item.profile_dir);
    push_identity_asset_optional_match_key(&mut keys, "runtimeLeaseId", &event.item.lease_id);
    keys
}

fn push_identity_asset_optional_match_key(
    keys: &mut BTreeSet<String>,
    normalized_key: &str,
    value: &Option<String>,
) {
    let Some(value) = value
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    keys.insert(format!("{normalized_key}:{value}"));
}

#[derive(Debug, Default)]
struct IdentityAssetHealthBuild {
    matched_event_count: usize,
    unmatched_event_count: usize,
    healthy_count: usize,
    watch_count: usize,
    degraded_count: usize,
    quarantine_count: usize,
    unknown_count: usize,
    updated_asset_count: usize,
    action_counts: BTreeMap<String, usize>,
    items: Vec<IdentityAssetHealthItem>,
}

fn build_identity_asset_health_report_items(
    manifest: &mut Value,
    events: &[IdentityAssetRuntimeReleaseEvent],
    generated_at: u64,
    repair_threshold: usize,
    quarantine_threshold: usize,
    cooldown_seconds: u64,
    apply_manifest_updates: bool,
) -> Result<IdentityAssetHealthBuild> {
    let entries = identity_plan_manifest_entries(manifest)?;
    let mut events_by_asset = vec![Vec::<usize>::new(); entries.len()];
    let mut matched_event_count = 0usize;
    let mut unmatched_event_count = 0usize;

    for (event_index, event) in events.iter().enumerate() {
        let event_keys = identity_asset_runtime_release_event_match_keys(event);
        if event_keys.is_empty() {
            unmatched_event_count += 1;
            continue;
        }
        let mut matched_index = None;
        for (asset_index, entry) in entries.iter().enumerate() {
            let entry_keys = identity_asset_runtime_manifest_entry_match_keys(entry);
            if !entry_keys.is_disjoint(&event_keys) {
                matched_index = Some(asset_index);
                break;
            }
        }
        if let Some(asset_index) = matched_index {
            events_by_asset[asset_index].push(event_index);
            matched_event_count += 1;
        } else {
            unmatched_event_count += 1;
        }
    }

    let entries = identity_plan_manifest_entries_mut(manifest)?;
    let mut build = IdentityAssetHealthBuild {
        matched_event_count,
        unmatched_event_count,
        ..Default::default()
    };

    for (asset_index, entry) in entries.iter_mut().enumerate() {
        let item = identity_asset_health_item(
            asset_index,
            entry,
            events,
            &events_by_asset[asset_index],
            generated_at,
            repair_threshold,
            quarantine_threshold,
            cooldown_seconds,
        );
        *build
            .action_counts
            .entry(item.recommended_action.clone())
            .or_insert(0) += 1;
        match item.health_state.as_str() {
            "healthy" => build.healthy_count += 1,
            "watch" => build.watch_count += 1,
            "degraded" => build.degraded_count += 1,
            "quarantine" => build.quarantine_count += 1,
            _ => build.unknown_count += 1,
        }
        if apply_manifest_updates
            && identity_asset_health_action_mutates_manifest(&item.recommended_action)
        {
            apply_identity_asset_health_action(entry, &item, generated_at);
            build.updated_asset_count += 1;
        }
        build.items.push(item);
    }

    Ok(build)
}

fn identity_asset_health_item(
    asset_index: usize,
    entry: &Value,
    events: &[IdentityAssetRuntimeReleaseEvent],
    event_indexes: &[usize],
    generated_at: u64,
    repair_threshold: usize,
    quarantine_threshold: usize,
    cooldown_seconds: u64,
) -> IdentityAssetHealthItem {
    let event_count = event_indexes.len();
    let mut success_count = 0usize;
    let mut failed_count = 0usize;
    let mut cancelled_count = 0usize;
    let mut other_count = 0usize;

    for event_index in event_indexes {
        match events[*event_index].status.as_str() {
            "succeeded" => success_count += 1,
            "failed" => failed_count += 1,
            "cancelled" => cancelled_count += 1,
            _ => other_count += 1,
        }
    }

    let mut consecutive_unsuccessful_count = 0usize;
    for event_index in event_indexes.iter().rev() {
        if events[*event_index].status == "succeeded" {
            break;
        }
        consecutive_unsuccessful_count += 1;
    }

    let unsuccessful_count = failed_count + cancelled_count + other_count;
    let failure_rate = (event_count > 0).then(|| unsuccessful_count as f64 / event_count as f64);
    let health_score = failure_rate.map(|rate| {
        let rate_penalty = (rate * 45.0).round() as i32;
        let consecutive_penalty = (consecutive_unsuccessful_count.min(6) * 10) as i32;
        let score = 100i32
            .saturating_sub(rate_penalty)
            .saturating_sub(consecutive_penalty);
        score.clamp(0, 100) as u8
    });

    let (health_state, recommended_action) = if event_count == 0 {
        ("unknown", "no_runtime_history")
    } else if consecutive_unsuccessful_count >= quarantine_threshold {
        ("quarantine", "mark_quarantine")
    } else if consecutive_unsuccessful_count >= repair_threshold {
        ("degraded", "mark_repair")
    } else if consecutive_unsuccessful_count > 0 {
        ("watch", "watch")
    } else {
        ("healthy", "keep_active")
    };

    let last_event = event_indexes
        .last()
        .map(|event_index| &events[*event_index]);
    let cooldown_until_unix_seconds =
        identity_asset_health_action_mutates_manifest(recommended_action)
            .then(|| generated_at.saturating_add(cooldown_seconds));

    IdentityAssetHealthItem {
        asset_index,
        account_id: identity_asset_field_string(entry, &["accountId", "account_id"]),
        profile_id: identity_asset_field_string(entry, &["profileId", "profile_id"]),
        identity_id: identity_asset_field_string(entry, &["identityId", "identity_id"]),
        label: identity_asset_field_string(entry, &["label"]),
        profile_dir: identity_asset_profile_dir(entry),
        event_count,
        success_count,
        failed_count,
        cancelled_count,
        other_count,
        unsuccessful_count,
        consecutive_unsuccessful_count,
        failure_rate,
        health_score,
        health_state: health_state.to_string(),
        recommended_action: recommended_action.to_string(),
        last_status: last_event.map(|event| event.status.clone()),
        last_message: last_event.and_then(|event| event.message.clone()),
        last_result: last_event.and_then(|event| event.result.clone()),
        last_released_at_unix_seconds: last_event.map(|event| event.generated_at_unix_seconds),
        cooldown_until_unix_seconds,
    }
}

fn identity_asset_health_action_mutates_manifest(action: &str) -> bool {
    matches!(action, "mark_repair" | "mark_quarantine")
}

fn apply_identity_asset_health_action(
    entry: &mut Value,
    item: &IdentityAssetHealthItem,
    checked_at: u64,
) {
    let Value::Object(map) = entry else {
        return;
    };
    let target_state = match item.recommended_action.as_str() {
        "mark_quarantine" => "quarantine",
        "mark_repair" => "repair",
        _ => return,
    };

    if let Some(previous_state) = map.get("state").cloned() {
        if previous_state != Value::String(target_state.to_string()) {
            map.insert("lastRuntimeHealthPreviousState".to_string(), previous_state);
        }
    }
    map.insert("state".to_string(), Value::String(target_state.to_string()));
    map.insert(
        "lastRuntimeHealthCheckedAtUnixSeconds".to_string(),
        json!(checked_at),
    );
    map.insert(
        "lastRuntimeHealthState".to_string(),
        Value::String(item.health_state.clone()),
    );
    map.insert(
        "lastRuntimeHealthAction".to_string(),
        Value::String(item.recommended_action.clone()),
    );
    map.insert(
        "runtimeHealthEventCount".to_string(),
        json!(item.event_count),
    );
    map.insert(
        "runtimeUnsuccessfulCount".to_string(),
        json!(item.unsuccessful_count),
    );
    map.insert(
        "runtimeConsecutiveUnsuccessfulCount".to_string(),
        json!(item.consecutive_unsuccessful_count),
    );
    if let Some(score) = item.health_score {
        map.insert("runtimeHealthScore".to_string(), json!(score));
    }
    if let Some(last_status) = &item.last_status {
        map.insert(
            "lastRuntimeHealthStatus".to_string(),
            Value::String(last_status.clone()),
        );
    }
    if let Some(last_message) = &item.last_message {
        map.insert(
            "lastRuntimeHealthMessage".to_string(),
            Value::String(last_message.clone()),
        );
    }
    if let Some(last_result) = &item.last_result {
        map.insert("lastRuntimeHealthResult".to_string(), last_result.clone());
    }
    if let Some(last_released_at) = item.last_released_at_unix_seconds {
        map.insert(
            "lastRuntimeHealthReleasedAtUnixSeconds".to_string(),
            json!(last_released_at),
        );
    }
    if let Some(cooldown_until) = item.cooldown_until_unix_seconds {
        map.insert(
            "cooldownUntilUnixSeconds".to_string(),
            json!(cooldown_until),
        );
    }
}

fn sweep_identity_asset_runtime_lease(
    asset_index: usize,
    entry: &mut Value,
    now: u64,
    grace_seconds: u64,
) -> Option<IdentityAssetSweepItem> {
    if !identity_asset_runtime_lease_present(entry) {
        return None;
    }
    let expires = identity_asset_field_u64(
        entry,
        &[
            "runtimeLeaseExpiresUnixSeconds",
            "runtime_lease_expires_unix_seconds",
        ],
    )?;
    if expires.saturating_add(grace_seconds) > now {
        return None;
    }
    let previous = json!({
        "runtimeLeaseState": identity_asset_field_string(entry, &["runtimeLeaseState", "runtime_lease_state"]),
        "runtimeLeaseId": identity_asset_field_string(entry, &["runtimeLeaseId", "runtime_lease_id"]),
        "runtimeLeaseWorkerId": identity_asset_field_string(entry, &["runtimeLeaseWorkerId", "runtime_lease_worker_id"]),
        "runtimeLeaseJobId": identity_asset_field_string(entry, &["runtimeLeaseJobId", "runtime_lease_job_id"]),
        "runtimeLeaseExpiresUnixSeconds": expires,
    });

    if let Value::Object(map) = entry {
        if let Some(value) = previous
            .get("runtimeLeaseId")
            .and_then(label_value_to_string)
        {
            map.insert("lastRuntimeLeaseId".to_string(), Value::String(value));
        }
        if let Some(value) = previous
            .get("runtimeLeaseWorkerId")
            .and_then(label_value_to_string)
        {
            map.insert("lastRuntimeWorkerId".to_string(), Value::String(value));
        }
        if let Some(value) = previous
            .get("runtimeLeaseJobId")
            .and_then(label_value_to_string)
        {
            map.insert("lastRuntimeJobId".to_string(), Value::String(value));
        }
        map.insert(
            "lastRuntimeLeaseExpiresUnixSeconds".to_string(),
            json!(expires),
        );
        map.insert(
            "runtimeLeaseState".to_string(),
            Value::String("expired".to_string()),
        );
        map.insert(
            "lastRuntimeStatus".to_string(),
            Value::String("expired".to_string()),
        );
        map.insert("lastRuntimeExpiredAtUnixSeconds".to_string(), json!(now));
        map.remove("runtimeLeaseId");
        map.remove("runtimeLeaseWorkerId");
        map.remove("runtimeLeaseJobId");
        map.remove("runtimeLeaseExpiresUnixSeconds");
    }

    Some(identity_asset_sweep_item(
        asset_index,
        entry,
        "runtime_lease_expired",
        Some(previous),
        Some(json!({
            "runtimeLeaseState": "expired",
            "lastRuntimeStatus": "expired",
            "lastRuntimeExpiredAtUnixSeconds": now,
        })),
    ))
}

fn sweep_identity_asset_dispatch_lease(
    asset_index: usize,
    entry: &mut Value,
    now: u64,
    grace_seconds: u64,
) -> Option<IdentityAssetSweepItem> {
    let dispatch_state = identity_asset_field_string(entry, &["dispatchState", "dispatch_state"])?
        .to_ascii_lowercase();
    if dispatch_state != "leased" {
        return None;
    }
    let expires = identity_asset_field_u64(
        entry,
        &[
            "lastDispatchLeaseExpiresUnixSeconds",
            "last_dispatch_lease_expires_unix_seconds",
        ],
    )?;
    if expires.saturating_add(grace_seconds) > now {
        return None;
    }
    let previous = json!({
        "dispatchState": "leased",
        "lastDispatchStatus": identity_asset_field_string(entry, &["lastDispatchStatus", "last_dispatch_status"]),
        "lastDispatchLeaseExpiresUnixSeconds": expires,
    });

    if let Value::Object(map) = entry {
        map.insert(
            "dispatchState".to_string(),
            Value::String("expired".to_string()),
        );
        map.insert(
            "lastDispatchStatus".to_string(),
            Value::String("expired".to_string()),
        );
        map.insert("lastDispatchExpiredAtUnixSeconds".to_string(), json!(now));
    }

    Some(identity_asset_sweep_item(
        asset_index,
        entry,
        "dispatch_lease_expired",
        Some(previous),
        Some(json!({
            "dispatchState": "expired",
            "lastDispatchStatus": "expired",
            "lastDispatchExpiredAtUnixSeconds": now,
        })),
    ))
}

fn sweep_identity_asset_cooldown(
    asset_index: usize,
    entry: &mut Value,
    now: u64,
    grace_seconds: u64,
) -> Option<IdentityAssetSweepItem> {
    let cooldown_until = identity_asset_field_u64(
        entry,
        &[
            "cooldownUntilUnixSeconds",
            "cooldown_until_unix_seconds",
            "nextAvailableUnixSeconds",
            "next_available_unix_seconds",
        ],
    )?;
    if cooldown_until.saturating_add(grace_seconds) > now {
        return None;
    }
    let previous = json!({
        "cooldownUntilUnixSeconds": identity_asset_field_u64(entry, &["cooldownUntilUnixSeconds", "cooldown_until_unix_seconds"]),
        "nextAvailableUnixSeconds": identity_asset_field_u64(entry, &["nextAvailableUnixSeconds", "next_available_unix_seconds"]),
    });

    if let Value::Object(map) = entry {
        map.insert(
            "lastCooldownUntilUnixSeconds".to_string(),
            json!(cooldown_until),
        );
        map.insert("lastCooldownClearedAtUnixSeconds".to_string(), json!(now));
        map.remove("cooldownUntilUnixSeconds");
        map.remove("cooldown_until_unix_seconds");
        map.remove("nextAvailableUnixSeconds");
        map.remove("next_available_unix_seconds");
    }

    Some(identity_asset_sweep_item(
        asset_index,
        entry,
        "cooldown_cleared",
        Some(previous),
        Some(json!({
            "lastCooldownUntilUnixSeconds": cooldown_until,
            "lastCooldownClearedAtUnixSeconds": now,
        })),
    ))
}

fn identity_asset_sweep_item(
    asset_index: usize,
    entry: &Value,
    action: &str,
    previous_value: Option<Value>,
    next_value: Option<Value>,
) -> IdentityAssetSweepItem {
    IdentityAssetSweepItem {
        asset_index,
        action: action.to_string(),
        account_id: identity_asset_field_string(entry, &["accountId", "account_id"]),
        profile_id: identity_asset_field_string(entry, &["profileId", "profile_id"]),
        identity_id: identity_asset_field_string(entry, &["identityId", "identity_id"]),
        label: identity_asset_field_string(entry, &["label", "name"]),
        previous_value,
        next_value,
    }
}

async fn write_identity_dispatch_claim(
    report: &IdentityDispatchClaimReport,
    path: Option<&Path>,
    append: bool,
) -> Result<Option<IdentityDispatchClaimOut>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if append {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        for item in &report.items {
            let line = json!({
                "claimId": report.claim_id,
                "workerId": report.worker_id,
                "generatedAtUnixSeconds": report.generated_at_unix_seconds,
                "leaseExpiresUnixSeconds": report.lease_expires_unix_seconds,
                "item": item,
            });
            file.write_all(serde_json::to_string(&line)?.as_bytes())
                .await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
    } else {
        tokio::fs::write(path, serde_json::to_vec_pretty(report)?)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(Some(IdentityDispatchClaimOut {
        path: path.display().to_string(),
        append,
        count: report.items.len(),
        format: if append {
            "ndjson_claim_items".to_string()
        } else {
            "json_report".to_string()
        },
    }))
}

async fn write_identity_dispatch_renewal(
    report: &IdentityDispatchRenewReport,
    path: Option<&Path>,
    append: bool,
) -> Result<Option<IdentityDispatchClaimOut>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if append {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        for item in &report.items {
            let line = json!({
                "renewalId": report.renewal_id,
                "workerId": item.worker_id,
                "generatedAtUnixSeconds": report.generated_at_unix_seconds,
                "leaseExpiresUnixSeconds": report.lease_expires_unix_seconds,
                "item": item,
            });
            file.write_all(serde_json::to_string(&line)?.as_bytes())
                .await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
    } else {
        tokio::fs::write(path, serde_json::to_vec_pretty(report)?)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(Some(IdentityDispatchClaimOut {
        path: path.display().to_string(),
        append,
        count: report.items.len(),
        format: if append {
            "ndjson_claim_items".to_string()
        } else {
            "json_report".to_string()
        },
    }))
}

async fn write_identity_dispatch_reconcile_manifest(
    source: &Path,
    destination: Option<&Path>,
    manifest: &Value,
    asset_count: usize,
    updated_count: usize,
    unmatched_event_count: usize,
    state_counts: BTreeMap<String, usize>,
    dispatch_state_counts: BTreeMap<String, usize>,
) -> Result<Option<IdentityDispatchReconcileManifestOut>> {
    let Some(destination) = destination else {
        return Ok(None);
    };
    if let Some(parent) = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(manifest)?;
    tokio::fs::write(destination, &bytes)
        .await
        .with_context(|| format!("failed to write {}", destination.display()))?;

    Ok(Some(IdentityDispatchReconcileManifestOut {
        source_path: source.display().to_string(),
        path: destination.display().to_string(),
        format: "profile_assets_json".to_string(),
        asset_count,
        updated_count,
        unchanged_count: asset_count.saturating_sub(updated_count),
        unmatched_event_count,
        state_counts,
        dispatch_state_counts,
        bytes: bytes.len(),
    }))
}

async fn write_identity_asset_selection_manifest(
    source: &Path,
    destination: Option<&Path>,
    manifest: &Value,
    asset_count: usize,
    leased_count: usize,
) -> Result<Option<IdentityAssetSelectionManifestOut>> {
    let Some(destination) = destination else {
        return Ok(None);
    };
    if let Some(parent) = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(manifest)?;
    tokio::fs::write(destination, &bytes)
        .await
        .with_context(|| format!("failed to write {}", destination.display()))?;

    Ok(Some(IdentityAssetSelectionManifestOut {
        source_path: source.display().to_string(),
        path: destination.display().to_string(),
        format: "profile_assets_json".to_string(),
        asset_count,
        leased_count,
        bytes: bytes.len(),
    }))
}

async fn write_identity_asset_selection_report(
    report: &IdentityAssetsSelectReport,
    path: Option<&Path>,
) -> Result<Option<IdentityAssetSelectionOut>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(report)?;
    tokio::fs::write(path, &bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(Some(IdentityAssetSelectionOut {
        path: path.display().to_string(),
        format: "json_report".to_string(),
        bytes: bytes.len(),
    }))
}

async fn write_identity_asset_status_report(
    report: &IdentityAssetsStatusReport,
    path: Option<&Path>,
) -> Result<Option<IdentityAssetStatusOut>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(report)?;
    tokio::fs::write(path, &bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(Some(IdentityAssetStatusOut {
        path: path.display().to_string(),
        format: "json_report".to_string(),
        bytes: bytes.len(),
    }))
}

async fn write_identity_asset_forecast_report(
    report: &IdentityAssetsForecastReport,
    path: Option<&Path>,
) -> Result<Option<IdentityAssetForecastOut>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(report)?;
    tokio::fs::write(path, &bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(Some(IdentityAssetForecastOut {
        path: path.display().to_string(),
        format: "json_report".to_string(),
        bytes: bytes.len(),
    }))
}

async fn write_identity_asset_gate_report(
    report: &IdentityAssetsGateReport,
    path: Option<&Path>,
) -> Result<Option<IdentityAssetGateOut>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(report)?;
    tokio::fs::write(path, &bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(Some(IdentityAssetGateOut {
        path: path.display().to_string(),
        format: "json_report".to_string(),
        bytes: bytes.len(),
    }))
}

async fn write_identity_asset_validate_report(
    report: &IdentityAssetsValidateReport,
    path: Option<&Path>,
) -> Result<Option<IdentityAssetValidateOut>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(report)?;
    tokio::fs::write(path, &bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(Some(IdentityAssetValidateOut {
        path: path.display().to_string(),
        format: "json_report".to_string(),
        bytes: bytes.len(),
    }))
}

async fn write_identity_asset_release_manifest(
    source: &Path,
    destination: Option<&Path>,
    manifest: &Value,
    asset_count: usize,
    released_count: usize,
) -> Result<Option<IdentityAssetReleaseManifestOut>> {
    let Some(destination) = destination else {
        return Ok(None);
    };
    if let Some(parent) = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(manifest)?;
    tokio::fs::write(destination, &bytes)
        .await
        .with_context(|| format!("failed to write {}", destination.display()))?;

    Ok(Some(IdentityAssetReleaseManifestOut {
        source_path: source.display().to_string(),
        path: destination.display().to_string(),
        format: "profile_assets_json".to_string(),
        asset_count,
        released_count,
        bytes: bytes.len(),
    }))
}

async fn write_identity_asset_release_report(
    report: &IdentityAssetsReleaseReport,
    path: Option<&Path>,
    append: bool,
) -> Result<Option<IdentityAssetReleaseOut>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let bytes = if append {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        let mut bytes = 0usize;
        for item in &report.released_assets {
            let line = json!({
                "scope": report.scope,
                "assetManifest": report.asset_manifest,
                "generatedAtUnixSeconds": report.generated_at_unix_seconds,
                "status": report.status,
                "workerId": report.worker_id,
                "jobId": report.job_id,
                "cooldownUntilUnixSeconds": report.cooldown_until_unix_seconds,
                "nextState": report.next_state,
                "message": report.message,
                "result": report.result,
                "item": item,
            });
            let text = serde_json::to_string(&line)?;
            bytes += text.len() + 1;
            file.write_all(text.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
        bytes
    } else {
        let bytes = serde_json::to_vec_pretty(report)?;
        tokio::fs::write(path, &bytes)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
        bytes.len()
    };

    Ok(Some(IdentityAssetReleaseOut {
        path: path.display().to_string(),
        append,
        count: report.released_assets.len(),
        format: if append {
            "ndjson_release_items".to_string()
        } else {
            "json_report".to_string()
        },
        bytes,
    }))
}

async fn write_identity_asset_runtime_reconcile_manifest(
    source: &Path,
    destination: Option<&Path>,
    manifest: &Value,
    asset_count: usize,
    updated_count: usize,
    unmatched_event_count: usize,
    state_counts: BTreeMap<String, usize>,
    runtime_lease_state_counts: BTreeMap<String, usize>,
) -> Result<Option<IdentityAssetRuntimeReconcileManifestOut>> {
    let Some(destination) = destination else {
        return Ok(None);
    };
    if let Some(parent) = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(manifest)?;
    tokio::fs::write(destination, &bytes)
        .await
        .with_context(|| format!("failed to write {}", destination.display()))?;

    Ok(Some(IdentityAssetRuntimeReconcileManifestOut {
        source_path: source.display().to_string(),
        path: destination.display().to_string(),
        format: "profile_assets_json".to_string(),
        asset_count,
        updated_count,
        unchanged_count: asset_count.saturating_sub(updated_count),
        unmatched_event_count,
        state_counts,
        runtime_lease_state_counts,
        bytes: bytes.len(),
    }))
}

async fn write_identity_asset_health_manifest(
    source: &Path,
    destination: Option<&Path>,
    manifest: &Value,
    asset_count: usize,
    updated_count: usize,
    action_counts: BTreeMap<String, usize>,
    state_counts: BTreeMap<String, usize>,
    runtime_lease_state_counts: BTreeMap<String, usize>,
) -> Result<Option<IdentityAssetHealthManifestOut>> {
    let Some(destination) = destination else {
        return Ok(None);
    };
    if let Some(parent) = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(manifest)?;
    tokio::fs::write(destination, &bytes)
        .await
        .with_context(|| format!("failed to write {}", destination.display()))?;

    Ok(Some(IdentityAssetHealthManifestOut {
        source_path: source.display().to_string(),
        path: destination.display().to_string(),
        format: "profile_assets_json".to_string(),
        asset_count,
        updated_count,
        action_counts,
        state_counts,
        runtime_lease_state_counts,
        bytes: bytes.len(),
    }))
}

async fn write_identity_asset_health_report(
    report: &IdentityAssetsHealthReport,
    path: Option<&Path>,
) -> Result<Option<IdentityAssetHealthOut>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(report)?;
    tokio::fs::write(path, &bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(Some(IdentityAssetHealthOut {
        path: path.display().to_string(),
        format: "json_report".to_string(),
        bytes: bytes.len(),
    }))
}

async fn write_identity_asset_sweep_manifest(
    source: &Path,
    destination: Option<&Path>,
    manifest: &Value,
    asset_count: usize,
    updated_count: usize,
) -> Result<Option<IdentityAssetSweepManifestOut>> {
    let Some(destination) = destination else {
        return Ok(None);
    };
    if let Some(parent) = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(manifest)?;
    tokio::fs::write(destination, &bytes)
        .await
        .with_context(|| format!("failed to write {}", destination.display()))?;

    Ok(Some(IdentityAssetSweepManifestOut {
        source_path: source.display().to_string(),
        path: destination.display().to_string(),
        format: "profile_assets_json".to_string(),
        asset_count,
        updated_count,
        bytes: bytes.len(),
    }))
}

async fn write_identity_asset_sweep_report(
    report: &IdentityAssetsSweepReport,
    path: Option<&Path>,
) -> Result<Option<IdentityAssetSweepOut>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(report)?;
    tokio::fs::write(path, &bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(Some(IdentityAssetSweepOut {
        path: path.display().to_string(),
        format: "json_report".to_string(),
        bytes: bytes.len(),
    }))
}

async fn read_identity_dispatch_completion_ledger(
    path: Option<&Path>,
) -> Result<Vec<IdentityDispatchCompletionItem>> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let text = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read completion ledger {}", path.display()))?;
    parse_identity_dispatch_completion_items(&text)
        .with_context(|| format!("failed to parse completion ledger {}", path.display()))
}

fn parse_identity_dispatch_completion_items(
    text: &str,
) -> Result<Vec<IdentityDispatchCompletionItem>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let values = if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        vec![value]
    } else {
        let mut values = Vec::new();
        for (line_index, line) in trimmed.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            values.push(
                serde_json::from_str::<Value>(line)
                    .with_context(|| format!("invalid JSON at line {}", line_index + 1))?,
            );
        }
        values
    };

    let mut items = Vec::new();
    for value in &values {
        items.extend(identity_dispatch_completion_items_from_value(value)?);
    }
    Ok(items)
}

fn identity_dispatch_completion_items_from_value(
    value: &Value,
) -> Result<Vec<IdentityDispatchCompletionItem>> {
    if let Some(data) = value.get("data") {
        return identity_dispatch_completion_items_from_value(data);
    }
    if let Some(item) = value.get("item") {
        return Ok(vec![
            serde_json::from_value(item.clone()).context("invalid completion item")?,
        ]);
    }
    if value.get("claim").is_some() && value.get("completedAtUnixSeconds").is_some() {
        return Ok(vec![
            serde_json::from_value(value.clone()).context("invalid completion item")?,
        ]);
    }
    if let Some(items) = value.get("items").and_then(Value::as_array) {
        return items
            .iter()
            .map(|item| serde_json::from_value(item.clone()).context("invalid completion item"))
            .collect();
    }
    if let Some(values) = value.as_array() {
        let mut out = Vec::new();
        for value in values {
            out.extend(identity_dispatch_completion_items_from_value(value)?);
        }
        return Ok(out);
    }
    Ok(Vec::new())
}

fn normalize_identity_dispatch_completion_status(status: &str) -> Result<String> {
    let normalized = status.trim().to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "succeeded" | "success" | "ok" => Ok("succeeded".to_string()),
        "failed" | "failure" | "error" => Ok("failed".to_string()),
        "retry" | "retryable" => Ok("retry".to_string()),
        "cancelled" | "canceled" | "cancel" => Ok("cancelled".to_string()),
        _ => bail!(
            "unsupported completion status {status:?}; expected succeeded, failed, retry, or cancelled"
        ),
    }
}

fn identity_dispatch_completion_retry_eligible(status: &str, retryable: bool) -> bool {
    status == "retry" || (status == "failed" && retryable)
}

fn terminal_identity_dispatch_completions(
    items: &[IdentityDispatchCompletionItem],
) -> (BTreeSet<String>, usize, usize) {
    let mut latest_by_dedupe: BTreeMap<String, &IdentityDispatchCompletionItem> = BTreeMap::new();
    for item in items {
        let key = item.dispatch.dedupe_key.clone();
        let replace = latest_by_dedupe
            .get(&key)
            .map(|previous| item.completed_at_unix_seconds >= previous.completed_at_unix_seconds)
            .unwrap_or(true);
        if replace {
            latest_by_dedupe.insert(key, item);
        }
    }

    let mut terminal = BTreeSet::new();
    let mut retryable_count = 0usize;
    for (dedupe_key, item) in latest_by_dedupe {
        if item.retry_eligible || item.status == "retry" {
            retryable_count += 1;
            continue;
        }
        if matches!(item.status.as_str(), "succeeded" | "failed" | "cancelled") {
            terminal.insert(dedupe_key);
        }
    }
    let terminal_count = terminal.len();
    (terminal, terminal_count, retryable_count)
}

async fn write_identity_dispatch_completion(
    report: &IdentityDispatchCompletionReport,
    path: Option<&Path>,
    append: bool,
) -> Result<Option<IdentityDispatchCompletionOut>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if append {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        for item in &report.items {
            let line = json!({
                "completionId": report.completion_id,
                "workerId": item.worker_id,
                "completedAtUnixSeconds": item.completed_at_unix_seconds,
                "item": item,
            });
            file.write_all(serde_json::to_string(&line)?.as_bytes())
                .await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
    } else {
        tokio::fs::write(path, serde_json::to_vec_pretty(report)?)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(Some(IdentityDispatchCompletionOut {
        path: path.display().to_string(),
        append,
        count: report.items.len(),
        format: if append {
            "ndjson_completion_items".to_string()
        } else {
            "json_report".to_string()
        },
    }))
}

fn parse_identity_plan_values(text: &str) -> Result<(String, Vec<Value>)> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("identity plan input is empty");
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Ok(("json".to_string(), vec![value]));
    }

    let mut values = Vec::new();
    for (line_index, line) in trimmed.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        values.push(
            serde_json::from_str::<Value>(line)
                .with_context(|| format!("invalid JSON at line {}", line_index + 1))?,
        );
    }
    if values.is_empty() {
        bail!("identity plan input has no JSON values");
    }
    Ok(("ndjson".to_string(), values))
}

fn identity_plan_action_values_from_value(value: &Value) -> Vec<Value> {
    if let Some(data) = value.get("data") {
        return identity_plan_action_values_from_value(data);
    }
    if let Some(queue) = value.get("actionQueue") {
        return identity_plan_action_values_from_value(queue);
    }
    if let Some(actions) = value.get("actions").and_then(Value::as_array) {
        return actions.clone();
    }
    if value.get("actionCode").is_some() || value.get("code").is_some() {
        return vec![value.clone()];
    }
    Vec::new()
}

fn identity_plan_asset_patch_values_from_value(value: &Value) -> Vec<Value> {
    if let Some(data) = value.get("data") {
        return identity_plan_asset_patch_values_from_value(data);
    }
    if let Some(patches) = value.get("assetPatches").and_then(Value::as_array) {
        return patches.clone();
    }
    if let Some(patches) = value.get("patches").and_then(Value::as_array) {
        return patches.clone();
    }
    if let Some(patch) = value.get("patch").and_then(Value::as_object) {
        return vec![Value::Object(patch.clone())];
    }
    Vec::new()
}

fn identity_plan_operation_values_from_value(value: &Value) -> Vec<Value> {
    if let Some(data) = value.get("data") {
        return identity_plan_operation_values_from_value(data);
    }
    value
        .get("operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn identity_plan_gate_from_value(value: &Value) -> Option<(bool, Vec<String>)> {
    if let Some(data) = value.get("data") {
        return identity_plan_gate_from_value(data);
    }
    let gate = value.get("gate")?;
    let passed = gate.get("passed").and_then(Value::as_bool)?;
    let failures = string_values_for_keys(gate, &["failures"]);
    Some((passed, failures))
}

fn identity_plan_scope_from_value(value: &Value) -> Option<String> {
    value
        .get("data")
        .and_then(identity_plan_scope_from_value)
        .or_else(|| {
            value
                .get("scope")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn identity_plan_ok_from_value(value: &Value) -> Option<bool> {
    value.get("ok").and_then(Value::as_bool)
}

fn identity_plan_status(value: &Value) -> Option<String> {
    value
        .get("status")
        .and_then(Value::as_str)
        .map(|status| status.to_ascii_lowercase())
}

fn identity_plan_action_from_value(
    value: &Value,
    plan_index: usize,
    input_index: usize,
    input_path: &str,
) -> IdentityPlanAction {
    IdentityPlanAction {
        plan_index,
        input_index,
        input_path: input_path.to_string(),
        action_code: string_field_for_keys(value, &["actionCode", "code"])
            .unwrap_or_else(|| "unknown".to_string()),
        source: string_field_for_keys(value, &["source"]),
        priority: string_field_for_keys(value, &["priority"]),
        target: identity_plan_stringified_field(value, "target"),
        state: string_field_for_keys(value, &["state"]),
        label: first_string_for_keys(value, &["label", "labels"]),
        identity_id: first_string_for_keys(
            value,
            &[
                "identityId",
                "identityIds",
                "beforeId",
                "afterId",
                "candidateId",
                "candidateIds",
            ],
        ),
        title: string_field_for_keys(value, &["title"]),
        detail: string_field_for_keys(value, &["detail"]),
        estimated_gain: value
            .get("estimatedGain")
            .or_else(|| value.get("estimated_gain"))
            .and_then(Value::as_f64),
        affected_count: value
            .get("affectedCount")
            .or_else(|| value.get("affected_count"))
            .and_then(Value::as_u64),
        reasons: string_values_for_keys(value, &["reasons", "reasonCodes", "reason_codes"]),
        signal_codes: string_values_for_keys(value, &["signalCodes", "signal_codes"]),
    }
}

fn identity_plan_patch_from_value(
    value: &Value,
    plan_index: usize,
    input_index: usize,
    input_path: &str,
) -> Option<IdentityPlanAssetPatch> {
    let Value::Object(map) = value else {
        return None;
    };
    Some(IdentityPlanAssetPatch {
        plan_index,
        input_index,
        input_path: input_path.to_string(),
        patch: map
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    })
}

fn first_string_for_keys(value: &Value, keys: &[&str]) -> Option<String> {
    string_values_for_keys(value, keys).into_iter().next()
}

fn identity_plan_stringified_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|field| match field {
        Value::String(text) => Some(text.clone()),
        Value::Null => None,
        other => serde_json::to_string(other).ok(),
    })
}

fn build_identity_plan_summary(
    input_count: usize,
    value_count: usize,
    actions: &[IdentityPlanAction],
    asset_patches: &[IdentityPlanAssetPatch],
    failed_operation_count: usize,
    unresolved_operation_count: usize,
    gate_failure_count: usize,
) -> IdentityPlanSummary {
    IdentityPlanSummary {
        input_count,
        value_count,
        action_count: actions.len(),
        high_priority_action_count: actions
            .iter()
            .filter(|action| {
                action
                    .priority
                    .as_deref()
                    .map(|priority| {
                        let priority = priority.to_ascii_lowercase();
                        priority == "high" || priority == "critical"
                    })
                    .unwrap_or(false)
            })
            .count(),
        quarantine_action_count: actions
            .iter()
            .filter(|action| action.action_code.contains("quarantine"))
            .count(),
        remediation_action_count: actions
            .iter()
            .filter(|action| {
                action.action_code.starts_with("drift.restore")
                    || action.action_code.starts_with("drift.hide")
                    || action.action_code.starts_with("drift.rebind")
                    || action.action_code.starts_with("drift.sync")
                    || action.action_code.starts_with("pool.disperse")
                    || action.action_code.starts_with("pool.rotate")
            })
            .count(),
        capacity_action_count: actions
            .iter()
            .filter(|action| {
                action.source.as_deref() == Some("capacity")
                    || action.action_code.starts_with("capacity.")
            })
            .count(),
        lifecycle_action_count: actions
            .iter()
            .filter(|action| {
                action.source.as_deref() == Some("lifecycle")
                    || action.action_code.starts_with("lifecycle.")
            })
            .count(),
        asset_patch_count: asset_patches.len(),
        failed_operation_count,
        unresolved_operation_count,
        dispatch_item_count: 0,
        gate_failed: gate_failure_count > 0,
        gate_failure_count,
    }
}

fn count_plan_action_codes(actions: &[IdentityPlanAction]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for action in actions {
        *counts.entry(action.action_code.clone()).or_insert(0) += 1;
    }
    counts
}

fn count_plan_priorities(actions: &[IdentityPlanAction]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for action in actions {
        if let Some(priority) = &action.priority {
            *counts.entry(priority.clone()).or_insert(0) += 1;
        }
    }
    counts
}

fn count_plan_states(
    actions: &[IdentityPlanAction],
    asset_patches: &[IdentityPlanAssetPatch],
) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for action in actions {
        if let Some(state) = &action.state {
            *counts.entry(state.clone()).or_insert(0) += 1;
        }
    }
    for patch in asset_patches {
        if let Some(state) = patch.patch.get("nextState").and_then(Value::as_str) {
            *counts.entry(state.to_string()).or_insert(0) += 1;
        }
    }
    counts
}

fn build_identity_plan_recommendations(summary: &IdentityPlanSummary) -> Vec<String> {
    let mut recommendations = Vec::new();
    if summary.gate_failed {
        recommendations.push(format!(
            "先处理 {} 条 gate failure,不要把失败批次直接写回可用资产池。",
            summary.gate_failure_count
        ));
    }
    if summary.quarantine_action_count > 0 {
        recommendations.push(format!(
            "先执行或确认 {} 条隔离动作,把高风险/重复画像从活跃池移出。",
            summary.quarantine_action_count
        ));
    }
    if summary.unresolved_operation_count > 0 || summary.failed_operation_count > 0 {
        recommendations.push(format!(
            "修复 apply 阶段的 {} 条 unresolved 和 {} 条 failed 操作,否则资产状态会和文件系统脱节。",
            summary.unresolved_operation_count, summary.failed_operation_count
        ));
    }
    if summary.capacity_action_count > 0 {
        recommendations.push(format!(
            "按 {} 条 capacity 动作补画像差异,优先补瓶颈信号而不是盲目加同模板账号。",
            summary.capacity_action_count
        ));
    }
    if summary.remediation_action_count > 0 {
        recommendations.push(format!(
            "把 {} 条修复动作转换成 profile 配置变更,再重新采样做 drift/pool 复验。",
            summary.remediation_action_count
        ));
    }
    if summary.asset_patch_count > 0 {
        recommendations.push(format!(
            "将 {} 条 asset patch 写回资产 manifest 或状态账本,让调度器下一轮按新状态分配任务。",
            summary.asset_patch_count
        ));
    }
    if recommendations.is_empty() {
        recommendations.push("当前输入没有可执行治理动作;保留本轮报告作为审计基线。".to_string());
    }
    recommendations
}

fn build_identity_plan_runbook(
    summary: &IdentityPlanSummary,
    actions: &[IdentityPlanAction],
    asset_patches: &[IdentityPlanAssetPatch],
) -> Vec<IdentityPlanRunbookStep> {
    let mut steps = Vec::new();
    if summary.gate_failed {
        steps.push(identity_plan_runbook_step(
            steps.len(),
            "gate_review",
            "处理 gate failure",
            "本轮审计门禁失败,先确认失败原因和策略阈值,避免把失败批次写回 active 池。",
            Vec::new(),
            Vec::new(),
            0,
            true,
            None,
        ));
    }

    push_identity_plan_phase_step(
        &mut steps,
        "quarantine",
        "隔离高风险画像",
        "先隔离重复画像、高风险漂移和明确标记为 quarantine 的资产,降低继续执行任务时的关联风险。",
        actions,
        asset_patches,
        Some("drs --json identity-apply ACTIONS --profile-root ./profiles --execute"),
    );
    push_identity_plan_phase_step(
        &mut steps,
        "repair",
        "修复可恢复画像",
        "把 drift.restore_*、pool.disperse_* 等动作转换成 profile 配置变更,再重新采样验证。",
        actions,
        asset_patches,
        None,
    );
    push_identity_plan_phase_step(
        &mut steps,
        "capacity",
        "补充身份容量",
        "按 capacity 动作补齐 canvas/WebGL/locale/browser persona 等瓶颈维度,提升有效身份数。",
        actions,
        asset_patches,
        None,
    );
    push_identity_plan_phase_step(
        &mut steps,
        "review",
        "人工复核新/缺失画像",
        "复核 new_current、missing_current、review 或 investigate 状态,决定是否入池、停用或重新采样。",
        actions,
        asset_patches,
        None,
    );

    if !asset_patches.is_empty() {
        steps.push(identity_plan_runbook_step(
            steps.len(),
            "manifest_writeback",
            "写回资产状态",
            "将 assetPatches 写入下一轮 profile asset manifest,让调度器按新状态分配任务。",
            Vec::new(),
            asset_patches
                .iter()
                .map(|patch| patch.plan_index)
                .collect::<Vec<_>>(),
            0,
            false,
            Some(
                "drs --json identity-plan INPUT... --asset-manifest profile-assets.json --asset-manifest-out next-profile-assets.json",
            ),
        ));
    }

    if summary.action_count > 0 || summary.asset_patch_count > 0 {
        steps.push(identity_plan_runbook_step(
            steps.len(),
            "resample_verify",
            "重新采样复验",
            "动作执行和资产回写后重新采集 fingerprint snapshots,再次运行 identity-pool / identity-drift / identity-lifecycle。",
            Vec::new(),
            Vec::new(),
            0,
            false,
            Some("drs --json identity --pool --snapshots-out ./snapshots.json"),
        ));
    }

    if steps.is_empty() {
        steps.push(identity_plan_runbook_step(
            0,
            "archive",
            "归档审计结果",
            "当前输入没有可执行动作,保留报告作为下一轮对照基线。",
            Vec::new(),
            Vec::new(),
            0,
            false,
            None,
        ));
    }
    steps
}

fn push_identity_plan_phase_step(
    steps: &mut Vec<IdentityPlanRunbookStep>,
    phase: &str,
    title: &str,
    rationale: &str,
    actions: &[IdentityPlanAction],
    asset_patches: &[IdentityPlanAssetPatch],
    next_command_hint: Option<&str>,
) {
    let action_indexes = actions
        .iter()
        .filter(|action| identity_plan_action_phase(action) == phase)
        .map(|action| action.plan_index)
        .collect::<Vec<_>>();
    let asset_patch_indexes = asset_patches
        .iter()
        .filter(|patch| identity_plan_asset_patch_phase(patch) == phase)
        .map(|patch| patch.plan_index)
        .collect::<Vec<_>>();
    if action_indexes.is_empty() && asset_patch_indexes.is_empty() {
        return;
    }
    let high_priority_action_count = actions
        .iter()
        .filter(|action| {
            action_indexes.contains(&action.plan_index)
                && action
                    .priority
                    .as_deref()
                    .map(|priority| {
                        let priority = priority.to_ascii_lowercase();
                        priority == "high" || priority == "critical"
                    })
                    .unwrap_or(false)
        })
        .count();
    steps.push(identity_plan_runbook_step(
        steps.len(),
        phase,
        title,
        rationale,
        action_indexes,
        asset_patch_indexes,
        high_priority_action_count,
        false,
        next_command_hint,
    ));
}

fn identity_plan_runbook_step(
    step_index: usize,
    phase: &str,
    title: &str,
    rationale: &str,
    action_indexes: Vec<usize>,
    asset_patch_indexes: Vec<usize>,
    high_priority_action_count: usize,
    blocked_by_gate: bool,
    next_command_hint: Option<&str>,
) -> IdentityPlanRunbookStep {
    IdentityPlanRunbookStep {
        step_index,
        phase: phase.to_string(),
        title: title.to_string(),
        rationale: rationale.to_string(),
        action_count: action_indexes.len(),
        asset_patch_count: asset_patch_indexes.len(),
        high_priority_action_count,
        blocked_by_gate,
        action_indexes,
        asset_patch_indexes,
        next_command_hint: next_command_hint.map(ToString::to_string),
    }
}

fn identity_plan_action_phase(action: &IdentityPlanAction) -> &'static str {
    let code = action.action_code.as_str();
    if code.contains("quarantine") {
        "quarantine"
    } else if action.source.as_deref() == Some("capacity") || code.starts_with("capacity.") {
        "capacity"
    } else if code.starts_with("drift.restore")
        || code.starts_with("drift.hide")
        || code.starts_with("drift.rebind")
        || code.starts_with("drift.sync")
        || code.starts_with("pool.disperse")
        || code.starts_with("pool.rotate")
    {
        "repair"
    } else if code.contains("review") || code.contains("investigate") {
        "review"
    } else {
        "repair"
    }
}

fn identity_plan_asset_patch_phase(patch: &IdentityPlanAssetPatch) -> &'static str {
    match patch
        .patch
        .get("nextState")
        .and_then(Value::as_str)
        .unwrap_or("")
    {
        "quarantine" => "quarantine",
        "repair" => "repair",
        "review" | "investigate" | "missing_current" | "new_current" => "review",
        _ => "repair",
    }
}

fn build_identity_plan_dispatch_queue(
    run_id: &str,
    runbook: &[IdentityPlanRunbookStep],
    actions: &[IdentityPlanAction],
    asset_patches: &[IdentityPlanAssetPatch],
) -> IdentityPlanDispatchQueue {
    let mut items = Vec::new();
    for step in runbook {
        for action_index in &step.action_indexes {
            if let Some(action) = actions
                .iter()
                .find(|action| action.plan_index == *action_index)
            {
                items.push(identity_plan_action_dispatch_item(
                    run_id,
                    step,
                    action,
                    items.len(),
                ));
            }
        }
        if step.phase == "manifest_writeback" {
            for patch_index in &step.asset_patch_indexes {
                if let Some(patch) = asset_patches
                    .iter()
                    .find(|patch| patch.plan_index == *patch_index)
                {
                    items.push(identity_plan_patch_dispatch_item(
                        run_id,
                        step,
                        patch,
                        items.len(),
                    ));
                }
            }
        }
        if step.action_indexes.is_empty()
            && step.asset_patch_indexes.is_empty()
            && (step.blocked_by_gate || step.next_command_hint.is_some())
        {
            items.push(identity_plan_command_dispatch_item(
                run_id,
                step,
                items.len(),
            ));
        }
    }

    let mut phases = Vec::new();
    for item in &items {
        if !phases.contains(&item.phase) {
            phases.push(item.phase.clone());
        }
    }
    let action_item_count = items
        .iter()
        .filter(|item| matches!(item.kind, IdentityPlanDispatchKind::Action))
        .count();
    let asset_patch_item_count = items
        .iter()
        .filter(|item| matches!(item.kind, IdentityPlanDispatchKind::AssetPatch))
        .count();
    let command_item_count = items
        .iter()
        .filter(|item| matches!(item.kind, IdentityPlanDispatchKind::Command))
        .count();
    let high_priority_count = items
        .iter()
        .filter(|item| {
            item.priority
                .as_deref()
                .map(|priority| {
                    let priority = priority.to_ascii_lowercase();
                    priority == "high" || priority == "critical"
                })
                .unwrap_or(false)
        })
        .count();
    let blocked_count = items.iter().filter(|item| item.blocked_by_gate).count();

    IdentityPlanDispatchQueue {
        item_count: items.len(),
        action_item_count,
        asset_patch_item_count,
        command_item_count,
        high_priority_count,
        blocked_count,
        phases,
        items,
    }
}

fn identity_plan_action_dispatch_item(
    run_id: &str,
    step: &IdentityPlanRunbookStep,
    action: &IdentityPlanAction,
    dispatch_index: usize,
) -> IdentityPlanDispatchItem {
    let entity_key = action
        .label
        .as_deref()
        .or(action.identity_id.as_deref())
        .unwrap_or("pool");
    let dedupe_key = format!(
        "{}:action:{}:{}:{}",
        step.phase, action.action_code, entity_key, action.plan_index
    );
    IdentityPlanDispatchItem {
        dispatch_index,
        step_index: step.step_index,
        phase: step.phase.clone(),
        kind: IdentityPlanDispatchKind::Action,
        sort_rank: identity_plan_dispatch_sort_rank(&step.phase, action.priority.as_deref()),
        lease_key: format!("{run_id}:{dedupe_key}"),
        dedupe_key,
        blocked_by_gate: step.blocked_by_gate,
        action_index: Some(action.plan_index),
        asset_patch_index: None,
        action_code: Some(action.action_code.clone()),
        priority: action.priority.clone(),
        label: action.label.clone(),
        identity_id: action.identity_id.clone(),
        account_id: None,
        profile_id: None,
        profile_dir: None,
        next_state: None,
        command_hint: step.next_command_hint.clone(),
        reasons: action.reasons.clone(),
        signal_codes: action.signal_codes.clone(),
    }
}

fn identity_plan_patch_dispatch_item(
    run_id: &str,
    step: &IdentityPlanRunbookStep,
    patch: &IdentityPlanAssetPatch,
    dispatch_index: usize,
) -> IdentityPlanDispatchItem {
    let action_code = identity_plan_map_string(&patch.patch, &["actionCode", "code"]);
    let account_id = identity_plan_map_string(&patch.patch, &["accountId", "account_id"]);
    let profile_id = identity_plan_map_string(&patch.patch, &["profileId", "profile_id"]);
    let identity_id = identity_plan_map_string(&patch.patch, &["identityId", "identity_id"]);
    let label = identity_plan_map_string(&patch.patch, &["label", "name"]);
    let profile_dir = identity_plan_map_string(
        &patch.patch,
        &["profileDir", "profilePath", "sourcePath", "destinationPath"],
    );
    let next_state = identity_plan_map_string(&patch.patch, &["nextState", "next_state"]);
    let entity_key = account_id
        .as_deref()
        .or(profile_id.as_deref())
        .or(label.as_deref())
        .or(identity_id.as_deref())
        .or(profile_dir.as_deref())
        .unwrap_or("asset");
    let action_key = action_code.as_deref().unwrap_or("asset_patch");
    let dedupe_key = format!(
        "{}:asset_patch:{}:{}:{}",
        step.phase, action_key, entity_key, patch.plan_index
    );
    IdentityPlanDispatchItem {
        dispatch_index,
        step_index: step.step_index,
        phase: step.phase.clone(),
        kind: IdentityPlanDispatchKind::AssetPatch,
        sort_rank: identity_plan_dispatch_sort_rank(&step.phase, None),
        lease_key: format!("{run_id}:{dedupe_key}"),
        dedupe_key,
        blocked_by_gate: step.blocked_by_gate,
        action_index: None,
        asset_patch_index: Some(patch.plan_index),
        action_code,
        priority: None,
        label,
        identity_id,
        account_id,
        profile_id,
        profile_dir,
        next_state,
        command_hint: step.next_command_hint.clone(),
        reasons: Vec::new(),
        signal_codes: Vec::new(),
    }
}

fn identity_plan_command_dispatch_item(
    run_id: &str,
    step: &IdentityPlanRunbookStep,
    dispatch_index: usize,
) -> IdentityPlanDispatchItem {
    let dedupe_key = format!("{}:command:{}", step.phase, step.step_index);
    IdentityPlanDispatchItem {
        dispatch_index,
        step_index: step.step_index,
        phase: step.phase.clone(),
        kind: IdentityPlanDispatchKind::Command,
        sort_rank: identity_plan_dispatch_sort_rank(&step.phase, None),
        lease_key: format!("{run_id}:{dedupe_key}"),
        dedupe_key,
        blocked_by_gate: step.blocked_by_gate,
        action_index: None,
        asset_patch_index: None,
        action_code: None,
        priority: None,
        label: None,
        identity_id: None,
        account_id: None,
        profile_id: None,
        profile_dir: None,
        next_state: None,
        command_hint: step.next_command_hint.clone(),
        reasons: Vec::new(),
        signal_codes: Vec::new(),
    }
}

fn identity_plan_dispatch_sort_rank(phase: &str, priority: Option<&str>) -> u16 {
    let base = match phase {
        "gate_review" => 0,
        "quarantine" => 100,
        "repair" => 200,
        "capacity" => 300,
        "review" => 400,
        "manifest_writeback" => 500,
        "resample_verify" => 600,
        "archive" => 900,
        _ => 800,
    };
    let priority_adjust = match priority.map(str::to_ascii_lowercase).as_deref() {
        Some("critical") => 0,
        Some("high") => 10,
        Some("medium") => 20,
        Some("low") => 30,
        _ => 40,
    };
    base + priority_adjust
}

async fn write_identity_plan_json(
    report: &IdentityPlanReport,
    path: Option<&Path>,
) -> Result<Option<IdentityPlanOutput>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(report)?;
    tokio::fs::write(path, &bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(Some(IdentityPlanOutput {
        path: path.display().to_string(),
        format: "json_report".to_string(),
        bytes: bytes.len(),
    }))
}

async fn write_identity_plan_dispatch(
    report: &IdentityPlanReport,
    path: Option<&Path>,
    append: bool,
) -> Result<Option<IdentityPlanDispatchOut>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if append {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        for item in &report.dispatch_queue.items {
            let line = json!({
                "runId": report.run_id,
                "generatedAtUnixSeconds": report.generated_at_unix_seconds,
                "dispatch": item,
            });
            file.write_all(serde_json::to_string(&line)?.as_bytes())
                .await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
    } else {
        let value = json!({
            "scope": "identity_dispatch_queue",
            "runId": report.run_id,
            "generatedAtUnixSeconds": report.generated_at_unix_seconds,
            "queue": report.dispatch_queue,
        });
        tokio::fs::write(path, serde_json::to_vec_pretty(&value)?)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(Some(IdentityPlanDispatchOut {
        path: path.display().to_string(),
        append,
        count: report.dispatch_queue.item_count,
        format: if append {
            "ndjson_dispatch_items".to_string()
        } else {
            "json_report".to_string()
        },
    }))
}

async fn write_identity_plan_asset_manifest(
    report: &IdentityPlanReport,
    source: Option<&Path>,
    destination: Option<&Path>,
) -> Result<Option<IdentityPlanAssetManifestOut>> {
    let (Some(source), Some(destination)) = (source, destination) else {
        return Ok(None);
    };
    let text = tokio::fs::read_to_string(source)
        .await
        .with_context(|| format!("failed to read asset manifest {}", source.display()))?;
    let mut manifest = serde_json::from_str::<Value>(&text)
        .with_context(|| format!("failed to parse asset manifest {}", source.display()))?;
    let asset_count = identity_plan_manifest_entries(&manifest)?.len();
    let (updated_count, unmatched_patch_count) =
        apply_identity_plan_manifest_patches(&mut manifest, report)?;
    let state_counts = count_identity_plan_manifest_states(&manifest)?;

    if let Some(parent) = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    tokio::fs::write(destination, &bytes)
        .await
        .with_context(|| format!("failed to write {}", destination.display()))?;

    Ok(Some(IdentityPlanAssetManifestOut {
        source_path: source.display().to_string(),
        path: destination.display().to_string(),
        format: "profile_assets_json".to_string(),
        asset_count,
        updated_count,
        unchanged_count: asset_count.saturating_sub(updated_count),
        unmatched_patch_count,
        state_counts,
        bytes: bytes.len(),
    }))
}

fn identity_plan_manifest_entries(value: &Value) -> Result<&Vec<Value>> {
    match value {
        Value::Array(items) => Ok(items),
        Value::Object(map) => {
            for key in ["profileAssets", "profile_assets", "assets", "profiles"] {
                if let Some(items) = map.get(key).and_then(Value::as_array) {
                    return Ok(items);
                }
            }
            bail!("asset manifest object must contain profileAssets, assets, or profiles array")
        }
        _ => bail!("asset manifest must be an array or object"),
    }
}

fn identity_plan_manifest_entries_mut(value: &mut Value) -> Result<&mut Vec<Value>> {
    match value {
        Value::Array(items) => Ok(items),
        Value::Object(map) => {
            let key = ["profileAssets", "profile_assets", "assets", "profiles"]
                .into_iter()
                .find(|key| map.get(*key).and_then(Value::as_array).is_some());
            if let Some(key) = key {
                return Ok(map
                    .get_mut(key)
                    .and_then(Value::as_array_mut)
                    .expect("asset manifest key was checked as an array"));
            }
            bail!("asset manifest object must contain profileAssets, assets, or profiles array")
        }
        _ => bail!("asset manifest must be an array or object"),
    }
}

fn apply_identity_plan_manifest_patches(
    manifest: &mut Value,
    report: &IdentityPlanReport,
) -> Result<(usize, usize)> {
    let entries = identity_plan_manifest_entries_mut(manifest)?;
    let mut updated_indexes = BTreeSet::new();
    let mut unmatched_patch_count = 0usize;

    for patch in &report.asset_patches {
        let patch_keys = identity_plan_patch_match_keys(&patch.patch);
        if patch_keys.is_empty() {
            unmatched_patch_count += 1;
            continue;
        }
        let mut matched_index = None;
        for (index, entry) in entries.iter().enumerate() {
            let entry_keys = identity_plan_manifest_entry_match_keys(entry);
            if !entry_keys.is_disjoint(&patch_keys) {
                matched_index = Some(index);
                break;
            }
        }

        if let Some(index) = matched_index {
            if let Some(entry) = entries.get_mut(index) {
                apply_identity_plan_patch_to_manifest_entry(
                    entry,
                    patch,
                    &report.run_id,
                    report.generated_at_unix_seconds,
                );
                updated_indexes.insert(index);
            }
        } else {
            unmatched_patch_count += 1;
        }
    }

    Ok((updated_indexes.len(), unmatched_patch_count))
}

fn apply_identity_plan_patch_to_manifest_entry(
    entry: &mut Value,
    patch: &IdentityPlanAssetPatch,
    run_id: &str,
    generated_at_unix_seconds: u64,
) {
    let Value::Object(map) = entry else {
        return;
    };
    if let Some(next_state) = patch.patch.get("nextState").cloned() {
        if let Some(previous_state) = map.get("state").cloned() {
            map.insert("lastIdentityPlanPreviousState".to_string(), previous_state);
        }
        map.insert("state".to_string(), next_state);
    }
    if identity_plan_patch_status(&patch.patch).as_deref() == Some("applied") {
        if let Some(destination) = identity_plan_map_string(&patch.patch, &["destinationPath"]) {
            map.insert("profileDir".to_string(), Value::String(destination));
        }
    }
    if let Some(action_code) = identity_plan_map_string(&patch.patch, &["actionCode", "code"]) {
        map.insert(
            "lastIdentityPlanActionCode".to_string(),
            Value::String(action_code),
        );
    }
    map.insert(
        "lastIdentityPlanRunId".to_string(),
        Value::String(run_id.to_string()),
    );
    map.insert(
        "lastIdentityPlanUpdatedAtUnixSeconds".to_string(),
        json!(generated_at_unix_seconds),
    );
}

fn identity_plan_patch_status(patch: &BTreeMap<String, Value>) -> Option<String> {
    identity_plan_map_string(patch, &["status"]).map(|status| status.to_ascii_lowercase())
}

fn identity_plan_manifest_entry_match_keys(entry: &Value) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    push_identity_plan_value_key(&mut keys, "accountId", entry.get("accountId"));
    push_identity_plan_value_key(&mut keys, "accountId", entry.get("account_id"));
    push_identity_plan_value_key(&mut keys, "profileId", entry.get("profileId"));
    push_identity_plan_value_key(&mut keys, "profileId", entry.get("profile_id"));
    push_identity_plan_value_key(&mut keys, "identityId", entry.get("identityId"));
    push_identity_plan_value_key(&mut keys, "identityId", entry.get("identity_id"));
    push_identity_plan_value_key(&mut keys, "label", entry.get("label"));
    push_identity_plan_value_key(&mut keys, "label", entry.get("name"));
    push_identity_plan_value_key(&mut keys, "profileDir", entry.get("profileDir"));
    push_identity_plan_value_key(&mut keys, "profileDir", entry.get("profilePath"));
    push_identity_plan_value_key(&mut keys, "profileDir", entry.get("path"));
    push_identity_plan_value_key(&mut keys, "profileDir", entry.get("userDataDir"));
    push_identity_plan_value_key(&mut keys, "profileDir", entry.get("profile_dir"));
    push_identity_plan_value_key(&mut keys, "profileDir", entry.get("profile_path"));
    push_identity_plan_value_key(&mut keys, "profileDir", entry.get("user_data_dir"));
    keys
}

fn identity_plan_patch_match_keys(patch: &BTreeMap<String, Value>) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for (normalized, aliases) in [
        ("accountId", &["accountId", "account_id"][..]),
        ("profileId", &["profileId", "profile_id"][..]),
        ("identityId", &["identityId", "identity_id"][..]),
        ("label", &["label", "name"][..]),
        (
            "profileDir",
            &[
                "profileDir",
                "profilePath",
                "path",
                "userDataDir",
                "profile_dir",
                "profile_path",
                "user_data_dir",
                "sourcePath",
                "destinationPath",
            ][..],
        ),
    ] {
        for alias in aliases {
            push_identity_plan_value_key(&mut keys, normalized, patch.get(*alias));
        }
    }
    keys
}

fn push_identity_plan_value_key(
    keys: &mut BTreeSet<String>,
    normalized_key: &str,
    value: Option<&Value>,
) {
    let Some(value) = value.and_then(label_value_to_string) else {
        return;
    };
    keys.insert(format!("{normalized_key}:{value}"));
}

fn identity_plan_map_string(map: &BTreeMap<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = map.get(*key).and_then(label_value_to_string) {
            return Some(value);
        }
    }
    None
}

fn count_identity_plan_manifest_states(value: &Value) -> Result<BTreeMap<String, usize>> {
    let mut counts = BTreeMap::new();
    for entry in identity_plan_manifest_entries(value)? {
        let state = entry
            .get("state")
            .and_then(label_value_to_string)
            .unwrap_or_else(|| "unknown".to_string());
        *counts.entry(state).or_insert(0) += 1;
    }
    Ok(counts)
}

fn count_identity_dispatch_manifest_states(value: &Value) -> Result<BTreeMap<String, usize>> {
    let mut counts = BTreeMap::new();
    for entry in identity_plan_manifest_entries(value)? {
        let state = entry
            .get("dispatchState")
            .and_then(label_value_to_string)
            .unwrap_or_else(|| "unknown".to_string());
        *counts.entry(state).or_insert(0) += 1;
    }
    Ok(counts)
}

fn count_identity_asset_runtime_lease_states(value: &Value) -> Result<BTreeMap<String, usize>> {
    let mut counts = BTreeMap::new();
    for entry in identity_plan_manifest_entries(value)? {
        let state = entry
            .get("runtimeLeaseState")
            .and_then(label_value_to_string)
            .unwrap_or_else(|| "none".to_string());
        *counts.entry(state).or_insert(0) += 1;
    }
    Ok(counts)
}

async fn write_identity_plan_html(
    report: &IdentityPlanReport,
    path: Option<&Path>,
) -> Result<Option<IdentityPlanOutput>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let html = render_identity_plan_html(report);
    tokio::fs::write(path, html.as_bytes())
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(Some(IdentityPlanOutput {
        path: path.display().to_string(),
        format: "html".to_string(),
        bytes: html.len(),
    }))
}

fn render_identity_plan_html(report: &IdentityPlanReport) -> String {
    let summary = &report.summary;
    let mut html = String::new();
    html.push_str("<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\">");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    html.push_str("<title>");
    html.push_str(&escape_html(&report.title));
    html.push_str("</title><style>");
    html.push_str("body{font-family:-apple-system,BlinkMacSystemFont,\"Segoe UI\",sans-serif;margin:0;background:#f6f7f9;color:#15171a}main{max-width:1180px;margin:0 auto;padding:28px}h1{font-size:28px;margin:0 0 6px}h2{font-size:18px;margin:28px 0 10px}.muted{color:#65707f}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(155px,1fr));gap:10px;margin:18px 0}.metric{background:white;border:1px solid #dde2e8;border-radius:8px;padding:12px}.metric b{display:block;font-size:24px;margin-top:5px}.warn{color:#9f4b00}.bad{color:#a52727}.ok{color:#176b3a}table{width:100%;border-collapse:collapse;background:white;border:1px solid #dde2e8;border-radius:8px;overflow:hidden}th,td{padding:9px 10px;border-bottom:1px solid #edf0f3;text-align:left;font-size:13px;vertical-align:top}th{background:#eef2f6;color:#303844}tr:last-child td{border-bottom:0}.pill{display:inline-block;border:1px solid #cbd3dc;border-radius:999px;padding:2px 8px;font-size:12px;background:#fff}ul{background:white;border:1px solid #dde2e8;border-radius:8px;padding:12px 18px}li{margin:6px 0}code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:12px}.small{font-size:12px}");
    html.push_str("</style></head><body><main>");
    html.push_str("<h1>");
    html.push_str(&escape_html(&report.title));
    html.push_str("</h1><div class=\"muted\">run ");
    html.push_str(&escape_html(&report.run_id));
    html.push_str(" · ");
    html.push_str(&report.generated_at_unix_seconds.to_string());
    html.push_str("</div>");

    html.push_str("<section class=\"grid\">");
    identity_plan_metric(&mut html, "输入文件", summary.input_count);
    identity_plan_metric(&mut html, "动作", summary.action_count);
    identity_plan_metric(&mut html, "高优先级", summary.high_priority_action_count);
    identity_plan_metric(&mut html, "隔离动作", summary.quarantine_action_count);
    identity_plan_metric(&mut html, "容量动作", summary.capacity_action_count);
    identity_plan_metric(&mut html, "资产 Patch", summary.asset_patch_count);
    identity_plan_metric(&mut html, "Gate 失败", summary.gate_failure_count);
    identity_plan_metric(
        &mut html,
        "Apply 异常",
        summary.failed_operation_count + summary.unresolved_operation_count,
    );
    html.push_str("</section>");

    html.push_str("<h2>建议</h2><ul>");
    for recommendation in &report.recommendations {
        html.push_str("<li>");
        html.push_str(&escape_html(recommendation));
        html.push_str("</li>");
    }
    html.push_str("</ul>");

    html.push_str("<h2>执行 Runbook</h2><table><thead><tr><th>#</th><th>阶段</th><th>标题</th><th>动作</th><th>Patch</th><th>说明</th></tr></thead><tbody>");
    for step in &report.execution_runbook {
        html.push_str("<tr><td>");
        html.push_str(&step.step_index.to_string());
        html.push_str("</td><td><code>");
        html.push_str(&escape_html(&step.phase));
        html.push_str("</code></td><td>");
        html.push_str(&escape_html(&step.title));
        html.push_str("</td><td>");
        html.push_str(&step.action_count.to_string());
        html.push_str("</td><td>");
        html.push_str(&step.asset_patch_count.to_string());
        html.push_str("</td><td class=\"small\">");
        html.push_str(&escape_html(&step.rationale));
        if let Some(command) = &step.next_command_hint {
            html.push_str("<br><code>");
            html.push_str(&escape_html(command));
            html.push_str("</code>");
        }
        html.push_str("</td></tr>");
    }
    html.push_str("</tbody></table>");

    html.push_str("<h2>Dispatch Queue</h2><table><thead><tr><th>#</th><th>阶段</th><th>类型</th><th>排序</th><th>目标</th><th>动作/状态</th><th>租约</th></tr></thead><tbody>");
    for item in report.dispatch_queue.items.iter().take(200) {
        html.push_str("<tr><td>");
        html.push_str(&item.dispatch_index.to_string());
        html.push_str("</td><td><code>");
        html.push_str(&escape_html(&item.phase));
        html.push_str("</code></td><td>");
        html.push_str(&escape_html(&format!("{:?}", item.kind)));
        html.push_str("</td><td>");
        html.push_str(&item.sort_rank.to_string());
        html.push_str("</td><td>");
        html.push_str(&escape_html(
            item.account_id
                .as_deref()
                .or(item.profile_id.as_deref())
                .or(item.label.as_deref())
                .or(item.identity_id.as_deref())
                .or(item.profile_dir.as_deref())
                .unwrap_or("-"),
        ));
        html.push_str("</td><td><code>");
        html.push_str(&escape_html(
            item.action_code
                .as_deref()
                .or(item.next_state.as_deref())
                .unwrap_or("-"),
        ));
        html.push_str("</code></td><td><code>");
        html.push_str(&escape_html(&item.lease_key));
        html.push_str("</code></td></tr>");
    }
    if report.dispatch_queue.items.len() > 200 {
        html.push_str(
            "<tr><td colspan=\"7\" class=\"muted\">仅显示前 200 条 dispatch item</td></tr>",
        );
    }
    html.push_str("</tbody></table>");

    if let Some(manifest) = &report.asset_manifest_out {
        html.push_str("<h2>资产 Manifest 回写</h2><table><thead><tr><th>来源</th><th>输出</th><th>资产</th><th>已更新</th><th>未命中 Patch</th><th>状态</th></tr></thead><tbody><tr><td><code>");
        html.push_str(&escape_html(&manifest.source_path));
        html.push_str("</code></td><td><code>");
        html.push_str(&escape_html(&manifest.path));
        html.push_str("</code></td><td>");
        html.push_str(&manifest.asset_count.to_string());
        html.push_str("</td><td>");
        html.push_str(&manifest.updated_count.to_string());
        html.push_str("</td><td>");
        html.push_str(&manifest.unmatched_patch_count.to_string());
        html.push_str("</td><td>");
        let state_text = manifest
            .state_counts
            .iter()
            .map(|(state, count)| format!("{state}:{count}"))
            .collect::<Vec<_>>()
            .join(", ");
        html.push_str(&escape_html(&state_text));
        html.push_str("</td></tr></tbody></table>");
    }

    html.push_str("<h2>输入</h2><table><thead><tr><th>#</th><th>路径</th><th>格式</th><th>scope</th><th>动作</th><th>Patch</th><th>Gate</th></tr></thead><tbody>");
    for input in &report.inputs {
        html.push_str("<tr><td>");
        html.push_str(&input.input_index.to_string());
        html.push_str("</td><td><code>");
        html.push_str(&escape_html(&input.path));
        html.push_str("</code></td><td>");
        html.push_str(&escape_html(&input.format));
        html.push_str("</td><td>");
        html.push_str(&escape_html(input.scope.as_deref().unwrap_or("-")));
        html.push_str("</td><td>");
        html.push_str(&input.action_count.to_string());
        html.push_str("</td><td>");
        html.push_str(&input.asset_patch_count.to_string());
        html.push_str("</td><td>");
        html.push_str(if input.gate_passed == Some(false) {
            "<span class=\"bad\">failed</span>"
        } else {
            "<span class=\"ok\">ok/none</span>"
        });
        html.push_str("</td></tr>");
    }
    html.push_str("</tbody></table>");

    html.push_str("<h2>动作队列</h2><table><thead><tr><th>#</th><th>动作</th><th>来源</th><th>优先级</th><th>标签</th><th>状态/目标</th><th>详情</th></tr></thead><tbody>");
    for action in report.actions.iter().take(200) {
        html.push_str("<tr><td>");
        html.push_str(&action.plan_index.to_string());
        html.push_str("</td><td><code>");
        html.push_str(&escape_html(&action.action_code));
        html.push_str("</code></td><td>");
        html.push_str(&escape_html(action.source.as_deref().unwrap_or("-")));
        html.push_str("</td><td>");
        html.push_str(&escape_html(action.priority.as_deref().unwrap_or("-")));
        html.push_str("</td><td>");
        html.push_str(&escape_html(
            action
                .label
                .as_deref()
                .unwrap_or(action.identity_id.as_deref().unwrap_or("-")),
        ));
        html.push_str("</td><td>");
        html.push_str(&escape_html(
            action
                .state
                .as_deref()
                .unwrap_or(action.target.as_deref().unwrap_or("-")),
        ));
        html.push_str("</td><td class=\"small\">");
        html.push_str(&escape_html(
            action
                .detail
                .as_deref()
                .unwrap_or(action.title.as_deref().unwrap_or("-")),
        ));
        html.push_str("</td></tr>");
    }
    if report.actions.len() > 200 {
        html.push_str("<tr><td colspan=\"7\" class=\"muted\">仅显示前 200 条动作</td></tr>");
    }
    html.push_str("</tbody></table>");

    html.push_str("<h2>资产状态 Patch</h2><table><thead><tr><th>#</th><th>账号/Profile</th><th>动作</th><th>状态</th><th>路径</th></tr></thead><tbody>");
    for patch in report.asset_patches.iter().take(200) {
        let label = patch
            .patch
            .get("accountId")
            .or_else(|| patch.patch.get("label"))
            .or_else(|| patch.patch.get("profileId"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        let action_code = patch
            .patch
            .get("actionCode")
            .and_then(Value::as_str)
            .unwrap_or("-");
        let next_state = patch
            .patch
            .get("nextState")
            .and_then(Value::as_str)
            .unwrap_or("-");
        let profile_dir = patch
            .patch
            .get("profileDir")
            .or_else(|| patch.patch.get("sourcePath"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        html.push_str("<tr><td>");
        html.push_str(&patch.plan_index.to_string());
        html.push_str("</td><td>");
        html.push_str(&escape_html(label));
        html.push_str("</td><td><code>");
        html.push_str(&escape_html(action_code));
        html.push_str("</code></td><td><span class=\"pill\">");
        html.push_str(&escape_html(next_state));
        html.push_str("</span></td><td><code>");
        html.push_str(&escape_html(profile_dir));
        html.push_str("</code></td></tr>");
    }
    if report.asset_patches.len() > 200 {
        html.push_str("<tr><td colspan=\"5\" class=\"muted\">仅显示前 200 条资产 patch</td></tr>");
    }
    html.push_str("</tbody></table>");
    html.push_str("</main></body></html>");
    html
}

fn identity_plan_metric(html: &mut String, label: &str, value: usize) {
    html.push_str("<div class=\"metric\"><span class=\"muted\">");
    html.push_str(&escape_html(label));
    html.push_str("</span><b>");
    html.push_str(&value.to_string());
    html.push_str("</b></div>");
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

pub async fn apply_identity_actions(
    actions_path: &Path,
    profile_root: Option<&Path>,
    profile_map: Option<&Path>,
    quarantine_dir: Option<&Path>,
    execute: bool,
    journal_out: Option<&Path>,
    append_journal: bool,
    asset_state_out: Option<&Path>,
    append_asset_state: bool,
) -> Result<JsonResponse> {
    let actions = read_identity_actions(actions_path).await?;
    let bindings = read_profile_bindings(profile_map, profile_root).await?;
    let effective_quarantine_dir = quarantine_dir
        .map(Path::to_path_buf)
        .or_else(|| profile_root.map(|root| root.join("_quarantine")));
    let generated_at = unix_seconds();
    let run_id = format!("apply_{}_{}", generated_at, std::process::id());
    let mut operations = Vec::new();

    for action in &actions {
        for target in apply_targets_for_action(action) {
            let operation = build_apply_operation(
                action,
                &target,
                &bindings,
                profile_root,
                effective_quarantine_dir.as_deref(),
                execute,
            )
            .await;
            operations.push(operation);
        }
    }
    for (index, operation) in operations.iter_mut().enumerate() {
        operation.operation_index = index;
    }
    let asset_patches = build_apply_asset_patches(&operations);

    let mut report = IdentityApplyReport {
        scope: "identity_apply".to_string(),
        path: actions_path.display().to_string(),
        execute,
        dry_run: !execute,
        profile_root: profile_root.map(|path| path.display().to_string()),
        profile_map: profile_map.map(|path| path.display().to_string()),
        quarantine_dir: effective_quarantine_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        run_id,
        generated_at_unix_seconds: generated_at,
        action_count: actions.len(),
        operation_count: operations.len(),
        executable_count: operations.iter().filter(|op| op.executable).count(),
        planned_count: operations
            .iter()
            .filter(|op| op.status == IdentityApplyStatus::Planned)
            .count(),
        applied_count: operations
            .iter()
            .filter(|op| op.status == IdentityApplyStatus::Applied)
            .count(),
        skipped_count: operations
            .iter()
            .filter(|op| op.status == IdentityApplyStatus::Skipped)
            .count(),
        unresolved_count: operations
            .iter()
            .filter(|op| op.status == IdentityApplyStatus::Unresolved)
            .count(),
        failed_count: operations
            .iter()
            .filter(|op| op.status == IdentityApplyStatus::Failed)
            .count(),
        operations,
        asset_patch_count: asset_patches.len(),
        asset_patches,
        asset_state_out: None,
        journal_out: None,
    };
    report.asset_state_out =
        write_apply_asset_state(&report, asset_state_out, append_asset_state).await?;
    report.journal_out = write_apply_journal(&report, journal_out, append_journal).await?;

    Ok(JsonResponse::ok(report))
}

async fn read_identity_actions(path: &Path) -> Result<Vec<IdentityApplyAction>> {
    let text = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read identity actions from {}", path.display()))?;
    parse_identity_actions(&text)
        .with_context(|| format!("failed to parse identity actions from {}", path.display()))
}

fn parse_identity_actions(text: &str) -> Result<Vec<IdentityApplyAction>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("identity action file is empty");
    }

    let mut actions = match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => identity_actions_from_value(&value)?,
        Err(_) => parse_ndjson_identity_actions(trimmed)?,
    };
    for (index, action) in actions.iter_mut().enumerate() {
        if action.action_index == usize::MAX {
            action.action_index = index;
        }
    }
    if actions.is_empty() {
        bail!("no identity actions found");
    }
    Ok(actions)
}

fn parse_ndjson_identity_actions(text: &str) -> Result<Vec<IdentityApplyAction>> {
    let mut actions = Vec::new();
    for (line_index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value = serde_json::from_str::<Value>(line)
            .with_context(|| format!("invalid JSON at line {}", line_index + 1))?;
        let mut parsed = identity_actions_from_value(&value)
            .with_context(|| format!("invalid identity action at line {}", line_index + 1))?;
        actions.append(&mut parsed);
    }
    Ok(actions)
}

fn identity_actions_from_value(value: &Value) -> Result<Vec<IdentityApplyAction>> {
    if let Some(data) = value.get("data") {
        return identity_actions_from_value(data);
    }
    if let Some(queue) = value.get("actionQueue") {
        return identity_actions_from_value(queue);
    }
    if let Some(actions) = value.get("actions").and_then(Value::as_array) {
        return actions
            .iter()
            .enumerate()
            .map(|(index, action)| identity_apply_action_from_value(action, index))
            .collect();
    }
    if let Some(actions) = value.as_array() {
        return actions
            .iter()
            .enumerate()
            .map(|(index, action)| identity_apply_action_from_value(action, index))
            .collect();
    }
    if value.get("actionCode").is_some() || value.get("code").is_some() {
        return Ok(vec![identity_apply_action_from_value(value, 0)?]);
    }
    bail!("expected actionQueue.actions, actions array, or a single action object")
}

fn identity_apply_action_from_value(
    value: &Value,
    fallback_index: usize,
) -> Result<IdentityApplyAction> {
    let action_code = value
        .get("actionCode")
        .or_else(|| value.get("code"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .context("identity action is missing actionCode")?
        .to_string();
    let action_index = value
        .get("actionIndex")
        .or_else(|| value.get("index"))
        .and_then(Value::as_u64)
        .map(|index| index as usize)
        .unwrap_or(fallback_index);
    let priority = value
        .get("priority")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let mut labels = string_values_for_keys(value, &["label", "labels"]);
    let mut identity_ids = string_values_for_keys(
        value,
        &[
            "identityId",
            "identityIds",
            "beforeId",
            "afterId",
            "candidateId",
            "candidateIds",
        ],
    );
    dedup_strings(&mut labels);
    dedup_strings(&mut identity_ids);

    Ok(IdentityApplyAction {
        action_index,
        action_code,
        priority,
        labels,
        identity_ids,
    })
}

fn string_values_for_keys(value: &Value, keys: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for key in keys {
        let Some(value) = value.get(*key) else {
            continue;
        };
        match value {
            Value::String(text) => push_non_empty(&mut out, text),
            Value::Array(items) => {
                for item in items {
                    if let Some(text) = item.as_str() {
                        push_non_empty(&mut out, text);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn push_non_empty(out: &mut Vec<String>, text: &str) {
    let text = text.trim();
    if !text.is_empty() {
        out.push(text.to_string());
    }
}

fn dedup_strings(values: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

async fn read_profile_bindings(
    profile_map: Option<&Path>,
    profile_root: Option<&Path>,
) -> Result<ProfileBindings> {
    let Some(path) = profile_map else {
        return Ok(ProfileBindings::default());
    };
    let text = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read profile map {}", path.display()))?;
    let value = serde_json::from_str::<Value>(&text)
        .with_context(|| format!("failed to parse profile map {}", path.display()))?;
    profile_bindings_from_value(&value, profile_root)
}

fn profile_bindings_from_value(
    value: &Value,
    profile_root: Option<&Path>,
) -> Result<ProfileBindings> {
    let mut bindings = ProfileBindings::default();
    match value {
        Value::Object(map) => {
            if let Some(items) = ["profileAssets", "profile_assets", "assets", "profiles"]
                .iter()
                .find_map(|key| map.get(*key).and_then(Value::as_array))
            {
                for item in items {
                    add_profile_asset_binding(&mut bindings, item, profile_root, None);
                }
                return Ok(bindings);
            }
            for (key, value) in map {
                if let Some(binding) = profile_binding_from_value(value, profile_root, Some(key)) {
                    let mut keys = vec![key.clone()];
                    keys.extend(profile_keys_from_value(value));
                    dedup_strings(&mut keys);
                    for key in keys {
                        bindings.by_key.insert(key, binding.clone());
                    }
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                add_profile_asset_binding(&mut bindings, item, profile_root, None);
            }
        }
        _ => bail!("profile map must be an object or array"),
    }
    Ok(bindings)
}

fn add_profile_asset_binding(
    bindings: &mut ProfileBindings,
    value: &Value,
    profile_root: Option<&Path>,
    fallback_label: Option<&str>,
) {
    let Some(binding) = profile_binding_from_value(value, profile_root, fallback_label) else {
        return;
    };
    let mut keys = profile_keys_from_value(value);
    if let Some(label) = fallback_label {
        push_non_empty(&mut keys, label);
    }
    if let Some(asset) = &binding.asset {
        for key in [
            asset.account_id.as_deref(),
            asset.profile_id.as_deref(),
            asset.identity_id.as_deref(),
            asset.label.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            push_non_empty(&mut keys, key);
        }
    }
    dedup_strings(&mut keys);
    for key in keys {
        bindings.by_key.insert(key, binding.clone());
    }
}

fn profile_binding_from_value(
    value: &Value,
    profile_root: Option<&Path>,
    fallback_label: Option<&str>,
) -> Option<ProfileBinding> {
    let path = profile_path_from_value(value, profile_root)?;
    let asset = profile_asset_from_value(value, &path, fallback_label);
    Some(ProfileBinding { path, asset })
}

fn profile_path_from_value(value: &Value, profile_root: Option<&Path>) -> Option<PathBuf> {
    let raw = match value {
        Value::String(path) => Some(path.as_str()),
        Value::Object(map) => [
            "profileDir",
            "profilePath",
            "path",
            "userDataDir",
            "profile_dir",
            "profile_path",
            "user_data_dir",
        ]
        .iter()
        .find_map(|key| map.get(*key).and_then(Value::as_str)),
        _ => None,
    }?;
    let path = PathBuf::from(raw);
    Some(if path.is_relative() {
        match profile_root {
            Some(root) => root.join(path),
            None => path,
        }
    } else {
        path
    })
}

fn profile_asset_from_value(
    value: &Value,
    path: &Path,
    fallback_label: Option<&str>,
) -> Option<IdentityProfileAsset> {
    let Value::Object(_) = value else {
        return None;
    };
    let label = string_field_for_keys(value, &["label", "name", "key"])
        .or_else(|| fallback_label.map(ToString::to_string));
    Some(IdentityProfileAsset {
        account_id: string_field_for_keys(value, &["accountId", "account_id"]),
        profile_id: string_field_for_keys(value, &["profileId", "profile_id"]),
        identity_id: string_field_for_keys(value, &["identityId", "identity_id"]),
        label,
        profile_dir: path.display().to_string(),
        proxy_id: string_field_for_keys(value, &["proxyId", "proxy_id"]),
        fingerprint_seed: string_field_for_keys(
            value,
            &["fingerprintSeed", "fingerprint_seed", "seed"],
        ),
        state: string_field_for_keys(
            value,
            &["state", "status", "lifecycleState", "lifecycle_state"],
        ),
    })
}

fn profile_keys_from_value(value: &Value) -> Vec<String> {
    let mut keys = Vec::new();
    for key in [
        "accountId",
        "account_id",
        "profileId",
        "profile_id",
        "identityKey",
        "identity_key",
        "identityId",
        "identity_id",
        "label",
        "id",
        "name",
        "key",
    ] {
        if let Some(value) = value.get(key).and_then(label_value_to_string) {
            keys.push(value);
        }
    }
    dedup_strings(&mut keys);
    keys
}

fn string_field_for_keys(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(label_value_to_string) {
            return Some(text);
        }
    }
    None
}

fn apply_targets_for_action(action: &IdentityApplyAction) -> Vec<IdentityApplyTarget> {
    if !action.labels.is_empty() {
        return action
            .labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                let mut selectors = vec![label.clone()];
                selectors.extend(action.identity_ids.iter().cloned());
                dedup_strings(&mut selectors);
                IdentityApplyTarget {
                    target_index: index,
                    label: Some(label.clone()),
                    identity_id: action.identity_ids.first().cloned(),
                    selectors,
                }
            })
            .collect();
    }
    if !action.identity_ids.is_empty() {
        return action
            .identity_ids
            .iter()
            .enumerate()
            .map(|(index, identity_id)| IdentityApplyTarget {
                target_index: index,
                label: None,
                identity_id: Some(identity_id.clone()),
                selectors: vec![identity_id.clone()],
            })
            .collect();
    }
    vec![IdentityApplyTarget {
        target_index: 0,
        label: None,
        identity_id: None,
        selectors: Vec::new(),
    }]
}

async fn build_apply_operation(
    action: &IdentityApplyAction,
    target: &IdentityApplyTarget,
    bindings: &ProfileBindings,
    profile_root: Option<&Path>,
    quarantine_dir: Option<&Path>,
    execute: bool,
) -> IdentityApplyOperation {
    let intent = apply_intent_for_action(&action.action_code);
    let candidates = candidate_profile_paths(target, bindings, profile_root);
    let selected = choose_profile_source(&candidates).await;
    if intent != IdentityApplyIntent::QuarantineProfile {
        let (source_path, source_exists, asset) = selected
            .map(|(candidate, exists)| {
                let asset = candidate.asset;
                (Some(candidate.path), Some(exists), asset)
            })
            .unwrap_or((None, None, None));
        return IdentityApplyOperation {
            operation_index: 0,
            action_index: action.action_index,
            target_index: target.target_index,
            action_code: action.action_code.clone(),
            intent,
            priority: action.priority.clone(),
            label: target.label.clone(),
            identity_id: target.identity_id.clone(),
            selectors: target.selectors.clone(),
            executable: false,
            execute,
            status: IdentityApplyStatus::Skipped,
            reason: "action_not_file_mutating".to_string(),
            source_path: source_path.map(|path| path.display().to_string()),
            destination_path: None,
            source_exists,
            asset,
        };
    }

    let Some((source, source_exists)) = selected else {
        return unresolved_apply_operation(
            action,
            target,
            intent,
            execute,
            "profile_path_unresolved",
            None,
            None,
            None,
            None,
        );
    };
    let source_path = source.path;
    let asset = source.asset;
    if !source_exists {
        return unresolved_apply_operation(
            action,
            target,
            intent,
            execute,
            "profile_path_missing",
            Some(source_path),
            None,
            Some(false),
            asset,
        );
    }
    let Some(quarantine_dir) = quarantine_dir else {
        return unresolved_apply_operation(
            action,
            target,
            intent,
            execute,
            "quarantine_dir_unresolved",
            Some(source_path),
            None,
            Some(true),
            asset,
        );
    };

    let destination = quarantine_dir.join(
        source_path
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("profile")),
    );
    if !execute {
        return IdentityApplyOperation {
            operation_index: 0,
            action_index: action.action_index,
            target_index: target.target_index,
            action_code: action.action_code.clone(),
            intent,
            priority: action.priority.clone(),
            label: target.label.clone(),
            identity_id: target.identity_id.clone(),
            selectors: target.selectors.clone(),
            executable: true,
            execute,
            status: IdentityApplyStatus::Planned,
            reason: "dry_run".to_string(),
            source_path: Some(source_path.display().to_string()),
            destination_path: Some(destination.display().to_string()),
            source_exists: Some(true),
            asset,
        };
    }

    match move_profile_to_quarantine(&source_path, &destination).await {
        Ok(destination) => IdentityApplyOperation {
            operation_index: 0,
            action_index: action.action_index,
            target_index: target.target_index,
            action_code: action.action_code.clone(),
            intent,
            priority: action.priority.clone(),
            label: target.label.clone(),
            identity_id: target.identity_id.clone(),
            selectors: target.selectors.clone(),
            executable: true,
            execute,
            status: IdentityApplyStatus::Applied,
            reason: "profile_moved_to_quarantine".to_string(),
            source_path: Some(source_path.display().to_string()),
            destination_path: Some(destination.display().to_string()),
            source_exists: Some(true),
            asset,
        },
        Err(error) => IdentityApplyOperation {
            operation_index: 0,
            action_index: action.action_index,
            target_index: target.target_index,
            action_code: action.action_code.clone(),
            intent,
            priority: action.priority.clone(),
            label: target.label.clone(),
            identity_id: target.identity_id.clone(),
            selectors: target.selectors.clone(),
            executable: true,
            execute,
            status: IdentityApplyStatus::Failed,
            reason: format!("move_failed: {error}"),
            source_path: Some(source_path.display().to_string()),
            destination_path: Some(destination.display().to_string()),
            source_exists: Some(true),
            asset,
        },
    }
}

fn unresolved_apply_operation(
    action: &IdentityApplyAction,
    target: &IdentityApplyTarget,
    intent: IdentityApplyIntent,
    execute: bool,
    reason: &str,
    source_path: Option<PathBuf>,
    destination_path: Option<PathBuf>,
    source_exists: Option<bool>,
    asset: Option<IdentityProfileAsset>,
) -> IdentityApplyOperation {
    IdentityApplyOperation {
        operation_index: 0,
        action_index: action.action_index,
        target_index: target.target_index,
        action_code: action.action_code.clone(),
        intent,
        priority: action.priority.clone(),
        label: target.label.clone(),
        identity_id: target.identity_id.clone(),
        selectors: target.selectors.clone(),
        executable: false,
        execute,
        status: IdentityApplyStatus::Unresolved,
        reason: reason.to_string(),
        source_path: source_path.map(|path| path.display().to_string()),
        destination_path: destination_path.map(|path| path.display().to_string()),
        source_exists,
        asset,
    }
}

fn apply_intent_for_action(code: &str) -> IdentityApplyIntent {
    if code.contains(".quarantine") || code.contains("quarantine_") {
        IdentityApplyIntent::QuarantineProfile
    } else if code.contains(".review") || code.contains("review_") {
        IdentityApplyIntent::ReviewProfile
    } else if code.contains(".investigate") || code.contains("investigate_") {
        IdentityApplyIntent::InvestigateProfile
    } else {
        IdentityApplyIntent::RemediationPlan
    }
}

fn candidate_profile_paths(
    target: &IdentityApplyTarget,
    bindings: &ProfileBindings,
    profile_root: Option<&Path>,
) -> Vec<ProfileCandidate> {
    let mut paths = Vec::new();
    for selector in &target.selectors {
        if let Some(binding) = bindings.by_key.get(selector) {
            paths.push(ProfileCandidate {
                path: binding.path.clone(),
                asset: binding.asset.clone(),
            });
        }
        if let Some(root) = profile_root.filter(|_| safe_profile_key(selector)) {
            paths.push(ProfileCandidate {
                path: root.join(selector),
                asset: None,
            });
        }
    }
    dedup_profile_candidates(&mut paths);
    paths
}

async fn choose_profile_source(
    candidates: &[ProfileCandidate],
) -> Option<(ProfileCandidate, bool)> {
    let first = candidates.first()?.clone();
    for candidate in candidates {
        if tokio::fs::metadata(&candidate.path).await.is_ok() {
            return Some((candidate.clone(), true));
        }
    }
    Some((first, false))
}

async fn move_profile_to_quarantine(source: &Path, destination: &Path) -> Result<PathBuf> {
    let Some(parent) = destination.parent() else {
        bail!("quarantine destination has no parent");
    };
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("failed to create {}", parent.display()))?;
    let destination = unique_destination_path(destination).await;
    tokio::fs::rename(source, &destination)
        .await
        .with_context(|| {
            format!(
                "failed to move {} to {}",
                source.display(),
                destination.display()
            )
        })?;
    Ok(destination)
}

async fn unique_destination_path(path: &Path) -> PathBuf {
    if tokio::fs::metadata(path).await.is_err() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("profile");
    let extension = path.extension().and_then(|extension| extension.to_str());
    for index in 1..10_000usize {
        let file_name = if let Some(extension) = extension {
            format!("{stem}-{index}.{extension}")
        } else {
            format!("{stem}-{index}")
        };
        let candidate = path.with_file_name(file_name);
        if tokio::fs::metadata(&candidate).await.is_err() {
            return candidate;
        }
    }
    path.with_file_name(format!("{stem}-{}", unix_seconds()))
}

fn safe_profile_key(key: &str) -> bool {
    let key = key.trim();
    !key.is_empty() && key != "." && key != ".." && !key.contains('/') && !key.contains('\\')
}

fn dedup_profile_candidates(paths: &mut Vec<ProfileCandidate>) {
    let mut seen = BTreeSet::new();
    paths.retain(|candidate| seen.insert(candidate.path.clone()));
}

fn build_apply_asset_patches(
    operations: &[IdentityApplyOperation],
) -> Vec<IdentityApplyAssetPatch> {
    let mut patches = Vec::new();
    for operation in operations {
        if operation.asset.is_none()
            && operation.label.is_none()
            && operation.identity_id.is_none()
            && operation.source_path.is_none()
        {
            continue;
        }
        let asset = operation.asset.as_ref();
        patches.push(IdentityApplyAssetPatch {
            patch_index: patches.len(),
            operation_index: operation.operation_index,
            action_index: operation.action_index,
            target_index: operation.target_index,
            action_code: operation.action_code.clone(),
            intent: operation.intent,
            status: operation.status,
            execute: operation.execute,
            label: operation
                .label
                .clone()
                .or_else(|| asset.and_then(|asset| asset.label.clone())),
            identity_id: operation
                .identity_id
                .clone()
                .or_else(|| asset.and_then(|asset| asset.identity_id.clone())),
            account_id: asset.and_then(|asset| asset.account_id.clone()),
            profile_id: asset.and_then(|asset| asset.profile_id.clone()),
            proxy_id: asset.and_then(|asset| asset.proxy_id.clone()),
            fingerprint_seed: asset.and_then(|asset| asset.fingerprint_seed.clone()),
            previous_state: asset.and_then(|asset| asset.state.clone()),
            next_state: asset_next_state(operation.intent).to_string(),
            profile_dir: asset
                .map(|asset| asset.profile_dir.clone())
                .or_else(|| operation.source_path.clone()),
            source_path: operation.source_path.clone(),
            destination_path: operation.destination_path.clone(),
            reason: operation.reason.clone(),
        });
    }
    patches
}

fn asset_next_state(intent: IdentityApplyIntent) -> &'static str {
    match intent {
        IdentityApplyIntent::QuarantineProfile => "quarantine",
        IdentityApplyIntent::ReviewProfile => "review",
        IdentityApplyIntent::InvestigateProfile => "investigate",
        IdentityApplyIntent::RemediationPlan => "repair",
    }
}

async fn write_apply_asset_state(
    report: &IdentityApplyReport,
    path: Option<&Path>,
    append: bool,
) -> Result<Option<IdentityApplyAssetStateOut>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if append {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        for patch in &report.asset_patches {
            let line = json!({
                "runId": report.run_id,
                "generatedAtUnixSeconds": report.generated_at_unix_seconds,
                "patch": patch,
            });
            file.write_all(serde_json::to_string(&line)?.as_bytes())
                .await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
    } else {
        let value = json!({
            "scope": "identity_asset_state",
            "runId": report.run_id,
            "generatedAtUnixSeconds": report.generated_at_unix_seconds,
            "sourceActions": report.path,
            "execute": report.execute,
            "dryRun": report.dry_run,
            "count": report.asset_patches.len(),
            "patches": report.asset_patches,
        });
        tokio::fs::write(path, serde_json::to_vec_pretty(&value)?)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(Some(IdentityApplyAssetStateOut {
        path: path.display().to_string(),
        append,
        count: report.asset_patches.len(),
        format: if append {
            "ndjson_asset_patches".to_string()
        } else {
            "json_report".to_string()
        },
    }))
}

async fn write_apply_journal(
    report: &IdentityApplyReport,
    path: Option<&Path>,
    append: bool,
) -> Result<Option<IdentityApplyJournalOut>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if append {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        for operation in &report.operations {
            let line = json!({
                "runId": report.run_id,
                "generatedAtUnixSeconds": report.generated_at_unix_seconds,
                "operation": operation,
            });
            file.write_all(serde_json::to_string(&line)?.as_bytes())
                .await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
    } else {
        let bytes = serde_json::to_vec_pretty(report)?;
        tokio::fs::write(path, bytes)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(Some(IdentityApplyJournalOut {
        path: path.display().to_string(),
        append,
        count: report.operations.len(),
        format: if append {
            "ndjson_operations".to_string()
        } else {
            "json_report".to_string()
        },
    }))
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub async fn analyze_pool(
    path: &Path,
    against: Option<&Path>,
    gate: IdentityGate,
    accept_out: Option<&Path>,
    quarantine_out: Option<&Path>,
    baseline_out: Option<&Path>,
    ledger_out: Option<&Path>,
    actions_out: Option<&Path>,
    append_split: bool,
    append_ledger: bool,
    append_actions: bool,
) -> Result<JsonResponse> {
    let snapshots = read_snapshot_file(path).await?;
    if snapshots.is_empty() {
        bail!("identity snapshot file is empty");
    }

    let report = IdentityPoolReport::analyze(&snapshots);
    let clusters = report.risk_clusters();
    let offenders = report.risk_offenders();
    let quarantine = report.quarantine_plan();
    let diversity = report.diversity.clone();
    let entropy_budget = report.entropy_budget.clone();
    let capacity_plan = report.capacity_plan.clone();
    let remediation = report.remediation_plan.clone();
    let baseline_snapshots = if let Some(path) = against {
        let baseline = read_snapshot_file(path).await?;
        if baseline.is_empty() {
            bail!("baseline identity snapshot file is empty");
        }
        Some(baseline)
    } else {
        None
    };
    let against_report = baseline_snapshots
        .as_ref()
        .map(|baseline| compare_against(&snapshots, baseline));
    let admission =
        build_offline_admission(&snapshots, &quarantine.indexes, against_report.as_ref());
    let ledger = build_offline_ledger(&snapshots, &report, against_report.as_ref(), &admission);
    let action_queue = build_pool_action_queue(&snapshots, &remediation, &capacity_plan, &ledger);
    let split_out = write_split_outputs(
        &snapshots,
        &admission,
        accept_out,
        quarantine_out,
        append_split,
    )
    .await?;
    let baseline_out = write_baseline_output(
        &snapshots,
        baseline_snapshots.as_deref(),
        &admission,
        baseline_out,
    )
    .await?;
    let ledger_out = write_ledger_output(&ledger, ledger_out, append_ledger).await?;
    let actions_out = write_pool_actions_output(&action_queue, actions_out, append_actions).await?;
    let gate = evaluate_gate(&gate, &report, against_report.as_ref());

    let mut data = json!({
        "scope": "offline_pool",
        "path": path.display().to_string(),
        "count": snapshots.len(),
        "gate": gate,
        "clusters": clusters,
        "offenders": offenders,
        "quarantine": quarantine,
        "diversity": diversity,
        "entropyBudget": entropy_budget,
        "capacityPlan": capacity_plan,
        "remediation": remediation,
        "admission": admission,
        "ledger": ledger,
        "actionQueue": action_queue,
        "report": report,
    });
    if let Some(split_out) = split_out {
        data["splitOut"] = split_out;
    }
    if let Some(baseline_out) = baseline_out {
        data["baselineOut"] = baseline_out;
    }
    if let Some(ledger_out) = ledger_out {
        data["ledgerOut"] = ledger_out;
    }
    if let Some(actions_out) = actions_out {
        data["actionsOut"] = actions_out;
    }
    if let (Some(path), Some(report)) = (against, against_report.as_ref()) {
        data["againstPath"] = json!(path.display().to_string());
        data["againstReport"] = json!(report);
    }
    Ok(JsonResponse::ok(data))
}

pub async fn analyze_drift(
    before_path: &Path,
    after_path: &Path,
    max_drift_score: Option<u8>,
    fail_on_high_risk_drift: bool,
    match_by: IdentityDriftMatchMode,
    actions_out: Option<&Path>,
    append_actions: bool,
) -> Result<JsonResponse> {
    let before = read_labeled_snapshot_file(before_path).await?;
    if before.is_empty() {
        bail!("before identity snapshot file is empty");
    }
    let after = read_labeled_snapshot_file(after_path).await?;
    if after.is_empty() {
        bail!("after identity snapshot file is empty");
    }

    let effective_match_by = resolve_drift_match_mode(match_by, &before, &after);
    let matched = match_labeled_snapshots(&before, &after, effective_match_by)?;

    let pair_count = matched.pairs.len();
    if pair_count == 0 {
        bail!("identity drift needs at least one matched snapshot pair");
    }

    let entries = matched
        .pairs
        .iter()
        .enumerate()
        .map(|(index, pair)| {
            let report = pair.before.snapshot.drift_to(&pair.after.snapshot);
            drift_entry(index, pair, &report)
        })
        .collect::<Vec<_>>();
    let changed_count = entries
        .iter()
        .filter(|entry| entry.changed_signal_count > 0)
        .count();
    let stable_count = entries.iter().filter(|entry| entry.stable).count();
    let high_risk_count = entries.iter().filter(|entry| entry.high_risk).count();
    let max_score = entries.iter().map(|entry| entry.score).max().unwrap_or(0);
    let gate = evaluate_drift_gate(max_drift_score, fail_on_high_risk_drift, &entries);
    let action_queue = build_drift_action_queue(&entries);
    let actions_out =
        write_drift_actions_output(&action_queue, actions_out, append_actions).await?;

    let mut data = json!({
        "scope": "identity_drift",
        "beforePath": before_path.display().to_string(),
        "afterPath": after_path.display().to_string(),
        "beforeCount": before.len(),
        "afterCount": after.len(),
        "pairCount": pair_count,
        "requestedMatchBy": match_by,
        "matchBy": effective_match_by,
        "changedCount": changed_count,
        "stableCount": stable_count,
        "highRiskCount": high_risk_count,
        "maxScore": max_score,
        "missingBeforeIndexes": matched.missing_before_indexes,
        "missingAfterIndexes": matched.missing_after_indexes,
        "missingBeforeLabels": matched.missing_before_labels,
        "missingAfterLabels": matched.missing_after_labels,
        "unlabeledBeforeIndexes": matched.unlabeled_before_indexes,
        "unlabeledAfterIndexes": matched.unlabeled_after_indexes,
        "gate": gate,
        "actionQueue": action_queue,
        "entries": entries,
    });
    if let Some(actions_out) = actions_out {
        data["actionsOut"] = actions_out;
    }
    Ok(JsonResponse::ok(data))
}

pub async fn analyze_lifecycle(
    baseline_path: &Path,
    current_path: &Path,
    max_drift_score: Option<u8>,
    fail_on_high_risk_drift: bool,
    fail_on_missing_current: bool,
    fail_on_new_current: bool,
    match_by: IdentityDriftMatchMode,
    ledger_out: Option<&Path>,
    delta_out: Option<&Path>,
    journal_out: Option<&Path>,
    state_out_dir: Option<&Path>,
    actions_out: Option<&Path>,
    next_baseline_out: Option<&Path>,
    next_baseline_policy: IdentityLifecycleBaselinePolicy,
    append_ledger: bool,
    append_journal: bool,
    append_actions: bool,
) -> Result<JsonResponse> {
    let baseline = read_labeled_snapshot_file(baseline_path).await?;
    if baseline.is_empty() {
        bail!("baseline identity snapshot file is empty");
    }
    let current = read_labeled_snapshot_file(current_path).await?;
    if current.is_empty() {
        bail!("current identity snapshot file is empty");
    }

    let effective_match_by = resolve_drift_match_mode(match_by, &baseline, &current);
    let matched = match_labeled_snapshots(&baseline, &current, effective_match_by)?;
    let ledger = build_lifecycle_ledger(&baseline, &current, &matched, max_drift_score);
    let gate = evaluate_lifecycle_gate(
        max_drift_score,
        fail_on_high_risk_drift,
        fail_on_missing_current,
        fail_on_new_current,
        &ledger,
    );
    let action_queue = build_lifecycle_action_queue(&ledger);
    let state_exports = build_lifecycle_state_exports(&baseline, &current, &ledger);
    let next_baseline =
        build_lifecycle_next_baseline(&baseline, &current, &ledger, next_baseline_policy);
    let delta = build_lifecycle_delta(&ledger, &next_baseline);
    let ledger_out = write_lifecycle_ledger_output(&ledger, ledger_out, append_ledger).await?;
    let delta_out = write_lifecycle_delta_output(&delta, delta_out).await?;
    let state_out = write_lifecycle_state_outputs(&state_exports, state_out_dir).await?;
    let actions_out =
        write_lifecycle_actions_output(&action_queue, actions_out, append_actions).await?;
    let next_baseline_out =
        write_lifecycle_next_baseline_output(&next_baseline, next_baseline_out).await?;
    let run_record = build_lifecycle_run_record(LifecycleRunRecordInput {
        baseline_path,
        current_path,
        requested_match_by: match_by,
        effective_match_by,
        baseline_count: baseline.len(),
        current_count: current.len(),
        gate: &gate,
        ledger: &ledger,
        delta: &delta,
        action_queue: &action_queue,
        next_baseline: &next_baseline,
        ledger_out: ledger_out.as_ref(),
        delta_out: delta_out.as_ref(),
        state_out: state_out.as_ref(),
        actions_out: actions_out.as_ref(),
        next_baseline_out: next_baseline_out.as_ref(),
    });
    let journal_out =
        write_lifecycle_journal_output(&run_record, journal_out, append_journal).await?;

    let mut data = json!({
        "scope": "identity_lifecycle",
        "baselinePath": baseline_path.display().to_string(),
        "currentPath": current_path.display().to_string(),
        "baselineCount": baseline.len(),
        "currentCount": current.len(),
        "requestedMatchBy": match_by,
        "matchBy": effective_match_by,
        "missingBaselineIndexes": matched.missing_before_indexes,
        "missingCurrentIndexes": matched.missing_after_indexes,
        "missingBaselineLabels": matched.missing_before_labels,
        "missingCurrentLabels": matched.missing_after_labels,
        "unlabeledBaselineIndexes": matched.unlabeled_before_indexes,
        "unlabeledCurrentIndexes": matched.unlabeled_after_indexes,
        "gate": gate,
        "summary": {
            "entryCount": ledger.entry_count,
            "activeCount": ledger.active_count,
            "repairCount": ledger.repair_count,
            "quarantineCount": ledger.quarantine_count,
            "missingCurrentCount": ledger.missing_current_count,
            "newCurrentCount": ledger.new_current_count,
            "changedCount": ledger.changed_count,
            "highRiskCount": ledger.high_risk_count,
            "maxDriftScore": ledger.max_drift_score,
        },
        "stateBuckets": lifecycle_state_buckets(&state_exports),
        "actionQueue": action_queue,
        "nextBaseline": next_baseline,
        "delta": delta,
        "run": run_record,
        "ledger": ledger,
    });
    if let Some(ledger_out) = ledger_out {
        data["ledgerOut"] = ledger_out;
    }
    if let Some(delta_out) = delta_out {
        data["deltaOut"] = delta_out;
    }
    if let Some(journal_out) = journal_out {
        data["journalOut"] = journal_out;
    }
    if let Some(state_out) = state_out {
        data["stateOut"] = state_out;
    }
    if let Some(actions_out) = actions_out {
        data["actionsOut"] = actions_out;
    }
    if let Some(next_baseline_out) = next_baseline_out {
        data["nextBaselineOut"] = next_baseline_out;
    }
    Ok(JsonResponse::ok(data))
}

pub async fn write_response_snapshots(
    response: &JsonResponse,
    path: &Path,
    append: bool,
) -> Result<usize> {
    let data = response
        .data
        .as_ref()
        .context("identity response does not contain data")?;
    let snapshots = snapshots_from_value(data)?;
    write_snapshots(path, &snapshots, append).await?;
    Ok(snapshots.len())
}

async fn write_snapshots(
    path: &Path,
    snapshots: &[FingerprintSnapshot],
    append: bool,
) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if append {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        for snapshot in snapshots {
            let line = serde_json::to_string(snapshot)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
    } else {
        let bytes = serde_json::to_vec_pretty(snapshots)?;
        tokio::fs::write(path, bytes)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(())
}

async fn write_split_outputs(
    snapshots: &[FingerprintSnapshot],
    admission: &OfflineAdmissionPlan,
    accept_out: Option<&Path>,
    quarantine_out: Option<&Path>,
    append: bool,
) -> Result<Option<Value>> {
    if accept_out.is_none() && quarantine_out.is_none() {
        return Ok(None);
    }

    let mut out = json!({
        "append": append,
        "format": if append { "ndjson" } else { "json_array" },
    });
    if let Some(path) = accept_out {
        let accepted = select_snapshots(snapshots, &admission.accept_indexes)?;
        write_snapshots(path, &accepted, append).await?;
        out["accepted"] = json!({
            "path": path.display().to_string(),
            "count": accepted.len(),
        });
    }
    if let Some(path) = quarantine_out {
        let quarantined = select_snapshots(snapshots, &admission.quarantine_indexes)?;
        write_snapshots(path, &quarantined, append).await?;
        out["quarantine"] = json!({
            "path": path.display().to_string(),
            "count": quarantined.len(),
        });
    }
    Ok(Some(out))
}

async fn write_baseline_output(
    candidates: &[FingerprintSnapshot],
    baseline: Option<&[FingerprintSnapshot]>,
    admission: &OfflineAdmissionPlan,
    path: Option<&Path>,
) -> Result<Option<Value>> {
    let Some(path) = path else {
        return Ok(None);
    };

    let accepted = select_snapshots(candidates, &admission.accept_indexes)?;
    let baseline_count = baseline.map_or(0, |snapshots| snapshots.len());
    let mut updated = Vec::with_capacity(baseline_count + accepted.len());
    if let Some(baseline) = baseline {
        updated.extend_from_slice(baseline);
    }
    updated.extend(accepted);

    write_snapshots(path, &updated, false).await?;
    Ok(Some(json!({
        "path": path.display().to_string(),
        "count": updated.len(),
        "baselineCount": baseline_count,
        "acceptedAdded": admission.accept_count,
        "format": "json_array",
    })))
}

async fn write_ledger_output(
    ledger: &OfflineLedgerReport,
    path: Option<&Path>,
    append: bool,
) -> Result<Option<Value>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if append {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        for entry in &ledger.entries {
            let line = serde_json::to_string(entry)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
    } else {
        let bytes = serde_json::to_vec_pretty(ledger)?;
        tokio::fs::write(path, bytes)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(Some(json!({
        "path": path.display().to_string(),
        "append": append,
        "count": ledger.entries.len(),
        "format": if append { "ndjson_entries" } else { "json_report" },
    })))
}

async fn write_pool_actions_output(
    queue: &OfflinePoolActionQueue,
    path: Option<&Path>,
    append: bool,
) -> Result<Option<Value>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if append {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        for action in &queue.actions {
            let line = serde_json::to_string(action)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
    } else {
        let bytes = serde_json::to_vec_pretty(queue)?;
        tokio::fs::write(path, bytes)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(Some(json!({
        "path": path.display().to_string(),
        "append": append,
        "count": queue.actions.len(),
        "format": if append { "ndjson_actions" } else { "json_report" },
    })))
}

async fn write_drift_actions_output(
    queue: &OfflineDriftActionQueue,
    path: Option<&Path>,
    append: bool,
) -> Result<Option<Value>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if append {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        for action in &queue.actions {
            let line = serde_json::to_string(action)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
    } else {
        let bytes = serde_json::to_vec_pretty(queue)?;
        tokio::fs::write(path, bytes)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(Some(json!({
        "path": path.display().to_string(),
        "append": append,
        "count": queue.actions.len(),
        "format": if append { "ndjson_actions" } else { "json_report" },
    })))
}

async fn write_lifecycle_ledger_output(
    ledger: &OfflineLifecycleLedger,
    path: Option<&Path>,
    append: bool,
) -> Result<Option<Value>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if append {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        for entry in &ledger.entries {
            let line = serde_json::to_string(entry)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
    } else {
        let bytes = serde_json::to_vec_pretty(ledger)?;
        tokio::fs::write(path, bytes)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(Some(json!({
        "path": path.display().to_string(),
        "append": append,
        "count": ledger.entries.len(),
        "format": if append { "ndjson_entries" } else { "json_report" },
    })))
}

async fn write_lifecycle_delta_output(
    delta: &OfflineLifecycleDelta,
    path: Option<&Path>,
) -> Result<Option<Value>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let bytes = serde_json::to_vec_pretty(delta)?;
    tokio::fs::write(path, bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(Some(json!({
        "path": path.display().to_string(),
        "count": delta.entries.len(),
        "format": "json_report",
    })))
}

async fn write_lifecycle_journal_output(
    record: &OfflineLifecycleRunRecord,
    path: Option<&Path>,
    append: bool,
) -> Result<Option<Value>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if append {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        let line = serde_json::to_string(record)?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
    } else {
        let bytes = serde_json::to_vec_pretty(record)?;
        tokio::fs::write(path, bytes)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(Some(json!({
        "path": path.display().to_string(),
        "append": append,
        "runId": record.run_id,
        "format": if append { "ndjson_records" } else { "json_record" },
    })))
}

async fn write_lifecycle_state_outputs(
    exports: &[OfflineLifecycleStateExport],
    dir: Option<&Path>,
) -> Result<Option<Value>> {
    let Some(dir) = dir else {
        return Ok(None);
    };
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("failed to create {}", dir.display()))?;

    let mut states = Vec::new();
    for export in exports {
        let path = dir.join(format!("{}.json", lifecycle_state_file_stem(export.state)));
        let bytes = serde_json::to_vec_pretty(&export.entries)?;
        tokio::fs::write(&path, bytes)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
        states.push(json!({
            "state": export.state,
            "path": path.display().to_string(),
            "count": export.count,
            "currentSourceCount": export.current_source_count,
            "baselineSourceCount": export.baseline_source_count,
        }));
    }

    Ok(Some(json!({
        "dir": dir.display().to_string(),
        "format": "labeled_snapshot_array_by_state",
        "states": states,
    })))
}

async fn write_lifecycle_actions_output(
    queue: &OfflineLifecycleActionQueue,
    path: Option<&Path>,
    append: bool,
) -> Result<Option<Value>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if append {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        for action in &queue.actions {
            let line = serde_json::to_string(action)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
    } else {
        let bytes = serde_json::to_vec_pretty(queue)?;
        tokio::fs::write(path, bytes)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(Some(json!({
        "path": path.display().to_string(),
        "append": append,
        "count": queue.actions.len(),
        "format": if append { "ndjson_actions" } else { "json_report" },
    })))
}

async fn write_lifecycle_next_baseline_output(
    baseline: &OfflineLifecycleNextBaseline,
    path: Option<&Path>,
) -> Result<Option<Value>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let bytes = serde_json::to_vec_pretty(&baseline.entries)?;
    tokio::fs::write(path, bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(Some(json!({
        "path": path.display().to_string(),
        "count": baseline.entries.len(),
        "policy": baseline.policy,
        "format": "labeled_snapshot_array",
    })))
}

fn select_snapshots(
    snapshots: &[FingerprintSnapshot],
    indexes: &[usize],
) -> Result<Vec<FingerprintSnapshot>> {
    let mut selected = Vec::with_capacity(indexes.len());
    for index in indexes {
        let snapshot = snapshots
            .get(*index)
            .with_context(|| format!("admission index {index} is out of range"))?;
        selected.push(snapshot.clone());
    }
    Ok(selected)
}

fn identity_ids(snapshots: &[FingerprintSnapshot]) -> Vec<String> {
    snapshots
        .iter()
        .map(FingerprintSnapshot::identity_id)
        .collect()
}

fn ids_for_indexes(ids: &[String], indexes: &[usize]) -> Vec<String> {
    indexes
        .iter()
        .filter_map(|index| ids.get(*index).cloned())
        .collect()
}

async fn read_snapshot_file(path: &Path) -> Result<Vec<FingerprintSnapshot>> {
    let text = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    parse_snapshots(&text)
        .with_context(|| format!("failed to parse identity snapshots from {}", path.display()))
}

async fn read_labeled_snapshot_file(path: &Path) -> Result<Vec<LabeledSnapshot>> {
    let text = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    parse_labeled_snapshots(&text).with_context(|| {
        format!(
            "failed to parse labeled identity snapshots from {}",
            path.display()
        )
    })
}

fn compare_against(
    candidates: &[FingerprintSnapshot],
    baseline: &[FingerprintSnapshot],
) -> BaselineCompareReport {
    let mut risky_pairs = Vec::new();
    let mut max_linkability = 0u8;
    let candidate_ids = identity_ids(candidates);
    let baseline_ids = identity_ids(baseline);

    for (candidate_index, candidate) in candidates.iter().enumerate() {
        for (baseline_index, existing) in baseline.iter().enumerate() {
            let pair = LinkabilityReport::compare(candidate, existing);
            max_linkability = max_linkability.max(pair.score);
            if pair.same_identity_likely || pair.has_strong_signal() || pair.score >= 30 {
                risky_pairs.push(BaselineLinkabilityPair {
                    candidate_index,
                    candidate_id: candidate_ids[candidate_index].clone(),
                    baseline_index,
                    baseline_id: baseline_ids[baseline_index].clone(),
                    score: pair.score,
                    same_identity_likely: pair.same_identity_likely,
                    signals: pair.signals,
                });
            }
        }
    }

    risky_pairs.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.candidate_index.cmp(&b.candidate_index))
            .then_with(|| a.baseline_index.cmp(&b.baseline_index))
    });

    BaselineCompareReport {
        candidate_count: candidates.len(),
        candidate_ids: candidate_ids.clone(),
        baseline_count: baseline.len(),
        baseline_ids: baseline_ids.clone(),
        max_linkability,
        clusters: build_baseline_clusters(&candidate_ids, &baseline_ids, &risky_pairs),
        candidate_offenders: build_baseline_offenders(
            &candidate_ids,
            &baseline_ids,
            &risky_pairs,
            true,
        ),
        baseline_offenders: build_baseline_offenders(
            &candidate_ids,
            &baseline_ids,
            &risky_pairs,
            false,
        ),
        candidate_quarantine: build_baseline_quarantine(&candidate_ids, &risky_pairs),
        risky_pairs,
    }
}

fn build_baseline_clusters(
    candidate_ids: &[String],
    baseline_ids: &[String],
    pairs: &[BaselineLinkabilityPair],
) -> Vec<BaselineIdentityCluster> {
    let candidate_count = candidate_ids.len();
    let baseline_count = baseline_ids.len();
    if candidate_count == 0 || baseline_count == 0 || pairs.is_empty() {
        return Vec::new();
    }

    let total = candidate_count + baseline_count;
    let mut parent: Vec<_> = (0..total).collect();
    for pair in pairs {
        if pair.candidate_index < candidate_count && pair.baseline_index < baseline_count {
            union_roots(
                &mut parent,
                pair.candidate_index,
                candidate_count + pair.baseline_index,
            );
        }
    }

    let mut groups: BTreeMap<usize, (Vec<usize>, Vec<usize>)> = BTreeMap::new();
    for candidate_index in 0..candidate_count {
        let root = find_root(&mut parent, candidate_index);
        groups.entry(root).or_default().0.push(candidate_index);
    }
    for baseline_index in 0..baseline_count {
        let node = candidate_count + baseline_index;
        let root = find_root(&mut parent, node);
        groups.entry(root).or_default().1.push(baseline_index);
    }

    let mut clusters = Vec::new();
    for (candidate_indexes, baseline_indexes) in groups
        .into_values()
        .filter(|(candidates, baseline)| !candidates.is_empty() && !baseline.is_empty())
    {
        let candidate_set: BTreeSet<_> = candidate_indexes.iter().copied().collect();
        let baseline_set: BTreeSet<_> = baseline_indexes.iter().copied().collect();
        let mut pair_count = 0usize;
        let mut max_score = 0u8;
        let mut strong_signal_count = 0usize;
        let mut signal_codes = BTreeSet::new();

        for pair in pairs.iter().filter(|pair| {
            candidate_set.contains(&pair.candidate_index)
                && baseline_set.contains(&pair.baseline_index)
        }) {
            pair_count += 1;
            max_score = max_score.max(pair.score);
            for signal in &pair.signals {
                if signal.strength == drission::fingerprint::LinkabilityStrength::Strong {
                    strong_signal_count += 1;
                }
                signal_codes.insert(signal.code.clone());
            }
        }

        if pair_count > 0 {
            let candidate_ids = ids_for_indexes(candidate_ids, &candidate_indexes);
            let baseline_ids = ids_for_indexes(baseline_ids, &baseline_indexes);
            clusters.push(BaselineIdentityCluster {
                candidate_indexes,
                candidate_ids,
                baseline_indexes,
                baseline_ids,
                pair_count,
                max_score,
                strong_signal_count,
                signal_codes: signal_codes.into_iter().collect(),
            });
        }
    }

    clusters.sort_by(|a, b| {
        b.max_score
            .cmp(&a.max_score)
            .then_with(|| {
                let a_len = a.candidate_indexes.len() + a.baseline_indexes.len();
                let b_len = b.candidate_indexes.len() + b.baseline_indexes.len();
                b_len.cmp(&a_len)
            })
            .then_with(|| a.candidate_indexes.cmp(&b.candidate_indexes))
            .then_with(|| a.baseline_indexes.cmp(&b.baseline_indexes))
    });
    clusters
}

fn build_baseline_offenders(
    candidate_ids: &[String],
    baseline_ids: &[String],
    pairs: &[BaselineLinkabilityPair],
    candidate_side: bool,
) -> Vec<BaselineIdentityOffender> {
    let size = if candidate_side {
        candidate_ids.len()
    } else {
        baseline_ids.len()
    };
    if size == 0 || pairs.is_empty() {
        return Vec::new();
    }

    let mut offenders: BTreeMap<usize, BaselineOffenderAcc> = BTreeMap::new();
    for pair in pairs {
        let index = if candidate_side {
            pair.candidate_index
        } else {
            pair.baseline_index
        };
        let linked = if candidate_side {
            pair.baseline_index
        } else {
            pair.candidate_index
        };
        if index >= size {
            continue;
        }
        let acc = offenders.entry(index).or_default();
        acc.pair_count += 1;
        acc.max_score = acc.max_score.max(pair.score);
        acc.linked_indexes.insert(linked);
        for signal in &pair.signals {
            if signal.strength == drission::fingerprint::LinkabilityStrength::Strong {
                acc.strong_signal_count += 1;
            }
            acc.signal_codes.insert(signal.code.clone());
        }
    }

    let mut out = offenders
        .into_iter()
        .map(|(index, acc)| {
            let linked_indexes = acc.linked_indexes.into_iter().collect::<Vec<_>>();
            let identity_id = if candidate_side {
                candidate_ids.get(index)
            } else {
                baseline_ids.get(index)
            }
            .cloned()
            .unwrap_or_default();
            let linked_ids = if candidate_side {
                ids_for_indexes(baseline_ids, &linked_indexes)
            } else {
                ids_for_indexes(candidate_ids, &linked_indexes)
            };
            BaselineIdentityOffender {
                index,
                identity_id,
                pair_count: acc.pair_count,
                max_score: acc.max_score,
                strong_signal_count: acc.strong_signal_count,
                linked_indexes,
                linked_ids,
                signal_codes: acc.signal_codes.into_iter().collect(),
            }
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| {
        b.max_score
            .cmp(&a.max_score)
            .then_with(|| b.pair_count.cmp(&a.pair_count))
            .then_with(|| b.strong_signal_count.cmp(&a.strong_signal_count))
            .then_with(|| a.index.cmp(&b.index))
    });
    out
}

#[derive(Debug, Default)]
struct BaselineOffenderAcc {
    pair_count: usize,
    max_score: u8,
    strong_signal_count: usize,
    linked_indexes: BTreeSet<usize>,
    signal_codes: BTreeSet<String>,
}

fn build_baseline_quarantine(
    candidate_ids: &[String],
    pairs: &[BaselineLinkabilityPair],
) -> BaselineQuarantinePlan {
    let candidate_count = candidate_ids.len();
    if candidate_count == 0 || pairs.is_empty() {
        return BaselineQuarantinePlan {
            candidate_indexes: Vec::new(),
            candidate_ids: Vec::new(),
            covered_pair_count: 0,
            remaining_pair_count: 0,
            max_covered_score: 0,
        };
    }

    let mut remaining: BTreeSet<usize> = (0..pairs.len()).collect();
    let mut candidate_indexes = Vec::new();
    let mut max_covered_score = 0u8;

    while !remaining.is_empty() {
        let mut best: Option<(usize, u8, usize, usize)> = None;
        for candidate_index in 0..candidate_count {
            if candidate_indexes.contains(&candidate_index) {
                continue;
            }
            let mut cover_count = 0usize;
            let mut best_score = 0u8;
            for pair_index in &remaining {
                let pair = &pairs[*pair_index];
                if pair.candidate_index == candidate_index {
                    cover_count += 1;
                    best_score = best_score.max(pair.score);
                }
            }
            if cover_count == 0 {
                continue;
            }
            let candidate = (
                cover_count,
                best_score,
                usize::MAX - candidate_index,
                candidate_index,
            );
            if best.map_or(true, |current| candidate > current) {
                best = Some(candidate);
            }
        }

        let Some((_, _, _, chosen)) = best else {
            break;
        };
        candidate_indexes.push(chosen);
        let covered_now = remaining
            .iter()
            .copied()
            .filter(|pair_index| pairs[*pair_index].candidate_index == chosen)
            .collect::<Vec<_>>();
        for pair_index in covered_now {
            max_covered_score = max_covered_score.max(pairs[pair_index].score);
            remaining.remove(&pair_index);
        }
    }

    BaselineQuarantinePlan {
        candidate_ids: ids_for_indexes(candidate_ids, &candidate_indexes),
        candidate_indexes,
        covered_pair_count: pairs.len().saturating_sub(remaining.len()),
        remaining_pair_count: remaining.len(),
        max_covered_score,
    }
}

fn union_roots(parent: &mut [usize], a: usize, b: usize) {
    let ra = find_root(parent, a);
    let rb = find_root(parent, b);
    if ra != rb {
        parent[rb] = ra;
    }
}

fn find_root(parent: &mut [usize], mut index: usize) -> usize {
    let mut root = index;
    while parent[root] != root {
        root = parent[root];
    }
    while parent[index] != index {
        let next = parent[index];
        parent[index] = root;
        index = next;
    }
    root
}

fn evaluate_gate(
    gate: &IdentityGate,
    report: &IdentityPoolReport,
    against: Option<&BaselineCompareReport>,
) -> IdentityGateReport {
    let criteria = gate.effective();
    let mut failures = criteria.evaluate_pool_report(report).failures;

    if let Some(against) = against {
        if let Some(max_linkability) = criteria.max_linkability {
            if against.max_linkability > max_linkability {
                failures.push(format!(
                    "baseline_linkability_above_max: max {} > {}",
                    against.max_linkability, max_linkability
                ));
            }
        }
        if criteria.fail_on_risky_pairs && !against.risky_pairs.is_empty() {
            failures.push(format!(
                "baseline_risky_identity_pairs: {} risky pairs",
                against.risky_pairs.len()
            ));
        }
    }

    IdentityGateReport::from_failures(criteria, failures)
}

#[derive(Debug, Clone)]
struct MatchedDriftPair {
    label: Option<String>,
    before_index: usize,
    after_index: usize,
    before: LabeledSnapshot,
    after: LabeledSnapshot,
}

#[derive(Debug, Clone, Default)]
struct MatchedDriftSet {
    pairs: Vec<MatchedDriftPair>,
    missing_before_indexes: Vec<usize>,
    missing_after_indexes: Vec<usize>,
    missing_before_labels: Vec<String>,
    missing_after_labels: Vec<String>,
    unlabeled_before_indexes: Vec<usize>,
    unlabeled_after_indexes: Vec<usize>,
}

fn resolve_drift_match_mode(
    requested: IdentityDriftMatchMode,
    before: &[LabeledSnapshot],
    after: &[LabeledSnapshot],
) -> IdentityDriftMatchMode {
    let before_has_labels = before.iter().all(|entry| entry.label.is_some());
    let after_has_labels = after.iter().all(|entry| entry.label.is_some());
    match requested {
        IdentityDriftMatchMode::Auto if before_has_labels && after_has_labels => {
            IdentityDriftMatchMode::Label
        }
        IdentityDriftMatchMode::Auto => IdentityDriftMatchMode::Index,
        explicit => explicit,
    }
}

fn match_labeled_snapshots(
    before: &[LabeledSnapshot],
    after: &[LabeledSnapshot],
    match_by: IdentityDriftMatchMode,
) -> Result<MatchedDriftSet> {
    Ok(match match_by {
        IdentityDriftMatchMode::Auto => unreachable!("auto match mode must be resolved first"),
        IdentityDriftMatchMode::Index => match_drift_by_index(before, after),
        IdentityDriftMatchMode::Label => match_drift_by_label(before, after)?,
    })
}

fn match_drift_by_index(before: &[LabeledSnapshot], after: &[LabeledSnapshot]) -> MatchedDriftSet {
    let pair_count = before.len().min(after.len());
    let pairs = (0..pair_count)
        .map(|index| MatchedDriftPair {
            label: matching_label(before[index].label.as_ref(), after[index].label.as_ref()),
            before_index: index,
            after_index: index,
            before: before[index].clone(),
            after: after[index].clone(),
        })
        .collect::<Vec<_>>();
    MatchedDriftSet {
        pairs,
        missing_before_indexes: (after.len()..before.len()).collect(),
        missing_after_indexes: (before.len()..after.len()).collect(),
        ..MatchedDriftSet::default()
    }
}

fn match_drift_by_label(
    before: &[LabeledSnapshot],
    after: &[LabeledSnapshot],
) -> Result<MatchedDriftSet> {
    let before_unlabeled = unlabeled_indexes(before);
    let after_unlabeled = unlabeled_indexes(after);
    if !before_unlabeled.is_empty() || !after_unlabeled.is_empty() {
        let mut set = MatchedDriftSet::default();
        set.unlabeled_before_indexes = before_unlabeled;
        set.unlabeled_after_indexes = after_unlabeled;
        bail!(
            "label match requires every snapshot to have accountId/id/label/name/key; unlabeled_before={:?}, unlabeled_after={:?}",
            set.unlabeled_before_indexes,
            set.unlabeled_after_indexes
        );
    }

    let before_map = labeled_snapshot_map(before, "before")?;
    let after_map = labeled_snapshot_map(after, "after")?;
    let before_labels = before_map.keys().cloned().collect::<BTreeSet<_>>();
    let after_labels = after_map.keys().cloned().collect::<BTreeSet<_>>();
    let matched_labels = before_labels
        .intersection(&after_labels)
        .cloned()
        .collect::<Vec<_>>();
    let missing_before_labels = after_labels
        .difference(&before_labels)
        .cloned()
        .collect::<Vec<_>>();
    let missing_after_labels = before_labels
        .difference(&after_labels)
        .cloned()
        .collect::<Vec<_>>();

    let pairs = matched_labels
        .iter()
        .filter_map(|label| {
            let (before_index, before) = before_map.get(label)?;
            let (after_index, after) = after_map.get(label)?;
            Some(MatchedDriftPair {
                label: Some(label.clone()),
                before_index: *before_index,
                after_index: *after_index,
                before: before.clone(),
                after: after.clone(),
            })
        })
        .collect::<Vec<_>>();

    Ok(MatchedDriftSet {
        pairs,
        missing_before_labels,
        missing_after_labels,
        ..MatchedDriftSet::default()
    })
}

fn matching_label(before: Option<&String>, after: Option<&String>) -> Option<String> {
    match (before, after) {
        (Some(before), Some(after)) if before == after => Some(before.clone()),
        _ => None,
    }
}

fn unlabeled_indexes(entries: &[LabeledSnapshot]) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.label.is_none().then_some(index))
        .collect()
}

fn labeled_snapshot_map(
    entries: &[LabeledSnapshot],
    side: &str,
) -> Result<BTreeMap<String, (usize, LabeledSnapshot)>> {
    let mut map = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(label) = entry.label.clone() else {
            continue;
        };
        if map.insert(label.clone(), (index, entry.clone())).is_some() {
            bail!("duplicate {side} identity drift label: {label}");
        }
    }
    Ok(map)
}

fn drift_entry(
    index: usize,
    pair: &MatchedDriftPair,
    report: &IdentityDriftReport,
) -> OfflineDriftEntry {
    OfflineDriftEntry {
        index,
        label: pair.label.clone(),
        before_index: pair.before_index,
        after_index: pair.after_index,
        before_id: report.before_id.clone(),
        after_id: report.after_id.clone(),
        before_stable_hash: report.before_stable_hash.clone(),
        after_stable_hash: report.after_stable_hash.clone(),
        stable_hash_changed: report.stable_hash_changed,
        stable: report.is_stable(),
        high_risk: report.has_high_risk_drift(),
        score: report.score,
        severity: report.severity,
        changed_signal_count: report.changed_signal_count,
        high_risk_signal_count: report.high_risk_signal_count,
        signals: report.signals.clone(),
        remediation: report.remediation_plan.clone(),
    }
}

fn build_drift_action_queue(entries: &[OfflineDriftEntry]) -> OfflineDriftActionQueue {
    let mut actions = Vec::new();
    let mut labels = BTreeSet::new();

    for entry in entries {
        if let Some(label) = &entry.label {
            labels.insert(label.clone());
        }
        for action in &entry.remediation.actions {
            actions.push(OfflineDriftActionEntry {
                entry_index: entry.index,
                label: entry.label.clone(),
                before_index: entry.before_index,
                after_index: entry.after_index,
                before_id: entry.before_id.clone(),
                after_id: entry.after_id.clone(),
                drift_score: entry.score,
                drift_severity: entry.severity,
                high_risk: entry.high_risk,
                stable_hash_changed: entry.stable_hash_changed,
                action_code: action.code.clone(),
                target: action.target,
                priority: action.priority,
                title: action.title.clone(),
                detail: action.detail.clone(),
                fields: action.fields.clone(),
                signal_codes: action.signal_codes.clone(),
                before_values: action.before_values.clone(),
                after_values: action.after_values.clone(),
            });
        }
    }

    actions.sort_by(|a, b| {
        a.entry_index
            .cmp(&b.entry_index)
            .then_with(|| {
                fix_priority_rank_for_queue(b.priority)
                    .cmp(&fix_priority_rank_for_queue(a.priority))
            })
            .then_with(|| a.action_code.cmp(&b.action_code))
    });

    OfflineDriftActionQueue {
        entry_count: entries.len(),
        action_count: actions.len(),
        high_priority_count: actions
            .iter()
            .filter(|action| action.priority == IdentityFixPriority::High)
            .count(),
        labels: labels.into_iter().collect(),
        actions,
    }
}

fn build_lifecycle_ledger(
    baseline: &[LabeledSnapshot],
    current: &[LabeledSnapshot],
    matched: &MatchedDriftSet,
    max_drift_score: Option<u8>,
) -> OfflineLifecycleLedger {
    let mut entries = Vec::new();

    for pair in &matched.pairs {
        let report = pair.before.snapshot.drift_to(&pair.after.snapshot);
        let state = lifecycle_state_for_drift(&report, max_drift_score);
        entries.push(lifecycle_entry_from_drift(
            entries.len(),
            pair,
            &report,
            state,
        ));
    }

    for before_index in &matched.missing_after_indexes {
        if let Some(entry) = baseline.get(*before_index) {
            entries.push(lifecycle_entry_for_missing_current(
                entries.len(),
                *before_index,
                entry,
            ));
        }
    }
    for label in &matched.missing_after_labels {
        if let Some((before_index, entry)) = labeled_snapshot_by_label(baseline, label) {
            entries.push(lifecycle_entry_for_missing_current(
                entries.len(),
                before_index,
                entry,
            ));
        }
    }

    for after_index in &matched.missing_before_indexes {
        if let Some(entry) = current.get(*after_index) {
            entries.push(lifecycle_entry_for_new_current(
                entries.len(),
                *after_index,
                entry,
            ));
        }
    }
    for label in &matched.missing_before_labels {
        if let Some((after_index, entry)) = labeled_snapshot_by_label(current, label) {
            entries.push(lifecycle_entry_for_new_current(
                entries.len(),
                after_index,
                entry,
            ));
        }
    }

    entries.sort_by(|a, b| {
        lifecycle_state_rank(a.state)
            .cmp(&lifecycle_state_rank(b.state))
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.before_index.cmp(&b.before_index))
            .then_with(|| a.after_index.cmp(&b.after_index))
    });
    for (index, entry) in entries.iter_mut().enumerate() {
        entry.index = index;
    }

    let labels = entries
        .iter()
        .filter_map(|entry| entry.label.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let max_score = entries
        .iter()
        .filter_map(|entry| entry.drift_score)
        .max()
        .unwrap_or(0);

    OfflineLifecycleLedger {
        baseline_count: baseline.len(),
        current_count: current.len(),
        entry_count: entries.len(),
        active_count: entries
            .iter()
            .filter(|entry| entry.state == OfflineLifecycleState::Active)
            .count(),
        repair_count: entries
            .iter()
            .filter(|entry| entry.state == OfflineLifecycleState::Repair)
            .count(),
        quarantine_count: entries
            .iter()
            .filter(|entry| entry.state == OfflineLifecycleState::Quarantine)
            .count(),
        missing_current_count: entries
            .iter()
            .filter(|entry| entry.state == OfflineLifecycleState::MissingCurrent)
            .count(),
        new_current_count: entries
            .iter()
            .filter(|entry| entry.state == OfflineLifecycleState::NewCurrent)
            .count(),
        changed_count: entries
            .iter()
            .filter(|entry| entry.changed_signal_count > 0)
            .count(),
        high_risk_count: entries
            .iter()
            .filter(|entry| entry.high_risk == Some(true))
            .count(),
        max_drift_score: max_score,
        labels,
        entries,
    }
}

fn lifecycle_state_for_drift(
    report: &IdentityDriftReport,
    max_drift_score: Option<u8>,
) -> OfflineLifecycleState {
    if report.is_stable() {
        OfflineLifecycleState::Active
    } else if report.has_high_risk_drift() || max_drift_score.is_some_and(|max| report.score > max)
    {
        OfflineLifecycleState::Quarantine
    } else {
        OfflineLifecycleState::Repair
    }
}

fn lifecycle_entry_from_drift(
    index: usize,
    pair: &MatchedDriftPair,
    report: &IdentityDriftReport,
    state: OfflineLifecycleState,
) -> OfflineLifecycleEntry {
    let mut reason_codes = Vec::new();
    match state {
        OfflineLifecycleState::Active => reason_codes.push("lifecycle.active".to_string()),
        OfflineLifecycleState::Repair => reason_codes.push("lifecycle.repair_drift".to_string()),
        OfflineLifecycleState::Quarantine => {
            reason_codes.push("lifecycle.quarantine_drift".to_string())
        }
        OfflineLifecycleState::MissingCurrent | OfflineLifecycleState::NewCurrent => {}
    }
    if report.has_high_risk_drift() {
        reason_codes.push("drift.high_risk".to_string());
    }
    if report.stable_hash_changed {
        reason_codes.push("drift.stable_hash_changed".to_string());
    }

    OfflineLifecycleEntry {
        index,
        state,
        label: pair.label.clone(),
        before_index: Some(pair.before_index),
        after_index: Some(pair.after_index),
        before_id: Some(report.before_id.clone()),
        after_id: Some(report.after_id.clone()),
        before_stable_hash: Some(report.before_stable_hash.clone()),
        after_stable_hash: Some(report.after_stable_hash.clone()),
        stable_hash_changed: Some(report.stable_hash_changed),
        stable: Some(report.is_stable()),
        high_risk: Some(report.has_high_risk_drift()),
        drift_score: Some(report.score),
        drift_severity: Some(report.severity),
        changed_signal_count: report.changed_signal_count,
        high_risk_signal_count: report.high_risk_signal_count,
        reason_codes,
        signals: report.signals.clone(),
        remediation: (!report.remediation_plan.is_empty())
            .then_some(report.remediation_plan.clone()),
    }
}

fn lifecycle_entry_for_missing_current(
    index: usize,
    before_index: usize,
    entry: &LabeledSnapshot,
) -> OfflineLifecycleEntry {
    OfflineLifecycleEntry {
        index,
        state: OfflineLifecycleState::MissingCurrent,
        label: entry.label.clone(),
        before_index: Some(before_index),
        after_index: None,
        before_id: Some(entry.snapshot.identity_id()),
        after_id: None,
        before_stable_hash: Some(entry.snapshot.stable_hash()),
        after_stable_hash: None,
        stable_hash_changed: None,
        stable: None,
        high_risk: None,
        drift_score: None,
        drift_severity: None,
        changed_signal_count: 0,
        high_risk_signal_count: 0,
        reason_codes: vec!["lifecycle.missing_current".to_string()],
        signals: Vec::new(),
        remediation: None,
    }
}

fn lifecycle_entry_for_new_current(
    index: usize,
    after_index: usize,
    entry: &LabeledSnapshot,
) -> OfflineLifecycleEntry {
    OfflineLifecycleEntry {
        index,
        state: OfflineLifecycleState::NewCurrent,
        label: entry.label.clone(),
        before_index: None,
        after_index: Some(after_index),
        before_id: None,
        after_id: Some(entry.snapshot.identity_id()),
        before_stable_hash: None,
        after_stable_hash: Some(entry.snapshot.stable_hash()),
        stable_hash_changed: None,
        stable: None,
        high_risk: None,
        drift_score: None,
        drift_severity: None,
        changed_signal_count: 0,
        high_risk_signal_count: 0,
        reason_codes: vec!["lifecycle.new_current".to_string()],
        signals: Vec::new(),
        remediation: None,
    }
}

fn labeled_snapshot_by_label<'a>(
    entries: &'a [LabeledSnapshot],
    label: &str,
) -> Option<(usize, &'a LabeledSnapshot)> {
    entries
        .iter()
        .enumerate()
        .find(|(_, entry)| entry.label.as_deref() == Some(label))
}

fn lifecycle_state_rank(state: OfflineLifecycleState) -> u8 {
    match state {
        OfflineLifecycleState::Quarantine => 0,
        OfflineLifecycleState::Repair => 1,
        OfflineLifecycleState::MissingCurrent => 2,
        OfflineLifecycleState::NewCurrent => 3,
        OfflineLifecycleState::Active => 4,
    }
}

fn lifecycle_states_in_output_order() -> [OfflineLifecycleState; 5] {
    [
        OfflineLifecycleState::Active,
        OfflineLifecycleState::Repair,
        OfflineLifecycleState::Quarantine,
        OfflineLifecycleState::MissingCurrent,
        OfflineLifecycleState::NewCurrent,
    ]
}

fn lifecycle_state_file_stem(state: OfflineLifecycleState) -> &'static str {
    match state {
        OfflineLifecycleState::Active => "active",
        OfflineLifecycleState::Repair => "repair",
        OfflineLifecycleState::Quarantine => "quarantine",
        OfflineLifecycleState::MissingCurrent => "missing_current",
        OfflineLifecycleState::NewCurrent => "new_current",
    }
}

fn build_lifecycle_state_exports(
    baseline: &[LabeledSnapshot],
    current: &[LabeledSnapshot],
    ledger: &OfflineLifecycleLedger,
) -> Vec<OfflineLifecycleStateExport> {
    lifecycle_states_in_output_order()
        .into_iter()
        .map(|state| {
            let mut entries = ledger
                .entries
                .iter()
                .filter(|entry| entry.state == state)
                .filter_map(|entry| lifecycle_state_export_entry(baseline, current, entry))
                .collect::<Vec<_>>();
            entries.sort_by(|a, b| {
                a.label
                    .cmp(&b.label)
                    .then_with(|| a.before_index.cmp(&b.before_index))
                    .then_with(|| a.after_index.cmp(&b.after_index))
                    .then_with(|| a.identity_id.cmp(&b.identity_id))
            });
            let labels = entries
                .iter()
                .filter_map(|entry| entry.label.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let current_source_count = entries
                .iter()
                .filter(|entry| matches!(entry.source, OfflineLifecycleBaselineSource::Current))
                .count();
            let baseline_source_count = entries
                .iter()
                .filter(|entry| matches!(entry.source, OfflineLifecycleBaselineSource::Baseline))
                .count();
            OfflineLifecycleStateExport {
                state,
                count: entries.len(),
                current_source_count,
                baseline_source_count,
                labels,
                entries,
            }
        })
        .collect()
}

fn lifecycle_state_export_entry(
    baseline: &[LabeledSnapshot],
    current: &[LabeledSnapshot],
    entry: &OfflineLifecycleEntry,
) -> Option<OfflineLifecycleStateExportEntry> {
    let (source, labeled) = if let Some(after_index) = entry.after_index {
        (
            OfflineLifecycleBaselineSource::Current,
            current.get(after_index)?,
        )
    } else {
        let before_index = entry.before_index?;
        (
            OfflineLifecycleBaselineSource::Baseline,
            baseline.get(before_index)?,
        )
    };

    Some(OfflineLifecycleStateExportEntry {
        entry_index: entry.index,
        state: entry.state,
        source,
        label: entry.label.clone().or_else(|| labeled.label.clone()),
        before_index: entry.before_index,
        after_index: entry.after_index,
        identity_id: labeled.snapshot.identity_id(),
        stable_hash: labeled.snapshot.stable_hash(),
        snapshot: labeled.snapshot.clone(),
    })
}

fn lifecycle_state_buckets(exports: &[OfflineLifecycleStateExport]) -> Vec<Value> {
    exports
        .iter()
        .map(|export| {
            json!({
                "state": export.state,
                "count": export.count,
                "currentSourceCount": export.current_source_count,
                "baselineSourceCount": export.baseline_source_count,
                "labels": export.labels.clone(),
            })
        })
        .collect()
}

fn build_lifecycle_next_baseline(
    baseline: &[LabeledSnapshot],
    current: &[LabeledSnapshot],
    ledger: &OfflineLifecycleLedger,
    policy: IdentityLifecycleBaselinePolicy,
) -> OfflineLifecycleNextBaseline {
    let mut entries = Vec::new();
    let mut skipped_states = BTreeSet::new();

    for entry in &ledger.entries {
        let selected = match (policy, entry.state) {
            (IdentityLifecycleBaselinePolicy::Conservative, OfflineLifecycleState::Active) => {
                lifecycle_baseline_entry_from_current(current, entry)
            }
            (IdentityLifecycleBaselinePolicy::Conservative, OfflineLifecycleState::Repair)
            | (
                IdentityLifecycleBaselinePolicy::Conservative,
                OfflineLifecycleState::MissingCurrent,
            ) => lifecycle_baseline_entry_from_baseline(baseline, entry),
            (IdentityLifecycleBaselinePolicy::ActiveOnly, OfflineLifecycleState::Active) => {
                lifecycle_baseline_entry_from_current(current, entry)
            }
            (
                IdentityLifecycleBaselinePolicy::AcceptCurrentRepair,
                OfflineLifecycleState::Active | OfflineLifecycleState::Repair,
            ) => lifecycle_baseline_entry_from_current(current, entry),
            _ => None,
        };

        if let Some(selected) = selected {
            entries.push(selected);
        } else {
            skipped_states.insert(entry.state);
        }
    }

    entries.sort_by(|a, b| {
        a.label
            .cmp(&b.label)
            .then_with(|| a.before_index.cmp(&b.before_index))
            .then_with(|| a.after_index.cmp(&b.after_index))
            .then_with(|| a.identity_id.cmp(&b.identity_id))
    });

    let kept_states = entries
        .iter()
        .map(|entry| entry.state)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let current_source_count = entries
        .iter()
        .filter(|entry| matches!(entry.source, OfflineLifecycleBaselineSource::Current))
        .count();
    let baseline_source_count = entries
        .iter()
        .filter(|entry| matches!(entry.source, OfflineLifecycleBaselineSource::Baseline))
        .count();

    OfflineLifecycleNextBaseline {
        policy,
        count: entries.len(),
        current_source_count,
        baseline_source_count,
        skipped_count: ledger.entry_count.saturating_sub(entries.len()),
        kept_states,
        skipped_states: skipped_states.into_iter().collect(),
        entries,
    }
}

fn lifecycle_baseline_entry_from_current(
    current: &[LabeledSnapshot],
    entry: &OfflineLifecycleEntry,
) -> Option<OfflineLifecycleBaselineEntry> {
    let after_index = entry.after_index?;
    let snapshot = current.get(after_index)?;
    Some(lifecycle_baseline_entry(
        snapshot,
        entry,
        OfflineLifecycleBaselineSource::Current,
    ))
}

fn lifecycle_baseline_entry_from_baseline(
    baseline: &[LabeledSnapshot],
    entry: &OfflineLifecycleEntry,
) -> Option<OfflineLifecycleBaselineEntry> {
    let before_index = entry.before_index?;
    let snapshot = baseline.get(before_index)?;
    Some(lifecycle_baseline_entry(
        snapshot,
        entry,
        OfflineLifecycleBaselineSource::Baseline,
    ))
}

fn lifecycle_baseline_entry(
    labeled: &LabeledSnapshot,
    entry: &OfflineLifecycleEntry,
    source: OfflineLifecycleBaselineSource,
) -> OfflineLifecycleBaselineEntry {
    OfflineLifecycleBaselineEntry {
        entry_index: entry.index,
        label: entry.label.clone().or_else(|| labeled.label.clone()),
        state: entry.state,
        source,
        before_index: entry.before_index,
        after_index: entry.after_index,
        identity_id: labeled.snapshot.identity_id(),
        stable_hash: labeled.snapshot.stable_hash(),
        snapshot: labeled.snapshot.clone(),
    }
}

struct LifecycleRunRecordInput<'a> {
    baseline_path: &'a Path,
    current_path: &'a Path,
    requested_match_by: IdentityDriftMatchMode,
    effective_match_by: IdentityDriftMatchMode,
    baseline_count: usize,
    current_count: usize,
    gate: &'a OfflineLifecycleGateReport,
    ledger: &'a OfflineLifecycleLedger,
    delta: &'a OfflineLifecycleDelta,
    action_queue: &'a OfflineLifecycleActionQueue,
    next_baseline: &'a OfflineLifecycleNextBaseline,
    ledger_out: Option<&'a Value>,
    delta_out: Option<&'a Value>,
    state_out: Option<&'a Value>,
    actions_out: Option<&'a Value>,
    next_baseline_out: Option<&'a Value>,
}

fn build_lifecycle_run_record(input: LifecycleRunRecordInput<'_>) -> OfflineLifecycleRunRecord {
    let generated_at_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let run_id = format!(
        "lifecycle-{}-{}-{}-{}",
        generated_at_unix_seconds,
        std::process::id(),
        input.ledger.entry_count,
        input.delta.change_count
    );
    let mut artifacts = BTreeMap::new();
    if let Some(value) = input.ledger_out {
        artifacts.insert("ledger".to_string(), value.clone());
    }
    if let Some(value) = input.delta_out {
        artifacts.insert("delta".to_string(), value.clone());
    }
    if let Some(value) = input.state_out {
        artifacts.insert("stateOut".to_string(), value.clone());
    }
    if let Some(value) = input.actions_out {
        artifacts.insert("actions".to_string(), value.clone());
    }
    if let Some(value) = input.next_baseline_out {
        artifacts.insert("nextBaseline".to_string(), value.clone());
    }

    OfflineLifecycleRunRecord {
        run_id,
        generated_at_unix_seconds,
        baseline_path: input.baseline_path.display().to_string(),
        current_path: input.current_path.display().to_string(),
        requested_match_by: input.requested_match_by,
        match_by: input.effective_match_by,
        baseline_count: input.baseline_count,
        current_count: input.current_count,
        gate_passed: input.gate.passed,
        gate_failures: input.gate.failures.clone(),
        summary: OfflineLifecycleRunSummary {
            entry_count: input.ledger.entry_count,
            active_count: input.ledger.active_count,
            repair_count: input.ledger.repair_count,
            quarantine_count: input.ledger.quarantine_count,
            missing_current_count: input.ledger.missing_current_count,
            new_current_count: input.ledger.new_current_count,
            changed_count: input.ledger.changed_count,
            high_risk_count: input.ledger.high_risk_count,
            max_drift_score: input.ledger.max_drift_score,
        },
        next_baseline_policy: input.next_baseline.policy,
        next_baseline_count: input.next_baseline.count,
        delta_change_count: input.delta.change_count,
        action_count: input.action_queue.action_count,
        high_priority_action_count: input.action_queue.high_priority_count,
        affected_labels: input.delta.affected_labels.clone(),
        artifacts,
    }
}

fn build_lifecycle_delta(
    ledger: &OfflineLifecycleLedger,
    next_baseline: &OfflineLifecycleNextBaseline,
) -> OfflineLifecycleDelta {
    let kept_by_entry = next_baseline
        .entries
        .iter()
        .map(|entry| (entry.entry_index, entry))
        .collect::<BTreeMap<_, _>>();
    let mut entries = Vec::new();

    for entry in &ledger.entries {
        if let Some(next) = kept_by_entry.get(&entry.index) {
            let change = if next.source == OfflineLifecycleBaselineSource::Current
                && entry
                    .before_stable_hash
                    .as_ref()
                    .is_some_and(|hash| hash != &next.stable_hash)
            {
                OfflineLifecycleDeltaChange::BaselineUpdated
            } else {
                OfflineLifecycleDeltaChange::BaselineRetained
            };
            entries.push(lifecycle_delta_entry(entry, Some(next), change));
            continue;
        }

        if entry.before_index.is_some() {
            entries.push(lifecycle_delta_entry(
                entry,
                None,
                OfflineLifecycleDeltaChange::BaselineRemoved,
            ));
        }
        if entry.after_index.is_some() {
            let change = if entry.state == OfflineLifecycleState::NewCurrent {
                OfflineLifecycleDeltaChange::NewCurrentUnadmitted
            } else {
                OfflineLifecycleDeltaChange::CurrentExcluded
            };
            entries.push(lifecycle_delta_entry(entry, None, change));
        }
    }

    entries.sort_by(|a, b| {
        a.entry_index
            .cmp(&b.entry_index)
            .then_with(|| {
                lifecycle_delta_change_rank(a.change).cmp(&lifecycle_delta_change_rank(b.change))
            })
            .then_with(|| a.label.cmp(&b.label))
    });
    for (change_index, entry) in entries.iter_mut().enumerate() {
        entry.change_index = change_index;
    }

    let affected_labels = entries
        .iter()
        .filter_map(|entry| entry.label.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    OfflineLifecycleDelta {
        baseline_count: ledger.baseline_count,
        current_count: ledger.current_count,
        next_baseline_count: next_baseline.count,
        change_count: entries.len(),
        retained_count: entries
            .iter()
            .filter(|entry| entry.change == OfflineLifecycleDeltaChange::BaselineRetained)
            .count(),
        updated_count: entries
            .iter()
            .filter(|entry| entry.change == OfflineLifecycleDeltaChange::BaselineUpdated)
            .count(),
        removed_count: entries
            .iter()
            .filter(|entry| entry.change == OfflineLifecycleDeltaChange::BaselineRemoved)
            .count(),
        current_excluded_count: entries
            .iter()
            .filter(|entry| entry.change == OfflineLifecycleDeltaChange::CurrentExcluded)
            .count(),
        new_unadmitted_count: entries
            .iter()
            .filter(|entry| entry.change == OfflineLifecycleDeltaChange::NewCurrentUnadmitted)
            .count(),
        affected_labels,
        entries,
    }
}

fn lifecycle_delta_entry(
    entry: &OfflineLifecycleEntry,
    next: Option<&OfflineLifecycleBaselineEntry>,
    change: OfflineLifecycleDeltaChange,
) -> OfflineLifecycleDeltaEntry {
    OfflineLifecycleDeltaEntry {
        change_index: 0,
        entry_index: entry.index,
        change,
        state: entry.state,
        label: entry.label.clone(),
        before_index: entry.before_index,
        after_index: entry.after_index,
        before_id: entry.before_id.clone(),
        after_id: entry.after_id.clone(),
        previous_stable_hash: entry.before_stable_hash.clone(),
        next_stable_hash: next.map(|entry| entry.stable_hash.clone()),
        source: next.map(|entry| entry.source),
        reason_codes: entry.reason_codes.clone(),
    }
}

fn lifecycle_delta_change_rank(change: OfflineLifecycleDeltaChange) -> u8 {
    match change {
        OfflineLifecycleDeltaChange::BaselineRemoved => 0,
        OfflineLifecycleDeltaChange::CurrentExcluded => 1,
        OfflineLifecycleDeltaChange::NewCurrentUnadmitted => 2,
        OfflineLifecycleDeltaChange::BaselineUpdated => 3,
        OfflineLifecycleDeltaChange::BaselineRetained => 4,
    }
}

fn build_lifecycle_action_queue(ledger: &OfflineLifecycleLedger) -> OfflineLifecycleActionQueue {
    let mut actions = Vec::new();

    for entry in &ledger.entries {
        match entry.state {
            OfflineLifecycleState::Quarantine => {
                actions.push(lifecycle_synthetic_action(
                    entry,
                    "lifecycle.quarantine_profile",
                    IdentityDriftRemediationTarget::Admission,
                    IdentityFixPriority::High,
                    "隔离高风险漂移画像",
                    "该 profile 当前画像与基线出现高风险或超阈值漂移,应先隔离当前运行态,再按 drift remediation 修复。",
                ));
            }
            OfflineLifecycleState::MissingCurrent => {
                actions.push(lifecycle_synthetic_action(
                    entry,
                    "lifecycle.investigate_missing_current",
                    IdentityDriftRemediationTarget::Baseline,
                    IdentityFixPriority::Medium,
                    "排查基线画像本轮缺席",
                    "该 profile 存在于基线但当前采样缺失,需要确认账号是否停用、浏览器是否未启动或采样流程是否漏扫。",
                ));
            }
            OfflineLifecycleState::NewCurrent => {
                actions.push(lifecycle_synthetic_action(
                    entry,
                    "lifecycle.review_new_profile",
                    IdentityDriftRemediationTarget::Admission,
                    IdentityFixPriority::Medium,
                    "审核新出现的当前画像",
                    "该 profile 本轮出现但基线中不存在,应先进入候选入池流程,通过 identity-pool admission 后再写入基线。",
                ));
            }
            OfflineLifecycleState::Active | OfflineLifecycleState::Repair => {}
        }

        if let Some(remediation) = &entry.remediation {
            for action in &remediation.actions {
                actions.push(OfflineLifecycleActionEntry {
                    action_index: 0,
                    entry_index: entry.index,
                    source: OfflineLifecycleActionSource::DriftRemediation,
                    state: entry.state,
                    label: entry.label.clone(),
                    before_index: entry.before_index,
                    after_index: entry.after_index,
                    before_id: entry.before_id.clone(),
                    after_id: entry.after_id.clone(),
                    drift_score: entry.drift_score,
                    high_risk: entry.high_risk,
                    action_code: action.code.clone(),
                    target: action.target,
                    priority: action.priority,
                    title: action.title.clone(),
                    detail: action.detail.clone(),
                    fields: action.fields.clone(),
                    signal_codes: action.signal_codes.clone(),
                    before_values: action.before_values.clone(),
                    after_values: action.after_values.clone(),
                    reason_codes: entry.reason_codes.clone(),
                });
            }
        }
    }

    actions.sort_by(|a, b| {
        a.entry_index
            .cmp(&b.entry_index)
            .then_with(|| {
                lifecycle_action_source_rank(a.source).cmp(&lifecycle_action_source_rank(b.source))
            })
            .then_with(|| {
                fix_priority_rank_for_queue(b.priority)
                    .cmp(&fix_priority_rank_for_queue(a.priority))
            })
            .then_with(|| a.action_code.cmp(&b.action_code))
    });
    for (action_index, action) in actions.iter_mut().enumerate() {
        action.action_index = action_index;
    }

    let labels = actions
        .iter()
        .filter_map(|action| action.label.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let lifecycle_action_count = actions
        .iter()
        .filter(|action| matches!(action.source, OfflineLifecycleActionSource::Lifecycle))
        .count();
    let remediation_action_count = actions
        .iter()
        .filter(|action| {
            matches!(
                action.source,
                OfflineLifecycleActionSource::DriftRemediation
            )
        })
        .count();

    OfflineLifecycleActionQueue {
        entry_count: ledger.entry_count,
        action_count: actions.len(),
        lifecycle_action_count,
        remediation_action_count,
        high_priority_count: actions
            .iter()
            .filter(|action| action.priority == IdentityFixPriority::High)
            .count(),
        labels,
        actions,
    }
}

fn lifecycle_synthetic_action(
    entry: &OfflineLifecycleEntry,
    code: &str,
    target: IdentityDriftRemediationTarget,
    priority: IdentityFixPriority,
    title: &str,
    detail: &str,
) -> OfflineLifecycleActionEntry {
    OfflineLifecycleActionEntry {
        action_index: 0,
        entry_index: entry.index,
        source: OfflineLifecycleActionSource::Lifecycle,
        state: entry.state,
        label: entry.label.clone(),
        before_index: entry.before_index,
        after_index: entry.after_index,
        before_id: entry.before_id.clone(),
        after_id: entry.after_id.clone(),
        drift_score: entry.drift_score,
        high_risk: entry.high_risk,
        action_code: code.to_string(),
        target,
        priority,
        title: title.to_string(),
        detail: detail.to_string(),
        fields: Vec::new(),
        signal_codes: entry
            .signals
            .iter()
            .map(|signal| signal.code.clone())
            .collect(),
        before_values: Vec::new(),
        after_values: Vec::new(),
        reason_codes: entry.reason_codes.clone(),
    }
}

fn lifecycle_action_source_rank(source: OfflineLifecycleActionSource) -> u8 {
    match source {
        OfflineLifecycleActionSource::Lifecycle => 0,
        OfflineLifecycleActionSource::DriftRemediation => 1,
    }
}

fn build_pool_action_queue(
    snapshots: &[FingerprintSnapshot],
    remediation: &IdentityPoolRemediationPlan,
    capacity: &IdentityCapacityPlan,
    ledger: &OfflineLedgerReport,
) -> OfflinePoolActionQueue {
    let snapshot_ids = identity_ids(snapshots);
    let mut actions = Vec::new();

    for action in &remediation.actions {
        actions.push(OfflinePoolActionEntry {
            action_index: 0,
            source: OfflinePoolActionSource::Remediation,
            action_code: action.code.clone(),
            target: action.target,
            priority: action.priority,
            title: action.title.clone(),
            detail: action.detail.clone(),
            identity_ids: ids_for_indexes(&snapshot_ids, &action.indexes),
            indexes: action.indexes.clone(),
            affected_count: action.affected_count,
            pair_count: action.pair_count,
            signal_codes: action.signal_codes.clone(),
            values: action.values.clone(),
            reasons: Vec::new(),
            decision: None,
            accepted: None,
            internal_linked_indexes: Vec::new(),
            internal_linked_ids: Vec::new(),
            baseline_linked_indexes: Vec::new(),
            baseline_linked_ids: Vec::new(),
            max_internal_linkability: None,
            max_baseline_linkability: None,
            estimated_gain: None,
        });
    }

    for action in &capacity.actions {
        actions.push(OfflinePoolActionEntry {
            action_index: 0,
            source: OfflinePoolActionSource::Capacity,
            action_code: action.code.clone(),
            target: capacity_action_target(&action.code),
            priority: action.priority,
            title: action.title.clone(),
            detail: action.detail.clone(),
            indexes: Vec::new(),
            identity_ids: Vec::new(),
            affected_count: capacity.additional_distinct_profiles_needed,
            pair_count: 0,
            signal_codes: action.signal_codes.clone(),
            values: Vec::new(),
            reasons: vec![
                format!(
                    "missing_effective_identity_count={:.2}",
                    capacity.missing_effective_identity_count
                ),
                format!(
                    "nominal_to_effective_ratio={:.2}",
                    capacity.nominal_to_effective_ratio
                ),
            ],
            decision: None,
            accepted: None,
            internal_linked_indexes: Vec::new(),
            internal_linked_ids: Vec::new(),
            baseline_linked_indexes: Vec::new(),
            baseline_linked_ids: Vec::new(),
            max_internal_linkability: None,
            max_baseline_linkability: None,
            estimated_gain: Some(action.estimated_gain),
        });
    }

    for entry in ledger.entries.iter().filter(|entry| !entry.accepted) {
        let mut values = BTreeSet::new();
        values.extend(entry.internal_linked_ids.iter().cloned());
        values.extend(entry.baseline_linked_ids.iter().cloned());
        actions.push(OfflinePoolActionEntry {
            action_index: 0,
            source: OfflinePoolActionSource::Admission,
            action_code: pool_admission_action_code(entry).to_string(),
            target: IdentityPoolRemediationTarget::Admission,
            priority: pool_admission_priority(entry),
            title: pool_admission_title(entry).to_string(),
            detail: format!(
                "候选画像未通过 admission,原因: {}。",
                entry.reasons.join(", ")
            ),
            indexes: vec![entry.index],
            identity_ids: vec![entry.identity_id.clone()],
            affected_count: 1,
            pair_count: entry.internal_linked_indexes.len() + entry.baseline_linked_indexes.len(),
            signal_codes: entry.signal_codes.clone(),
            values: values.into_iter().collect(),
            reasons: entry.reasons.clone(),
            decision: Some(entry.decision),
            accepted: Some(entry.accepted),
            internal_linked_indexes: entry.internal_linked_indexes.clone(),
            internal_linked_ids: entry.internal_linked_ids.clone(),
            baseline_linked_indexes: entry.baseline_linked_indexes.clone(),
            baseline_linked_ids: entry.baseline_linked_ids.clone(),
            max_internal_linkability: Some(entry.max_internal_linkability),
            max_baseline_linkability: Some(entry.max_baseline_linkability),
            estimated_gain: None,
        });
    }

    actions.sort_by(|a, b| {
        fix_priority_rank_for_queue(b.priority)
            .cmp(&fix_priority_rank_for_queue(a.priority))
            .then_with(|| pool_action_source_rank(a.source).cmp(&pool_action_source_rank(b.source)))
            .then_with(|| a.indexes.cmp(&b.indexes))
            .then_with(|| a.action_code.cmp(&b.action_code))
    });
    for (action_index, action) in actions.iter_mut().enumerate() {
        action.action_index = action_index;
    }

    let affected_identity_ids = actions
        .iter()
        .flat_map(|action| action.identity_ids.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let admission_action_count = actions
        .iter()
        .filter(|action| matches!(action.source, OfflinePoolActionSource::Admission))
        .count();
    let remediation_action_count = actions
        .iter()
        .filter(|action| matches!(action.source, OfflinePoolActionSource::Remediation))
        .count();
    let capacity_action_count = actions
        .iter()
        .filter(|action| matches!(action.source, OfflinePoolActionSource::Capacity))
        .count();

    OfflinePoolActionQueue {
        snapshot_count: snapshots.len(),
        action_count: actions.len(),
        admission_action_count,
        remediation_action_count,
        capacity_action_count,
        high_priority_count: actions
            .iter()
            .filter(|action| action.priority == IdentityFixPriority::High)
            .count(),
        quarantine_count: ledger.quarantine_count,
        affected_identity_ids,
        actions,
    }
}

fn capacity_action_target(code: &str) -> IdentityPoolRemediationTarget {
    if code.contains("canvas") {
        IdentityPoolRemediationTarget::Canvas
    } else if code.contains("webgl") {
        IdentityPoolRemediationTarget::GpuWebgl
    } else if code.contains("locale") || code.contains("proxy") {
        IdentityPoolRemediationTarget::LocaleProxy
    } else if code.contains("browser") || code.contains("persona") {
        IdentityPoolRemediationTarget::UserAgent
    } else if code.contains("device") || code.contains("hardware") {
        IdentityPoolRemediationTarget::Hardware
    } else {
        IdentityPoolRemediationTarget::Admission
    }
}

fn pool_admission_action_code(entry: &OfflineLedgerEntry) -> &'static str {
    if entry.known_in_baseline {
        "pool.quarantine_baseline_collision"
    } else if entry.duplicate_in_batch {
        "pool.quarantine_duplicate_candidate"
    } else {
        "pool.quarantine_candidate"
    }
}

fn pool_admission_title(entry: &OfflineLedgerEntry) -> &'static str {
    if entry.known_in_baseline {
        "隔离撞到基线的候选画像"
    } else if entry.duplicate_in_batch {
        "隔离批内重复候选画像"
    } else {
        "隔离未通过入池门禁的候选画像"
    }
}

fn pool_admission_priority(entry: &OfflineLedgerEntry) -> IdentityFixPriority {
    let max_linkability = entry
        .max_internal_linkability
        .max(entry.max_baseline_linkability);
    if entry.known_in_baseline || entry.duplicate_in_batch || max_linkability >= 60 {
        IdentityFixPriority::High
    } else if max_linkability >= 30 || !entry.signal_codes.is_empty() {
        IdentityFixPriority::Medium
    } else {
        IdentityFixPriority::Low
    }
}

fn pool_action_source_rank(source: OfflinePoolActionSource) -> u8 {
    match source {
        OfflinePoolActionSource::Admission => 0,
        OfflinePoolActionSource::Remediation => 1,
        OfflinePoolActionSource::Capacity => 2,
    }
}

fn fix_priority_rank_for_queue(priority: IdentityFixPriority) -> u8 {
    match priority {
        IdentityFixPriority::Low => 0,
        IdentityFixPriority::Medium => 1,
        IdentityFixPriority::High => 2,
    }
}

fn evaluate_drift_gate(
    max_drift_score: Option<u8>,
    fail_on_high_risk_drift: bool,
    entries: &[OfflineDriftEntry],
) -> OfflineDriftGateReport {
    let criteria = OfflineDriftGateCriteria {
        max_drift_score,
        fail_on_high_risk_drift,
    };
    let mut failures = Vec::new();
    if let Some(max_drift_score) = max_drift_score {
        let offenders = entries
            .iter()
            .filter_map(|entry| (entry.score > max_drift_score).then_some(entry.index))
            .collect::<Vec<_>>();
        if !offenders.is_empty() {
            failures.push(format!(
                "identity_drift_score_above_max: indexes {:?} above {}",
                offenders, max_drift_score
            ));
        }
    }
    if fail_on_high_risk_drift {
        let offenders = entries
            .iter()
            .filter_map(|entry| entry.high_risk.then_some(entry.index))
            .collect::<Vec<_>>();
        if !offenders.is_empty() {
            failures.push(format!("high_risk_identity_drift: indexes {:?}", offenders));
        }
    }

    OfflineDriftGateReport {
        passed: failures.is_empty(),
        criteria,
        failures,
    }
}

fn evaluate_lifecycle_gate(
    max_drift_score: Option<u8>,
    fail_on_high_risk_drift: bool,
    fail_on_missing_current: bool,
    fail_on_new_current: bool,
    ledger: &OfflineLifecycleLedger,
) -> OfflineLifecycleGateReport {
    let criteria = OfflineLifecycleGateCriteria {
        max_drift_score,
        fail_on_high_risk_drift,
        fail_on_missing_current,
        fail_on_new_current,
    };
    let mut failures = Vec::new();

    if let Some(max_drift_score) = max_drift_score {
        let offenders = ledger
            .entries
            .iter()
            .filter_map(|entry| {
                (entry.drift_score.unwrap_or(0) > max_drift_score).then_some(entry.index)
            })
            .collect::<Vec<_>>();
        if !offenders.is_empty() {
            failures.push(format!(
                "lifecycle_drift_score_above_max: indexes {:?} above {}",
                offenders, max_drift_score
            ));
        }
    }
    if fail_on_high_risk_drift {
        let offenders = ledger
            .entries
            .iter()
            .filter_map(|entry| (entry.high_risk == Some(true)).then_some(entry.index))
            .collect::<Vec<_>>();
        if !offenders.is_empty() {
            failures.push(format!(
                "lifecycle_high_risk_drift: indexes {:?}",
                offenders
            ));
        }
    }
    if fail_on_missing_current && ledger.missing_current_count > 0 {
        failures.push(format!(
            "lifecycle_missing_current_profiles: {} missing",
            ledger.missing_current_count
        ));
    }
    if fail_on_new_current && ledger.new_current_count > 0 {
        failures.push(format!(
            "lifecycle_new_current_profiles: {} new",
            ledger.new_current_count
        ));
    }

    OfflineLifecycleGateReport {
        passed: failures.is_empty(),
        criteria,
        failures,
    }
}

fn build_offline_admission(
    candidates: &[FingerprintSnapshot],
    internal_quarantine: &[usize],
    against: Option<&BaselineCompareReport>,
) -> OfflineAdmissionPlan {
    let total_count = candidates.len();
    let candidate_ids = identity_ids(candidates);
    let mut quarantine_set: BTreeSet<_> = internal_quarantine
        .iter()
        .copied()
        .filter(|index| *index < total_count)
        .collect();
    if let Some(against) = against {
        quarantine_set.extend(
            against
                .candidate_quarantine
                .candidate_indexes
                .iter()
                .copied()
                .filter(|index| *index < total_count),
        );
    }

    let accept_indexes = (0..total_count)
        .filter(|index| !quarantine_set.contains(index))
        .collect::<Vec<_>>();
    let quarantine_indexes = quarantine_set.into_iter().collect::<Vec<_>>();
    let action = if quarantine_indexes.is_empty() {
        IdentityAdmissionAction::Accept
    } else if accept_indexes.is_empty() {
        IdentityAdmissionAction::RejectAll
    } else {
        IdentityAdmissionAction::PartialQuarantine
    };

    OfflineAdmissionPlan {
        action,
        total_count,
        accept_count: accept_indexes.len(),
        quarantine_count: quarantine_indexes.len(),
        accept_ids: ids_for_indexes(&candidate_ids, &accept_indexes),
        quarantine_ids: ids_for_indexes(&candidate_ids, &quarantine_indexes),
        accept_indexes,
        quarantine_indexes,
    }
}

fn build_offline_ledger(
    candidates: &[FingerprintSnapshot],
    report: &IdentityPoolReport,
    against: Option<&BaselineCompareReport>,
    admission: &OfflineAdmissionPlan,
) -> OfflineLedgerReport {
    let candidate_ids = identity_ids(candidates);
    let baseline_ids = against
        .map(|against| against.baseline_ids.clone())
        .unwrap_or_default();
    let baseline_id_set: BTreeSet<_> = baseline_ids.iter().cloned().collect();
    let mut candidate_id_counts: BTreeMap<String, usize> = BTreeMap::new();
    for id in &candidate_ids {
        *candidate_id_counts.entry(id.clone()).or_default() += 1;
    }
    let accept_set: BTreeSet<_> = admission.accept_indexes.iter().copied().collect();
    let quarantine_set: BTreeSet<_> = admission.quarantine_indexes.iter().copied().collect();

    let mut entries = Vec::with_capacity(candidates.len());
    for (index, identity_id) in candidate_ids.iter().enumerate() {
        let accepted = accept_set.contains(&index) && !quarantine_set.contains(&index);
        let known_in_baseline = baseline_id_set.contains(identity_id);
        let duplicate_in_batch = candidate_id_counts.get(identity_id).copied().unwrap_or(0) > 1;

        let mut internal_linked_indexes = BTreeSet::new();
        let mut internal_linked_ids = BTreeSet::new();
        let mut baseline_linked_indexes = BTreeSet::new();
        let mut baseline_linked_ids = BTreeSet::new();
        let mut signal_codes = BTreeSet::new();
        let mut max_internal_linkability = 0u8;
        let mut max_baseline_linkability = 0u8;

        for pair in &report.risky_pairs {
            let linked = if pair.left_index == index {
                Some(pair.right_index)
            } else if pair.right_index == index {
                Some(pair.left_index)
            } else {
                None
            };
            if let Some(linked) = linked {
                internal_linked_indexes.insert(linked);
                if let Some(id) = candidate_ids.get(linked) {
                    internal_linked_ids.insert(id.clone());
                }
                max_internal_linkability = max_internal_linkability.max(pair.score);
                for signal in &pair.signals {
                    signal_codes.insert(signal.code.clone());
                }
            }
        }

        if let Some(against) = against {
            for pair in against
                .risky_pairs
                .iter()
                .filter(|pair| pair.candidate_index == index)
            {
                baseline_linked_indexes.insert(pair.baseline_index);
                baseline_linked_ids.insert(pair.baseline_id.clone());
                max_baseline_linkability = max_baseline_linkability.max(pair.score);
                for signal in &pair.signals {
                    signal_codes.insert(signal.code.clone());
                }
            }
        }

        let mut reasons = Vec::new();
        if accepted {
            reasons.push("admission.accept".to_string());
        } else {
            reasons.push("admission.quarantine".to_string());
        }
        if known_in_baseline {
            reasons.push("known_in_baseline".to_string());
        }
        if duplicate_in_batch {
            reasons.push("duplicate_in_batch".to_string());
        }
        if !internal_linked_indexes.is_empty() {
            reasons.push("internal_linkability".to_string());
        }
        if !baseline_linked_indexes.is_empty() {
            reasons.push("baseline_linkability".to_string());
        }

        entries.push(OfflineLedgerEntry {
            index,
            identity_id: identity_id.clone(),
            decision: if accepted {
                OfflineLedgerDecision::Accept
            } else {
                OfflineLedgerDecision::Quarantine
            },
            accepted,
            known_in_baseline,
            duplicate_in_batch,
            internal_linked_indexes: internal_linked_indexes.into_iter().collect(),
            internal_linked_ids: internal_linked_ids.into_iter().collect(),
            baseline_linked_indexes: baseline_linked_indexes.into_iter().collect(),
            baseline_linked_ids: baseline_linked_ids.into_iter().collect(),
            max_internal_linkability,
            max_baseline_linkability,
            signal_codes: signal_codes.into_iter().collect(),
            reasons,
        });
    }

    OfflineLedgerReport {
        candidate_count: entries.len(),
        accepted_count: entries.iter().filter(|entry| entry.accepted).count(),
        quarantine_count: entries.iter().filter(|entry| !entry.accepted).count(),
        known_baseline_count: entries
            .iter()
            .filter(|entry| entry.known_in_baseline)
            .count(),
        duplicate_candidate_count: entries
            .iter()
            .filter(|entry| entry.duplicate_in_batch)
            .count(),
        risky_internal_count: entries
            .iter()
            .filter(|entry| !entry.internal_linked_indexes.is_empty())
            .count(),
        risky_baseline_count: entries
            .iter()
            .filter(|entry| !entry.baseline_linked_indexes.is_empty())
            .count(),
        entries,
    }
}

fn parse_snapshots(text: &str) -> Result<Vec<FingerprintSnapshot>> {
    match serde_json::from_str::<Value>(text) {
        Ok(value) => snapshots_from_value(&value),
        Err(json_error) => parse_ndjson_snapshots(text)
            .with_context(|| format!("whole-file JSON parse error: {json_error}")),
    }
}

fn parse_labeled_snapshots(text: &str) -> Result<Vec<LabeledSnapshot>> {
    match serde_json::from_str::<Value>(text) {
        Ok(value) => labeled_snapshots_from_value(&value),
        Err(json_error) => parse_ndjson_labeled_snapshots(text)
            .with_context(|| format!("whole-file JSON parse error: {json_error}")),
    }
}

fn snapshots_from_value(value: &Value) -> Result<Vec<FingerprintSnapshot>> {
    if let Some(array) = value.as_array() {
        let mut out = Vec::new();
        for item in array {
            let mut parsed = snapshots_from_value(item)?;
            out.append(&mut parsed);
        }
        return Ok(out);
    }

    if let Ok(snapshot) = serde_json::from_value::<FingerprintSnapshot>(value.clone()) {
        return Ok(vec![snapshot]);
    }

    let Some(object) = value.as_object() else {
        bail!("expected a snapshot object, snapshot array, or object containing snapshots");
    };

    if let Some(snapshots) = object.get("snapshots") {
        return snapshots_from_value(snapshots);
    }
    if let Some(data) = object.get("data") {
        return snapshots_from_value(data);
    }
    if let Some(report) = object.get("report") {
        return snapshots_from_report(report);
    }
    if let Some(snapshot) = object.get("snapshot") {
        return snapshots_from_value(snapshot);
    }

    bail!("no snapshots, data, report, or snapshot field found")
}

fn snapshots_from_report(value: &Value) -> Result<Vec<FingerprintSnapshot>> {
    if let Ok(pool) = serde_json::from_value::<IdentityPoolReport>(value.clone()) {
        let snapshots = pool
            .identity_reports
            .into_iter()
            .map(|report| report.snapshot)
            .collect::<Vec<_>>();
        if !snapshots.is_empty() {
            return Ok(snapshots);
        }
    }

    if let Ok(identity) = serde_json::from_value::<IdentityReport>(value.clone()) {
        return Ok(vec![identity.snapshot]);
    }

    if let Ok(linkability) = serde_json::from_value::<LinkabilityReport>(value.clone()) {
        return Ok(vec![linkability.left, linkability.right]);
    }

    snapshots_from_value(value)
}

fn labeled_snapshots_from_value(value: &Value) -> Result<Vec<LabeledSnapshot>> {
    if let Some(array) = value.as_array() {
        let mut out = Vec::new();
        for item in array {
            let mut parsed = labeled_snapshots_from_value(item)?;
            out.append(&mut parsed);
        }
        return Ok(out);
    }

    let label = label_from_value(value);
    if let Ok(snapshot) = serde_json::from_value::<FingerprintSnapshot>(value.clone()) {
        return Ok(vec![LabeledSnapshot { label, snapshot }]);
    }

    let Some(object) = value.as_object() else {
        bail!("expected a snapshot object, snapshot array, or object containing snapshots");
    };

    if let Some(snapshot) = object.get("snapshot") {
        if let Ok(snapshot) = serde_json::from_value::<FingerprintSnapshot>(snapshot.clone()) {
            return Ok(vec![LabeledSnapshot { label, snapshot }]);
        }
        let mut parsed = labeled_snapshots_from_value(snapshot)?;
        apply_outer_label(&mut parsed, label);
        return Ok(parsed);
    }
    if let Some(snapshots) = object.get("snapshots") {
        let mut parsed = labeled_snapshots_from_value(snapshots)?;
        apply_outer_label(&mut parsed, label);
        return Ok(parsed);
    }
    if let Some(data) = object.get("data") {
        let mut parsed = labeled_snapshots_from_value(data)?;
        apply_outer_label(&mut parsed, label);
        return Ok(parsed);
    }
    if let Some(report) = object.get("report") {
        let mut parsed = labeled_snapshots_from_report(report)?;
        apply_outer_label(&mut parsed, label);
        return Ok(parsed);
    }

    bail!("no snapshots, data, report, or snapshot field found")
}

fn labeled_snapshots_from_report(value: &Value) -> Result<Vec<LabeledSnapshot>> {
    if let Ok(pool) = serde_json::from_value::<IdentityPoolReport>(value.clone()) {
        let snapshots = pool
            .identity_reports
            .into_iter()
            .map(|report| LabeledSnapshot {
                label: None,
                snapshot: report.snapshot,
            })
            .collect::<Vec<_>>();
        if !snapshots.is_empty() {
            return Ok(snapshots);
        }
    }

    if let Ok(identity) = serde_json::from_value::<IdentityReport>(value.clone()) {
        return Ok(vec![LabeledSnapshot {
            label: label_from_value(value),
            snapshot: identity.snapshot,
        }]);
    }

    if let Ok(linkability) = serde_json::from_value::<LinkabilityReport>(value.clone()) {
        return Ok(vec![
            LabeledSnapshot {
                label: None,
                snapshot: linkability.left,
            },
            LabeledSnapshot {
                label: None,
                snapshot: linkability.right,
            },
        ]);
    }

    labeled_snapshots_from_value(value)
}

fn apply_outer_label(entries: &mut [LabeledSnapshot], label: Option<String>) {
    if entries.len() == 1 && entries[0].label.is_none() {
        entries[0].label = label;
    }
}

fn label_from_value(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    for key in [
        "accountId",
        "account_id",
        "profileId",
        "profile_id",
        "identityKey",
        "identity_key",
        "label",
        "id",
        "name",
        "key",
    ] {
        if let Some(label) = object.get(key).and_then(label_value_to_string) {
            return Some(label);
        }
    }
    None
}

fn label_value_to_string(value: &Value) -> Option<String> {
    let label = match value {
        Value::String(value) => value.trim().to_string(),
        Value::Number(value) => value.to_string(),
        _ => return None,
    };
    (!label.is_empty()).then_some(label)
}

fn parse_ndjson_snapshots(text: &str) -> Result<Vec<FingerprintSnapshot>> {
    let mut snapshots = Vec::new();
    for (line_index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value = serde_json::from_str::<Value>(line)
            .with_context(|| format!("invalid JSON at line {}", line_index + 1))?;
        let mut parsed = snapshots_from_value(&value)
            .with_context(|| format!("invalid snapshot at line {}", line_index + 1))?;
        snapshots.append(&mut parsed);
    }

    if snapshots.is_empty() {
        bail!("no snapshots found in NDJSON input");
    }
    Ok(snapshots)
}

fn parse_ndjson_labeled_snapshots(text: &str) -> Result<Vec<LabeledSnapshot>> {
    let mut snapshots = Vec::new();
    for (line_index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value = serde_json::from_str::<Value>(line)
            .with_context(|| format!("invalid JSON at line {}", line_index + 1))?;
        let mut parsed = labeled_snapshots_from_value(&value)
            .with_context(|| format!("invalid snapshot at line {}", line_index + 1))?;
        snapshots.append(&mut parsed);
    }

    if snapshots.is_empty() {
        bail!("no snapshots found in NDJSON input");
    }
    Ok(snapshots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_snapshot_array_and_camel_probe_fields() {
        let snapshots = parse_snapshots(
            r#"[{
                "ua":"Mozilla/5.0",
                "platform":"MacIntel",
                "uaDataPlatform":"macOS",
                "uaDataMobile":false,
                "webdriver":false,
                "languages":"en-US,en",
                "maxTouchPoints":0,
                "hardwareConcurrency":8,
                "deviceMemory":8,
                "screen":"1440x900",
                "devicePixelRatio":2,
                "timezone":"America/Los_Angeles",
                "webglRenderer":"ANGLE (Apple, Apple M2)",
                "canvasHash":"abc12345"
            }]"#,
        )
        .unwrap();

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].ua_data_platform, "macOS");
        assert_eq!(snapshots[0].hardware_concurrency, 8);
        assert_eq!(snapshots[0].canvas_hash, "abc12345");
    }

    #[test]
    fn parses_drs_identity_pool_output() {
        let raw = r#"{
            "ok": true,
            "data": {
                "scope": "pool",
                "report": {
                    "size": 1,
                    "max_linkability": 0,
                    "identity_reports": [{
                        "score": 100,
                        "snapshot": {
                            "ua":"Mozilla/5.0",
                            "platform":"Win32",
                            "ua_data_platform":"Windows",
                            "ua_data_mobile":false,
                            "webdriver":false,
                            "languages":"en-US,en",
                            "max_touch_points":0,
                            "hardware_concurrency":8,
                            "device_memory":8.0,
                            "screen":"1920x1080",
                            "device_pixel_ratio":1.0,
                            "timezone":"America/New_York",
                            "webgl_renderer":"ANGLE (NVIDIA)",
                            "canvas_hash":"def67890"
                        },
                        "issues": []
                    }],
                    "risky_pairs": [],
                    "duplicate_signals": []
                }
            }
        }"#;

        let snapshots = parse_snapshots(raw).unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].platform, "Win32");
    }

    #[test]
    fn parses_ndjson_snapshots() {
        let raw = r#"{"ua":"A","platform":"Win32","languages":"en-US","hardware_concurrency":8,"device_memory":8.0,"screen":"1920x1080","device_pixel_ratio":1.0,"timezone":"UTC","webgl_renderer":"GPU A","canvas_hash":"aaa"}
{"ua":"B","platform":"MacIntel","languages":"en-US","hardware_concurrency":8,"device_memory":8.0,"screen":"1440x900","device_pixel_ratio":2.0,"timezone":"UTC","webgl_renderer":"GPU B","canvas_hash":"bbb"}"#;

        let snapshots = parse_snapshots(raw).unwrap();
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[1].platform, "MacIntel");
    }

    #[test]
    fn identity_policy_merges_defaults_and_cli_overrides() {
        let spec = parse_identity_policy(
            r#"{
                "gatePreset": "balanced",
                "minScore": 75,
                "minEntropyScore": 45,
                "gate": {
                    "gatePreset": "strict",
                    "maxLinkability": 40,
                    "minEffectiveIdentities": 6,
                    "maxNominalToEffectiveRatio": 3,
                    "failOnRiskyPairs": false
                },
                "drift": {
                    "maxDriftScore": 30,
                    "matchBy": "label",
                    "failOnHighRiskDrift": false
                },
                "lifecycle": {
                    "maxDriftScore": 20,
                    "failOnMissingCurrent": true,
                    "failOnNewCurrent": true,
                    "nextBaselinePolicy": "active-only"
                },
                "health": {
                    "windowSeconds": 86400,
                    "repairThreshold": 2,
                    "quarantineThreshold": 4,
                    "cooldownSeconds": 1800
                },
                "job": {
                    "desiredConcurrency": 4,
                    "limit": 3,
                    "leaseSeconds": 1200,
                    "maxWaitSeconds": 60,
                    "allowWait": true,
                    "perAsset": true,
                    "childConcurrency": 2,
                    "runtimeRenewIntervalSeconds": 30,
                    "childTimeoutSeconds": 180,
                    "childResultDir": "child-results",
                    "maxFailedAssets": 1,
                    "maxFailedAssetsPerReason": 2,
                    "allowState": ["active", "repair"],
                    "includeRetry": true,
                    "runtimeGraceSeconds": 5,
                    "dispatchGraceSeconds": 6,
                    "cooldownGraceSeconds": 7,
                    "failureCooldownSeconds": 600,
                    "failureNextState": "repair",
                    "runtimeRiskLedgers": ["runtime-risk.ndjson"],
                    "runtimeRiskWindowSeconds": 900,
                    "runtimeRiskOut": "runtime-risk.ndjson",
                    "appendRuntimeRisk": true,
                    "failureReasonRules": {
                        "Rate Limited": {
                            "cooldownSeconds": 900,
                            "nextState": "repair",
                            "recommendedAction": "pause_failure_reason",
                            "runtimeRiskSeverity": "critical",
                            "nextSuggestedLimit": 0,
                            "nextSuggestedDesiredConcurrency": 0,
                            "runtimeRiskMessage": "pause rate-limited publish jobs",
                            "runtimeRiskCooldownSeconds": 1800
                        },
                        "risk_control": {
                            "cooldownSeconds": 3600,
                            "nextState": "quarantine"
                        }
                    }
                }
            }"#,
        )
        .unwrap();
        let policy = LoadedIdentityPolicy {
            path: PathBuf::from("identity-policy.json"),
            spec,
        };

        let gate = policy.merge_gate(IdentityGate {
            max_linkability: Some(25),
            min_entropy_score: Some(80),
            fail_on_risky_pairs: true,
            ..IdentityGate::default()
        });
        assert_eq!(
            gate.preset,
            Some(crate::protocol::IdentityGatePreset::Strict)
        );
        assert_eq!(gate.min_score, Some(75));
        assert_eq!(gate.max_linkability, Some(25));
        assert_eq!(gate.min_entropy_score, Some(80));
        assert_eq!(gate.min_effective_identity_count, Some(6.0));
        assert_eq!(gate.max_nominal_to_effective_ratio, Some(3.0));
        assert!(gate.fail_on_risky_pairs);

        let drift = policy.merge_drift(None, false, None);
        assert_eq!(drift.max_drift_score, Some(30));
        assert!(!drift.fail_on_high_risk_drift);
        assert_eq!(drift.match_by, IdentityDriftMatchMode::Label);

        let drift_override =
            policy.merge_drift(Some(10), true, Some(IdentityDriftMatchMode::Index));
        assert_eq!(drift_override.max_drift_score, Some(10));
        assert!(drift_override.fail_on_high_risk_drift);
        assert_eq!(drift_override.match_by, IdentityDriftMatchMode::Index);

        let lifecycle = policy.merge_lifecycle(None, false, false, false, None, None);
        assert_eq!(lifecycle.max_drift_score, Some(20));
        assert_eq!(lifecycle.match_by, IdentityDriftMatchMode::Label);
        assert!(lifecycle.fail_on_missing_current);
        assert!(lifecycle.fail_on_new_current);
        assert_eq!(
            lifecycle.next_baseline_policy,
            IdentityLifecycleBaselinePolicy::ActiveOnly
        );

        let lifecycle_override = policy.merge_lifecycle(
            None,
            false,
            false,
            false,
            None,
            Some(IdentityLifecycleBaselinePolicy::AcceptCurrentRepair),
        );
        assert_eq!(
            lifecycle_override.next_baseline_policy,
            IdentityLifecycleBaselinePolicy::AcceptCurrentRepair
        );

        let health = policy.merge_health(None, None, None, None);
        assert_eq!(health.window_seconds, Some(86400));
        assert_eq!(health.repair_threshold, 2);
        assert_eq!(health.quarantine_threshold, 4);
        assert_eq!(health.cooldown_seconds, 1800);

        let health_override = policy.merge_health(Some(60), Some(3), Some(6), Some(300));
        assert_eq!(health_override.window_seconds, Some(60));
        assert_eq!(health_override.repair_threshold, 3);
        assert_eq!(health_override.quarantine_threshold, 6);
        assert_eq!(health_override.cooldown_seconds, 300);

        let job = policy.merge_job_run(IdentityJobRunOptions {
            asset_manifest: PathBuf::from("profile-assets.json"),
            policy: Some(PathBuf::from("identity-policy.json")),
            job_preset: None,
            desired_concurrency: None,
            limit: None,
            worker: None,
            job: None,
            lease_seconds: None,
            max_wait_seconds: None,
            allow_wait: false,
            per_asset: false,
            child_concurrency: None,
            runtime_renew_interval_seconds: None,
            child_timeout_seconds: None,
            child_result_dir: None,
            max_failed_assets: None,
            max_failed_assets_per_reason: None,
            allow_states: Vec::new(),
            include_dispatch_leased: false,
            include_retry: false,
            include_failed: false,
            include_cancelled: false,
            include_runtime_leased: false,
            include_missing_profile_dir: false,
            skip_sweep: false,
            skip_validate: false,
            runtime_grace_seconds: None,
            dispatch_grace_seconds: None,
            cooldown_grace_seconds: None,
            failure_cooldown_seconds: None,
            failure_next_state: None,
            failure_reason_rules: BTreeMap::new(),
            asset_manifest_out: None,
            sweep_out: None,
            validate_out: None,
            gate_out: None,
            selection_out: None,
            release_out: None,
            append_release: false,
            runtime_risk_ledgers: Vec::new(),
            runtime_risk_window_seconds: None,
            runtime_risk_out: None,
            append_runtime_risk: false,
            explain_out: None,
            job_out: None,
            command: vec!["python".to_string(), "publish.py".to_string()],
        });
        assert_eq!(job.desired_concurrency, Some(4));
        assert_eq!(job.limit, Some(3));
        assert_eq!(job.lease_seconds, Some(1200));
        assert_eq!(job.max_wait_seconds, Some(60));
        assert!(job.allow_wait);
        assert!(job.per_asset);
        assert_eq!(job.child_concurrency, Some(2));
        assert_eq!(job.runtime_renew_interval_seconds, Some(30));
        assert_eq!(job.child_timeout_seconds, Some(180));
        assert_eq!(job.child_result_dir, Some(PathBuf::from("child-results")));
        assert_eq!(job.max_failed_assets, Some(1));
        assert_eq!(job.max_failed_assets_per_reason, Some(2));
        assert_eq!(
            job.allow_states,
            vec!["active".to_string(), "repair".to_string()]
        );
        assert!(job.include_retry);
        assert_eq!(job.runtime_grace_seconds, Some(5));
        assert_eq!(job.dispatch_grace_seconds, Some(6));
        assert_eq!(job.cooldown_grace_seconds, Some(7));
        assert_eq!(job.failure_cooldown_seconds, Some(600));
        assert_eq!(job.failure_next_state, Some("repair".to_string()));
        assert_eq!(
            job.runtime_risk_out,
            Some(PathBuf::from("runtime-risk.ndjson"))
        );
        assert_eq!(
            job.runtime_risk_ledgers,
            vec![PathBuf::from("runtime-risk.ndjson")]
        );
        assert_eq!(job.runtime_risk_window_seconds, Some(900));
        assert!(job.append_runtime_risk);
        assert_eq!(
            job.failure_reason_rules
                .get("rate_limited")
                .and_then(|rule| rule.cooldown_seconds),
            Some(900)
        );
        assert_eq!(
            job.failure_reason_rules
                .get("rate_limited")
                .and_then(|rule| rule.recommended_action.as_deref()),
            Some("pause_failure_reason")
        );
        assert_eq!(
            job.failure_reason_rules
                .get("rate_limited")
                .and_then(|rule| rule.runtime_risk_severity.as_deref()),
            Some("critical")
        );
        assert_eq!(
            job.failure_reason_rules
                .get("rate_limited")
                .and_then(|rule| rule.next_suggested_limit),
            Some(0)
        );
        assert_eq!(
            job.failure_reason_rules
                .get("rate_limited")
                .and_then(|rule| rule.runtime_risk_cooldown_seconds),
            Some(1800)
        );
        assert_eq!(
            job.failure_reason_rules
                .get("risk_control")
                .and_then(|rule| rule.next_state.as_deref()),
            Some("quarantine")
        );

        let job_override = policy.merge_job_run(IdentityJobRunOptions {
            asset_manifest: PathBuf::from("profile-assets.json"),
            policy: None,
            job_preset: None,
            desired_concurrency: Some(2),
            limit: Some(1),
            worker: None,
            job: None,
            lease_seconds: Some(300),
            max_wait_seconds: Some(5),
            allow_wait: false,
            per_asset: false,
            child_concurrency: Some(4),
            runtime_renew_interval_seconds: Some(10),
            child_timeout_seconds: Some(20),
            child_result_dir: Some(PathBuf::from("override-results")),
            max_failed_assets: Some(2),
            max_failed_assets_per_reason: Some(3),
            allow_states: vec!["active".to_string()],
            include_dispatch_leased: false,
            include_retry: false,
            include_failed: true,
            include_cancelled: false,
            include_runtime_leased: false,
            include_missing_profile_dir: false,
            skip_sweep: false,
            skip_validate: false,
            runtime_grace_seconds: Some(1),
            dispatch_grace_seconds: None,
            cooldown_grace_seconds: None,
            failure_cooldown_seconds: Some(30),
            failure_next_state: Some("quarantine".to_string()),
            failure_reason_rules: BTreeMap::new(),
            asset_manifest_out: None,
            sweep_out: None,
            validate_out: None,
            gate_out: None,
            selection_out: None,
            release_out: None,
            append_release: false,
            runtime_risk_ledgers: Vec::new(),
            runtime_risk_window_seconds: None,
            runtime_risk_out: None,
            append_runtime_risk: false,
            explain_out: None,
            job_out: None,
            command: vec!["python".to_string(), "publish.py".to_string()],
        });
        assert_eq!(job_override.desired_concurrency, Some(2));
        assert_eq!(job_override.limit, Some(1));
        assert_eq!(job_override.lease_seconds, Some(300));
        assert_eq!(job_override.max_wait_seconds, Some(5));
        assert!(job_override.per_asset);
        assert_eq!(job_override.child_concurrency, Some(4));
        assert_eq!(job_override.runtime_renew_interval_seconds, Some(10));
        assert_eq!(job_override.child_timeout_seconds, Some(20));
        assert_eq!(
            job_override.child_result_dir,
            Some(PathBuf::from("override-results"))
        );
        assert_eq!(job_override.max_failed_assets, Some(2));
        assert_eq!(job_override.max_failed_assets_per_reason, Some(3));
        assert_eq!(job_override.allow_states, vec!["active".to_string()]);
        assert!(job_override.include_retry);
        assert!(job_override.include_failed);
        assert_eq!(job_override.runtime_grace_seconds, Some(1));
        assert_eq!(job_override.dispatch_grace_seconds, Some(6));
        assert_eq!(job_override.failure_cooldown_seconds, Some(30));
        assert_eq!(
            job_override.failure_next_state,
            Some("quarantine".to_string())
        );
    }

    #[test]
    fn identity_job_policy_preset_defaults_and_overrides() {
        fn base_job_options(job_preset: Option<&str>) -> IdentityJobRunOptions {
            IdentityJobRunOptions {
                asset_manifest: PathBuf::from("profile-assets.json"),
                policy: None,
                job_preset: job_preset.map(str::to_string),
                desired_concurrency: None,
                limit: None,
                worker: None,
                job: None,
                lease_seconds: None,
                max_wait_seconds: None,
                allow_wait: false,
                per_asset: false,
                child_concurrency: None,
                runtime_renew_interval_seconds: None,
                child_timeout_seconds: None,
                child_result_dir: None,
                max_failed_assets: None,
                max_failed_assets_per_reason: None,
                allow_states: Vec::new(),
                include_dispatch_leased: false,
                include_retry: false,
                include_failed: false,
                include_cancelled: false,
                include_runtime_leased: false,
                include_missing_profile_dir: false,
                skip_sweep: false,
                skip_validate: false,
                runtime_grace_seconds: None,
                dispatch_grace_seconds: None,
                cooldown_grace_seconds: None,
                failure_cooldown_seconds: None,
                failure_next_state: None,
                failure_reason_rules: BTreeMap::new(),
                asset_manifest_out: None,
                sweep_out: None,
                validate_out: None,
                gate_out: None,
                selection_out: None,
                release_out: None,
                append_release: false,
                runtime_risk_ledgers: Vec::new(),
                runtime_risk_window_seconds: None,
                runtime_risk_out: None,
                append_runtime_risk: false,
                explain_out: None,
                job_out: None,
                command: vec!["python".to_string(), "publish.py".to_string()],
            }
        }

        let spec = parse_identity_policy(
            r#"{
                "job": {
                    "preset": "publish_conservative",
                    "childConcurrency": 3,
                    "failureReasonRules": {
                        "rate_limited": {
                            "cooldownSeconds": 1200,
                            "nextState": "repair",
                            "recommendedAction": "pause_failure_reason",
                            "runtimeRiskSeverity": "critical",
                            "nextSuggestedLimit": 0,
                            "nextSuggestedDesiredConcurrency": 0,
                            "runtimeRiskCooldownSeconds": 2400
                        }
                    }
                }
            }"#,
        )
        .unwrap();
        let policy = LoadedIdentityPolicy {
            path: PathBuf::from("identity-policy.json"),
            spec,
        };

        let publish = policy.merge_job_run(base_job_options(None));
        assert_eq!(publish.job_preset.as_deref(), Some("publish_conservative"));
        assert!(publish.per_asset);
        assert_eq!(publish.child_concurrency, Some(3));
        assert_eq!(publish.max_failed_assets, Some(1));
        assert_eq!(publish.max_failed_assets_per_reason, Some(2));
        assert_eq!(
            publish
                .failure_reason_rules
                .get("rate_limited")
                .and_then(|rule| rule.cooldown_seconds),
            Some(1200)
        );
        assert_eq!(
            publish
                .failure_reason_rules
                .get("rate_limited")
                .and_then(|rule| rule.runtime_risk_cooldown_seconds),
            Some(2400)
        );
        assert_eq!(
            publish
                .failure_reason_rules
                .get("risk_control")
                .and_then(|rule| rule.recommended_action.as_deref()),
            Some("pause_pool")
        );

        let scrape = policy.merge_job_run(base_job_options(Some("scrape")));
        assert_eq!(scrape.job_preset.as_deref(), Some("scrape_aggressive"));
        assert_eq!(scrape.child_concurrency, Some(5));
        assert_eq!(scrape.max_failed_assets, Some(5));
        assert_eq!(
            scrape
                .failure_reason_rules
                .get("rate_limited")
                .and_then(|rule| rule.recommended_action.as_deref()),
            Some("reduce_concurrency")
        );
        assert_eq!(
            identity_job_preset_canonical("login"),
            Some("login_sensitive")
        );

        let standalone_publish =
            merge_identity_job_without_loaded_policy(base_job_options(Some("publish")));
        assert_eq!(
            standalone_publish.job_preset.as_deref(),
            Some("publish_conservative")
        );
        assert!(standalone_publish.per_asset);
        assert_eq!(standalone_publish.max_failed_assets, Some(1));
        assert_eq!(
            standalone_publish
                .failure_reason_rules
                .get("rate_limited")
                .and_then(|rule| rule.runtime_risk_cooldown_seconds),
            Some(1800)
        );
    }

    #[test]
    fn attach_identity_policy_adds_response_provenance() {
        let spec = parse_identity_policy(r#"{ "gatePreset": "strict" }"#).unwrap();
        let policy = LoadedIdentityPolicy {
            path: PathBuf::from("identity-policy.json"),
            spec,
        };
        let mut response = JsonResponse::ok(json!({ "scope": "identity_pool" }));

        attach_identity_policy(&mut response, Some(&policy));
        let value = response.into_value();

        assert_eq!(value["data"]["policy"]["path"], "identity-policy.json");
        assert_eq!(value["data"]["policy"]["format"], "json");
        assert_eq!(value["data"]["policy"]["rules"]["gatePreset"], "strict");
    }

    #[test]
    fn parses_identity_apply_actions_from_reports_and_ndjson() {
        let from_report = parse_identity_actions(
            r#"{
                "ok": true,
                "data": {
                    "actionQueue": {
                        "actions": [{
                            "actionIndex": 7,
                            "actionCode": "lifecycle.quarantine_profile",
                            "label": "acct-a",
                            "afterId": "fp_after",
                            "priority": "high"
                        }]
                    }
                }
            }"#,
        )
        .unwrap();
        assert_eq!(from_report.len(), 1);
        assert_eq!(from_report[0].action_index, 7);
        assert_eq!(from_report[0].action_code, "lifecycle.quarantine_profile");
        assert_eq!(from_report[0].labels[0], "acct-a");
        assert_eq!(from_report[0].identity_ids[0], "fp_after");

        let from_ndjson = parse_identity_actions(
            r#"{"actionCode":"pool.quarantine_candidate","identityIds":["fp_a","fp_b"]}
{"actionCode":"drift.restore_canvas_seed","label":"acct-a"}"#,
        )
        .unwrap();
        assert_eq!(from_ndjson.len(), 2);
        assert_eq!(from_ndjson[0].identity_ids, vec!["fp_a", "fp_b"]);
        assert_eq!(from_ndjson[1].labels, vec!["acct-a"]);
    }

    #[tokio::test]
    async fn identity_apply_dry_run_resolves_profiles_and_writes_journal() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-apply-dry-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let profile = root.join("acct-a");
        let actions_path = root.join("actions.json");
        let journal_path = root.join("apply.json");
        tokio::fs::create_dir_all(&profile).await.unwrap();
        tokio::fs::write(
            &actions_path,
            r#"{
                "actionQueue": {
                    "actions": [{
                        "actionIndex": 0,
                        "actionCode": "lifecycle.quarantine_profile",
                        "label": "acct-a",
                        "afterId": "fp_after",
                        "priority": "high"
                    },{
                        "actionIndex": 1,
                        "actionCode": "drift.restore_canvas_seed",
                        "label": "acct-a",
                        "priority": "high"
                    }]
                }
            }"#,
        )
        .await
        .unwrap();

        let response = apply_identity_actions(
            &actions_path,
            Some(&root),
            None,
            None,
            false,
            Some(&journal_path),
            false,
            None,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let journal: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&journal_path).await.unwrap()).unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["scope"], "identity_apply");
        assert_eq!(value["data"]["dryRun"], true);
        assert_eq!(value["data"]["actionCount"], 2);
        assert_eq!(value["data"]["operationCount"], 2);
        assert_eq!(value["data"]["plannedCount"], 1);
        assert_eq!(value["data"]["skippedCount"], 1);
        assert_eq!(value["data"]["operations"][0]["status"], "planned");
        assert_eq!(
            value["data"]["operations"][0]["destinationPath"],
            root.join("_quarantine")
                .join("acct-a")
                .display()
                .to_string()
        );
        assert_eq!(value["data"]["operations"][1]["status"], "skipped");
        assert!(tokio::fs::metadata(&profile).await.is_ok());
        assert_eq!(journal["scope"], "identity_apply");
        assert_eq!(journal["operationCount"], 2);

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_apply_execute_moves_profile_and_appends_journal() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-apply-exec-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let profile = root.join("acct-a");
        let actions_path = root.join("actions.ndjson");
        let journal_path = root.join("apply.ndjson");
        tokio::fs::create_dir_all(&profile).await.unwrap();
        tokio::fs::write(profile.join("marker.txt"), "ok")
            .await
            .unwrap();
        tokio::fs::write(
            &actions_path,
            r#"{"actionCode":"lifecycle.quarantine_profile","label":"acct-a","priority":"high"}"#,
        )
        .await
        .unwrap();

        let response = apply_identity_actions(
            &actions_path,
            Some(&root),
            None,
            None,
            true,
            Some(&journal_path),
            true,
            None,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let destination = root.join("_quarantine").join("acct-a");
        let journal_lines = tokio::fs::read_to_string(&journal_path)
            .await
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(value["data"]["execute"], true);
        assert_eq!(value["data"]["appliedCount"], 1);
        assert_eq!(value["data"]["failedCount"], 0);
        assert!(tokio::fs::metadata(&profile).await.is_err());
        assert!(
            tokio::fs::metadata(destination.join("marker.txt"))
                .await
                .is_ok()
        );
        assert_eq!(value["data"]["journalOut"]["format"], "ndjson_operations");
        assert_eq!(journal_lines.len(), 1);
        assert_eq!(
            journal_lines[0]["operation"]["status"],
            Value::String("applied".to_string())
        );

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_apply_reads_profile_asset_manifest_and_writes_state_patches() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-apply-assets-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let profile = root.join("profiles").join("acct-a");
        let actions_path = root.join("actions.json");
        let assets_path = root.join("profile-assets.json");
        let state_path = root.join("asset-state.json");
        tokio::fs::create_dir_all(&profile).await.unwrap();
        tokio::fs::write(
            &assets_path,
            r#"{
                "profileAssets": [{
                    "accountId": "acct-a",
                    "profileId": "profile-a",
                    "identityId": "fp_after",
                    "label": "acct-a",
                    "profileDir": "profiles/acct-a",
                    "proxyId": "proxy-us-1",
                    "fingerprintSeed": "seed-a",
                    "state": "active"
                }]
            }"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            &actions_path,
            r#"{
                "actionQueue": {
                    "actions": [{
                        "actionIndex": 0,
                        "actionCode": "lifecycle.quarantine_profile",
                        "label": "acct-a",
                        "afterId": "fp_after",
                        "priority": "high"
                    },{
                        "actionIndex": 1,
                        "actionCode": "drift.restore_canvas_seed",
                        "label": "acct-a",
                        "priority": "high"
                    }]
                }
            }"#,
        )
        .await
        .unwrap();

        let response = apply_identity_actions(
            &actions_path,
            Some(&root),
            Some(&assets_path),
            None,
            false,
            None,
            false,
            Some(&state_path),
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let state: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&state_path).await.unwrap()).unwrap();

        assert_eq!(value["data"]["assetPatchCount"], 2);
        assert_eq!(
            value["data"]["operations"][0]["asset"]["accountId"],
            "acct-a"
        );
        assert_eq!(
            value["data"]["operations"][0]["asset"]["proxyId"],
            "proxy-us-1"
        );
        assert_eq!(
            value["data"]["operations"][0]["sourcePath"],
            profile.display().to_string()
        );
        assert_eq!(value["data"]["assetPatches"][0]["previousState"], "active");
        assert_eq!(value["data"]["assetPatches"][0]["nextState"], "quarantine");
        assert_eq!(value["data"]["assetPatches"][1]["nextState"], "repair");
        assert_eq!(value["data"]["assetStateOut"]["format"], "json_report");
        assert_eq!(state["scope"], "identity_asset_state");
        assert_eq!(state["count"], 2);
        assert_eq!(state["patches"][0]["profileId"], "profile-a");
        assert_eq!(state["patches"][0]["fingerprintSeed"], "seed-a");

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_plan_merges_actions_asset_patches_and_writes_html() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-plan-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let report_path = root.join("pool.json");
        let patch_path = root.join("asset-state.ndjson");
        let manifest_path = root.join("profile-assets.json");
        let next_manifest_path = root.join("next-profile-assets.json");
        let out_path = root.join("plan.json");
        let html_path = root.join("plan.html");
        let dispatch_path = root.join("dispatch.ndjson");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(
            &report_path,
            r#"{
                "ok": true,
                "data": {
                    "scope": "identity_pool",
                    "gate": {
                        "passed": false,
                        "failures": ["identity_entropy_score_below_min: 42 < 60"]
                    },
                    "actionQueue": {
                        "actions": [{
                            "actionIndex": 0,
                            "source": "capacity",
                            "actionCode": "capacity.disperse_canvas_seed",
                            "priority": "high",
                            "title": "分散 canvas/audio 噪声种子",
                            "estimatedGain": 1.5
                        },{
                            "actionIndex": 1,
                            "source": "admission",
                            "actionCode": "pool.quarantine_duplicate_candidate",
                            "priority": "high",
                            "labels": ["acct-a"]
                        }]
                    }
                }
            }"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            &patch_path,
            r#"{"runId":"apply_1","patch":{"actionCode":"lifecycle.quarantine_profile","status":"applied","accountId":"acct-a","profileId":"profile-a","nextState":"quarantine","profileDir":"/tmp/acct-a","destinationPath":"/tmp/quarantine/acct-a"}}"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            &manifest_path,
            r#"{
                "profileAssets": [{
                    "accountId": "acct-a",
                    "profileId": "profile-a",
                    "identityId": "fp_after",
                    "label": "acct-a",
                    "profileDir": "/tmp/acct-a",
                    "proxyId": "proxy-us-1",
                    "fingerprintSeed": "seed-a",
                    "state": "active"
                },{
                    "accountId": "acct-b",
                    "profileId": "profile-b",
                    "profileDir": "/tmp/acct-b",
                    "state": "active"
                }]
            }"#,
        )
        .await
        .unwrap();

        let response = build_identity_plan(
            &[report_path.clone(), patch_path.clone()],
            Some("Nightly identity audit"),
            Some(&out_path),
            Some(&html_path),
            Some(&manifest_path),
            Some(&next_manifest_path),
            Some(&dispatch_path),
            true,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let plan: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&out_path).await.unwrap()).unwrap();
        let next_manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&next_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();
        let dispatch_lines = tokio::fs::read_to_string(&dispatch_path)
            .await
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let html = tokio::fs::read_to_string(&html_path).await.unwrap();

        assert_eq!(value["data"]["scope"], "identity_plan");
        assert_eq!(value["data"]["title"], "Nightly identity audit");
        assert_eq!(value["data"]["summary"]["inputCount"], 2);
        assert_eq!(value["data"]["summary"]["actionCount"], 2);
        assert_eq!(value["data"]["summary"]["capacityActionCount"], 1);
        assert_eq!(value["data"]["summary"]["quarantineActionCount"], 1);
        assert_eq!(value["data"]["summary"]["assetPatchCount"], 1);
        assert_eq!(value["data"]["summary"]["gateFailed"], true);
        assert_eq!(
            value["data"]["actionCodeCounts"]["capacity.disperse_canvas_seed"],
            1
        );
        assert_eq!(value["data"]["stateCounts"]["quarantine"], 1);
        assert_eq!(value["data"]["assetManifestOut"]["assetCount"], 2);
        assert_eq!(value["data"]["assetManifestOut"]["updatedCount"], 1);
        assert_eq!(value["data"]["assetManifestOut"]["unchangedCount"], 1);
        assert_eq!(
            value["data"]["assetManifestOut"]["stateCounts"]["quarantine"],
            1
        );
        let runbook = value["data"]["executionRunbook"].as_array().unwrap();
        assert_eq!(runbook[0]["phase"], "gate_review");
        assert_eq!(runbook[1]["phase"], "quarantine");
        assert_eq!(runbook[1]["actionCount"], 1);
        assert_eq!(runbook[1]["assetPatchCount"], 1);
        assert_eq!(runbook[2]["phase"], "capacity");
        assert_eq!(runbook[2]["actionCount"], 1);
        assert_eq!(runbook[3]["phase"], "manifest_writeback");
        assert_eq!(runbook[4]["phase"], "resample_verify");
        assert_eq!(value["data"]["summary"]["dispatchItemCount"], 5);
        assert_eq!(value["data"]["dispatchQueue"]["itemCount"], 5);
        assert_eq!(value["data"]["dispatchQueue"]["actionItemCount"], 2);
        assert_eq!(value["data"]["dispatchQueue"]["assetPatchItemCount"], 1);
        assert_eq!(value["data"]["dispatchQueue"]["commandItemCount"], 2);
        assert_eq!(
            value["data"]["dispatchQueue"]["items"][1]["phase"],
            "quarantine"
        );
        assert_eq!(value["data"]["dispatchQueue"]["items"][1]["kind"], "action");
        assert_eq!(
            value["data"]["dispatchQueue"]["items"][3]["kind"],
            "asset_patch"
        );
        assert!(
            value["data"]["dispatchQueue"]["items"][3]["dedupeKey"]
                .as_str()
                .unwrap()
                .contains("acct-a")
        );
        assert!(
            value["data"]["dispatchQueue"]["items"][3]["leaseKey"]
                .as_str()
                .unwrap()
                .starts_with(value["data"]["runId"].as_str().unwrap())
        );
        assert_eq!(
            value["data"]["dispatchOut"]["format"],
            "ndjson_dispatch_items"
        );
        assert_eq!(dispatch_lines.len(), 5);
        assert_eq!(dispatch_lines[3]["dispatch"]["kind"], "asset_patch");
        assert_eq!(plan["scope"], "identity_plan");
        assert_eq!(plan["summary"]["assetPatchCount"], 1);
        assert_eq!(
            next_manifest["profileAssets"][0]["state"],
            Value::String("quarantine".to_string())
        );
        assert_eq!(
            next_manifest["profileAssets"][0]["profileDir"],
            Value::String("/tmp/quarantine/acct-a".to_string())
        );
        assert_eq!(
            next_manifest["profileAssets"][0]["lastIdentityPlanActionCode"],
            Value::String("lifecycle.quarantine_profile".to_string())
        );
        assert_eq!(
            next_manifest["profileAssets"][1]["state"],
            Value::String("active".to_string())
        );
        assert!(html.contains("Nightly identity audit"));
        assert!(html.contains("执行 Runbook"));
        assert!(html.contains("Dispatch Queue"));
        assert!(html.contains("capacity.disperse_canvas_seed"));
        assert!(html.contains("acct-a"));
        assert!(html.contains("资产 Manifest 回写"));

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_dispatch_claims_sorted_unique_unblocked_items() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-dispatch-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let dispatch_path = root.join("dispatch.ndjson");
        let claim_path = root.join("claims.ndjson");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(
            &dispatch_path,
            r#"{"runId":"plan_1","dispatch":{"dispatchIndex":0,"stepIndex":0,"phase":"gate_review","kind":"command","sortRank":40,"dedupeKey":"gate_review:command:0","leaseKey":"plan_1:gate_review:command:0","blockedByGate":true}}
{"runId":"plan_1","dispatch":{"dispatchIndex":1,"stepIndex":1,"phase":"quarantine","kind":"action","sortRank":110,"dedupeKey":"quarantine:action:pool.quarantine_duplicate_candidate:acct-a:1","leaseKey":"plan_1:quarantine:action:pool.quarantine_duplicate_candidate:acct-a:1","blockedByGate":false,"actionIndex":1,"actionCode":"pool.quarantine_duplicate_candidate","priority":"high","label":"acct-a"}}
{"runId":"plan_1","dispatch":{"dispatchIndex":2,"stepIndex":1,"phase":"quarantine","kind":"action","sortRank":111,"dedupeKey":"quarantine:action:pool.quarantine_duplicate_candidate:acct-a:1","leaseKey":"plan_1:duplicate","blockedByGate":false,"actionIndex":1,"actionCode":"pool.quarantine_duplicate_candidate","priority":"high","label":"acct-a"}}
{"runId":"plan_1","dispatch":{"dispatchIndex":3,"stepIndex":2,"phase":"capacity","kind":"action","sortRank":310,"dedupeKey":"capacity:action:capacity.disperse_canvas_seed:pool:0","leaseKey":"plan_1:capacity:action:capacity.disperse_canvas_seed:pool:0","blockedByGate":false,"actionIndex":0,"actionCode":"capacity.disperse_canvas_seed","priority":"high"}}"#,
        )
        .await
        .unwrap();

        let response = claim_identity_dispatch(
            &dispatch_path,
            Some("worker-a"),
            2,
            120,
            false,
            None,
            false,
            None,
            false,
            Some(&claim_path),
            true,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let claim_lines = tokio::fs::read_to_string(&claim_path)
            .await
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(value["data"]["scope"], "identity_dispatch_claim");
        assert_eq!(value["data"]["workerId"], "worker-a");
        assert_eq!(value["data"]["inputCount"], 4);
        assert_eq!(value["data"]["candidateCount"], 3);
        assert_eq!(value["data"]["claimedCount"], 2);
        assert_eq!(value["data"]["skippedBlockedCount"], 1);
        assert_eq!(value["data"]["duplicateDedupeKeyCount"], 1);
        assert_eq!(value["data"]["remainingCandidateCount"], 0);
        assert_eq!(value["data"]["claimedPhases"][0], "quarantine");
        assert_eq!(value["data"]["claimedPhases"][1], "capacity");
        assert_eq!(
            value["data"]["items"][0]["dispatch"]["dedupeKey"],
            "quarantine:action:pool.quarantine_duplicate_candidate:acct-a:1"
        );
        assert_eq!(
            value["data"]["items"][1]["dispatch"]["actionCode"],
            "capacity.disperse_canvas_seed"
        );
        assert_eq!(value["data"]["claimOut"]["format"], "ndjson_claim_items");
        assert_eq!(claim_lines.len(), 2);
        assert_eq!(claim_lines[0]["item"]["workerId"], "worker-a");
        assert_eq!(claim_lines[0]["item"]["status"], "leased");

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_dispatch_skips_active_claim_ledger_leases() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-dispatch-ledger-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let dispatch_path = root.join("dispatch.ndjson");
        let ledger_path = root.join("claims.ndjson");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let now = unix_seconds();
        let active_expires = now + 3_600;
        let expired_expires = now.saturating_sub(1);
        tokio::fs::write(
            &dispatch_path,
            r#"{"runId":"plan_1","dispatch":{"dispatchIndex":0,"stepIndex":1,"phase":"quarantine","kind":"action","sortRank":110,"dedupeKey":"quarantine:action:pool.quarantine_duplicate_candidate:acct-a:1","leaseKey":"plan_1:quarantine:action:pool.quarantine_duplicate_candidate:acct-a:1","blockedByGate":false,"actionIndex":1,"actionCode":"pool.quarantine_duplicate_candidate","priority":"high","label":"acct-a"}}
{"runId":"plan_1","dispatch":{"dispatchIndex":1,"stepIndex":2,"phase":"capacity","kind":"action","sortRank":310,"dedupeKey":"capacity:action:capacity.disperse_canvas_seed:pool:0","leaseKey":"plan_1:capacity:action:capacity.disperse_canvas_seed:pool:0","blockedByGate":false,"actionIndex":0,"actionCode":"capacity.disperse_canvas_seed","priority":"high"}}"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            &ledger_path,
            format!(
                r#"{{"claimId":"claim_old","workerId":"worker-old","generatedAtUnixSeconds":{now},"leaseExpiresUnixSeconds":{active_expires},"item":{{"claimIndex":0,"status":"leased","workerId":"worker-old","claimId":"claim_old","leaseExpiresUnixSeconds":{active_expires},"dispatch":{{"dispatchIndex":0,"stepIndex":1,"phase":"quarantine","kind":"action","sortRank":110,"dedupeKey":"quarantine:action:pool.quarantine_duplicate_candidate:acct-a:1","leaseKey":"plan_1:quarantine:action:pool.quarantine_duplicate_candidate:acct-a:1","blockedByGate":false,"actionIndex":1,"actionCode":"pool.quarantine_duplicate_candidate","priority":"high","label":"acct-a"}}}}}}
{{"claimId":"claim_expired","workerId":"worker-old","generatedAtUnixSeconds":{now},"leaseExpiresUnixSeconds":{expired_expires},"item":{{"claimIndex":1,"status":"leased","workerId":"worker-old","claimId":"claim_expired","leaseExpiresUnixSeconds":{expired_expires},"dispatch":{{"dispatchIndex":1,"stepIndex":2,"phase":"capacity","kind":"action","sortRank":310,"dedupeKey":"capacity:action:capacity.disperse_canvas_seed:pool:0","leaseKey":"plan_1:capacity:action:capacity.disperse_canvas_seed:pool:0","blockedByGate":false,"actionIndex":0,"actionCode":"capacity.disperse_canvas_seed","priority":"high"}}}}}}"#
            ),
        )
        .await
        .unwrap();

        let response = claim_identity_dispatch(
            &dispatch_path,
            Some("worker-b"),
            10,
            120,
            false,
            Some(&ledger_path),
            false,
            None,
            false,
            None,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();

        assert_eq!(value["data"]["claimedCount"], 1);
        assert_eq!(value["data"]["activeLeaseCount"], 1);
        assert_eq!(value["data"]["expiredLeaseCount"], 1);
        assert_eq!(value["data"]["skippedLeasedCount"], 1);
        assert_eq!(
            value["data"]["items"][0]["dispatch"]["actionCode"],
            "capacity.disperse_canvas_seed"
        );

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_dispatch_renew_extends_claim_ledger_leases() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-dispatch-renew-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let dispatch_path = root.join("dispatch.ndjson");
        let claims_path = root.join("claims.ndjson");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let now = unix_seconds();
        let expired_expires = now.saturating_sub(1);
        tokio::fs::write(
            &dispatch_path,
            r#"{"runId":"plan_1","dispatch":{"dispatchIndex":0,"stepIndex":1,"phase":"quarantine","kind":"action","sortRank":110,"dedupeKey":"quarantine:action:pool.quarantine_duplicate_candidate:acct-a:1","leaseKey":"plan_1:quarantine:action:pool.quarantine_duplicate_candidate:acct-a:1","blockedByGate":false,"actionIndex":1,"actionCode":"pool.quarantine_duplicate_candidate","priority":"high","label":"acct-a"}}"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            &claims_path,
            format!(
                "{}\n",
                serde_json::to_string(&json!({
                "claimId": "claim_old",
                "workerId": "worker-a",
                "generatedAtUnixSeconds": now,
                "leaseExpiresUnixSeconds": expired_expires,
                "item": {
                    "claimIndex": 0,
                    "status": "leased",
                    "workerId": "worker-a",
                    "claimId": "claim_old",
                    "leaseExpiresUnixSeconds": expired_expires,
                    "dispatch": {
                        "dispatchIndex": 0,
                        "stepIndex": 1,
                        "phase": "quarantine",
                        "kind": "action",
                        "sortRank": 110,
                        "dedupeKey": "quarantine:action:pool.quarantine_duplicate_candidate:acct-a:1",
                        "leaseKey": "plan_1:quarantine:action:pool.quarantine_duplicate_candidate:acct-a:1",
                        "blockedByGate": false,
                        "actionIndex": 1,
                        "actionCode": "pool.quarantine_duplicate_candidate",
                        "priority": "high",
                        "label": "acct-a"
                    }
                }
            }))
                .unwrap()
            ),
        )
        .await
        .unwrap();

        let response = renew_identity_dispatch(
            &claims_path,
            Some("worker-a"),
            Some("claim_old"),
            &[],
            3_600,
            false,
            None,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        assert_eq!(value["data"]["renewedCount"], 0);
        assert_eq!(value["data"]["skippedExpiredCount"], 1);

        let response = renew_identity_dispatch(
            &claims_path,
            Some("worker-a"),
            Some("claim_old"),
            &[],
            3_600,
            true,
            Some(&claims_path),
            true,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let claim_lines = tokio::fs::read_to_string(&claims_path)
            .await
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(value["data"]["scope"], "identity_dispatch_renewal");
        assert_eq!(value["data"]["renewedCount"], 1);
        assert_eq!(value["data"]["items"][0]["renewalId"].is_string(), true);
        assert_eq!(
            value["data"]["items"][0]["previousLeaseExpiresUnixSeconds"],
            expired_expires
        );
        assert_eq!(claim_lines.len(), 2);
        assert_eq!(claim_lines[1]["item"]["renewalId"].is_string(), true);

        let response = claim_identity_dispatch(
            &dispatch_path,
            Some("worker-b"),
            10,
            120,
            false,
            Some(&claims_path),
            false,
            None,
            false,
            None,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        assert_eq!(value["data"]["claimedCount"], 0);
        assert_eq!(value["data"]["activeLeaseCount"], 1);
        assert_eq!(value["data"]["expiredLeaseCount"], 1);
        assert_eq!(value["data"]["skippedLeasedCount"], 1);

        let response = complete_identity_dispatch(
            &claims_path,
            "succeeded",
            Some("worker-a"),
            Some("claim_old"),
            &[],
            false,
            None,
            None,
            None,
            None,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        assert_eq!(value["data"]["completedCount"], 1);
        assert_eq!(value["data"]["duplicateDedupeKeyCount"], 1);
        assert_eq!(
            value["data"]["items"][0]["claim"]["renewalId"].is_string(),
            true
        );

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_dispatch_completion_ledger_skips_terminal_but_retries_retryable() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-dispatch-completion-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let dispatch_path = root.join("dispatch.ndjson");
        let claim_path = root.join("claims.ndjson");
        let completion_path = root.join("completed.ndjson");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(
            &dispatch_path,
            r#"{"runId":"plan_1","dispatch":{"dispatchIndex":0,"stepIndex":1,"phase":"quarantine","kind":"action","sortRank":110,"dedupeKey":"quarantine:action:pool.quarantine_duplicate_candidate:acct-a:1","leaseKey":"plan_1:quarantine:action:pool.quarantine_duplicate_candidate:acct-a:1","blockedByGate":false,"actionIndex":1,"actionCode":"pool.quarantine_duplicate_candidate","priority":"high","label":"acct-a"}}
{"runId":"plan_1","dispatch":{"dispatchIndex":1,"stepIndex":2,"phase":"capacity","kind":"action","sortRank":310,"dedupeKey":"capacity:action:capacity.disperse_canvas_seed:pool:0","leaseKey":"plan_1:capacity:action:capacity.disperse_canvas_seed:pool:0","blockedByGate":false,"actionIndex":0,"actionCode":"capacity.disperse_canvas_seed","priority":"high"}}"#,
        )
        .await
        .unwrap();

        claim_identity_dispatch(
            &dispatch_path,
            Some("worker-a"),
            10,
            120,
            false,
            None,
            false,
            None,
            false,
            Some(&claim_path),
            true,
        )
        .await
        .unwrap();

        let first_key =
            vec!["quarantine:action:pool.quarantine_duplicate_candidate:acct-a:1".to_string()];
        let response = complete_identity_dispatch(
            &claim_path,
            "succeeded",
            Some("worker-a"),
            None,
            &first_key,
            false,
            None,
            Some("profile quarantined"),
            Some(r#"{"profileDir":"/tmp/quarantine/acct-a"}"#),
            Some(&completion_path),
            true,
        )
        .await
        .unwrap();
        let value = response.into_value();
        assert_eq!(value["data"]["scope"], "identity_dispatch_completion");
        assert_eq!(value["data"]["completedCount"], 1);
        assert_eq!(value["data"]["items"][0]["status"], "succeeded");
        assert_eq!(
            value["data"]["items"][0]["result"]["profileDir"],
            "/tmp/quarantine/acct-a"
        );

        let second_key = vec!["capacity:action:capacity.disperse_canvas_seed:pool:0".to_string()];
        complete_identity_dispatch(
            &claim_path,
            "retry",
            Some("worker-a"),
            None,
            &second_key,
            false,
            Some(60),
            Some("seed service busy"),
            None,
            Some(&completion_path),
            true,
        )
        .await
        .unwrap();

        let response = claim_identity_dispatch(
            &dispatch_path,
            Some("worker-b"),
            10,
            120,
            false,
            None,
            false,
            Some(&completion_path),
            false,
            None,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let completion_lines = tokio::fs::read_to_string(&completion_path)
            .await
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(completion_lines.len(), 2);
        assert_eq!(completion_lines[0]["item"]["status"], "succeeded");
        assert_eq!(completion_lines[1]["item"]["retryEligible"], true);
        assert_eq!(value["data"]["claimedCount"], 1);
        assert_eq!(value["data"]["completionLedgerCount"], 2);
        assert_eq!(value["data"]["terminalCompletionCount"], 1);
        assert_eq!(value["data"]["retryableCompletionCount"], 1);
        assert_eq!(value["data"]["skippedCompletedCount"], 1);
        assert_eq!(
            value["data"]["items"][0]["dispatch"]["actionCode"],
            "capacity.disperse_canvas_seed"
        );

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_dispatch_reconcile_writes_dispatch_state_to_asset_manifest() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-dispatch-reconcile-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let dispatch_path = root.join("dispatch.ndjson");
        let claims_path = root.join("claims.ndjson");
        let completions_path = root.join("completed.ndjson");
        let manifest_path = root.join("profile-assets.json");
        let next_manifest_path = root.join("next-profile-assets.json");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(
            &manifest_path,
            r#"{"profileAssets":[
{"accountId":"acct-a","label":"acct-a","profileDir":"/profiles/acct-a","state":"active"},
{"accountId":"acct-b","label":"acct-b","profileDir":"/profiles/acct-b","state":"repair"}
]}"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            &dispatch_path,
            r#"{"runId":"plan_1","dispatch":{"dispatchIndex":0,"stepIndex":1,"phase":"quarantine","kind":"action","sortRank":110,"dedupeKey":"quarantine:action:lifecycle.quarantine_profile:acct-a:0","leaseKey":"plan_1:quarantine:action:lifecycle.quarantine_profile:acct-a:0","blockedByGate":false,"actionIndex":0,"actionCode":"lifecycle.quarantine_profile","priority":"high","label":"acct-a"}}
{"runId":"plan_1","dispatch":{"dispatchIndex":1,"stepIndex":3,"phase":"manifest_writeback","kind":"asset_patch","sortRank":540,"dedupeKey":"manifest_writeback:asset_patch:lifecycle.quarantine_profile:acct-b:1","leaseKey":"plan_1:manifest_writeback:asset_patch:lifecycle.quarantine_profile:acct-b:1","blockedByGate":false,"assetPatchIndex":1,"actionCode":"lifecycle.quarantine_profile","accountId":"acct-b","label":"acct-b","profileDir":"/profiles/acct-b","nextState":"quarantine"}}"#,
        )
        .await
        .unwrap();

        claim_identity_dispatch(
            &dispatch_path,
            Some("worker-a"),
            10,
            3_600,
            false,
            None,
            false,
            None,
            false,
            Some(&claims_path),
            true,
        )
        .await
        .unwrap();
        let complete_key = vec![
            "manifest_writeback:asset_patch:lifecycle.quarantine_profile:acct-b:1".to_string(),
        ];
        complete_identity_dispatch(
            &claims_path,
            "succeeded",
            Some("worker-a"),
            None,
            &complete_key,
            false,
            None,
            Some("manifest updated"),
            Some(r#"{"profileDir":"/quarantine/acct-b"}"#),
            Some(&completions_path),
            true,
        )
        .await
        .unwrap();

        let response = reconcile_identity_dispatch_manifest(
            &manifest_path,
            Some(&claims_path),
            Some(&completions_path),
            Some(&next_manifest_path),
        )
        .await
        .unwrap();
        let value = response.into_value();
        let next_manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&next_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();

        assert_eq!(value["data"]["scope"], "identity_dispatch_reconcile");
        assert_eq!(value["data"]["assetCount"], 2);
        assert_eq!(value["data"]["updatedAssetCount"], 2);
        assert_eq!(value["data"]["unmatchedEventCount"], 0);
        assert_eq!(value["data"]["dispatchStateCounts"]["leased"], 1);
        assert_eq!(value["data"]["dispatchStateCounts"]["succeeded"], 1);
        assert_eq!(next_manifest["profileAssets"][0]["dispatchState"], "leased");
        assert_eq!(
            next_manifest["profileAssets"][0]["lastDispatchWorkerId"],
            "worker-a"
        );
        assert_eq!(
            next_manifest["profileAssets"][1]["dispatchState"],
            "succeeded"
        );
        assert_eq!(
            next_manifest["profileAssets"][1]["state"],
            Value::String("quarantine".to_string())
        );
        assert_eq!(
            next_manifest["profileAssets"][1]["profileDir"],
            Value::String("/quarantine/acct-b".to_string())
        );
        assert_eq!(
            next_manifest["profileAssets"][1]["lastDispatchPreviousState"],
            Value::String("repair".to_string())
        );
        assert_eq!(
            value["data"]["assetManifestOut"]["format"],
            "profile_assets_json"
        );

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_assets_validate_reports_schema_errors_and_warnings() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-assets-validate-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let manifest_path = root.join("runtime-profile-assets.json");
        let validate_path = root.join("validate.json");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let now = unix_seconds();
        let expired = now.saturating_sub(60);
        tokio::fs::write(
            &manifest_path,
            serde_json::to_string(&json!({
                "manifestVersion": "1",
                "profileAssets": [
                    {
                        "accountId": "acct-a",
                        "label": "acct-a",
                        "profileDir": "/profiles/acct-a",
                        "state": "active",
                        "runtimeLeaseState": "leased",
                        "runtimeLeaseId": "lease-a",
                        "runtimeLeaseExpiresUnixSeconds": "bad"
                    },
                    {
                        "accountId": "acct-a",
                        "label": "acct-b",
                        "profileDir": "/profiles/acct-b",
                        "state": "weird",
                        "dispatchState": "mystery"
                    },
                    {
                        "state": "active"
                    },
                    {
                        "accountId": "acct-c",
                        "label": "acct-c",
                        "profileDir": "/profiles/acct-c",
                        "state": "active",
                        "dispatchState": "leased",
                        "lastDispatchLeaseExpiresUnixSeconds": expired,
                        "cooldownUntilUnixSeconds": expired
                    }
                ]
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let response = validate_identity_assets(&manifest_path, true, Some(&validate_path))
            .await
            .unwrap();
        let value = response.into_value();
        let validate_report: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&validate_path).await.unwrap())
                .unwrap();

        assert_eq!(value["data"]["scope"], "identity_assets_validate");
        assert_eq!(value["data"]["manifestVersion"], "1");
        assert_eq!(value["data"]["strict"], true);
        assert_eq!(value["data"]["assetCount"], 4);
        assert_eq!(value["data"]["valid"], false);
        assert_eq!(value["data"]["errorCount"], 3);
        assert_eq!(value["data"]["issueCodeCounts"]["duplicate_account_id"], 1);
        assert_eq!(value["data"]["issueCodeCounts"]["invalid_timestamp"], 1);
        assert_eq!(value["data"]["issueCodeCounts"]["missing_match_key"], 1);
        assert_eq!(value["data"]["issueCodeCounts"]["missing_profile_dir"], 1);
        assert_eq!(value["data"]["issueCodeCounts"]["unknown_state"], 1);
        assert_eq!(
            value["data"]["issueCodeCounts"]["unknown_dispatch_state"],
            1
        );
        assert_eq!(
            value["data"]["issueCodeCounts"]["dispatch_lease_expired"],
            1
        );
        assert_eq!(value["data"]["issueCodeCounts"]["cooldown_expired"], 1);
        assert_eq!(
            value["data"]["issueCodeCounts"]["runtime_lease_missing_expires"],
            1
        );
        assert_eq!(value["data"]["validateOut"]["format"], "json_report");
        assert_eq!(validate_report["scope"], "identity_assets_validate");
        assert_eq!(validate_report["valid"], false);

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_assets_status_reports_capacity_and_block_reasons() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-assets-status-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let manifest_path = root.join("runtime-profile-assets.json");
        let status_path = root.join("status.json");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let now = unix_seconds();
        let expired = now.saturating_sub(60);
        let future = now + 3_600;
        tokio::fs::write(
            &manifest_path,
            serde_json::to_string(&json!({
                "profileAssets": [
                    {"accountId":"acct-a","label":"acct-a","profileDir":"/profiles/acct-a","state":"active"},
                    {"accountId":"acct-b","label":"acct-b","profileDir":"/profiles/acct-b","state":"active","dispatchState":"leased","lastDispatchLeaseExpiresUnixSeconds":future},
                    {"accountId":"acct-c","label":"acct-c","profileDir":"/profiles/acct-c","state":"repair"},
                    {"accountId":"acct-d","label":"acct-d","profileDir":"/profiles/acct-d","state":"active","dispatchState":"retry","lastDispatchRetryAfterUnixSeconds":future},
                    {"accountId":"acct-e","label":"acct-e","profileDir":"/profiles/acct-e","state":"active","runtimeLeaseState":"leased","runtimeLeaseExpiresUnixSeconds":future},
                    {"accountId":"acct-f","label":"acct-f","state":"active"},
                    {"accountId":"acct-g","label":"acct-g","profileDir":"/profiles/acct-g","state":"active","runtimeLeaseState":"leased","runtimeLeaseExpiresUnixSeconds":expired},
                    {"accountId":"acct-h","label":"acct-h","profileDir":"/profiles/acct-h","state":"active","cooldownUntilUnixSeconds":expired},
                    {"accountId":"acct-i","label":"acct-i","profileDir":"/profiles/acct-i","state":"active","dispatchState":"leased","lastDispatchLeaseExpiresUnixSeconds":expired},
                    {"accountId":"acct-j","label":"acct-j","profileDir":"/profiles/acct-j","state":"active","dispatchState":"failed"},
                    {"accountId":"acct-k","label":"acct-k","profileDir":"/profiles/acct-k","state":"active","cooldownUntilUnixSeconds":future}
                ]
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let response = status_identity_assets(
            &manifest_path,
            &[],
            Some(5),
            false,
            false,
            false,
            false,
            false,
            false,
            Some(&status_path),
        )
        .await
        .unwrap();
        let value = response.into_value();
        let status_report: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&status_path).await.unwrap()).unwrap();

        assert_eq!(value["data"]["scope"], "identity_assets_status");
        assert_eq!(value["data"]["assetCount"], 11);
        assert_eq!(value["data"]["runnableCount"], 4);
        assert_eq!(value["data"]["blockedCount"], 7);
        assert_eq!(value["data"]["capacityStatus"], "shortage");
        assert_eq!(value["data"]["capacityShortageCount"], 1);
        assert_eq!(value["data"]["recommendedLimit"], 4);
        assert_eq!(value["data"]["activeRuntimeLeaseCount"], 1);
        assert_eq!(value["data"]["expiredRuntimeLeaseCount"], 1);
        assert_eq!(value["data"]["activeDispatchLeaseCount"], 1);
        assert_eq!(value["data"]["expiredDispatchLeaseCount"], 1);
        assert_eq!(value["data"]["activeCooldownCount"], 1);
        assert_eq!(value["data"]["expiredCooldownCount"], 1);
        assert_eq!(value["data"]["dispatchRetryWaitingCount"], 1);
        assert_eq!(
            value["data"]["blockReasonCounts"]["dispatch_lease_active"],
            1
        );
        assert_eq!(
            value["data"]["blockReasonCounts"]["state_not_allowed:repair"],
            1
        );
        assert_eq!(
            value["data"]["blockReasonCounts"]["dispatch_retry_waiting"],
            1
        );
        assert_eq!(
            value["data"]["blockReasonCounts"]["runtime_lease_active"],
            1
        );
        assert_eq!(value["data"]["blockReasonCounts"]["missing_profile_dir"], 1);
        assert_eq!(value["data"]["blockReasonCounts"]["dispatch_failed"], 1);
        assert_eq!(value["data"]["blockReasonCounts"]["cooldown_active"], 1);
        assert!(
            value["data"]["recommendations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|recommendation| recommendation["code"] == "capacity_shortage")
        );
        assert!(
            value["data"]["recommendations"]
                .as_array()
                .unwrap()
                .iter()
                .any(
                    |recommendation| recommendation["code"] == "run_assets_sweep"
                        && recommendation["affectedCount"] == 3
                )
        );
        assert_eq!(status_report["scope"], "identity_assets_status");
        assert_eq!(status_report["runnableCount"], 4);

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_assets_forecast_reports_recovery_timeline() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-assets-forecast-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let manifest_path = root.join("runtime-profile-assets.json");
        let forecast_path = root.join("forecast.json");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let now = unix_seconds();
        let cooldown = now + 300;
        let runtime_expires = now + 600;
        let retry_after = now + 900;
        tokio::fs::write(
            &manifest_path,
            serde_json::to_string(&json!({
                "profileAssets": [
                    {"accountId":"acct-a","label":"acct-a","profileDir":"/profiles/acct-a","state":"active"},
                    {"accountId":"acct-b","label":"acct-b","profileDir":"/profiles/acct-b","state":"active","cooldownUntilUnixSeconds":cooldown},
                    {"accountId":"acct-c","label":"acct-c","profileDir":"/profiles/acct-c","state":"active","runtimeLeaseState":"leased","runtimeLeaseExpiresUnixSeconds":runtime_expires},
                    {"accountId":"acct-d","label":"acct-d","profileDir":"/profiles/acct-d","state":"active","dispatchState":"retry","lastDispatchRetryAfterUnixSeconds":retry_after},
                    {"accountId":"acct-e","label":"acct-e","state":"active"},
                    {"accountId":"acct-f","label":"acct-f","profileDir":"/profiles/acct-f","state":"repair"}
                ]
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let response = forecast_identity_assets(
            &manifest_path,
            &[],
            Some(4),
            Some(600),
            false,
            false,
            false,
            false,
            false,
            false,
            Some(&forecast_path),
        )
        .await
        .unwrap();
        let value = response.into_value();
        let forecast_report: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&forecast_path).await.unwrap())
                .unwrap();

        assert_eq!(value["data"]["scope"], "identity_assets_forecast");
        assert_eq!(value["data"]["assetCount"], 6);
        assert_eq!(value["data"]["currentRunnableCount"], 1);
        assert_eq!(value["data"]["blockedCount"], 5);
        assert_eq!(value["data"]["recoverableCount"], 3);
        assert_eq!(value["data"]["recoverableWithinHorizonCount"], 2);
        assert_eq!(value["data"]["hardBlockedCount"], 2);
        assert_eq!(value["data"]["predictedRunnableCount"], 3);
        assert_eq!(value["data"]["currentShortageCount"], 3);
        assert_eq!(value["data"]["predictedShortageCount"], 1);
        assert_eq!(value["data"]["capacityStatus"], "shortage");
        assert_eq!(value["data"]["predictedCapacityStatus"], "shortage");
        assert_eq!(value["data"]["nextRecoveryAtUnixSeconds"], json!(cooldown));
        assert_eq!(value["data"]["enoughAtUnixSeconds"], json!(retry_after));
        assert_eq!(value["data"]["blockReasonCounts"]["cooldown_active"], 1);
        assert_eq!(
            value["data"]["blockReasonCounts"]["runtime_lease_active"],
            1
        );
        assert_eq!(
            value["data"]["blockReasonCounts"]["dispatch_retry_waiting"],
            1
        );
        assert_eq!(value["data"]["blockReasonCounts"]["missing_profile_dir"], 1);
        assert_eq!(
            value["data"]["blockReasonCounts"]["state_not_allowed:repair"],
            1
        );
        assert_eq!(value["data"]["recoveryEvents"][0]["label"], "acct-b");
        assert_eq!(
            value["data"]["recoveryEvents"][0]["availableAtUnixSeconds"],
            json!(cooldown)
        );
        assert_eq!(value["data"]["forecastOut"]["format"], "json_report");
        assert_eq!(forecast_report["scope"], "identity_assets_forecast");

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_assets_gate_decides_run_wait_or_insufficient() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-assets-gate-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let manifest_path = root.join("runtime-profile-assets.json");
        let gate_path = root.join("gate.json");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let now = unix_seconds();
        let cooldown = now + 300;
        tokio::fs::write(
            &manifest_path,
            serde_json::to_string(&json!({
                "profileAssets": [
                    {"accountId":"acct-a","label":"acct-a","profileDir":"/profiles/acct-a","state":"active"},
                    {"accountId":"acct-b","label":"acct-b","profileDir":"/profiles/acct-b","state":"active","cooldownUntilUnixSeconds":cooldown},
                    {"accountId":"acct-c","label":"acct-c","state":"active"}
                ]
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let response = gate_identity_assets(
            &manifest_path,
            1,
            Some(600),
            false,
            &[],
            false,
            false,
            false,
            false,
            false,
            false,
            Some(&gate_path),
        )
        .await
        .unwrap();
        let value = response.into_value();
        let gate_report: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&gate_path).await.unwrap()).unwrap();
        assert_eq!(value["data"]["scope"], "identity_assets_gate");
        assert_eq!(value["data"]["decision"], "run_now");
        assert_eq!(value["data"]["passed"], true);
        assert_eq!(value["data"]["exitCode"], 0);
        assert_eq!(value["data"]["gateOut"]["format"], "json_report");
        assert_eq!(gate_report["scope"], "identity_assets_gate");

        let response = gate_identity_assets(
            &manifest_path,
            2,
            Some(600),
            false,
            &[],
            false,
            false,
            false,
            false,
            false,
            false,
            None,
        )
        .await
        .unwrap();
        let value = response.into_value();
        assert_eq!(value["data"]["decision"], "wait");
        assert_eq!(value["data"]["passed"], false);
        assert_eq!(value["data"]["exitCode"], 2);
        assert_eq!(value["data"]["enoughAtUnixSeconds"], json!(cooldown));

        let response = gate_identity_assets(
            &manifest_path,
            2,
            Some(600),
            true,
            &[],
            false,
            false,
            false,
            false,
            false,
            false,
            None,
        )
        .await
        .unwrap();
        let value = response.into_value();
        assert_eq!(value["data"]["decision"], "wait");
        assert_eq!(value["data"]["passed"], true);
        assert_eq!(value["data"]["exitCode"], 0);

        let response = gate_identity_assets(
            &manifest_path,
            3,
            Some(600),
            true,
            &[],
            false,
            false,
            false,
            false,
            false,
            false,
            None,
        )
        .await
        .unwrap();
        let value = response.into_value();
        assert_eq!(value["data"]["decision"], "insufficient");
        assert_eq!(value["data"]["passed"], false);
        assert_eq!(value["data"]["exitCode"], 2);

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_assets_select_filters_and_marks_runtime_leases() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-assets-select-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let manifest_path = root.join("runtime-profile-assets.json");
        let leased_manifest_path = root.join("leased-profile-assets.json");
        let selection_path = root.join("selection.json");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let now = unix_seconds();
        let future = now + 3_600;
        tokio::fs::write(
            &manifest_path,
            serde_json::to_string(&json!({
                "profileAssets": [
                    {"accountId":"acct-a","label":"acct-a","profileDir":"/profiles/acct-a","state":"active"},
                    {"accountId":"acct-b","label":"acct-b","profileDir":"/profiles/acct-b","state":"active","dispatchState":"leased","lastDispatchLeaseExpiresUnixSeconds":future},
                    {"accountId":"acct-c","label":"acct-c","profileDir":"/profiles/acct-c","state":"repair"},
                    {"accountId":"acct-d","label":"acct-d","profileDir":"/profiles/acct-d","state":"active","dispatchState":"retry","lastDispatchRetryAfterUnixSeconds":future},
                    {"accountId":"acct-e","label":"acct-e","profileDir":"/profiles/acct-e","state":"active","runtimeLeaseState":"leased","runtimeLeaseExpiresUnixSeconds":future},
                    {"accountId":"acct-f","label":"acct-f","state":"active"},
                    {"accountId":"acct-g","label":"acct-g","profileDir":"/profiles/acct-g","state":"active"}
                ]
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let response = select_identity_assets(
            &manifest_path,
            1,
            &[],
            Some("worker-a"),
            Some("publish"),
            900,
            false,
            false,
            false,
            false,
            false,
            false,
            Some(&leased_manifest_path),
            Some(&selection_path),
        )
        .await
        .unwrap();
        let value = response.into_value();
        let leased_manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&leased_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();
        let selection: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&selection_path).await.unwrap())
                .unwrap();

        assert_eq!(value["data"]["scope"], "identity_assets_select");
        assert_eq!(value["data"]["assetCount"], 7);
        assert_eq!(value["data"]["selectedCount"], 1);
        assert_eq!(value["data"]["blockedCount"], 6);
        assert_eq!(value["data"]["overflowCount"], 1);
        assert_eq!(value["data"]["selectedAssets"][0]["label"], "acct-a");
        assert_eq!(
            value["data"]["selectedAssets"][0]["leaseId"].is_string(),
            true
        );
        assert_eq!(
            value["data"]["blockReasonCounts"]["dispatch_lease_active"],
            1
        );
        assert_eq!(
            value["data"]["blockReasonCounts"]["state_not_allowed:repair"],
            1
        );
        assert_eq!(
            value["data"]["blockReasonCounts"]["dispatch_retry_waiting"],
            1
        );
        assert_eq!(
            value["data"]["blockReasonCounts"]["runtime_lease_active"],
            1
        );
        assert_eq!(value["data"]["blockReasonCounts"]["missing_profile_dir"], 1);
        assert_eq!(value["data"]["blockReasonCounts"]["limit_reached"], 1);
        assert_eq!(
            leased_manifest["profileAssets"][0]["runtimeLeaseState"],
            "leased"
        );
        assert_eq!(
            leased_manifest["profileAssets"][0]["runtimeLeaseWorkerId"],
            "worker-a"
        );
        assert_eq!(
            leased_manifest["profileAssets"][0]["runtimeLeaseJobId"],
            "publish"
        );
        assert_eq!(
            leased_manifest["profileAssets"][1]["runtimeLeaseState"],
            Value::Null
        );
        assert_eq!(selection["scope"], "identity_assets_select");
        assert_eq!(selection["selectedCount"], 1);

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_assets_release_clears_runtime_lease_and_sets_cooldown() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-assets-release-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let manifest_path = root.join("runtime-profile-assets.json");
        let leased_manifest_path = root.join("leased-profile-assets.json");
        let released_manifest_path = root.join("released-profile-assets.json");
        let release_path = root.join("release.json");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(
            &manifest_path,
            r#"{"profileAssets":[
{"accountId":"acct-a","profileId":"profile-a","identityId":"fp-a","label":"acct-a","profileDir":"/profiles/acct-a","state":"active"},
{"accountId":"acct-b","label":"acct-b","profileDir":"/profiles/acct-b","state":"active"}
]}"#,
        )
        .await
        .unwrap();

        select_identity_assets(
            &manifest_path,
            1,
            &[],
            Some("worker-a"),
            Some("publish"),
            3_600,
            false,
            false,
            false,
            false,
            false,
            false,
            Some(&leased_manifest_path),
            None,
        )
        .await
        .unwrap();

        let response = release_identity_assets(
            &leased_manifest_path,
            "failed",
            Some("worker-a"),
            Some("publish"),
            &[],
            &[],
            &[],
            &[],
            &[],
            Some(600),
            Some("repair"),
            Some("publish failed"),
            Some(r#"{"error":"captcha"}"#),
            Some(&released_manifest_path),
            Some(&release_path),
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let released_manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&released_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();
        let release_report: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&release_path).await.unwrap()).unwrap();

        assert_eq!(value["data"]["scope"], "identity_assets_release");
        assert_eq!(value["data"]["releasedCount"], 1);
        assert_eq!(value["data"]["matchedCount"], 1);
        assert_eq!(value["data"]["runtimeLeaseStateCounts"]["released"], 1);
        assert_eq!(
            released_manifest["profileAssets"][0]["runtimeLeaseState"],
            "released"
        );
        assert_eq!(
            released_manifest["profileAssets"][0]["lastRuntimeStatus"],
            "failed"
        );
        assert_eq!(
            released_manifest["profileAssets"][0]["lastRuntimeWorkerId"],
            "worker-a"
        );
        assert_eq!(
            released_manifest["profileAssets"][0]["lastRuntimeJobId"],
            "publish"
        );
        assert_eq!(
            released_manifest["profileAssets"][0]["lastRuntimeResult"]["error"],
            "captcha"
        );
        assert_eq!(
            released_manifest["profileAssets"][0]["state"],
            Value::String("repair".to_string())
        );
        assert!(released_manifest["profileAssets"][0]["cooldownUntilUnixSeconds"].is_number());
        assert_eq!(
            released_manifest["profileAssets"][0]["runtimeLeaseId"],
            Value::Null
        );
        assert_eq!(release_report["scope"], "identity_assets_release");

        let allow_states = vec!["active".to_string(), "repair".to_string()];
        let response = select_identity_assets(
            &released_manifest_path,
            10,
            &allow_states,
            Some("worker-b"),
            Some("publish"),
            900,
            false,
            false,
            false,
            false,
            false,
            false,
            None,
            None,
        )
        .await
        .unwrap();
        let value = response.into_value();
        assert_eq!(value["data"]["selectedCount"], 1);
        assert_eq!(value["data"]["selectedAssets"][0]["label"], "acct-b");
        assert_eq!(value["data"]["blockReasonCounts"]["cooldown_active"], 1);

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_assets_release_appends_runtime_ledger_items() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-assets-release-ledger-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let manifest_path = root.join("runtime-profile-assets.json");
        let leased_manifest_path = root.join("leased-profile-assets.json");
        let release_ledger_path = root.join("runtime-release.ndjson");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(
            &manifest_path,
            r#"{"profileAssets":[
{"accountId":"acct-a","profileId":"profile-a","identityId":"fp-a","label":"acct-a","profileDir":"/profiles/acct-a","state":"active"},
{"accountId":"acct-b","profileId":"profile-b","identityId":"fp-b","label":"acct-b","profileDir":"/profiles/acct-b","state":"active"}
]}"#,
        )
        .await
        .unwrap();

        select_identity_assets(
            &manifest_path,
            2,
            &[],
            Some("worker-a"),
            Some("publish"),
            3_600,
            false,
            false,
            false,
            false,
            false,
            false,
            Some(&leased_manifest_path),
            None,
        )
        .await
        .unwrap();

        let response = release_identity_assets(
            &leased_manifest_path,
            "succeeded",
            Some("worker-a"),
            Some("publish"),
            &[],
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            Some("publish ok"),
            Some(r#"{"published":true}"#),
            None,
            Some(&release_ledger_path),
            true,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let ledger_text = tokio::fs::read_to_string(&release_ledger_path)
            .await
            .unwrap();
        let lines = ledger_text
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(value["data"]["scope"], "identity_assets_release");
        assert_eq!(value["data"]["releasedCount"], 2);
        assert_eq!(value["data"]["releaseOut"]["append"], true);
        assert_eq!(value["data"]["releaseOut"]["count"], 2);
        assert_eq!(
            value["data"]["releaseOut"]["format"],
            "ndjson_release_items"
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["scope"], "identity_assets_release");
        assert_eq!(lines[0]["status"], "succeeded");
        assert_eq!(lines[0]["workerId"], "worker-a");
        assert_eq!(lines[0]["jobId"], "publish");
        assert_eq!(lines[0]["message"], "publish ok");
        assert_eq!(lines[0]["result"]["published"], true);
        assert_eq!(lines[0]["item"]["status"], "succeeded");
        assert_eq!(lines[0]["item"]["label"], "acct-a");
        assert_eq!(lines[1]["item"]["label"], "acct-b");

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_ledger_compact_retains_summary_and_active_suppression() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-ledger-compact-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let release_ledger_path = root.join("runtime-release.ndjson");
        let runtime_risk_path = root.join("runtime-risk.ndjson");
        let compact_path = root.join("ledger-compact.json");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let now = unix_seconds();
        let suppress_until = now + 600;
        tokio::fs::write(
            &release_ledger_path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&json!({
                    "scope": "identity_assets_release",
                    "generatedAtUnixSeconds": now.saturating_sub(3_600),
                    "status": "failed",
                    "workerId": "worker-a",
                    "jobId": "publish",
                    "result": {"failureReason": "old_failure"},
                    "item": {
                        "assetIndex": 0,
                        "status": "failed",
                        "accountId": "acct-old",
                        "label": "acct-old",
                        "workerId": "worker-a",
                        "jobId": "publish"
                    }
                }))
                .unwrap(),
                serde_json::to_string(&json!({
                    "scope": "identity_assets_release",
                    "generatedAtUnixSeconds": now.saturating_sub(10),
                    "status": "failed",
                    "workerId": "worker-a",
                    "jobId": "publish",
                    "message": "rate limited",
                    "result": {"failureReason": "rate_limited"},
                    "item": {
                        "assetIndex": 1,
                        "status": "failed",
                        "accountId": "acct-a",
                        "profileId": "profile-a",
                        "identityId": "fp-a",
                        "label": "acct-a",
                        "profileDir": "/profiles/acct-a",
                        "workerId": "worker-a",
                        "jobId": "publish"
                    }
                }))
                .unwrap()
            ),
        )
        .await
        .unwrap();
        tokio::fs::write(
            &runtime_risk_path,
            serde_json::to_string(&json!({
                "scope": "identity_job_runtime_risk_event",
                "generatedAtUnixSeconds": now.saturating_sub(3_600),
                "workerId": "worker-a",
                "jobId": "publish",
                "recommendedAction": "pause_failure_reason",
                "severity": "critical",
                "dominantFailureReason": "rate_limited",
                "failureReasonCounts": {"rate_limited": 1},
                "suppressUntilUnixSeconds": suppress_until,
                "message": "pause rate-limited publish jobs"
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let response = compact_identity_ledgers(
            std::slice::from_ref(&release_ledger_path),
            std::slice::from_ref(&runtime_risk_path),
            Some(60),
            Some("publish"),
            Some("worker-a"),
            None,
            5,
            5,
            None,
            None,
            Some(&compact_path),
        )
        .await
        .unwrap();
        let value = response.into_value();
        let compact_report: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&compact_path).await.unwrap()).unwrap();

        assert_eq!(value["data"]["scope"], "identity_ledger_compact");
        assert_eq!(value["data"]["releaseEventReadCount"], 2);
        assert_eq!(value["data"]["releaseEventCount"], 1);
        assert_eq!(value["data"]["runtimeRiskEventReadCount"], 1);
        assert_eq!(value["data"]["runtimeRiskEventCount"], 1);
        assert_eq!(value["data"]["sourceEventCount"], 3);
        assert_eq!(value["data"]["compactedEventCount"], 2);
        assert_eq!(value["data"]["assetSummaryCount"], 1);
        assert_eq!(value["data"]["failureReasonCounts"]["rate_limited"], 1);
        assert_eq!(value["data"]["activeSuppressionCount"], 1);
        assert_eq!(
            value["data"]["nextSuppressionUntilUnixSeconds"],
            suppress_until
        );
        assert_eq!(value["data"]["assetSummaries"][0]["accountId"], "acct-a");
        assert_eq!(
            value["data"]["retainedRuntimeRiskEvidence"][0]["recommendedAction"],
            "pause_failure_reason"
        );
        assert_eq!(
            value["data"]["out"]["format"],
            "json_identity_ledger_compact"
        );
        assert_eq!(compact_report["scope"], "identity_ledger_compact");
        assert_eq!(compact_report["activeSuppressionCount"], 1);

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_ledger_compact_uses_checkpoint_offsets_and_carries_suppression() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-ledger-checkpoint-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let release_ledger_path = root.join("runtime-release.ndjson");
        let runtime_risk_path = root.join("runtime-risk.ndjson");
        let first_compact_path = root.join("first-compact.json");
        let checkpoint_path = root.join("ledger-checkpoint.json");
        let second_compact_path = root.join("second-compact.json");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let now = unix_seconds();
        let suppress_until = now + 600;
        tokio::fs::write(
            &release_ledger_path,
            format!(
                "{}\n",
                serde_json::to_string(&json!({
                    "scope": "identity_assets_release",
                    "generatedAtUnixSeconds": now.saturating_sub(10),
                    "status": "failed",
                    "workerId": "worker-a",
                    "jobId": "publish",
                    "result": {"failureReason": "rate_limited"},
                    "item": {
                        "assetIndex": 0,
                        "status": "failed",
                        "accountId": "acct-a",
                        "label": "acct-a",
                        "workerId": "worker-a",
                        "jobId": "publish"
                    }
                }))
                .unwrap()
            ),
        )
        .await
        .unwrap();
        tokio::fs::write(
            &runtime_risk_path,
            format!(
                "{}\n",
                serde_json::to_string(&json!({
                    "scope": "identity_job_runtime_risk_event",
                    "generatedAtUnixSeconds": now.saturating_sub(10),
                    "workerId": "worker-a",
                    "jobId": "publish",
                    "recommendedAction": "pause_failure_reason",
                    "severity": "critical",
                    "dominantFailureReason": "rate_limited",
                    "failureReasonCounts": {"rate_limited": 1},
                    "suppressUntilUnixSeconds": suppress_until,
                    "message": "pause rate-limited publish jobs"
                }))
                .unwrap()
            ),
        )
        .await
        .unwrap();

        compact_identity_ledgers(
            std::slice::from_ref(&release_ledger_path),
            std::slice::from_ref(&runtime_risk_path),
            Some(60),
            Some("publish"),
            Some("worker-a"),
            None,
            5,
            5,
            None,
            Some(&checkpoint_path),
            Some(&first_compact_path),
        )
        .await
        .unwrap();

        let mut release_file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&release_ledger_path)
            .await
            .unwrap();
        release_file
            .write_all(
                format!(
                    "{}\n",
                    serde_json::to_string(&json!({
                        "scope": "identity_assets_release",
                        "generatedAtUnixSeconds": now,
                        "status": "succeeded",
                        "workerId": "worker-a",
                        "jobId": "publish",
                        "item": {
                            "assetIndex": 1,
                            "status": "succeeded",
                            "accountId": "acct-b",
                            "label": "acct-b",
                            "workerId": "worker-a",
                            "jobId": "publish"
                        }
                    }))
                    .unwrap()
                )
                .as_bytes(),
            )
            .await
            .unwrap();

        let response = compact_identity_ledgers(
            std::slice::from_ref(&release_ledger_path),
            std::slice::from_ref(&runtime_risk_path),
            Some(60),
            Some("publish"),
            Some("worker-a"),
            None,
            5,
            5,
            Some(&checkpoint_path),
            Some(&checkpoint_path),
            Some(&second_compact_path),
        )
        .await
        .unwrap();
        let value = response.into_value();
        let second_compact: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&second_compact_path)
                .await
                .unwrap(),
        )
        .unwrap();

        assert_eq!(value["data"]["incremental"], true);
        assert_eq!(value["data"]["releaseEventReadCount"], 1);
        assert_eq!(value["data"]["runtimeRiskEventReadCount"], 0);
        assert_eq!(value["data"]["carriedActiveSuppressionCount"], 1);
        assert_eq!(value["data"]["activeSuppressionCount"], 1);
        assert_eq!(value["data"]["releaseStatusCounts"]["succeeded"], 1);
        assert_eq!(
            value["data"]["sourceCheckpoints"][0]["readFromByte"]
                .as_u64()
                .unwrap()
                > 0,
            true
        );
        assert_eq!(
            value["data"]["checkpointOut"]["format"],
            "json_identity_ledger_checkpoint"
        );
        assert_eq!(second_compact["scope"], "identity_ledger_compact");
        assert_eq!(second_compact["activeSuppressionCount"], 1);

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_ledger_dashboard_writes_json_and_html() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-ledger-dashboard-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let release_ledger_path = root.join("runtime-release.ndjson");
        let runtime_risk_path = root.join("runtime-risk.ndjson");
        let dashboard_path = root.join("ledger-dashboard.json");
        let html_path = root.join("ledger-dashboard.html");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let now = unix_seconds();
        let suppress_until = now + 600;
        tokio::fs::write(
            &release_ledger_path,
            serde_json::to_string(&json!({
                "scope": "identity_assets_release",
                "generatedAtUnixSeconds": now.saturating_sub(10),
                "status": "failed",
                "workerId": "worker-a",
                "jobId": "publish",
                "message": "rate limited",
                "result": {"failureReason": "rate_limited"},
                "item": {
                    "assetIndex": 0,
                    "status": "failed",
                    "accountId": "acct-a",
                    "profileId": "profile-a",
                    "identityId": "fp-a",
                    "label": "acct-a",
                    "profileDir": "/profiles/acct-a",
                    "workerId": "worker-a",
                    "jobId": "publish"
                }
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        tokio::fs::write(
            &runtime_risk_path,
            serde_json::to_string(&json!({
                "scope": "identity_job_runtime_risk_event",
                "generatedAtUnixSeconds": now.saturating_sub(3_600),
                "workerId": "worker-a",
                "jobId": "publish",
                "recommendedAction": "pause_failure_reason",
                "severity": "critical",
                "dominantFailureReason": "rate_limited",
                "failureReasonCounts": {"rate_limited": 1},
                "suppressUntilUnixSeconds": suppress_until,
                "message": "pause rate-limited publish jobs"
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let response = dashboard_identity_ledgers(
            std::slice::from_ref(&release_ledger_path),
            std::slice::from_ref(&runtime_risk_path),
            Some(60),
            Some("publish"),
            Some("worker-a"),
            None,
            5,
            5,
            None,
            None,
            Some(&dashboard_path),
            Some(&html_path),
        )
        .await
        .unwrap();
        let value = response.into_value();
        let dashboard_report: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&dashboard_path).await.unwrap())
                .unwrap();
        let html = tokio::fs::read_to_string(&html_path).await.unwrap();

        assert_eq!(value["data"]["scope"], "identity_ledger_dashboard");
        assert_eq!(value["data"]["summary"]["status"], "blocked");
        assert_eq!(
            value["data"]["summary"]["recommendedAction"],
            "honor_active_suppression"
        );
        assert_eq!(value["data"]["compact"]["scope"], "identity_ledger_compact");
        assert_eq!(
            value["data"]["htmlOut"]["format"],
            "html_identity_ledger_dashboard"
        );
        assert_eq!(
            value["data"]["out"]["format"],
            "json_identity_ledger_dashboard"
        );
        assert_eq!(dashboard_report["scope"], "identity_ledger_dashboard");
        assert!(html.contains("drs identity ledger dashboard"));
        assert!(html.contains("acct-a"));
        assert!(html.contains("pause_failure_reason"));

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_ledger_query_aggregates_release_and_runtime_risk_ledgers() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-ledger-query-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let release_ledger_path = root.join("runtime-release.ndjson");
        let runtime_risk_path = root.join("runtime-risk.ndjson");
        let query_path = root.join("ledger-query.json");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let now = unix_seconds();
        tokio::fs::write(
            &release_ledger_path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&json!({
                    "scope": "identity_assets_release",
                    "generatedAtUnixSeconds": now.saturating_sub(10),
                    "status": "failed",
                    "workerId": "worker-a",
                    "jobId": "publish",
                    "message": "rate limited",
                    "result": {"failureReason": "rate_limited"},
                    "item": {
                        "assetIndex": 0,
                        "status": "failed",
                        "accountId": "acct-a",
                        "profileId": "profile-a",
                        "identityId": "fp-a",
                        "label": "acct-a",
                        "profileDir": "/profiles/acct-a",
                        "leaseId": "lease-a",
                        "workerId": "worker-a",
                        "jobId": "publish"
                    }
                }))
                .unwrap(),
                serde_json::to_string(&json!({
                    "scope": "identity_assets_release",
                    "generatedAtUnixSeconds": now.saturating_sub(5),
                    "status": "succeeded",
                    "workerId": "worker-a",
                    "jobId": "publish",
                    "item": {
                        "assetIndex": 1,
                        "status": "succeeded",
                        "accountId": "acct-b",
                        "label": "acct-b",
                        "workerId": "worker-a",
                        "jobId": "publish"
                    }
                }))
                .unwrap()
            ),
        )
        .await
        .unwrap();
        tokio::fs::write(
            &runtime_risk_path,
            serde_json::to_string(&json!({
                "scope": "identity_job_runtime_risk_event",
                "generatedAtUnixSeconds": now.saturating_sub(3_600),
                "workerId": "worker-a",
                "jobId": "publish",
                "recommendedAction": "pause_failure_reason",
                "severity": "critical",
                "dominantFailureReason": "rate_limited",
                "failureReasonCounts": {"rate_limited": 1},
                "runtimeRiskCooldownSeconds": 1_800,
                "suppressUntilUnixSeconds": now + 600,
                "message": "pause rate-limited publish jobs"
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let response = query_identity_ledgers(
            std::slice::from_ref(&release_ledger_path),
            std::slice::from_ref(&runtime_risk_path),
            Some(60),
            Some("publish"),
            Some("worker-a"),
            Some("rate_limited"),
            5,
            Some(&query_path),
        )
        .await
        .unwrap();
        let value = response.into_value();
        let query_report: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&query_path).await.unwrap()).unwrap();

        assert_eq!(value["data"]["scope"], "identity_ledger_query");
        assert_eq!(value["data"]["releaseEventReadCount"], 2);
        assert_eq!(value["data"]["releaseEventCount"], 1);
        assert_eq!(value["data"]["runtimeRiskEventReadCount"], 1);
        assert_eq!(value["data"]["runtimeRiskEventCount"], 1);
        assert_eq!(value["data"]["releaseStatusCounts"]["failed"], 1);
        assert_eq!(value["data"]["failureReasonCounts"]["rate_limited"], 1);
        assert_eq!(
            value["data"]["runtimeRiskActionCounts"]["pause_failure_reason"],
            1
        );
        assert_eq!(value["data"]["activeSuppressionCount"], 1);
        assert_eq!(value["data"]["topAssets"][0]["key"], "label:acct-a");
        assert_eq!(
            value["data"]["topAssets"][0]["lastFailureReason"],
            "rate_limited"
        );
        assert_eq!(
            value["data"]["activeSuppressions"][0]["failureReason"],
            "rate_limited"
        );
        assert_eq!(value["data"]["out"]["format"], "json_identity_ledger_query");
        assert_eq!(query_report["scope"], "identity_ledger_query");
        assert_eq!(query_report["activeSuppressionCount"], 1);

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_ledger_explain_reports_blocking_suppression_for_asset() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-ledger-explain-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let release_ledger_path = root.join("runtime-release.ndjson");
        let runtime_risk_path = root.join("runtime-risk.ndjson");
        let explain_path = root.join("ledger-explain.json");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let now = unix_seconds();
        let suppress_until = now + 600;
        tokio::fs::write(
            &release_ledger_path,
            serde_json::to_string(&json!({
                "scope": "identity_assets_release",
                "generatedAtUnixSeconds": now.saturating_sub(10),
                "status": "failed",
                "workerId": "worker-a",
                "jobId": "publish",
                "cooldownUntilUnixSeconds": now + 300,
                "message": "rate limited",
                "result": {"failureReason": "rate_limited"},
                "item": {
                    "assetIndex": 0,
                    "status": "failed",
                    "accountId": "acct-a",
                    "profileId": "profile-a",
                    "identityId": "fp-a",
                    "label": "acct-a",
                    "profileDir": "/profiles/acct-a",
                    "leaseId": "lease-a",
                    "workerId": "worker-a",
                    "jobId": "publish"
                }
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        tokio::fs::write(
            &runtime_risk_path,
            serde_json::to_string(&json!({
                "scope": "identity_job_runtime_risk_event",
                "generatedAtUnixSeconds": now.saturating_sub(3_600),
                "workerId": "worker-a",
                "jobId": "publish",
                "recommendedAction": "pause_failure_reason",
                "severity": "critical",
                "dominantFailureReason": "rate_limited",
                "failureReasonCounts": {"rate_limited": 1},
                "runtimeRiskCooldownSeconds": 1_800,
                "suppressUntilUnixSeconds": suppress_until,
                "message": "pause rate-limited publish jobs"
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let response = explain_identity_ledger(
            std::slice::from_ref(&release_ledger_path),
            std::slice::from_ref(&runtime_risk_path),
            Some(60),
            Some("publish"),
            Some("worker-a"),
            Some("rate_limited"),
            Some("acct-a"),
            Some("profile-a"),
            Some("fp-a"),
            Some("acct-a"),
            Some("/profiles/acct-a"),
            Some("lease-a"),
            5,
            Some(&explain_path),
        )
        .await
        .unwrap();
        let value = response.into_value();
        let explain_report: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&explain_path).await.unwrap()).unwrap();

        assert_eq!(value["data"]["scope"], "identity_ledger_explain");
        assert_eq!(
            value["data"]["decision"],
            "blocked_by_runtime_risk_suppression"
        );
        assert_eq!(value["data"]["blockedByActiveSuppression"], true);
        assert_eq!(value["data"]["blockedByCooldown"], true);
        assert_eq!(value["data"]["releaseEventCount"], 1);
        assert_eq!(value["data"]["runtimeRiskEventCount"], 1);
        assert_eq!(value["data"]["activeSuppressionCount"], 1);
        assert_eq!(value["data"]["activeCooldownCount"], 1);
        assert_eq!(value["data"]["latestReleaseFailureReason"], "rate_limited");
        assert_eq!(value["data"]["nextRunnableUnixSeconds"], suppress_until);
        assert_eq!(value["data"]["releaseEvidence"][0]["accountId"], "acct-a");
        assert_eq!(
            value["data"]["runtimeRiskEvidence"][0]["recommendedAction"],
            "pause_failure_reason"
        );
        assert_eq!(
            value["data"]["out"]["format"],
            "json_identity_ledger_explain"
        );
        assert_eq!(explain_report["scope"], "identity_ledger_explain");
        assert_eq!(explain_report["activeSuppressionCount"], 1);

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_job_run_wraps_command_and_releases_runtime_lease() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-job-run-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let profile = root.join("profile-a");
        let manifest_path = root.join("profile-assets.json");
        let working_manifest_path = root.join("working-profile-assets.json");
        let release_path = root.join("runtime-release.ndjson");
        let runtime_risk_path = root.join("runtime-risk.ndjson");
        let explain_path = root.join("explain.json");
        let job_path = root.join("job.json");
        tokio::fs::create_dir_all(&profile).await.unwrap();
        tokio::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&json!({
                "profileAssets": [{
                    "accountId": "acct-a",
                    "profileId": "profile-a",
                    "identityId": "fp-a",
                    "label": "acct-a",
                    "profileDir": profile.display().to_string(),
                    "state": "active"
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let command = if cfg!(windows) {
            vec![
                "cmd".to_string(),
                "/C".to_string(),
                "echo selected=%DRS_IDENTITY_SELECTED_COUNT%".to_string(),
            ]
        } else {
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "printf selected=$DRS_IDENTITY_SELECTED_COUNT".to_string(),
            ]
        };

        let response = run_identity_job(IdentityJobRunOptions {
            asset_manifest: manifest_path.clone(),
            policy: None,
            job_preset: None,
            desired_concurrency: None,
            limit: Some(1),
            worker: Some("worker-a".to_string()),
            job: Some("publish".to_string()),
            lease_seconds: Some(300),
            max_wait_seconds: Some(0),
            allow_wait: false,
            per_asset: false,
            child_concurrency: None,
            runtime_renew_interval_seconds: None,
            child_timeout_seconds: None,
            child_result_dir: None,
            max_failed_assets: None,
            max_failed_assets_per_reason: None,
            allow_states: Vec::new(),
            include_dispatch_leased: false,
            include_retry: false,
            include_failed: false,
            include_cancelled: false,
            include_runtime_leased: false,
            include_missing_profile_dir: false,
            skip_sweep: false,
            skip_validate: false,
            runtime_grace_seconds: Some(0),
            dispatch_grace_seconds: Some(0),
            cooldown_grace_seconds: Some(0),
            failure_cooldown_seconds: Some(600),
            failure_next_state: Some("repair".to_string()),
            failure_reason_rules: BTreeMap::new(),
            asset_manifest_out: Some(working_manifest_path.clone()),
            sweep_out: None,
            validate_out: None,
            gate_out: None,
            selection_out: None,
            release_out: Some(release_path.clone()),
            append_release: true,
            runtime_risk_ledgers: Vec::new(),
            runtime_risk_window_seconds: None,
            runtime_risk_out: Some(runtime_risk_path.clone()),
            append_runtime_risk: true,
            explain_out: Some(explain_path.clone()),
            job_out: Some(job_path.clone()),
            command,
        })
        .await
        .unwrap();
        let value = response.into_value();
        let manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&working_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();
        let release_lines = tokio::fs::read_to_string(&release_path).await.unwrap();
        let runtime_risk_lines = tokio::fs::read_to_string(&runtime_risk_path)
            .await
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let job_report: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&job_path).await.unwrap()).unwrap();
        let explain_report: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&explain_path).await.unwrap()).unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["scope"], "identity_job_run");
        assert_eq!(value["data"]["phase"], "complete");
        assert_eq!(value["data"]["passed"], true);
        assert_eq!(value["data"]["exitCode"], 0);
        assert_eq!(value["data"]["selection"]["selectedCount"], 1);
        assert_eq!(value["data"]["release"]["releasedCount"], 1);
        assert_eq!(value["data"]["runtimeRisk"]["severity"], "healthy");
        assert_eq!(
            value["data"]["runtimeRisk"]["recommendedAction"],
            "continue_current"
        );
        assert_eq!(value["data"]["runtimeRisk"]["nextSuggestedLimit"], 1);
        assert_eq!(
            value["data"]["runtimeRiskOut"]["format"],
            "ndjson_runtime_risk_events"
        );
        assert_eq!(runtime_risk_lines.len(), 1);
        assert_eq!(
            runtime_risk_lines[0]["scope"],
            "identity_job_runtime_risk_event"
        );
        assert_eq!(
            runtime_risk_lines[0]["riskScope"],
            "identity_job_runtime_risk"
        );
        assert_eq!(runtime_risk_lines[0]["jobId"], "publish");
        assert_eq!(runtime_risk_lines[0]["workerId"], "worker-a");
        assert_eq!(
            runtime_risk_lines[0]["recommendedAction"],
            "continue_current"
        );
        assert_eq!(runtime_risk_lines[0]["nextSuggestedLimit"], 1);
        assert!(
            value["data"]["child"]["stdout"]
                .as_str()
                .unwrap_or_default()
                .contains("selected=1")
        );
        assert_eq!(
            manifest["profileAssets"][0]["runtimeLeaseState"],
            "released"
        );
        assert_eq!(
            manifest["profileAssets"][0]["lastRuntimeStatus"],
            "succeeded"
        );
        assert!(manifest["profileAssets"][0].get("runtimeLeaseId").is_none());
        assert!(release_lines.contains(r#""status":"succeeded""#));
        assert_eq!(job_report["scope"], "identity_job_run");
        assert_eq!(job_report["runtimeRisk"]["severity"], "healthy");
        assert_eq!(value["data"]["explain"]["scope"], "identity_job_explain");
        assert_eq!(value["data"]["explainOut"]["format"], "json_explain");
        assert_eq!(explain_report["finalDecision"], "run_completed");
        assert!(
            explain_report["stageDecisions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|stage| stage["stage"] == "select"
                    && stage["decision"] == "runtime_leases_acquired")
        );
        assert!(
            explain_report["assetDecisions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|asset| asset["phase"] == "select"
                    && asset["decision"] == "selected"
                    && asset["asset"]["label"] == "acct-a")
        );
        assert!(
            explain_report["assetDecisions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|asset| asset["phase"] == "release"
                    && asset["status"] == "succeeded"
                    && asset["asset"]["label"] == "acct-a")
        );

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_job_run_runtime_risk_ledger_blocks_before_leasing() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-job-run-risk-gate-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let profile = root.join("profile-a");
        let manifest_path = root.join("profile-assets.json");
        let working_manifest_path = root.join("working-profile-assets.json");
        let risk_ledger_path = root.join("runtime-risk.ndjson");
        let run_log_path = root.join("child-ran.txt");
        tokio::fs::create_dir_all(&profile).await.unwrap();
        tokio::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&json!({
                "profileAssets": [{
                    "accountId": "acct-a",
                    "profileId": "profile-a",
                    "identityId": "fp-a",
                    "label": "acct-a",
                    "profileDir": profile.display().to_string(),
                    "state": "active"
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        let generated_at = unix_seconds().saturating_sub(3600);
        let suppress_until = unix_seconds().saturating_add(600);
        tokio::fs::write(
            &risk_ledger_path,
            format!(
                "{}\n",
                serde_json::to_string(&json!({
                    "scope": "identity_job_runtime_risk_event",
                    "generatedAtUnixSeconds": generated_at,
                    "jobId": "publish",
                    "severity": "critical",
                    "recommendedAction": "pause_failure_reason",
                    "nextSuggestedLimit": 0,
                    "nextSuggestedDesiredConcurrency": 0,
                    "suppressUntilUnixSeconds": suppress_until,
                    "dominantFailureReason": "rate_limited",
                    "message": "pause rate limited jobs"
                }))
                .unwrap()
            ),
        )
        .await
        .unwrap();

        let command = if cfg!(windows) {
            vec![
                "cmd".to_string(),
                "/C".to_string(),
                format!("echo ran>{}", run_log_path.display()),
            ]
        } else {
            vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("printf ran > {}", run_log_path.display()),
            ]
        };

        let response = run_identity_job(IdentityJobRunOptions {
            asset_manifest: manifest_path.clone(),
            policy: None,
            job_preset: None,
            desired_concurrency: None,
            limit: Some(1),
            worker: Some("worker-a".to_string()),
            job: Some("publish".to_string()),
            lease_seconds: Some(300),
            max_wait_seconds: Some(0),
            allow_wait: false,
            per_asset: false,
            child_concurrency: None,
            runtime_renew_interval_seconds: None,
            child_timeout_seconds: None,
            child_result_dir: None,
            max_failed_assets: None,
            max_failed_assets_per_reason: None,
            allow_states: Vec::new(),
            include_dispatch_leased: false,
            include_retry: false,
            include_failed: false,
            include_cancelled: false,
            include_runtime_leased: false,
            include_missing_profile_dir: false,
            skip_sweep: false,
            skip_validate: false,
            runtime_grace_seconds: Some(0),
            dispatch_grace_seconds: Some(0),
            cooldown_grace_seconds: Some(0),
            failure_cooldown_seconds: None,
            failure_next_state: None,
            failure_reason_rules: BTreeMap::new(),
            asset_manifest_out: Some(working_manifest_path.clone()),
            sweep_out: None,
            validate_out: None,
            gate_out: None,
            selection_out: None,
            release_out: None,
            append_release: false,
            runtime_risk_ledgers: vec![risk_ledger_path.clone()],
            runtime_risk_window_seconds: Some(60),
            runtime_risk_out: None,
            append_runtime_risk: false,
            explain_out: None,
            job_out: None,
            command,
        })
        .await
        .unwrap();
        let value = response.into_value();
        let manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&working_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["phase"], "runtime_risk_gate");
        assert_eq!(value["data"]["passed"], false);
        assert_eq!(value["data"]["exitCode"], 2);
        assert_eq!(value["data"]["selection"], Value::Null);
        assert_eq!(value["data"]["runtimeRiskGate"]["blocked"], true);
        assert_eq!(
            value["data"]["runtimeRiskGate"]["activeSuppressionCount"],
            1
        );
        assert_eq!(value["data"]["runtimeRiskGate"]["consideredEventCount"], 1);
        assert_eq!(
            value["data"]["runtimeRiskGate"]["recommendedAction"],
            "pause_failure_reason"
        );
        assert_eq!(
            value["data"]["runtimeRiskGate"]["failureReason"],
            "rate_limited"
        );
        assert_eq!(
            value["data"]["runtimeRisk"]["recommendedAction"],
            "pause_failure_reason"
        );
        assert_eq!(
            value["data"]["explain"]["finalDecision"],
            "blocked_before_child"
        );
        assert!(
            value["data"]["explain"]["stageDecisions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|stage| stage["stage"] == "runtime_risk_gate"
                    && stage["status"] == "blocked"
                    && stage["decision"] == "pause_failure_reason")
        );
        assert!(
            value["data"]["explain"]["assetDecisions"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(manifest["profileAssets"][0].get("runtimeLeaseId").is_none());
        assert!(
            manifest["profileAssets"][0]
                .get("lastRuntimeStatus")
                .is_none()
        );
        assert!(!run_log_path.exists());

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_job_run_runtime_risk_ledger_reduces_next_limit() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-job-run-risk-reduce-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let profile_a = root.join("profile-a");
        let profile_b = root.join("profile-b");
        let manifest_path = root.join("profile-assets.json");
        let working_manifest_path = root.join("working-profile-assets.json");
        let risk_ledger_path = root.join("runtime-risk.ndjson");
        tokio::fs::create_dir_all(&profile_a).await.unwrap();
        tokio::fs::create_dir_all(&profile_b).await.unwrap();
        tokio::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&json!({
                "profileAssets": [{
                    "accountId": "acct-a",
                    "profileId": "profile-a",
                    "identityId": "fp-a",
                    "label": "acct-a",
                    "profileDir": profile_a.display().to_string(),
                    "state": "active"
                },{
                    "accountId": "acct-b",
                    "profileId": "profile-b",
                    "identityId": "fp-b",
                    "label": "acct-b",
                    "profileDir": profile_b.display().to_string(),
                    "state": "active"
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        tokio::fs::write(
            &risk_ledger_path,
            format!(
                "{}\n",
                serde_json::to_string(&json!({
                    "scope": "identity_job_runtime_risk_event",
                    "generatedAtUnixSeconds": unix_seconds(),
                    "jobId": "publish",
                    "severity": "high",
                    "recommendedAction": "reduce_concurrency",
                    "nextSuggestedLimit": 1,
                    "nextSuggestedDesiredConcurrency": 1,
                    "message": "reduce next publish run"
                }))
                .unwrap()
            ),
        )
        .await
        .unwrap();

        let command = if cfg!(windows) {
            vec![
                "cmd".to_string(),
                "/C".to_string(),
                "echo selected=%DRS_IDENTITY_SELECTED_COUNT%".to_string(),
            ]
        } else {
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "printf selected=$DRS_IDENTITY_SELECTED_COUNT".to_string(),
            ]
        };

        let response = run_identity_job(IdentityJobRunOptions {
            asset_manifest: manifest_path.clone(),
            policy: None,
            job_preset: None,
            desired_concurrency: None,
            limit: Some(2),
            worker: Some("worker-a".to_string()),
            job: Some("publish".to_string()),
            lease_seconds: Some(300),
            max_wait_seconds: Some(0),
            allow_wait: false,
            per_asset: false,
            child_concurrency: None,
            runtime_renew_interval_seconds: None,
            child_timeout_seconds: None,
            child_result_dir: None,
            max_failed_assets: None,
            max_failed_assets_per_reason: None,
            allow_states: Vec::new(),
            include_dispatch_leased: false,
            include_retry: false,
            include_failed: false,
            include_cancelled: false,
            include_runtime_leased: false,
            include_missing_profile_dir: false,
            skip_sweep: false,
            skip_validate: false,
            runtime_grace_seconds: Some(0),
            dispatch_grace_seconds: Some(0),
            cooldown_grace_seconds: Some(0),
            failure_cooldown_seconds: None,
            failure_next_state: None,
            failure_reason_rules: BTreeMap::new(),
            asset_manifest_out: Some(working_manifest_path.clone()),
            sweep_out: None,
            validate_out: None,
            gate_out: None,
            selection_out: None,
            release_out: None,
            append_release: false,
            runtime_risk_ledgers: vec![risk_ledger_path.clone()],
            runtime_risk_window_seconds: Some(3600),
            runtime_risk_out: None,
            append_runtime_risk: false,
            explain_out: None,
            job_out: None,
            command,
        })
        .await
        .unwrap();
        let value = response.into_value();
        let manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&working_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["phase"], "complete");
        assert_eq!(value["data"]["passed"], true);
        assert_eq!(value["data"]["limit"], 1);
        assert_eq!(value["data"]["desiredConcurrency"], 1);
        assert_eq!(value["data"]["selection"]["selectedCount"], 1);
        assert_eq!(value["data"]["runtimeRiskGate"]["adjusted"], true);
        assert_eq!(value["data"]["runtimeRiskGate"]["originalLimit"], 2);
        assert_eq!(value["data"]["runtimeRiskGate"]["nextSuggestedLimit"], 1);
        assert!(
            value["data"]["child"]["stdout"]
                .as_str()
                .unwrap_or_default()
                .contains("selected=1")
        );
        assert_eq!(
            manifest["profileAssets"][0]["lastRuntimeStatus"],
            "succeeded"
        );
        assert!(
            manifest["profileAssets"][1]
                .get("lastRuntimeStatus")
                .is_none()
        );

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_job_run_per_asset_releases_each_child_result() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-job-run-per-asset-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let profile_a = root.join("profile-a");
        let profile_b = root.join("profile-b");
        let manifest_path = root.join("profile-assets.json");
        let working_manifest_path = root.join("working-profile-assets.json");
        let release_path = root.join("runtime-release.ndjson");
        tokio::fs::create_dir_all(&profile_a).await.unwrap();
        tokio::fs::create_dir_all(&profile_b).await.unwrap();
        tokio::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&json!({
                "profileAssets": [{
                    "accountId": "acct-a",
                    "profileId": "profile-a",
                    "identityId": "fp-a",
                    "label": "acct-a",
                    "profileDir": profile_a.display().to_string(),
                    "state": "active"
                },{
                    "accountId": "acct-b",
                    "profileId": "profile-b",
                    "identityId": "fp-b",
                    "label": "acct-b",
                    "profileDir": profile_b.display().to_string(),
                    "state": "active"
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let command = if cfg!(windows) {
            vec![
                "cmd".to_string(),
                "/C".to_string(),
                "if \"%DRS_IDENTITY_LABEL%\"==\"acct-b\" (exit /b 7) else (echo %DRS_IDENTITY_LABEL%)"
                    .to_string(),
            ]
        } else {
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "if [ \"$DRS_IDENTITY_LABEL\" = acct-b ]; then exit 7; else printf $DRS_IDENTITY_LABEL; fi"
                    .to_string(),
            ]
        };

        let response = run_identity_job(IdentityJobRunOptions {
            asset_manifest: manifest_path.clone(),
            policy: None,
            job_preset: None,
            desired_concurrency: None,
            limit: Some(2),
            worker: Some("worker-a".to_string()),
            job: Some("publish".to_string()),
            lease_seconds: Some(300),
            max_wait_seconds: Some(0),
            allow_wait: false,
            per_asset: true,
            child_concurrency: Some(2),
            runtime_renew_interval_seconds: None,
            child_timeout_seconds: None,
            child_result_dir: None,
            max_failed_assets: None,
            max_failed_assets_per_reason: None,
            allow_states: Vec::new(),
            include_dispatch_leased: false,
            include_retry: false,
            include_failed: false,
            include_cancelled: false,
            include_runtime_leased: false,
            include_missing_profile_dir: false,
            skip_sweep: false,
            skip_validate: false,
            runtime_grace_seconds: Some(0),
            dispatch_grace_seconds: Some(0),
            cooldown_grace_seconds: Some(0),
            failure_cooldown_seconds: Some(600),
            failure_next_state: Some("repair".to_string()),
            failure_reason_rules: BTreeMap::new(),
            asset_manifest_out: Some(working_manifest_path.clone()),
            sweep_out: None,
            validate_out: None,
            gate_out: None,
            selection_out: None,
            release_out: Some(release_path.clone()),
            append_release: true,
            runtime_risk_ledgers: Vec::new(),
            runtime_risk_window_seconds: None,
            runtime_risk_out: None,
            append_runtime_risk: false,
            explain_out: None,
            job_out: None,
            command,
        })
        .await
        .unwrap();
        let value = response.into_value();
        let manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&working_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();
        let release_lines = tokio::fs::read_to_string(&release_path)
            .await
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["phase"], "complete");
        assert_eq!(value["data"]["passed"], false);
        assert_eq!(value["data"]["exitCode"], 7);
        assert_eq!(value["data"]["child"]["mode"], "per_asset");
        assert_eq!(value["data"]["child"]["childConcurrency"], 2);
        assert_eq!(value["data"]["child"]["childCount"], 2);
        assert_eq!(value["data"]["child"]["succeededCount"], 1);
        assert_eq!(value["data"]["child"]["failedCount"], 1);
        assert_eq!(value["data"]["release"]["releasedCount"], 2);
        assert_eq!(value["data"]["release"]["succeededCount"], 1);
        assert_eq!(value["data"]["release"]["failedCount"], 1);
        assert_eq!(
            value["data"]["child"]["children"][0]["child"]["stdout"],
            "acct-a"
        );
        assert_eq!(
            value["data"]["child"]["children"][1]["child"]["exitCode"],
            7
        );
        assert_eq!(
            manifest["profileAssets"][0]["lastRuntimeStatus"],
            "succeeded"
        );
        assert_eq!(manifest["profileAssets"][1]["lastRuntimeStatus"], "failed");
        assert_eq!(manifest["profileAssets"][1]["state"], "repair");
        assert!(manifest["profileAssets"][1]["cooldownUntilUnixSeconds"].is_number());
        assert_eq!(release_lines.len(), 2);
        assert_eq!(release_lines[0]["status"], "succeeded");
        assert_eq!(release_lines[0]["item"]["label"], "acct-a");
        assert_eq!(release_lines[1]["status"], "failed");
        assert_eq!(release_lines[1]["item"]["label"], "acct-b");

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn identity_job_run_per_asset_circuit_breaker_cancels_remaining_assets() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-job-run-circuit-breaker-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let profile_a = root.join("profile-a");
        let profile_b = root.join("profile-b");
        let profile_c = root.join("profile-c");
        let manifest_path = root.join("profile-assets.json");
        let working_manifest_path = root.join("working-profile-assets.json");
        let release_path = root.join("runtime-release.ndjson");
        let run_log_path = root.join("child-runs.txt");
        tokio::fs::create_dir_all(&profile_a).await.unwrap();
        tokio::fs::create_dir_all(&profile_b).await.unwrap();
        tokio::fs::create_dir_all(&profile_c).await.unwrap();
        tokio::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&json!({
                "profileAssets": [{
                    "accountId": "acct-a",
                    "profileId": "profile-a",
                    "identityId": "fp-a",
                    "label": "acct-a",
                    "profileDir": profile_a.display().to_string(),
                    "state": "active"
                },{
                    "accountId": "acct-b",
                    "profileId": "profile-b",
                    "identityId": "fp-b",
                    "label": "acct-b",
                    "profileDir": profile_b.display().to_string(),
                    "state": "active"
                },{
                    "accountId": "acct-c",
                    "profileId": "profile-c",
                    "identityId": "fp-c",
                    "label": "acct-c",
                    "profileDir": profile_c.display().to_string(),
                    "state": "active"
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            r#"printf '%s\n' "$DRS_IDENTITY_LABEL" >> "$1"
if [ "$DRS_IDENTITY_LABEL" = acct-a ]; then
    exit 7
fi"#
            .to_string(),
            "drs-child".to_string(),
            run_log_path.display().to_string(),
        ];

        let response = run_identity_job(IdentityJobRunOptions {
            asset_manifest: manifest_path.clone(),
            policy: None,
            job_preset: None,
            desired_concurrency: None,
            limit: Some(3),
            worker: Some("worker-a".to_string()),
            job: Some("publish".to_string()),
            lease_seconds: Some(300),
            max_wait_seconds: Some(0),
            allow_wait: false,
            per_asset: true,
            child_concurrency: Some(1),
            runtime_renew_interval_seconds: None,
            child_timeout_seconds: None,
            child_result_dir: None,
            max_failed_assets: Some(1),
            max_failed_assets_per_reason: None,
            allow_states: Vec::new(),
            include_dispatch_leased: false,
            include_retry: false,
            include_failed: false,
            include_cancelled: false,
            include_runtime_leased: false,
            include_missing_profile_dir: false,
            skip_sweep: false,
            skip_validate: false,
            runtime_grace_seconds: Some(0),
            dispatch_grace_seconds: Some(0),
            cooldown_grace_seconds: Some(0),
            failure_cooldown_seconds: Some(600),
            failure_next_state: Some("repair".to_string()),
            failure_reason_rules: BTreeMap::new(),
            asset_manifest_out: Some(working_manifest_path.clone()),
            sweep_out: None,
            validate_out: None,
            gate_out: None,
            selection_out: None,
            release_out: Some(release_path.clone()),
            append_release: true,
            runtime_risk_ledgers: Vec::new(),
            runtime_risk_window_seconds: None,
            runtime_risk_out: None,
            append_runtime_risk: false,
            explain_out: None,
            job_out: None,
            command,
        })
        .await
        .unwrap();
        let value = response.into_value();
        let manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&working_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();
        let run_log = tokio::fs::read_to_string(&run_log_path).await.unwrap();
        let release_lines = tokio::fs::read_to_string(&release_path)
            .await
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["phase"], "complete");
        assert_eq!(value["data"]["passed"], false);
        assert_eq!(value["data"]["exitCode"], 7);
        assert_eq!(value["data"]["child"]["mode"], "per_asset");
        assert_eq!(value["data"]["child"]["selectedCount"], 3);
        assert_eq!(value["data"]["child"]["childCount"], 1);
        assert_eq!(value["data"]["child"]["succeededCount"], 0);
        assert_eq!(value["data"]["child"]["failedCount"], 1);
        assert_eq!(value["data"]["child"]["skippedCount"], 2);
        assert_eq!(value["data"]["child"]["cancelledCount"], 2);
        assert_eq!(value["data"]["child"]["circuitBreaker"]["enabled"], true);
        assert_eq!(value["data"]["child"]["circuitBreaker"]["tripped"], true);
        assert_eq!(
            value["data"]["child"]["circuitBreaker"]["maxFailedAssets"],
            1
        );
        assert_eq!(value["data"]["runtimeRisk"]["severity"], "critical");
        assert_eq!(
            value["data"]["runtimeRisk"]["recommendedAction"],
            "pause_pool"
        );
        assert_eq!(value["data"]["runtimeRisk"]["nextSuggestedLimit"], 0);
        assert_eq!(
            value["data"]["runtimeRisk"]["circuitBreakerKind"],
            "failed_assets"
        );
        assert_eq!(value["data"]["release"]["releasedCount"], 3);
        assert_eq!(value["data"]["release"]["failedCount"], 1);
        assert_eq!(value["data"]["release"]["cancelledCount"], 2);
        assert_eq!(value["data"]["release"]["items"][1]["skipped"], true);
        assert_eq!(value["data"]["release"]["items"][1]["status"], "cancelled");
        assert_eq!(run_log.lines().collect::<Vec<_>>(), vec!["acct-a"]);

        assert_eq!(manifest["profileAssets"][0]["lastRuntimeStatus"], "failed");
        assert_eq!(manifest["profileAssets"][0]["state"], "repair");
        assert_eq!(
            manifest["profileAssets"][0]["runtimeLeaseState"],
            "released"
        );
        assert_eq!(
            manifest["profileAssets"][1]["lastRuntimeStatus"],
            "cancelled"
        );
        assert_eq!(manifest["profileAssets"][1]["state"], "active");
        assert_eq!(
            manifest["profileAssets"][1]["runtimeLeaseState"],
            "released"
        );
        assert_eq!(
            manifest["profileAssets"][1]["lastRuntimeResult"]["cancelledByCircuitBreaker"],
            true
        );
        assert!(manifest["profileAssets"][1].get("runtimeLeaseId").is_none());
        assert_eq!(
            manifest["profileAssets"][2]["lastRuntimeStatus"],
            "cancelled"
        );
        assert!(manifest["profileAssets"][2].get("runtimeLeaseId").is_none());
        assert_eq!(release_lines.len(), 3);
        assert_eq!(release_lines[0]["status"], "failed");
        assert_eq!(release_lines[1]["status"], "cancelled");
        assert_eq!(release_lines[2]["status"], "cancelled");

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn identity_job_run_per_asset_circuit_breaker_groups_failures_by_reason() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-job-run-reason-breaker-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let profile_a = root.join("profile-a");
        let profile_b = root.join("profile-b");
        let profile_c = root.join("profile-c");
        let profile_d = root.join("profile-d");
        let manifest_path = root.join("profile-assets.json");
        let working_manifest_path = root.join("working-profile-assets.json");
        let result_dir = root.join("child-results");
        let run_log_path = root.join("child-runs.txt");
        tokio::fs::create_dir_all(&profile_a).await.unwrap();
        tokio::fs::create_dir_all(&profile_b).await.unwrap();
        tokio::fs::create_dir_all(&profile_c).await.unwrap();
        tokio::fs::create_dir_all(&profile_d).await.unwrap();
        tokio::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&json!({
                "profileAssets": [{
                    "accountId": "acct-a",
                    "profileId": "profile-a",
                    "identityId": "fp-a",
                    "label": "acct-a",
                    "profileDir": profile_a.display().to_string(),
                    "state": "active"
                },{
                    "accountId": "acct-b",
                    "profileId": "profile-b",
                    "identityId": "fp-b",
                    "label": "acct-b",
                    "profileDir": profile_b.display().to_string(),
                    "state": "active"
                },{
                    "accountId": "acct-c",
                    "profileId": "profile-c",
                    "identityId": "fp-c",
                    "label": "acct-c",
                    "profileDir": profile_c.display().to_string(),
                    "state": "active"
                },{
                    "accountId": "acct-d",
                    "profileId": "profile-d",
                    "identityId": "fp-d",
                    "label": "acct-d",
                    "profileDir": profile_d.display().to_string(),
                    "state": "active"
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            r#"printf '%s\n' "$DRS_IDENTITY_LABEL" >> "$1"
case "$DRS_IDENTITY_LABEL" in
    acct-a|acct-c) reason=rate_limited ;;
    acct-b) reason=captcha_failed ;;
    *) reason=should_not_run ;;
esac
printf '{"status":"failed","message":"%s","result":{"reason":"%s"}}' "$reason" "$reason" > "$DRS_IDENTITY_RESULT_OUT""#
                .to_string(),
            "drs-child".to_string(),
            run_log_path.display().to_string(),
        ];

        let response = run_identity_job(IdentityJobRunOptions {
            asset_manifest: manifest_path.clone(),
            policy: None,
            job_preset: None,
            desired_concurrency: None,
            limit: Some(4),
            worker: Some("worker-a".to_string()),
            job: Some("publish".to_string()),
            lease_seconds: Some(300),
            max_wait_seconds: Some(0),
            allow_wait: false,
            per_asset: true,
            child_concurrency: Some(1),
            runtime_renew_interval_seconds: None,
            child_timeout_seconds: None,
            child_result_dir: Some(result_dir.clone()),
            max_failed_assets: None,
            max_failed_assets_per_reason: Some(2),
            allow_states: Vec::new(),
            include_dispatch_leased: false,
            include_retry: false,
            include_failed: false,
            include_cancelled: false,
            include_runtime_leased: false,
            include_missing_profile_dir: false,
            skip_sweep: false,
            skip_validate: false,
            runtime_grace_seconds: Some(0),
            dispatch_grace_seconds: Some(0),
            cooldown_grace_seconds: Some(0),
            failure_cooldown_seconds: None,
            failure_next_state: None,
            failure_reason_rules: BTreeMap::new(),
            asset_manifest_out: Some(working_manifest_path.clone()),
            sweep_out: None,
            validate_out: None,
            gate_out: None,
            selection_out: None,
            release_out: None,
            append_release: false,
            runtime_risk_ledgers: Vec::new(),
            runtime_risk_window_seconds: None,
            runtime_risk_out: None,
            append_runtime_risk: false,
            explain_out: None,
            job_out: None,
            command,
        })
        .await
        .unwrap();
        let value = response.into_value();
        let manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&working_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();
        let run_log = tokio::fs::read_to_string(&run_log_path).await.unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["phase"], "complete");
        assert_eq!(value["data"]["passed"], false);
        assert_eq!(value["data"]["child"]["selectedCount"], 4);
        assert_eq!(value["data"]["child"]["childCount"], 3);
        assert_eq!(value["data"]["child"]["failedCount"], 3);
        assert_eq!(value["data"]["child"]["skippedCount"], 1);
        assert_eq!(value["data"]["child"]["cancelledCount"], 1);
        assert_eq!(
            value["data"]["child"]["failureReasonCounts"]["rate_limited"],
            2
        );
        assert_eq!(
            value["data"]["child"]["failureReasonCounts"]["captcha_failed"],
            1
        );
        assert_eq!(
            value["data"]["child"]["circuitBreaker"]["kind"],
            "failure_reason"
        );
        assert_eq!(
            value["data"]["child"]["circuitBreaker"]["failureReason"],
            "rate_limited"
        );
        assert_eq!(
            value["data"]["child"]["circuitBreaker"]["failureReasonCount"],
            2
        );
        assert_eq!(value["data"]["runtimeRisk"]["severity"], "critical");
        assert_eq!(
            value["data"]["runtimeRisk"]["recommendedAction"],
            "pause_failure_reason"
        );
        assert_eq!(value["data"]["runtimeRisk"]["nextSuggestedLimit"], 0);
        assert_eq!(
            value["data"]["runtimeRisk"]["dominantFailureReason"],
            "rate_limited"
        );
        assert_eq!(
            value["data"]["runtimeRisk"]["failureReasonCounts"]["rate_limited"],
            2
        );
        assert_eq!(
            value["data"]["runtimeRisk"]["circuitBreakerKind"],
            "failure_reason"
        );
        assert_eq!(value["data"]["release"]["releasedCount"], 4);
        assert_eq!(value["data"]["release"]["failedCount"], 3);
        assert_eq!(value["data"]["release"]["cancelledCount"], 1);
        assert_eq!(
            run_log.lines().collect::<Vec<_>>(),
            vec!["acct-a", "acct-b", "acct-c"]
        );

        assert_eq!(manifest["profileAssets"][0]["lastRuntimeStatus"], "failed");
        assert_eq!(
            manifest["profileAssets"][0]["lastRuntimeResult"]["failureReason"],
            "rate_limited"
        );
        assert_eq!(manifest["profileAssets"][1]["lastRuntimeStatus"], "failed");
        assert_eq!(
            manifest["profileAssets"][1]["lastRuntimeResult"]["failureReason"],
            "captcha_failed"
        );
        assert_eq!(manifest["profileAssets"][2]["lastRuntimeStatus"], "failed");
        assert_eq!(
            manifest["profileAssets"][2]["lastRuntimeResult"]["failureReason"],
            "rate_limited"
        );
        assert_eq!(
            manifest["profileAssets"][3]["lastRuntimeStatus"],
            "cancelled"
        );
        assert_eq!(
            manifest["profileAssets"][3]["lastRuntimeResult"]["cancelledByCircuitBreaker"],
            true
        );
        assert_eq!(
            manifest["profileAssets"][3]["lastRuntimeResult"]["circuitBreaker"]["failureReason"],
            "rate_limited"
        );
        assert!(manifest["profileAssets"][3].get("runtimeLeaseId").is_none());
        assert!(result_dir.join("child-0.json").exists());
        assert!(result_dir.join("child-1.json").exists());
        assert!(result_dir.join("child-2.json").exists());
        assert!(!result_dir.join("child-3.json").exists());

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn identity_job_run_applies_failure_reason_policy_defaults() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-job-run-reason-policy-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let profile = root.join("profile-a");
        let manifest_path = root.join("profile-assets.json");
        let working_manifest_path = root.join("working-profile-assets.json");
        let result_dir = root.join("child-results");
        tokio::fs::create_dir_all(&profile).await.unwrap();
        tokio::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&json!({
                "profileAssets": [{
                    "accountId": "acct-a",
                    "profileId": "profile-a",
                    "identityId": "fp-a",
                    "label": "acct-a",
                    "profileDir": profile.display().to_string(),
                    "state": "active"
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            r#"printf '%s' '{"status":"failed","reason":"rate_limited","message":"rate limited"}' > "$DRS_IDENTITY_RESULT_OUT""#
                .to_string(),
        ];
        let mut failure_reason_rules = BTreeMap::new();
        failure_reason_rules.insert(
            "rate_limited".to_string(),
            IdentityJobFailureReasonRule {
                cooldown_seconds: Some(60),
                next_state: Some("repair".to_string()),
                recommended_action: Some("pause_failure_reason".to_string()),
                runtime_risk_severity: Some("critical".to_string()),
                next_suggested_limit: Some(0),
                next_suggested_desired_concurrency: Some(0),
                runtime_risk_message: Some(
                    "rate limit policy pauses this failure reason".to_string(),
                ),
                runtime_risk_cooldown_seconds: Some(120),
            },
        );

        let response = run_identity_job(IdentityJobRunOptions {
            asset_manifest: manifest_path.clone(),
            policy: None,
            job_preset: None,
            desired_concurrency: None,
            limit: Some(1),
            worker: Some("worker-a".to_string()),
            job: Some("publish".to_string()),
            lease_seconds: Some(300),
            max_wait_seconds: Some(0),
            allow_wait: false,
            per_asset: true,
            child_concurrency: Some(1),
            runtime_renew_interval_seconds: None,
            child_timeout_seconds: None,
            child_result_dir: Some(result_dir.clone()),
            max_failed_assets: None,
            max_failed_assets_per_reason: None,
            allow_states: Vec::new(),
            include_dispatch_leased: false,
            include_retry: false,
            include_failed: false,
            include_cancelled: false,
            include_runtime_leased: false,
            include_missing_profile_dir: false,
            skip_sweep: false,
            skip_validate: false,
            runtime_grace_seconds: Some(0),
            dispatch_grace_seconds: Some(0),
            cooldown_grace_seconds: Some(0),
            failure_cooldown_seconds: None,
            failure_next_state: None,
            failure_reason_rules,
            asset_manifest_out: Some(working_manifest_path.clone()),
            sweep_out: None,
            validate_out: None,
            gate_out: None,
            selection_out: None,
            release_out: None,
            append_release: false,
            runtime_risk_ledgers: Vec::new(),
            runtime_risk_window_seconds: None,
            runtime_risk_out: None,
            append_runtime_risk: false,
            explain_out: None,
            job_out: None,
            command,
        })
        .await
        .unwrap();
        let value = response.into_value();
        let manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&working_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["phase"], "complete");
        assert_eq!(value["data"]["passed"], false);
        assert_eq!(
            value["data"]["child"]["failureReasonCounts"]["rate_limited"],
            1
        );
        assert_eq!(
            value["data"]["runtimeRisk"]["dominantFailureReason"],
            "rate_limited"
        );
        assert_eq!(
            value["data"]["runtimeRisk"]["recommendedAction"],
            "pause_failure_reason"
        );
        assert_eq!(value["data"]["runtimeRisk"]["severity"], "critical");
        assert_eq!(value["data"]["runtimeRisk"]["nextSuggestedLimit"], 0);
        assert_eq!(
            value["data"]["runtimeRisk"]["policyFailureReason"],
            "rate_limited"
        );
        assert_eq!(
            value["data"]["runtimeRisk"]["runtimeRiskCooldownSeconds"],
            120
        );
        assert!(
            value["data"]["runtimeRisk"]["suppressUntilUnixSeconds"]
                .as_u64()
                .unwrap_or(0)
                > value["data"]["generatedAtUnixSeconds"]
                    .as_u64()
                    .unwrap_or(0)
        );
        assert_eq!(
            value["data"]["runtimeRisk"]["failureReasonRuleAppliedToRuntimeRisk"],
            true
        );
        assert_eq!(manifest["profileAssets"][0]["lastRuntimeStatus"], "failed");
        assert_eq!(manifest["profileAssets"][0]["state"], "repair");
        assert!(manifest["profileAssets"][0]["cooldownUntilUnixSeconds"].is_number());
        assert_eq!(
            manifest["profileAssets"][0]["lastRuntimeResult"]["failureReason"],
            "rate_limited"
        );
        assert_eq!(
            manifest["profileAssets"][0]["lastRuntimeResult"]["failureReasonRuleApplied"],
            true
        );
        assert_eq!(
            manifest["profileAssets"][0]["lastRuntimeResult"]["releaseOverride"]["reason"],
            "rate_limited"
        );
        assert!(result_dir.join("child-0.json").exists());

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn identity_job_run_child_result_file_overrides_release_decision() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-job-run-child-result-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let profile_a = root.join("profile-a");
        let profile_b = root.join("profile-b");
        let manifest_path = root.join("profile-assets.json");
        let working_manifest_path = root.join("working-profile-assets.json");
        let result_dir = root.join("child-results");
        tokio::fs::create_dir_all(&profile_a).await.unwrap();
        tokio::fs::create_dir_all(&profile_b).await.unwrap();
        tokio::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&json!({
                "profileAssets": [{
                    "accountId": "acct-a",
                    "profileId": "profile-a",
                    "identityId": "fp-a",
                    "label": "acct-a",
                    "profileDir": profile_a.display().to_string(),
                    "state": "active"
                },{
                    "accountId": "acct-b",
                    "profileId": "profile-b",
                    "identityId": "fp-b",
                    "label": "acct-b",
                    "profileDir": profile_b.display().to_string(),
                    "state": "active"
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            r#"if [ "$DRS_IDENTITY_LABEL" = acct-b ]; then
printf '%s' '{"status":"failed","cooldownSeconds":60,"nextState":"repair","message":"rate limited","result":{"reason":"rate_limited"}}' > "$DRS_IDENTITY_RESULT_OUT"
else
printf '%s' '{"status":"succeeded","message":"published","result":{"published":true}}' > "$DRS_IDENTITY_RESULT_OUT"
fi"#
            .to_string(),
        ];

        let response = run_identity_job(IdentityJobRunOptions {
            asset_manifest: manifest_path.clone(),
            policy: None,
            job_preset: None,
            desired_concurrency: None,
            limit: Some(2),
            worker: Some("worker-a".to_string()),
            job: Some("publish".to_string()),
            lease_seconds: Some(300),
            max_wait_seconds: Some(0),
            allow_wait: false,
            per_asset: true,
            child_concurrency: Some(2),
            runtime_renew_interval_seconds: None,
            child_timeout_seconds: None,
            child_result_dir: Some(result_dir.clone()),
            max_failed_assets: None,
            max_failed_assets_per_reason: None,
            allow_states: Vec::new(),
            include_dispatch_leased: false,
            include_retry: false,
            include_failed: false,
            include_cancelled: false,
            include_runtime_leased: false,
            include_missing_profile_dir: false,
            skip_sweep: false,
            skip_validate: false,
            runtime_grace_seconds: Some(0),
            dispatch_grace_seconds: Some(0),
            cooldown_grace_seconds: Some(0),
            failure_cooldown_seconds: None,
            failure_next_state: None,
            failure_reason_rules: BTreeMap::new(),
            asset_manifest_out: Some(working_manifest_path.clone()),
            sweep_out: None,
            validate_out: None,
            gate_out: None,
            selection_out: None,
            release_out: None,
            append_release: false,
            runtime_risk_ledgers: Vec::new(),
            runtime_risk_window_seconds: None,
            runtime_risk_out: None,
            append_runtime_risk: false,
            explain_out: None,
            job_out: None,
            command,
        })
        .await
        .unwrap();
        let value = response.into_value();
        let manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&working_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["phase"], "complete");
        assert_eq!(value["data"]["passed"], false);
        assert_eq!(value["data"]["exitCode"], 1);
        assert_eq!(value["data"]["child"]["failedCount"], 1);
        assert_eq!(
            value["data"]["child"]["children"][0]["child"]["releaseOverride"]["status"],
            "succeeded"
        );
        assert_eq!(
            value["data"]["child"]["children"][1]["child"]["releaseOverride"]["status"],
            "failed"
        );
        assert_eq!(
            value["data"]["child"]["children"][1]["child"]["releaseOverride"]["result"]["reason"],
            "rate_limited"
        );
        assert_eq!(
            manifest["profileAssets"][0]["lastRuntimeStatus"],
            "succeeded"
        );
        assert_eq!(
            manifest["profileAssets"][0]["lastRuntimeMessage"],
            "published"
        );
        assert_eq!(manifest["profileAssets"][1]["lastRuntimeStatus"], "failed");
        assert_eq!(
            manifest["profileAssets"][1]["lastRuntimeMessage"],
            "rate limited"
        );
        assert_eq!(manifest["profileAssets"][1]["state"], "repair");
        assert!(manifest["profileAssets"][1]["cooldownUntilUnixSeconds"].is_number());
        assert_eq!(
            manifest["profileAssets"][1]["lastRuntimeResult"]["releaseOverride"]["result"]["reason"],
            "rate_limited"
        );
        assert!(result_dir.join("child-0.json").exists());
        assert!(result_dir.join("child-1.json").exists());

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_job_run_renews_runtime_leases_while_child_runs() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-job-run-renew-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let profile = root.join("profile-a");
        let manifest_path = root.join("profile-assets.json");
        let working_manifest_path = root.join("working-profile-assets.json");
        tokio::fs::create_dir_all(&profile).await.unwrap();
        tokio::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&json!({
                "profileAssets": [{
                    "accountId": "acct-a",
                    "profileId": "profile-a",
                    "identityId": "fp-a",
                    "label": "acct-a",
                    "profileDir": profile.display().to_string(),
                    "state": "active"
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let command = if cfg!(windows) {
            vec![
                "cmd".to_string(),
                "/C".to_string(),
                "ping -n 3 127.0.0.1 > nul && echo renew=%DRS_IDENTITY_SELECTED_COUNT%".to_string(),
            ]
        } else {
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "sleep 2; printf renew=$DRS_IDENTITY_SELECTED_COUNT".to_string(),
            ]
        };

        let response = run_identity_job(IdentityJobRunOptions {
            asset_manifest: manifest_path.clone(),
            policy: None,
            job_preset: None,
            desired_concurrency: None,
            limit: Some(1),
            worker: Some("worker-a".to_string()),
            job: Some("publish".to_string()),
            lease_seconds: Some(2),
            max_wait_seconds: Some(0),
            allow_wait: false,
            per_asset: false,
            child_concurrency: None,
            runtime_renew_interval_seconds: Some(1),
            child_timeout_seconds: None,
            child_result_dir: None,
            max_failed_assets: None,
            max_failed_assets_per_reason: None,
            allow_states: Vec::new(),
            include_dispatch_leased: false,
            include_retry: false,
            include_failed: false,
            include_cancelled: false,
            include_runtime_leased: false,
            include_missing_profile_dir: false,
            skip_sweep: false,
            skip_validate: false,
            runtime_grace_seconds: Some(0),
            dispatch_grace_seconds: Some(0),
            cooldown_grace_seconds: Some(0),
            failure_cooldown_seconds: None,
            failure_next_state: None,
            failure_reason_rules: BTreeMap::new(),
            asset_manifest_out: Some(working_manifest_path.clone()),
            sweep_out: None,
            validate_out: None,
            gate_out: None,
            selection_out: None,
            release_out: None,
            append_release: false,
            runtime_risk_ledgers: Vec::new(),
            runtime_risk_window_seconds: None,
            runtime_risk_out: None,
            append_runtime_risk: false,
            explain_out: None,
            job_out: None,
            command,
        })
        .await
        .unwrap();
        let value = response.into_value();
        let manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&working_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["phase"], "complete");
        assert_eq!(value["data"]["exitCode"], 0);
        assert_eq!(value["data"]["leaseRenewal"]["enabled"], true);
        assert!(
            value["data"]["leaseRenewal"]["tickCount"]
                .as_u64()
                .unwrap_or(0)
                >= 1
        );
        assert!(
            value["data"]["leaseRenewal"]["renewedCount"]
                .as_u64()
                .unwrap_or(0)
                >= 1
        );
        assert_eq!(
            manifest["profileAssets"][0]["lastRuntimeStatus"],
            "succeeded"
        );
        assert!(
            manifest["profileAssets"][0]["runtimeLeaseRenewalCount"]
                .as_u64()
                .unwrap_or(0)
                >= 1
        );
        assert!(manifest["profileAssets"][0]["runtimeLeaseRenewedAtUnixSeconds"].is_number());

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_job_run_times_out_stuck_child_and_releases_failed() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-job-run-timeout-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let profile = root.join("profile-a");
        let manifest_path = root.join("profile-assets.json");
        let working_manifest_path = root.join("working-profile-assets.json");
        tokio::fs::create_dir_all(&profile).await.unwrap();
        tokio::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&json!({
                "profileAssets": [{
                    "accountId": "acct-a",
                    "profileId": "profile-a",
                    "identityId": "fp-a",
                    "label": "acct-a",
                    "profileDir": profile.display().to_string(),
                    "state": "active"
                }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let command = if cfg!(windows) {
            vec![
                "cmd".to_string(),
                "/C".to_string(),
                "ping -n 4 127.0.0.1 > nul".to_string(),
            ]
        } else {
            vec!["sh".to_string(), "-c".to_string(), "sleep 3".to_string()]
        };

        let response = run_identity_job(IdentityJobRunOptions {
            asset_manifest: manifest_path.clone(),
            policy: None,
            job_preset: None,
            desired_concurrency: None,
            limit: Some(1),
            worker: Some("worker-a".to_string()),
            job: Some("publish".to_string()),
            lease_seconds: Some(10),
            max_wait_seconds: Some(0),
            allow_wait: false,
            per_asset: false,
            child_concurrency: None,
            runtime_renew_interval_seconds: None,
            child_timeout_seconds: Some(1),
            child_result_dir: None,
            max_failed_assets: None,
            max_failed_assets_per_reason: None,
            allow_states: Vec::new(),
            include_dispatch_leased: false,
            include_retry: false,
            include_failed: false,
            include_cancelled: false,
            include_runtime_leased: false,
            include_missing_profile_dir: false,
            skip_sweep: false,
            skip_validate: false,
            runtime_grace_seconds: Some(0),
            dispatch_grace_seconds: Some(0),
            cooldown_grace_seconds: Some(0),
            failure_cooldown_seconds: Some(60),
            failure_next_state: Some("repair".to_string()),
            failure_reason_rules: BTreeMap::new(),
            asset_manifest_out: Some(working_manifest_path.clone()),
            sweep_out: None,
            validate_out: None,
            gate_out: None,
            selection_out: None,
            release_out: None,
            append_release: false,
            runtime_risk_ledgers: Vec::new(),
            runtime_risk_window_seconds: None,
            runtime_risk_out: None,
            append_runtime_risk: false,
            explain_out: None,
            job_out: None,
            command,
        })
        .await
        .unwrap();
        let value = response.into_value();
        let manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&working_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["phase"], "complete");
        assert_eq!(value["data"]["passed"], false);
        assert_eq!(value["data"]["exitCode"], 1);
        assert_eq!(value["data"]["child"]["success"], false);
        assert_eq!(value["data"]["child"]["timedOut"], true);
        assert_eq!(value["data"]["child"]["timeoutSeconds"], 1);
        assert_eq!(value["data"]["release"]["releasedCount"], 1);
        assert_eq!(manifest["profileAssets"][0]["lastRuntimeStatus"], "failed");
        assert_eq!(manifest["profileAssets"][0]["state"], "repair");
        assert!(manifest["profileAssets"][0]["cooldownUntilUnixSeconds"].is_number());
        assert_eq!(
            manifest["profileAssets"][0]["lastRuntimeResult"]["timedOut"],
            true
        );
        assert_eq!(
            manifest["profileAssets"][0]["lastRuntimeResult"]["timeoutSeconds"],
            1
        );

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_assets_reconcile_runtime_replays_release_ledgers() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-assets-reconcile-runtime-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let manifest_path = root.join("runtime-profile-assets.json");
        let ledger_path = root.join("runtime-release.ndjson");
        let reconciled_manifest_path = root.join("reconciled-profile-assets.json");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let now = unix_seconds();
        let future = now + 3_600;
        let cooldown = now + 600;
        tokio::fs::write(
            &manifest_path,
            serde_json::to_string(&json!({
                "profileAssets": [
                    {
                        "accountId": "acct-a",
                        "profileId": "profile-a",
                        "identityId": "fp-a",
                        "label": "acct-a",
                        "profileDir": "/profiles/acct-a",
                        "state": "active",
                        "runtimeLeaseState": "leased",
                        "runtimeLeaseId": "lease-a",
                        "runtimeLeaseWorkerId": "worker-a",
                        "runtimeLeaseJobId": "publish",
                        "runtimeLeaseExpiresUnixSeconds": future
                    },
                    {
                        "accountId": "acct-b",
                        "profileId": "profile-b",
                        "identityId": "fp-b",
                        "label": "acct-b",
                        "profileDir": "/profiles/acct-b",
                        "state": "active",
                        "runtimeLeaseState": "leased",
                        "runtimeLeaseId": "lease-b",
                        "runtimeLeaseWorkerId": "worker-b",
                        "runtimeLeaseJobId": "publish",
                        "runtimeLeaseExpiresUnixSeconds": future,
                        "cooldownUntilUnixSeconds": future
                    },
                    {
                        "accountId": "acct-c",
                        "label": "acct-c",
                        "profileDir": "/profiles/acct-c",
                        "state": "active"
                    }
                ]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        let ledger_lines = [
            json!({
                "scope": "identity_assets_release",
                "assetManifest": manifest_path.display().to_string(),
                "generatedAtUnixSeconds": now.saturating_sub(30),
                "status": "failed",
                "workerId": "worker-a",
                "jobId": "publish",
                "cooldownUntilUnixSeconds": cooldown,
                "nextState": "repair",
                "message": "captcha required",
                "result": {"error": "captcha"},
                "item": {
                    "assetIndex": 0,
                    "status": "failed",
                    "accountId": "acct-a",
                    "profileId": "profile-a",
                    "identityId": "fp-a",
                    "label": "acct-a",
                    "profileDir": "/profiles/acct-a",
                    "leaseId": "lease-a",
                    "workerId": "worker-a",
                    "jobId": "publish"
                }
            }),
            json!({
                "scope": "identity_assets_release",
                "assetManifest": manifest_path.display().to_string(),
                "generatedAtUnixSeconds": now.saturating_sub(20),
                "status": "succeeded",
                "workerId": "worker-b",
                "jobId": "publish",
                "item": {
                    "assetIndex": 1,
                    "status": "succeeded",
                    "accountId": "acct-b",
                    "profileId": "profile-b",
                    "identityId": "fp-b",
                    "label": "acct-b",
                    "profileDir": "/profiles/acct-b",
                    "leaseId": "lease-b",
                    "workerId": "worker-b",
                    "jobId": "publish"
                }
            }),
            json!({
                "scope": "identity_assets_release",
                "assetManifest": manifest_path.display().to_string(),
                "generatedAtUnixSeconds": now.saturating_sub(10),
                "status": "failed",
                "workerId": "worker-z",
                "jobId": "publish",
                "item": {
                    "assetIndex": 99,
                    "status": "failed",
                    "accountId": "acct-z",
                    "label": "acct-z",
                    "profileDir": "/profiles/acct-z",
                    "leaseId": "lease-z",
                    "workerId": "worker-z",
                    "jobId": "publish"
                }
            }),
        ]
        .into_iter()
        .map(|line| serde_json::to_string(&line).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
        tokio::fs::write(&ledger_path, format!("{ledger_lines}\n"))
            .await
            .unwrap();

        let response = reconcile_identity_asset_runtime_manifest(
            &manifest_path,
            std::slice::from_ref(&ledger_path),
            Some(&reconciled_manifest_path),
        )
        .await
        .unwrap();
        let value = response.into_value();
        let reconciled_manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&reconciled_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();

        assert_eq!(value["data"]["scope"], "identity_assets_reconcile_runtime");
        assert_eq!(value["data"]["assetCount"], 3);
        assert_eq!(value["data"]["releaseEventCount"], 3);
        assert_eq!(value["data"]["updatedAssetCount"], 2);
        assert_eq!(value["data"]["unmatchedEventCount"], 1);
        assert_eq!(value["data"]["runtimeLeaseStateCounts"]["released"], 2);
        assert_eq!(value["data"]["runtimeLeaseStateCounts"]["none"], 1);
        assert_eq!(value["data"]["updates"][0]["label"], "acct-a");
        assert_eq!(value["data"]["updates"][1]["label"], "acct-b");
        assert_eq!(
            value["data"]["assetManifestOut"]["format"],
            "profile_assets_json"
        );
        assert_eq!(value["data"]["assetManifestOut"]["updatedCount"], 2);
        assert_eq!(value["data"]["assetManifestOut"]["unmatchedEventCount"], 1);
        assert_eq!(
            reconciled_manifest["profileAssets"][0]["runtimeLeaseState"],
            "released"
        );
        assert_eq!(
            reconciled_manifest["profileAssets"][0]["lastRuntimeStatus"],
            "failed"
        );
        assert_eq!(
            reconciled_manifest["profileAssets"][0]["lastRuntimeWorkerId"],
            "worker-a"
        );
        assert_eq!(
            reconciled_manifest["profileAssets"][0]["lastRuntimeLeaseId"],
            "lease-a"
        );
        assert_eq!(
            reconciled_manifest["profileAssets"][0]["lastRuntimeMessage"],
            "captcha required"
        );
        assert_eq!(
            reconciled_manifest["profileAssets"][0]["lastRuntimeResult"]["error"],
            "captcha"
        );
        assert_eq!(
            reconciled_manifest["profileAssets"][0]["state"],
            Value::String("repair".to_string())
        );
        assert_eq!(
            reconciled_manifest["profileAssets"][0]["cooldownUntilUnixSeconds"],
            json!(cooldown)
        );
        assert_eq!(
            reconciled_manifest["profileAssets"][0]["runtimeLeaseId"],
            Value::Null
        );
        assert_eq!(
            reconciled_manifest["profileAssets"][0]["runtimeLeaseWorkerId"],
            Value::Null
        );
        assert_eq!(
            reconciled_manifest["profileAssets"][0]["runtimeLeaseJobId"],
            Value::Null
        );
        assert_eq!(
            reconciled_manifest["profileAssets"][0]["runtimeLeaseExpiresUnixSeconds"],
            Value::Null
        );
        assert_eq!(
            reconciled_manifest["profileAssets"][1]["runtimeLeaseState"],
            "released"
        );
        assert_eq!(
            reconciled_manifest["profileAssets"][1]["lastRuntimeStatus"],
            "succeeded"
        );
        assert_eq!(
            reconciled_manifest["profileAssets"][1]["cooldownUntilUnixSeconds"],
            Value::Null
        );

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_assets_health_marks_repair_and_quarantine_from_release_history() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-assets-health-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let manifest_path = root.join("runtime-profile-assets.json");
        let ledger_path = root.join("runtime-release.ndjson");
        let health_manifest_path = root.join("health-profile-assets.json");
        let health_path = root.join("asset-health.json");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let now = unix_seconds();
        tokio::fs::write(
            &manifest_path,
            serde_json::to_string(&json!({
                "profileAssets": [
                    {
                        "accountId": "acct-a",
                        "profileId": "profile-a",
                        "identityId": "fp-a",
                        "label": "acct-a",
                        "profileDir": "/profiles/acct-a",
                        "state": "active"
                    },
                    {
                        "accountId": "acct-b",
                        "profileId": "profile-b",
                        "identityId": "fp-b",
                        "label": "acct-b",
                        "profileDir": "/profiles/acct-b",
                        "state": "active"
                    },
                    {
                        "accountId": "acct-c",
                        "profileId": "profile-c",
                        "identityId": "fp-c",
                        "label": "acct-c",
                        "profileDir": "/profiles/acct-c",
                        "state": "active"
                    }
                ]
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let mut ledger_lines = Vec::new();
        let mut push_release = |offset: u64, status: &str, account: &str, label: &str| {
            ledger_lines.push(
                serde_json::to_string(&json!({
                    "scope": "identity_assets_release",
                    "assetManifest": manifest_path.display().to_string(),
                    "generatedAtUnixSeconds": now.saturating_sub(offset),
                    "status": status,
                    "workerId": "worker-a",
                    "jobId": "publish",
                    "message": format!("{label} {status}"),
                    "result": {"status": status},
                    "item": {
                        "assetIndex": 0,
                        "status": status,
                        "accountId": account,
                        "label": label,
                        "profileDir": format!("/profiles/{label}"),
                        "leaseId": format!("lease-{label}-{offset}"),
                        "workerId": "worker-a",
                        "jobId": "publish"
                    }
                }))
                .unwrap(),
            );
        };
        push_release(80, "failed", "acct-a", "acct-a");
        push_release(70, "failed", "acct-a", "acct-a");
        push_release(60, "failed", "acct-b", "acct-b");
        push_release(50, "failed", "acct-b", "acct-b");
        push_release(40, "failed", "acct-b", "acct-b");
        push_release(30, "failed", "acct-b", "acct-b");
        push_release(20, "succeeded", "acct-c", "acct-c");
        push_release(10, "failed", "acct-z", "acct-z");
        tokio::fs::write(&ledger_path, format!("{}\n", ledger_lines.join("\n")))
            .await
            .unwrap();

        let response = health_identity_assets(
            &manifest_path,
            std::slice::from_ref(&ledger_path),
            None,
            2,
            4,
            1_800,
            Some(&health_manifest_path),
            Some(&health_path),
        )
        .await
        .unwrap();
        let value = response.into_value();
        let health_manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&health_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();
        let health_report: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&health_path).await.unwrap()).unwrap();

        assert_eq!(value["data"]["scope"], "identity_assets_health");
        assert_eq!(value["data"]["assetCount"], 3);
        assert_eq!(value["data"]["releaseEventCount"], 8);
        assert_eq!(value["data"]["matchedEventCount"], 7);
        assert_eq!(value["data"]["unmatchedEventCount"], 1);
        assert_eq!(value["data"]["updatedAssetCount"], 2);
        assert_eq!(value["data"]["degradedCount"], 1);
        assert_eq!(value["data"]["quarantineCount"], 1);
        assert_eq!(value["data"]["healthyCount"], 1);
        assert_eq!(value["data"]["actionCounts"]["mark_repair"], 1);
        assert_eq!(value["data"]["actionCounts"]["mark_quarantine"], 1);
        assert_eq!(value["data"]["actionCounts"]["keep_active"], 1);
        assert_eq!(
            value["data"]["assets"][0]["recommendedAction"],
            "mark_repair"
        );
        assert_eq!(
            value["data"]["assets"][0]["consecutiveUnsuccessfulCount"],
            2
        );
        assert_eq!(
            value["data"]["assets"][1]["recommendedAction"],
            "mark_quarantine"
        );
        assert_eq!(
            value["data"]["assets"][2]["recommendedAction"],
            "keep_active"
        );
        assert_eq!(
            value["data"]["assetManifestOut"]["format"],
            "profile_assets_json"
        );
        assert_eq!(value["data"]["healthOut"]["format"], "json_report");
        assert_eq!(health_report["scope"], "identity_assets_health");
        assert_eq!(
            health_manifest["profileAssets"][0]["state"],
            Value::String("repair".to_string())
        );
        assert_eq!(
            health_manifest["profileAssets"][0]["lastRuntimeHealthAction"],
            "mark_repair"
        );
        assert_eq!(
            health_manifest["profileAssets"][0]["runtimeConsecutiveUnsuccessfulCount"],
            2
        );
        assert!(health_manifest["profileAssets"][0]["cooldownUntilUnixSeconds"].is_number());
        assert_eq!(
            health_manifest["profileAssets"][1]["state"],
            Value::String("quarantine".to_string())
        );
        assert_eq!(
            health_manifest["profileAssets"][1]["lastRuntimeHealthAction"],
            "mark_quarantine"
        );
        assert_eq!(
            health_manifest["profileAssets"][2]["state"],
            Value::String("active".to_string())
        );
        assert_eq!(
            health_manifest["profileAssets"][2]["lastRuntimeHealthAction"],
            Value::Null
        );

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn identity_assets_sweep_expires_stale_runtime_and_dispatch_state() {
        let root = std::env::temp_dir().join(format!(
            "drs-identity-assets-sweep-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let manifest_path = root.join("runtime-profile-assets.json");
        let swept_manifest_path = root.join("swept-profile-assets.json");
        let sweep_path = root.join("sweep.json");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let now = unix_seconds();
        let expired = now.saturating_sub(60);
        let future = now + 3_600;
        tokio::fs::write(
            &manifest_path,
            serde_json::to_string(&json!({
                "profileAssets": [
                    {
                        "accountId": "acct-a",
                        "label": "acct-a",
                        "profileDir": "/profiles/acct-a",
                        "state": "active",
                        "runtimeLeaseState": "leased",
                        "runtimeLeaseId": "lease-a",
                        "runtimeLeaseWorkerId": "worker-old",
                        "runtimeLeaseJobId": "publish",
                        "runtimeLeaseExpiresUnixSeconds": expired,
                        "dispatchState": "leased",
                        "lastDispatchLeaseExpiresUnixSeconds": expired,
                        "cooldownUntilUnixSeconds": expired
                    },
                    {
                        "accountId": "acct-b",
                        "label": "acct-b",
                        "profileDir": "/profiles/acct-b",
                        "state": "active",
                        "runtimeLeaseState": "leased",
                        "runtimeLeaseId": "lease-b",
                        "runtimeLeaseWorkerId": "worker-new",
                        "runtimeLeaseJobId": "publish",
                        "runtimeLeaseExpiresUnixSeconds": future,
                        "dispatchState": "leased",
                        "lastDispatchLeaseExpiresUnixSeconds": future,
                        "cooldownUntilUnixSeconds": future
                    }
                ]
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let response = sweep_identity_assets(
            &manifest_path,
            0,
            0,
            0,
            Some(&swept_manifest_path),
            Some(&sweep_path),
        )
        .await
        .unwrap();
        let value = response.into_value();
        let swept_manifest: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&swept_manifest_path)
                .await
                .unwrap(),
        )
        .unwrap();
        let sweep_report: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&sweep_path).await.unwrap()).unwrap();

        assert_eq!(value["data"]["scope"], "identity_assets_sweep");
        assert_eq!(value["data"]["updatedAssetCount"], 1);
        assert_eq!(value["data"]["expiredRuntimeLeaseCount"], 1);
        assert_eq!(value["data"]["expiredDispatchLeaseCount"], 1);
        assert_eq!(value["data"]["clearedCooldownCount"], 1);
        assert_eq!(value["data"]["actions"].as_array().unwrap().len(), 3);
        assert_eq!(
            swept_manifest["profileAssets"][0]["runtimeLeaseState"],
            "expired"
        );
        assert_eq!(
            swept_manifest["profileAssets"][0]["lastRuntimeLeaseId"],
            "lease-a"
        );
        assert_eq!(
            swept_manifest["profileAssets"][0]["dispatchState"],
            "expired"
        );
        assert_eq!(
            swept_manifest["profileAssets"][0]["cooldownUntilUnixSeconds"],
            Value::Null
        );
        assert_eq!(
            swept_manifest["profileAssets"][1]["runtimeLeaseState"],
            "leased"
        );
        assert_eq!(
            swept_manifest["profileAssets"][1]["dispatchState"],
            "leased"
        );
        assert!(swept_manifest["profileAssets"][1]["cooldownUntilUnixSeconds"].is_number());
        assert_eq!(sweep_report["scope"], "identity_assets_sweep");

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn analyze_pool_reads_file_and_applies_gate() {
        let path = std::env::temp_dir().join(format!(
            "drs-identity-pool-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        tokio::fs::write(
            &path,
            r#"[{
                "ua":"Mozilla/5.0",
                "platform":"Win32",
                "languages":"en-US,en",
                "hardware_concurrency":8,
                "device_memory":8,
                "screen":"1920x1080",
                "device_pixel_ratio":1,
                "timezone":"UTC",
                "webgl_renderer":"ANGLE (NVIDIA)",
                "canvas_hash":"same"
            },{
                "ua":"Mozilla/5.0",
                "platform":"Win32",
                "languages":"en-US,en",
                "hardware_concurrency":8,
                "device_memory":8,
                "screen":"1920x1080",
                "device_pixel_ratio":1,
                "timezone":"UTC",
                "webgl_renderer":"ANGLE (NVIDIA)",
                "canvas_hash":"same"
            }]"#,
        )
        .await
        .unwrap();

        let response = analyze_pool(
            &path,
            None,
            IdentityGate {
                max_linkability: Some(10),
                ..IdentityGate::default()
            },
            None,
            None,
            None,
            None,
            None,
            false,
            false,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["gate"]["passed"], false);
        assert_eq!(value["data"]["count"], 2);
        assert_eq!(value["data"]["clusters"][0]["indexes"][0], 0);
        assert_eq!(value["data"]["clusters"][0]["indexes"][1], 1);
        assert_eq!(value["data"]["offenders"][0]["index"], 0);
        assert_eq!(value["data"]["offenders"][0]["linkedIndexes"][0], 1);
        assert_eq!(value["data"]["quarantine"]["indexes"][0], 0);
        assert_eq!(value["data"]["quarantine"]["coveredPairCount"], 1);
        assert_eq!(value["data"]["diversity"]["size"], 2);
        assert!(
            value["data"]["diversity"]["concentratedSignalCount"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert_eq!(
            value["data"]["diversity"]["signals"][0]["maxBucketCount"],
            2
        );
        assert_eq!(
            value["data"]["report"]["diversity"]["size"],
            value["data"]["diversity"]["size"]
        );
        assert_eq!(value["data"]["remediation"]["quarantineIndexes"][0], 0);
        assert!(
            value["data"]["remediation"]["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action["code"] == "pool.quarantine_offenders")
        );
        assert_eq!(
            value["data"]["report"]["remediationPlan"]["quarantineIndexes"][0],
            0
        );
        assert_eq!(value["data"]["admission"]["action"], "partial_quarantine");
        assert_eq!(value["data"]["admission"]["acceptIndexes"][0], 1);
        assert_eq!(value["data"]["admission"]["quarantineIndexes"][0], 0);
        assert_eq!(value["data"]["ledger"]["candidateCount"], 2);
        assert_eq!(value["data"]["ledger"]["acceptedCount"], 1);
        assert_eq!(value["data"]["ledger"]["quarantineCount"], 1);
        assert_eq!(value["data"]["ledger"]["duplicateCandidateCount"], 2);
        assert_eq!(value["data"]["ledger"]["riskyInternalCount"], 2);
        assert_eq!(
            value["data"]["ledger"]["entries"][0]["decision"],
            "quarantine"
        );
        assert_eq!(
            value["data"]["ledger"]["entries"][0]["duplicateInBatch"],
            true
        );
        assert_eq!(
            value["data"]["ledger"]["entries"][0]["internalLinkedIndexes"][0],
            1
        );
        assert_eq!(
            value["data"]["ledger"]["entries"][0]["identityId"],
            value["data"]["admission"]["quarantineIds"][0]
        );
        assert!(
            value["data"]["admission"]["acceptIds"][0]
                .as_str()
                .unwrap()
                .starts_with("fp_")
        );
        assert!(
            value["data"]["admission"]["quarantineIds"][0]
                .as_str()
                .unwrap()
                .starts_with("fp_")
        );
        assert!(
            value["data"]["actionQueue"]["actionCount"]
                .as_u64()
                .unwrap()
                >= 2
        );
        assert_eq!(value["data"]["actionQueue"]["quarantineCount"], 1);
        assert!(
            value["data"]["actionQueue"]["capacityActionCount"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert!(
            value["data"]["actionQueue"]["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action["actionCode"] == "pool.quarantine_duplicate_candidate")
        );
        assert!(
            value["data"]["actionQueue"]["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action["actionCode"] == "pool.quarantine_offenders")
        );
        assert!(
            value["data"]["actionQueue"]["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action["source"] == "capacity"
                    && action["actionCode"] == "capacity.disperse_canvas_seed"
                    && action["estimatedGain"].as_f64().unwrap_or(0.0) > 0.0)
        );

        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn analyze_pool_applies_diversity_gate() {
        let path = std::env::temp_dir().join(format!(
            "drs-identity-pool-diversity-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        tokio::fs::write(
            &path,
            r#"[{
                "ua":"Mozilla/5.0",
                "platform":"Win32",
                "languages":"en-US,en",
                "hardware_concurrency":8,
                "device_memory":8,
                "screen":"1920x1080",
                "device_pixel_ratio":1,
                "timezone":"UTC",
                "webgl_renderer":"ANGLE (NVIDIA)",
                "canvas_hash":"same"
            },{
                "ua":"Mozilla/5.0",
                "platform":"Win32",
                "languages":"en-US,en",
                "hardware_concurrency":8,
                "device_memory":8,
                "screen":"1920x1080",
                "device_pixel_ratio":1,
                "timezone":"UTC",
                "webgl_renderer":"ANGLE (NVIDIA)",
                "canvas_hash":"same"
            }]"#,
        )
        .await
        .unwrap();

        let response = analyze_pool(
            &path,
            None,
            IdentityGate {
                max_concentration_ratio: Some(0.8),
                max_concentrated_signals: Some(3),
                ..IdentityGate::default()
            },
            None,
            None,
            None,
            None,
            None,
            false,
            false,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let failures = value["data"]["gate"]["failures"].as_array().unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["gate"]["passed"], false);
        assert!(failures.iter().any(|failure| {
            failure
                .as_str()
                .unwrap()
                .starts_with("pool_concentration_ratio_above_max")
        }));
        assert!(failures.iter().any(|failure| {
            failure
                .as_str()
                .unwrap()
                .starts_with("pool_concentrated_signals_above_max")
        }));

        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn analyze_drift_reports_high_risk_changes_and_gate_failure() {
        let before_path = std::env::temp_dir().join(format!(
            "drs-identity-drift-before-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let after_path = std::env::temp_dir().join(format!(
            "drs-identity-drift-after-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        tokio::fs::write(
            &before_path,
            r#"[{
                "ua":"Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/149.0.0.0",
                "platform":"Win32",
                "ua_data_platform":"Windows",
                "ua_data_mobile":false,
                "webdriver":false,
                "languages":"en-US,en",
                "hardware_concurrency":8,
                "device_memory":8,
                "screen":"1920x1080",
                "device_pixel_ratio":1,
                "timezone":"America/New_York",
                "webgl_renderer":"ANGLE (NVIDIA)",
                "canvas_hash":"a1b2c3d4"
            }]"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            &after_path,
            r#"[{
                "ua":"Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/149.0.0.0",
                "platform":"Win32",
                "ua_data_platform":"Windows",
                "ua_data_mobile":false,
                "webdriver":false,
                "languages":"en-US,en",
                "hardware_concurrency":8,
                "device_memory":8,
                "screen":"1920x1080",
                "device_pixel_ratio":1,
                "timezone":"Asia/Tokyo",
                "webgl_renderer":"ANGLE (Apple, Apple M2)",
                "canvas_hash":"ffffffff"
            }]"#,
        )
        .await
        .unwrap();

        let response = analyze_drift(
            &before_path,
            &after_path,
            Some(20),
            true,
            IdentityDriftMatchMode::Auto,
            None,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let failures = value["data"]["gate"]["failures"].as_array().unwrap();
        let signals = value["data"]["entries"][0]["signals"].as_array().unwrap();
        let remediation_actions = value["data"]["entries"][0]["remediation"]["actions"]
            .as_array()
            .unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["scope"], "identity_drift");
        assert_eq!(value["data"]["pairCount"], 1);
        assert_eq!(value["data"]["changedCount"], 1);
        assert_eq!(value["data"]["highRiskCount"], 1);
        assert_eq!(value["data"]["actionQueue"]["actionCount"], 4);
        assert_eq!(
            value["data"]["actionQueue"]["actions"][0]["actionCode"],
            "drift.quarantine_current"
        );
        assert_eq!(value["data"]["gate"]["passed"], false);
        assert!(failures.iter().any(|failure| {
            failure
                .as_str()
                .unwrap()
                .starts_with("identity_drift_score_above_max")
        }));
        assert!(failures.iter().any(|failure| {
            failure
                .as_str()
                .unwrap()
                .starts_with("high_risk_identity_drift")
        }));
        assert!(
            signals
                .iter()
                .any(|signal| signal["code"] == "webgl.changed")
        );
        assert!(
            signals
                .iter()
                .any(|signal| signal["code"] == "canvas.changed")
        );
        assert!(
            remediation_actions
                .iter()
                .any(|action| action["code"] == "drift.quarantine_current")
        );
        assert!(
            remediation_actions
                .iter()
                .any(|action| action["code"] == "drift.restore_canvas_seed")
        );

        let _ = tokio::fs::remove_file(before_path).await;
        let _ = tokio::fs::remove_file(after_path).await;
    }

    #[tokio::test]
    async fn analyze_drift_matches_labeled_snapshots_out_of_order() {
        let before_path = std::env::temp_dir().join(format!(
            "drs-identity-drift-labeled-before-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let after_path = std::env::temp_dir().join(format!(
            "drs-identity-drift-labeled-after-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let actions_path = before_path.with_extension("actions.json");
        tokio::fs::write(
            &before_path,
            r#"[
                {"accountId":"acct-a","snapshot":{
                    "ua":"Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/149.0.0.0",
                    "platform":"Win32",
                    "ua_data_platform":"Windows",
                    "ua_data_mobile":false,
                    "webdriver":false,
                    "languages":"en-US,en",
                    "hardware_concurrency":8,
                    "device_memory":8,
                    "screen":"1920x1080",
                    "device_pixel_ratio":1,
                    "timezone":"America/New_York",
                    "webgl_renderer":"ANGLE (NVIDIA)",
                    "canvas_hash":"a1b2c3d4"
                }},
                {"accountId":"acct-b","snapshot":{
                    "ua":"Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) Safari/605.1.15",
                    "platform":"MacIntel",
                    "languages":"ja-JP,ja",
                    "hardware_concurrency":8,
                    "device_memory":4,
                    "screen":"2560x1440",
                    "device_pixel_ratio":2,
                    "timezone":"Asia/Tokyo",
                    "webgl_renderer":"ANGLE (Apple, Apple M2)",
                    "canvas_hash":"ffeeddcc"
                }}
            ]"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            &after_path,
            r#"[
                {"accountId":"acct-b","snapshot":{
                    "ua":"Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) Safari/605.1.15",
                    "platform":"MacIntel",
                    "languages":"ja-JP,ja",
                    "hardware_concurrency":8,
                    "device_memory":4,
                    "screen":"2560x1440",
                    "device_pixel_ratio":2,
                    "timezone":"Asia/Tokyo",
                    "webgl_renderer":"ANGLE (Apple, Apple M2)",
                    "canvas_hash":"ffeeddcc"
                }},
                {"accountId":"acct-a","snapshot":{
                    "ua":"Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/149.0.0.0",
                    "platform":"Win32",
                    "ua_data_platform":"Windows",
                    "ua_data_mobile":false,
                    "webdriver":false,
                    "languages":"en-US,en",
                    "hardware_concurrency":8,
                    "device_memory":8,
                    "screen":"1920x1080",
                    "device_pixel_ratio":1,
                    "timezone":"Asia/Tokyo",
                    "webgl_renderer":"ANGLE (Apple, Apple M2)",
                    "canvas_hash":"ffffffff"
                }}
            ]"#,
        )
        .await
        .unwrap();

        let response = analyze_drift(
            &before_path,
            &after_path,
            Some(20),
            true,
            IdentityDriftMatchMode::Auto,
            Some(&actions_path),
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let actions: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&actions_path).await.unwrap()).unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["requestedMatchBy"], "auto");
        assert_eq!(value["data"]["matchBy"], "label");
        assert_eq!(value["data"]["pairCount"], 2);
        assert_eq!(value["data"]["entries"][0]["label"], "acct-a");
        assert_eq!(value["data"]["entries"][0]["beforeIndex"], 0);
        assert_eq!(value["data"]["entries"][0]["afterIndex"], 1);
        assert_eq!(value["data"]["entries"][0]["highRisk"], true);
        assert_eq!(value["data"]["actionsOut"]["format"], "json_report");
        assert_eq!(
            value["data"]["actionsOut"]["count"],
            value["data"]["actionQueue"]["actionCount"]
        );
        assert!(
            value["data"]["entries"][0]["remediation"]["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action["code"] == "drift.restore_webgl_renderer")
        );
        assert_eq!(value["data"]["entries"][1]["label"], "acct-b");
        assert_eq!(value["data"]["entries"][1]["beforeIndex"], 1);
        assert_eq!(value["data"]["entries"][1]["afterIndex"], 0);
        assert_eq!(value["data"]["entries"][1]["stable"], true);
        assert!(
            value["data"]["missingBeforeLabels"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            value["data"]["missingAfterLabels"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(actions["entryCount"], 2);
        assert_eq!(actions["actions"][0]["label"], "acct-a");
        assert_eq!(
            actions["actions"][0]["actionCode"],
            "drift.quarantine_current"
        );
        assert_eq!(actions["actions"][0]["beforeIndex"], 0);
        assert_eq!(actions["actions"][0]["afterIndex"], 1);

        let _ = tokio::fs::remove_file(before_path).await;
        let _ = tokio::fs::remove_file(after_path).await;
        let _ = tokio::fs::remove_file(actions_path).await;
    }

    #[tokio::test]
    async fn analyze_lifecycle_classifies_profile_states_and_writes_outputs() {
        let baseline_path = std::env::temp_dir().join(format!(
            "drs-identity-lifecycle-baseline-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let current_path = std::env::temp_dir().join(format!(
            "drs-identity-lifecycle-current-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let ledger_path = baseline_path.with_extension("ledger.json");
        let delta_path = baseline_path.with_extension("delta.json");
        let journal_path = baseline_path.with_extension("journal.ndjson");
        let actions_path = baseline_path.with_extension("actions.ndjson");
        let next_baseline_path = baseline_path.with_extension("next-baseline.json");
        let state_dir = baseline_path.with_extension("states");

        tokio::fs::write(
            &baseline_path,
            r#"[
                {"accountId":"acct-a","snapshot":{
                    "ua":"Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/149.0.0.0",
                    "platform":"Win32",
                    "ua_data_platform":"Windows",
                    "ua_data_mobile":false,
                    "webdriver":false,
                    "languages":"en-US,en",
                    "hardware_concurrency":8,
                    "device_memory":8,
                    "screen":"1920x1080",
                    "device_pixel_ratio":1,
                    "timezone":"America/New_York",
                    "webgl_renderer":"ANGLE (NVIDIA)",
                    "canvas_hash":"a1b2c3d4"
                }},
                {"accountId":"acct-b","snapshot":{
                    "ua":"Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) Safari/605.1.15",
                    "platform":"MacIntel",
                    "languages":"ja-JP,ja",
                    "hardware_concurrency":8,
                    "device_memory":4,
                    "screen":"2560x1440",
                    "device_pixel_ratio":2,
                    "timezone":"Asia/Tokyo",
                    "webgl_renderer":"ANGLE (Apple, Apple M2)",
                    "canvas_hash":"ffeeddcc"
                }}
            ]"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            &current_path,
            r#"[
                {"accountId":"acct-a","snapshot":{
                    "ua":"Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/149.0.0.0",
                    "platform":"Win32",
                    "ua_data_platform":"Windows",
                    "ua_data_mobile":false,
                    "webdriver":false,
                    "languages":"en-US,en",
                    "hardware_concurrency":8,
                    "device_memory":8,
                    "screen":"1920x1080",
                    "device_pixel_ratio":1,
                    "timezone":"Asia/Tokyo",
                    "webgl_renderer":"ANGLE (Apple, Apple M2)",
                    "canvas_hash":"ffffffff"
                }},
                {"accountId":"acct-c","snapshot":{
                    "ua":"Mozilla/5.0 (X11; Linux x86_64) Chrome/149.0.0.0",
                    "platform":"Linux x86_64",
                    "ua_data_platform":"Linux",
                    "ua_data_mobile":false,
                    "webdriver":false,
                    "languages":"en-GB,en",
                    "hardware_concurrency":4,
                    "device_memory":4,
                    "screen":"1366x768",
                    "device_pixel_ratio":1,
                    "timezone":"Europe/London",
                    "webgl_renderer":"ANGLE (Intel)",
                    "canvas_hash":"ccddeeff"
                }}
            ]"#,
        )
        .await
        .unwrap();

        let response = analyze_lifecycle(
            &baseline_path,
            &current_path,
            Some(20),
            true,
            true,
            true,
            IdentityDriftMatchMode::Auto,
            Some(&ledger_path),
            Some(&delta_path),
            Some(&journal_path),
            Some(&state_dir),
            Some(&actions_path),
            Some(&next_baseline_path),
            IdentityLifecycleBaselinePolicy::Conservative,
            false,
            true,
            true,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let ledger: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&ledger_path).await.unwrap()).unwrap();
        let delta: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&delta_path).await.unwrap()).unwrap();
        let journal_lines = tokio::fs::read_to_string(&journal_path)
            .await
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let quarantine_state_text = tokio::fs::read_to_string(state_dir.join("quarantine.json"))
            .await
            .unwrap();
        let missing_state_text = tokio::fs::read_to_string(state_dir.join("missing_current.json"))
            .await
            .unwrap();
        let quarantine_state = parse_labeled_snapshots(&quarantine_state_text).unwrap();
        let missing_state = parse_labeled_snapshots(&missing_state_text).unwrap();
        let quarantine_snapshots = parse_snapshots(&quarantine_state_text).unwrap();
        let missing_snapshots = parse_snapshots(&missing_state_text).unwrap();
        let action_lines = tokio::fs::read_to_string(&actions_path)
            .await
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let next_baseline_text = tokio::fs::read_to_string(&next_baseline_path)
            .await
            .unwrap();
        let next_labeled = parse_labeled_snapshots(&next_baseline_text).unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["scope"], "identity_lifecycle");
        assert_eq!(value["data"]["matchBy"], "label");
        assert_eq!(value["data"]["summary"]["quarantineCount"], 1);
        assert_eq!(value["data"]["summary"]["missingCurrentCount"], 1);
        assert_eq!(value["data"]["summary"]["newCurrentCount"], 1);
        assert_eq!(value["data"]["gate"]["passed"], false);
        assert!(
            value["data"]["gate"]["failures"]
                .as_array()
                .unwrap()
                .iter()
                .any(|failure| failure
                    .as_str()
                    .unwrap()
                    .starts_with("lifecycle_high_risk_drift"))
        );
        assert!(
            value["data"]["ledger"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["label"] == "acct-a" && entry["state"] == "quarantine")
        );
        assert!(
            value["data"]["ledger"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["label"] == "acct-b" && entry["state"] == "missing_current")
        );
        assert!(
            value["data"]["ledger"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["label"] == "acct-c" && entry["state"] == "new_current")
        );
        assert_eq!(value["data"]["ledgerOut"]["format"], "json_report");
        assert_eq!(value["data"]["deltaOut"]["format"], "json_report");
        assert_eq!(value["data"]["journalOut"]["format"], "ndjson_records");
        assert_eq!(
            value["data"]["stateOut"]["format"],
            "labeled_snapshot_array_by_state"
        );
        assert_eq!(
            value["data"]["stateOut"]["states"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
        assert_eq!(value["data"]["actionsOut"]["format"], "ndjson_actions");
        assert!(
            value["data"]["stateBuckets"]
                .as_array()
                .unwrap()
                .iter()
                .any(|bucket| bucket["state"] == "quarantine" && bucket["count"] == 1)
        );
        assert_eq!(
            value["data"]["nextBaselineOut"]["format"],
            "labeled_snapshot_array"
        );
        assert_eq!(value["data"]["nextBaseline"]["policy"], "conservative");
        assert_eq!(value["data"]["nextBaseline"]["count"], 1);
        assert_eq!(value["data"]["nextBaseline"]["baselineSourceCount"], 1);
        assert_eq!(value["data"]["nextBaseline"]["currentSourceCount"], 0);
        assert_eq!(
            value["data"]["nextBaseline"]["entries"][0]["label"],
            "acct-b"
        );
        assert_eq!(
            value["data"]["nextBaseline"]["entries"][0]["source"],
            "baseline"
        );
        assert_eq!(ledger["entryCount"], 3);
        assert_eq!(next_labeled.len(), 1);
        assert_eq!(next_labeled[0].label.as_deref(), Some("acct-b"));
        assert_eq!(next_labeled[0].snapshot.canvas_hash, "ffeeddcc");
        assert_eq!(value["data"]["delta"]["changeCount"], 4);
        assert_eq!(value["data"]["delta"]["retainedCount"], 1);
        assert_eq!(value["data"]["delta"]["removedCount"], 1);
        assert_eq!(value["data"]["delta"]["currentExcludedCount"], 1);
        assert_eq!(value["data"]["delta"]["newUnadmittedCount"], 1);
        assert_eq!(delta["changeCount"], 4);
        assert_eq!(journal_lines.len(), 1);
        assert_eq!(journal_lines[0]["gatePassed"], false);
        assert_eq!(journal_lines[0]["nextBaselineCount"], 1);
        assert_eq!(journal_lines[0]["deltaChangeCount"], 4);
        assert_eq!(
            journal_lines[0]["artifacts"]["stateOut"]["dir"],
            state_dir.display().to_string()
        );
        assert_eq!(
            journal_lines[0]["artifacts"]["delta"]["path"],
            delta_path.display().to_string()
        );
        assert_eq!(quarantine_state.len(), 1);
        assert_eq!(quarantine_state[0].label.as_deref(), Some("acct-a"));
        assert_eq!(quarantine_state[0].snapshot.canvas_hash, "ffffffff");
        assert_eq!(quarantine_snapshots.len(), 1);
        assert_eq!(quarantine_snapshots[0].canvas_hash, "ffffffff");
        assert_eq!(missing_state.len(), 1);
        assert_eq!(missing_state[0].label.as_deref(), Some("acct-b"));
        assert_eq!(missing_state[0].snapshot.canvas_hash, "ffeeddcc");
        assert_eq!(missing_snapshots.len(), 1);
        assert_eq!(missing_snapshots[0].canvas_hash, "ffeeddcc");
        assert_eq!(
            value["data"]["run"]["runId"],
            value["data"]["journalOut"]["runId"]
        );
        assert!(
            delta["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["label"] == "acct-a" && entry["change"] == "baseline_removed")
        );
        assert!(
            delta["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["label"] == "acct-a" && entry["change"] == "current_excluded")
        );
        assert!(
            delta["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["label"] == "acct-b" && entry["change"] == "baseline_retained")
        );
        assert!(
            delta["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["label"] == "acct-c"
                    && entry["change"] == "new_current_unadmitted")
        );
        assert_eq!(
            action_lines.len() as u64,
            value["data"]["actionQueue"]["actionCount"]
                .as_u64()
                .unwrap()
        );
        assert!(
            action_lines
                .iter()
                .any(|action| action["actionCode"] == "lifecycle.quarantine_profile")
        );
        assert!(
            action_lines
                .iter()
                .any(|action| action["actionCode"] == "lifecycle.investigate_missing_current")
        );
        assert!(
            action_lines
                .iter()
                .any(|action| action["actionCode"] == "lifecycle.review_new_profile")
        );
        assert!(
            action_lines
                .iter()
                .any(|action| action["actionCode"] == "drift.restore_canvas_seed")
        );

        let _ = tokio::fs::remove_file(baseline_path).await;
        let _ = tokio::fs::remove_file(current_path).await;
        let _ = tokio::fs::remove_file(ledger_path).await;
        let _ = tokio::fs::remove_file(delta_path).await;
        let _ = tokio::fs::remove_file(journal_path).await;
        let _ = tokio::fs::remove_file(actions_path).await;
        let _ = tokio::fs::remove_file(next_baseline_path).await;
        let _ = tokio::fs::remove_dir_all(state_dir).await;
    }

    #[tokio::test]
    async fn analyze_pool_compares_candidates_against_baseline() {
        let candidate_path = std::env::temp_dir().join(format!(
            "drs-candidates-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let baseline_path = std::env::temp_dir().join(format!(
            "drs-baseline-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let snapshot = r#"{
            "ua":"Mozilla/5.0",
            "platform":"Win32",
            "languages":"en-US,en",
            "hardware_concurrency":8,
            "device_memory":8,
            "screen":"1920x1080",
            "device_pixel_ratio":1,
            "timezone":"UTC",
            "webgl_renderer":"ANGLE (NVIDIA)",
            "canvas_hash":"11111111"
        }"#;
        tokio::fs::write(&candidate_path, format!("[{snapshot}]"))
            .await
            .unwrap();
        tokio::fs::write(&baseline_path, format!("[{snapshot}]"))
            .await
            .unwrap();

        let response = analyze_pool(
            &candidate_path,
            Some(&baseline_path),
            IdentityGate {
                max_linkability: Some(25),
                fail_on_risky_pairs: true,
                ..IdentityGate::default()
            },
            None,
            None,
            None,
            None,
            None,
            false,
            false,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();

        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["gate"]["passed"], false);
        assert_eq!(value["data"]["againstReport"]["maxLinkability"], 100);
        assert!(
            value["data"]["againstReport"]["candidateIds"][0]
                .as_str()
                .unwrap()
                .starts_with("fp_")
        );
        assert_eq!(
            value["data"]["againstReport"]["candidateIds"][0],
            value["data"]["againstReport"]["baselineIds"][0]
        );
        assert_eq!(
            value["data"]["againstReport"]["clusters"][0]["candidateIndexes"][0],
            0
        );
        assert_eq!(
            value["data"]["againstReport"]["clusters"][0]["candidateIds"][0],
            value["data"]["againstReport"]["candidateIds"][0]
        );
        assert_eq!(
            value["data"]["againstReport"]["clusters"][0]["baselineIndexes"][0],
            0
        );
        assert_eq!(
            value["data"]["againstReport"]["clusters"][0]["baselineIds"][0],
            value["data"]["againstReport"]["baselineIds"][0]
        );
        assert_eq!(
            value["data"]["againstReport"]["candidateOffenders"][0]["index"],
            0
        );
        assert_eq!(
            value["data"]["againstReport"]["candidateOffenders"][0]["identityId"],
            value["data"]["againstReport"]["candidateIds"][0]
        );
        assert_eq!(
            value["data"]["againstReport"]["baselineOffenders"][0]["index"],
            0
        );
        assert_eq!(
            value["data"]["againstReport"]["baselineOffenders"][0]["identityId"],
            value["data"]["againstReport"]["baselineIds"][0]
        );
        assert_eq!(
            value["data"]["againstReport"]["candidateQuarantine"]["candidateIndexes"][0],
            0
        );
        assert_eq!(
            value["data"]["againstReport"]["candidateQuarantine"]["candidateIds"][0],
            value["data"]["againstReport"]["candidateIds"][0]
        );
        assert_eq!(
            value["data"]["againstReport"]["candidateQuarantine"]["coveredPairCount"],
            1
        );
        assert_eq!(value["data"]["admission"]["action"], "reject_all");
        assert_eq!(value["data"]["admission"]["quarantineIndexes"][0], 0);
        assert_eq!(value["data"]["ledger"]["knownBaselineCount"], 1);
        assert_eq!(value["data"]["ledger"]["riskyBaselineCount"], 1);
        assert_eq!(
            value["data"]["ledger"]["entries"][0]["knownInBaseline"],
            true
        );
        assert_eq!(
            value["data"]["ledger"]["entries"][0]["decision"],
            "quarantine"
        );
        assert_eq!(
            value["data"]["ledger"]["entries"][0]["baselineLinkedIds"][0],
            value["data"]["againstReport"]["baselineIds"][0]
        );
        assert_eq!(
            value["data"]["ledger"]["entries"][0]["identityId"],
            value["data"]["admission"]["quarantineIds"][0]
        );
        assert_eq!(
            value["data"]["admission"]["quarantineIds"][0],
            value["data"]["againstReport"]["candidateIds"][0]
        );
        assert_eq!(
            value["data"]["againstReport"]["riskyPairs"][0]["candidateIndex"],
            0
        );
        assert_eq!(
            value["data"]["againstReport"]["riskyPairs"][0]["candidateId"],
            value["data"]["againstReport"]["candidateIds"][0]
        );
        assert_eq!(
            value["data"]["againstReport"]["riskyPairs"][0]["baselineIndex"],
            0
        );
        assert_eq!(
            value["data"]["againstReport"]["riskyPairs"][0]["baselineId"],
            value["data"]["againstReport"]["baselineIds"][0]
        );

        let _ = tokio::fs::remove_file(candidate_path).await;
        let _ = tokio::fs::remove_file(baseline_path).await;
    }

    #[tokio::test]
    async fn analyze_pool_writes_split_outputs_from_admission() {
        let path = std::env::temp_dir().join(format!(
            "drs-split-source-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let accept_path = path.with_extension("accepted.json");
        let quarantine_path = path.with_extension("quarantine.json");
        tokio::fs::write(
            &path,
            r#"[{
                "ua":"Mozilla/5.0",
                "platform":"Win32",
                "languages":"en-US,en",
                "hardware_concurrency":8,
                "device_memory":8,
                "screen":"1920x1080",
                "device_pixel_ratio":1,
                "timezone":"UTC",
                "webgl_renderer":"ANGLE (NVIDIA)",
                "canvas_hash":"same"
            },{
                "ua":"Mozilla/5.0",
                "platform":"Win32",
                "languages":"en-US,en",
                "hardware_concurrency":8,
                "device_memory":8,
                "screen":"1920x1080",
                "device_pixel_ratio":1,
                "timezone":"UTC",
                "webgl_renderer":"ANGLE (NVIDIA)",
                "canvas_hash":"same"
            }]"#,
        )
        .await
        .unwrap();

        let response = analyze_pool(
            &path,
            None,
            IdentityGate {
                max_linkability: Some(10),
                ..IdentityGate::default()
            },
            Some(&accept_path),
            Some(&quarantine_path),
            None,
            None,
            None,
            false,
            false,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let accepted =
            parse_snapshots(&tokio::fs::read_to_string(&accept_path).await.unwrap()).unwrap();
        let quarantined =
            parse_snapshots(&tokio::fs::read_to_string(&quarantine_path).await.unwrap()).unwrap();

        assert_eq!(value["data"]["splitOut"]["accepted"]["count"], 1);
        assert_eq!(value["data"]["splitOut"]["quarantine"]["count"], 1);
        assert_eq!(accepted.len(), 1);
        assert_eq!(quarantined.len(), 1);
        assert_eq!(accepted[0].canvas_hash, "same");

        let _ = tokio::fs::remove_file(path).await;
        let _ = tokio::fs::remove_file(accept_path).await;
        let _ = tokio::fs::remove_file(quarantine_path).await;
    }

    #[tokio::test]
    async fn analyze_pool_writes_ledger_outputs() {
        let path = std::env::temp_dir().join(format!(
            "drs-ledger-source-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let ledger_path = path.with_extension("ledger.json");
        let ledger_ndjson_path = path.with_extension("ledger.ndjson");
        tokio::fs::write(
            &path,
            r#"[{
                "ua":"Mozilla/5.0",
                "platform":"Win32",
                "languages":"en-US,en",
                "hardware_concurrency":8,
                "device_memory":8,
                "screen":"1920x1080",
                "device_pixel_ratio":1,
                "timezone":"UTC",
                "webgl_renderer":"ANGLE (NVIDIA)",
                "canvas_hash":"same"
            },{
                "ua":"Mozilla/5.0",
                "platform":"Win32",
                "languages":"en-US,en",
                "hardware_concurrency":8,
                "device_memory":8,
                "screen":"1920x1080",
                "device_pixel_ratio":1,
                "timezone":"UTC",
                "webgl_renderer":"ANGLE (NVIDIA)",
                "canvas_hash":"same"
            }]"#,
        )
        .await
        .unwrap();

        let response = analyze_pool(
            &path,
            None,
            IdentityGate::default(),
            None,
            None,
            None,
            Some(&ledger_path),
            None,
            false,
            false,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let ledger: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&ledger_path).await.unwrap()).unwrap();

        assert_eq!(value["data"]["ledgerOut"]["format"], "json_report");
        assert_eq!(value["data"]["ledgerOut"]["count"], 2);
        assert_eq!(ledger["candidateCount"], 2);
        assert_eq!(ledger["entries"][0]["decision"], "quarantine");

        let response = analyze_pool(
            &path,
            None,
            IdentityGate::default(),
            None,
            None,
            None,
            Some(&ledger_ndjson_path),
            None,
            false,
            true,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let lines = tokio::fs::read_to_string(&ledger_ndjson_path)
            .await
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(value["data"]["ledgerOut"]["format"], "ndjson_entries");
        assert_eq!(value["data"]["ledgerOut"]["append"], true);
        assert_eq!(lines.len(), 2);
        assert!(lines[0]["identityId"].as_str().unwrap().starts_with("fp_"));

        let _ = tokio::fs::remove_file(path).await;
        let _ = tokio::fs::remove_file(ledger_path).await;
        let _ = tokio::fs::remove_file(ledger_ndjson_path).await;
    }

    #[tokio::test]
    async fn analyze_pool_writes_action_queue_outputs() {
        let path = std::env::temp_dir().join(format!(
            "drs-pool-actions-source-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let actions_path = path.with_extension("actions.json");
        let actions_ndjson_path = path.with_extension("actions.ndjson");
        tokio::fs::write(
            &path,
            r#"[{
                "ua":"Mozilla/5.0",
                "platform":"Win32",
                "languages":"en-US,en",
                "hardware_concurrency":8,
                "device_memory":8,
                "screen":"1920x1080",
                "device_pixel_ratio":1,
                "timezone":"UTC",
                "webgl_renderer":"ANGLE (NVIDIA)",
                "canvas_hash":"same"
            },{
                "ua":"Mozilla/5.0",
                "platform":"Win32",
                "languages":"en-US,en",
                "hardware_concurrency":8,
                "device_memory":8,
                "screen":"1920x1080",
                "device_pixel_ratio":1,
                "timezone":"UTC",
                "webgl_renderer":"ANGLE (NVIDIA)",
                "canvas_hash":"same"
            }]"#,
        )
        .await
        .unwrap();

        let response = analyze_pool(
            &path,
            None,
            IdentityGate::default(),
            None,
            None,
            None,
            None,
            Some(&actions_path),
            false,
            false,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let actions: Value =
            serde_json::from_str(&tokio::fs::read_to_string(&actions_path).await.unwrap()).unwrap();

        assert_eq!(value["data"]["actionsOut"]["format"], "json_report");
        assert_eq!(
            value["data"]["actionsOut"]["count"],
            value["data"]["actionQueue"]["actionCount"]
        );
        assert_eq!(
            actions["actionCount"],
            value["data"]["actionQueue"]["actionCount"]
        );
        assert!(
            actions["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action["actionCode"] == "pool.quarantine_duplicate_candidate")
        );
        assert!(
            actions["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action["source"] == "capacity"
                    && action["actionCode"] == "capacity.disperse_canvas_seed")
        );

        let response = analyze_pool(
            &path,
            None,
            IdentityGate::default(),
            None,
            None,
            None,
            None,
            Some(&actions_ndjson_path),
            false,
            false,
            true,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let lines = tokio::fs::read_to_string(&actions_ndjson_path)
            .await
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(value["data"]["actionsOut"]["format"], "ndjson_actions");
        assert_eq!(value["data"]["actionsOut"]["append"], true);
        assert_eq!(
            lines.len() as u64,
            value["data"]["actionQueue"]["actionCount"]
                .as_u64()
                .unwrap()
        );
        assert!(lines.iter().any(|action| action["source"] == "admission"));
        assert!(lines.iter().any(|action| action["source"] == "capacity"));

        let _ = tokio::fs::remove_file(path).await;
        let _ = tokio::fs::remove_file(actions_path).await;
        let _ = tokio::fs::remove_file(actions_ndjson_path).await;
    }

    #[tokio::test]
    async fn analyze_pool_writes_next_baseline_from_accepted_candidates() {
        let candidate_path = std::env::temp_dir().join(format!(
            "drs-next-baseline-candidates-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let baseline_path = candidate_path.with_extension("baseline.json");
        let next_baseline_path = candidate_path.with_extension("next-baseline.json");
        let baseline_snapshot = r#"{
            "ua":"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            "platform":"Win32",
            "ua_data_platform":"Windows",
            "ua_data_mobile":false,
            "webdriver":false,
            "languages":"en-US,en",
            "max_touch_points":0,
            "hardware_concurrency":8,
            "device_memory":8,
            "screen":"1920x1080",
            "device_pixel_ratio":1,
            "timezone":"America/New_York",
            "webgl_renderer":"ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11)",
            "canvas_hash":"11111111"
        }"#;
        let safe_candidate = r#"{
            "ua":"Mozilla/5.0 (Macintosh; Intel Mac OS X 13_6) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36",
            "platform":"MacIntel",
            "ua_data_platform":"macOS",
            "ua_data_mobile":false,
            "webdriver":false,
            "languages":"fr-FR,fr",
            "max_touch_points":0,
            "hardware_concurrency":10,
            "device_memory":16,
            "screen":"1440x900",
            "device_pixel_ratio":2,
            "timezone":"Europe/Paris",
            "webgl_renderer":"ANGLE (Apple, Apple M2, OpenGL)",
            "canvas_hash":"22222222"
        }"#;
        tokio::fs::write(
            &candidate_path,
            format!("[{baseline_snapshot},{safe_candidate}]"),
        )
        .await
        .unwrap();
        tokio::fs::write(&baseline_path, format!("[{baseline_snapshot}]"))
            .await
            .unwrap();

        let response = analyze_pool(
            &candidate_path,
            Some(&baseline_path),
            IdentityGate::default(),
            None,
            None,
            Some(&next_baseline_path),
            None,
            None,
            false,
            false,
            false,
        )
        .await
        .unwrap();
        let value = response.into_value();
        let next_baseline = parse_snapshots(
            &tokio::fs::read_to_string(&next_baseline_path)
                .await
                .unwrap(),
        )
        .unwrap();

        assert_eq!(value["data"]["admission"]["action"], "partial_quarantine");
        assert_eq!(value["data"]["admission"]["acceptIndexes"][0], 1);
        assert_eq!(value["data"]["admission"]["quarantineIndexes"][0], 0);
        assert_eq!(value["data"]["baselineOut"]["count"], 2);
        assert_eq!(value["data"]["baselineOut"]["baselineCount"], 1);
        assert_eq!(value["data"]["baselineOut"]["acceptedAdded"], 1);
        assert_eq!(value["data"]["baselineOut"]["format"], "json_array");
        assert_eq!(next_baseline.len(), 2);
        assert_eq!(next_baseline[0].canvas_hash, "11111111");
        assert_eq!(next_baseline[1].canvas_hash, "22222222");

        let _ = tokio::fs::remove_file(candidate_path).await;
        let _ = tokio::fs::remove_file(baseline_path).await;
        let _ = tokio::fs::remove_file(next_baseline_path).await;
    }

    #[tokio::test]
    async fn write_response_snapshots_appends_reparseable_ndjson() {
        let path = std::env::temp_dir().join(format!(
            "drs-identity-snapshots-{}-{}.ndjson",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let response = JsonResponse::ok(json!({
            "scope": "tab",
            "report": {
                "score": 100,
                "snapshot": {
                    "ua":"Mozilla/5.0",
                    "platform":"MacIntel",
                    "languages":"en-US,en",
                    "hardware_concurrency":8,
                    "device_memory":8,
                    "screen":"1440x900",
                    "device_pixel_ratio":2,
                    "timezone":"America/Los_Angeles",
                    "webgl_renderer":"ANGLE (Apple)",
                    "canvas_hash":"abc12345"
                },
                "issues": []
            }
        }));

        let n = write_response_snapshots(&response, &path, true)
            .await
            .unwrap();
        let m = write_response_snapshots(&response, &path, true)
            .await
            .unwrap();
        let text = tokio::fs::read_to_string(&path).await.unwrap();
        let snapshots = parse_snapshots(&text).unwrap();

        assert_eq!(n, 1);
        assert_eq!(m, 1);
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].canvas_hash, "abc12345");

        let _ = tokio::fs::remove_file(path).await;
    }
}
