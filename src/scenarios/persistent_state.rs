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

pub const ID: &str = "persistent_state";
const VERSION: u32 = 4;
const DELIVERABLE_ID: &str = "stored_state";
const KEY: &str = "persistent_state";
const DURABLE_RESULT: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "durable_result",
    60,
    "The exact requested JSON is present at the requested state key.",
    EvaluationDimension::Deliverable,
);
const FUNCTION_DISCIPLINE: AssessmentSpec = AssessmentSpec::hard_gated(
    "function_discipline",
    30,
    "Exactly one successful write targets the requested scope and key.",
);
const CONFIRMATION: AssessmentSpec = AssessmentSpec::score_only(
    "confirmation",
    10,
    "The final response briefly confirms completion.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[DURABLE_RESULT, FUNCTION_DISCIPLINE, CONFIRMATION];

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id, super::stable_seed(ID))
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let expected = expected(seed);
    let contract = deliverable_contract();
    let spec = scenario_for_case(namespace, seed);
    let case = ScenarioCase::new(
        ID,
        VERSION,
        seed,
        json!({
            "key": KEY,
            "value": expected,
        }),
        ComplexityProfile {
            planning_depth: 1,
            external_systems: 1,
            state_transitions: 2,
            artifact_count: 1,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::state".to_string(),
        ],
        contract,
    )?;
    Ok(MaterializedScenario {
        spec,
        case,
        capture: Some(capture),
    })
}

fn scenario_for_case(run_id: &str, seed: u64) -> ScenarioSpec {
    let scope = scope(run_id);
    let expected = expected(seed);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Store this JSON value for later use in scope `{scope}` under key `{KEY}`: {}. Confirm briefly after it has been stored.",
            serde_json::to_string(&expected).expect("serialize static scenario value")
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 12,
            max_output_tokens: Some(8_192),
            max_total_tokens: Some(122_880),
            stuck_timeout_seconds: 240,
            max_validation_retries: None,
        },
        denied_functions: &[],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: None,
        evaluate,
        cleanup: Some(cleanup),
    }
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    _run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let expected = observation
            .case
            .inputs
            .get("value")
            .cloned()
            .unwrap_or(Value::Null);
        let deliverable = observation
            .deliverables
            .iter()
            .find(|deliverable| deliverable.id == DELIVERABLE_ID)
            .ok_or_else(|| anyhow::anyhow!("captured state deliverable is missing"))?;
        let exact_write = invariant_passed(deliverable, "single_exact_write");
        let writes = common::function_calls(&observation.transcript)
            .into_iter()
            .filter(|call| call.function_id == "state::set")
            .count();
        assess(
            &expected,
            deliverable
                .content
                .as_json()
                .ok_or_else(|| anyhow::anyhow!("state deliverable is not JSON"))?,
            exact_write,
            writes,
            observation.metrics.totals.function_call_errors,
            observation.response.as_str(),
        )
    })
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let expected = observation
            .case
            .inputs
            .get("value")
            .cloned()
            .unwrap_or(Value::Null);
        let observed = common::state_value(
            context
                .trigger("state::get", json!({ "scope": scope(run_id), "key": KEY }))
                .await?,
        );
        let invocations = common::function_invocations(&observation.transcript);
        let writes = invocations
            .iter()
            .filter(|invocation| invocation.call.function_id == "state::set")
            .collect::<Vec<_>>();
        let exact_arguments = json!({
            "scope": scope(run_id),
            "key": KEY,
            "value": expected,
        });
        let exact_write = writes.len() == 1 && writes[0].call.arguments == exact_arguments;
        let provenance = if exact_write && observation.metrics.totals.function_call_errors == 0 {
            writes
                .iter()
                .map(|invocation| ProvenanceEvidence {
                    kind: "function_call".to_string(),
                    source_id: invocation
                        .call_id
                        .clone()
                        .unwrap_or_else(|| "state::set".to_string()),
                    relation: "created".to_string(),
                })
                .collect()
        } else {
            Vec::new()
        };
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "state_value".to_string(),
            content: observed.clone().into(),
            invariants: vec![
                CapturedInvariant {
                    id: "matches_expected".to_string(),
                    passed: observed == expected,
                    reason: format!("expected {expected}, observed {observed}"),
                },
                CapturedInvariant {
                    id: "single_exact_write".to_string(),
                    passed: exact_write,
                    reason: format!("observed {} state::set call(s)", writes.len()),
                },
            ],
            provenance,
        }])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "state_value".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["owner", "case_seed", "status"],
                "properties": {
                    "owner": { "const": "quality-suite" },
                    "case_seed": { "type": "integer", "minimum": 0 },
                    "status": { "const": "stored" }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 4_096,
        }],
        invariants: vec![
            InvariantSpec {
                id: "matches_expected".to_string(),
                description: "The captured state value equals the materialized input.".to_string(),
            },
            InvariantSpec {
                id: "single_exact_write".to_string(),
                description: "Exactly one state write created the captured value.".to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn invariant_passed(deliverable: &CapturedDeliverable, id: &str) -> bool {
    deliverable
        .invariants
        .iter()
        .any(|invariant| invariant.id == id && invariant.passed)
}

fn assess(
    expected: &Value,
    observed: &Value,
    exact_write: bool,
    state_set_calls: usize,
    function_call_errors: u64,
    response: &str,
) -> anyhow::Result<ObjectiveEvaluation> {
    let state_matches = observed == expected;
    let function_discipline = exact_write && function_call_errors == 0;
    let response_present = !response.trim().is_empty();
    let response_chars = response.chars().count();
    let concise_confirmation = response_present && response_chars <= 240;
    let confirmation_points = if concise_confirmation {
        CONFIRMATION.weight()
    } else {
        0
    };

    Ok(assessment::build_evaluation([
        DURABLE_RESULT.full_or_zero(
            state_matches,
            format!("expected {expected}, observed {observed}"),
        ),
        FUNCTION_DISCIPLINE.full_or_zero(
            function_discipline,
            format!(
                "exact_write={exact_write}; observed {state_set_calls} state::set call(s); observed {function_call_errors} function-call error(s)"
            ),
        ),
        CONFIRMATION.award(
            confirmation_points,
            format!(
                "response_present={response_present}; observed {response_chars} character(s); limit 240"
            ),
        )?,
    ]))
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let scope = scope(run_id);
        let _: Value = context
            .trigger("state::delete", json!({ "scope": scope, "key": KEY }))
            .await?;
        Ok(())
    })
}

fn scope(run_id: &str) -> String {
    format!("e2e:{run_id}")
}

fn expected(seed: u64) -> Value {
    json!({
        "owner": "quality-suite",
        "case_seed": seed,
        "status": "stored"
    })
}
