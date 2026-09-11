use anyhow::{ensure, Context, Result};
use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use super::{field, local_id, timestamp, Execution, HistoryImport};
use crate::{artifact, persistence::Persistence};

pub(crate) const SQL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS history_campaigns (source_instance TEXT NOT NULL, source_id TEXT NOT NULL, plan_id TEXT NOT NULL, source_updated_at TEXT NOT NULL, payload_json TEXT NOT NULL, PRIMARY KEY(source_instance, source_id))",
    "CREATE TABLE IF NOT EXISTS history_reports (source_instance TEXT NOT NULL, source_id TEXT NOT NULL, execution_id TEXT NOT NULL, payload_sha256 TEXT NOT NULL, storage_sha256 TEXT NOT NULL, payload_json TEXT NOT NULL, PRIMARY KEY(source_instance, source_id))",
    "CREATE INDEX IF NOT EXISTS history_reports_execution_idx ON history_reports(execution_id)",
    "CREATE TABLE IF NOT EXISTS history_runs (source_instance TEXT NOT NULL, source_id TEXT NOT NULL, execution_id TEXT NOT NULL, source_updated_at TEXT NOT NULL, storage_sha256 TEXT NOT NULL, payload_json TEXT NOT NULL, PRIMARY KEY(source_instance, source_id))",
    "CREATE INDEX IF NOT EXISTS history_runs_execution_idx ON history_runs(execution_id)",
];

impl Persistence {
    pub async fn import_history(&self, input: HistoryImport) -> Result<Value> {
        let history = input.decode()?;
        let source = &history.source.instance_id;
        let plan_id = history.local_plan_id();
        let mut statements = vec![
            json!({"sql": "CREATE TEMP TABLE history_import_check (valid INTEGER NOT NULL CHECK(valid = 1))", "params": []}),
        ];
        let mut count_steps = Vec::new();
        let plan = json!({"plan": history.plan, "source": history.source, "captured_at": history.captured_at});
        let plan_json = serde_json::to_string(&plan)?;
        let plan_updated = normalized_time(&history.plan.source_updated_at)?;
        count_steps.push(statements.len());
        statements.push(change_count("saved_plans", &plan_id, &plan_updated));
        statements.push(capture_check(
            "saved_plans",
            &plan_id,
            source,
            &history.plan.key,
            &plan_updated,
            &history.plan.content_sha256,
        ));
        statements.push(json!({
                "sql": "INSERT INTO saved_plans(id, origin, source_instance, source_id, source_updated_at, source_content_sha256, updated_at, payload_json, payload_sha256) VALUES (?, 'remote', ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET source_updated_at=excluded.source_updated_at, source_content_sha256=excluded.source_content_sha256, updated_at=excluded.updated_at, payload_json=excluded.payload_json, payload_sha256=excluded.payload_sha256 WHERE excluded.source_updated_at > saved_plans.source_updated_at",
            "params": [plan_id, source, history.plan.key, plan_updated, history.plan.content_sha256, history.captured_at, plan_json, artifact::sha256_bytes(plan_json.as_bytes())],
        }));
        for campaign in &history.campaigns {
            statements.push(json!({
                "sql": "INSERT INTO history_import_check(valid) SELECT CASE WHEN NOT EXISTS (SELECT 1 FROM history_campaigns WHERE source_instance = ? AND source_id = ? AND (plan_id != ? OR (source_updated_at = ? AND payload_json != ?))) THEN 1 ELSE 0 END",
                "params": [source, campaign["id"], plan_id, normalized_time(field(campaign, "updatedAt")?)?, serde_json::to_string(campaign)?],
            }));
            statements.push(json!({
                "sql": "INSERT INTO history_campaigns(source_instance, source_id, plan_id, source_updated_at, payload_json) VALUES (?, ?, ?, ?, ?) ON CONFLICT(source_instance, source_id) DO UPDATE SET source_updated_at=excluded.source_updated_at, payload_json=excluded.payload_json WHERE excluded.source_updated_at > history_campaigns.source_updated_at",
                "params": [source, campaign["id"], plan_id, normalized_time(field(campaign, "updatedAt")?)?, serde_json::to_string(campaign)?],
            }));
        }
        for execution in &history.executions {
            let source_id = field(&execution.record, "id")?;
            let id = local_id("execution", source, source_id);
            let updated = normalized_time(&execution.source_updated_at)?;
            let body = serde_json::to_string(
                &json!({"execution": execution, "source": history.source, "captured_at": history.captured_at}),
            )?;
            count_steps.push(statements.len());
            statements.push(change_count("saved_plan_executions", &id, &updated));
            statements.push(capture_check(
                "saved_plan_executions",
                &id,
                source,
                source_id,
                &updated,
                &execution.content_sha256,
            ));
            statements.push(json!({
                "sql": "INSERT INTO history_import_check(valid) SELECT CASE WHEN NOT EXISTS (SELECT 1 FROM saved_plan_executions WHERE id = ? AND (plan_id != ? OR json_quote(json_extract(payload_json, '$.execution.record.plan')) IS NOT json(?))) THEN 1 ELSE 0 END",
                "params": [id, plan_id, serde_json::to_string(&execution.record["plan"])?],
            }));
            for report in &execution.reports {
                let payload = serde_json::to_string(report)?;
                statements.push(json!({
                    "sql": "INSERT INTO history_reports(source_instance, source_id, execution_id, payload_sha256, storage_sha256, payload_json) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(source_instance, source_id) DO UPDATE SET payload_sha256 = CASE WHEN history_reports.execution_id = excluded.execution_id AND history_reports.payload_sha256 = excluded.payload_sha256 AND history_reports.storage_sha256 = excluded.storage_sha256 THEN history_reports.payload_sha256 ELSE NULL END WHERE history_reports.execution_id != excluded.execution_id OR history_reports.payload_sha256 != excluded.payload_sha256 OR history_reports.storage_sha256 != excluded.storage_sha256",
                    "params": [source, report["id"], id, report["payloadSha256"], artifact::sha256_bytes(payload.as_bytes()), payload],
                }));
            }
            for run in &execution.runs {
                let payload = serde_json::to_string(run)?;
                statements.push(json!({
                    "sql": "INSERT INTO history_import_check(valid) SELECT CASE WHEN NOT EXISTS (SELECT 1 FROM history_runs WHERE source_instance = ? AND source_id = ? AND (execution_id != ? OR (source_updated_at = ? AND storage_sha256 != ?))) THEN 1 ELSE 0 END",
                    "params": [source, run["id"], id, normalized_time(field(run, "capturedAt")?)?, artifact::sha256_bytes(payload.as_bytes())],
                }));
                statements.push(json!({
                    "sql": "INSERT INTO history_runs(source_instance, source_id, execution_id, source_updated_at, storage_sha256, payload_json) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(source_instance, source_id) DO UPDATE SET source_updated_at=excluded.source_updated_at, storage_sha256=excluded.storage_sha256, payload_json=excluded.payload_json WHERE excluded.source_updated_at > history_runs.source_updated_at",
                    "params": [source, run["id"], id, normalized_time(field(run, "capturedAt")?)?, artifact::sha256_bytes(payload.as_bytes()), payload],
                }));
            }
            statements.push(json!({
                "sql": "INSERT INTO saved_plan_executions(id, plan_id, origin, idempotency_key, state, started_at, updated_at, payload_json, payload_sha256, source_instance, source_id, source_updated_at, source_content_sha256) VALUES (?, ?, 'remote', NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET state=excluded.state, updated_at=excluded.updated_at, payload_json=excluded.payload_json, payload_sha256=excluded.payload_sha256, source_updated_at=excluded.source_updated_at, source_content_sha256=excluded.source_content_sha256 WHERE excluded.source_updated_at > saved_plan_executions.source_updated_at",
                "params": [id, plan_id, execution.record["phase"], execution.record["requestedAt"], history.captured_at, body, artifact::sha256_bytes(body.as_bytes()), source, source_id, updated, execution.content_sha256],
            }));
        }
        statements.push(json!({"sql": "DROP TABLE history_import_check", "params": []}));
        let results = self.transaction_results(statements).await.context("import history atomically; a conflicting source identity, capture or immutable message leaves the previous history unchanged")?;
        let mut counts = [0_u64; 3];
        for index in count_steps {
            let kind = results[index]["rows"][0][0]
                .as_u64()
                .context("History transaction did not return import counts")?
                as usize;
            ensure!(kind < counts.len(), "Invalid history import count");
            counts[kind] += 1;
        }
        Ok(
            json!({"plan_id": plan_id, "inserted": counts[0], "updated": counts[1], "unchanged": counts[2], "reports": history.counts.reports, "runs": history.counts.runs}),
        )
    }

    pub(crate) async fn imported_plans(&self) -> Result<Vec<Value>> {
        let rows = self.query("SELECT id, payload_json, payload_sha256 FROM saved_plans WHERE origin = 'remote' ORDER BY updated_at DESC, id DESC", json!([])).await?;
        let mut plans = Vec::new();
        for row in rows {
            plans.push(self.imported_plan_view(&row).await?);
        }
        Ok(plans)
    }

    pub(crate) async fn imported_plan(&self, id: &str) -> Result<Option<Value>> {
        let rows = self.query("SELECT id, payload_json, payload_sha256 FROM saved_plans WHERE id = ? AND origin = 'remote'", json!([id])).await?;
        match rows.first() {
            Some(row) => self.imported_plan_view(row).await.map(Some),
            None => Ok(None),
        }
    }

    async fn imported_plan_view(&self, row: &Value) -> Result<Value> {
        let value = retained_payload(row)?;
        let plan = &value["plan"];
        let config = &plan["configuration"];
        let executions = self.query("SELECT id, started_at, updated_at FROM saved_plan_executions WHERE plan_id = ? AND origin = 'remote' ORDER BY started_at DESC, id DESC", json!([row["id"]])).await?;
        Ok(json!({
            "origin": "remote", "id": row["id"],
            "label": config["name"].as_str().unwrap_or(plan["key"].as_str().unwrap_or("")),
            "purpose": config["description"].as_str().unwrap_or(""),
            "created_at": executions.last().map(|e| &e["started_at"]),
            "updated_at": executions.iter().filter_map(|execution| execution["updated_at"].as_str()).max().unwrap_or_else(|| plan["source_updated_at"].as_str().unwrap_or_default()),
            "template_id": config["profile"].as_str(), "configuration": config,
            "source": {"instance_id": value["source"]["instance_id"], "plan_key": plan["key"], "captured_at": value["captured_at"], "active": plan["active"], "limitation": plan["limitation"]},
            "execution_ids": executions.iter().map(|e| &e["id"]).collect::<Vec<_>>()
        }))
    }

    pub(crate) async fn imported_execution_records(&self) -> Result<Vec<Value>> {
        self.query("SELECT id, plan_id, payload_json, payload_sha256 FROM saved_plan_executions WHERE origin = 'remote' ORDER BY started_at DESC, id DESC", json!([])).await
    }

    pub(crate) async fn imported_execution(&self, id: &str) -> Result<Option<(Value, Execution)>> {
        let rows = self.query("SELECT id, plan_id, payload_json, payload_sha256 FROM saved_plan_executions WHERE id = ? AND origin = 'remote'", json!([id])).await?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let mut value = retained_payload(row)?;
        let mut execution: Execution = serde_json::from_value(value["execution"].take())?;
        for (table, destination) in [
            ("history_reports", &mut execution.reports),
            ("history_runs", &mut execution.runs),
        ] {
            *destination = self.query(&format!("SELECT payload_json, storage_sha256 AS payload_sha256 FROM {table} WHERE execution_id = ? ORDER BY source_id"), json!([id])).await?
                .iter().map(retained_payload).collect::<Result<_>>()?;
        }
        value["id"] = row["id"].clone();
        value["plan_id"] = row["plan_id"].clone();
        Ok(Some((value, execution)))
    }
}

pub(crate) fn retained_payload(row: &Value) -> Result<Value> {
    let body = row["payload_json"]
        .as_str()
        .context("Missing retained history payload")?;
    ensure!(
        row["payload_sha256"] == artifact::sha256_bytes(body.as_bytes()),
        "Retained history payload checksum mismatch"
    );
    serde_json::from_str(body).context("decode retained history payload")
}

fn normalized_time(value: &str) -> Result<String> {
    Ok(timestamp(value)?
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Nanos, true))
}

fn change_count(table: &str, id: &str, updated: &str) -> Value {
    json!({
        "sql": format!("SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM {table} WHERE id = ?) THEN 0 WHEN EXISTS(SELECT 1 FROM {table} WHERE id = ? AND source_updated_at < ?) THEN 1 ELSE 2 END"),
        "params": [id, id, updated],
    })
}

fn capture_check(
    table: &str,
    id: &str,
    source: &str,
    source_id: &str,
    updated: &str,
    hash: &str,
) -> Value {
    json!({
        "sql": format!("INSERT INTO history_import_check(valid) SELECT CASE WHEN NOT EXISTS (SELECT 1 FROM {table} WHERE id = ? AND (origin != 'remote' OR source_instance != ? OR source_id != ? OR (source_updated_at = ? AND source_content_sha256 != ?))) THEN 1 ELSE 0 END"),
        "params": [id, source, source_id, updated, hash],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transport(history: &super::super::History) -> HistoryImport {
        let json = serde_json::to_string(history).unwrap();
        HistoryImport {
            sha256: artifact::sha256_bytes(json.as_bytes()),
            json,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires HARNESS_E2E_TEST_DATABASE_URL pointing at an isolated database worker"]
    async fn real_database_import_preserves_history_and_rolls_back_conflicts() {
        let url = std::env::var("HARNESS_E2E_TEST_DATABASE_URL").unwrap();
        let client = iii_sdk::register_worker(&url, iii_sdk::InitOptions::default());
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while client.get_connection_state() != iii_sdk::runtime::IIIConnectionState::Connected {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let db = Persistence::new(client.clone(), "harness_e2e".into(), "default".into());
        db.initialize().await.unwrap();
        let input: HistoryImport = serde_json::from_str(include_str!(
            "../../tests/fixtures/history/retained-history.json"
        ))
        .unwrap();
        let mut original = input.decode().unwrap();
        original.source.instance_id = format!("test-{}", uuid::Uuid::new_v4());
        let imported = db.import_history(transport(&original)).await.unwrap();
        assert_eq!(imported["inserted"], 52);
        assert_eq!(
            db.import_history(transport(&original)).await.unwrap()["unchanged"],
            52
        );
        let plan_id = original.local_plan_id();
        let execution_id = local_id("execution", &original.source.instance_id, "execution-0");
        let retained_capture = db
            .imported_execution(&execution_id)
            .await
            .unwrap()
            .unwrap()
            .0["captured_at"]
            .clone();
        let mut recaptured = original.clone();
        recaptured.captured_at = "2026-09-11T12:30:00Z".into();
        assert_eq!(
            db.import_history(transport(&recaptured)).await.unwrap()["unchanged"],
            52
        );
        assert_eq!(
            db.imported_execution(&execution_id)
                .await
                .unwrap()
                .unwrap()
                .0["captured_at"],
            retained_capture
        );
        let (first, second) = tokio::join!(
            db.import_history(transport(&original)),
            db.import_history(transport(&original)),
        );
        assert_eq!(first.unwrap()["unchanged"], 52);
        assert_eq!(second.unwrap()["unchanged"], 52);
        assert_eq!(
            db.imported_plan(&plan_id).await.unwrap().unwrap()["execution_ids"]
                .as_array()
                .unwrap()
                .len(),
            51
        );
        assert!(db.execution(&execution_id).await.unwrap().is_none());
        let mut updated = original.clone();
        updated.captured_at = "2026-09-11T13:00:00Z".into();
        updated.executions[0].source_updated_at = updated.captured_at.clone();
        updated.executions[0].record["phase"] = json!("running");
        updated.executions[0].record["terminal"] = json!(false);
        updated.executions[0].content_sha256 = "b".repeat(64);
        assert_eq!(
            db.import_history(transport(&updated)).await.unwrap()["updated"],
            1
        );
        db.import_history(transport(&original)).await.unwrap();
        assert_eq!(
            db.imported_execution(&execution_id)
                .await
                .unwrap()
                .unwrap()
                .1
                .record["phase"],
            "running"
        );
        assert!(db
            .active_executions()
            .await
            .unwrap()
            .iter()
            .all(|r| r.execution_id != execution_id));
        let mut omitted = updated.clone();
        omitted.captured_at = "2026-09-11T14:00:00Z".into();
        for execution in &mut omitted.executions {
            execution.reports.clear();
            execution.runs.clear();
        }
        omitted.executions[0].source_updated_at = omitted.captured_at.clone();
        omitted.executions[0].content_sha256 = "d".repeat(64);
        omitted.counts.reports = 0;
        omitted.counts.runs = 0;
        assert_eq!(
            db.import_history(transport(&omitted)).await.unwrap()["updated"],
            1
        );
        let detail = db
            .imported_execution_detail(&execution_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail["retained_reports"].as_array().unwrap().len(), 3);
        assert_eq!(detail["retained_runs"].as_array().unwrap().len(), 1);
        assert_eq!(
            db.imported_execution_summaries()
                .await
                .unwrap()
                .iter()
                .filter(|e| e["plan_id"] == plan_id)
                .count(),
            51
        );
        let mut equal_revision_conflict = omitted.clone();
        equal_revision_conflict.executions[0].content_sha256 = "e".repeat(64);
        assert!(db
            .import_history(transport(&equal_revision_conflict))
            .await
            .is_err());
        let mut conflict = updated.clone();
        conflict.executions[0].reports[0]["payloadSha256"] = json!("c".repeat(64));
        let mut new_execution = original.executions[50].clone();
        new_execution.record["id"] = json!("must-roll-back");
        new_execution.record["attempt"] = json!(52);
        conflict.executions.insert(0, new_execution);
        conflict.counts.executions += 1;
        assert!(db.import_history(transport(&conflict)).await.is_err());
        let rolled_back = local_id("execution", &original.source.instance_id, "must-roll-back");
        assert!(db.imported_execution(&rolled_back).await.unwrap().is_none());
        let restored = Persistence::new(client.clone(), "harness_e2e".into(), "default".into());
        let (_, retained) = restored
            .imported_execution(&execution_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.record["phase"], "running");
        assert_eq!(retained.reports.len(), 3);
        assert_eq!(retained.runs[0]["record"]["objective_score"], 0.125);
        assert!(retained.runs[0]["record"]["efficiency"].is_null());
        client.shutdown_async().await;
    }
}
