mod backend;
mod cli;
mod daemon;
mod engine;
mod identity_cmd;
mod mcp;
mod ocr_cmd;
mod paths;
mod protocol;
mod setup;

use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde_json::{Value, json};

use crate::cli::{Cli, Command};
use crate::protocol::{IdentityGate, JsonResponse};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let json_mode = cli.json;

    match cli.command {
        Command::Serve(args) => {
            engine::validate_backend_or_bail(args.backend)?;
            let user_data_dir = resolve_profile_dir(args.user_data_dir)?;
            daemon::run_server(args.backend, args.headless, user_data_dir).await?;
            Ok(())
        }
        Command::EnsureServe(args) => {
            engine::validate_backend_or_bail(args.backend)?;
            let user_data_dir = resolve_profile_dir(args.user_data_dir)?;
            let state = daemon::ensure_daemon(args.backend, args.headless, user_data_dir).await?;
            let response = JsonResponse::ok(serde_json::json!({
                "ready": true,
                "endpoint": state.endpoint(),
                "backend": state.backend,
                "pid": state.pid,
            }));
            print_response(response, json_mode)?;
            Ok(())
        }
        Command::Mcp(args) => {
            init_tracing();
            engine::validate_backend_or_bail(args.backend)?;
            let user_data_dir = resolve_profile_dir(args.user_data_dir)?;
            mcp::run_mcp(args.backend, args.headless, user_data_dir, args.standalone).await?;
            Ok(())
        }
        Command::Setup(args) => {
            let response = setup::run_setup(setup::SetupOptions {
                target: args.target,
                scope: args.scope,
                dir: args.dir,
                backend: args.backend,
                headless: !args.no_headless,
                name: args.name,
                dry_run: args.dry_run,
            })
            .await?;
            print_response(response, json_mode)?;
            Ok(())
        }
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
            let policy = identity_cmd::load_identity_policy(policy.as_deref()).await?;
            let cli_gate: IdentityGate = gate.into();
            let gate = match policy.as_ref() {
                Some(policy) => policy.merge_gate(cli_gate),
                None => cli_gate,
            };
            let mut response = identity_cmd::analyze_pool(
                &snapshots,
                against.as_deref(),
                gate,
                accept_out.as_deref(),
                quarantine_out.as_deref(),
                baseline_out.as_deref(),
                ledger_out.as_deref(),
                actions_out.as_deref(),
                append_split,
                append_ledger,
                append_actions,
            )
            .await?;
            identity_cmd::attach_identity_policy(&mut response, policy.as_ref());
            let gate_failed = identity_gate_failed(&response);
            print_response(response, json_mode)?;
            if gate_failed {
                std::process::exit(2);
            }
            Ok(())
        }
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
            let policy = identity_cmd::load_identity_policy(policy.as_deref()).await?;
            let resolved = policy.as_ref().map_or_else(
                || identity_cmd::ResolvedDriftPolicy {
                    max_drift_score,
                    fail_on_high_risk_drift,
                    match_by: match_by.unwrap_or_default(),
                },
                |policy| policy.merge_drift(max_drift_score, fail_on_high_risk_drift, match_by),
            );
            let mut response = identity_cmd::analyze_drift(
                &before,
                &after,
                resolved.max_drift_score,
                resolved.fail_on_high_risk_drift,
                resolved.match_by,
                actions_out.as_deref(),
                append_actions,
            )
            .await?;
            identity_cmd::attach_identity_policy(&mut response, policy.as_ref());
            let gate_failed = identity_gate_failed(&response);
            print_response(response, json_mode)?;
            if gate_failed {
                std::process::exit(2);
            }
            Ok(())
        }
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
            let policy = identity_cmd::load_identity_policy(policy.as_deref()).await?;
            let resolved = policy.as_ref().map_or_else(
                || identity_cmd::ResolvedLifecyclePolicy {
                    max_drift_score,
                    fail_on_high_risk_drift,
                    fail_on_missing_current,
                    fail_on_new_current,
                    match_by: match_by.unwrap_or_default(),
                    next_baseline_policy: next_baseline_policy.unwrap_or_default(),
                },
                |policy| {
                    policy.merge_lifecycle(
                        max_drift_score,
                        fail_on_high_risk_drift,
                        fail_on_missing_current,
                        fail_on_new_current,
                        match_by,
                        next_baseline_policy,
                    )
                },
            );
            let mut response = identity_cmd::analyze_lifecycle(
                &baseline,
                &current,
                resolved.max_drift_score,
                resolved.fail_on_high_risk_drift,
                resolved.fail_on_missing_current,
                resolved.fail_on_new_current,
                resolved.match_by,
                ledger_out.as_deref(),
                delta_out.as_deref(),
                journal_out.as_deref(),
                state_out_dir.as_deref(),
                actions_out.as_deref(),
                next_baseline_out.as_deref(),
                resolved.next_baseline_policy,
                append_ledger,
                append_journal,
                append_actions,
            )
            .await?;
            identity_cmd::attach_identity_policy(&mut response, policy.as_ref());
            let gate_failed = identity_gate_failed(&response);
            print_response(response, json_mode)?;
            if gate_failed {
                std::process::exit(2);
            }
            Ok(())
        }
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
            let response = identity_cmd::apply_identity_actions(
                &actions,
                profile_root.as_deref(),
                profile_map.as_deref(),
                quarantine_dir.as_deref(),
                execute,
                journal_out.as_deref(),
                append_journal,
                asset_state_out.as_deref(),
                append_asset_state,
            )
            .await?;
            print_response(response, json_mode)?;
            Ok(())
        }
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
            let response = identity_cmd::build_identity_plan(
                &inputs,
                title.as_deref(),
                out.as_deref(),
                html_out.as_deref(),
                asset_manifest.as_deref(),
                asset_manifest_out.as_deref(),
                dispatch_out.as_deref(),
                append_dispatch,
            )
            .await?;
            print_response(response, json_mode)?;
            Ok(())
        }
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
            let response = identity_cmd::claim_identity_dispatch(
                &dispatch,
                worker.as_deref(),
                limit,
                lease_seconds,
                include_blocked,
                claim_ledger.as_deref(),
                include_leased,
                completion_ledger.as_deref(),
                include_completed,
                claim_out.as_deref(),
                append_claim,
            )
            .await?;
            print_response(response, json_mode)?;
            Ok(())
        }
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
            let response = identity_cmd::renew_identity_dispatch(
                &claims,
                worker.as_deref(),
                claim_id.as_deref(),
                &dedupe_key,
                lease_seconds,
                include_expired,
                claim_out.as_deref(),
                append_claim,
            )
            .await?;
            print_response(response, json_mode)?;
            Ok(())
        }
        Command::IdentityDispatchReconcile {
            asset_manifest,
            claim_ledger,
            completion_ledger,
            asset_manifest_out,
        } => {
            let response = identity_cmd::reconcile_identity_dispatch_manifest(
                &asset_manifest,
                claim_ledger.as_deref(),
                completion_ledger.as_deref(),
                asset_manifest_out.as_deref(),
            )
            .await?;
            print_response(response, json_mode)?;
            Ok(())
        }
        Command::IdentityAssetsValidate {
            asset_manifest,
            strict,
            validate_out,
        } => {
            let response = identity_cmd::validate_identity_assets(
                &asset_manifest,
                strict,
                validate_out.as_deref(),
            )
            .await?;
            let invalid = response
                .data
                .as_ref()
                .and_then(|data| data.get("valid"))
                .and_then(Value::as_bool)
                == Some(false);
            print_response(response, json_mode)?;
            if strict && invalid {
                std::process::exit(2);
            }
            Ok(())
        }
        Command::IdentityAssetsReconcileRuntime {
            asset_manifest,
            release_ledger,
            asset_manifest_out,
        } => {
            let response = identity_cmd::reconcile_identity_asset_runtime_manifest(
                &asset_manifest,
                &release_ledger,
                asset_manifest_out.as_deref(),
            )
            .await?;
            print_response(response, json_mode)?;
            Ok(())
        }
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
            let policy = identity_cmd::load_identity_policy(policy.as_deref()).await?;
            let resolved = policy.as_ref().map_or_else(
                || identity_cmd::ResolvedHealthPolicy {
                    window_seconds,
                    repair_threshold: repair_threshold.unwrap_or(3),
                    quarantine_threshold: quarantine_threshold.unwrap_or(5),
                    cooldown_seconds: cooldown_seconds.unwrap_or(900),
                },
                |policy| {
                    policy.merge_health(
                        window_seconds,
                        repair_threshold,
                        quarantine_threshold,
                        cooldown_seconds,
                    )
                },
            );
            let mut response = identity_cmd::health_identity_assets(
                &asset_manifest,
                &release_ledger,
                resolved.window_seconds,
                resolved.repair_threshold,
                resolved.quarantine_threshold,
                resolved.cooldown_seconds,
                asset_manifest_out.as_deref(),
                health_out.as_deref(),
            )
            .await?;
            identity_cmd::attach_identity_policy(&mut response, policy.as_ref());
            print_response(response, json_mode)?;
            Ok(())
        }
        Command::IdentityAssetsForecast {
            asset_manifest,
            allow_state,
            desired_concurrency,
            horizon_seconds,
            include_dispatch_leased,
            include_retry,
            include_failed,
            include_cancelled,
            include_runtime_leased,
            include_missing_profile_dir,
            forecast_out,
        } => {
            let response = identity_cmd::forecast_identity_assets(
                &asset_manifest,
                &allow_state,
                desired_concurrency,
                horizon_seconds,
                include_dispatch_leased,
                include_retry,
                include_failed,
                include_cancelled,
                include_runtime_leased,
                include_missing_profile_dir,
                forecast_out.as_deref(),
            )
            .await?;
            print_response(response, json_mode)?;
            Ok(())
        }
        Command::IdentityAssetsGate {
            asset_manifest,
            desired_concurrency,
            max_wait_seconds,
            allow_wait,
            allow_state,
            include_dispatch_leased,
            include_retry,
            include_failed,
            include_cancelled,
            include_runtime_leased,
            include_missing_profile_dir,
            gate_out,
        } => {
            let response = identity_cmd::gate_identity_assets(
                &asset_manifest,
                desired_concurrency,
                max_wait_seconds,
                allow_wait,
                &allow_state,
                include_dispatch_leased,
                include_retry,
                include_failed,
                include_cancelled,
                include_runtime_leased,
                include_missing_profile_dir,
                gate_out.as_deref(),
            )
            .await?;
            let passed = response
                .data
                .as_ref()
                .and_then(|data| data.get("passed"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            print_response(response, json_mode)?;
            if !passed {
                std::process::exit(2);
            }
            Ok(())
        }
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
            let response = identity_cmd::status_identity_assets(
                &asset_manifest,
                &allow_state,
                desired_concurrency,
                include_dispatch_leased,
                include_retry,
                include_failed,
                include_cancelled,
                include_runtime_leased,
                include_missing_profile_dir,
                status_out.as_deref(),
            )
            .await?;
            print_response(response, json_mode)?;
            Ok(())
        }
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
            let response = identity_cmd::select_identity_assets(
                &asset_manifest,
                limit,
                &allow_state,
                worker.as_deref(),
                job.as_deref(),
                lease_seconds,
                include_dispatch_leased,
                include_retry,
                include_failed,
                include_cancelled,
                include_runtime_leased,
                include_missing_profile_dir,
                asset_manifest_out.as_deref(),
                selection_out.as_deref(),
            )
            .await?;
            print_response(response, json_mode)?;
            Ok(())
        }
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
            let response = identity_cmd::release_identity_assets(
                &asset_manifest,
                status.as_str(),
                worker.as_deref(),
                job.as_deref(),
                &lease_id,
                &account_id,
                &profile_id,
                &identity_id,
                &label,
                cooldown_seconds,
                next_state.as_deref(),
                message.as_deref(),
                result_json.as_deref(),
                asset_manifest_out.as_deref(),
                release_out.as_deref(),
                append_release,
            )
            .await?;
            print_response(response, json_mode)?;
            Ok(())
        }
        Command::IdentityAssetsSweep {
            asset_manifest,
            runtime_grace_seconds,
            dispatch_grace_seconds,
            cooldown_grace_seconds,
            asset_manifest_out,
            sweep_out,
        } => {
            let response = identity_cmd::sweep_identity_assets(
                &asset_manifest,
                runtime_grace_seconds,
                dispatch_grace_seconds,
                cooldown_grace_seconds,
                asset_manifest_out.as_deref(),
                sweep_out.as_deref(),
            )
            .await?;
            print_response(response, json_mode)?;
            Ok(())
        }
        Command::IdentityJob { command } => match command {
            cli::IdentityJobCommand::Run {
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
            } => {
                let response =
                    identity_cmd::run_identity_job(identity_cmd::IdentityJobRunOptions {
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
                        allow_states: allow_state,
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
                        failure_reason_rules: Default::default(),
                        asset_manifest_out,
                        sweep_out,
                        validate_out,
                        gate_out,
                        selection_out,
                        release_out,
                        append_release,
                        runtime_risk_ledgers: runtime_risk_ledger,
                        runtime_risk_window_seconds,
                        runtime_risk_out,
                        append_runtime_risk,
                        explain_out,
                        job_out,
                        command,
                    })
                    .await?;
                let exit_code = response
                    .data
                    .as_ref()
                    .and_then(|data| data.get("exitCode"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    .clamp(0, 255) as i32;
                print_response(response, json_mode)?;
                if exit_code != 0 {
                    std::process::exit(exit_code);
                }
                Ok(())
            }
        },
        Command::IdentityLedger { command } => match command {
            cli::IdentityLedgerCommand::Dashboard {
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
            } => {
                let response = identity_cmd::dashboard_identity_ledgers(
                    &release_ledger,
                    &runtime_risk_ledger,
                    window_seconds,
                    job.as_deref(),
                    worker.as_deref(),
                    reason.as_deref(),
                    retain_recent,
                    top,
                    checkpoint_in.as_deref(),
                    checkpoint_out.as_deref(),
                    out.as_deref(),
                    html_out.as_deref(),
                )
                .await?;
                print_response(response, json_mode)?;
                Ok(())
            }
            cli::IdentityLedgerCommand::Compact {
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
            } => {
                let response = identity_cmd::compact_identity_ledgers(
                    &release_ledger,
                    &runtime_risk_ledger,
                    window_seconds,
                    job.as_deref(),
                    worker.as_deref(),
                    reason.as_deref(),
                    retain_recent,
                    top,
                    checkpoint_in.as_deref(),
                    checkpoint_out.as_deref(),
                    out.as_deref(),
                )
                .await?;
                print_response(response, json_mode)?;
                Ok(())
            }
            cli::IdentityLedgerCommand::Query {
                release_ledger,
                runtime_risk_ledger,
                window_seconds,
                job,
                worker,
                reason,
                top,
                out,
            } => {
                let response = identity_cmd::query_identity_ledgers(
                    &release_ledger,
                    &runtime_risk_ledger,
                    window_seconds,
                    job.as_deref(),
                    worker.as_deref(),
                    reason.as_deref(),
                    top,
                    out.as_deref(),
                )
                .await?;
                print_response(response, json_mode)?;
                Ok(())
            }
            cli::IdentityLedgerCommand::Explain {
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
            } => {
                let profile_dir = profile_dir.as_ref().map(|path| path.display().to_string());
                let response = identity_cmd::explain_identity_ledger(
                    &release_ledger,
                    &runtime_risk_ledger,
                    window_seconds,
                    job.as_deref(),
                    worker.as_deref(),
                    reason.as_deref(),
                    account_id.as_deref(),
                    profile_id.as_deref(),
                    identity_id.as_deref(),
                    label.as_deref(),
                    profile_dir.as_deref(),
                    lease_id.as_deref(),
                    evidence_limit,
                    out.as_deref(),
                )
                .await?;
                print_response(response, json_mode)?;
                Ok(())
            }
        },
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
            let response = identity_cmd::complete_identity_dispatch(
                &claims,
                status.as_str(),
                worker.as_deref(),
                claim_id.as_deref(),
                &dedupe_key,
                retryable,
                retry_after_seconds,
                message.as_deref(),
                result_json.as_deref(),
                complete_out.as_deref(),
                append_complete,
            )
            .await?;
            print_response(response, json_mode)?;
            Ok(())
        }
        #[cfg(feature = "ocr")]
        Command::Ocr { command } => {
            let response = match command {
                cli::OcrCommand::Clickword { image, targets } => {
                    ocr_cmd::clickword(&image, &targets).await?
                }
            };
            print_response(response, json_mode)?;
            Ok(())
        }
        other => {
            if cli.ensure.ensure_serve {
                engine::validate_backend_or_bail(cli.ensure.ensure_backend)?;
                let user_data_dir = resolve_profile_dir(cli.ensure.ensure_user_data_dir.clone())?;
                daemon::ensure_daemon(
                    cli.ensure.ensure_backend,
                    cli.ensure.ensure_headless,
                    user_data_dir,
                )
                .await?;
            }
            let gate_should_exit = other.identity_gate_is_active();
            let snapshot_export = other.identity_snapshot_export();
            let extract_save_out = other.extract_save_out();
            let command = other
                .into_engine()
                .expect("non-local command must map to EngineCommand");
            let mut response = daemon::send_to_daemon(command).await?;
            let ok = response.ok;
            if ok {
                if let Some(path) = extract_save_out {
                    if let Some(data) = response.data.clone() {
                        tokio::fs::write(&path, serde_json::to_string_pretty(&data)?).await?;
                        attach_extract_save_out(&mut response, &path);
                    }
                }
                if let Some(export) = snapshot_export {
                    let count = identity_cmd::write_response_snapshots(
                        &response,
                        &export.path,
                        export.append,
                    )
                    .await?;
                    attach_snapshot_export(&mut response, &export.path, export.append, count);
                }
            }
            let gate_failed = gate_should_exit && identity_gate_failed(&response);
            print_response(response, json_mode)?;
            if !ok {
                std::process::exit(1);
            }
            if gate_failed {
                std::process::exit(2);
            }
            Ok(())
        }
    }
}

/// Default runtime browser commands (`serve`/`ensure-serve`/`mcp`) to a stable
/// per-user profile so cookies and logins persist across restarts, unless the
/// caller supplies an explicit directory.
fn resolve_profile_dir(explicit: Option<std::path::PathBuf>) -> Result<Option<std::path::PathBuf>> {
    match explicit {
        Some(dir) => Ok(Some(dir)),
        None => Ok(Some(paths::default_profile_dir()?)),
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}

fn print_response(response: JsonResponse, json_mode: bool) -> Result<()> {
    if json_mode {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }

    if response.ok {
        let data = response.data.unwrap_or(Value::Null);
        if let Some(s) = data.get("text").and_then(Value::as_str) {
            println!("{s}");
        } else if let Some(s) = data.get("html").and_then(Value::as_str) {
            println!("{s}");
        } else if let Some(s) = data.get("outline").and_then(Value::as_str) {
            println!("{s}");
        } else if let Some(value) = data.get("value") {
            println!("{}", serde_json::to_string_pretty(value)?);
        } else {
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
    } else if let Some(error) = response.error {
        eprintln!("{}: {}", error.code, error.message);
        if let Some(hint) = error.hint {
            eprintln!("hint: {hint}");
        }
    }
    Ok(())
}

fn identity_gate_failed(response: &JsonResponse) -> bool {
    response
        .data
        .as_ref()
        .and_then(|data| data.get("gate"))
        .and_then(|gate| gate.get("passed"))
        .and_then(Value::as_bool)
        == Some(false)
}

fn attach_extract_save_out(response: &mut JsonResponse, path: &Path) {
    if let Some(Value::Object(data)) = response.data.as_mut() {
        data.insert(
            "saveOut".to_string(),
            json!({
                "path": path.display().to_string(),
                "format": "json",
            }),
        );
    }
}

fn attach_snapshot_export(response: &mut JsonResponse, path: &Path, append: bool, count: usize) {
    if let Some(Value::Object(data)) = response.data.as_mut() {
        data.insert(
            "snapshotsOut".to_string(),
            json!({
                "path": path.display().to_string(),
                "append": append,
                "count": count,
                "format": if append { "ndjson" } else { "json_array" },
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_gate_failed_reads_gate_result() {
        let failed = JsonResponse::ok(serde_json::json!({
            "gate": { "passed": false }
        }));
        let passed = JsonResponse::ok(serde_json::json!({
            "gate": { "passed": true }
        }));
        let unrelated = JsonResponse::ok(serde_json::json!({ "answer": 42 }));

        assert!(identity_gate_failed(&failed));
        assert!(!identity_gate_failed(&passed));
        assert!(!identity_gate_failed(&unrelated));
    }

    #[test]
    fn attach_snapshot_export_adds_metadata() {
        let mut response = JsonResponse::ok(serde_json::json!({ "scope": "tab" }));
        attach_snapshot_export(&mut response, Path::new("/tmp/fp.jsonl"), true, 3);
        let value = response.into_value();

        assert_eq!(value["data"]["snapshotsOut"]["append"], true);
        assert_eq!(value["data"]["snapshotsOut"]["count"], 3);
        assert_eq!(value["data"]["snapshotsOut"]["format"], "ndjson");
    }
}
