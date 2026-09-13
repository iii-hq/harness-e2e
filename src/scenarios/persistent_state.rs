//! `persistent_state` — read an existing record, migrate it in place with one
//! write, and preserve everything the migration does not own. Deterministic
//! checks compare the final value field by field against the authored
//! baseline and the expected result.

use anyhow::bail;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::CompletionState;

use super::assessment::{self, AssessmentSpec};
use super::{
    common, CapturedDeliverable, CleanupFuture, DeliverableCaptureFuture, DeliverableContract,
    EvaluationFuture, ExecutionPolicy, MaterializedScenario, ProvenanceEvidence, ScenarioCase,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "persistent_state";
const KEY: &str = "migration_record";
const OWNER: &str = "quality-suite";
const DELIVERABLE_ID: &str = "migration_record";

const EXACT_MIGRATED_STATE: AssessmentSpec = AssessmentSpec::scored(
    "exact_migrated_state",
    50,
    "The stored value exactly equals the migrated record with no missing or additional fields.",
);
const READ_THEN_WRITE_ONCE: AssessmentSpec = AssessmentSpec::scored(
    "read_then_write_once",
    25,
    "The subject read the owned key before exactly one successful state::set to it, with no errors and no writes elsewhere.",
);
const EXISTING_DATA_PRESERVED: AssessmentSpec = AssessmentSpec::scored(
    "existing_data_preserved",
    15,
    "The baseline owner, the alpha item, and the metadata survive unchanged while beta is completed and gamma appended once.",
);
const CONCISE_MIGRATION_CONFIRMATION: AssessmentSpec = AssessmentSpec::scored(
    "concise_migration_confirmation",
    10,
    "The final response is concise and states that revision 2 now contains 3 items.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    EXACT_MIGRATED_STATE,
    READ_THEN_WRITE_ONCE,
    EXISTING_DATA_PRESERVED,
    CONCISE_MIGRATION_CONFIRMATION,
];

fn scope(run_id: &str) -> String {
    format!("e2e-persistent-{run_id}")
}

fn metadata() -> Value {
    json!({ "schema_version": 1, "retention": "test-only" })
}

fn baseline() -> Value {
    json!({
        "owner": OWNER,
        "revision": 1,
        "status": "pending",
        "items": [
            { "id": "alpha", "completed": true },
            { "id": "beta", "completed": false }
        ],
        "metadata": metadata(),
    })
}

fn expected() -> Value {
    json!({
        "owner": OWNER,
        "revision": 2,
        "status": "migrated",
        "items": [
            { "id": "alpha", "completed": true },
            { "id": "beta", "completed": true },
            { "id": "gamma", "completed": true }
        ],
        "metadata": metadata(),
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        ID,
        seed,
        json!({
            "key": KEY,
            "baseline": baseline(),
            "expected": expected(),
        }),
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::state".to_string(),
        ],
        deliverable_contract(),
    )?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(namespace),
        case,
        capture: Some(capture),
    })
}

fn deliverable_contract() -> DeliverableContract {
    super::validation_loop::validation_contract(
        DELIVERABLE_ID,
        "state_record",
        json!({
            "type": "object",
            "required": ["state", "response"],
            "additionalProperties": true
        }),
    )
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let scope = scope(run_id);
        let state = common::state_value(
            context
                .trigger_value("state::get", json!({ "scope": scope, "key": KEY }))
                .await?,
        );
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "state_record".to_string(),
            content: json!({ "state": state, "response": observation.response }).into(),
            invariants: Vec::new(),
            provenance: vec![
                ProvenanceEvidence {
                    kind: "state_location".to_string(),
                    source_id: format!("{scope}/{KEY}"),
                    relation: "captured_migrated_record".to_string(),
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
    let scope = scope(run_id);
    ScenarioSpec {
        id: ID,
        prompt: format!(
            "Use `state::get` to read key `{KEY}` from scope `{scope}`. Migrate the stored \
             object with exactly one successful `state::set`: preserve `owner`, preserve the \
             existing `alpha` item, mark `beta` as completed, append \
             `{{\"id\":\"gamma\",\"completed\":true}}`, change `revision` to `2`, change \
             `status` to `migrated`, and preserve `metadata` unchanged. Do not write any other \
             scope or key. Then respond with a concise confirmation that includes the new \
             revision and total item count."
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 8,
            max_output_tokens: Some(4_096),
            max_total_tokens: Some(80_000),
            stuck_timeout_seconds: 180,
            max_validation_retries: None,
        },
        denied_functions: &[],
        criteria: assessment::criteria(ASSESSMENTS),
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let scope = scope(run_id);
        let _: Value = context
            .trigger_value(
                "state::set",
                json!({ "scope": scope, "key": KEY, "value": baseline() }),
            )
            .await?;
        let stored = common::state_value(
            context
                .trigger_value("state::get", json!({ "scope": scope, "key": KEY }))
                .await?,
        );
        if stored != baseline() {
            bail!("persistent_state baseline was not established in {scope}/{KEY}: {stored}");
        }
        Ok(())
    })
}

fn mentions_number(text: &str, expected: u64) -> bool {
    text.split(|character: char| !character.is_ascii_digit())
        .filter(|token| !token.is_empty())
        .any(|token| token.parse::<u64>().ok() == Some(expected))
}

fn targets_owned(arguments: &Value, scope: &str) -> bool {
    arguments.get("scope").and_then(Value::as_str) == Some(scope)
        && arguments.get("key").and_then(Value::as_str) == Some(KEY)
}

fn evaluate<'a>(
    context: &'a E2eContext,
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
        let scope = scope(run_id);
        let state = common::state_value(
            context
                .trigger_value("state::get", json!({ "scope": scope, "key": KEY }))
                .await?,
        );
        let calls: Vec<_> = common::function_outcomes(&observation.transcript)
            .into_iter()
            .filter(|call| !common::is_contract_discovery(&call.function_id))
            .collect();
        let first_read = calls.iter().position(|call| {
            call.function_id == "state::get"
                && call.is_error == Some(false)
                && targets_owned(&call.arguments, &scope)
        });
        let writes: Vec<usize> = calls
            .iter()
            .enumerate()
            .filter(|(_, call)| {
                call.function_id == "state::set"
                    && call.is_error == Some(false)
                    && targets_owned(&call.arguments, &scope)
            })
            .map(|(index, _)| index)
            .collect();
        let foreign_writes = calls
            .iter()
            .filter(|call| {
                call.function_id.starts_with("state::")
                    && call.function_id != "state::get"
                    && !targets_owned(&call.arguments, &scope)
            })
            .count();
        let call_errors = calls
            .iter()
            .filter(|call| call.is_error != Some(false))
            .count();
        let errors = observation.metrics.totals.function_call_errors;
        let read_then_write_once = matches!((first_read, writes.as_slice()), (Some(read), [write]) if read < *write)
            && foreign_writes == 0
            && call_errors == 0
            && errors == 0;

        let expected = expected();
        let exact = state == expected;
        let items = state
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let owner_preserved = state.get("owner").and_then(Value::as_str) == Some(OWNER);
        let alpha_preserved = items.first() == Some(&json!({ "id": "alpha", "completed": true }));
        let metadata_preserved = state.get("metadata") == Some(&metadata());
        let beta_completed = items
            .iter()
            .any(|item| item == &json!({ "id": "beta", "completed": true }));
        let gamma_once = items
            .iter()
            .filter(|item| item.get("id").and_then(Value::as_str) == Some("gamma"))
            .count()
            == 1;
        let preserved = owner_preserved
            && alpha_preserved
            && metadata_preserved
            && beta_completed
            && gamma_once;

        let reply = observation.response.trim();
        let concise = !reply.is_empty()
            && reply.lines().count() <= 3
            && reply.chars().count() <= 400
            && mentions_number(reply, 2)
            && mentions_number(reply, 3);

        Ok(assessment::build_evaluation(
            if exact {
                CompletionState::Completed
            } else {
                CompletionState::TaskIncomplete
            },
            [
                EXACT_MIGRATED_STATE.full_or_zero(exact, format!("state_matches={exact}")),
                READ_THEN_WRITE_ONCE.full_or_zero(
                    read_then_write_once,
                    format!(
                        "first_owned_read={first_read:?}, successful_owned_writes={writes:?}, foreign_state_writes={foreign_writes}, failed_calls={call_errors}, function_call_errors={errors}"
                    ),
                ),
                EXISTING_DATA_PRESERVED.full_or_zero(
                    preserved,
                    format!(
                        "owner={owner_preserved}, alpha={alpha_preserved}, metadata={metadata_preserved}, beta_completed={beta_completed}, gamma_once={gamma_once}"
                    ),
                ),
                CONCISE_MIGRATION_CONFIRMATION.full_or_zero(
                    concise,
                    format!(
                        "lines={}, chars={}, mentions_revision_2={}, mentions_3_items={}",
                        reply.lines().count(),
                        reply.chars().count(),
                        mentions_number(reply, 2),
                        mentions_number(reply, 3)
                    ),
                ),
            ],
        ))
    })
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let _: Value = context
            .trigger_value(
                "state::delete",
                json!({ "scope": scope(run_id), "key": KEY }),
            )
            .await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_record_is_the_migrated_baseline() {
        let baseline = baseline();
        let expected = expected();
        assert_eq!(baseline["owner"], expected["owner"]);
        assert_eq!(baseline["metadata"], expected["metadata"]);
        assert_eq!(baseline["items"][0], expected["items"][0]);
        assert_eq!(expected["revision"], 2);
        assert_eq!(expected["items"].as_array().unwrap().len(), 3);
        scenario("run").validate().unwrap();
        materialize("case", 3).unwrap();
    }

    #[test]
    fn number_mentions_match_whole_numbers_only() {
        assert!(mentions_number("Revision 2 now holds 3 items.", 2));
        assert!(mentions_number("Revision 2 now holds 3 items.", 3));
        assert!(!mentions_number("Revision 12 now holds 30 items.", 2));
        assert!(!mentions_number("Revision 12 now holds 30 items.", 3));
    }
}
