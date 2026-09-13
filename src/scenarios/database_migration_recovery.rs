//! `database_migration_recovery` — resume an interrupted money-column
//! migration inside one idempotent `database::transaction`, prove it by
//! replaying the same transaction, and touch nothing outside the run-scoped
//! prefix. The capture reads every owned relation back before cleanup and the
//! evaluator scores that record together with the subject's write calls, for
//! scope and transaction discipline.

use anyhow::bail;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::CompletionState;

use super::assessment::{self, AssessmentSpec};
use super::{
    common, Capability, CapturedDeliverable, CleanupFuture, DeliverableCaptureFuture,
    DeliverableContract, EvaluationFuture, ExecutionPolicy, ProvenanceEvidence, Scenario,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "database_migration_recovery";
const DATABASE: &str = "primary";
const MIGRATION_ID: &str = "order-money-v2";
const EXPECTED_REPORT: &str = "MIGRATION-RECOVERED 5/1 REPLAY-2";
const DELIVERABLE_ID: &str = "migration_recovery_record";
const WRITE_FUNCTIONS: [&str; 3] = [
    "database::execute",
    "database::executeBatch",
    "database::transaction",
];

/// `(id, customer, total_text, status)` — the immutable legacy source.
const LEGACY_ROWS: [(i64, &str, &str, &str); 6] = [
    (101, "alpha", "10.50", "open"),
    (102, "beta", "7.25", "paid"),
    (103, "gamma", "N/A", "open"),
    (104, "delta", "0.99", "paid"),
    (105, "epsilon", "125.00", "open"),
    (106, "zeta", "12.00", "paid"),
];
/// `(id, customer, amount_cents, status, source_legacy_id)` after recovery.
const TARGET_ROWS: [(i64, &str, i64, &str, i64); 5] = [
    (101, "alpha", 1050, "open", 101),
    (102, "beta", 725, "paid", 102),
    (104, "delta", 99, "paid", 104),
    (105, "epsilon", 12500, "open", 105),
    (106, "zeta", 1200, "paid", 106),
];
const QUARANTINED_LEGACY_ID: i64 = 103;

const EXACT_MIGRATION_RESULT: AssessmentSpec = AssessmentSpec::scored(
    "exact_migration_result",
    40,
    "Five target rows with exact cents, one quarantine row for legacy id 103, and six compatibility rows with only 103 quarantined.",
);
const IDEMPOTENT_REPLAY: AssessmentSpec = AssessmentSpec::scored(
    "idempotent_replay",
    20,
    "The journal shows one complete order-money-v2 entry with replay_count=2, no duplicates, and exactly two successful database::transaction calls.",
);
const SOURCE_AND_SENTINEL_PRESERVED: AssessmentSpec = AssessmentSpec::scored(
    "source_and_sentinel_preserved",
    25,
    "The six legacy rows and the sentinel row are unchanged.",
);
const TRANSACTION_SCOPE_AND_REPORT: AssessmentSpec = AssessmentSpec::scored(
    "transaction_scope_and_report",
    15,
    "All subject writes were two database::transaction calls on prefixed relations only, and the final response is exactly the success report.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    EXACT_MIGRATION_RESULT,
    IDEMPOTENT_REPLAY,
    SOURCE_AND_SENTINEL_PRESERVED,
    TRANSACTION_SCOPE_AND_REPORT,
];

struct Relations {
    prefix: String,
    legacy: String,
    target: String,
    quarantine: String,
    journal: String,
    sentinel: String,
    compat: String,
}

impl Relations {
    fn new(run_id: &str) -> Self {
        let prefix = format!("e2e_{run_id}_");
        Self {
            legacy: format!("{prefix}legacy_orders"),
            target: format!("{prefix}orders_v2"),
            quarantine: format!("{prefix}orders_quarantine"),
            journal: format!("{prefix}migration_journal"),
            sentinel: format!("{prefix}sentinel"),
            compat: format!("{prefix}orders_compat"),
            prefix,
        }
    }

    fn drop_statements(&self) -> Vec<String> {
        vec![
            format!("DROP VIEW IF EXISTS {}", self.compat),
            format!("DROP TABLE IF EXISTS {}", self.target),
            format!("DROP TABLE IF EXISTS {}", self.quarantine),
            format!("DROP TABLE IF EXISTS {}", self.journal),
            format!("DROP TABLE IF EXISTS {}", self.legacy),
            format!("DROP TABLE IF EXISTS {}", self.sentinel),
        ]
    }

    fn setup_statements(&self) -> Vec<String> {
        let legacy_values = LEGACY_ROWS
            .iter()
            .map(|(id, customer, total, status)| {
                format!("({id}, '{customer}', '{total}', '{status}')")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let mut statements = self.drop_statements();
        statements.extend([
            format!(
                "CREATE TABLE {} (id INTEGER PRIMARY KEY, customer TEXT NOT NULL, total_text TEXT NOT NULL, status TEXT NOT NULL)",
                self.legacy
            ),
            format!(
                "CREATE TABLE {} (id INTEGER PRIMARY KEY, customer TEXT NOT NULL, amount_cents INTEGER NOT NULL CHECK(amount_cents >= 0), status TEXT NOT NULL, source_legacy_id INTEGER NOT NULL UNIQUE)",
                self.target
            ),
            format!(
                "CREATE TABLE {} (legacy_id INTEGER PRIMARY KEY, customer TEXT NOT NULL, raw_total TEXT NOT NULL, status TEXT NOT NULL, reason TEXT NOT NULL)",
                self.quarantine
            ),
            format!(
                "CREATE TABLE {} (migration_id TEXT PRIMARY KEY, status TEXT NOT NULL, applied_rows INTEGER NOT NULL, quarantined_rows INTEGER NOT NULL, replay_count INTEGER NOT NULL)",
                self.journal
            ),
            format!(
                "CREATE TABLE {} (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
                self.sentinel
            ),
            format!(
                "INSERT INTO {} (id, customer, total_text, status) VALUES {legacy_values}",
                self.legacy
            ),
            format!(
                "INSERT INTO {} (id, customer, amount_cents, status, source_legacy_id) VALUES (101, 'alpha', 1050, 'open', 101)",
                self.target
            ),
            format!(
                "INSERT INTO {} (migration_id, status, applied_rows, quarantined_rows, replay_count) VALUES ('{MIGRATION_ID}', 'backfill_in_progress', 1, 0, 0)",
                self.journal
            ),
            format!(
                "INSERT INTO {} (key, value) VALUES ('control', 'do-not-touch')",
                self.sentinel
            ),
        ]);
        statements
    }
}

pub struct DatabaseMigrationRecovery;

impl Scenario for DatabaseMigrationRecovery {
    fn id(&self) -> &'static str {
        ID
    }

    fn case(&self, seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "database": DATABASE,
                "migration_id": MIGRATION_ID,
                "legacy_rows": LEGACY_ROWS.iter().map(|(id, customer, total, status)| json!({
                    "id": id, "customer": customer, "total_text": total, "status": status
                })).collect::<Vec<_>>(),
                "expected_target_rows": TARGET_ROWS.iter().map(|(id, customer, cents, status, source)| json!({
                    "id": id, "customer": customer, "amount_cents": cents, "status": status, "source_legacy_id": source
                })).collect::<Vec<_>>(),
                "quarantined_legacy_id": QUARANTINED_LEGACY_ID,
                "expected_report": EXPECTED_REPORT,
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiFunctions,
                Capability::IiiDatabase,
            ],
            deliverable_contract(),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        scenario_for_case(run_id)
    }

    fn setup<'a>(&'a self, context: &'a E2eContext, run_id: &'a str) -> Option<CleanupFuture<'a>> {
        Some(setup(context, run_id))
    }

    fn capture<'a>(
        &'a self,
        context: &'a E2eContext,
        observation: &'a ScenarioObservation,
        run_id: &'a str,
    ) -> Option<DeliverableCaptureFuture<'a>> {
        Some(capture(context, observation, run_id))
    }

    fn evaluate<'a>(
        &'a self,
        context: &'a E2eContext,
        observation: &'a ScenarioObservation,
        run_id: &'a str,
    ) -> EvaluationFuture<'a> {
        evaluate(context, observation, run_id)
    }

    fn cleanup<'a>(
        &'a self,
        context: &'a E2eContext,
        run_id: &'a str,
    ) -> Option<CleanupFuture<'a>> {
        Some(cleanup(context, run_id))
    }
}

fn deliverable_contract() -> DeliverableContract {
    super::validation_loop::validation_contract(
        DELIVERABLE_ID,
        "database_record",
        json!({
            "type": "object",
            "required": ["target", "quarantine", "compat", "journal", "legacy", "sentinel", "response"],
            "additionalProperties": true
        }),
    )
}

/// Every owned relation as rows, or the function error when one is unreadable.
async fn owned_relations(
    context: &E2eContext,
    relations: &Relations,
) -> serde_json::Map<String, Value> {
    let mut content = serde_json::Map::new();
    for (label, sql) in [
        (
            "target",
            format!(
                "SELECT id, customer, amount_cents, status, source_legacy_id FROM {} ORDER BY id",
                relations.target
            ),
        ),
        (
            "quarantine",
            format!(
                "SELECT legacy_id, customer, raw_total, status, reason FROM {} ORDER BY legacy_id",
                relations.quarantine
            ),
        ),
        (
            "compat",
            format!(
                "SELECT id, customer, total_text, status, migration_status FROM {} ORDER BY id",
                relations.compat
            ),
        ),
        (
            "journal",
            format!(
                "SELECT migration_id, status, applied_rows, quarantined_rows, replay_count FROM {}",
                relations.journal
            ),
        ),
        (
            "legacy",
            format!(
                "SELECT id, customer, total_text, status FROM {} ORDER BY id",
                relations.legacy
            ),
        ),
        (
            "sentinel",
            format!("SELECT key, value FROM {}", relations.sentinel),
        ),
    ] {
        let value = match query_rows(context, sql).await {
            Ok(rows) => json!(rows),
            Err(error) => json!({ "error": error }),
        };
        content.insert(label.to_string(), value);
    }
    content
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let relations = Relations::new(run_id);
        let mut content = owned_relations(context, &relations).await;
        content.insert("response".to_string(), json!(observation.response));
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "database_record".to_string(),
            content: Value::Object(content).into(),
            invariants: Vec::new(),
            provenance: vec![
                ProvenanceEvidence {
                    kind: "database_prefix".to_string(),
                    source_id: format!("{DATABASE}/{}", relations.prefix),
                    relation: "captured_owned_relations".to_string(),
                },
                ProvenanceEvidence {
                    kind: "session".to_string(),
                    source_id: observation.metrics.root_session_id.clone(),
                    relation: "captured_subject_calls".to_string(),
                },
            ],
        }])
    })
}

fn scenario_for_case(run_id: &str) -> ScenarioSpec {
    let relations = Relations::new(run_id);
    ScenarioSpec {
        id: ID,
        prompt: format!(
            "Recover the interrupted `{MIGRATION_ID}` migration in database `{DATABASE}`, then \
             prove it is idempotent by replaying the same migration once.\n\n\
             The run-scoped relations already exist:\n\n\
             - `{legacy}`: six immutable legacy rows with columns `id`, `customer`, \
             `total_text`, and `status`.\n\
             - `{target}`: columns `id`, `customer`, `amount_cents`, `status`, and \
             `source_legacy_id`; legacy row 101 is already copied.\n\
             - `{quarantine}`: columns `legacy_id`, `customer`, `raw_total`, `status`, and \
             `reason`; initially empty.\n\
             - `{journal}`: columns `migration_id`, `status`, `applied_rows`, \
             `quarantined_rows`, and `replay_count`; it contains an incomplete \
             `{MIGRATION_ID}` entry.\n\
             - `{compat}`: the required compatibility view, not created yet.\n\
             - `{sentinel}`: columns `key` and `value`; unrelated state that must remain \
             `control=do-not-touch`.\n\n\
             Inspect `database::transaction` before using it. Perform one idempotent migration \
             transaction that:\n\n\
             1. preserves target row 101 and inserts every other valid decimal total exactly \
             once, converting dollars to integer cents;\n\
             2. quarantines legacy id 103 exactly once with reason `invalid_total`;\n\
             3. creates the compatibility view over all six legacy rows with columns `id`, \
             `customer`, `total_text`, `status`, and `migration_status`, where id 103 is \
             `quarantined` and every other row is `migrated`;\n\
             4. sets the journal to `status=complete`, `applied_rows=5`, `quarantined_rows=1`, \
             and increments `replay_count` by one.\n\n\
             Then execute that exact logical migration transaction a second time. Both passes \
             must use `database::transaction`; do not use `database::execute` or \
             `database::executeBatch` for subject writes. Re-read all six owned relations. The \
             final state must contain five target rows, one quarantine row, six compatibility \
             rows, `replay_count=2`, the unchanged six-row legacy source, and the unchanged \
             sentinel. Use no relation outside prefix `{prefix}`.\n\n\
             Finish with exactly `{EXPECTED_REPORT}` only if every verification succeeds; \
             otherwise report `FAIL` and the discrepancy.",
            legacy = relations.legacy,
            target = relations.target,
            quarantine = relations.quarantine,
            journal = relations.journal,
            compat = relations.compat,
            sentinel = relations.sentinel,
            prefix = relations.prefix,
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 24,
            max_output_tokens: Some(8_192),
            max_total_tokens: Some(200_000),
            stuck_timeout_seconds: 300,
            max_validation_retries: None,
        },
        denied_functions: &[],
        criteria: assessment::criteria(ASSESSMENTS),
    }
}

async fn execute_batch(context: &E2eContext, statements: Vec<String>) -> anyhow::Result<Value> {
    context
        .trigger_value(
            "database::executeBatch",
            json!({ "db": DATABASE, "statements": statements }),
        )
        .await
}

/// Rows of one query, or the function error text when the relation is
/// unreadable (dropped or never created). Errors are evidence here, not
/// infrastructure failures: the setup already proved the database works.
async fn query_rows(context: &E2eContext, sql: String) -> Result<Vec<Value>, String> {
    context
        .trigger_value("database::query", json!({ "db": DATABASE, "sql": sql }))
        .await
        .map(|response| {
            response
                .get("rows")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .map_err(|error| format!("{error:#}"))
}

async fn count(context: &E2eContext, table: &str) -> anyhow::Result<u64> {
    Ok(context
        .trigger_value(
            "database::query",
            json!({ "db": DATABASE, "sql": format!("SELECT COUNT(*) AS n FROM {table}") }),
        )
        .await?
        .pointer("/rows/0/n")
        .and_then(Value::as_u64)
        .unwrap_or(0))
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let relations = Relations::new(run_id);
        execute_batch(context, relations.setup_statements()).await?;
        let legacy = count(context, &relations.legacy).await?;
        let target = count(context, &relations.target).await?;
        let journal = query_rows(
            context,
            format!(
                "SELECT status, replay_count FROM {} WHERE migration_id = '{MIGRATION_ID}'",
                relations.journal
            ),
        )
        .await
        .map_err(anyhow::Error::msg)?;
        let sentinel = query_rows(
            context,
            format!("SELECT key, value FROM {}", relations.sentinel),
        )
        .await
        .map_err(anyhow::Error::msg)?;
        if legacy != 6
            || target != 1
            || journal != [json!({ "status": "backfill_in_progress", "replay_count": 0 })]
            || sentinel != [json!({ "key": "control", "value": "do-not-touch" })]
        {
            bail!(
                "database_migration_recovery prepared state mismatch: legacy={legacy}, target={target}, journal={journal:?}, sentinel={sentinel:?}"
            );
        }
        Ok(())
    })
}

fn text<'a>(row: &'a Value, column: &str) -> Option<&'a str> {
    row.get(column).and_then(Value::as_str)
}

fn integer(row: &Value, column: &str) -> Option<i64> {
    row.get(column).and_then(Value::as_i64)
}

fn target_rows_exact(rows: &[Value]) -> bool {
    rows.len() == TARGET_ROWS.len()
        && rows.iter().zip(TARGET_ROWS.iter()).all(
            |(row, (id, customer, cents, status, source))| {
                integer(row, "id") == Some(*id)
                    && text(row, "customer") == Some(customer)
                    && integer(row, "amount_cents") == Some(*cents)
                    && text(row, "status") == Some(status)
                    && integer(row, "source_legacy_id") == Some(*source)
            },
        )
}

fn quarantine_rows_exact(rows: &[Value]) -> bool {
    rows.len() == 1
        && integer(&rows[0], "legacy_id") == Some(QUARANTINED_LEGACY_ID)
        && text(&rows[0], "customer") == Some("gamma")
        && text(&rows[0], "raw_total") == Some("N/A")
        && text(&rows[0], "status") == Some("open")
        && text(&rows[0], "reason") == Some("invalid_total")
}

fn compat_rows_exact(rows: &[Value]) -> bool {
    rows.len() == LEGACY_ROWS.len()
        && rows
            .iter()
            .zip(LEGACY_ROWS.iter())
            .all(|(row, (id, customer, total, status))| {
                let expected_status = if *id == QUARANTINED_LEGACY_ID {
                    "quarantined"
                } else {
                    "migrated"
                };
                integer(row, "id") == Some(*id)
                    && text(row, "customer") == Some(customer)
                    && text(row, "total_text") == Some(total)
                    && text(row, "status") == Some(status)
                    && text(row, "migration_status") == Some(expected_status)
            })
}

fn legacy_rows_exact(rows: &[Value]) -> bool {
    rows.len() == LEGACY_ROWS.len()
        && rows
            .iter()
            .zip(LEGACY_ROWS.iter())
            .all(|(row, (id, customer, total, status))| {
                integer(row, "id") == Some(*id)
                    && text(row, "customer") == Some(customer)
                    && text(row, "total_text") == Some(total)
                    && text(row, "status") == Some(status)
            })
}

fn journal_rows_exact(rows: &[Value]) -> bool {
    rows.len() == 1
        && text(&rows[0], "migration_id") == Some(MIGRATION_ID)
        && text(&rows[0], "status") == Some("complete")
        && integer(&rows[0], "applied_rows") == Some(5)
        && integer(&rows[0], "quarantined_rows") == Some(1)
        && integer(&rows[0], "replay_count") == Some(2)
}

fn sentinel_rows_exact(rows: &[Value]) -> bool {
    rows.len() == 1
        && text(&rows[0], "key") == Some("control")
        && text(&rows[0], "value") == Some("do-not-touch")
}

/// SQL statements carried by one subject write call, regardless of surface.
fn write_statements(arguments: &Value) -> Vec<String> {
    if let Some(sql) = arguments.get("sql").and_then(Value::as_str) {
        return vec![sql.to_string()];
    }
    arguments
        .get("statements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|statement| {
            statement
                .as_str()
                .or_else(|| statement.get("sql").and_then(Value::as_str))
        })
        .map(str::to_string)
        .collect()
}

/// Relation names a statement mutates or reads that do not carry the run
/// prefix. Names are taken after the SQL keywords that introduce a relation
/// (`FROM`, `INTO`, `UPDATE`, `TABLE`, `VIEW`, `JOIN`), skipping the
/// existence modifiers and subqueries.
fn foreign_relations(sql: &str, prefix: &str) -> Vec<String> {
    const INTRODUCERS: [&str; 6] = ["FROM", "INTO", "UPDATE", "TABLE", "VIEW", "JOIN"];
    const MODIFIERS: [&str; 7] = ["IF", "NOT", "EXISTS", "OR", "REPLACE", "TEMP", "TEMPORARY"];
    let tokens: Vec<&str> = sql
        .split(|character: char| {
            character.is_whitespace() || matches!(character, '(' | ')' | ',' | ';')
        })
        .filter(|token| !token.is_empty())
        .collect();
    let mut foreign = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let keyword = tokens[index].to_ascii_uppercase();
        index += 1;
        if !INTRODUCERS.contains(&keyword.as_str()) {
            continue;
        }
        while index < tokens.len()
            && MODIFIERS.contains(&tokens[index].to_ascii_uppercase().as_str())
        {
            index += 1;
        }
        let Some(candidate) = tokens.get(index) else {
            break;
        };
        let name = candidate.trim_matches(|character| matches!(character, '`' | '"' | '\''));
        if name.eq_ignore_ascii_case("SELECT")
            || name.is_empty()
            || !name
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        {
            continue;
        }
        if !name.starts_with(prefix) && !name.eq_ignore_ascii_case("sqlite_master") {
            foreign.push(name.to_string());
        }
    }
    foreign
}

/// The owned relations exactly as `capture` stored them before cleanup. A
/// record is always present when the evaluator runs — capture precedes
/// evaluation and a failed capture aborts the attempt.
fn captured_relations(deliverables: &[CapturedDeliverable]) -> serde_json::Map<String, Value> {
    deliverables
        .iter()
        .find(|deliverable| deliverable.id == DELIVERABLE_ID)
        .and_then(|deliverable| deliverable.content.as_json())
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The rows captured for one relation, or the reason it was unreadable.
fn relation_rows(
    captured: &serde_json::Map<String, Value>,
    label: &str,
) -> Result<Vec<Value>, String> {
    match captured.get(label) {
        Some(Value::Array(rows)) => Ok(rows.clone()),
        Some(Value::Object(failure)) => Err(failure
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unreported error")
            .to_string()),
        _ => Err("relation was not captured".to_string()),
    }
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        if !observation.metrics.complete {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "metrics_complete",
                "subject metrics are incomplete",
            ));
        }
        let relations = Relations::new(run_id);
        // `capture` read every owned relation with these exact statements
        // immediately before this evaluation, so the record is the read: a
        // second round trip could only observe the same rows.
        let captured = captured_relations(&observation.deliverables);
        let mut notes = Vec::new();
        let mut read = |label: &'static str| match relation_rows(&captured, label) {
            Ok(rows) => rows,
            Err(error) => {
                notes.push(format!("{label} unreadable: {error}"));
                Vec::new()
            }
        };
        let target = read("target");
        let quarantine = read("quarantine");
        let compat = read("compat");
        let journal = read("journal");
        let legacy = read("legacy");
        let sentinel = read("sentinel");

        let target_ok = target_rows_exact(&target);
        let quarantine_ok = quarantine_rows_exact(&quarantine);
        let compat_ok = compat_rows_exact(&compat);
        let journal_ok = journal_rows_exact(&journal);
        let legacy_ok = legacy_rows_exact(&legacy);
        let sentinel_ok = sentinel_rows_exact(&sentinel);

        let calls: Vec<_> = common::function_outcomes(&observation.transcript)
            .into_iter()
            .filter(|call| !common::is_contract_discovery(&call.function_id))
            .collect();
        let writes: Vec<_> = calls
            .iter()
            .filter(|call| WRITE_FUNCTIONS.contains(&call.function_id.as_str()))
            .collect();
        let transactions = writes
            .iter()
            .filter(|call| call.function_id == "database::transaction")
            .count();
        let successful_transactions = writes
            .iter()
            .filter(|call| {
                call.function_id == "database::transaction" && call.is_error == Some(false)
            })
            .count();
        let non_transaction_writes = writes.len() - transactions;
        let foreign: Vec<String> = writes
            .iter()
            .flat_map(|call| write_statements(&call.arguments))
            .flat_map(|sql| foreign_relations(&sql, &relations.prefix))
            .collect();
        let reply = observation.response.trim();
        let reported = reply == EXPECTED_REPORT;

        let exact_migration = target_ok && quarantine_ok && compat_ok;
        let idempotent = journal_ok && successful_transactions == 2 && target_ok && quarantine_ok;
        let preserved = legacy_ok && sentinel_ok;
        let disciplined = !writes.is_empty()
            && non_transaction_writes == 0
            && transactions == 2
            && successful_transactions == 2
            && foreign.is_empty()
            && reported;

        Ok(assessment::build_evaluation(
            if journal_ok {
                CompletionState::Completed
            } else {
                CompletionState::TaskIncomplete
            },
            [
                EXACT_MIGRATION_RESULT.full_or_zero(
                    exact_migration,
                    format!(
                        "target_rows={target_ok}, quarantine_row={quarantine_ok}, compat_rows={compat_ok}; {}",
                        notes.join("; ")
                    ),
                ),
                IDEMPOTENT_REPLAY.full_or_zero(
                    idempotent,
                    format!(
                        "journal_complete_replay_2={journal_ok}, successful_transactions={successful_transactions}, target_rows={target_ok}, quarantine_row={quarantine_ok}"
                    ),
                ),
                SOURCE_AND_SENTINEL_PRESERVED.full_or_zero(
                    preserved,
                    format!("legacy_rows={legacy_ok}, sentinel_row={sentinel_ok}"),
                ),
                TRANSACTION_SCOPE_AND_REPORT.full_or_zero(
                    disciplined,
                    format!(
                        "write_calls={}, transactions={transactions}, successful_transactions={successful_transactions}, non_transaction_writes={non_transaction_writes}, foreign_relations={foreign:?}, exact_report={reported}",
                        writes.len()
                    ),
                ),
            ],
        ))
    })
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        execute_batch(context, Relations::new(run_id).drop_statements()).await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_statements_cover_every_owned_relation() {
        let relations = Relations::new("run");
        let statements = relations.setup_statements();
        assert_eq!(statements.len(), 15);
        for name in [
            &relations.legacy,
            &relations.target,
            &relations.quarantine,
            &relations.journal,
            &relations.sentinel,
        ] {
            assert!(
                statements
                    .iter()
                    .any(|sql| sql.starts_with(&format!("CREATE TABLE {name} "))),
                "{name}"
            );
        }
        crate::scenarios::ScenarioId::DatabaseMigrationRecovery
            .spec("run")
            .validate()
            .unwrap();
        crate::scenarios::ScenarioId::DatabaseMigrationRecovery
            .materialize("case", 9)
            .unwrap();
    }

    #[test]
    fn foreign_relations_flag_only_unprefixed_names() {
        let prefix = "e2e_run_";
        assert!(foreign_relations(
            "INSERT INTO e2e_run_orders_v2 (id) SELECT id FROM e2e_run_legacy_orders WHERE id NOT IN (SELECT source_legacy_id FROM e2e_run_orders_v2)",
            prefix
        )
        .is_empty());
        assert!(foreign_relations(
            "CREATE VIEW IF NOT EXISTS e2e_run_orders_compat AS SELECT l.id FROM e2e_run_legacy_orders l LEFT JOIN e2e_run_orders_v2 v ON v.source_legacy_id = l.id",
            prefix
        )
        .is_empty());
        assert_eq!(
            foreign_relations(
                "UPDATE orders SET status = 'x'; DELETE FROM `audit`",
                prefix
            ),
            vec!["orders".to_string(), "audit".to_string()]
        );
    }

    #[test]
    fn row_matchers_require_exact_content() {
        let target: Vec<Value> = TARGET_ROWS
            .iter()
            .map(|(id, customer, cents, status, source)| {
                json!({ "id": id, "customer": customer, "amount_cents": cents, "status": status, "source_legacy_id": source })
            })
            .collect();
        assert!(target_rows_exact(&target));
        let mut wrong = target.clone();
        wrong[1]["amount_cents"] = json!(7);
        assert!(!target_rows_exact(&wrong));
        assert!(journal_rows_exact(&[json!({
            "migration_id": MIGRATION_ID, "status": "complete", "applied_rows": 5, "quarantined_rows": 1, "replay_count": 2
        })]));
        assert!(!journal_rows_exact(&[json!({
            "migration_id": MIGRATION_ID, "status": "complete", "applied_rows": 5, "quarantined_rows": 1, "replay_count": 1
        })]));
        assert_eq!(
            write_statements(&json!({ "statements": ["A", { "sql": "B" }] })),
            vec!["A".to_string(), "B".to_string()]
        );
    }

    #[test]
    fn the_evaluator_reads_rows_and_failures_out_of_the_captured_record() {
        let rows = vec![json!({ "id": 1 })];
        let captured = captured_relations(&[CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "database_record".to_string(),
            content: json!({
                "target": rows,
                "journal": { "error": "no such table" },
                "response": EXPECTED_REPORT,
            })
            .into(),
            invariants: Vec::new(),
            provenance: Vec::new(),
        }]);
        assert_eq!(relation_rows(&captured, "target"), Ok(rows));
        assert_eq!(
            relation_rows(&captured, "journal"),
            Err("no such table".to_string())
        );
        assert_eq!(
            relation_rows(&captured, "legacy"),
            Err("relation was not captured".to_string())
        );
        assert!(relation_rows(&captured_relations(&[]), "target").is_err());
    }
}
