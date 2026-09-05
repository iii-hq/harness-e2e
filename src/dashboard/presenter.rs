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
use crate::report::{E2eReport, E2eRunReport, E2eScenarioReport, ScenarioAggregate};
use crate::workflow::{WorkflowStepReport, WorkflowStepStatus};

pub(super) const MAX_EXECUTIONS: usize = 100;

pub(super) fn stored_execution_summary(run: &super::store::StoredRun) -> Result<Value> {
    let mut value = execution_summary(&run.metadata, run.report.as_ref())?;
    attach_live_progress(&mut value, run)?;
    attach_report_compatibility(&mut value, run)?;
    Ok(value)
}

pub(super) fn stored_execution_detail(run: &super::store::StoredRun) -> Result<Value> {
    let mut value = execution_detail_value_optional(&run.metadata, run.report.as_ref())?;
    attach_live_progress(&mut value, run)?;
    attach_report_compatibility(&mut value, run)?;
    Ok(value)
}

fn attach_live_progress(value: &mut Value, run: &super::store::StoredRun) -> Result<()> {
    value["live_progress"] = serde_json::to_value(&run.live_progress)?;
    value["live_progress_error"] = json!(run.live_progress_error);
    // A report can be written while final assessments are still running.
    // Keep lifecycle authoritative until the control plane finishes.
    if run.metadata.status.active() {
        value["status"] = json!(if run.metadata.status == JobStatus::Cancelling {
            "cancelling"
        } else {
            "running"
        });
        value["conclusion"] = json!("");
        value["completed_at"] = json!("");
    }
    if let Some(progress) = &run.live_progress {
        value["generated_at"] = json!(progress.updated_at);
    }
    Ok(())
}

fn attach_report_compatibility(value: &mut Value, run: &super::store::StoredRun) -> Result<()> {
    let Some(unsupported) = &run.unsupported_report else {
        return Ok(());
    };
    value["status"] = json!("unsupported");
    value["availability"] = json!("unsupported");
    value["conclusion"] = json!("");
    value["execution"]["conclusion"] = json!("");
    value["baseline_comparable"] = json!(false);
    value["requested_runs"] = Value::Null;
    value["result_compatibility"] = serde_json::to_value(unsupported)?;
    value["first_failure"] = json!({
        "kind": "unsupported_results_schema",
        "message": format!(
            "Results schema v{} is retained as historical evidence, but is incompatible with v{}. Metrics and baseline comparisons are unavailable; no migration or re-execution was performed.",
            unsupported.schema_version, unsupported.expected_schema_version,
        ),
    });
    if !run.metadata.request.model.is_empty() || !run.metadata.request.provider.is_empty() {
        value["subjects"] = json!([{
            "id": slug(&format!("{}-{}", run.metadata.request.provider, run.metadata.request.model)),
            "model": run.metadata.request.model,
            "provider": run.metadata.request.provider,
            "scenarios": [],
        }]);
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn load_execution_summaries(runs_dir: &Path) -> Result<Vec<Value>> {
    let mut values = Vec::new();
    for run in load_runs(runs_dir)? {
        values.push(stored_execution_summary(&run)?);
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
    let status = if report.persistence_errors.is_empty() {
        semantic_status(report.passed, hard_gate_failures, technical_failures)
    } else {
        "incomplete"
    };
    let totals = efficiency_totals(report);
    let aggregates: Vec<&ScenarioAggregate> = report
        .scenarios
        .iter()
        .map(|scenario| &scenario.aggregate)
        .collect();
    // Built apart from the summary literal: `json!` recurses per key and the
    // summary already sits at the macro's recursion limit.
    let execution_totals = json!({
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
        "function_call_errors": totals.2,
        "failed_attempt_tokens": failed_attempt_token_total(&aggregates),
        "tokens_per_completion": pooled_tokens_per_completion(&aggregates),
    });
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
    let mut summary = json!({
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
        "conclusion": if !report.persistence_errors.is_empty() || hard_gate_failures > 0 || technical_failures > 0 { "failure" } else { "success" },
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
        "totals": execution_totals,
        "workflow_duration_seconds": wall_time_seconds,
        "first_failure": first_failure(report),
    });
    summary["persistence_errors"] = json!(report.persistence_errors);
    summary["slot_start_deadline_seconds"] = json!(report.slot_start_deadline_seconds);
    if !report.persistence_errors.is_empty() {
        summary["baseline_comparable"] = json!(false);
    }
    Ok(summary)
}

/// Build the detail payload even when the runner has not persisted a final
/// report yet. Active and cancelled runs are still real executions: the
/// metadata contains their identity and lifecycle state, while `reports` is
/// empty until a report becomes available.
pub(super) fn execution_detail_value_optional(
    metadata: &RunMetadata,
    report: Option<&E2eReport>,
) -> Result<Value> {
    let summary = execution_summary(metadata, report)?;
    let Some(report) = report else {
        let mut detail = summary;
        detail["reports"] = json!([]);
        return Ok(detail);
    };
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
                    "tokens" => run_total_tokens(&scenario.scenario_id, run),
                    "duration_seconds" => Some(run.wall_time_ms as f64 / 1000.0),
                    "cost_usd" => run_total_cost(&scenario.scenario_id, run),
                    "function_calls" => run_function_calls(&scenario.scenario_id, run),
                    "function_call_errors" => {
                        run_function_call_errors(&scenario.scenario_id, run)
                    }
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
            // Retry and completion accounting comes from the scenario aggregate,
            // which already reasons over every attempt; per run it is the mean,
            // so it reads beside `tokens`, and per completion it is the ratio
            // the aggregate publishes.
            let run_count = scenario.runs.len();
            let failed_attempt_tokens = scenario
                .aggregate
                .failed_attempt_tokens
                .filter(|_| run_count > 0)
                .map(|value| value as f64 / run_count as f64);
            averages.insert("failed_attempt_tokens".into(), json!(failed_attempt_tokens));
            samples.insert(
                "failed_attempt_tokens".into(),
                json!(failed_attempt_tokens.map_or(0, |_| run_count)),
            );
            averages.insert(
                "tokens_per_completion".into(),
                json!(scenario.aggregate.tokens_per_completion),
            );
            samples.insert(
                "tokens_per_completion".into(),
                json!(scenario
                    .aggregate
                    .tokens_per_completion
                    .map_or(0, |_| scenario.aggregate.completed_runs)),
            );
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

/// Summarize metrics persisted by composite scenario steps. Workflow usage is
/// derived only from step metrics; direct worker/GitHub/cron steps may
/// legitimately report no model-token or function-call counters.
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
    let mut input_tokens = 0_u64;
    let mut output_tokens = 0_u64;
    let mut total_tokens = 0_u64;
    let mut function_calls = 0_u64;
    let mut function_call_errors = 0_u64;
    let mut input_token_metric_steps = 0_u64;
    let mut output_token_metric_steps = 0_u64;
    let mut token_metric_steps = 0_u64;
    let mut function_call_metric_steps = 0_u64;
    let mut function_call_error_metric_steps = 0_u64;

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
            let usage = workflow_step_usage(metrics);
            if let Some(value) = usage.input_tokens {
                input_tokens = input_tokens.saturating_add(value);
                input_token_metric_steps += 1;
            }
            if let Some(value) = usage.output_tokens {
                output_tokens = output_tokens.saturating_add(value);
                output_token_metric_steps += 1;
            }
            if let Some(value) = usage.total_tokens {
                total_tokens = total_tokens.saturating_add(value);
                token_metric_steps += 1;
            }
            if let Some(value) = usage.function_calls {
                function_calls = function_calls.saturating_add(value);
                function_call_metric_steps += 1;
            }
            if let Some(value) = usage.function_call_errors {
                function_call_errors = function_call_errors.saturating_add(value);
                function_call_error_metric_steps += 1;
            }
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
        "input_tokens": (input_token_metric_steps > 0).then_some(input_tokens),
        "output_tokens": (output_token_metric_steps > 0).then_some(output_tokens),
        "total_tokens": (token_metric_steps > 0).then_some(total_tokens),
        "function_calls": (function_call_metric_steps > 0).then_some(function_calls),
        "function_call_errors": (function_call_error_metric_steps > 0)
            .then_some(function_call_errors),
        "input_token_metric_steps": input_token_metric_steps,
        "output_token_metric_steps": output_token_metric_steps,
        "token_metric_steps": token_metric_steps,
        "function_call_metric_steps": function_call_metric_steps,
        "function_call_error_metric_steps": function_call_error_metric_steps,
        "numeric_metrics": numeric_metrics,
    })
}

#[derive(Debug, PartialEq)]
struct WorkflowStepUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
    function_calls: Option<u64>,
    function_call_errors: Option<u64>,
}

fn workflow_step_usage(metrics: &Value) -> WorkflowStepUsage {
    let input_tokens = first_usage_metric(metrics, &["/totals/input_tokens", "/input_tokens"]);
    let output_tokens = first_usage_metric(metrics, &["/totals/output_tokens", "/output_tokens"]);
    let total_tokens = first_usage_metric(
        metrics,
        &["/totals/total_tokens", "/total_tokens", "/tokens"],
    )
    .or_else(|| {
        input_tokens
            .zip(output_tokens)
            .map(|(input, output)| input.saturating_add(output))
    });
    WorkflowStepUsage {
        input_tokens,
        output_tokens,
        total_tokens,
        function_calls: first_usage_metric(metrics, &["/totals/function_calls", "/function_calls"]),
        function_call_errors: first_usage_metric(
            metrics,
            &["/totals/function_call_errors", "/function_call_errors"],
        ),
    }
}

fn first_usage_metric(metrics: &Value, pointers: &[&str]) -> Option<u64> {
    pointers
        .iter()
        .find_map(|pointer| metrics.pointer(pointer).and_then(Value::as_u64))
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

fn efficiency_totals(report: &E2eReport) -> (Option<f64>, Option<f64>, Option<f64>) {
    let mut tokens = Vec::new();
    let mut calls = Vec::new();
    let mut errors = Vec::new();
    for scenario in &report.scenarios {
        for run in &scenario.runs {
            tokens.push(run_total_tokens(&scenario.scenario_id, run));
            calls.push(run_function_calls(&scenario.scenario_id, run));
            errors.push(run_function_call_errors(&scenario.scenario_id, run));
        }
    }
    (
        sum_complete(&tokens),
        sum_complete(&calls),
        sum_complete(&errors),
    )
}

fn run_total_tokens(scenario_id: &str, run: &E2eRunReport) -> Option<f64> {
    run.metrics
        .as_ref()
        .and_then(|metrics| {
            metrics
                .totals
                .input_tokens
                .zip(metrics.totals.output_tokens)
                .map(|(input, output)| (input + output) as f64)
        })
        .or_else(|| {
            (scenario_id == "security_review").then_some(())?;
            run.judge_usage
                .as_ref()?
                .input_tokens
                .zip(run.judge_usage.as_ref()?.output_tokens)
                .map(|(input, output)| (input + output) as f64)
        })
}

fn run_function_calls(scenario_id: &str, run: &E2eRunReport) -> Option<f64> {
    run.metrics
        .as_ref()
        .map(|metrics| metrics.totals.function_calls as f64)
        .or_else(|| {
            (scenario_id == "security_review").then(|| {
                run.semantic_tests
                    .iter()
                    .map(|test| {
                        let metrics = test.metrics.as_ref();
                        let operations = [
                            metrics.and_then(|value| value.pointer("/request_count")),
                            metrics.and_then(|value| value.pointer("/poll/poll_count")),
                            metrics.and_then(|value| value.pointer("/reconciliation_operations")),
                        ]
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_u64)
                        .sum::<u64>();
                        operations
                            + u64::from(matches!(
                                test.node_id.as_str(),
                                "scan_commit_a" | "list_run_history"
                            ))
                    })
                    .sum::<u64>() as f64
            })
        })
}

fn run_function_call_errors(scenario_id: &str, run: &E2eRunReport) -> Option<f64> {
    run.metrics
        .as_ref()
        .map(|metrics| metrics.totals.function_call_errors as f64)
        .or_else(|| {
            (scenario_id == "security_review").then(|| {
                run.semantic_tests
                    .iter()
                    .map(|test| test.failures.len() as u64)
                    .sum::<u64>() as f64
            })
        })
}

fn run_total_cost(scenario_id: &str, run: &E2eRunReport) -> Option<f64> {
    run.cost
        .total_usd
        .or_else(|| (scenario_id == "security_review").then_some(0.0))
}

fn first_failure(report: &E2eReport) -> Value {
    if let Some(error) = report.persistence_errors.first() {
        return json!({"kind": "persistence", "message": error});
    }
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
        "conclusion": if metadata.status == JobStatus::Failed || report.is_some_and(|value| !value.persistence_errors.is_empty()) { "failure" } else { "success" },
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
    let native_or_plan_id = value.strip_prefix("plan-").unwrap_or(value);
    let control_plane_id = native_or_plan_id.len() == 32
        && native_or_plan_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit());
    if local_id || control_plane_id {
        Ok(())
    } else {
        Err("invalid execution id".into())
    }
}

/// Failed-attempt tokens pooled across scenarios. `None` as soon as one
/// scenario could not account for them: a partial sum is not a total.
fn failed_attempt_token_total(aggregates: &[&ScenarioAggregate]) -> Option<u64> {
    complete_sum_u64(
        aggregates
            .iter()
            .map(|aggregate| aggregate.failed_attempt_tokens),
    )
}

/// Subject tokens over completed logical runs, pooled across scenarios rather
/// than averaged from per-scenario ratios. `None` without complete token
/// accounting or without a completed run.
fn pooled_tokens_per_completion(aggregates: &[&ScenarioAggregate]) -> Option<f64> {
    tokens_per_completion(
        aggregates
            .iter()
            .map(|aggregate| (aggregate.total_tokens_consumed, aggregate.completed_runs)),
    )
}

fn complete_sum_u64(values: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    let mut total = 0_u64;
    let mut seen = false;
    for value in values {
        total = total.checked_add(value?)?;
        seen = true;
    }
    seen.then_some(total)
}

fn tokens_per_completion(scenarios: impl Iterator<Item = (Option<u64>, u32)>) -> Option<f64> {
    let mut tokens = 0_u64;
    let mut completed = 0_u32;
    for (scenario_tokens, scenario_completed) in scenarios {
        tokens = tokens.checked_add(scenario_tokens?)?;
        completed = completed.saturating_add(scenario_completed);
    }
    (completed > 0).then(|| tokens as f64 / completed as f64)
}

fn sum_complete(values: &[Option<f64>]) -> Option<f64> {
    (!values.is_empty())
        .then(|| {
            values
                .iter()
                .copied()
                .try_fold(0.0, |total, value| Some(total + value?))
        })
        .flatten()
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

#[cfg(test)]
mod workflow_usage_tests {
    use super::*;

    #[test]
    fn reads_canonical_step_usage_and_derives_total_tokens() {
        let usage = workflow_step_usage(&json!({
            "totals": {
                "input_tokens": 1_200,
                "output_tokens": 300,
                "function_calls": 4,
                "function_call_errors": 1,
            }
        }));

        assert_eq!(
            usage,
            WorkflowStepUsage {
                input_tokens: Some(1_200),
                output_tokens: Some(300),
                total_tokens: Some(1_500),
                function_calls: Some(4),
                function_call_errors: Some(1),
            }
        );
    }

    #[test]
    fn pools_failed_attempt_tokens_only_when_every_scenario_accounts_for_them() {
        assert_eq!(
            complete_sum_u64([Some(1_500), Some(0), Some(250)].into_iter()),
            Some(1_750)
        );
        assert_eq!(
            complete_sum_u64([Some(1_500), None].into_iter()),
            None,
            "a partial sum is not a total"
        );
        assert_eq!(complete_sum_u64(std::iter::empty()), None);
    }

    #[test]
    fn pools_tokens_per_completion_over_completed_runs_not_per_scenario_ratios() {
        // 6,000 tokens over 3 completed runs = 2,000, not the mean of the
        // per-scenario ratios (4,000 / 1 and 2,000 / 2 would average 3,000).
        assert_eq!(
            tokens_per_completion([(Some(4_000), 1), (Some(2_000), 2)].into_iter()),
            Some(2_000.0)
        );
        assert_eq!(
            tokens_per_completion([(Some(4_000), 1), (None, 2)].into_iter()),
            None
        );
        assert_eq!(tokens_per_completion([(Some(4_000), 0)].into_iter()), None);
    }

    #[test]
    fn does_not_present_a_partial_token_pair_as_a_total() {
        let usage = workflow_step_usage(&json!({
            "totals": { "input_tokens": 1_200 }
        }));

        assert_eq!(usage.input_tokens, Some(1_200));
        assert_eq!(usage.total_tokens, None);
    }
}
