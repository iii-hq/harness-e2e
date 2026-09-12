use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;
#[cfg(test)]
use std::fs;

use crate::plans::store::{execution_summary, PlanExecution, PlanStore};

impl PlanStore {
    pub(crate) async fn dashboard_summaries(
        &self,
        native_summaries: &[Value],
    ) -> Result<(Vec<Value>, std::collections::BTreeMap<String, String>)> {
        let mut values = Vec::new();
        let mut children = std::collections::BTreeMap::new();
        #[cfg(not(test))]
        let executions = self.executions().await?;
        #[cfg(test)]
        let executions = {
            let mut values = Vec::new();
            for entry in fs::read_dir(self.root.join("plan-store/executions"))? {
                let path = entry?.path();
                if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                    continue;
                }
                let Some(id) = path.file_stem().and_then(|name| name.to_str()) else {
                    continue;
                };
                match self.read_execution(id).await {
                    Ok(execution) => values.push(execution),
                    Err(error) => {
                        tracing::warn!(path = %path.display(), %error, "ignoring an unsupported or corrupt local E2E plan execution")
                    }
                }
            }
            values
        };
        for execution in executions {
            let id = &execution.id;
            let plan = match self.read_plan(&execution.plan_id).await {
                Ok(plan) => plan,
                Err(error) => {
                    tracing::warn!(execution_id = %id, plan_id = %execution.plan_id, error = %error,
                        "ignoring a local E2E plan execution without a readable plan");
                    continue;
                }
            };
            let summary = execution_summary(&execution);
            for slot in &execution.slots {
                children.insert(slot.execution_id.clone(), execution.id.clone());
            }
            let status = match execution.state.as_str() {
                "completed"
                    if execution
                        .slots
                        .iter()
                        .all(|slot| slot.completed == 1 && slot.technical_valid == 1) =>
                {
                    "passed"
                }
                "completed"
                    if execution
                        .slots
                        .iter()
                        .any(|slot| slot.observed == 1 && slot.technical_valid == 0) =>
                {
                    "technical_failed"
                }
                "completed" | "interrupted" => "incomplete",
                other => other,
            };
            let mut value = json!({"id": execution.id, "label": plan.plan.label, "run_id": execution.id,
                "kind": "plan", "plan_id": plan.plan.id, "template_id": plan.plan.template_id, "plan_execution": summary,
                "attempt": 1, "workflow_name": "Harness plan", "workflow_url": null, "event": "local", "actor": "local",
                "started_at": execution.started_at, "completed_at": execution.finished_at.as_deref().unwrap_or(""), "generated_at": execution.updated_at,
                "status": status, "conclusion": if status == "passed" { "success" } else { "" }, "availability": "available", "lane": plan.snapshot.profile.lane,
                "subjects": [{"id": plan.plan.model, "model": plan.plan.model, "provider": plan.plan.provider, "judge": if plan.plan.judge_model.is_empty() { Value::Null } else { json!({"model": plan.plan.judge_model, "provider": plan.plan.judge_provider}) }, "scenarios": []}],
                "requested_runs": execution.slots.len(), "scenario_metrics": [], "execution": {"id": execution.id},
                "totals": {"expected_reports": execution.slots.len(), "received_reports": summary["observed"], "missing_reports": execution.slots.len() as u64 - summary["observed"].as_u64().unwrap_or(0),
                    "report_coverage": summary["observed"].as_f64().map(|observed| observed / execution.slots.len().max(1) as f64), "passed_scenarios": summary["passed"], "total_tokens": null, "total_cost_usd": null},
                "first_failure": execution.error.as_ref().map(|error| json!({"kind": "plan_execution", "message": error}))});
            project_measurements(&mut value, &execution, native_summaries);
            values.push(value);
        }
        Ok((values, children))
    }
    pub(crate) async fn execution_detail(
        &self,
        id: &str,
        native_summaries: &[Value],
    ) -> Result<Option<Value>> {
        if self.read_execution(id).await.is_err() {
            return Ok(None);
        }
        let (summaries, _) = self.dashboard_summaries(native_summaries).await?;
        let mut summary = summaries
            .into_iter()
            .find(|value| value["id"] == id)
            .context("Plan execution missing")?;
        let execution = self.read_execution(id).await?;
        let mut reports = Vec::new();
        let mut assessments = Vec::new();
        let mut assessed = BTreeSet::new();
        for slot in &execution.slots {
            let native = if slot.observed > 0 {
                super::store::read_stored_run(&self.root.join(&slot.execution_id)).and_then(|run| {
                    run.map(|run| {
                        let detail = super::presenter::stored_execution_detail(&run)?;
                        if assessed.insert(slot.execution_id.clone()) {
                            let report = run.report.context("Native report is unavailable")?;
                            assessments.extend(report.assessment_contract.runs);
                        }
                        Ok(detail)
                    })
                    .transpose()
                })
            } else {
                Ok(None)
            };
            match native {
                Ok(Some(detail)) if detail["reports"].as_array().is_some_and(|reports| !reports.is_empty()) => {
                    for mut report in detail["reports"].as_array().unwrap().iter()
                        .filter(|report| report["scenario_id"] == slot.scenario_id)
                        .cloned() {
                        report["subject_id"] = summary["subjects"][0]["id"].clone();
                        report["native_execution_id"] = json!(slot.execution_id);
                        report["round"] = json!(slot.round);
                        reports.push(report);
                    }
                }
                result => reports.push(json!({
                    "subject_id": summary["subjects"][0]["id"], "scenario_id": slot.scenario_id,
                    "native_execution_id": slot.execution_id, "round": slot.round,
                    "available": false, "report": null,
                    "error": result.err().map(|error| format!("{error:#}")).or_else(|| slot.error.clone()),
                })),
            }
        }
        summary["assessment_summary"] =
            json!(super::assessment_projection::summarize(assessments.iter()));
        summary["reports"] = json!(reports);
        summary["plan_execution"] = serde_json::to_value(&execution)?;
        summary["native_execution_ids"] = json!(execution
            .slots
            .iter()
            .map(|slot| &slot.execution_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>());
        Ok(Some(summary))
    }
}

fn project_measurements(value: &mut Value, execution: &PlanExecution, native_summaries: &[Value]) {
    let Some(cohorts) = execution
        .measurements
        .as_ref()
        .and_then(|m| m["cohorts"].as_array())
    else {
        return;
    };
    let sum =
        |path: &str| -> Option<f64> { cohorts.iter().map(|c| c.pointer(path)?.as_f64()).sum() };
    let completed = sum("/aggregate/completed_runs");
    let tokens = sum("/aggregate/total_tokens_consumed");
    value["totals"]["total_tokens"] = json!(tokens);
    value["totals"]["failed_attempt_tokens"] = json!(sum("/aggregate/failed_attempt_tokens"));
    value["totals"]["total_cost_usd"] = json!(sum("/aggregate/cost/total_usd"));
    value["totals"]["tokens_per_completion"] = json!(tokens
        .zip(completed)
        .filter(|(_, n)| *n > 0.0)
        .map(|(tokens, n)| tokens / n));
    value["totals"]["scenario_pass_rate"] =
        json!(completed.map(|completed| completed / execution.slots.len().max(1) as f64));
    value["totals"]["technical_failures"] = json!(sum("/aggregate/technical_failures"));
    let native = execution
        .slots
        .iter()
        .map(|slot| &slot.execution_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|id| native_summaries.iter().find(|summary| summary["id"] == *id))
        .collect::<Option<Vec<_>>>();
    for key in [
        "wall_time_seconds",
        "function_calls",
        "function_call_errors",
        "turns",
    ] {
        let total: Option<f64> = native
            .as_ref()
            .filter(|runs| !runs.is_empty())
            .and_then(|runs| runs.iter().map(|run| run["totals"][key].as_f64()).sum());
        value["totals"][key] = json!(total);
    }
    let mut scenarios = Vec::new();
    let mut metrics = Vec::new();
    for cohort in cohorts {
        let aggregate = &cohort["aggregate"];
        let id = &cohort["scenario_id"];
        let count = aggregate["observed_runs"].as_f64().unwrap_or(0.0);
        let completed = aggregate["completed_runs"].as_u64().unwrap_or(0);
        let planned = aggregate["planned_runs"].as_u64().unwrap_or(0);
        let technical_failures = aggregate["technical_failures"].as_u64().unwrap_or(0);
        let status = if technical_failures > 0 {
            "technical_failed"
        } else if planned > 0 && completed == planned {
            "passed"
        } else {
            "incomplete"
        };
        scenarios.push(json!({"id": id, "case_id": cohort["identity"]["case"]["case_id"], "runs": count,
            "status": status,
            "passed": status == "passed", "pass_rate": if planned == 0 { 0.0 } else { completed as f64 / planned as f64 },
            "technical_failures": aggregate["technical_failures"],
            "median_score": aggregate["median_score"], "total_cost_usd": aggregate["cost"]["total_usd"]}));
        let contract = json!({"case_id": cohort["identity"]["case"]["case_id"], "case": cohort["identity"]["case"],
            "scenario_id": id, "scenario_version": cohort["identity"]["case"]["scenario_version"], "execution_policy": cohort["identity"]["execution_policy"]});
        metrics.push(json!({"subject_id": value["subjects"][0]["id"], "scenario_id": id,
            "scenario_version": cohort["identity"]["case"]["scenario_version"], "contract_fingerprint": super::presenter::contract_fingerprint(&contract), "run_count": count,
            "averages": {"tokens": aggregate["total_tokens_consumed"].as_f64().filter(|_| count > 0.0).map(|tokens| tokens / count), "tokens_per_completion": aggregate["tokens_per_completion"]},
            "samples": {"tokens": if aggregate["total_tokens_consumed"].is_number() { count } else { 0.0 }, "tokens_per_completion": aggregate["completed_runs"]}}));
    }
    value["subjects"][0]["scenarios"] = json!(scenarios);
    value["scenario_metrics"] = json!(metrics);
    value["baseline_comparable"] = json!(execution.baseline_eligible);
}
