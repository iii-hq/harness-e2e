//! Durable, queryable control-plane state.
//!
//! The database worker owns SQL access.  The runner deliberately owns neither
//! a driver nor a connection string: it only sends parameterised statements to
//! the dedicated control-plane database namespace.
//!
//! Storage carries no version number and needs no migration step.  Every
//! table records the fingerprint of the statements that create it; at start
//! the worker recreates the tables whose fingerprint moved, keeping the rows
//! it can still read (executions, local suites and stacks, and receipts) and
//! reprojecting runs from the native bundles.  Everything else is reported as
//! a warning and never refused.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, Context, Result};
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::IIIClient;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::control::ExecutionRecord;
use crate::plans::stacks::LocalStack;
use crate::plans::store::PlanExecution;
use crate::plans::LocalSuite;
use crate::report::{E2eRunReport, E2eScenarioReport};

const DATABASE_QUERY: &str = "database::query";
const DATABASE_TRANSACTION: &str = "database::transaction";

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS harness_e2e_storage (name TEXT PRIMARY KEY, fingerprint TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS executions (execution_id TEXT PRIMARY KEY, idempotency_key TEXT NOT NULL UNIQUE, phase TEXT NOT NULL, requested_at TEXT NOT NULL, updated_at TEXT NOT NULL, lane TEXT NOT NULL, subject_provider TEXT NOT NULL, subject_model TEXT NOT NULL, terminal INTEGER NOT NULL CHECK (terminal IN (0, 1)), result_path TEXT NULL, result_sha256 TEXT NULL, record_json TEXT NOT NULL, record_sha256 TEXT NOT NULL)",
    "CREATE INDEX IF NOT EXISTS executions_requested_at_idx ON executions(requested_at DESC)",
    "CREATE INDEX IF NOT EXISTS executions_phase_idx ON executions(terminal, updated_at)",
    "CREATE TABLE IF NOT EXISTS attempts (execution_id TEXT NOT NULL, attempt_id TEXT NOT NULL, run_id TEXT NOT NULL, session_id TEXT NULL, phase TEXT NOT NULL, started_at TEXT NOT NULL, completed_at TEXT NULL, resume_state_path TEXT NULL, resume_state_sha256 TEXT NULL, failure_reason TEXT NULL, payload_json TEXT NOT NULL, PRIMARY KEY (execution_id, attempt_id))",
    "CREATE TABLE IF NOT EXISTS runs (execution_id TEXT NOT NULL, run_id TEXT NOT NULL, scenario_id TEXT NOT NULL, behavior_sha256 TEXT NULL, case_id TEXT NULL, seed TEXT NULL, selected_attempt_id TEXT NULL, status TEXT NULL, completion TEXT NULL, technical TEXT NULL, score REAL NULL, wall_time_ms INTEGER NULL, total_tokens INTEGER NULL, cost_total_usd REAL NULL, function_calls INTEGER NULL, function_call_errors INTEGER NULL, technical_attempts INTEGER NOT NULL DEFAULT 0, telemetry_json TEXT NOT NULL DEFAULT '{}', payload_json TEXT NOT NULL, PRIMARY KEY (execution_id, run_id))",
    "CREATE INDEX IF NOT EXISTS runs_execution_scenario_idx ON runs(execution_id, scenario_id, case_id)",
    "CREATE TABLE IF NOT EXISTS artifacts (execution_id TEXT NOT NULL, artifact_id TEXT NOT NULL, kind TEXT NOT NULL, relative_path TEXT NOT NULL, sha256 TEXT NOT NULL, size_bytes INTEGER NOT NULL, media_type TEXT NOT NULL, available INTEGER NOT NULL CHECK (available IN (0, 1)), archive_uri TEXT NULL, PRIMARY KEY (execution_id, artifact_id, sha256))",
    "CREATE TABLE IF NOT EXISTS archives (execution_id TEXT PRIMARY KEY, archive_id TEXT NOT NULL UNIQUE, manifest_uri TEXT NOT NULL, manifest_sha256 TEXT NOT NULL, expires_at TEXT NULL, payload_json TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS local_suites (id TEXT PRIMARY KEY, updated_at TEXT NOT NULL, payload_json TEXT NOT NULL, payload_sha256 TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS local_stacks (id TEXT PRIMARY KEY, updated_at TEXT NOT NULL, payload_json TEXT NOT NULL, payload_sha256 TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS saved_plan_executions (id TEXT PRIMARY KEY, idempotency_key TEXT NOT NULL UNIQUE, state TEXT NOT NULL, started_at TEXT NOT NULL, updated_at TEXT NOT NULL, payload_json TEXT NOT NULL, payload_sha256 TEXT NOT NULL)",
    "CREATE INDEX IF NOT EXISTS saved_plan_executions_started_idx ON saved_plan_executions(started_at DESC)",
];

const STORAGE_TABLE: &str = "harness_e2e_storage";
const PROJECTION_TABLES: &[&str] = &["attempts", "runs", "artifacts", "archives"];
/// Tables of layouts this runner no longer writes; dropped when found. The
/// `history_*` tables held Release Control history imported as a parallel
/// record; imported executions are ordinary executions now. `saved_plans`
/// held the baseline/candidate plans suites replaced.
const LEGACY_TABLES: &[&str] = &[
    "harness_e2e_schema",
    "saved_plans",
    "plans",
    "plan_executions",
    "plan_slots",
    "local_scenarios",
    "history_campaigns",
    "history_reports",
    "history_runs",
    "history_execution_labels",
];

/// One table with every statement that creates it (table and indexes).
struct TableLayout {
    name: String,
    statements: Vec<&'static str>,
}

impl TableLayout {
    /// Identity of the table layout: the statements that create it.
    fn fingerprint(&self) -> String {
        crate::artifact::sha256_bytes(self.statements.join("\n").as_bytes())
    }
}

fn statement_table(sql: &str) -> &str {
    let rest = sql
        .strip_prefix("CREATE TABLE IF NOT EXISTS ")
        .or_else(|| {
            sql.strip_prefix("CREATE INDEX IF NOT EXISTS ")
                .and_then(|rest| rest.split_once(" ON "))
                .map(|(_, table)| table)
        })
        .unwrap_or_else(|| panic!("unsupported storage statement: {sql}"));
    rest.split([' ', '(']).next().unwrap_or_default()
}

/// Every table this runner creates, in statement order.
fn table_layouts() -> Vec<TableLayout> {
    let mut layouts: Vec<TableLayout> = Vec::new();
    for sql in SCHEMA {
        let table = statement_table(sql);
        match layouts.iter_mut().find(|layout| layout.name == table) {
            Some(layout) => layout.statements.push(sql),
            None => layouts.push(TableLayout {
                name: table.to_owned(),
                statements: vec![sql],
            }),
        }
    }
    layouts
}

fn layout_statements() -> impl Iterator<Item = Value> {
    SCHEMA.iter().map(|sql| json!({"sql": sql, "params": []}))
}

/// Rows that survive the recreation of stale tables, and what did not.
#[derive(Default)]
struct Salvage {
    statements: Vec<Value>,
    executions: usize,
    suites: usize,
    stacks: usize,
    saved_executions: usize,
    dropped: Value,
    unavailable: Vec<Value>,
}

impl Salvage {
    fn count(&mut self, key: &str) {
        if self.dropped.is_null() {
            self.dropped = json!({});
        }
        let next = self.dropped[key].as_u64().unwrap_or(0) + 1;
        self.dropped[key] = json!(next);
    }
}

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

    /// Create the layout and reconcile the tables whose fingerprint moved.
    /// A stale table is dropped and recreated with the rows this runner can
    /// still read: execution records, local suites and stacks, and receipts
    /// are decoded and reinserted, run projections are rebuilt from the native
    /// bundles under `data_dir`, and tables of layouts this runner no longer
    /// writes are dropped. Returns the summary it logs as a warning; nothing here
    /// refuses the database.
    pub async fn initialize(&self, data_dir: &Path) -> Result<Value> {
        let layouts = table_layouts();
        let present = self.present_tables().await?;
        let mut recorded = BTreeMap::new();
        let mut marker_readable = true;
        if present.contains(STORAGE_TABLE) {
            for row in self
                .query(&format!("SELECT * FROM {STORAGE_TABLE}"), json!([]))
                .await?
            {
                match (row["name"].as_str(), row["fingerprint"].as_str()) {
                    (Some(name), Some(fingerprint)) => {
                        recorded.insert(name.to_owned(), fingerprint.to_owned());
                    }
                    _ => marker_readable = false,
                }
            }
        }
        let mut stale = layouts
            .iter()
            .filter(|layout| layout.name != STORAGE_TABLE)
            .filter(|layout| {
                present.contains(&layout.name)
                    && recorded.get(&layout.name) != Some(&layout.fingerprint())
            })
            .map(|layout| layout.name.clone())
            .collect::<BTreeSet<_>>();
        if present.contains(STORAGE_TABLE) && !marker_readable {
            stale.insert(STORAGE_TABLE.to_owned());
        }
        let legacy = LEGACY_TABLES
            .iter()
            .filter(|table| present.contains(**table))
            .map(|table| (*table).to_owned())
            .collect::<Vec<_>>();

        let mut statements = Vec::new();
        let mut summary = json!({});
        if !stale.is_empty() || !legacy.is_empty() {
            let salvage = self.salvage(&present, &stale, data_dir).await?;
            for table in stale.iter().chain(legacy.iter()) {
                statements
                    .push(json!({"sql": format!("DROP TABLE IF EXISTS {table}"), "params": []}));
            }
            statements.extend(layout_statements());
            statements.extend(salvage.statements);
            summary = json!({
                "recreated": stale,
                "legacy_dropped": legacy,
                "executions": salvage.executions,
                "suites": salvage.suites,
                "stacks": salvage.stacks,
                "saved_executions": salvage.saved_executions,
                "dropped": salvage.dropped,
                "unavailable_evidence": salvage.unavailable,
            });
        } else {
            statements.extend(layout_statements());
        }
        let names = layouts
            .iter()
            .map(|layout| &layout.name)
            .collect::<Vec<_>>();
        statements.push(json!({
            "sql": format!("DELETE FROM {STORAGE_TABLE} WHERE name NOT IN ({})", vec!["?"; names.len()].join(", ")),
            "params": names,
        }));
        for layout in &layouts {
            statements.push(json!({
                "sql": format!("INSERT INTO {STORAGE_TABLE}(name, fingerprint) VALUES (?, ?) ON CONFLICT(name) DO UPDATE SET fingerprint = excluded.fingerprint"),
                "params": [layout.name, layout.fingerprint()]
            }));
        }
        self.transaction(statements).await?;
        if !summary
            .as_object()
            .is_some_and(|summary| summary.is_empty())
        {
            tracing::warn!(%summary, "recreated stale Harness E2E storage tables at start");
        }
        Ok(summary)
    }

    /// What survives the recreation of the stale tables.
    async fn salvage(
        &self,
        present: &BTreeSet<String>,
        stale: &BTreeSet<String>,
        data_dir: &Path,
    ) -> Result<Salvage> {
        let is_stale = |table: &str| stale.contains(table);
        let projections_stale = PROJECTION_TABLES.iter().any(|table| is_stale(table));
        let mut salvage = Salvage::default();

        // Execution records: reinserted when their table moved, reprojected
        // from the native bundles when a projection table moved.
        if (is_stale("executions") || projections_stale) && present.contains("executions") {
            let mut records = Vec::new();
            for row in self.query("SELECT * FROM executions", json!([])).await? {
                match decode_record(&row) {
                    Ok(record) => records.push(record),
                    Err(error) => {
                        tracing::warn!(execution_id = %row["execution_id"], %error, "dropping an execution record this runner cannot read");
                        salvage.count("executions");
                    }
                }
            }
            salvage.executions = records.len();
            for mut record in records {
                if projections_stale {
                    if let Some(path) = record
                        .result_path
                        .as_deref()
                        .filter(|_| record.phase.terminal())
                    {
                        match crate::report::E2eReport::read_from(&data_dir.join(path)) {
                            Ok((report, _))
                                if report.execution.execution_id == record.execution_id =>
                            {
                                record.report = Some(report);
                            }
                            Ok(_) => salvage.unavailable.push(json!({"execution_id": record.execution_id, "error": "native report identity differs"})),
                            Err(error) => salvage.unavailable.push(json!({"execution_id": record.execution_id, "error": format!("{error:#}")})),
                        }
                    }
                    if record.report.is_none()
                        && record
                            .dashboard_projection
                            .as_ref()
                            .is_some_and(|projection| {
                                serde_json::from_value::<crate::dashboard::ExecutionProjection>(
                                    projection.clone(),
                                )
                                .is_err()
                            })
                    {
                        salvage.unavailable.push(json!({"execution_id": record.execution_id, "error": "retained dashboard projection is unreadable"}));
                        record.dashboard_projection = None;
                    }
                    salvage
                        .statements
                        .extend(terminal_projection_statements(&record)?);
                } else {
                    salvage.statements.push(execution_statement(&record)?);
                }
            }
        }

        // Suites and executions: decoded and validated before reinsertion.
        if is_stale("local_suites") && present.contains("local_suites") {
            for row in self.query("SELECT * FROM local_suites", json!([])).await? {
                match decode_hashed_payload::<LocalSuite>(&row, "local suite") {
                    Ok(suite) => {
                        salvage.suites += 1;
                        salvage.statements.push(local_suite_statement(&suite)?);
                    }
                    Err(error) => {
                        tracing::warn!(id = %row["id"], %error, "dropping a suite this runner cannot read");
                        salvage.count("suites");
                    }
                }
            }
        }
        if is_stale("local_stacks") && present.contains("local_stacks") {
            for row in self.query("SELECT * FROM local_stacks", json!([])).await? {
                match decode_hashed_payload::<LocalStack>(&row, "local stack") {
                    Ok(stack) => {
                        salvage.stacks += 1;
                        salvage.statements.push(local_stack_statement(&stack)?);
                    }
                    Err(error) => {
                        tracing::warn!(id = %row["id"], %error, "dropping a stack this runner cannot read");
                        salvage.count("stacks");
                    }
                }
            }
        }
        if is_stale("saved_plan_executions") && present.contains("saved_plan_executions") {
            for row in self
                .query("SELECT * FROM saved_plan_executions", json!([]))
                .await?
            {
                match decode_hashed_payload::<PlanExecution>(&row, "execution")
                    .and_then(|execution| validate_saved_execution(&execution).map(|()| execution))
                {
                    Ok(execution) => {
                        salvage.saved_executions += 1;
                        salvage
                            .statements
                            .push(saved_execution_statement(&execution)?);
                    }
                    Err(error) => {
                        tracing::warn!(id = %row["id"], %error, "dropping an execution this runner cannot read");
                        salvage.count("saved_executions");
                    }
                }
            }
        }
        Ok(salvage)
    }

    async fn present_tables(&self) -> Result<BTreeSet<String>> {
        Ok(self
            .query(
                "SELECT name FROM sqlite_master WHERE type = 'table'",
                json!([]),
            )
            .await?
            .iter()
            .filter_map(|row| row["name"].as_str())
            .map(str::to_owned)
            .collect())
    }

    pub(crate) async fn local_suite(&self, id: &str) -> Result<Option<LocalSuite>> {
        Ok(self
            .retained_suites(
                "SELECT id, payload_json, payload_sha256 FROM local_suites WHERE id = ?",
                json!([id]),
            )
            .await?
            .pop())
    }

    pub(crate) async fn local_suites(&self) -> Result<Vec<LocalSuite>> {
        self.retained_suites(
            "SELECT id, payload_json, payload_sha256 FROM local_suites ORDER BY updated_at DESC, id",
            json!([]),
        )
        .await
    }

    pub(crate) async fn local_stack(&self, id: &str) -> Result<Option<LocalStack>> {
        Ok(self
            .retained_stacks(
                "SELECT id, payload_json, payload_sha256 FROM local_stacks WHERE id = ?",
                json!([id]),
            )
            .await?
            .pop())
    }

    pub(crate) async fn local_stacks(&self) -> Result<Vec<LocalStack>> {
        self.retained_stacks(
            "SELECT id, payload_json, payload_sha256 FROM local_stacks ORDER BY updated_at DESC, id",
            json!([]),
        )
        .await
    }

    pub(crate) async fn saved_execution(&self, id: &str) -> Result<Option<PlanExecution>> {
        Ok(self
            .retained_executions(
                "SELECT id, payload_json, payload_sha256 FROM saved_plan_executions WHERE id = ?",
                json!([id]),
            )
            .await?
            .pop())
    }

    pub(crate) async fn saved_executions(&self) -> Result<Vec<PlanExecution>> {
        self.retained_executions(
            "SELECT id, payload_json, payload_sha256 FROM saved_plan_executions ORDER BY started_at DESC",
            json!([]),
        )
        .await
    }

    async fn retained_suites(&self, sql: &str, params: Value) -> Result<Vec<LocalSuite>> {
        self.retained(
            sql,
            params,
            "local suite",
            |_| Ok(()),
            |id| vec![json!({"sql": "DELETE FROM local_suites WHERE id = ?", "params": [id]})],
        )
        .await
    }

    async fn retained_stacks(&self, sql: &str, params: Value) -> Result<Vec<LocalStack>> {
        self.retained(
            sql,
            params,
            "local stack",
            |_| Ok(()),
            |id| vec![json!({"sql": "DELETE FROM local_stacks WHERE id = ?", "params": [id]})],
        )
        .await
    }

    async fn retained_executions(&self, sql: &str, params: Value) -> Result<Vec<PlanExecution>> {
        self.retained(sql, params, "execution", validate_saved_execution, |id| {
            vec![json!({"sql": "DELETE FROM saved_plan_executions WHERE id = ?", "params": [id]})]
        })
        .await
    }

    /// Decode the rows this runner can read. A row it cannot read was written
    /// under another layout and is deleted instead of carried along.
    async fn retained<T: DeserializeOwned>(
        &self,
        sql: &str,
        params: Value,
        kind: &str,
        validate: fn(&T) -> Result<()>,
        discard: fn(&str) -> Vec<Value>,
    ) -> Result<Vec<T>> {
        let mut values = Vec::new();
        let mut discarded = Vec::new();
        for row in self.query(sql, params).await? {
            match decode_hashed_payload::<T>(&row, kind)
                .and_then(|value| validate(&value).map(|()| value))
            {
                Ok(value) => values.push(value),
                Err(error) => {
                    let id = row["id"].as_str().unwrap_or_default();
                    tracing::warn!(kind, id, %error, "deleting a row this runner cannot read");
                    discarded.extend(discard(id));
                }
            }
        }
        if !discarded.is_empty() {
            self.transaction(discarded).await?;
        }
        Ok(values)
    }

    pub(crate) async fn save_local_suite(&self, suite: &LocalSuite) -> Result<()> {
        self.transaction(vec![local_suite_statement(suite)?]).await
    }
    pub(crate) async fn delete_local_suite(&self, id: &str) -> Result<()> {
        self.transaction(vec![
            json!({"sql": "DELETE FROM local_suites WHERE id = ?", "params": [id]}),
        ])
        .await
    }
    pub(crate) async fn save_local_stack(&self, stack: &LocalStack) -> Result<()> {
        self.transaction(vec![local_stack_statement(stack)?]).await
    }
    pub(crate) async fn delete_local_stack(&self, id: &str) -> Result<()> {
        self.transaction(vec![
            json!({"sql": "DELETE FROM local_stacks WHERE id = ?", "params": [id]}),
        ])
        .await
    }
    pub(crate) async fn save_execution_receipt(&self, execution: &PlanExecution) -> Result<()> {
        self.transaction(vec![saved_execution_statement(execution)?])
            .await
    }
    pub(crate) async fn delete_execution_receipt(&self, id: &str) -> Result<()> {
        self.transaction(vec![
            json!({"sql": "DELETE FROM saved_plan_executions WHERE id = ?", "params": [id]}),
        ])
        .await
    }

    pub async fn save_execution(&self, record: &ExecutionRecord) -> Result<()> {
        self.transaction(vec![execution_statement(record)?]).await
    }

    /// Commit a terminal execution and all queryable observations together.
    /// The full report remains in the native bundle; rows carry only fields
    /// required by filtering, aggregation and the attempt/detail projection.
    pub async fn save_terminal_projection(&self, record: &ExecutionRecord) -> Result<()> {
        self.transaction(terminal_projection_statements(record)?)
            .await
    }

    /// Remove one execution and every row that projects it. SQLite foreign-key
    /// cascades are deliberately not relied on by the control plane.
    pub async fn delete_execution(&self, execution_id: &str) -> Result<()> {
        self.transaction(delete_execution_statements(execution_id))
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
            "SELECT execution_id, record_json, record_sha256 FROM executions ORDER BY requested_at DESC",
            json!([]),
        )
        .await
    }

    pub async fn active_executions(&self) -> Result<Vec<ExecutionRecord>> {
        self.records_query(
            "SELECT execution_id, record_json, record_sha256 FROM executions WHERE terminal = 0 ORDER BY requested_at DESC",
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

    /// Decode the records this runner can read. A record it cannot read
    /// (one naming a removed scenario, say) is deleted with a warning, as
    /// suites are, so it never fails the start or a list.
    async fn records_query(&self, sql: &str, params: Value) -> Result<Vec<ExecutionRecord>> {
        let mut records = Vec::new();
        let mut discarded = Vec::new();
        for row in self.query(sql, params).await? {
            match decode_record(&row) {
                Ok(record) => records.push(record),
                Err(error) => {
                    let execution_id = row["execution_id"].as_str().unwrap_or_default();
                    let error = format!("{error:#}");
                    let reason = error.split(", expected").next().unwrap_or_default();
                    tracing::warn!(
                        execution_id,
                        reason,
                        "deleting an execution record this runner cannot read"
                    );
                    discarded.extend(delete_execution_statements(execution_id));
                }
            }
        }
        if !discarded.is_empty() {
            self.transaction(discarded).await?;
        }
        Ok(records)
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

fn terminal_projection_statements(record: &ExecutionRecord) -> Result<Vec<Value>> {
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
    Ok(statements)
}

fn delete_execution_statements(execution_id: &str) -> Vec<Value> {
    vec![
        json!({"sql": "DELETE FROM attempts WHERE execution_id = ?", "params": [execution_id]}),
        json!({"sql": "DELETE FROM runs WHERE execution_id = ?", "params": [execution_id]}),
        json!({"sql": "DELETE FROM artifacts WHERE execution_id = ?", "params": [execution_id]}),
        json!({"sql": "DELETE FROM archives WHERE execution_id = ?", "params": [execution_id]}),
        json!({"sql": "DELETE FROM executions WHERE execution_id = ?", "params": [execution_id]}),
    ]
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
        "sql": "INSERT INTO runs(execution_id, run_id, scenario_id, behavior_sha256, case_id, seed, selected_attempt_id, status, completion, technical, score, wall_time_ms, total_tokens, cost_total_usd, function_calls, function_call_errors, technical_attempts, telemetry_json, payload_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(execution_id, run_id) DO UPDATE SET selected_attempt_id=excluded.selected_attempt_id, status=excluded.status, completion=excluded.completion, technical=excluded.technical, score=excluded.score, wall_time_ms=excluded.wall_time_ms, total_tokens=excluded.total_tokens, cost_total_usd=excluded.cost_total_usd, function_calls=excluded.function_calls, function_call_errors=excluded.function_call_errors, technical_attempts=excluded.technical_attempts, telemetry_json=excluded.telemetry_json, payload_json=excluded.payload_json",
        "params": [execution_id, run.run_id, scenario.scenario_id, scenario.behavior_sha256, scenario.case_id, scenario.case.as_ref().map(|case| case.seed.to_string()), run.attempt_id, format!("{:?}", run.status).to_ascii_lowercase(), format!("{:?}", run.completion).to_ascii_lowercase(), format!("{:?}", run.technical).to_ascii_lowercase(), run.score, run.wall_time_ms, efficiency.and_then(|value| value.total_tokens), run.cost.total_usd, efficiency.and_then(|value| value.function_calls), efficiency.and_then(|value| value.function_call_errors), efficiency.map(|value| value.technical_attempts).unwrap_or(1), serde_json::to_string(&payload)?, serde_json::to_string(&payload)?]
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

fn local_suite_statement(suite: &LocalSuite) -> Result<Value> {
    let payload = serde_json::to_string(suite)?;
    let hash = crate::artifact::sha256_bytes(payload.as_bytes());
    Ok(
        json!({"sql": "INSERT INTO local_suites(id, updated_at, payload_json, payload_sha256) VALUES (?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET updated_at=excluded.updated_at, payload_json=excluded.payload_json, payload_sha256=excluded.payload_sha256", "params": [suite.id, suite.updated_at, payload, hash]}),
    )
}

fn local_stack_statement(stack: &LocalStack) -> Result<Value> {
    let payload = serde_json::to_string(stack)?;
    let hash = crate::artifact::sha256_bytes(payload.as_bytes());
    Ok(
        json!({"sql": "INSERT INTO local_stacks(id, updated_at, payload_json, payload_sha256) VALUES (?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET updated_at=excluded.updated_at, payload_json=excluded.payload_json, payload_sha256=excluded.payload_sha256", "params": [stack.id, stack.updated_at, payload, hash]}),
    )
}

fn saved_execution_statement(execution: &PlanExecution) -> Result<Value> {
    validate_saved_execution(execution)?;
    let payload = serde_json::to_string(execution)?;
    let hash = crate::artifact::sha256_bytes(payload.as_bytes());
    Ok(
        json!({"sql": "INSERT INTO saved_plan_executions(id, idempotency_key, state, started_at, updated_at, payload_json, payload_sha256) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET state=excluded.state, updated_at=excluded.updated_at, payload_json=excluded.payload_json, payload_sha256=excluded.payload_sha256", "params": [execution.id, execution.idempotency_key, execution.state, execution.started_at, execution.updated_at, payload, hash]}),
    )
}

fn validate_saved_execution(execution: &PlanExecution) -> Result<()> {
    crate::plans::store::validate_saved_execution(execution)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    use super::*;

    #[test]
    fn every_layout_statement_belongs_to_one_table() {
        let layouts = table_layouts();
        let statements = layouts
            .iter()
            .map(|layout| layout.statements.len())
            .sum::<usize>();
        assert_eq!(statements, SCHEMA.len());
        let names = layouts
            .iter()
            .map(|layout| layout.name.as_str())
            .collect::<Vec<_>>();
        for table in [
            "harness_e2e_storage",
            "executions",
            "runs",
            "local_suites",
            "local_stacks",
            "saved_plan_executions",
        ] {
            assert!(names.contains(&table), "{table}");
        }
        assert!(!names.contains(&"saved_plans"));
        let runs = layouts.iter().find(|layout| layout.name == "runs").unwrap();
        assert_eq!(runs.statements.len(), 2);
        assert!(runs.fingerprint().starts_with("sha256:"));
    }

    #[tokio::test]
    #[ignore = "requires HARNESS_E2E_TEST_DATABASE_URL and an isolated database worker"]
    async fn real_database_start_recreates_stale_tables() {
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
        let root = tempfile::tempdir().unwrap();
        let persistence = Persistence::new(client.clone(), "harness_e2e".into(), "default".into());
        persistence.initialize(root.path()).await.unwrap();
        let current = table_layouts()
            .into_iter()
            .map(|layout| (layout.name.clone(), layout.fingerprint()))
            .collect::<BTreeMap<_, _>>();
        // A moved `runs` layout and a legacy table from an older runner.
        persistence
            .transaction(vec![
                json!({"sql": "UPDATE harness_e2e_storage SET fingerprint = 'sha256:foreign' WHERE name = 'runs'", "params": []}),
                json!({"sql": "CREATE TABLE IF NOT EXISTS harness_e2e_schema (version INTEGER PRIMARY KEY)", "params": []}),
            ])
            .await
            .unwrap();
        persistence.initialize(root.path()).await.unwrap();
        let recorded = persistence
            .query(
                "SELECT name, fingerprint FROM harness_e2e_storage",
                json!([]),
            )
            .await
            .unwrap()
            .into_iter()
            .map(|row| {
                (
                    row["name"].as_str().unwrap().to_owned(),
                    row["fingerprint"].as_str().unwrap().to_owned(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(recorded, current);
        let present = persistence.present_tables().await.unwrap();
        assert!(present.contains("runs"));
        assert!(!present.contains("harness_e2e_schema"));
        client.shutdown_async().await;
    }

    /// Answers the database calls one start makes from `rows_for` and hands
    /// back the statements of its one transaction.
    async fn fake_database(
        rows_for: impl Fn(&str) -> Value + Send + 'static,
    ) -> (iii_sdk::IIIClient, tokio::task::JoinHandle<Vec<Value>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
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
                let transaction = message["function_id"] == DATABASE_TRANSACTION;
                let result = if transaction {
                    json!({"committed": true, "results": []})
                } else {
                    json!({"rows": rows_for(message["data"]["sql"].as_str().unwrap())})
                };
                socket
                    .send(Message::Text(
                        json!({
                            "type": "invocationresult",
                            "invocation_id": message["invocation_id"],
                            "function_id": message["function_id"],
                            "result": result,
                        })
                        .to_string(),
                    ))
                    .await
                    .unwrap();
                if transaction {
                    return message["data"]["statements"].as_array().unwrap().clone();
                }
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
        (client, server)
    }

    #[tokio::test]
    async fn a_record_of_a_removed_scenario_is_deleted_and_the_rest_listed() {
        let record = |id: &str, scenario: &str| {
            let body = json!({
                "execution_id": id, "idempotency_key": id, "phase": "completed",
                "requested_at": "2026-09-20T12:00:00Z", "updated_at": "2026-09-20T12:00:02Z",
                "request": {"idempotency_key": id, "model": "model", "provider": "provider", "scenarios": [scenario]},
                "lane_budget": {"max_cases": 1, "max_runs_per_case": 1, "max_technical_retries": 0, "max_declared_turns": 1},
                "transitions": [], "cancel_requested": false,
            })
            .to_string();
            json!({"execution_id": id, "record_sha256": crate::artifact::sha256_bytes(body.as_bytes()), "record_json": body})
        };
        let rows = json!([
            record("current", "context_pressure"),
            record("removed", "security_review")
        ]);
        let (client, server) = fake_database(move |_| rows.clone()).await;
        let records = Persistence::new(client.clone(), "harness_e2e".into(), "default".into())
            .executions()
            .await
            .unwrap();
        let statements = server.await.unwrap();
        client.shutdown_async().await;

        assert_eq!(
            records
                .iter()
                .map(|record| record.execution_id.as_str())
                .collect::<Vec<_>>(),
            vec!["current"]
        );
        assert_eq!(statements, delete_execution_statements("removed"));
    }

    #[tokio::test]
    async fn start_drops_plans_and_imported_history_and_keeps_suites_stacks_and_executions() {
        let suite = LocalSuite {
            id: "suite-0123456789ab".into(),
            label: "Mine".into(),
            scenarios: vec!["context_pressure".into()],
            repetitions: 1,
            technical_retries: 0,
            created_at: "2026-09-24T00:00:00Z".into(),
            updated_at: "2026-09-24T00:00:00Z".into(),
        };
        let stack = LocalStack {
            id: "stack-0123456789ab".into(),
            label: "Mine".into(),
            yaml: "containers: {}\n".into(),
            created_at: "2026-09-24T00:00:00Z".into(),
            updated_at: "2026-09-24T00:00:00Z".into(),
        };
        let row = |id: &str, body: String| json!({"id": id, "payload_sha256": crate::artifact::sha256_bytes(body.as_bytes()), "payload_json": body});
        let suites = json!([
            row(&suite.id, serde_json::to_string(&suite).unwrap()),
            row(
                "suite-unreadable",
                json!({"id": "suite-unreadable"}).to_string()
            ),
        ]);
        let stacks = json!([
            row(&stack.id, serde_json::to_string(&stack).unwrap()),
            row(
                "stack-unreadable",
                json!({"id": "stack-unreadable"}).to_string()
            ),
        ]);
        // An execution a saved plan ran, as the plan layout stored it: it
        // stays, without its plan and role.
        let executions = json!([
            row(
                "plan-receipt",
                json!({
                    "id": "plan-receipt", "plan_id": "plan-local", "idempotency_key": "key",
                    "configuration_sha256": "sha256:plan", "role": "baseline",
                    "state": "completed", "started_at": "2026-09-01T00:00:00Z",
                    "updated_at": "2026-09-01T00:10:00Z", "finished_at": "2026-09-01T00:10:00Z",
                    "cancel_requested": false, "error": null, "baseline_eligible": true,
                    "slots": [], "measurements": null,
                })
                .to_string()
            ),
            row(
                "remote-execution-rc",
                json!({"execution": {"record": {"id": "rc"}}, "source": {"instance_id": "rc"}})
                    .to_string()
            ),
        ]);
        let legacy = [
            "saved_plans",
            "history_campaigns",
            "history_reports",
            "history_runs",
            "history_execution_labels",
        ];
        let (client, server) = fake_database(move |sql| {
            let layouts = table_layouts();
            if sql.contains("sqlite_master") {
                json!(layouts
                    .iter()
                    .map(|layout| layout.name.as_str())
                    .chain(legacy)
                    .map(|name| json!({"name": name}))
                    .collect::<Vec<_>>())
            } else if sql == format!("SELECT * FROM {STORAGE_TABLE}") {
                json!(layouts
                    .iter()
                    .map(|layout| {
                        let moved = matches!(layout.name.as_str(), "local_suites" | "local_stacks" | "saved_plan_executions");
                        json!({"name": layout.name, "fingerprint": if moved { "sha256:before".into() } else { layout.fingerprint() }})
                    })
                    .chain(legacy.iter().map(|name| json!({"name": name, "fingerprint": "sha256:legacy"})))
                    .collect::<Vec<_>>())
            } else if sql == "SELECT * FROM local_suites" {
                suites.clone()
            } else if sql == "SELECT * FROM local_stacks" {
                stacks.clone()
            } else if sql == "SELECT * FROM saved_plan_executions" {
                executions.clone()
            } else {
                panic!("unexpected start query: {sql}")
            }
        })
        .await;
        let root = tempfile::tempdir().unwrap();
        let summary = Persistence::new(client.clone(), "harness_e2e".into(), "default".into())
            .initialize(root.path())
            .await
            .unwrap();
        let statements = server.await.unwrap();
        client.shutdown_async().await;

        assert_eq!(summary["legacy_dropped"], json!(legacy));
        assert_eq!(
            (
                &summary["suites"],
                &summary["stacks"],
                &summary["saved_executions"]
            ),
            (&json!(1), &json!(1), &json!(1))
        );
        assert_eq!(
            summary["dropped"],
            json!({"suites": 1, "stacks": 1, "saved_executions": 1})
        );
        let sql = |statement: &Value| statement["sql"].as_str().unwrap().to_owned();
        for table in legacy {
            assert!(statements
                .iter()
                .any(|statement| sql(statement) == format!("DROP TABLE IF EXISTS {table}")));
        }
        let inserted = statements
            .iter()
            .filter(|statement| {
                sql(statement).starts_with("INSERT INTO local_suites")
                    || sql(statement).starts_with("INSERT INTO local_stacks")
                    || sql(statement).starts_with("INSERT INTO saved_plan_executions")
            })
            .map(|statement| statement["params"][0].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            inserted,
            vec!["suite-0123456789ab", "stack-0123456789ab", "plan-receipt"]
        );
        let receipt = statements
            .iter()
            .find(|statement| sql(statement).starts_with("INSERT INTO saved_plan_executions"))
            .unwrap();
        let payload: Value = serde_json::from_str(receipt["params"][5].as_str().unwrap()).unwrap();
        assert!(payload.get("plan_id").is_none() && payload.get("role").is_none());
        assert!(statements.iter().any(|statement| sql(statement)
            .starts_with("DELETE FROM harness_e2e_storage WHERE name NOT IN")));
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
