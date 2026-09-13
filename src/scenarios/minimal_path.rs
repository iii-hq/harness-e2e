//! `minimal_path` — one exact state write and nothing else. The task is the
//! cheapest possible unit of work; the score measures precision and friction
//! (extra calls, extra turns, errors) rather than capability.

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

pub const ID: &str = "minimal_path";
const KEY: &str = "minimal_result";
const OWNER: &str = "efficiency-suite";
const DONE_MARKER: &str = "MIN-DONE";
const DELIVERABLE_ID: &str = "minimal_result_record";

const EXACT_RESULT: AssessmentSpec = AssessmentSpec::scored(
    "exact_result",
    40,
    "The final value is exactly the requested object and exactly one successful state::set wrote it.",
);
const SINGLE_TASK_CALL: AssessmentSpec = AssessmentSpec::scored(
    "single_task_call",
    13,
    "The session made exactly one non-discovery function call: the required state::set.",
);
const AT_MOST_TWO_TASK_CALLS: AssessmentSpec = AssessmentSpec::scored(
    "at_most_two_task_calls",
    12,
    "No more than two non-discovery calls, all on the owned scope and key, including the required write.",
);
const SINGLE_TURN: AssessmentSpec = AssessmentSpec::scored(
    "single_turn",
    10,
    "The session completed in exactly one turn.",
);
const AT_MOST_TWO_TURNS: AssessmentSpec = AssessmentSpec::scored(
    "at_most_two_turns",
    10,
    "The session completed in no more than two turns.",
);
const ZERO_FRICTION: AssessmentSpec = AssessmentSpec::scored(
    "zero_friction",
    15,
    "No function-call errors, no call besides the single write, and one concise confirmation line containing MIN-DONE.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    EXACT_RESULT,
    SINGLE_TASK_CALL,
    AT_MOST_TWO_TASK_CALLS,
    SINGLE_TURN,
    AT_MOST_TWO_TURNS,
    ZERO_FRICTION,
];

fn scope(run_id: &str) -> String {
    format!("e2e-minimal-{run_id}")
}

fn baseline() -> Value {
    json!({ "owner": OWNER, "status": "prepared" })
}

fn expected(seed: u64) -> Value {
    json!({ "owner": OWNER, "case_seed": seed, "status": "stored" })
}

pub struct MinimalPath;

impl Scenario for MinimalPath {
    fn id(&self) -> &'static str {
        ID
    }

    fn case(&self, seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "key": KEY,
                "baseline": baseline(),
                "expected": expected(seed),
                "done_marker": DONE_MARKER,
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiFunctions,
                Capability::IiiState,
            ],
            deliverable_contract(),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        scenario_for_case(run_id, super::stable_seed(ID))
    }

    fn case_spec(&self, case: &ScenarioCase, run_id: &str) -> ScenarioSpec {
        scenario_for_case(run_id, case.seed)
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
        "state_record",
        json!({
            "type": "object",
            "required": ["state", "task_calls", "turns", "response"],
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
        let task_calls = common::function_calls(&observation.transcript)
            .into_iter()
            .filter(|call| !common::is_contract_discovery(&call.function_id))
            .map(|call| json!({ "function_id": call.function_id, "arguments": call.arguments }))
            .collect::<Vec<_>>();
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "state_record".to_string(),
            content: json!({
                "state": state,
                "task_calls": task_calls,
                "turns": observation.metrics.totals.turns,
                "response": observation.response,
            })
            .into(),
            invariants: Vec::new(),
            provenance: vec![
                ProvenanceEvidence {
                    kind: "state_location".to_string(),
                    source_id: format!("{scope}/{KEY}"),
                    relation: "captured_final_state".to_string(),
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

fn scenario_for_case(run_id: &str, seed: u64) -> ScenarioSpec {
    let scope = scope(run_id);
    let expected = serde_json::to_string(&expected(seed)).expect("serialize expected value");
    ScenarioSpec {
        id: ID,
        prompt: format!(
            "Store exactly `{expected}` in scope `{scope}` under key `{KEY}` using a single \
             `state::set` call. Then reply with one short confirmation line containing \
             `{DONE_MARKER}`. Make no other function call except function discovery if it is \
             necessary."
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
            bail!("minimal_path baseline was not established in {scope}/{KEY}: {stored}");
        }
        Ok(())
    })
}

/// The stored state the capture recorded for this run.
fn captured_state(observation: &ScenarioObservation) -> Option<Value> {
    observation
        .deliverables
        .iter()
        .find(|deliverable| deliverable.id == DELIVERABLE_ID)?
        .content
        .as_json()?
        .get("state")
        .cloned()
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
        let expected = expected(observation.case.seed);
        // The capture stored this same state value before cleanup; reuse it
        // instead of reading the scope a second time.
        let state = match captured_state(observation) {
            Some(state) => state,
            None => common::state_value(
                context
                    .trigger_value("state::get", json!({ "scope": scope, "key": KEY }))
                    .await?,
            ),
        };
        let calls: Vec<_> = common::function_outcomes(&observation.transcript)
            .into_iter()
            .filter(|call| !common::is_contract_discovery(&call.function_id))
            .collect();
        let exact_writes = calls
            .iter()
            .filter(|call| {
                call.function_id == "state::set"
                    && call.is_error == Some(false)
                    && call.arguments == json!({ "scope": scope, "key": KEY, "value": expected })
            })
            .count();
        let owned = calls.iter().all(|call| {
            call.arguments.get("scope").and_then(Value::as_str) == Some(scope.as_str())
                && call.arguments.get("key").and_then(Value::as_str) == Some(KEY)
        });
        let single = calls.len() == 1 && exact_writes == 1;
        let turns = observation.metrics.totals.turns;
        let errors = observation.metrics.totals.function_call_errors;
        let reply = observation.response.trim();
        let concise = reply.contains(DONE_MARKER) && reply.lines().count() == 1;
        let state_matches = state == expected;

        Ok(assessment::build_evaluation(
            if state_matches {
                CompletionState::Completed
            } else {
                CompletionState::TaskIncomplete
            },
            [
                EXACT_RESULT.full_or_zero(
                    state_matches && exact_writes == 1,
                    format!("state_matches={state_matches}, successful_exact_writes={exact_writes}"),
                ),
                SINGLE_TASK_CALL.full_or_zero(
                    single,
                    format!(
                        "task_calls={}, successful_exact_writes={exact_writes}",
                        calls.len()
                    ),
                ),
                AT_MOST_TWO_TASK_CALLS.full_or_zero(
                    calls.len() <= 2 && owned && exact_writes >= 1,
                    format!(
                        "task_calls={}, owned_targets={owned}, successful_exact_writes={exact_writes}",
                        calls.len()
                    ),
                ),
                SINGLE_TURN.full_or_zero(turns == 1, format!("turns={turns}")),
                AT_MOST_TWO_TURNS.full_or_zero((1..=2).contains(&turns), format!("turns={turns}")),
                ZERO_FRICTION.full_or_zero(
                    errors == 0 && single && concise,
                    format!(
                        "function_call_errors={errors}, single_task_call={single}, single_line_confirmation={concise}"
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
    fn materialized_case_carries_the_seeded_expected_value() {
        let materialized = crate::scenarios::ScenarioId::MinimalPath
            .materialize("case", 7)
            .unwrap();
        assert_eq!(materialized.case.scenario_id, ID);
        assert_eq!(materialized.case.seed, 7);
        assert_eq!(materialized.case.inputs["expected"]["case_seed"], 7);
        assert!(materialized.spec.prompt.contains("e2e-minimal-case"));
        assert!(materialized.spec.prompt.contains("\"case_seed\":7"));
    }

    #[test]
    fn criteria_weights_total_one_hundred() {
        let spec = MinimalPath.spec("run");
        spec.validate().unwrap();
        assert_eq!(spec.criteria.len(), ASSESSMENTS.len());
    }
}
