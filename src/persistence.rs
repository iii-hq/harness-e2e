//! Durable, queryable control-plane state.
//!
//! The database worker owns SQL access.  The runner deliberately owns neither
//! a driver nor a connection string: it only sends parameterised statements to
//! the dedicated control-plane database namespace.
use std::path::Path;

use anyhow::{bail, Context, Result};
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::IIIClient;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::control::ExecutionRecord;
use crate::plans::store::{PlanExecution, SavedPlan};
use crate::report::{E2eRunReport, E2eScenarioReport};

const DATABASE_QUERY: &str = "database::query";
const DATABASE_TRANSACTION: &str = "database::transaction";

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS harness_e2e_schema (version INTEGER PRIMARY KEY CHECK (version = 3))",
    "CREATE TABLE IF NOT EXISTS executions (execution_id TEXT PRIMARY KEY, idempotency_key TEXT NOT NULL UNIQUE, phase TEXT NOT NULL, requested_at TEXT NOT NULL, updated_at TEXT NOT NULL, lane TEXT NOT NULL, subject_provider TEXT NOT NULL, subject_model TEXT NOT NULL, terminal INTEGER NOT NULL CHECK (terminal IN (0, 1)), result_path TEXT NULL, result_sha256 TEXT NULL, record_json TEXT NOT NULL, record_sha256 TEXT NOT NULL)",
    "CREATE INDEX IF NOT EXISTS executions_requested_at_idx ON executions(requested_at DESC)",
    "CREATE INDEX IF NOT EXISTS executions_phase_idx ON executions(terminal, updated_at)",
    "CREATE TABLE IF NOT EXISTS attempts (execution_id TEXT NOT NULL, attempt_id TEXT NOT NULL, run_id TEXT NOT NULL, session_id TEXT NULL, phase TEXT NOT NULL, started_at TEXT NOT NULL, completed_at TEXT NULL, resume_state_path TEXT NULL, resume_state_sha256 TEXT NULL, failure_reason TEXT NULL, payload_json TEXT NOT NULL, PRIMARY KEY (execution_id, attempt_id))",
    "CREATE TABLE IF NOT EXISTS runs (execution_id TEXT NOT NULL, run_id TEXT NOT NULL, scenario_id TEXT NOT NULL, scenario_version INTEGER NULL, case_id TEXT NULL, seed TEXT NULL, selected_attempt_id TEXT NULL, status TEXT NULL, completion TEXT NULL, technical TEXT NULL, objective_score REAL NULL, quality_score_completed REAL NULL, wall_time_ms INTEGER NULL, total_tokens INTEGER NULL, cost_total_usd REAL NULL, function_calls INTEGER NULL, function_call_errors INTEGER NULL, technical_attempts INTEGER NOT NULL DEFAULT 0, telemetry_json TEXT NOT NULL DEFAULT '{}', payload_json TEXT NOT NULL, PRIMARY KEY (execution_id, run_id))",
    "CREATE INDEX IF NOT EXISTS runs_execution_scenario_idx ON runs(execution_id, scenario_id, case_id)",
    "CREATE TABLE IF NOT EXISTS artifacts (execution_id TEXT NOT NULL, artifact_id TEXT NOT NULL, kind TEXT NOT NULL, relative_path TEXT NOT NULL, sha256 TEXT NOT NULL, size_bytes INTEGER NOT NULL, media_type TEXT NOT NULL, available INTEGER NOT NULL CHECK (available IN (0, 1)), archive_uri TEXT NULL, PRIMARY KEY (execution_id, artifact_id, sha256))",
    "CREATE TABLE IF NOT EXISTS archives (execution_id TEXT PRIMARY KEY, archive_id TEXT NOT NULL UNIQUE, manifest_uri TEXT NOT NULL, manifest_sha256 TEXT NOT NULL, expires_at TEXT NULL, payload_json TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS local_scenarios (scenario_id TEXT PRIMARY KEY, source_sha256 TEXT NOT NULL, source_path TEXT NOT NULL, created_at TEXT NOT NULL, payload_json TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS saved_plans (id TEXT PRIMARY KEY, origin TEXT NOT NULL CHECK (origin IN ('local', 'remote')), source_instance TEXT NULL, source_id TEXT NULL, source_updated_at TEXT NULL, source_content_sha256 TEXT NULL, updated_at TEXT NOT NULL, payload_json TEXT NOT NULL, payload_sha256 TEXT NOT NULL, UNIQUE(source_instance, source_id))",
    "CREATE INDEX IF NOT EXISTS saved_plans_origin_updated_idx ON saved_plans(origin, updated_at DESC)",
    "CREATE TABLE IF NOT EXISTS saved_plan_executions (id TEXT PRIMARY KEY, plan_id TEXT NOT NULL, origin TEXT NOT NULL CHECK (origin IN ('local', 'remote')), source_instance TEXT NULL, source_id TEXT NULL, source_updated_at TEXT NULL, source_content_sha256 TEXT NULL, idempotency_key TEXT NULL UNIQUE, state TEXT NOT NULL, started_at TEXT NOT NULL, updated_at TEXT NOT NULL, payload_json TEXT NOT NULL, payload_sha256 TEXT NOT NULL, UNIQUE(source_instance, source_id))",
    "CREATE INDEX IF NOT EXISTS saved_plan_executions_plan_started_idx ON saved_plan_executions(plan_id, started_at DESC)",
];

#[derive(Clone)]
pub struct Persistence {
    iii: IIIClient,
    database: String,
    namespace: String,
}

impl Persistence {
    pub fn new(iii: IIIClient, database: String, namespace: String) -> Self {
        Self {
            iii,
            database,
            namespace,
        }
    }

    pub async fn initialize(&self) -> Result<()> {
        let existing = self
            .query("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'harness_e2e_schema'", json!([]))
            .await?;
        if !existing.is_empty() {
            let versions = self
                .query("SELECT version FROM harness_e2e_schema", json!([]))
                .await?;
            if versions.len() != 1 || versions[0].get("version").and_then(Value::as_i64) != Some(3)
            {
                bail!("incompatible Harness E2E persistence schema; stop the E2E worker and run `harness-e2e migrate-storage --config <worker config>` before starting this version")
            }
        }
        let statements = SCHEMA
            .iter()
            .chain(crate::history::store::SQL.iter())
            .map(|sql| json!({"sql": sql, "params": []}))
            .chain(std::iter::once(json!({
                "sql": "INSERT INTO harness_e2e_schema(version) SELECT 3 WHERE NOT EXISTS (SELECT 1 FROM harness_e2e_schema)",
                "params": []
            })))
            .collect();
        self.transaction(statements).await?;
        let versions = self
            .query("SELECT version FROM harness_e2e_schema", json!([]))
            .await?;
        if versions.len() != 1 || versions[0].get("version").and_then(Value::as_i64) != Some(3) {
            bail!("incompatible Harness E2E persistence schema; stop the E2E worker and run `harness-e2e migrate-storage --config <worker config>` before starting this version")
        }
        Ok(())
    }

    /// Run with the E2E worker stopped. The database worker must remain available.
    pub async fn migrate_storage(&self, output_root: &Path, apply: bool) -> Result<Value> {
        let versions = self
            .query("SELECT version FROM harness_e2e_schema", json!([]))
            .await?;
        if versions.len() != 1 {
            bail!("expected one Harness E2E storage version");
        }
        match versions[0]["version"].as_u64() {
            Some(3) => return Ok(json!({"version": 3, "migrated": 0, "already_current": true})),
            Some(1 | 2) => {}
            _ => bail!("unsupported Harness E2E storage version"),
        }
        if self.active_count().await? > 0 {
            bail!("finish or cancel active executions with the previous worker before migrating");
        }
        let tables = self.query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN ('plans', 'plan_executions', 'plan_slots')",
            json!([]),
        ).await?;
        for table in ["plans", "plan_executions", "plan_slots"] {
            if tables.iter().any(|row| row["name"] == table) {
                let count = self
                    .query(&format!("SELECT COUNT(*) AS count FROM {table}"), json!([]))
                    .await?;
                if count.first().and_then(|row| row["count"].as_u64()) != Some(0) {
                    bail!("obsolete SQL table {table} contains data; migrate it to PlanStore before cutover");
                }
            }
        }
        let records = self.executions().await?;
        let (plans, plan_executions) = load_plan_store(output_root)?;
        let mut statements: Vec<Value> = SCHEMA
            .iter()
            .skip(1)
            .chain(crate::history::store::SQL.iter())
            .map(|sql| json!({"sql": sql, "params": []}))
            .collect();
        let mut unavailable = Vec::new();
        for mut record in records.iter().cloned() {
            if record.dashboard_projection.is_none() {
                if let Some(path) = record.result_path.as_deref() {
                    match crate::report::E2eReport::read_from(&output_root.join(path)) {
                        Ok((report, _)) if report.execution.execution_id == record.execution_id => {
                            record.report = Some(report);
                        }
                        Ok(_) => bail!("native report identity differs for {}", record.execution_id),
                        Err(error) => unavailable.push(json!({"execution_id": record.execution_id, "error": format!("{error:#}")})),
                    }
                }
                record.dashboard_projection = Some(serde_json::to_value(
                    crate::dashboard::ExecutionProjection::from_record(&record)?,
                )?);
            } else {
                serde_json::from_value::<crate::dashboard::ExecutionProjection>(
                    record.dashboard_projection.clone().unwrap(),
                )
                .context("validate retained execution projection")?;
            }
            statements.push(execution_statement(&record)?);
        }
        for plan in &plans {
            statements.push(saved_plan_statement(plan)?);
        }
        for execution in &plan_executions {
            statements.push(saved_plan_execution_statement(execution)?);
        }
        statements.extend([
            json!({"sql": "DROP TABLE IF EXISTS plan_slots", "params": []}),
            json!({"sql": "DROP TABLE IF EXISTS plan_executions", "params": []}),
            json!({"sql": "DROP TABLE IF EXISTS plans", "params": []}),
            json!({"sql": "DROP TABLE harness_e2e_schema", "params": []}),
        ]);
        statements.extend(
            SCHEMA
                .iter()
                .chain(crate::history::store::SQL.iter())
                .map(|sql| json!({"sql": sql, "params": []})),
        );
        statements.push(
            json!({"sql": "INSERT INTO harness_e2e_schema(version) VALUES (3)", "params": []}),
        );
        if apply {
            self.transaction(statements).await?;
        }
        Ok(
            json!({"version": if apply { 3 } else { versions[0]["version"].as_u64().unwrap_or_default() }, "target_version": 3, "executions": records.len(), "plans": plans.len(), "plan_executions": plan_executions.len(), "apply": apply, "unavailable_evidence": unavailable}),
        )
    }

    pub(crate) async fn saved_plan(&self, id: &str) -> Result<Option<SavedPlan>> {
        self.payload_one("SELECT payload_json, payload_sha256 FROM saved_plans WHERE id = ? AND origin = 'local'", json!([id]), "saved plan").await
    }

    pub(crate) async fn saved_plans(&self) -> Result<Vec<SavedPlan>> {
        self.payloads("SELECT payload_json, payload_sha256 FROM saved_plans WHERE origin = 'local' ORDER BY updated_at DESC, id DESC", json!([]), "saved plan").await
    }

    pub(crate) async fn saved_plan_execution(&self, id: &str) -> Result<Option<PlanExecution>> {
        self.payload_one("SELECT payload_json, payload_sha256 FROM saved_plan_executions WHERE id = ? AND origin = 'local'", json!([id]), "saved plan execution").await
    }

    pub(crate) async fn saved_plan_executions(
        &self,
        plan_id: Option<&str>,
    ) -> Result<Vec<PlanExecution>> {
        let (sql, params) = match plan_id { Some(id) => ("SELECT payload_json, payload_sha256 FROM saved_plan_executions WHERE origin = 'local' AND plan_id = ? ORDER BY started_at DESC", json!([id])), None => ("SELECT payload_json, payload_sha256 FROM saved_plan_executions WHERE origin = 'local' ORDER BY started_at DESC", json!([])) };
        self.payloads(sql, params, "saved plan execution").await
    }

    pub(crate) async fn save_plan(&self, plan: &SavedPlan) -> Result<()> {
        self.transaction(vec![saved_plan_statement(plan)?]).await
    }
    pub(crate) async fn save_plan_execution(&self, execution: &PlanExecution) -> Result<()> {
        self.transaction(vec![saved_plan_execution_statement(execution)?])
            .await
    }
    pub(crate) async fn save_plan_and_execution(
        &self,
        plan: &SavedPlan,
        execution: &PlanExecution,
    ) -> Result<()> {
        self.transaction(vec![
            saved_plan_statement(plan)?,
            saved_plan_execution_statement(execution)?,
        ])
        .await
    }
    pub(crate) async fn delete_plan_and_executions(&self, id: &str) -> Result<()> {
        self.transaction(vec![json!({"sql":"DELETE FROM saved_plan_executions WHERE plan_id = ? AND origin = 'local'","params":[id]}),json!({"sql":"DELETE FROM saved_plans WHERE id = ? AND origin = 'local'","params":[id]})]).await
    }

    async fn payload_one<T: DeserializeOwned>(
        &self,
        sql: &str,
        params: Value,
        kind: &str,
    ) -> Result<Option<T>> {
        self.query(sql, params)
            .await?
            .first()
            .map(|row| decode_hashed_payload(row, kind))
            .transpose()
    }
    async fn payloads<T: DeserializeOwned>(
        &self,
        sql: &str,
        params: Value,
        kind: &str,
    ) -> Result<Vec<T>> {
        self.query(sql, params)
            .await?
            .iter()
            .map(|row| decode_hashed_payload(row, kind))
            .collect()
    }

    pub async fn save_execution(&self, record: &ExecutionRecord) -> Result<()> {
        self.transaction(vec![execution_statement(record)?]).await
    }

    /// Commit a terminal execution and all queryable observations together.
    /// The full report remains in the native bundle; rows carry only fields
    /// required by filtering, aggregation and the attempt/detail projection.
    pub async fn save_terminal_projection(&self, record: &ExecutionRecord) -> Result<()> {
        let mut statements = vec![execution_statement(record)?];
        statements.extend([
            json!({"sql": "DELETE FROM attempts WHERE execution_id = ?", "params": [record.execution_id]}),
            json!({"sql": "DELETE FROM runs WHERE execution_id = ?", "params": [record.execution_id]}),
            json!({"sql": "DELETE FROM artifacts WHERE execution_id = ?", "params": [record.execution_id]}),
        ]);
        if let Some(report) = &record.report {
            for scenario in &report.scenarios {
                for run in &scenario.runs {
                    statements.extend(run_projection(&record.execution_id, scenario, run)?);
                }
            }
        }
        if let Some(archive) = &record.archive {
            statements.push(json!({
                "sql": "INSERT INTO archives(execution_id, archive_id, manifest_uri, manifest_sha256, expires_at, payload_json) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(execution_id) DO UPDATE SET archive_id=excluded.archive_id, manifest_uri=excluded.manifest_uri, manifest_sha256=excluded.manifest_sha256, expires_at=excluded.expires_at, payload_json=excluded.payload_json",
                "params": [record.execution_id, archive.archive_id, archive.manifest.uri, archive.manifest.sha256, archive.expires_at, serde_json::to_string(archive)?]
            }));
        }
        self.transaction(statements).await
    }

    /// Remove one execution and every row that projects it. SQLite foreign-key
    /// cascades are deliberately not relied on by the control plane.
    pub async fn delete_execution(&self, execution_id: &str) -> Result<()> {
        self.transaction(vec![
            json!({"sql": "DELETE FROM attempts WHERE execution_id = ?", "params": [execution_id]}),
            json!({"sql": "DELETE FROM runs WHERE execution_id = ?", "params": [execution_id]}),
            json!({"sql": "DELETE FROM artifacts WHERE execution_id = ?", "params": [execution_id]}),
            json!({"sql": "DELETE FROM archives WHERE execution_id = ?", "params": [execution_id]}),
            json!({"sql": "DELETE FROM executions WHERE execution_id = ?", "params": [execution_id]}),
        ])
        .await
    }

    pub async fn execution(&self, execution_id: &str) -> Result<Option<ExecutionRecord>> {
        let rows = self
            .query(
                "SELECT record_json, record_sha256 FROM executions WHERE execution_id = ?",
                json!([execution_id]),
            )
            .await?;
        rows.into_iter()
            .next()
            .map(|row| decode_record(&row))
            .transpose()
    }

    pub async fn executions(&self) -> Result<Vec<ExecutionRecord>> {
        self.records_query(
            "SELECT record_json, record_sha256 FROM executions ORDER BY requested_at DESC",
            json!([]),
        )
        .await
    }

    pub async fn active_executions(&self) -> Result<Vec<ExecutionRecord>> {
        self.records_query(
            "SELECT record_json, record_sha256 FROM executions WHERE terminal = 0 ORDER BY requested_at DESC",
            json!([]),
        ).await
    }

    pub async fn execution_for_key(&self, key: &str) -> Result<Option<ExecutionRecord>> {
        let rows = self
            .query(
                "SELECT record_json, record_sha256 FROM executions WHERE idempotency_key = ?",
                json!([key]),
            )
            .await?;
        rows.into_iter()
            .next()
            .map(|row| decode_record(&row))
            .transpose()
    }

    pub async fn active_count(&self) -> Result<u64> {
        let rows = self
            .query(
                "SELECT COUNT(*) AS count FROM executions WHERE terminal = 0",
                json!([]),
            )
            .await?;
        Ok(rows
            .first()
            .and_then(|row| row.get("count"))
            .and_then(Value::as_u64)
            .unwrap_or(0))
    }

    pub async fn attempt(
        &self,
        execution_id: &str,
        run_id: &str,
        attempt_id: &str,
    ) -> Result<Option<Value>> {
        let rows = self
            .query(
                "SELECT payload_json FROM attempts WHERE execution_id = ? AND run_id = ? AND attempt_id = ?",
                json!([execution_id, run_id, attempt_id]),
            )
            .await?;
        rows.into_iter()
            .next()
            .map(|row| decode_payload(&row))
            .transpose()
    }

    pub async fn save_local_scenario(&self, id: &str, payload: &Value) -> Result<()> {
        self.transaction(vec![json!({
            "sql": "INSERT INTO local_scenarios(scenario_id, source_sha256, source_path, created_at, payload_json) VALUES (?, ?, ?, ?, ?) ON CONFLICT(scenario_id) DO UPDATE SET source_sha256=excluded.source_sha256, source_path=excluded.source_path, payload_json=excluded.payload_json",
            "params": [id, payload["source_sha256"], payload["source_path"], payload["created_at"], serde_json::to_string(payload)?],
        })]).await
    }

    async fn records_query(&self, sql: &str, params: Value) -> Result<Vec<ExecutionRecord>> {
        self.query(sql, params)
            .await?
            .iter()
            .map(decode_record)
            .collect()
    }

    pub(crate) async fn query(&self, sql: &str, params: Value) -> Result<Vec<Value>> {
        let response = self
            .call(
                DATABASE_QUERY,
                json!({"db": self.database, "sql": sql, "params": params}),
            )
            .await?;
        response
            .get("rows")
            .and_then(Value::as_array)
            .cloned()
            .context("database query response has no rows")
    }

    pub(crate) async fn transaction(&self, statements: Vec<Value>) -> Result<()> {
        self.transaction_results(statements).await.map(|_| ())
    }

    pub(crate) async fn transaction_results(&self, statements: Vec<Value>) -> Result<Vec<Value>> {
        let response = self
            .call(
                DATABASE_TRANSACTION,
                json!({"db": self.database, "isolation": "serializable", "statements": statements}),
            )
            .await?;
        if response.get("committed") == Some(&Value::Bool(true)) {
            return Ok(response
                .get("results")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default());
        }
        bail!(
            "database transaction did not commit: {}",
            response.get("error").cloned().unwrap_or(response)
        )
    }

    async fn call(&self, function_id: &str, payload: Value) -> Result<Value> {
        self.iii
            .trigger(
                TriggerRequest {
                    function_id: function_id.into(),
                    payload,
                    action: None,
                    timeout_ms: Some(120_000),
                }
                .namespace(self.namespace.clone()),
            )
            .await
            .map_err(|error| anyhow::anyhow!("{function_id}: {error}"))
    }
}

fn execution_statement(record: &ExecutionRecord) -> Result<Value> {
    let mut persisted = record.clone();
    if record.phase.terminal() && record.report.is_some() {
        persisted.dashboard_projection = match crate::dashboard::ExecutionProjection::from_record(
            record,
        ) {
            Ok(projection) => Some(serde_json::to_value(projection)?),
            Err(error) => {
                tracing::warn!(execution_id = %record.execution_id, %error, "dashboard projection unavailable; retaining terminal execution");
                None
            }
        };
    }
    // Native bundles remain the sole complete representation of reports,
    // manifests and observations. SQL holds operational state and projections.
    persisted.report = None;
    persisted.manifest = None;
    persisted.observation = None;
    persisted.observation_artifact = None;
    let body = serde_json::to_string(&persisted).context("encode execution record")?;
    let body_sha = crate::artifact::sha256_bytes(body.as_bytes());
    Ok(json!({
        "sql": "INSERT INTO executions (execution_id, idempotency_key, phase, requested_at, updated_at, lane, subject_provider, subject_model, terminal, result_path, result_sha256, record_json, record_sha256) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?) ON CONFLICT(execution_id) DO UPDATE SET phase = excluded.phase, updated_at = excluded.updated_at, lane = excluded.lane, subject_provider = excluded.subject_provider, subject_model = excluded.subject_model, terminal = excluded.terminal, result_path = excluded.result_path, record_json = excluded.record_json, record_sha256 = excluded.record_sha256",
        "params": [record.execution_id, record.idempotency_key, format!("{:?}", record.phase).to_ascii_lowercase(), record.requested_at, record.updated_at, record.request.lane, record.request.provider, record.request.model, i64::from(record.phase.terminal()), record.result_path, body, body_sha]
    }))
}

fn run_projection(
    execution_id: &str,
    scenario: &E2eScenarioReport,
    run: &E2eRunReport,
) -> Result<Vec<Value>> {
    let payload = json!({
        "criteria": run.criteria,
        "dimensions": run.dimensions,
        "failures": run.failures,
        "scenario_measurements": run.scenario_measurements,
        "efficiency": run.efficiency,
        "telemetry_unavailable": run.efficiency.as_ref().map(|value| &value.observed_complexity.unavailable),
    });
    let efficiency = run.efficiency.as_ref();
    let mut statements = vec![json!({
        "sql": "INSERT INTO runs(execution_id, run_id, scenario_id, scenario_version, case_id, seed, selected_attempt_id, status, completion, technical, objective_score, quality_score_completed, wall_time_ms, total_tokens, cost_total_usd, function_calls, function_call_errors, technical_attempts, telemetry_json, payload_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(execution_id, run_id) DO UPDATE SET selected_attempt_id=excluded.selected_attempt_id, status=excluded.status, completion=excluded.completion, technical=excluded.technical, objective_score=excluded.objective_score, quality_score_completed=excluded.quality_score_completed, wall_time_ms=excluded.wall_time_ms, total_tokens=excluded.total_tokens, cost_total_usd=excluded.cost_total_usd, function_calls=excluded.function_calls, function_call_errors=excluded.function_call_errors, technical_attempts=excluded.technical_attempts, telemetry_json=excluded.telemetry_json, payload_json=excluded.payload_json",
        "params": [execution_id, run.run_id, scenario.scenario_id, scenario.scenario_version, scenario.case_id, scenario.case.as_ref().map(|case| case.seed.to_string()), run.attempt_id, format!("{:?}", run.status).to_ascii_lowercase(), format!("{:?}", run.completion).to_ascii_lowercase(), format!("{:?}", run.technical).to_ascii_lowercase(), run.objective_score, run.quality_score_completed, run.wall_time_ms, efficiency.and_then(|value| value.total_tokens), run.cost.total_usd, efficiency.and_then(|value| value.function_calls), efficiency.and_then(|value| value.function_call_errors), efficiency.map(|value| value.technical_attempts).unwrap_or(1), serde_json::to_string(&payload)?, serde_json::to_string(&payload)?]
    })];
    statements.extend(attempt_projection(execution_id, scenario, run, false)?);
    for retry in &run.retry_attempts {
        let payload = json!({
            "status": retry.status, "completion": retry.completion, "technical": retry.technical,
            "dimensions": retry.dimensions, "failures": retry.failures, "efficiency": retry.efficiency,
        });
        statements.push(json!({
            "sql": "INSERT INTO attempts(execution_id, attempt_id, run_id, session_id, phase, started_at, completed_at, resume_state_path, resume_state_sha256, failure_reason, payload_json) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?, ?) ON CONFLICT(execution_id, attempt_id) DO UPDATE SET phase=excluded.phase, completed_at=excluded.completed_at, failure_reason=excluded.failure_reason, payload_json=excluded.payload_json",
            "params": [execution_id, retry.attempt_id, retry.run_id, retry.session_id, format!("{:?}", retry.status).to_ascii_lowercase(), "", "", retry.failures.first().map(|failure| &failure.message), serde_json::to_string(&payload)?]
        }));
        statements.extend(artifact_statements(execution_id, &retry.evidence));
    }
    Ok(statements)
}

fn attempt_projection(
    execution_id: &str,
    _scenario: &E2eScenarioReport,
    run: &E2eRunReport,
    _retry: bool,
) -> Result<Vec<Value>> {
    let payload = json!({
        "status": run.status, "completion": run.completion, "technical": run.technical,
        "criteria": run.criteria, "dimensions": run.dimensions, "failures": run.failures,
        "efficiency": run.efficiency,
    });
    let mut statements = vec![json!({
        "sql": "INSERT INTO attempts(execution_id, attempt_id, run_id, session_id, phase, started_at, completed_at, resume_state_path, resume_state_sha256, failure_reason, payload_json) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?, ?) ON CONFLICT(execution_id, attempt_id) DO UPDATE SET phase=excluded.phase, completed_at=excluded.completed_at, failure_reason=excluded.failure_reason, payload_json=excluded.payload_json",
        "params": [execution_id, run.attempt_id, run.run_id, run.session_id, format!("{:?}", run.status).to_ascii_lowercase(), "", "", run.failures.first().map(|failure| &failure.message), serde_json::to_string(&payload)?]
    })];
    statements.extend(artifact_statements(execution_id, &run.evidence));
    for deliverable in &run.deliverables {
        if let Some(artifact) = &deliverable.artifact {
            statements.extend(artifact_statements(
                execution_id,
                std::slice::from_ref(artifact),
            ));
        }
    }
    Ok(statements)
}

fn artifact_statements(
    execution_id: &str,
    artifacts: &[crate::artifact::ArtifactReference],
) -> Vec<Value> {
    artifacts.iter().map(|artifact| json!({
        "sql": "INSERT OR REPLACE INTO artifacts(execution_id, artifact_id, kind, relative_path, sha256, size_bytes, media_type, available, archive_uri) VALUES (?, ?, ?, ?, ?, ?, ?, 1, NULL)",
        "params": [execution_id, artifact.id, artifact.kind, artifact.path, artifact.sha256, artifact.size_bytes, artifact.media_type]
    })).collect()
}

fn decode_record(row: &Value) -> Result<ExecutionRecord> {
    let source = row
        .get("record_json")
        .and_then(Value::as_str)
        .context("execution row lacks record_json")?;
    let hash = row
        .get("record_sha256")
        .and_then(Value::as_str)
        .context("execution row lacks record_sha256")?;
    if crate::artifact::sha256_bytes(source.as_bytes()) != hash {
        bail!("execution record hash does not match");
    }
    serde_json::from_str(source).context("decode execution record")
}

fn decode_payload<T: DeserializeOwned>(row: &Value) -> Result<T> {
    serde_json::from_str(
        row.get("payload_json")
            .and_then(Value::as_str)
            .context("persistence row lacks payload_json")?,
    )
    .context("decode persistence payload")
}

fn decode_hashed_payload<T: DeserializeOwned>(row: &Value, kind: &str) -> Result<T> {
    let payload = row
        .get("payload_json")
        .and_then(Value::as_str)
        .with_context(|| format!("{kind} row lacks payload_json"))?;
    let hash = row
        .get("payload_sha256")
        .and_then(Value::as_str)
        .with_context(|| format!("{kind} row lacks payload_sha256"))?;
    if crate::artifact::sha256_bytes(payload.as_bytes()) != hash {
        bail!("{kind} payload hash does not match");
    }
    serde_json::from_str(payload).with_context(|| format!("decode {kind}"))
}

fn saved_plan_statement(plan: &SavedPlan) -> Result<Value> {
    validate_saved_plan(plan)?;
    let payload = serde_json::to_string(plan)?;
    let hash = crate::artifact::sha256_bytes(payload.as_bytes());
    Ok(
        json!({"sql": "INSERT INTO saved_plans(id, origin, source_instance, source_id, source_updated_at, source_content_sha256, updated_at, payload_json, payload_sha256) VALUES (?, 'local', NULL, NULL, NULL, NULL, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET updated_at=excluded.updated_at, payload_json=excluded.payload_json, payload_sha256=excluded.payload_sha256", "params": [plan.plan.id, plan.plan.updated_at, payload, hash]}),
    )
}

fn saved_plan_execution_statement(execution: &PlanExecution) -> Result<Value> {
    validate_saved_plan_execution(execution)?;
    let payload = serde_json::to_string(execution)?;
    let hash = crate::artifact::sha256_bytes(payload.as_bytes());
    Ok(
        json!({"sql": "INSERT INTO saved_plan_executions(id, plan_id, origin, source_instance, source_id, source_updated_at, source_content_sha256, idempotency_key, state, started_at, updated_at, payload_json, payload_sha256) VALUES (?, ?, 'local', NULL, NULL, NULL, NULL, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET state=excluded.state, updated_at=excluded.updated_at, payload_json=excluded.payload_json, payload_sha256=excluded.payload_sha256", "params": [execution.id, execution.plan_id, execution.idempotency_key, execution.state, execution.started_at, execution.updated_at, payload, hash]}),
    )
}

fn validate_saved_plan(plan: &SavedPlan) -> Result<()> {
    crate::plans::store::validate_saved_plan(plan)
}

fn validate_saved_plan_execution(execution: &PlanExecution) -> Result<()> {
    crate::plans::store::validate_saved_plan_execution(execution)
}

pub(crate) fn load_plan_store(root: &Path) -> Result<(Vec<SavedPlan>, Vec<PlanExecution>)> {
    let plans_root = root.join("plan-store/plans");
    let executions_root = root.join("plan-store/executions");
    let mut migrated_hashes = std::collections::BTreeMap::new();
    let plans = load_json_directory(&plans_root, "plan", |plan: &mut SavedPlan| {
        if let Some(previous) = crate::plans::store::migrate_saved_plan(plan)? {
            migrated_hashes.insert(
                plan.plan.id.clone(),
                (previous, plan.configuration_sha256.clone()),
            );
        }
        Ok(())
    })?;
    let executions = load_json_directory(
        &executions_root,
        "plan execution",
        |execution: &mut PlanExecution| {
            validate_saved_plan_execution(execution)?;
            if let Some((previous, current)) = migrated_hashes.get(&execution.plan_id) {
                if execution.configuration_sha256 == *previous {
                    execution.configuration_sha256 = current.clone();
                }
            }
            Ok(())
        },
    )?;
    let ids = plans
        .iter()
        .map(|plan| plan.plan.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for execution in &executions {
        if matches!(execution.state.as_str(), "running" | "cancelling") {
            bail!(
                "finish or cancel active plan execution {} before migrating",
                execution.id
            );
        }
        if !ids.contains(execution.plan_id.as_str()) {
            bail!(
                "saved plan execution {} references missing plan {}",
                execution.id,
                execution.plan_id
            );
        }
    }
    Ok((plans, executions))
}

fn load_json_directory<T>(
    path: &Path,
    kind: &str,
    mut validate: impl FnMut(&mut T) -> Result<()>,
) -> Result<Vec<T>>
where
    T: DeserializeOwned,
{
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut values = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let raw: Value = serde_json::from_slice(&std::fs::read(&path)?)
            .with_context(|| format!("read {kind} {}", path.display()))?;
        if raw["id"].as_str() != path.file_stem().and_then(|name| name.to_str()) {
            bail!(
                "{kind} identity differs from its filename: {}",
                path.display()
            );
        }
        let mut value = serde_json::from_value(raw)?;
        validate(&mut value).with_context(|| format!("validate {kind} {}", path.display()))?;
        values.push(value);
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    use super::*;

    #[tokio::test]
    #[ignore = "requires HARNESS_E2E_TEST_DATABASE_URL and an isolated database worker"]
    async fn real_database_migrates_v2_dry_run_apply_and_noop() {
        let url = std::env::var("HARNESS_E2E_TEST_DATABASE_URL").unwrap();
        let client = iii_sdk::register_worker(
            &url,
            iii_sdk::InitOptions {
                namespace: Some("default".into()),
                ..iii_sdk::InitOptions::default()
            },
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            while client.get_connection_state() != iii_sdk::runtime::IIIConnectionState::Connected {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let persistence = Persistence::new(client.clone(), "harness_e2e".into(), "default".into());
        persistence.initialize().await.unwrap();
        persistence
            .transaction(vec![
                json!({"sql": "DROP TABLE harness_e2e_schema", "params": []}),
                json!({"sql": "CREATE TABLE harness_e2e_schema (version INTEGER PRIMARY KEY CHECK (version = 2))", "params": []}),
                json!({"sql": "INSERT INTO harness_e2e_schema(version) VALUES (2)", "params": []}),
                json!({"sql": "DROP TABLE saved_plan_executions", "params": []}),
                json!({"sql": "DROP TABLE saved_plans", "params": []}),
            ])
            .await
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let dry = persistence
            .migrate_storage(root.path(), false)
            .await
            .unwrap();
        assert_eq!(dry["version"], 2);
        assert_eq!(dry["target_version"], 3);
        assert_eq!(
            persistence
                .query("SELECT version FROM harness_e2e_schema", json!([]))
                .await
                .unwrap()[0]["version"],
            2
        );
        let applied = persistence
            .migrate_storage(root.path(), true)
            .await
            .unwrap();
        assert_eq!(applied["version"], 3);
        assert_eq!(applied["plans"], 0);
        assert_eq!(
            persistence
                .query("SELECT version FROM harness_e2e_schema", json!([]))
                .await
                .unwrap()[0]["version"],
            3
        );
        assert!(persistence
            .migrate_storage(root.path(), true)
            .await
            .unwrap()["already_current"]
            .as_bool()
            .unwrap());
        client.shutdown_async().await;
    }

    #[test]
    fn terminal_storage_keeps_dashboard_projection_without_native_report() {
        let mut record: ExecutionRecord = serde_json::from_value(json!({
            "execution_id": "execution", "idempotency_key": "test", "phase": "completed",
            "requested_at": "2026-08-07T12:00:00Z", "updated_at": "2026-08-07T12:00:02Z",
            "request": {"idempotency_key": "test", "model": "model", "provider": "provider"},
            "lane_budget": {"max_cases": 1, "max_runs_per_case": 1, "max_technical_retries": 0, "max_declared_turns": 1},
            "transitions": [], "cancel_requested": false, "result_path": "execution/results.json",
        })).unwrap();
        record.report = Some(crate::dashboard::tests::report());
        let roundtrip = |record: &ExecutionRecord| {
            let statement = execution_statement(record).unwrap();
            decode_record(&json!({
                "record_json": statement["params"][10],
                "record_sha256": statement["params"][11],
            }))
            .unwrap()
        };
        let restored = roundtrip(&record);
        assert!(restored.report.is_none());
        assert!(restored.manifest.is_none());
        let projection = restored.dashboard_projection.as_ref().unwrap();
        assert_eq!(projection["summary"]["status"], "passed");
        assert!(projection["tests"]["direct_answer"].is_object());
        assert!(!serde_json::to_string(projection)
            .unwrap()
            .contains("prompt"));
        assert_eq!(
            roundtrip(&restored).dashboard_projection,
            restored.dashboard_projection
        );

        record.phase = crate::control::ExecutionPhase::Finalizing;
        assert!(roundtrip(&record).dashboard_projection.is_none());
        record.phase = crate::control::ExecutionPhase::Completed;
        record
            .report
            .as_mut()
            .unwrap()
            .assessment_contract
            .runs
            .clear();
        let restored = roundtrip(&record);
        assert_eq!(restored.phase, crate::control::ExecutionPhase::Completed);
        assert!(restored.dashboard_projection.is_none());
    }

    #[tokio::test]
    async fn deletion_removes_every_execution_projection_in_one_transaction() {
        let execution_id = "a".repeat(32);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let expected_id = execution_id.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            while let Some(Ok(frame)) = socket.next().await {
                let Message::Text(message) = frame else {
                    continue;
                };
                let message: Value = serde_json::from_str(&message).unwrap();
                if message["type"] != "invokefunction" || message["invocation_id"].is_null() {
                    continue;
                }
                assert_eq!(message["function_id"], DATABASE_TRANSACTION);
                let statements = message["data"]["statements"].as_array().unwrap();
                assert_eq!(
                    statements
                        .iter()
                        .map(|statement| statement["sql"].as_str().unwrap())
                        .collect::<Vec<_>>(),
                    vec![
                        "DELETE FROM attempts WHERE execution_id = ?",
                        "DELETE FROM runs WHERE execution_id = ?",
                        "DELETE FROM artifacts WHERE execution_id = ?",
                        "DELETE FROM archives WHERE execution_id = ?",
                        "DELETE FROM executions WHERE execution_id = ?",
                    ]
                );
                assert!(statements
                    .iter()
                    .all(|statement| { statement["params"] == json!([expected_id]) }));
                socket
                    .send(Message::Text(
                        json!({
                            "type": "invocationresult",
                            "invocation_id": message["invocation_id"],
                            "function_id": message["function_id"],
                            "result": {"committed": true},
                        })
                        .to_string(),
                    ))
                    .await
                    .unwrap();
                return;
            }
            panic!("database transaction was not invoked");
        });
        let client = iii_sdk::register_worker(&url, iii_sdk::InitOptions::default());
        tokio::time::timeout(Duration::from_secs(5), async {
            while client.get_connection_state() != iii_sdk::runtime::IIIConnectionState::Connected {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        Persistence::new(client.clone(), "harness_e2e".into(), "test".into())
            .delete_execution(&execution_id)
            .await
            .unwrap();
        client.shutdown_async().await;
        server.await.unwrap();
    }
}
