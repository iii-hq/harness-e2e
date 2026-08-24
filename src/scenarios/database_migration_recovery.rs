//! `database_migration_recovery` — finish and replay an interrupted schema
//! migration without losing legacy rows or touching an unrelated sentinel.
//!
//! Setup creates a run-scoped SQLite fixture in database `primary`: the v2
//! schema and migration journal exist, but only one valid legacy row has been
//! copied. The subject must finish the backfill, quarantine one invalid row,
//! create a compatibility view, and execute the same idempotent transaction a
//! second time. Capture and evaluation query the database independently.

use std::collections::BTreeSet;

use anyhow::Context;
use serde::Serialize;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::common;
use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture, ComplexityProfile,
    DeliverableCaptureFuture, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    InvariantSpec, MaterializedScenario, ObjectiveEvaluation, ProvenanceEvidence, ScenarioCase,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "database_migration_recovery";
const VERSION: u32 = 1;
pub const CANONICAL_SEED: u64 = 0x6462_6d69_6772_0001;
const DELIVERABLE_ID: &str = "database_migration_snapshot";
const DATABASE: &str = "primary";
const MIGRATION_ID: &str = "order-money-v2";
const REPORT_MARKER: &str = "MIGRATION-RECOVERED 5/1 REPLAY-2";

const EXACT_MIGRATION: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "exact_migration",
    40,
    "Five valid orders are represented exactly in v2, one invalid order is quarantined, and the compatibility view covers all six.",
    EvaluationDimension::Deliverable,
);
const IDEMPOTENT_REPLAY: AssessmentSpec = AssessmentSpec::hard_gated(
    "idempotent_replay",
    20,
    "The same migration transaction executes exactly twice and the journal records replay_count=2 without duplicate rows.",
);
const SOURCE_AND_SENTINEL_PRESERVED: AssessmentSpec = AssessmentSpec::hard_gated(
    "source_and_sentinel_preserved",
    25,
    "The legacy source remains exact and the unrelated sentinel is unchanged.",
);
const TRANSACTION_SCOPE: AssessmentSpec = AssessmentSpec::hard_gated(
    "transaction_scope",
    15,
    "All subject writes are two run-scoped database transactions; no external function or relation is mutated.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    EXACT_MIGRATION,
    IDEMPOTENT_REPLAY,
    SOURCE_AND_SENTINEL_PRESERVED,
    TRANSACTION_SCOPE,
];

#[derive(Debug, Clone)]
struct Names {
    prefix: String,
    legacy: String,
    v2: String,
    quarantine: String,
    journal: String,
    compat: String,
    sentinel: String,
}

impl Names {
    fn new(run_id: &str) -> Self {
        let prefix = format!("e2emig_{}", run_suffix(run_id));
        Self {
            legacy: format!("{prefix}_legacy_orders"),
            v2: format!("{prefix}_orders_v2"),
            quarantine: format!("{prefix}_orders_quarantine"),
            journal: format!("{prefix}_migration_journal"),
            compat: format!("{prefix}_orders_compat"),
            sentinel: format!("{prefix}_sentinel"),
            prefix,
        }
    }

    fn relations(&self) -> [&str; 6] {
        [
            &self.legacy,
            &self.v2,
            &self.quarantine,
            &self.journal,
            &self.compat,
            &self.sentinel,
        ]
    }
}

fn run_suffix(run_id: &str) -> String {
    format!(
        "{:016x}",
        super::stable_seed(&format!("{ID}:{run_id}:namespace"))
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct LegacyOrder {
    id: i64,
    customer: String,
    total_text: String,
    status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct MigratedOrder {
    id: i64,
    customer: String,
    amount_cents: i64,
    status: String,
    source_legacy_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct QuarantinedOrder {
    legacy_id: i64,
    customer: String,
    raw_total: String,
    status: String,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct JournalRow {
    migration_id: String,
    status: String,
    applied_rows: i64,
    quarantined_rows: i64,
    replay_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CompatOrder {
    id: i64,
    customer: String,
    total_text: String,
    status: String,
    migration_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SentinelRow {
    key: String,
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct DatabaseObject {
    kind: String,
    name: String,
}

#[derive(Debug, Clone, Default, Serialize)]
struct MigrationSnapshot {
    legacy: Vec<LegacyOrder>,
    v2: Vec<MigratedOrder>,
    quarantine: Vec<QuarantinedOrder>,
    journal: Vec<JournalRow>,
    compatibility: Vec<CompatOrder>,
    sentinel: Vec<SentinelRow>,
    objects: Vec<DatabaseObject>,
    v2_schema_sql: String,
    compatibility_view_present: bool,
}

#[derive(Debug, Clone, Copy)]
struct MigrationChecks {
    exact_rows: bool,
    row_conservation: bool,
    journal_complete: bool,
    compatibility_exact: bool,
    constraints_present: bool,
    object_inventory_exact: bool,
    source_preserved: bool,
    sentinel_preserved: bool,
}

impl MigrationChecks {
    fn exact_migration(self) -> bool {
        self.exact_rows
            && self.row_conservation
            && self.compatibility_exact
            && self.constraints_present
            && self.object_inventory_exact
    }
}

fn expected_legacy() -> Vec<LegacyOrder> {
    [
        (101, "alpha", "10.50", "open"),
        (102, "beta", "7.25", "paid"),
        (103, "gamma", "N/A", "open"),
        (104, "delta", "0.99", "paid"),
        (105, "epsilon", "125.00", "open"),
        (106, "zeta", "12.00", "paid"),
    ]
    .into_iter()
    .map(|(id, customer, total_text, status)| LegacyOrder {
        id,
        customer: customer.to_string(),
        total_text: total_text.to_string(),
        status: status.to_string(),
    })
    .collect()
}

fn expected_v2() -> Vec<MigratedOrder> {
    [
        (101, "alpha", 1_050, "open"),
        (102, "beta", 725, "paid"),
        (104, "delta", 99, "paid"),
        (105, "epsilon", 12_500, "open"),
        (106, "zeta", 1_200, "paid"),
    ]
    .into_iter()
    .map(|(id, customer, amount_cents, status)| MigratedOrder {
        id,
        customer: customer.to_string(),
        amount_cents,
        status: status.to_string(),
        source_legacy_id: id,
    })
    .collect()
}

fn expected_quarantine() -> Vec<QuarantinedOrder> {
    vec![QuarantinedOrder {
        legacy_id: 103,
        customer: "gamma".to_string(),
        raw_total: "N/A".to_string(),
        status: "open".to_string(),
        reason: "invalid_total".to_string(),
    }]
}

fn expected_compatibility() -> Vec<CompatOrder> {
    expected_legacy()
        .into_iter()
        .map(|row| CompatOrder {
            id: row.id,
            customer: row.customer,
            total_text: row.total_text,
            status: row.status,
            migration_status: if row.id == 103 {
                "quarantined".to_string()
            } else {
                "migrated".to_string()
            },
        })
        .collect()
}

fn expected_sentinel() -> Vec<SentinelRow> {
    vec![SentinelRow {
        key: "control".to_string(),
        value: "do-not-touch".to_string(),
    }]
}

fn normalized_sql(sql: &str) -> String {
    sql.chars()
        .filter(|character| !character.is_whitespace() && *character != '"' && *character != '`')
        .flat_map(char::to_lowercase)
        .collect()
}

fn check_snapshot(snapshot: &MigrationSnapshot) -> MigrationChecks {
    let migrated_ids = snapshot
        .v2
        .iter()
        .map(|row| row.source_legacy_id)
        .chain(snapshot.quarantine.iter().map(|row| row.legacy_id))
        .collect::<BTreeSet<_>>();
    let legacy_ids = snapshot
        .legacy
        .iter()
        .map(|row| row.id)
        .collect::<BTreeSet<_>>();
    let journal_complete = snapshot.journal
        == vec![JournalRow {
            migration_id: MIGRATION_ID.to_string(),
            status: "complete".to_string(),
            applied_rows: 5,
            quarantined_rows: 1,
            replay_count: 2,
        }];
    let schema = normalized_sql(&snapshot.v2_schema_sql);
    MigrationChecks {
        exact_rows: snapshot.v2 == expected_v2() && snapshot.quarantine == expected_quarantine(),
        row_conservation: migrated_ids == legacy_ids
            && snapshot.v2.len() + snapshot.quarantine.len() == snapshot.legacy.len(),
        journal_complete,
        compatibility_exact: snapshot.compatibility_view_present
            && snapshot.compatibility == expected_compatibility(),
        constraints_present: schema.contains("check(amount_cents>=0)")
            && schema.contains("source_legacy_idintegernotnullunique"),
        object_inventory_exact: snapshot.objects.len() == 6
            && snapshot
                .objects
                .iter()
                .filter(|object| object.kind == "table")
                .count()
                == 5
            && snapshot
                .objects
                .iter()
                .filter(|object| object.kind == "view")
                .count()
                == 1,
        source_preserved: snapshot.legacy == expected_legacy(),
        sentinel_preserved: snapshot.sentinel == expected_sentinel(),
    }
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn required_functions(_run_id: &str) -> Vec<String> {
    [
        "database::listDatabases",
        "database::query",
        "database::transaction",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub fn allowed_functions(run_id: &str) -> Vec<String> {
    let mut functions = required_functions(run_id);
    functions.extend([
        "engine::functions::list".to_string(),
        "engine::functions::info".to_string(),
    ]);
    functions
}

pub fn materialize(namespace: &str, _seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        ID,
        VERSION,
        CANONICAL_SEED,
        json!({
            "database": DATABASE,
            "migration_id": MIGRATION_ID,
            "legacy_rows": 6,
            "valid_rows": 5,
            "quarantined_rows": 1,
            "partial_state": "v2 schema exists with legacy row 101 already copied",
            "required_replays": 2,
            "report_marker": REPORT_MARKER,
        }),
        ComplexityProfile {
            planning_depth: 3,
            dependency_depth: 3,
            external_systems: 1,
            state_transitions: 5,
            validation_loops: 2,
            artifact_count: 1,
            ambiguity_level: 3,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::database".to_string(),
        ],
        deliverable_contract(),
    )?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(namespace),
        case,
        capture: Some(capture),
    })
}

fn scenario_for_case(run_id: &str) -> ScenarioSpec {
    let names = Names::new(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: prompt(&names),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 22,
            max_output_tokens: Some(12_288),
            max_total_tokens: Some(400_000),
            stuck_timeout_seconds: 420,
            max_validation_retries: None,
        },
        denied_functions: &[
            "state::*",
            "http::*",
            "shell::*",
            "coder::*",
            "harness::spawn",
        ],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
}

fn prompt(names: &Names) -> String {
    format!(
        r#"Recover the interrupted `{migration_id}` migration in database `primary`, then prove it is idempotent by replaying the same migration once.

The run-scoped relations already exist:
- legacy source `{legacy}`: `id`, `customer`, `total_text`, `status` (six rows; never mutate it)
- partial target `{v2}`: `id`, `customer`, `amount_cents`, `status`, `source_legacy_id` (one valid row is already copied)
- quarantine `{quarantine}`: `legacy_id`, `customer`, `raw_total`, `status`, `reason`
- journal `{journal}`: `migration_id`, `status`, `applied_rows`, `quarantined_rows`, `replay_count`
- required compatibility view `{compat}` (not created yet)
- unrelated sentinel `{sentinel}` (never mutate it)

Inspect `database::transaction` before use. Perform one idempotent migration transaction that:
1. preserves the existing target row and inserts every other valid decimal total exactly once, converting dollars to integer cents;
2. quarantines legacy id 103 exactly once with reason `invalid_total`;
3. creates `{compat}` over all six legacy rows with columns `id`, `customer`, `total_text`, `status`, `migration_status`, where id 103 is `quarantined` and the others are `migrated`;
4. sets journal `{migration_id}` to status `complete`, applied_rows 5, quarantined_rows 1, and increments `replay_count` by one.

Then execute that exact logical migration transaction a SECOND time. Both passes must use `database::transaction`; do not use `database::execute` or `database::executeBatch` for writes. Re-read the target, quarantine, compatibility view, journal, legacy source, and sentinel. There must still be 5 target rows, 1 quarantine row, 6 compatibility rows, replay_count 2, an unchanged legacy source, and sentinel `control=do-not-touch`.

Use no relation outside prefix `{prefix}`. Finish with exactly `{marker}` only if your verification proves every requirement; otherwise report FAIL and the discrepancy."#,
        migration_id = MIGRATION_ID,
        legacy = names.legacy,
        v2 = names.v2,
        quarantine = names.quarantine,
        journal = names.journal,
        compat = names.compat,
        sentinel = names.sentinel,
        prefix = names.prefix,
        marker = REPORT_MARKER,
    )
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        require_database(context).await?;
        let names = Names::new(run_id);
        drop_run_objects(context, &names).await?;
        for sql in reset_statements(&names) {
            execute(context, &sql).await?;
        }
        Ok(())
    })
}

fn reset_statements(names: &Names) -> Vec<String> {
    let mut statements = cleanup_statements(names);
    statements.extend([
        format!(
            "CREATE TABLE {legacy} (id INTEGER PRIMARY KEY, customer TEXT NOT NULL, total_text TEXT NOT NULL, status TEXT NOT NULL)",
            legacy = names.legacy
        ),
        format!(
            "CREATE TABLE {v2} (id INTEGER PRIMARY KEY, customer TEXT NOT NULL, amount_cents INTEGER NOT NULL CHECK(amount_cents >= 0), status TEXT NOT NULL, source_legacy_id INTEGER NOT NULL UNIQUE)",
            v2 = names.v2
        ),
        format!(
            "CREATE TABLE {quarantine} (legacy_id INTEGER PRIMARY KEY, customer TEXT NOT NULL, raw_total TEXT NOT NULL, status TEXT NOT NULL, reason TEXT NOT NULL)",
            quarantine = names.quarantine
        ),
        format!(
            "CREATE TABLE {journal} (migration_id TEXT PRIMARY KEY, status TEXT NOT NULL, applied_rows INTEGER NOT NULL, quarantined_rows INTEGER NOT NULL, replay_count INTEGER NOT NULL)",
            journal = names.journal
        ),
        format!(
            "CREATE TABLE {sentinel} (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            sentinel = names.sentinel
        ),
        format!(
            "INSERT INTO {legacy} (id, customer, total_text, status) VALUES (101, 'alpha', '10.50', 'open'), (102, 'beta', '7.25', 'paid'), (103, 'gamma', 'N/A', 'open'), (104, 'delta', '0.99', 'paid'), (105, 'epsilon', '125.00', 'open'), (106, 'zeta', '12.00', 'paid')",
            legacy = names.legacy
        ),
        format!(
            "INSERT INTO {v2} (id, customer, amount_cents, status, source_legacy_id) VALUES (101, 'alpha', 1050, 'open', 101)",
            v2 = names.v2
        ),
        format!(
            "INSERT INTO {journal} (migration_id, status, applied_rows, quarantined_rows, replay_count) VALUES ('{MIGRATION_ID}', 'backfill_in_progress', 1, 0, 0)",
            journal = names.journal
        ),
        format!(
            "INSERT INTO {sentinel} (key, value) VALUES ('control', 'do-not-touch')",
            sentinel = names.sentinel
        ),
    ]);
    statements
}

fn cleanup_statements(names: &Names) -> Vec<String> {
    vec![
        format!("DROP VIEW IF EXISTS {}", names.compat),
        format!("DROP TABLE IF EXISTS {}", names.v2),
        format!("DROP TABLE IF EXISTS {}", names.quarantine),
        format!("DROP TABLE IF EXISTS {}", names.journal),
        format!("DROP TABLE IF EXISTS {}", names.legacy),
        format!("DROP TABLE IF EXISTS {}", names.sentinel),
    ]
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        if let Err(error) = require_database(context).await {
            return Ok(missing_database(error.to_string()));
        }
        let names = Names::new(run_id);
        let snapshot = migration_snapshot(context, &names).await?;
        let checks = check_snapshot(&snapshot);
        let transaction_audit = audit_subject_calls(&observation.transcript, &names);
        let report_exact = observation.response.trim() == REPORT_MARKER;
        Ok(assessment::build_evaluation([
            EXACT_MIGRATION.full_or_zero(
                checks.exact_migration(),
                format!(
                    "v2={}, quarantine={}, compatibility={}, conserved={}, constraints={}, objects_exact={}",
                    snapshot.v2.len(),
                    snapshot.quarantine.len(),
                    snapshot.compatibility.len(),
                    checks.row_conservation,
                    checks.constraints_present,
                    checks.object_inventory_exact,
                ),
            ),
            IDEMPOTENT_REPLAY.full_or_zero(
                checks.journal_complete && transaction_audit.exact_two_transactions,
                format!(
                    "journal_complete={}, transaction_calls={}",
                    checks.journal_complete, transaction_audit.transaction_calls
                ),
            ),
            SOURCE_AND_SENTINEL_PRESERVED.full_or_zero(
                checks.source_preserved && checks.sentinel_preserved,
                format!(
                    "source_preserved={}, sentinel_preserved={}",
                    checks.source_preserved, checks.sentinel_preserved
                ),
            ),
            TRANSACTION_SCOPE.full_or_zero(
                transaction_audit.scoped && report_exact,
                format!(
                    "scoped_calls={}, write_calls={}, report_exact={report_exact}",
                    transaction_audit.scoped, transaction_audit.write_calls
                ),
            ),
        ]))
    })
}

#[derive(Debug, Clone, Copy)]
struct SubjectCallAudit {
    transaction_calls: usize,
    write_calls: usize,
    exact_two_transactions: bool,
    scoped: bool,
}

fn audit_subject_calls(transcript: &Value, names: &Names) -> SubjectCallAudit {
    let calls = common::function_calls(transcript);
    let transaction_calls = calls
        .iter()
        .filter(|call| call.function_id == "database::transaction")
        .count();
    let writes = calls
        .iter()
        .filter(|call| {
            matches!(
                call.function_id.as_str(),
                "database::execute" | "database::executeBatch" | "database::transaction"
            )
        })
        .collect::<Vec<_>>();
    let writes_scoped = writes.iter().all(|call| {
        let arguments = call.arguments.to_string();
        arguments.contains(&names.prefix)
            && !arguments.to_ascii_lowercase().contains("sqlite_master")
    });
    let functions_scoped = calls.iter().all(|call| {
        matches!(
            call.function_id.as_str(),
            "database::query" | "database::listDatabases" | "database::transaction"
        ) || call.function_id.starts_with("engine::functions::")
    });
    SubjectCallAudit {
        transaction_calls,
        write_calls: writes.len(),
        exact_two_transactions: transaction_calls == 2 && writes.len() == 2,
        scoped: writes_scoped && functions_scoped,
    }
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let names = Names::new(run_id);
        let snapshot = if require_database(context).await.is_ok() {
            migration_snapshot(context, &names).await?
        } else {
            MigrationSnapshot::default()
        };
        let checks = check_snapshot(&snapshot);
        let call_audit = audit_subject_calls(&observation.transcript, &names);
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "database_migration_snapshot".to_string(),
            content: json!({
                "relations": serde_json::to_value(&snapshot)?,
                "transaction_calls": call_audit.transaction_calls,
                "write_calls": call_audit.write_calls,
                "response": observation.response,
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: "exact_migration".to_string(),
                    passed: checks.exact_migration(),
                    reason: format!(
                        "v2={}, quarantine={}, compatibility={}",
                        snapshot.v2.len(),
                        snapshot.quarantine.len(),
                        snapshot.compatibility.len()
                    ),
                },
                CapturedInvariant {
                    id: "idempotent_replay".to_string(),
                    passed: checks.journal_complete && call_audit.exact_two_transactions,
                    reason: format!(
                        "journal_complete={}, transactions={}",
                        checks.journal_complete, call_audit.transaction_calls
                    ),
                },
                CapturedInvariant {
                    id: "source_and_sentinel_preserved".to_string(),
                    passed: checks.source_preserved && checks.sentinel_preserved,
                    reason: format!(
                        "source_preserved={}, sentinel_preserved={}",
                        checks.source_preserved, checks.sentinel_preserved
                    ),
                },
                CapturedInvariant {
                    id: "transaction_scope".to_string(),
                    passed: call_audit.scoped,
                    reason: format!("write_calls={}", call_audit.write_calls),
                },
            ],
            provenance: names
                .relations()
                .into_iter()
                .map(|relation| ProvenanceEvidence {
                    kind: "database_relation".to_string(),
                    source_id: format!("{DATABASE}/{relation}"),
                    relation: "captured_before_cleanup".to_string(),
                })
                .collect(),
        }])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "database_migration_snapshot".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["relations", "transaction_calls", "write_calls", "response"],
                "properties": {
                    "relations": { "type": "object" },
                    "transaction_calls": { "type": "integer", "minimum": 0 },
                    "write_calls": { "type": "integer", "minimum": 0 },
                    "response": { "type": "string" }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 131_072,
        }],
        invariants: ASSESSMENTS
            .iter()
            .map(|assessment| InvariantSpec {
                id: assessment.id().to_string(),
                description: assessment.description().to_string(),
            })
            .collect(),
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        if require_database(context).await.is_err() {
            return Ok(());
        }
        drop_run_objects(context, &Names::new(run_id)).await?;
        Ok(())
    })
}

async fn require_database(context: &E2eContext) -> anyhow::Result<()> {
    anyhow::ensure!(
        context.function_exists("database::query").await?,
        "database::query is unavailable"
    );
    anyhow::ensure!(
        context.function_exists("database::execute").await?,
        "database::execute is unavailable"
    );
    let databases = available_databases(context).await?;
    anyhow::ensure!(
        databases.contains(DATABASE),
        "database `primary` is unavailable; configured={databases:?}"
    );
    Ok(())
}

async fn available_databases(context: &E2eContext) -> anyhow::Result<BTreeSet<String>> {
    Ok(context
        .trigger_value("database::listDatabases", json!({}))
        .await?
        .get("databases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|database| {
            database
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect())
}

async fn execute(context: &E2eContext, sql: &str) -> anyhow::Result<()> {
    let _: Value = context
        .trigger("database::execute", json!({ "db": DATABASE, "sql": sql }))
        .await?;
    Ok(())
}

async fn query_rows(context: &E2eContext, sql: &str) -> anyhow::Result<Vec<Value>> {
    context
        .trigger_value("database::query", json!({ "db": DATABASE, "sql": sql }))
        .await?
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .with_context(|| format!("database::query returned malformed rows for {sql}"))
}

async fn relation_exists(context: &E2eContext, relation: &str, kind: &str) -> anyhow::Result<bool> {
    let rows = query_rows(
        context,
        &format!("SELECT name FROM sqlite_master WHERE type = '{kind}' AND name = '{relation}'"),
    )
    .await?;
    Ok(rows.len() == 1)
}

async fn migration_snapshot(
    context: &E2eContext,
    names: &Names,
) -> anyhow::Result<MigrationSnapshot> {
    let legacy = if relation_exists(context, &names.legacy, "table").await? {
        query_rows(
            context,
            &format!(
                "SELECT id, customer, total_text, status FROM {} ORDER BY id",
                names.legacy
            ),
        )
        .await?
        .iter()
        .map(parse_legacy)
        .collect::<anyhow::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let v2_present = relation_exists(context, &names.v2, "table").await?;
    let v2 = if v2_present {
        query_rows(
            context,
            &format!(
                "SELECT id, customer, amount_cents, status, source_legacy_id FROM {} ORDER BY source_legacy_id",
                names.v2
            ),
        )
        .await?
        .iter()
        .map(parse_migrated)
        .collect::<anyhow::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let quarantine = if relation_exists(context, &names.quarantine, "table").await? {
        query_rows(
            context,
            &format!(
                "SELECT legacy_id, customer, raw_total, status, reason FROM {} ORDER BY legacy_id",
                names.quarantine
            ),
        )
        .await?
        .iter()
        .map(parse_quarantine)
        .collect::<anyhow::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let journal = if relation_exists(context, &names.journal, "table").await? {
        query_rows(
            context,
            &format!(
                "SELECT migration_id, status, applied_rows, quarantined_rows, replay_count FROM {} ORDER BY migration_id",
                names.journal
            ),
        )
        .await?
        .iter()
        .map(parse_journal)
        .collect::<anyhow::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let compatibility_view_present = relation_exists(context, &names.compat, "view").await?;
    let compatibility = if compatibility_view_present {
        query_rows(
            context,
            &format!(
                "SELECT id, customer, total_text, status, migration_status FROM {} ORDER BY id",
                names.compat
            ),
        )
        .await?
        .iter()
        .map(parse_compat)
        .collect::<anyhow::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let sentinel = if relation_exists(context, &names.sentinel, "table").await? {
        query_rows(
            context,
            &format!("SELECT key, value FROM {} ORDER BY key", names.sentinel),
        )
        .await?
        .iter()
        .map(parse_sentinel)
        .collect::<anyhow::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let v2_schema_sql = if v2_present {
        query_rows(
            context,
            &format!(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = '{}'",
                names.v2
            ),
        )
        .await?
        .first()
        .and_then(|row| row.get("sql"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
    } else {
        String::new()
    };
    let objects = database_objects(context, names).await?;
    Ok(MigrationSnapshot {
        legacy,
        v2,
        quarantine,
        journal,
        compatibility,
        sentinel,
        objects,
        v2_schema_sql,
        compatibility_view_present,
    })
}

async fn database_objects(
    context: &E2eContext,
    names: &Names,
) -> anyhow::Result<Vec<DatabaseObject>> {
    query_rows(
        context,
        &format!(
            "SELECT type, name FROM sqlite_master WHERE substr(name, 1, {}) = '{}' AND type IN ('table', 'view', 'trigger', 'index') AND (type != 'index' OR sql IS NOT NULL) ORDER BY type, name",
            names.prefix.len(), names.prefix
        ),
    )
    .await?
    .iter()
    .map(|row| {
        Ok(DatabaseObject {
            kind: string(row, "type")?,
            name: string(row, "name")?,
        })
    })
    .collect()
}

async fn drop_run_objects(context: &E2eContext, names: &Names) -> anyhow::Result<()> {
    let mut objects = database_objects(context, names).await?;
    objects.sort_by_key(|object| match object.kind.as_str() {
        "trigger" => 0,
        "view" => 1,
        "index" => 2,
        "table" => 3,
        _ => 4,
    });
    for object in objects {
        if !object.name.starts_with(&names.prefix) || !sql_safe_name(&object.name) {
            anyhow::bail!("refusing to drop unsafe run object `{}`", object.name);
        }
        let kind = match object.kind.as_str() {
            "trigger" => "TRIGGER",
            "view" => "VIEW",
            "index" => "INDEX",
            "table" => "TABLE",
            other => anyhow::bail!("refusing to drop unsupported object kind `{other}`"),
        };
        execute(context, &format!("DROP {kind} IF EXISTS {}", object.name)).await?;
    }
    Ok(())
}

fn sql_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn parse_legacy(row: &Value) -> anyhow::Result<LegacyOrder> {
    Ok(LegacyOrder {
        id: integer(row, "id")?,
        customer: string(row, "customer")?,
        total_text: string(row, "total_text")?,
        status: string(row, "status")?,
    })
}

fn parse_migrated(row: &Value) -> anyhow::Result<MigratedOrder> {
    Ok(MigratedOrder {
        id: integer(row, "id")?,
        customer: string(row, "customer")?,
        amount_cents: integer(row, "amount_cents")?,
        status: string(row, "status")?,
        source_legacy_id: integer(row, "source_legacy_id")?,
    })
}

fn parse_quarantine(row: &Value) -> anyhow::Result<QuarantinedOrder> {
    Ok(QuarantinedOrder {
        legacy_id: integer(row, "legacy_id")?,
        customer: string(row, "customer")?,
        raw_total: string(row, "raw_total")?,
        status: string(row, "status")?,
        reason: string(row, "reason")?,
    })
}

fn parse_journal(row: &Value) -> anyhow::Result<JournalRow> {
    Ok(JournalRow {
        migration_id: string(row, "migration_id")?,
        status: string(row, "status")?,
        applied_rows: integer(row, "applied_rows")?,
        quarantined_rows: integer(row, "quarantined_rows")?,
        replay_count: integer(row, "replay_count")?,
    })
}

fn parse_compat(row: &Value) -> anyhow::Result<CompatOrder> {
    Ok(CompatOrder {
        id: integer(row, "id")?,
        customer: string(row, "customer")?,
        total_text: string(row, "total_text")?,
        status: string(row, "status")?,
        migration_status: string(row, "migration_status")?,
    })
}

fn parse_sentinel(row: &Value) -> anyhow::Result<SentinelRow> {
    Ok(SentinelRow {
        key: string(row, "key")?,
        value: string(row, "value")?,
    })
}

fn string(row: &Value, field: &str) -> anyhow::Result<String> {
    row.get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("row is missing text field {field}"))
}

fn integer(row: &Value, field: &str) -> anyhow::Result<i64> {
    row.get(field)
        .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
        .with_context(|| format!("row is missing integer field {field}"))
}

fn missing_database(reason: String) -> ObjectiveEvaluation {
    assessment::prerequisite_failure(ASSESSMENTS, "primary_database_available", reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_snapshot() -> MigrationSnapshot {
        MigrationSnapshot {
            legacy: expected_legacy(),
            v2: expected_v2(),
            quarantine: expected_quarantine(),
            journal: vec![JournalRow {
                migration_id: MIGRATION_ID.to_string(),
                status: "complete".to_string(),
                applied_rows: 5,
                quarantined_rows: 1,
                replay_count: 2,
            }],
            compatibility: expected_compatibility(),
            sentinel: expected_sentinel(),
            objects: vec![
                DatabaseObject { kind: "table".to_string(), name: "legacy".to_string() },
                DatabaseObject { kind: "table".to_string(), name: "v2".to_string() },
                DatabaseObject { kind: "table".to_string(), name: "quarantine".to_string() },
                DatabaseObject { kind: "table".to_string(), name: "journal".to_string() },
                DatabaseObject { kind: "table".to_string(), name: "sentinel".to_string() },
                DatabaseObject { kind: "view".to_string(), name: "compat".to_string() },
            ],
            v2_schema_sql: "CREATE TABLE orders_v2 (id INTEGER PRIMARY KEY, customer TEXT NOT NULL, amount_cents INTEGER NOT NULL CHECK(amount_cents >= 0), status TEXT NOT NULL, source_legacy_id INTEGER NOT NULL UNIQUE)".to_string(),
            compatibility_view_present: true,
        }
    }

    #[test]
    fn complete_snapshot_passes_every_independent_check() {
        let checks = check_snapshot(&complete_snapshot());
        assert!(checks.exact_migration());
        assert!(checks.journal_complete);
        assert!(checks.source_preserved);
        assert!(checks.sentinel_preserved);
    }

    #[test]
    fn duplicate_or_missing_rows_fail_conservation_and_exactness() {
        let mut duplicate = complete_snapshot();
        duplicate.v2.push(duplicate.v2[0].clone());
        let checks = check_snapshot(&duplicate);
        assert!(!checks.exact_rows);
        assert!(!checks.row_conservation);

        let mut missing = complete_snapshot();
        missing.quarantine.clear();
        let checks = check_snapshot(&missing);
        assert!(!checks.exact_rows);
        assert!(!checks.row_conservation);
    }

    #[test]
    fn legacy_sentinel_and_constraint_mutants_are_detected() {
        let mut source_changed = complete_snapshot();
        source_changed.legacy[0].total_text = "99.00".to_string();
        assert!(!check_snapshot(&source_changed).source_preserved);

        let mut sentinel_changed = complete_snapshot();
        sentinel_changed.sentinel[0].value = "changed".to_string();
        assert!(!check_snapshot(&sentinel_changed).sentinel_preserved);

        let mut constraint_removed = complete_snapshot();
        constraint_removed.v2_schema_sql =
            "CREATE TABLE orders_v2 (amount_cents INTEGER, source_legacy_id INTEGER)".to_string();
        assert!(!check_snapshot(&constraint_removed).constraints_present);

        let mut extra_object = complete_snapshot();
        extra_object.objects.push(DatabaseObject {
            kind: "table".to_string(),
            name: "temporary_copy".to_string(),
        });
        assert!(!check_snapshot(&extra_object).object_inventory_exact);
    }

    #[test]
    fn replay_requires_complete_journal_with_exact_count() {
        for replay_count in [0, 1, 3] {
            let mut snapshot = complete_snapshot();
            snapshot.journal[0].replay_count = replay_count;
            assert!(!check_snapshot(&snapshot).journal_complete);
        }
        let mut snapshot = complete_snapshot();
        snapshot.journal[0].status = "backfill_in_progress".to_string();
        assert!(!check_snapshot(&snapshot).journal_complete);
    }

    fn transcript_of(calls: &[(&str, Value)]) -> Value {
        let content = calls
            .iter()
            .enumerate()
            .map(|(index, (function_id, arguments))| {
                json!({
                    "type": "function_call",
                    "id": format!("call-{index}"),
                    "function_id": function_id,
                    "arguments": arguments,
                })
            })
            .collect::<Vec<_>>();
        json!({
            "messages": [
                { "message": { "role": "assistant", "content": content } }
            ]
        })
    }

    #[test]
    fn call_audit_requires_exactly_two_scoped_transactions() {
        let names = Names::new("attempt-a");
        let args = json!({
            "db": DATABASE,
            "operations": [{ "sql": format!("INSERT OR IGNORE INTO {} SELECT * FROM {}", names.v2, names.legacy) }]
        });
        let transcript = transcript_of(&[
            ("database::transaction", args.clone()),
            ("database::transaction", args),
            (
                "database::query",
                json!({ "db": DATABASE, "sql": format!("SELECT * FROM {}", names.v2) }),
            ),
        ]);
        let audit = audit_subject_calls(&transcript, &names);
        assert!(audit.exact_two_transactions);
        assert!(audit.scoped);

        let extra_write = transcript_of(&[
            (
                "database::transaction",
                json!({ "sql": format!("UPDATE {} SET status='complete'", names.journal) }),
            ),
            (
                "database::transaction",
                json!({ "sql": format!("UPDATE {} SET status='complete'", names.journal) }),
            ),
            (
                "database::execute",
                json!({ "sql": format!("DELETE FROM {}", names.v2) }),
            ),
        ]);
        let audit = audit_subject_calls(&extra_write, &names);
        assert!(!audit.exact_two_transactions);
        assert!(
            !audit.scoped,
            "database::execute is outside the allowed subject surface"
        );
    }

    #[test]
    fn setup_and_cleanup_statements_are_run_scoped_and_resettable() {
        let names = Names::new("attempt-a");
        let reset = reset_statements(&names);
        let cleanup = cleanup_statements(&names);
        assert!(reset.iter().all(|sql| sql.contains(&names.prefix)));
        assert!(cleanup.iter().all(|sql| sql.contains(&names.prefix)));
        assert_eq!(&reset[..cleanup.len()], cleanup.as_slice());
        assert!(cleanup.iter().all(|sql| sql.contains("IF EXISTS")));
    }

    #[test]
    fn materialized_case_is_canonical_and_valid() {
        let first = materialize("attempt-a", 43).unwrap();
        let retry = materialize("attempt-b", 43).unwrap();
        first.validate().unwrap();
        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs, retry.case.inputs);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert_eq!(first.case.seed, CANONICAL_SEED);
        assert_ne!(first.spec.prompt, retry.spec.prompt);
        assert_eq!(first.case.deliverable_contract.artifacts.len(), 1);
        assert!(first.case.deliverable_contract.capture_before_cleanup);
    }
}
