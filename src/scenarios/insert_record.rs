//! `insert_record` — one row into a prepared table through the database
//! function surface. The smallest database task; the evaluator reads the
//! table back and counts turns.

use anyhow::{bail, Context};
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
    "Does the prepared database table contain exactly one row with the requested text in its value field?",
);
const FEWER_THAN_TEN_TURNS: AssessmentSpec = AssessmentSpec::scored(
    "fewer_than_ten_turns",
    20,
    "Did the session use fewer than 10 turns?",
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

    fn title(&self) -> Option<&'static str> {
        Some("Insert Database Record")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Inserts one requested row into a prepared database table within a small turn budget.")
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
                stuck_timeout_seconds: 180,
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
        let rows = table_rows(context, &table).await?;
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
        let rows = match captured_rows(observation) {
            Some(captured) => captured,
            None => table_rows(context, &table).await?,
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
                RECORD_CREATED.full_or_zero(record_created, format!("rows={values:?}")),
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

async fn table_rows(context: &E2eContext, table: &str) -> anyhow::Result<Vec<Value>> {
    let queried = context
        .trigger_value(
            "database::query",
            json!({ "db": DATABASE, "sql": format!("SELECT id, value FROM {table} ORDER BY id") }),
        )
        .await;
    let result = match queried {
        Err(error)
            if matches!(
                error.downcast_ref::<iii_sdk::errors::Error>(),
                Some(iii_sdk::errors::Error::Handler(_) | iii_sdk::errors::Error::Remote { .. })
            ) =>
        {
            let absent = match context
                .trigger_value("database::listTables", json!({ "db": DATABASE }))
                .await
                .and_then(|catalog| table_absent(&catalog, table))
            {
                Ok(absent) => absent,
                Err(catalog_error) => {
                    return Err(error.context(format!("confirm table presence: {catalog_error:#}")))
                }
            };
            if !absent {
                return Err(error);
            }
            return Ok(Vec::new());
        }
        other => other?,
    };
    result
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .context("insert_record query did not return rows")
}

fn table_absent(catalog: &Value, table: &str) -> anyhow::Result<bool> {
    let names = catalog["tables"]
        .as_array()
        .context("database catalog did not return tables")?
        .iter()
        .map(|entry| {
            entry["name"]
                .as_str()
                .context("database catalog table name is missing")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(!names.contains(&table))
}

async fn execute(context: &E2eContext, sql: String) -> anyhow::Result<Value> {
    context
        .trigger_value("database::execute", json!({ "db": DATABASE, "sql": sql }))
        .await
}

fn captured_rows(observation: &ScenarioObservation) -> Option<Vec<Value>> {
    observation
        .deliverables
        .iter()
        .find(|deliverable| deliverable.id == DELIVERABLE_ID)?
        .content
        .as_json()?
        .get("rows")?
        .as_array()
        .cloned()
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

    #[test]
    fn only_a_valid_catalog_proves_table_absence() {
        let table = "e2e_insert_record_test";
        assert!(!table_absent(&json!({"tables":[{"name":table}]}), table).unwrap());
        assert!(table_absent(&json!({"tables":[{"name":"other"}]}), table).unwrap());
        assert!(table_absent(&json!({"tables":[]}), table).unwrap());
        for catalog in [json!({}), json!({"tables":{}}), json!({"tables":[{}]})] {
            assert!(table_absent(&catalog, table).is_err());
        }
    }

    #[tokio::test]
    async fn query_transport_failure_remains_unavailable() {
        let context = E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let error = table_rows(&context, "e2e_insert_record_test")
            .await
            .unwrap_err();
        assert_eq!(
            crate::context::transport_failure_code(&error),
            Some("transport_not_connected")
        );
        context.shutdown().await;
    }
}
