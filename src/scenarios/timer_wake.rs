use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::{
    common, ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture,
    ComplexityProfile, DeliverableCaptureFuture, DeliverableContract, EvaluationFuture,
    ExecutionPolicy, InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "timer_wake";
const VERSION: u32 = 4;
const DELIVERABLE_ID: &str = "timer_result";

const RESULT_KEY: &str = "result";
const TIMER_ARMED: AssessmentSpec = AssessmentSpec::hard_gated(
    "timer_armed",
    30,
    "One wake-only relative timer is armed before any result write.",
);
const PARENT_WOKEN: AssessmentSpec = AssessmentSpec::hard_gated(
    "parent_woken",
    30,
    "The timer retires after waking the original session exactly once.",
);
const WAKE_ACTION: AssessmentSpec = AssessmentSpec::hard_gated(
    "wake_action",
    25,
    "The timer-woken turn persists the requested result.",
);
const CLEAN_COMPLETION: AssessmentSpec = AssessmentSpec::hard_gated(
    "clean_completion",
    15,
    "The root completes without children, errors, or surviving bindings.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[TIMER_ARMED, PARENT_WOKEN, WAKE_ACTION, CLEAN_COMPLETION];

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        ID,
        VERSION,
        seed,
        json!({
            "delay_ms": 6_000,
            "result_key": RESULT_KEY,
            "expected": expected_result(),
        }),
        ComplexityProfile {
            planning_depth: 2,
            dependency_depth: 1,
            external_systems: 1,
            state_transitions: 3,
            wake_cycles: 1,
            artifact_count: 1,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::state".to_string(),
            "iii::triggers".to_string(),
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
        prompt: format!(
            r#"Test the parent-owned timer control plane in isolated state scope `{scope}`.

Register exactly one wake-only timer for roughly six seconds from now:

- use trigger type `timer` with `in_ms: 6000`;
- give it any non-empty top-level label;
- omit every function target so it wakes this session;
- do not spawn a child.

After the registration succeeds, end the turn immediately without writing state.

When the timer notification starts a new turn, write exactly
`{{ "status": "fired" }}` to `{scope}` / `{result_key}`, then respond briefly that the timer
fired. Leave no binding armed."#,
            scope = names.scope,
            result_key = RESULT_KEY,
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 24,
            max_output_tokens: Some(8_192),
            max_total_tokens: 400_000,
            stuck_timeout_seconds: 120,
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
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let names = Names::new(run_id);
        let expected = observation
            .case
            .inputs
            .get("expected")
            .cloned()
            .unwrap_or(Value::Null);
        let observed = common::state_value(
            context
                .trigger_value(
                    "state::get",
                    json!({ "scope": names.scope, "key": RESULT_KEY }),
                )
                .await?,
        );
        let calls = common::function_calls(&observation.transcript);
        let registrations: Vec<_> = calls
            .iter()
            .enumerate()
            .filter(|(_, call)| call.function_id == "engine::register_trigger")
            .collect();
        let timers: Vec<_> = registrations
            .iter()
            .filter(|(_, call)| is_timer_registration(call))
            .collect();
        let writes: Vec<_> = calls
            .iter()
            .enumerate()
            .filter(|(_, call)| call.function_id == "state::set")
            .collect();
        let exact_write = writes.len() == 1
            && writes[0].1.arguments
                == json!({ "scope": names.scope, "key": RESULT_KEY, "value": expected });
        let timer_armed = registrations.len() == 1
            && timers.len() == 1
            && writes.len() == 1
            && timers[0].0 < writes[0].0;

        let records = common::trigger_fired_records(&observation.transcript);
        let timer_fired = records.len() == 1
            && records[0].get("retired").and_then(Value::as_bool) == Some(true)
            && records[0].get("once").and_then(Value::as_bool) == Some(true)
            && records[0].get("target").and_then(Value::as_str) == Some("harness::send");
        let root_only = observation.metrics.totals.sessions == 1
            && calls
                .iter()
                .all(|call| call.function_id != "harness::spawn");
        let active_bindings = common::active_binding_count(context, &names.root_session).await?;
        let no_errors = observation.metrics.totals.function_call_errors == 0;
        let response = observation.response.to_ascii_lowercase();
        let confirmed = response.contains("timer") && response.contains("fired");

        let parent_woken = timer_fired && root_only;
        let wake_action = exact_write && observed == expected;
        let clean_completion = active_bindings == 0 && no_errors && confirmed;

        Ok(assessment::build_evaluation([
            TIMER_ARMED.full_or_zero(
                timer_armed,
                format!(
                    "registrations={}, timers={}, writes={}",
                    registrations.len(),
                    timers.len(),
                    writes.len()
                ),
            ),
            PARENT_WOKEN.full_or_zero(
                parent_woken,
                format!("timer_fired={timer_fired}, root_only={root_only}"),
            ),
            WAKE_ACTION.full_or_zero(
                wake_action,
                format!("exact_write={exact_write}, observed={observed}"),
            ),
            CLEAN_COMPLETION.full_or_zero(
                clean_completion,
                format!(
                    "active_bindings={active_bindings}, function_errors={}, confirmed={confirmed}",
                    observation.metrics.totals.function_call_errors
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
        let names = Names::new(run_id);
        let expected = observation
            .case
            .inputs
            .get("expected")
            .cloned()
            .unwrap_or(Value::Null);
        let observed = common::state_value(
            context
                .trigger_value(
                    "state::get",
                    json!({ "scope": names.scope, "key": RESULT_KEY }),
                )
                .await?,
        );
        let calls = common::function_calls(&observation.transcript);
        let writes = calls
            .iter()
            .filter(|call| call.function_id == "state::set")
            .collect::<Vec<_>>();
        let exact_write = writes.len() == 1
            && writes[0].arguments
                == json!({ "scope": names.scope, "key": RESULT_KEY, "value": expected });
        let records = common::trigger_fired_records(&observation.transcript);
        let timer_records = records
            .iter()
            .filter(|record| record.get("target").and_then(Value::as_str) == Some("harness::send"))
            .collect::<Vec<_>>();
        let one_shot_wake = timer_records.len() == 1
            && timer_records[0].get("retired").and_then(Value::as_bool) == Some(true)
            && timer_records[0].get("once").and_then(Value::as_bool) == Some(true);

        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "state_value".to_string(),
            content: observed.clone().into(),
            invariants: vec![
                CapturedInvariant {
                    id: "matches_expected_result".to_string(),
                    passed: observed == expected,
                    reason: format!("expected {expected}, observed {observed}"),
                },
                CapturedInvariant {
                    id: "single_wake_write".to_string(),
                    passed: exact_write,
                    reason: format!("observed {} state::set call(s)", writes.len()),
                },
                CapturedInvariant {
                    id: "one_shot_timer_retired".to_string(),
                    passed: one_shot_wake,
                    reason: format!("observed {} timer wake record(s)", timer_records.len()),
                },
            ],
            provenance: vec![ProvenanceEvidence {
                kind: "state_location".to_string(),
                source_id: format!("{}/{}", names.scope, RESULT_KEY),
                relation: "captured_after_timer_wake".to_string(),
            }],
        }])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "state_value".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({ "const": expected_result() }),
            max_size_bytes: 4_096,
        }],
        invariants: vec![
            InvariantSpec {
                id: "matches_expected_result".to_string(),
                description: "The timer-woken turn persisted the exact result.".to_string(),
            },
            InvariantSpec {
                id: "single_wake_write".to_string(),
                description: "Exactly one state write followed the timer wake.".to_string(),
            },
            InvariantSpec {
                id: "one_shot_timer_retired".to_string(),
                description: "The timer delivered once and retired itself.".to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn expected_result() -> Value {
    json!({ "status": "fired" })
}

fn is_timer_registration(call: &common::ObservedFunctionCall) -> bool {
    call.function_id == "engine::register_trigger"
        && call.arguments.get("trigger_type").and_then(Value::as_str) == Some("timer")
        && call
            .arguments
            .pointer("/config/in_ms")
            .and_then(Value::as_u64)
            .is_some_and(|in_ms| (3_000..=15_000).contains(&in_ms))
        && call
            .arguments
            .get("label")
            .and_then(Value::as_str)
            .is_some_and(|label| !label.trim().is_empty())
        && common::is_wake_registration(&call.arguments)
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let names = Names::new(run_id);
        let listed = context
            .trigger_value(
                "harness::triggers::list",
                json!({ "session_id": names.root_session }),
            )
            .await?;
        for subscription_id in listed
            .get("subscriptions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|subscription| subscription.get("subscription_id").and_then(Value::as_str))
        {
            let _: Value = context
                .trigger(
                    "harness::triggers::unregister",
                    json!({
                        "session_id": names.root_session,
                        "subscription_id": subscription_id,
                    }),
                )
                .await?;
        }
        let _: Value = context
            .trigger(
                "state::delete",
                json!({ "scope": names.scope, "key": RESULT_KEY }),
            )
            .await?;
        Ok(())
    })
}

struct Names {
    scope: String,
    root_session: String,
}

impl Names {
    fn new(run_id: &str) -> Self {
        Self {
            scope: format!("e2e:timer:{run_id}"),
            root_session: format!("e2e_{run_id}"),
        }
    }
}
