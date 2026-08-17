use std::collections::BTreeMap;
use std::env;
#[cfg(test)]
use std::path::Path;

use anyhow::Result;
use serde_json::{json, Value};

use super::assessment_projection::{
    contracts_for_scenario, project_scenario_report, summarize, AssessmentSummary,
};
#[cfg(test)]
use super::store::load_runs;
use super::{JobStatus, RunMetadata};
use crate::identity::StackIdentity;
use crate::longitudinal::{capability_for_report, CapabilityPolicy};
use crate::report::{E2eReport, E2eRunReport, E2eScenarioReport};
use crate::workflow::{WorkflowStepReport, WorkflowStepStatus};

pub(super) const MAX_EXECUTIONS: usize = 100;

#[cfg(test)]
pub(super) fn load_execution_summaries(runs_dir: &Path) -> Result<Vec<Value>> {
    let mut values = Vec::new();
    for run in load_runs(runs_dir)? {
        values.push(execution_summary(&run.metadata, run.report.as_ref())?);
    }
    values.sort_by(|left, right| {
        right
            .get("started_at")
            .and_then(Value::as_str)
            .cmp(&left.get("started_at").and_then(Value::as_str))
    });
    values.truncate(MAX_EXECUTIONS);
    Ok(values)
}

pub(super) fn execution_summary(
    metadata: &RunMetadata,
    report: Option<&E2eReport>,
) -> Result<Value> {
    let generated_at = if metadata.completed_at.is_empty() {
        &metadata.started_at
    } else {
        &metadata.completed_at
    };
    let execution = execution_identity(metadata, report);
    let Some(report) = report else {
        let status = match metadata.status {
            JobStatus::Cancelled => "cancelled",
            JobStatus::Running | JobStatus::Cancelling => "running",
            JobStatus::Completed => "incomplete",
            JobStatus::Failed => "infra_failed",
        };
        return Ok(json!({
            "id": metadata.id,
            "label": metadata.label,
            "run_id": metadata.id,
            "attempt": 1,
            "workflow_name": "Harness E2E Local",
            "workflow_url": null,
            "event": "local",
            "actor": actor(),
            "started_at": metadata.started_at,
            "completed_at": metadata.completed_at,
            "conclusion": if metadata.status == JobStatus::Failed { "failure" } else { "" },
            "status": status,
            "availability": "unavailable",
            "detail_path": null,
            "generated_at": generated_at,
            "lane": "local",
            "execution": execution,
            "release": release_identity(None),
            "source": source_identity(None),
            "stack": stack_identity(None),
            "requested_runs": metadata.request.runs,
            "subjects": [],
            "scenario_metrics": [],
            "workflow_metrics": Value::Null,
            "assessment_summary": AssessmentSummary::default(),
            "capability": {},
            "totals": {},
            "first_failure": if metadata.error.is_empty() { Value::Null } else { json!({"kind":"runner", "message": metadata.error}) },
        }));
    };

    let subject_id = slug(&format!(
        "{}-{}",
        report.subject.provider, report.subject.model
    ));
    let assessment_summary = summarize(report.assessment_contract.runs.iter());
    let scenarios: Vec<_> = report
        .scenarios
        .iter()
        .map(|scenario| scenario_summary(report, scenario))
        .collect();
    let hard_gate_failures: u32 = report
        .scenarios
        .iter()
        .map(|value| value.aggregate.hard_gate_failures)
        .sum();
    let technical_failures: u32 = report
        .scenarios
        .iter()
        .map(|value| value.aggregate.technical_failures)
        .sum();
    let retries: usize = report
        .scenarios
        .iter()
        .flat_map(|scenario| &scenario.runs)
        .map(|run| run.retry_attempts.len())
        .sum();
    let wall_time_seconds = report
        .scenarios
        .iter()
        .flat_map(|scenario| &scenario.runs)
        .map(|run| run.wall_time_ms as f64 / 1000.0)
        .sum::<f64>();
    let costs: Vec<_> = report
        .scenarios
        .iter()
        .map(|scenario| scenario.aggregate.cost.total_usd)
        .collect();
    let total_cost_usd = sum_complete(&costs);
    let expected = report.scenarios.len();
    let passed = report
        .scenarios
        .iter()
        .filter(|scenario| scenario.passed)
        .count();
    let status = semantic_status(report.passed, hard_gate_failures, technical_failures);
    let totals = efficiency_totals(report);
    let execution_identity = &report.execution;
    let system = &report.system_under_test;
    let engine_revision = system
        .engine_revision
        .as_ref()
        .or(report.engine_revision.as_ref());
    let subject = json!({
        "id": subject_id,
        "model": report.subject.model,
        "provider": report.subject.provider,
        "judge": report.judge,
        "engine_revision": engine_revision,
        "passed": report.passed,
        "expected_reports": expected,
        "received_reports": expected,
        "scenario_pass_rate": if expected == 0 { 0.0 } else { passed as f64 / expected as f64 },
        "report_coverage": 1.0,
        "hard_gate_failures": hard_gate_failures,
        "technical_failures": technical_failures,
        "infra_failures": 0,
        "retry_attempts": retries,
        "total_cost_usd": total_cost_usd,
        "wall_time_seconds": wall_time_seconds,
        "assessment_summary": assessment_summary,
        "scenarios": scenarios,
    });
    Ok(json!({
        "id": metadata.id,
        "label": metadata.label,
        "run_id": execution_identity.execution_id,
        "attempt": 1,
        "workflow_name": "Harness E2E Local",
        "workflow_url": null,
        "event": "local",
        "actor": actor(),
        "started_at": execution_identity.started_at,
        "completed_at": execution_identity.completed_at,
        "conclusion": if hard_gate_failures > 0 || technical_failures > 0 { "failure" } else { "success" },
        "status": status,
        "availability": "full",
        "detail_path": format!("runs/{}.json", metadata.id),
        "generated_at": generated_at,
        "lane": execution_identity.lane,
        "execution": execution,
        "release": release_identity(Some(system)),
        "source": source_identity(Some(system)),
        "stack": stack_identity(Some(system)),
        "requested_runs": metadata.request.runs,
        "subjects": [subject],
        "scenario_metrics": scenario_metrics(&subject_id, report),
        "workflow_metrics": workflow_metric_summary_for_report(report),
        "assessment_summary": assessment_summary,
        "capability": capability_summary(report),
        "totals": {
            "expected_reports": expected,
            "received_reports": expected,
            "report_coverage": 100.0,
            "passed_scenarios": passed,
            "scenario_pass_rate": if expected == 0 { 0.0 } else { passed as f64 / expected as f64 * 100.0 },
            "total_cost_usd": total_cost_usd,
            "wall_time_seconds": wall_time_seconds,
            "hard_gate_failures": hard_gate_failures,
            "technical_failures": technical_failures,
            "missing_reports": 0,
            "retries": retries,
            "total_tokens": totals.0,
            "function_calls": totals.1,
        },
        "workflow_duration_seconds": wall_time_seconds,
        "first_failure": first_failure(report),
    }))
}

pub(super) fn execution_detail_value(metadata: &RunMetadata, report: &E2eReport) -> Result<Value> {
    let summary = execution_summary(metadata, Some(report))?;
    let subject_id = slug(&format!(
        "{}-{}",
        report.subject.provider, report.subject.model
    ));
    let reports: Vec<_> = report
        .scenarios
        .iter()
        .map(|scenario| {
            Ok(json!({
                "subject_id": subject_id,
                "scenario_id": scenario.scenario_id,
                "available": true,
                "report": project_scenario_report(report, scenario)?,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut detail = summary;
    detail["reports"] = json!(reports);
    Ok(detail)
}

fn scenario_summary(report: &E2eReport, scenario: &E2eScenarioReport) -> Value {
    let wall_time_seconds = scenario
        .runs
        .iter()
        .map(|run| run.wall_time_ms as f64 / 1000.0)
        .sum::<f64>();
    let retries: usize = scenario
        .runs
        .iter()
        .map(|run| run.retry_attempts.len())
        .sum();
    json!({
        "id": scenario.scenario_id,
        "case_id": scenario.case_id,
        "complexity_tier": scenario.case.as_ref().map(|case| case.complexity.tier),
        "seed": scenario.case.as_ref().map(|case| case.seed),
        "status": semantic_status(scenario.passed, scenario.aggregate.hard_gate_failures, scenario.aggregate.technical_failures),
        "passed": scenario.passed,
        "runs": scenario.aggregate.runs,
        "median_score": scenario.aggregate.median_score,
        "pass_rate": scenario.aggregate.pass_rate,
        "hard_gate_failures": scenario.aggregate.hard_gate_failures,
        "technical_failures": scenario.aggregate.technical_failures,
        "infra_failures": 0,
        "retries": retries,
        "total_cost_usd": scenario.aggregate.cost.total_usd,
        "wall_time_seconds": wall_time_seconds,
        "robustness": scenario.aggregate.robustness,
        "efficiency": scenario_efficiency(scenario),
        "assessment_summary": summarize(contracts_for_scenario(report, scenario)),
    })
}

fn scenario_efficiency(scenario: &E2eScenarioReport) -> Value {
    let work_amplification = scenario
        .runs
        .iter()
        .filter_map(|run| run.efficiency.as_ref()?.work_amplification)
        .collect::<Vec<_>>();
    let fan_out = scenario
        .runs
        .iter()
        .filter_map(|run| {
            run.efficiency
                .as_ref()?
                .effective_fan_out
                .map(|value| value as f64)
        })
        .collect::<Vec<_>>();
    json!({
        "mean_work_amplification": mean(&work_amplification),
        "mean_effective_fan_out": mean(&fan_out),
    })
}

fn capability_summary(report: &E2eReport) -> Value {
    serde_json::to_value(capability_for_report(report, CapabilityPolicy::default()))
        .expect("serialize capability frontier")
}

fn semantic_status(passed: bool, hard_gates: u32, technical: u32) -> &'static str {
    if technical > 0 {
        "technical_failed"
    } else if hard_gates > 0 {
        "hard_gate_failed"
    } else if passed {
        "passed"
    } else {
        "infra_failed"
    }
}

fn scenario_metrics(subject_id: &str, report: &E2eReport) -> Vec<Value> {
    report
        .scenarios
        .iter()
        .map(|scenario| {
            let metric = |run: &E2eRunReport, name: &str| -> Option<f64> {
                match name {
                    "tokens" => run.metrics.as_ref().and_then(|value| {
                        value
                            .totals
                            .input_tokens
                            .zip(value.totals.output_tokens)
                            .map(|(input, output)| (input + output) as f64)
                    }),
                    "duration_seconds" => Some(run.wall_time_ms as f64 / 1000.0),
                    "cost_usd" => run.cost.total_usd,
                    "function_calls" => run
                        .metrics
                        .as_ref()
                        .map(|value| value.totals.function_calls as f64),
                    "function_call_errors" => run
                        .metrics
                        .as_ref()
                        .map(|value| value.totals.function_call_errors as f64),
                    "sessions" => run
                        .metrics
                        .as_ref()
                        .map(|value| value.totals.sessions as f64),
                    "turns" => run.metrics.as_ref().map(|value| value.totals.turns as f64),
                    "work_amplification" => run
                        .efficiency
                        .as_ref()
                        .and_then(|value| value.work_amplification),
                    "effective_fan_out" => run
                        .efficiency
                        .as_ref()
                        .and_then(|value| value.effective_fan_out)
                        .map(|value| value as f64),
                    _ => None,
                }
            };
            let mut averages = serde_json::Map::new();
            let mut samples = serde_json::Map::new();
            for name in [
                "tokens",
                "duration_seconds",
                "cost_usd",
                "function_calls",
                "function_call_errors",
                "sessions",
                "turns",
                "work_amplification",
                "effective_fan_out",
            ] {
                let values: Vec<_> = scenario
                    .runs
                    .iter()
                    .filter_map(|run| metric(run, name))
                    .collect();
                averages.insert(name.into(), json!(mean(&values)));
                samples.insert(name.into(), json!(values.len()));
            }
            let mut contract = json!({
                "case_id": if scenario.case_id.is_empty() { Value::Null } else { json!(scenario.case_id) },
                "execution_policy": scenario.execution_policy,
                "scenario_id": scenario.scenario_id,
                "scenario_version": scenario.scenario_version,
            });
            if let Some(case) = &scenario.case {
                contract["case"] = json!(case);
            }
            json!({
                "subject_id": subject_id,
                "scenario_id": scenario.scenario_id,
                "scenario_version": scenario.scenario_version,
                "contract_fingerprint": contract_fingerprint(&contract),
                "run_count": scenario.runs.len(),
                "averages": averages,
                "samples": samples,
                "workflow": workflow_metric_summary_for_scenario(scenario),
            })
        })
        .collect()
}

/// Summarize operational metrics emitted by Rust-owned composite scenario
/// steps. These values are intentionally separate from Harness session
/// metrics: a direct worker/GitHub/cron workflow has no model tokens,
/// function-call, or Harness-session counters to report.
fn workflow_metric_summary_for_report(report: &E2eReport) -> Value {
    let tests = report
        .scenarios
        .iter()
        .flat_map(|scenario| scenario.runs.iter())
        .flat_map(|run| run.semantic_tests.iter())
        .collect::<Vec<_>>();
    workflow_metric_summary(&tests)
}

fn workflow_metric_summary_for_scenario(scenario: &E2eScenarioReport) -> Value {
    let tests = scenario
        .runs
        .iter()
        .flat_map(|run| run.semantic_tests.iter())
        .collect::<Vec<_>>();
    workflow_metric_summary(&tests)
}

fn workflow_metric_summary(tests: &[&WorkflowStepReport]) -> Value {
    let mut numeric_metrics = BTreeMap::<String, f64>::new();
    let mut succeeded_steps = 0_u64;
    let mut failed_steps = 0_u64;
    let mut hard_gate_failed_steps = 0_u64;
    let mut skipped_steps = 0_u64;
    let mut cancelled_steps = 0_u64;
    let mut running_steps = 0_u64;
    let mut pending_steps = 0_u64;
    let mut duration_ms = 0_u64;
    let mut asset_count = 0_u64;
    let mut hard_gate_count = 0_u64;
    let mut passed_hard_gate_count = 0_u64;
    let mut evaluation_count = 0_u64;
    let mut failure_count = 0_u64;

    for test in tests {
        match test.status {
            WorkflowStepStatus::Succeeded => succeeded_steps += 1,
            WorkflowStepStatus::Failed => failed_steps += 1,
            WorkflowStepStatus::HardGateFailed => hard_gate_failed_steps += 1,
            WorkflowStepStatus::Skipped => skipped_steps += 1,
            WorkflowStepStatus::Cancelled => cancelled_steps += 1,
            WorkflowStepStatus::Running => running_steps += 1,
            WorkflowStepStatus::Pending => pending_steps += 1,
        }
        duration_ms = duration_ms.saturating_add(test.duration_ms);
        asset_count = asset_count.saturating_add(test.assets.len() as u64);
        hard_gate_count = hard_gate_count.saturating_add(test.hard_gates.len() as u64);
        passed_hard_gate_count = passed_hard_gate_count
            .saturating_add(test.hard_gates.iter().filter(|gate| gate.passed).count() as u64);
        evaluation_count = evaluation_count.saturating_add(test.evaluations.len() as u64);
        failure_count = failure_count.saturating_add(test.failures.len() as u64);
        if let Some(metrics) = &test.metrics {
            collect_numeric_metric_leaves(metrics, "", &mut numeric_metrics);
        }
    }

    json!({
        "step_count": tests.len(),
        "succeeded_steps": succeeded_steps,
        "failed_steps": failed_steps,
        "hard_gate_failed_steps": hard_gate_failed_steps,
        "skipped_steps": skipped_steps,
        "cancelled_steps": cancelled_steps,
        "running_steps": running_steps,
        "pending_steps": pending_steps,
        "duration_ms": duration_ms,
        "asset_count": asset_count,
        "hard_gate_count": hard_gate_count,
        "passed_hard_gate_count": passed_hard_gate_count,
        "evaluation_count": evaluation_count,
        "failure_count": failure_count,
        "numeric_metrics": numeric_metrics,
    })
}

fn collect_numeric_metric_leaves(value: &Value, path: &str, output: &mut BTreeMap<String, f64>) {
    match value {
        Value::Number(number) => {
            if let Some(number) = number.as_f64() {
                let key = if path.is_empty() { "value" } else { path };
                *output.entry(key.to_owned()).or_default() += number;
            }
        }
        Value::Object(object) => {
            for (key, child) in object {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                collect_numeric_metric_leaves(child, &child_path, output);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_numeric_metric_leaves(child, path, output);
            }
        }
        Value::Null | Value::Bool(_) | Value::String(_) => {}
    }
}

pub(super) fn contract_fingerprint(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).expect("serialize scenario contract");
    let hash = bytes.into_iter().fold(2_166_136_261_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    });
    format!("fnv1a32:{hash:08x}")
}

fn efficiency_totals(report: &E2eReport) -> (Option<f64>, Option<f64>) {
    let mut tokens = Vec::new();
    let mut calls = Vec::new();
    for run in report.scenarios.iter().flat_map(|scenario| &scenario.runs) {
        if let Some(metrics) = &run.metrics {
            if let Some((input, output)) = metrics
                .totals
                .input_tokens
                .zip(metrics.totals.output_tokens)
            {
                tokens.push((input + output) as f64);
            }
            calls.push(metrics.totals.function_calls as f64);
        }
    }
    (sum_available(&tokens), sum_available(&calls))
}

fn first_failure(report: &E2eReport) -> Value {
    for scenario in &report.scenarios {
        for run in &scenario.runs {
            if let Some(failure) = run.failures.first() {
                return json!({
                    "kind": "run_failure",
                    "scenario_id": scenario.scenario_id,
                    "domain": failure.domain,
                    "phase": failure.phase,
                    "message": failure.message,
                });
            }
            if let Some(gate) = run.hard_gates.iter().find(|gate| !gate.passed) {
                return json!({
                    "kind": "hard_gate",
                    "scenario_id": scenario.scenario_id,
                    "message": format!("{}: {}", gate.id, gate.reason),
                });
            }
        }
    }
    Value::Null
}

fn execution_identity(metadata: &RunMetadata, report: Option<&E2eReport>) -> Value {
    let report_identity = report.map(|value| &value.execution);
    let system = report.map(|value| &value.system_under_test);
    let (head_sha, repository) = match system.map(|value| &value.stack) {
        Some(StackIdentity::Source {
            workers_repository,
            workers_revision,
        }) => (
            Some(workers_revision.as_str()),
            Some(workers_repository.as_str()),
        ),
        Some(StackIdentity::Registry { .. }) | None => (None, None),
    };
    json!({
        "id": report_identity.map(|value| value.execution_id.as_str()).unwrap_or(metadata.id.as_str()),
        "run_id": report_identity.map(|value| value.execution_id.as_str()).unwrap_or(metadata.id.as_str()),
        "attempt": 1,
        "event": "local",
        "actor": actor(),
        "workflow_name": "Harness E2E Local",
        "workflow_url": null,
        "label": metadata.label,
        "started_at": report_identity.map(|value| value.started_at.as_str()).unwrap_or(metadata.started_at.as_str()),
        "completed_at": report_identity.map(|value| value.completed_at.as_str()).unwrap_or(metadata.completed_at.as_str()),
        "conclusion": if metadata.status == JobStatus::Failed { "failure" } else { "success" },
        "head_sha": head_sha,
        "head_branch": null,
        "repository": repository,
    })
}

fn source_identity(system: Option<&crate::identity::SystemUnderTestIdentity>) -> Value {
    match system.map(|value| &value.stack) {
        Some(StackIdentity::Source {
            workers_repository,
            workers_revision,
        }) => json!({
            "sha": workers_revision,
            "ref": null,
            "repository": workers_repository,
        }),
        Some(StackIdentity::Registry { .. }) | None => json!({
            "sha": null,
            "ref": null,
            "repository": null,
        }),
    }
}

fn release_identity(system: Option<&crate::identity::SystemUnderTestIdentity>) -> Value {
    match system.map(|value| &value.stack) {
        Some(StackIdentity::Registry {
            stack_versions,
            stack_lock_digest,
        }) => json!({
            "tag": null,
            "worker": null,
            "version": null,
            "url": null,
            "registry_tag": null,
            "stack_versions": stack_versions,
            "stack_lock_digest": stack_lock_digest,
        }),
        Some(StackIdentity::Source { .. }) | None => json!({
            "tag": null,
            "worker": null,
            "version": null,
            "url": null,
            "registry_tag": null,
            "stack_versions": null,
            "stack_lock_digest": null,
        }),
    }
}

fn stack_identity(system: Option<&crate::identity::SystemUnderTestIdentity>) -> Value {
    match system.map(|value| &value.stack) {
        Some(StackIdentity::Source { .. }) => json!({
            "mode": "source",
            "versions": null,
            "lock_digest": null,
        }),
        Some(StackIdentity::Registry {
            stack_versions,
            stack_lock_digest,
        }) => json!({
            "mode": "registry",
            "versions": stack_versions,
            "lock_digest": stack_lock_digest,
        }),
        None => json!({
            "mode": null,
            "versions": null,
            "lock_digest": null,
        }),
    }
}

pub(super) fn validate_execution_id(value: &str) -> std::result::Result<(), String> {
    let local_id = value.starts_with("local-")
        && value.len() <= 80
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');
    let control_plane_id = value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    if local_id || control_plane_id {
        Ok(())
    } else {
        Err("invalid execution id".into())
    }
}

fn sum_complete(values: &[Option<f64>]) -> Option<f64> {
    values
        .iter()
        .copied()
        .try_fold(0.0, |total, value| Some(total + value?))
}

fn sum_available(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum())
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn slug(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn actor() -> String {
    env::var("USER").unwrap_or_else(|_| "local".into())
}

pub(super) fn repository_url() -> String {
    "https://github.com/iii-hq/harness-e2e".into()
}
