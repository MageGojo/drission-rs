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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum IdentityDriftMatchMode {
    /// Use labels when both files provide them, otherwise fall back to input order.
    Auto,
    /// Compare snapshots by input order.
    Index,
    /// Compare snapshots by accountId/id/label/name/key fields.
    Label,
}

impl Default for IdentityDriftMatchMode {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum IdentityLifecycleBaselinePolicy {
    /// Keep active current snapshots, preserve baseline snapshots for repair/missing profiles.
    Conservative,
    /// Keep only profiles that stayed stable in the current sample.
    ActiveOnly,
    /// Accept current snapshots for active and repair states, excluding quarantine/new/missing.
    AcceptCurrentRepair,
}

impl Default for IdentityLifecycleBaselinePolicy {
    fn default() -> Self {
        Self::Conservative
    }
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
    Identity {
        pool: bool,
        gate: IdentityGate,
    },
    Title,
    Url,
    /// Open a URL (or reuse active tab) and return a page content bundle for agents.
    Extract {
        url: Option<String>,
        wait_selector: Option<String>,
        timeout_ms: Option<u64>,
        pass_cf: bool,
        include_html: bool,
        include_ax_json: bool,
        max_text_chars: Option<usize>,
        screenshot_out: Option<PathBuf>,
        full_screenshot: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum IdentityGatePreset {
    /// Loose smoke-test gate: catches only obvious identity and pool problems.
    Lenient,
    /// Default production-ish gate for normal account pools.
    Balanced,
    /// Strict gate for sensitive pools where weak separation should fail early.
    Strict,
}

impl IdentityGatePreset {
    pub fn gate(self) -> IdentityGate {
        let mut gate = match self {
            IdentityGatePreset::Lenient => IdentityGate {
                min_score: Some(70),
                max_linkability: Some(60),
                fail_on_high_risk: true,
                fail_on_risky_pairs: false,
                ..IdentityGate::default()
            },
            IdentityGatePreset::Balanced => IdentityGate {
                min_score: Some(80),
                max_linkability: Some(30),
                max_concentrated_signals: Some(8),
                min_entropy_score: Some(55),
                fail_on_high_risk: true,
                fail_on_risky_pairs: true,
                ..IdentityGate::default()
            },
            IdentityGatePreset::Strict => IdentityGate {
                min_score: Some(90),
                max_linkability: Some(20),
                max_concentrated_signals: Some(4),
                min_entropy_score: Some(70),
                max_nominal_to_effective_ratio: Some(2.0),
                fail_on_high_risk: true,
                fail_on_risky_pairs: true,
                ..IdentityGate::default()
            },
        };
        gate.preset = Some(self);
        gate
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IdentityGate {
    /// Optional named policy preset. Explicit fields below override numeric preset thresholds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<IdentityGatePreset>,
    /// Minimum acceptable identity score for every checked tab.
    pub min_score: Option<u8>,
    /// Maximum acceptable pairwise linkability score. Only meaningful for pool checks.
    pub max_linkability: Option<u8>,
    /// Maximum acceptable largest stable-signal bucket ratio. Only meaningful for pool checks.
    pub max_concentration_ratio: Option<f64>,
    /// Maximum number of stable signals that may have repeated values in the pool.
    pub max_concentrated_signals: Option<usize>,
    /// Minimum acceptable identity entropy score for the pool.
    pub min_entropy_score: Option<u8>,
    /// Minimum acceptable effective identity count for the pool.
    pub min_effective_identity_count: Option<f64>,
    /// Maximum acceptable nominal/effective identity ratio for the pool.
    pub max_nominal_to_effective_ratio: Option<f64>,
    /// Fail the gate if any checked tab has a high-risk identity issue.
    pub fail_on_high_risk: bool,
    /// Fail the gate if the pool report contains risky pairs.
    pub fail_on_risky_pairs: bool,
}

impl IdentityGate {
    pub fn effective(&self) -> Self {
        let mut gate = self
            .preset
            .map(IdentityGatePreset::gate)
            .unwrap_or_default();
        gate.preset = self.preset;
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
        gate.fail_on_high_risk |= self.fail_on_high_risk;
        gate.fail_on_risky_pairs |= self.fail_on_risky_pairs;
        gate
    }

    pub fn is_active(&self) -> bool {
        self.preset.is_some()
            || self.min_score.is_some()
            || self.max_linkability.is_some()
            || self.max_concentration_ratio.is_some()
            || self.max_concentrated_signals.is_some()
            || self.min_entropy_score.is_some()
            || self.min_effective_identity_count.is_some()
            || self.max_nominal_to_effective_ratio.is_some()
            || self.fail_on_high_risk
            || self.fail_on_risky_pairs
    }

    pub fn evaluate_identity_report(
        &self,
        report: &drission::fingerprint::IdentityReport,
    ) -> IdentityGateReport {
        let criteria = self.effective();
        if !criteria.is_active() {
            return IdentityGateReport::pass(criteria);
        }

        let mut failures = Vec::new();
        if let Some(min_score) = criteria.min_score {
            if report.score < min_score {
                failures.push(format!(
                    "identity_score_below_min: score {} < {}",
                    report.score, min_score
                ));
            }
        }
        if criteria.fail_on_high_risk && report.has_high_risk() {
            let codes = report
                .issues
                .iter()
                .filter(|issue| issue.severity == drission::fingerprint::IdentitySeverity::High)
                .map(|issue| issue.code.as_str())
                .collect::<Vec<_>>()
                .join(",");
            failures.push(format!("high_risk_identity_issue: {codes}"));
        }

        IdentityGateReport::from_failures(criteria, failures)
    }

    pub fn evaluate_pool_report(
        &self,
        report: &drission::fingerprint::IdentityPoolReport,
    ) -> IdentityGateReport {
        let criteria = self.effective();
        if !criteria.is_active() {
            return IdentityGateReport::pass(criteria);
        }

        let mut failures = Vec::new();
        if let Some(min_score) = criteria.min_score {
            let indexes = report
                .identity_reports
                .iter()
                .enumerate()
                .filter_map(|(index, identity)| (identity.score < min_score).then_some(index))
                .collect::<Vec<_>>();
            if !indexes.is_empty() {
                failures.push(format!(
                    "identity_score_below_min: indexes {:?} below {}",
                    indexes, min_score
                ));
            }
        }
        if let Some(max_linkability) = criteria.max_linkability {
            if report.max_linkability > max_linkability {
                failures.push(format!(
                    "pool_linkability_above_max: max {} > {}",
                    report.max_linkability, max_linkability
                ));
            }
        }
        if let Some(max_concentration_ratio) = criteria.max_concentration_ratio {
            if report.diversity.max_concentration_ratio > max_concentration_ratio {
                failures.push(format!(
                    "pool_concentration_ratio_above_max: max {:.3} > {:.3}",
                    report.diversity.max_concentration_ratio, max_concentration_ratio
                ));
            }
        }
        if let Some(max_concentrated_signals) = criteria.max_concentrated_signals {
            if report.diversity.concentrated_signal_count > max_concentrated_signals {
                failures.push(format!(
                    "pool_concentrated_signals_above_max: {} > {}",
                    report.diversity.concentrated_signal_count, max_concentrated_signals
                ));
            }
        }
        if let Some(min_entropy_score) = criteria.min_entropy_score {
            if report.entropy_budget.entropy_score < min_entropy_score {
                failures.push(format!(
                    "pool_entropy_score_below_min: score {} < {}",
                    report.entropy_budget.entropy_score, min_entropy_score
                ));
            }
        }
        if let Some(min_effective_identity_count) = criteria.min_effective_identity_count {
            if report.entropy_budget.effective_identity_count < min_effective_identity_count {
                failures.push(format!(
                    "pool_effective_identities_below_min: {:.2} < {:.2}",
                    report.entropy_budget.effective_identity_count, min_effective_identity_count
                ));
            }
        }
        if let Some(max_nominal_to_effective_ratio) = criteria.max_nominal_to_effective_ratio {
            if report.entropy_budget.nominal_to_effective_ratio > max_nominal_to_effective_ratio {
                failures.push(format!(
                    "pool_nominal_to_effective_ratio_above_max: {:.2} > {:.2}",
                    report.entropy_budget.nominal_to_effective_ratio,
                    max_nominal_to_effective_ratio
                ));
            }
        }
        if criteria.fail_on_high_risk {
            let indexes = report
                .identity_reports
                .iter()
                .enumerate()
                .filter_map(|(index, identity)| identity.has_high_risk().then_some(index))
                .collect::<Vec<_>>();
            if !indexes.is_empty() {
                failures.push(format!("high_risk_identity_issue: indexes {:?}", indexes));
            }
        }
        if criteria.fail_on_risky_pairs && report.has_risky_pairs() {
            failures.push(format!(
                "risky_identity_pairs: {} risky pairs",
                report.risky_pairs.len()
            ));
        }

        IdentityGateReport::from_failures(criteria, failures)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IdentityGateReport {
    pub passed: bool,
    pub criteria: IdentityGate,
    pub failures: Vec<String>,
}

impl IdentityGateReport {
    pub fn pass(criteria: IdentityGate) -> Self {
        Self {
            passed: true,
            criteria,
            failures: Vec::new(),
        }
    }

    pub fn from_failures(criteria: IdentityGate, failures: Vec<String>) -> Self {
        Self {
            passed: failures.is_empty(),
            criteria,
            failures,
        }
    }
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

    #[test]
    fn identity_gate_fails_on_low_score_and_high_risk() {
        let report = drission::fingerprint::IdentityReport {
            identity_id: "fp_test".into(),
            stable_hash: "test".into(),
            score: 60,
            snapshot: drission::fingerprint::FingerprintSnapshot::default(),
            issues: vec![drission::fingerprint::IdentityIssue {
                severity: drission::fingerprint::IdentitySeverity::High,
                code: "webdriver_true".into(),
                message: "webdriver exposed".into(),
                suggestion: "patch webdriver".into(),
            }],
            fix_plan: drission::fingerprint::IdentityFixPlan::default(),
        };

        let gate = IdentityGate {
            min_score: Some(80),
            fail_on_high_risk: true,
            ..IdentityGate::default()
        }
        .evaluate_identity_report(&report);

        assert!(!gate.passed);
        assert_eq!(gate.failures.len(), 2);
    }

    #[test]
    fn identity_pool_gate_fails_on_linkability_and_risky_pairs() {
        let report = drission::fingerprint::IdentityPoolReport {
            pool_id: "pool_test".into(),
            stable_hash: "test".into(),
            snapshot_ids: Vec::new(),
            size: 2,
            max_linkability: 70,
            identity_reports: Vec::new(),
            risky_pairs: vec![drission::fingerprint::LinkabilityPair {
                left_index: 0,
                right_index: 1,
                score: 70,
                same_identity_likely: true,
                signals: Vec::new(),
            }],
            duplicate_signals: Vec::new(),
            diversity: drission::fingerprint::IdentityPoolDiversityReport::default(),
            entropy_budget: drission::fingerprint::IdentityEntropyBudget::default(),
            capacity_plan: drission::fingerprint::IdentityCapacityPlan::default(),
            remediation_plan: drission::fingerprint::IdentityPoolRemediationPlan::default(),
        };

        let gate = IdentityGate {
            max_linkability: Some(25),
            fail_on_risky_pairs: true,
            ..IdentityGate::default()
        }
        .evaluate_pool_report(&report);

        assert!(!gate.passed);
        assert_eq!(gate.failures.len(), 2);
    }

    #[test]
    fn identity_pool_gate_fails_on_diversity_concentration() {
        let report = drission::fingerprint::IdentityPoolReport {
            pool_id: "pool_test".into(),
            stable_hash: "test".into(),
            snapshot_ids: Vec::new(),
            size: 3,
            max_linkability: 0,
            identity_reports: Vec::new(),
            risky_pairs: Vec::new(),
            duplicate_signals: Vec::new(),
            diversity: drission::fingerprint::IdentityPoolDiversityReport {
                size: 3,
                signal_count: 10,
                concentrated_signal_count: 5,
                max_concentration_ratio: 1.0,
                average_unique_ratio: 0.4,
                signals: Vec::new(),
            },
            entropy_budget: drission::fingerprint::IdentityEntropyBudget::default(),
            capacity_plan: drission::fingerprint::IdentityCapacityPlan::default(),
            remediation_plan: drission::fingerprint::IdentityPoolRemediationPlan::default(),
        };

        let gate = IdentityGate {
            max_concentration_ratio: Some(0.8),
            max_concentrated_signals: Some(3),
            ..IdentityGate::default()
        }
        .evaluate_pool_report(&report);

        assert!(!gate.passed);
        assert_eq!(gate.failures.len(), 2);
        assert!(
            gate.failures
                .iter()
                .any(|failure| failure.starts_with("pool_concentration_ratio_above_max"))
        );
        assert!(
            gate.failures
                .iter()
                .any(|failure| failure.starts_with("pool_concentrated_signals_above_max"))
        );
    }

    #[test]
    fn identity_pool_gate_fails_on_entropy_budget() {
        let report = drission::fingerprint::IdentityPoolReport {
            pool_id: "pool_test".into(),
            stable_hash: "test".into(),
            snapshot_ids: Vec::new(),
            size: 10,
            max_linkability: 0,
            identity_reports: Vec::new(),
            risky_pairs: Vec::new(),
            duplicate_signals: Vec::new(),
            diversity: drission::fingerprint::IdentityPoolDiversityReport::default(),
            entropy_budget: drission::fingerprint::IdentityEntropyBudget {
                size: 10,
                signal_count: 8,
                status: drission::fingerprint::IdentityEntropyStatus::Collapsed,
                weighted_entropy: 0.2,
                entropy_score: 20,
                effective_identity_count: 2.0,
                nominal_to_effective_ratio: 5.0,
                bottleneck_count: 2,
                bottleneck_signals: Vec::new(),
            },
            capacity_plan: drission::fingerprint::IdentityCapacityPlan::default(),
            remediation_plan: drission::fingerprint::IdentityPoolRemediationPlan::default(),
        };

        let gate = IdentityGate {
            min_entropy_score: Some(60),
            min_effective_identity_count: Some(6.0),
            max_nominal_to_effective_ratio: Some(3.0),
            ..IdentityGate::default()
        }
        .evaluate_pool_report(&report);

        assert!(!gate.passed);
        assert_eq!(gate.failures.len(), 3);
        assert!(
            gate.failures
                .iter()
                .any(|failure| failure.starts_with("pool_entropy_score_below_min"))
        );
        assert!(
            gate.failures
                .iter()
                .any(|failure| { failure.starts_with("pool_effective_identities_below_min") })
        );
        assert!(
            gate.failures.iter().any(|failure| {
                failure.starts_with("pool_nominal_to_effective_ratio_above_max")
            })
        );
    }

    #[test]
    fn identity_gate_preset_expands_and_allows_numeric_override() {
        let gate = IdentityGate {
            preset: Some(IdentityGatePreset::Strict),
            max_linkability: Some(15),
            max_concentrated_signals: Some(2),
            min_entropy_score: Some(75),
            ..IdentityGate::default()
        }
        .effective();

        assert_eq!(gate.preset, Some(IdentityGatePreset::Strict));
        assert_eq!(gate.min_score, Some(90));
        assert_eq!(gate.max_linkability, Some(15));
        assert_eq!(gate.max_concentration_ratio, None);
        assert_eq!(gate.max_concentrated_signals, Some(2));
        assert_eq!(gate.min_entropy_score, Some(75));
        assert_eq!(gate.max_nominal_to_effective_ratio, Some(2.0));
        assert!(gate.fail_on_high_risk);
        assert!(gate.fail_on_risky_pairs);
    }
}
