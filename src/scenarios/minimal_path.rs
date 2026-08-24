//! `minimal_path` — the deliverable is trivial ON PURPOSE.
//!
//! Every other scenario in this registry measures whether the harness can do
//! something. This one measures how CHEAPLY it does something it certainly
//! can: one exact state write plus one short confirmation. The correctness
//! gate is a formality; the signal is the banded efficiency scoring — calls,
//! turns, friction. Tracked version-over-version, it answers a question the
//! correctness scenarios cannot: is the harness getting leaner?

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

pub const ID: &str = "minimal_path";
const VERSION: u32 = 2;
const DELIVERABLE_ID: &str = "minimal_result";
const KEY: &str = "minimal_result";
/// Band: a two-call session keeps just under half of `LEAN_CALLS`.
const TWO_CALL_POINTS: u8 = 12;
/// Band: a two-turn session keeps half of `LEAN_TURNS`.
const TWO_TURN_POINTS: u8 = 10;

const EXACT_RESULT: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "exact_result",
    40,
    "The exact expected JSON is present at the requested state key, created by exactly one exact write.",
    EvaluationDimension::Deliverable,
);
const LEAN_CALLS: AssessmentSpec = AssessmentSpec::score_only(
    "lean_calls",
    25,
    "Banded: full credit for a single non-exempt function call, partial for two, none otherwise.",
);
const LEAN_TURNS: AssessmentSpec = AssessmentSpec::score_only(
    "lean_turns",
    20,
    "Banded: full credit for finishing in one turn, partial for two, none otherwise.",
);
const ZERO_FRICTION: AssessmentSpec = AssessmentSpec::score_only(
    "zero_friction",
    15,
    "No function-call errors and no non-exempt call besides the single state write.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[EXACT_RESULT, LEAN_CALLS, LEAN_TURNS, ZERO_FRICTION];

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
            state_transitions: 1,
            artifact_count: 1,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::state".to_string(),
        ],
        contract,
    )?
    // The derived minimum (planning + artifact) overstates a task whose whole
    // point is that two units of work suffice; pin it so work_amplification
    // in the report layer stays meaningful for this scenario.
    .with_minimum_expected_work(2)?;
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
            "Store this JSON value in scope `{scope}` under key `{KEY}` using a single `state::set` call: {}. Then reply with one short confirmation line containing `MIN-DONE`.",
            serde_json::to_string(&expected).expect("serialize static scenario value")
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 6,
            max_output_tokens: Some(4_096),
            max_total_tokens: Some(60_000),
            stuck_timeout_seconds: 180,
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
            .ok_or_else(|| anyhow::anyhow!("captured minimal-path deliverable is missing"))?;
        let calls = common::function_calls(&observation.transcript);
        assess(
            &expected,
            deliverable
                .content
                .as_json()
                .ok_or_else(|| anyhow::anyhow!("minimal-path deliverable is not JSON"))?,
            invariant_passed(deliverable, "single_exact_write"),
            &calls,
            observation.metrics.totals.turns,
            assistant_turns(&observation.transcript),
            observation.metrics.totals.function_call_errors,
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
                    "owner": { "const": "efficiency-suite" },
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
                description: "Exactly one exact state write created the captured value."
                    .to_string(),
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

/// Function discovery (`engine::functions::*`) is legitimate machinery, not
/// spend — the same exemption every leaf-discipline audit in this registry
/// grants.
fn is_exempt(function_id: &str) -> bool {
    function_id.starts_with("engine::functions::")
}

fn non_exempt_calls(calls: &[common::ObservedFunctionCall]) -> Vec<&common::ObservedFunctionCall> {
    calls
        .iter()
        .filter(|call| !is_exempt(&call.function_id))
        .collect()
}

fn banded_call_points(non_exempt_call_count: usize) -> u8 {
    match non_exempt_call_count {
        1 => LEAN_CALLS.weight(),
        2 => TWO_CALL_POINTS,
        _ => 0,
    }
}

fn banded_turn_points(turns: u64) -> u8 {
    match turns {
        1 => LEAN_TURNS.weight(),
        2 => TWO_TURN_POINTS,
        _ => 0,
    }
}

/// Assistant turns observed in the transcript: assistant messages carrying at
/// least one content block. The metered `totals.turns` counter is the scoring
/// source; this count corroborates it in the `lean_turns` reason.
fn assistant_turns(transcript: &Value) -> usize {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("message"))
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .filter(|message| {
            message
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(|blocks| !blocks.is_empty())
        })
        .count()
}

fn assess(
    expected: &Value,
    observed: &Value,
    exact_write: bool,
    calls: &[common::ObservedFunctionCall],
    metered_turns: u64,
    transcript_turns: usize,
    function_call_errors: u64,
) -> anyhow::Result<ObjectiveEvaluation> {
    let state_matches = observed == expected;
    let non_exempt = non_exempt_calls(calls);
    let state_set_calls = non_exempt
        .iter()
        .filter(|call| call.function_id == "state::set")
        .count();
    let only_the_write = non_exempt.len() == 1 && state_set_calls == 1;

    Ok(assessment::build_evaluation([
        EXACT_RESULT.full_or_zero(
            state_matches && exact_write,
            format!(
                "state_matches={state_matches} (expected {expected}, observed {observed}); single_exact_write={exact_write}"
            ),
        ),
        LEAN_CALLS.award(
            banded_call_points(non_exempt.len()),
            format!(
                "observed {} non-exempt function call(s); bands: 1 call = {} point(s), 2 calls = {TWO_CALL_POINTS}, otherwise 0",
                non_exempt.len(),
                LEAN_CALLS.weight()
            ),
        )?,
        LEAN_TURNS.award(
            banded_turn_points(metered_turns),
            format!(
                "observed {metered_turns} metered turn(s) ({transcript_turns} assistant transcript turn(s)); bands: 1 turn = {} point(s), 2 turns = {TWO_TURN_POINTS}, otherwise 0",
                LEAN_TURNS.weight()
            ),
        )?,
        ZERO_FRICTION.full_or_zero(
            function_call_errors == 0 && only_the_write,
            format!(
                "observed {function_call_errors} function-call error(s); observed {} non-exempt call(s) of which {state_set_calls} state::set write(s)",
                non_exempt.len()
            ),
        ),
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
    format!("e2e:minpath:{run_id}")
}

fn expected(seed: u64) -> Value {
    json!({
        "owner": "efficiency-suite",
        "case_seed": seed,
        "status": "stored"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(function_id: &str) -> common::ObservedFunctionCall {
        common::ObservedFunctionCall {
            function_id: function_id.to_string(),
            arguments: json!({}),
        }
    }

    fn transcript_of(calls: &[(&str, Value)]) -> Value {
        let blocks = calls
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
                { "message": { "role": "assistant", "content": blocks } }
            ]
        })
    }

    #[test]
    fn call_bands_award_full_then_partial_then_zero() {
        assert_eq!(banded_call_points(1), LEAN_CALLS.weight());
        assert_eq!(banded_call_points(2), TWO_CALL_POINTS);
        assert_eq!(banded_call_points(3), 0);
        assert_eq!(banded_call_points(0), 0);
    }

    #[test]
    fn turn_bands_award_full_then_partial_then_zero() {
        assert_eq!(banded_turn_points(1), LEAN_TURNS.weight());
        assert_eq!(banded_turn_points(2), TWO_TURN_POINTS);
        assert_eq!(banded_turn_points(3), 0);
        assert_eq!(banded_turn_points(0), 0);
    }

    #[test]
    fn assistant_turn_counter_ignores_non_assistant_and_empty_messages() {
        let transcript = json!({
            "messages": [
                { "message": { "role": "user", "content": [
                    { "type": "text", "text": "task" }
                ] } },
                { "message": { "role": "assistant", "content": [{
                    "type": "function_call",
                    "id": "call-0",
                    "function_id": "state::set",
                    "arguments": { "scope": "s", "key": KEY, "value": 1 }
                }] } },
                { "message": {
                    "role": "function_result",
                    "function_call_id": "call-0",
                    "function_id": "state::set",
                    "is_error": false,
                    "details": { "ok": true }
                } },
                { "message": { "role": "assistant", "content": [] } },
                { "message": { "role": "assistant", "content": [
                    { "type": "text", "text": "MIN-DONE" }
                ] } }
            ]
        });
        assert_eq!(assistant_turns(&transcript), 2);
    }

    #[test]
    fn call_counting_exempts_function_discovery() {
        let transcript = transcript_of(&[
            ("engine::functions::list", json!({ "search": "state" })),
            (
                "engine::functions::info",
                json!({ "function_id": "state::set" }),
            ),
            (
                "state::set",
                json!({ "scope": "s", "key": KEY, "value": 1 }),
            ),
        ]);
        let calls = common::function_calls(&transcript);

        assert_eq!(calls.len(), 3);
        assert_eq!(non_exempt_calls(&calls).len(), 1);
    }

    #[test]
    fn assessment_awards_full_marks_on_the_single_write_single_turn_path() {
        let expected = expected(7);
        let calls = vec![call("engine::functions::list"), call("state::set")];
        let evaluation = assess(&expected, &expected, true, &calls, 1, 1, 0).unwrap();

        assert_eq!(evaluation.hard_gates.len(), 1);
        assert_eq!(evaluation.hard_gates[0].id, "exact_result");
        assert!(evaluation.hard_gates[0].passed);
        let total: u16 = evaluation
            .awards
            .iter()
            .map(|award| u16::from(award.awarded))
            .sum();
        assert_eq!(total, 100);
    }

    #[test]
    fn assessment_applies_banded_partial_credit_on_a_two_step_path() {
        let expected = expected(7);
        let calls = vec![call("state::get"), call("state::set")];
        let evaluation = assess(&expected, &expected, true, &calls, 2, 2, 0).unwrap();
        let awarded = |id: &str| {
            evaluation
                .awards
                .iter()
                .find(|award| award.id == id)
                .expect("award present")
                .awarded
        };

        assert_eq!(awarded("exact_result"), EXACT_RESULT.weight());
        assert_eq!(awarded("lean_calls"), TWO_CALL_POINTS);
        assert_eq!(awarded("lean_turns"), TWO_TURN_POINTS);
        assert_eq!(awarded("zero_friction"), 0);
    }

    #[test]
    fn materialized_case_is_reproducible_across_namespaces() {
        let first = materialize("attempt-a", 42).unwrap();
        let retry = materialize("attempt-b", 42).unwrap();
        let other_seed = materialize("attempt-c", 43).unwrap();

        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs, retry.case.inputs);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert_ne!(first.spec.prompt, retry.spec.prompt);
        assert_ne!(first.case.case_id, other_seed.case.case_id);
        assert_ne!(first.case.inputs, other_seed.case.inputs);
        assert_eq!(
            first.case.complexity.tier,
            super::super::ComplexityTier::L2Stateful
        );
        assert_eq!(first.case.work.minimum_expected_work, 2);
        assert_eq!(
            usize::from(first.case.complexity.profile.artifact_count),
            first.case.deliverable_contract.artifacts.len()
        );
    }
}
