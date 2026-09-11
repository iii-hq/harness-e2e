use std::collections::BTreeMap;

use anyhow::Result;
use serde_json::{json, Value};

use super::{store::retained_payload, Execution};
use crate::persistence::Persistence;

impl Persistence {
    pub(crate) async fn imported_execution_summaries(&self) -> Result<Vec<Value>> {
        let mut reports: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut runs: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for (table, records) in [
            ("history_reports", &mut reports),
            ("history_runs", &mut runs),
        ] {
            for row in self.query(&format!("SELECT execution_id, payload_json, storage_sha256 AS payload_sha256 FROM {table} ORDER BY source_id"), json!([])).await? {
                records.entry(super::field(&row, "execution_id")?.into()).or_default()
                    .push(retained_payload(&row)?);
            }
        }
        self.imported_execution_records()
            .await?
            .iter()
            .map(|row| {
                let mut metadata = retained_payload(row)?;
                let mut execution: Execution =
                    serde_json::from_value(metadata["execution"].take())?;
                let id = super::field(row, "id")?;
                execution.reports = reports.remove(id).unwrap_or_default();
                execution.runs = runs.remove(id).unwrap_or_default();
                metadata["id"] = row["id"].clone();
                metadata["plan_id"] = row["plan_id"].clone();
                Ok(project_execution(&metadata, &execution))
            })
            .collect()
    }

    pub(crate) async fn imported_execution_detail(&self, id: &str) -> Result<Option<Value>> {
        Ok(self
            .imported_execution(id)
            .await?
            .map(|(metadata, execution)| {
                let mut value = project_execution(&metadata, &execution);
                value["reports"] = json!([]);
                value["retained_reports"] = json!(execution.reports);
                value["retained_runs"] = json!(execution.runs);
                value["materialization"] = json!(execution.materialization);
                value["bundles"] = json!(execution.bundles);
                value
            }))
    }
}

fn complete_sum(runs: &[Value], key: &str) -> Option<f64> {
    if runs.is_empty() {
        return None;
    }
    runs.iter()
        .map(|run| {
            if key != "wallTimeMs" && run["attemptsComplete"] != true {
                return None;
            }
            run[key].as_f64().filter(|v| v.is_finite())
        })
        .sum()
}

fn project_execution(metadata: &Value, execution: &Execution) -> Value {
    let record = &execution.record;
    let config = &record["plan"];
    let subject = &config["subject"];
    let snapshot = execution.materialization.snapshot.as_ref();
    let planned = snapshot.and_then(|s| s["budget"]["planned_runs"].as_u64());
    let completed = execution
        .runs
        .iter()
        .filter(|r| r["completion"] == "completed")
        .count();
    let determined = execution
        .runs
        .iter()
        .filter(|r| {
            matches!(
                r["completion"].as_str(),
                Some("completed" | "task_incomplete")
            )
        })
        .count();
    let valid = execution
        .runs
        .iter()
        .filter(|r| r["technical"] == "valid")
        .count();
    let technical_known = execution
        .runs
        .iter()
        .filter(|r| matches!(r["technical"].as_str(), Some("valid" | "technical_invalid")))
        .count();
    let ratio = |n: usize, d: usize| (d > 0).then(|| n as f64 / d as f64);
    let mut by_scenario: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for run in &execution.runs {
        if let Some(id) = run["scenarioId"].as_str() {
            by_scenario.entry(id).or_default().push(run);
        }
    }
    let scenarios = by_scenario.iter().map(|(id, runs)| json!({
        "id": id, "scenario_version": runs[0]["scenarioVersion"], "case_id": runs[0]["caseId"],
        "runs": runs.len(), "passed": runs.iter().all(|r| r["status"] == "passed"),
        "pass_rate": ratio(runs.iter().filter(|r| r["status"] == "passed").count(), runs.len()),
    })).collect::<Vec<_>>();
    let repository = config["executor"]["repository"].as_str();
    let workflow_url = repository
        .zip(record["ghRunId"].as_u64())
        .map(|(repository, id)| format!("https://github.com/{repository}/actions/runs/{id}"));
    let mut reference_execution = record.clone();
    reference_execution["local_id"] = metadata["id"].clone();
    reference_execution["reportCount"] = json!(execution.reports.len());
    reference_execution["runCount"] = json!(execution.runs.len());
    reference_execution["ghRunUrl"] = json!(workflow_url);
    let materialized = execution
        .reports
        .iter()
        .filter(|r| r["kind"] == "materialized")
        .max_by_key(|r| (r["runAttempt"].as_u64(), r["receivedAt"].as_str()));
    let aggregate = json!({"planned_runs": planned, "observed_runs": execution.runs.len(), "completion_rate": ratio(completed, determined), "execution_reliability": ratio(valid, technical_known)});
    json!({
        "origin": "remote", "id": metadata["id"], "plan_id": metadata["plan_id"], "kind": "history",
        "label": record["label"].as_str().or(config["name"].as_str()).or(record["planKey"].as_str()),
        "run_id": record["id"], "attempt": record["attempt"], "event": "remote", "actor": record["requestedBy"],
        "status": record["phase"], "started_at": record["requestedAt"], "completed_at": record["completedAt"],
        "generated_at": metadata["captured_at"], "workflow_url": workflow_url,
        "availability": if execution.reports.is_empty() { "unavailable" } else { "aggregate" },
        "history_source": {"instance_id": metadata["source"]["instance_id"], "execution_id": record["id"], "captured_at": metadata["captured_at"]},
        "release_control": {"execution_id": record["id"], "attempt": record["attempt"], "profile": record["planKey"], "campaign_id": record["campaignId"], "group_id": null},
        "subjects": if subject["model"].is_string() && subject["provider"].is_string() { json!([{"id": subject["model"], "model": subject["model"], "provider": subject["provider"], "judge": config["judge"], "scenarios": scenarios}]) } else { json!([]) },
        "scenario_metrics": [],
        "totals": {"expected_reports": planned, "received_reports": execution.runs.len(), "missing_reports": planned.map(|n| n.saturating_sub(execution.runs.len() as u64)),
            "total_tokens": complete_sum(&execution.runs, "totalTokens"), "total_cost_usd": complete_sum(&execution.runs, "costSubjectUsd"),
            "wall_time_seconds": complete_sum(&execution.runs, "wallTimeMs").map(|v| v / 1000.0),
            "function_calls": complete_sum(&execution.runs, "functionCalls"), "function_call_errors": complete_sum(&execution.runs, "functionCallErrors"),
            "turns": complete_sum(&execution.runs, "turns"), "report_coverage": planned.filter(|n| *n > 0).map(|n| execution.runs.len() as f64 / n as f64),
            "scenario_pass_rate": ratio(completed, determined)},
        "remote_reference": {"execution": reference_execution, "runs": execution.runs, "aggregate": aggregate,
            "materialized": materialized.map(|r| &r["payload"]),
            "shards": execution.reports.iter().filter(|r| r["kind"] == "shard" && execution.runs.iter().any(|run| run["reportId"] == r["id"])).map(|r| &r["payload"]).collect::<Vec<_>>()},
    })
}
