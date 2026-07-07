//! 公开 API 离线集成测试(不开浏览器,CI 可跑)。
//!
//! 与 `src/**` 里的 inline 单测互补:这里只通过 crate 的**对外公开导出**(`prelude` 等)
//! 验证契约,确保发布给用户的 API 表面行为稳定。全部确定性、无网络、无浏览器。

use std::collections::HashMap;

use drission::codec::{FrameDecoder, encode_frame};
use drission::prelude::*;

#[test]
fn locator_prelude_parses_dp_syntax() {
    assert!(matches!(parse_locator("#kw"), Query::Css(_)));
    assert!(matches!(parse_locator("css:div.box"), Query::Css(_)));
    assert!(matches!(parse_locator("@id:kw"), Query::Xpath(_)));
    assert!(matches!(parse_locator("登录"), Query::Xpath(_)));
    assert_eq!(parse_locator("tag: li").as_str(), "li");
    assert!(parse_locator("xpath://a").is_xpath());
}

#[test]
fn codec_roundtrip_via_public_api() {
    let mut stream = Vec::new();
    stream.extend_from_slice(&encode_frame(b"{\"a\":1}"));
    stream.extend_from_slice(&encode_frame(b"{\"b\":2}"));

    let mut d = FrameDecoder::new();
    d.push(&stream);
    assert_eq!(d.next_frame().unwrap(), b"{\"a\":1}");
    assert_eq!(d.next_frame().unwrap(), b"{\"b\":2}");
    assert!(d.next_frame().is_none());
}

#[test]
fn scrape_exports_csv_and_json() {
    let rows = vec![
        vec!["name".to_string(), "price".to_string()],
        vec!["苹果".to_string(), "3,5".to_string()],
    ];
    let csv = rows_to_csv(&rows);
    assert!(csv.contains("\"3,5\""), "含逗号字段应被引号包裹: {csv:?}");
    assert!(csv.ends_with("\r\n"));

    let mut rec = HashMap::new();
    rec.insert("k".to_string(), "v".to_string());
    let json = records_to_json(&[rec]);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed[0]["k"], "v");
}

#[test]
fn fingerprint_identity_report_is_public_api() {
    let fp = FingerprintSnapshot {
        ua: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/149.0.0.0".into(),
        platform: "MacIntel".into(),
        webgl_renderer: "ANGLE (Apple, ANGLE Metal Renderer: Apple M1)".into(),
        canvas_hash: "a1b2c3d4".into(),
        ..Default::default()
    };
    let identity_id = fp.identity_id();
    assert!(identity_id.starts_with("fp_"));
    let report: IdentityReport = fp.diagnose();
    assert_eq!(report.identity_id, identity_id);
    assert_eq!(report.stable_hash, fp.stable_hash());
    assert!(report.has_high_risk());
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.severity == IdentitySeverity::High)
    );
    let fix_plan: IdentityFixPlan = report.fix_plan.clone();
    assert!(!fix_plan.is_empty());
    let fix_action: &IdentityFixAction = fix_plan
        .actions
        .iter()
        .find(|action| action.target == IdentityFixTarget::ProfileOs)
        .expect("os mismatch should produce a profile fix action");
    assert_eq!(fix_action.priority, IdentityFixPriority::High);

    let link: LinkabilityReport = fp.linkability_to(&fp);
    assert!(link.same_identity_likely);
    assert!(
        link.signals
            .iter()
            .any(|s| s.strength == LinkabilityStrength::Strong)
    );
    let mut drifted = fp.clone();
    drifted.canvas_hash = "ffffffff".into();
    let drift: IdentityDriftReport = fp.drift_to(&drifted);
    assert_eq!(drift.severity, IdentityDriftSeverity::High);
    let drift_signal: &IdentityDriftSignal = drift
        .signals
        .iter()
        .find(|signal| signal.code == "canvas.changed")
        .expect("canvas change should be reported as drift");
    assert_eq!(drift_signal.severity, IdentityDriftSeverity::High);
    let drift_plan: IdentityDriftRemediationPlan = drift.remediation_plan.clone();
    assert!(!drift_plan.is_empty());
    let drift_action: &IdentityDriftRemediationAction = drift_plan
        .actions
        .iter()
        .find(|action| action.code == "drift.restore_canvas_seed")
        .expect("canvas drift should produce a remediation action");
    assert_eq!(drift_action.target, IdentityDriftRemediationTarget::Canvas);

    let pool: IdentityPoolReport = IdentityPoolReport::analyze(&[fp.clone(), fp]);
    assert!(pool.pool_id.starts_with("pool_"));
    assert_eq!(pool.snapshot_ids.len(), 2);
    let diversity: IdentityPoolDiversityReport = pool.diversity_report();
    assert!(!diversity.is_diverse());
    let diversity_signal: &IdentityPoolDiversitySignal = diversity
        .signals
        .iter()
        .find(|signal| signal.code == "canvas.same")
        .expect("duplicate pool should report canvas diversity");
    assert_eq!(diversity_signal.max_bucket_count, 2);
    let diversity_bucket: &IdentityPoolDiversityBucket = &diversity_signal.buckets[0];
    assert_eq!(diversity_bucket.count, 2);
    let entropy: IdentityEntropyBudget = pool.entropy_budget();
    assert_eq!(entropy.status, IdentityEntropyStatus::Collapsed);
    assert_eq!(entropy.effective_identity_count, 1.0);
    let entropy_signal: &IdentityEntropySignalBudget = entropy
        .bottleneck_signals
        .iter()
        .find(|signal| signal.code == "canvas.same")
        .expect("duplicate canvas should be an entropy bottleneck");
    assert_eq!(entropy_signal.normalized_entropy, 0.0);
    let capacity: IdentityCapacityPlan = pool.capacity_plan();
    assert_eq!(capacity.status, IdentityCapacityStatus::Exhausted);
    assert_eq!(capacity.additional_distinct_profiles_needed, 1);
    let capacity_action: &IdentityCapacityAction = capacity
        .actions
        .iter()
        .find(|action| action.code == "capacity.disperse_canvas_seed")
        .expect("duplicate canvas should produce a capacity action");
    assert_eq!(capacity_action.priority, IdentityFixPriority::High);
    assert!(pool.has_risky_pairs());
    let clusters: Vec<IdentityCluster> = pool.risk_clusters();
    assert_eq!(clusters.len(), 1);
    assert_eq!(clusters[0].indexes, vec![0, 1]);
    let offenders: Vec<IdentityOffender> = pool.risk_offenders();
    assert_eq!(offenders.len(), 2);
    assert_eq!(offenders[0].linked_indexes, vec![1]);
    let quarantine: IdentityQuarantinePlan = pool.quarantine_plan();
    assert_eq!(quarantine.indexes, vec![0]);
    let admission: IdentityAdmissionPlan = pool.admission_plan();
    assert!(matches!(
        admission.action,
        IdentityAdmissionAction::PartialQuarantine
    ));
    assert_eq!(admission.quarantine_indexes, vec![0]);
    let remediation: IdentityPoolRemediationPlan = pool.remediation_plan();
    assert!(!remediation.is_empty());
    let pool_action: &IdentityPoolRemediationAction = remediation
        .actions
        .iter()
        .find(|action| action.target == IdentityPoolRemediationTarget::Admission)
        .expect("risky pool should include an admission remediation action");
    assert_eq!(pool_action.priority, IdentityFixPriority::High);
    assert!(
        pool.duplicate_signals
            .iter()
            .any(|s| matches!(s.strength, LinkabilityStrength::Strong))
    );
}

#[tokio::test]
async fn scrape_write_csv_roundtrip() {
    let dir = std::env::temp_dir().join(format!("drission_it_{}", std::process::id()));
    let csv_path = dir.join("out.csv");
    let rows = vec![vec!["a".to_string(), "b".to_string()]];

    write_csv(&csv_path, &rows).await.expect("write csv");
    let back = tokio::fs::read_to_string(&csv_path)
        .await
        .expect("read csv");
    assert_eq!(back, "a,b\r\n");

    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[cfg(feature = "camoufox")]
#[test]
fn browser_options_builder_is_public() {
    // 仅验证公开 builder 可链式构造(不启动浏览器)。Camoufox 后端专有。
    let _opts = BrowserOptions::new().headless(true);
}
