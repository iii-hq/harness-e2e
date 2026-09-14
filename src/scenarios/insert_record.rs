//! `insert_record` — one row into a prepared table through the database
//! function surface. The smallest database task; the evaluator reads the
//! table back and counts turns.

use anyhow::bail;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::CompletionState;

use super::assessment::{self, AssessmentSpec};
use super::{
    async_trait, Capability, CapturedDeliverable, DeliverableContract, ExecutionPolicy,
    ObjectiveEvaluation, ProvenanceEvidence, Scenario, ScenarioCase, ScenarioObservation,
    ScenarioSpec,
};

pub const ID: &str = "insert_record";
const DATABASE: &str = "primary";
const VALUE: &str = "harness-e2e-record";
const MAX_TURNS_FOR_CREDIT: u64 = 10;
const DELIVERABLE_ID: &str = "inserted_rows";

const RECORD_CREATED: AssessmentSpec = AssessmentSpec::scored(
    "record_created",
    80,
    "The table contains exactly one row whose value field is the requested text.",
);
const FEWER_THAN_TEN_TURNS: AssessmentSpec = AssessmentSpec::scored(
    "fewer_than_ten_turns",
    20,
    "The evaluated session used fewer than 10 turns.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[RECORD_CREATED, FEWER_THAN_TEN_TURNS];

fn table(run_id: &str) -> String {
    format!("e2e_insert_record_{run_id}")
}

pub struct InsertRecord;

#[async_trait]
impl Scenario for InsertRecord {
    fn id(&self) -> &'static str {
        ID
    }

    fn case(&self, seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({ "database": DATABASE, "value": VALUE }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiFunctions,
                Capability::IiiDatabase,
            ],
            deliverable_contract(),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let table = table(run_id);
        ScenarioSpec {
            id: ID,
            prompt: format!(
                "Use `database::execute` on the `{DATABASE}` database to add exactly one row to the \
                 `{table}` table with the text value `{VALUE}`. Then respond with a short \
                 confirmation."
            ),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(12),
                max_output_tokens: Some(4_096),
                max_total_tokens: Some(80_000),
                stuck_timeout_seconds: Some(180),
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        let table = table(run_id);
        execute(context, format!("DROP TABLE IF EXISTS {table}")).await?;
        execute(
            context,
            format!("CREATE TABLE {table} (id INTEGER PRIMARY KEY AUTOINCREMENT, value TEXT)"),
        )
        .await?;
        let count = context
            .trigger_value(
                "database::query",
                json!({ "db": DATABASE, "sql": format!("SELECT COUNT(*) AS n FROM {table}") }),
            )
            .await?
            .pointer("/rows/0/n")
            .and_then(Value::as_u64);
        if count != Some(0) {
            bail!("insert_record table {table} was not prepared empty: count={count:?}");
        }
        Ok(())
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let table = table(run_id);
        let rows = match table_rows(context, &table).await {
            Ok(rows) => json!(rows),
            Err(error) => json!({ "error": error }),
        };
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "database_record".to_string(),
            content: json!({ "rows": rows, "response": observation.response }).into(),
            invariants: Vec::new(),
            provenance: vec![
                ProvenanceEvidence {
                    kind: "database_table".to_string(),
                    source_id: format!("{DATABASE}/{table}"),
                    relation: "captured_final_rows".to_string(),
                },
                ProvenanceEvidence {
                    kind: "session".to_string(),
                    source_id: observation.metrics.root_session_id.clone(),
                    relation: "captured_subject_calls".to_string(),
                },
            ],
        }])
    }

    async fn evaluate(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        if !observation.metrics.complete {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "metrics_complete",
                "subject metrics are incomplete",
            ));
        }
        let table = table(run_id);
        // The capture stored the rows of this same table before cleanup; reuse
        // them instead of querying the database a second time.
        let (rows, query_error) = match captured_rows(observation) {
            Some(captured) => captured,
            None => match table_rows(context, &table).await {
                Ok(rows) => (rows, None),
                Err(error) => (Vec::new(), Some(error)),
            },
        };
        let values: Vec<Option<&str>> = rows
            .iter()
            .map(|row| row.get("value").and_then(Value::as_str))
            .collect();
        let record_created = values.as_slice() == [Some(VALUE)];
        let turns = observation.metrics.totals.turns;

        Ok(assessment::build_evaluation(
            if record_created {
                CompletionState::Completed
            } else {
                CompletionState::TaskIncomplete
            },
            [
                RECORD_CREATED.full_or_zero(
                    record_created,
                    match query_error {
                        Some(error) => format!("rows unavailable: {error}"),
                        None => format!("rows={values:?}"),
                    },
                ),
                FEWER_THAN_TEN_TURNS
                    .full_or_zero(turns < MAX_TURNS_FOR_CREDIT, format!("turns={turns}")),
            ],
        ))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        execute(context, format!("DROP TABLE IF EXISTS {}", table(run_id))).await?;
        Ok(())
    }
}

fn deliverable_contract() -> DeliverableContract {
    super::validation_loop::validation_contract(
        DELIVERABLE_ID,
        "database_record",
        json!({
            "type": "object",
            "required": ["rows", "response"],
            "additionalProperties": true
        }),
    )
}

/// Rows of the owned table, or the function error when it is unreadable.
async fn table_rows(context: &E2eContext, table: &str) -> Result<Vec<Value>, String> {
    context
        .trigger_value(
            "database::query",
            json!({ "db": DATABASE, "sql": format!("SELECT id, value FROM {table} ORDER BY id") }),
        )
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

async fn execute(context: &E2eContext, sql: String) -> anyhow::Result<Value> {
    context
        .trigger_value("database::execute", json!({ "db": DATABASE, "sql": sql }))
        .await
}

/// The rows the capture stored for this run, as `(rows, query_error)`.
fn captured_rows(observation: &ScenarioObservation) -> Option<(Vec<Value>, Option<String>)> {
    let content = observation
        .deliverables
        .iter()
        .find(|deliverable| deliverable.id == DELIVERABLE_ID)?
        .content
        .as_json()?
        .get("rows")?
        .clone();
    match content {
        Value::Array(rows) => Some((rows, None)),
        other => Some((
            Vec::new(),
            Some(
                other
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("rows unavailable")
                    .to_string(),
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_names_the_run_scoped_table() {
        let spec = InsertRecord.spec("abc123");
        spec.validate().unwrap();
        assert!(spec.prompt.contains("e2e_insert_record_abc123"));
        assert!(spec.prompt.contains(VALUE));
        crate::scenarios::ScenarioId::InsertRecord
            .materialize("case", 5)
            .unwrap();
    }
}
