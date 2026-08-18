//! Build a schema migration tool, then judge it by migrating a database it
//! has never seen: forward, backward, and forward twice.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::deliverable::workspace;
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

use super::repo;

pub const ID: &str = "build.migration_tool";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "migration_tool_system";
const ENTRYPOINT: &str = "migrate/migrate.py";
const SAMPLE_DATABASE: &str = "sample/orders.db";
const HELD_DATABASE: &str = "verification/orders.db";
const RUN_TIMEOUT: Duration = Duration::from_secs(90);
/// Rows the runner seeds into the held-out database: (id, customer, cents).
const SEEDED_ROWS: [(i64, &str, i64); 4] = [
    (1, "ana", 1250),
    (2, "bo", 40),
    (3, "cyd", 99_950),
    (4, "dee", 0),
];

const SYSTEM_RUNS: AssessmentSpec = AssessmentSpec::hard_gated(
    "system_runs",
    15,
    "The tool runs `up` and `down` from its documented entrypoint without failing.",
);
const FORWARD_CORRECT: AssessmentSpec = AssessmentSpec::hard_gated(
    "forward_correct",
    35,
    "After `up`, the held-out database has the new column, the backfilled values, and the index.",
);
const ROLLBACK_RESTORES: AssessmentSpec = AssessmentSpec::hard_gated(
    "rollback_restores",
    30,
    "After `down`, the schema and every original row are back as they were.",
);
const RERUN_IDEMPOTENT: AssessmentSpec = AssessmentSpec::hard_gated(
    "rerun_idempotent",
    20,
    "Running `up` twice leaves the same database as running it once.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    SYSTEM_RUNS,
    FORWARD_CORRECT,
    ROLLBACK_RESTORES,
    RERUN_IDEMPOTENT,
];

/// A python one-liner is the only reader the runner needs: sqlite3 ships with
/// the interpreter, so this adds no dependency to the crate.
const INSPECT: &str = r#"
import json, sqlite3, sys
connection = sqlite3.connect(sys.argv[1])
columns = [row[1] for row in connection.execute("PRAGMA table_info(orders)")]
indexes = sorted(row[1] for row in connection.execute("PRAGMA index_list(orders)"))
rows = [list(row) for row in connection.execute("SELECT id, customer, amount_cents FROM orders ORDER BY id")]
totals = []
if "amount_major" in columns:
    totals = [list(row) for row in connection.execute("SELECT id, amount_major FROM orders ORDER BY id")]
print(json.dumps({"columns": sorted(columns), "indexes": indexes, "rows": rows, "amount_major": totals}))
"#;

const SEED: &str = r#"
import sqlite3, sys
connection = sqlite3.connect(sys.argv[1])
connection.execute("CREATE TABLE orders (id INTEGER PRIMARY KEY, customer TEXT NOT NULL, amount_cents INTEGER NOT NULL)")
for row in json.loads(sys.argv[2]):
    connection.execute("INSERT INTO orders (id, customer, amount_cents) VALUES (?, ?, ?)", row)
connection.commit()
"#;

fn seed_program() -> String {
    format!("import json\n{SEED}")
}

fn rows_json() -> String {
    json!(SEEDED_ROWS
        .iter()
        .map(|(id, customer, cents)| json!([id, customer, cents]))
        .collect::<Vec<_>>())
    .to_string()
}

async fn seed_database(root: &Path, relative: &str) -> bool {
    if let Some(parent) = Path::new(relative).parent() {
        let _ = std::fs::create_dir_all(root.join(parent));
    }
    let _ = std::fs::remove_file(root.join(relative));
    repo::run(
        root,
        "python3",
        &["-c", &seed_program(), relative, &rows_json()],
        RUN_TIMEOUT,
    )
    .await
    .is_some_and(|run| run.status == Some(0))
}

async fn inspect(root: &Path, relative: &str) -> Option<Value> {
    repo::run(root, "python3", &["-c", INSPECT, relative], RUN_TIMEOUT)
        .await
        .and_then(|run| run.json())
}

fn expected_before() -> Value {
    json!({
        "columns": ["amount_cents", "customer", "id"],
        "rows": SEEDED_ROWS
            .iter()
            .map(|(id, customer, cents)| json!([id, customer, cents]))
            .collect::<Vec<_>>(),
    })
}

/// The backfill the migration must perform: cents rendered as major units.
fn expected_major() -> Value {
    json!(SEEDED_ROWS
        .iter()
        .map(|(id, _, cents)| json!([id, format!("{}.{:02}", cents / 100, cents % 100)]))
        .collect::<Vec<_>>())
}

fn setup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let root = workspace::root(ID, run_id);
        std::fs::create_dir_all(&root)?;
        seed_database(&root, SAMPLE_DATABASE).await;
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Build a schema migration tool in this workspace, then leave it ready to run. Take \
             as many turns as you need.\n\n\
             The database is SQLite. Its current schema is one table, `orders`, with columns \
             `id`, `customer`, and `amount_cents`.\n\n\
             The system:\n\
             1. `{ENTRYPOINT}` is the entrypoint. `python3 {ENTRYPOINT} up <database>` applies \
             the migration and `python3 {ENTRYPOINT} down <database>` reverses it. Both exit 0 on \
             success.\n\
             2. `up` adds a text column `amount_major` to `orders`, backfills it for every \
             existing row from `amount_cents` as major units with exactly two decimal places \
             (4 becomes `0.04`, 1250 becomes `12.50`), and creates an index named \
             `orders_customer_idx` on `customer`.\n\
             3. `down` reverses `up` exactly: the column and the index are gone and every \
             original row is unchanged.\n\
             4. Running `up` on an already-migrated database must leave it exactly as it was, \
             not fail and not double-apply. The same holds for `down` on an unmigrated one.\n\
             5. Use only the Python 3 standard library, including `sqlite3`.\n\n\
             A sample database is at `{SAMPLE_DATABASE}` with its own rows. It is a sample, not \
             the corpus: your tool will be run against a database you have not seen, with \
             different rows.\n\n\
             When the tool works, reply with exactly one line: `MIGRATION_READY`."
        ),
        filesystem_root: Some(workspace::root(ID, run_id)),
        execution: kit::policy(40, 600_000, 900),
        assessments: ASSESSMENTS,
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
    .spec()
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = super::case(
        ID,
        VERSION,
        seed,
        json!({
            "entrypoint": ENTRYPOINT,
            "sample_database": SAMPLE_DATABASE,
            "verification": {
                "held_database": HELD_DATABASE,
                "seeded_rows": SEEDED_ROWS.len(),
                "expected_major": expected_major(),
                "index": "orders_customer_idx",
            },
        }),
        super::system_profile(3, 5),
        &["iii::shell"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["after_up", "after_down", "response"],
                "additionalProperties": true
            }),
            ASSESSMENTS,
        ),
    )?;
    Ok(MaterializedScenario {
        spec: scenario(namespace),
        case,
        capture: Some(capture),
    })
}

struct Verification {
    after_up: Option<Value>,
    after_second_up: Option<Value>,
    after_down: Option<Value>,
    ran: bool,
}

impl Verification {
    fn forward_correct(&self) -> bool {
        let Some(after) = self.after_up.as_ref() else {
            return false;
        };
        let columns = after.get("columns").cloned().unwrap_or(Value::Null);
        let indexes = after
            .get("indexes")
            .and_then(Value::as_array)
            .map(|indexes| {
                indexes
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|index| index == "orders_customer_idx")
            })
            .unwrap_or(false);
        columns == json!(["amount_cents", "amount_major", "customer", "id"])
            && indexes
            && after.get("amount_major").cloned().unwrap_or(Value::Null) == expected_major()
            && after.get("rows").cloned().unwrap_or(Value::Null)
                == expected_before()
                    .get("rows")
                    .cloned()
                    .unwrap_or(Value::Null)
    }

    fn rollback_restores(&self) -> bool {
        let Some(after) = self.after_down.as_ref() else {
            return false;
        };
        after.get("columns").cloned().unwrap_or(Value::Null)
            == expected_before()
                .get("columns")
                .cloned()
                .unwrap_or(Value::Null)
            && after.get("rows").cloned().unwrap_or(Value::Null)
                == expected_before()
                    .get("rows")
                    .cloned()
                    .unwrap_or(Value::Null)
    }

    fn rerun_idempotent(&self) -> bool {
        self.after_up.is_some() && self.after_up == self.after_second_up
    }
}

/// One verification per attempt, shared by the evaluation and the captured
/// evidence. Re-running it would repeat the work and, where anything is
/// timed, answer differently the second time.
static VERIFIED: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<Verification>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

fn cached(run_id: &str) -> Option<std::sync::Arc<Verification>> {
    VERIFIED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(run_id)
        .cloned()
}

async fn verify(run_id: &str) -> std::sync::Arc<Verification> {
    if let Some(verification) = cached(run_id) {
        return verification;
    }
    let verification = std::sync::Arc::new(run_verification(run_id).await);
    VERIFIED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(run_id.to_string(), std::sync::Arc::clone(&verification));
    verification
}

fn forget_verification(run_id: &str) {
    VERIFIED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(run_id);
}

async fn run_verification(run_id: &str) -> Verification {
    let root = workspace::root(ID, run_id);
    if !seed_database(&root, HELD_DATABASE).await {
        return Verification {
            after_up: None,
            after_second_up: None,
            after_down: None,
            ran: false,
        };
    }

    let up = repo::run(
        &root,
        "python3",
        &[ENTRYPOINT, "up", HELD_DATABASE],
        RUN_TIMEOUT,
    )
    .await;
    let after_up = inspect(&root, HELD_DATABASE).await;
    let second = repo::run(
        &root,
        "python3",
        &[ENTRYPOINT, "up", HELD_DATABASE],
        RUN_TIMEOUT,
    )
    .await;
    let after_second_up = inspect(&root, HELD_DATABASE).await;
    let down = repo::run(
        &root,
        "python3",
        &[ENTRYPOINT, "down", HELD_DATABASE],
        RUN_TIMEOUT,
    )
    .await;
    let after_down = inspect(&root, HELD_DATABASE).await;

    Verification {
        ran: up.as_ref().is_some_and(|run| run.status == Some(0))
            && down.as_ref().is_some_and(|run| run.status == Some(0))
            && second.is_some(),
        after_up,
        after_second_up,
        after_down,
    }
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let verification = verify(run_id).await;

        Ok(assessment::build_evaluation([
            SYSTEM_RUNS.full_or_zero(
                verification.ran && observation.response.contains("MIGRATION_READY"),
                format!("both directions exited cleanly: {}", verification.ran),
            ),
            FORWARD_CORRECT.full_or_zero(
                verification.forward_correct(),
                format!("after up: {:?}", verification.after_up),
            ),
            ROLLBACK_RESTORES.full_or_zero(
                verification.rollback_restores(),
                format!("after down: {:?}", verification.after_down),
            ),
            RERUN_IDEMPOTENT.full_or_zero(
                verification.rerun_idempotent(),
                format!(
                    "second up matched the first: {}",
                    verification.rerun_idempotent()
                ),
            ),
        ]))
    })
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let verification = verify(run_id).await;
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "after_up": verification.after_up,
                "after_down": verification.after_down,
                "idempotent": verification.rerun_idempotent(),
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_migration_verification_before_cleanup",
            )],
        )])
    })
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        forget_verification(run_id);
        workspace::remove(&workspace::root(ID, run_id));
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backfill_renders_cents_as_major_units() {
        assert_eq!(
            expected_major(),
            json!([[1, "12.50"], [2, "0.40"], [3, "999.50"], [4, "0.00"]])
        );
    }

    #[test]
    fn the_pre_migration_shape_is_the_rollback_target() {
        assert_eq!(
            expected_before().get("columns").unwrap(),
            &json!(["amount_cents", "customer", "id"])
        );
    }
}
