use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::{
    common, ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture,
    ComplexityProfile, DeliverableCaptureFuture, DeliverableContract, EvaluationFuture,
    ExecutionPolicy, InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "mechanical_reaction";
const VERSION: u32 = 4;
const DELIVERABLE_ID: &str = "mechanical_mirror";

const SOURCE_KEY: &str = "source";
const MIRROR_KEY: &str = "mirror";
const REACTIONS_ARMED: AssessmentSpec = AssessmentSpec::hard_gated(
    "reactions_armed",
    30,
    "The wake and mechanical call are registered before the source write.",
);
const MECHANICAL_MIRROR: AssessmentSpec = AssessmentSpec::hard_gated(
    "mechanical_mirror",
    35,
    "The call binding mirrors the complete source event without a root write.",
);
const PARENT_WOKEN: AssessmentSpec = AssessmentSpec::hard_gated(
    "parent_woken",
    20,
    "The mirror state event wakes only the original session.",
);
const CLEAN_COMPLETION: AssessmentSpec = AssessmentSpec::hard_gated(
    "clean_completion",
    15,
    "The run finishes without children, errors, or surviving bindings.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    REACTIONS_ARMED,
    MECHANICAL_MIRROR,
    PARENT_WOKEN,
    CLEAN_COMPLETION,
];

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id, super::stable_seed(ID))
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let source = source_value(seed);
    let case = ScenarioCase::new(
        ID,
        VERSION,
        seed,
        json!({
            "source_key": SOURCE_KEY,
            "mirror_key": MIRROR_KEY,
            "source": source,
            "event_into": "/value",
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
        spec: scenario_for_case(namespace, seed),
        case,
        capture: Some(capture),
    })
}

fn scenario_for_case(run_id: &str, seed: u64) -> ScenarioSpec {
    let names = Names::new(run_id);
    let source = source_value(seed);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            r#"Test a zero-token mechanical reaction in isolated state scope `{scope}`.

Register both reactions before writing any state:

1. A one-shot wake-only state reaction for `{scope}` / `{mirror_key}`, with any non-empty label.
2. A one-shot state call reaction for `{scope}` / `{source_key}` targeting `state::set`.
   Its fixed payload is `{{ "scope": "{scope}", "key": "{mirror_key}" }}` and its
   `event_into` is `/value`, so the full source event becomes the mirror value.

Then write exactly this value once to `{scope}` / `{source_key}`:

`{source}`

Do not write `{mirror_key}` yourself and do not spawn an agent. End the turn after the source
write. The call reaction must create the mirror without a model turn.

When the mirror wake starts a new turn, report briefly that the mirror completed and leave no
binding armed."#,
            scope = names.scope,
            source_key = SOURCE_KEY,
            mirror_key = MIRROR_KEY,
            source = serde_json::to_string(&source).expect("serialize scenario source"),
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 20,
            max_output_tokens: Some(8_192),
            max_total_tokens: Some(250_000),
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
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let names = Names::new(run_id);
        let source = observation
            .case
            .inputs
            .get("source")
            .cloned()
            .unwrap_or(Value::Null);
        let mirror = common::state_value(
            context
                .trigger_value(
                    "state::get",
                    json!({ "scope": names.scope, "key": MIRROR_KEY }),
                )
                .await?,
        );
        let calls = common::function_calls(&observation.transcript);
        let registrations: Vec<_> = calls
            .iter()
            .enumerate()
            .filter(|(_, call)| call.function_id == "engine::register_trigger")
            .collect();
        let wakes: Vec<_> = registrations
            .iter()
            .filter(|(_, call)| is_mirror_wake(call, &names))
            .collect();
        let mirrors: Vec<_> = registrations
            .iter()
            .filter(|(_, call)| is_mirror_call(call, &names))
            .collect();
        let writes: Vec<_> = calls
            .iter()
            .enumerate()
            .filter(|(_, call)| call.function_id == "state::set")
            .collect();
        let exact_source_write = writes.len() == 1
            && writes[0].1.arguments
                == json!({ "scope": names.scope, "key": SOURCE_KEY, "value": source });
        let reactions_armed = registrations.len() == 2
            && wakes.len() == 1
            && mirrors.len() == 1
            && writes.len() == 1
            && wakes[0].0 < writes[0].0
            && mirrors[0].0 < writes[0].0;

        let mirror_valid = valid_mirror(&mirror, &names, &source);
        let records = common::trigger_fired_records(&observation.transcript);
        let call_records: Vec<_> = records
            .iter()
            .filter(|record| record.get("target").and_then(Value::as_str) == Some("state::set"))
            .collect();
        let wake_records: Vec<_> = records
            .iter()
            .filter(|record| record.get("target").and_then(Value::as_str) == Some("harness::send"))
            .collect();
        let call_delivered = call_records.len() == 1;
        // A delivered ƒ-call fire always records what it dispatched; only a
        // skip/gc/expiry record omits `payload`, and neither of those can
        // have produced the mirror write `mirror_valid` checks below — so
        // pinning presence here catches a regression that stops recording it.
        let call_payload_recorded =
            call_records.len() == 1 && call_records[0].get("payload").is_some();
        let parent_woken = wake_records.len() == 1
            && wake_records[0].get("retired").and_then(Value::as_bool) == Some(true)
            && observation.metrics.totals.sessions == 1
            && calls
                .iter()
                .all(|call| call.function_id != "harness::spawn");

        let active_bindings = common::active_binding_count(context, &names.root_session).await?;
        let no_errors = observation.metrics.totals.function_call_errors == 0;
        let confirmed = observation.response.to_ascii_lowercase().contains("mirror");
        let mechanical_mirror =
            exact_source_write && mirror_valid && call_delivered && call_payload_recorded;
        let clean_completion = active_bindings == 0 && no_errors && confirmed;

        Ok(assessment::build_evaluation([
            REACTIONS_ARMED.full_or_zero(
                reactions_armed,
                format!(
                    "registrations={}, wakes={}, call_bindings={}, writes={}",
                    registrations.len(),
                    wakes.len(),
                    mirrors.len(),
                    writes.len()
                ),
            ),
            MECHANICAL_MIRROR.full_or_zero(
                mechanical_mirror,
                format!(
                    "exact_source_write={exact_source_write}, mirror_valid={mirror_valid}, \
                         call_delivered={call_delivered}, call_payload_recorded={call_payload_recorded}"
                ),
            ),
            PARENT_WOKEN.full_or_zero(
                parent_woken,
                format!(
                    "wake_records={}, sessions={}",
                    wake_records.len(),
                    observation.metrics.totals.sessions
                ),
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
        let source = observation
            .case
            .inputs
            .get("source")
            .cloned()
            .unwrap_or(Value::Null);
        let mirror = common::state_value(
            context
                .trigger_value(
                    "state::get",
                    json!({ "scope": names.scope, "key": MIRROR_KEY }),
                )
                .await?,
        );
        let records = common::trigger_fired_records(&observation.transcript);
        let call_records = records
            .iter()
            .filter(|record| record.get("target").and_then(Value::as_str) == Some("state::set"))
            .collect::<Vec<_>>();
        let wake_records = records
            .iter()
            .filter(|record| record.get("target").and_then(Value::as_str) == Some("harness::send"))
            .collect::<Vec<_>>();
        let zero_token_delivery =
            call_records.len() == 1 && call_records[0].get("payload").is_some();
        let root_owned_wake = wake_records.len() == 1
            && wake_records[0].get("retired").and_then(Value::as_bool) == Some(true)
            && observation.metrics.totals.sessions == 1;

        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "state_event_mirror".to_string(),
            content: mirror.clone().into(),
            invariants: vec![
                CapturedInvariant {
                    id: "matches_source_event".to_string(),
                    passed: valid_mirror(&mirror, &names, &source),
                    reason: "captured mirror compared with the materialized source event"
                        .to_string(),
                },
                CapturedInvariant {
                    id: "zero_token_delivery".to_string(),
                    passed: zero_token_delivery,
                    reason: format!(
                        "observed {} state::set trigger delivery record(s)",
                        call_records.len()
                    ),
                },
                CapturedInvariant {
                    id: "root_owned_wake".to_string(),
                    passed: root_owned_wake,
                    reason: format!(
                        "wake_records={}, sessions={}",
                        wake_records.len(),
                        observation.metrics.totals.sessions
                    ),
                },
            ],
            provenance: vec![ProvenanceEvidence {
                kind: "state_location".to_string(),
                source_id: format!("{}/{}", names.scope, MIRROR_KEY),
                relation: "captured_after_mechanical_delivery".to_string(),
            }],
        }])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "state_event_mirror".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["scope", "key", "new_value", "event_type"],
                "properties": {
                    "scope": { "type": "string" },
                    "key": { "const": SOURCE_KEY },
                    "new_value": { "type": "object" },
                    "event_type": { "type": "string", "pattern": "^state:" }
                },
                "additionalProperties": true
            }),
            max_size_bytes: 16_384,
        }],
        invariants: vec![
            InvariantSpec {
                id: "matches_source_event".to_string(),
                description: "The mirror preserves the complete materialized source event."
                    .to_string(),
            },
            InvariantSpec {
                id: "zero_token_delivery".to_string(),
                description: "One mechanical trigger delivery created the mirror.".to_string(),
            },
            InvariantSpec {
                id: "root_owned_wake".to_string(),
                description: "The one-shot mirror wake resumed only the root session.".to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn is_mirror_wake(call: &common::ObservedFunctionCall, names: &Names) -> bool {
    call.function_id == "engine::register_trigger"
        && call.arguments.get("trigger_type").and_then(Value::as_str) == Some("state")
        && call
            .arguments
            .pointer("/config/scope")
            .and_then(Value::as_str)
            == Some(names.scope.as_str())
        && call
            .arguments
            .pointer("/config/key")
            .and_then(Value::as_str)
            == Some(MIRROR_KEY)
        && call
            .arguments
            .get("label")
            .and_then(Value::as_str)
            .is_some_and(|label| !label.trim().is_empty())
        && common::is_wake_registration(&call.arguments)
}

fn is_mirror_call(call: &common::ObservedFunctionCall, names: &Names) -> bool {
    if call.function_id != "engine::register_trigger"
        || call.arguments.get("trigger_type").and_then(Value::as_str) != Some("state")
        || call
            .arguments
            .pointer("/config/scope")
            .and_then(Value::as_str)
            != Some(names.scope.as_str())
        || call
            .arguments
            .pointer("/config/key")
            .and_then(Value::as_str)
            != Some(SOURCE_KEY)
    {
        return false;
    }

    let (function_id, payload, event_into) = if let Some(target) = call
        .arguments
        .get("target")
        .filter(|target| !target.is_null())
    {
        (
            target.get("function_id"),
            target.get("payload"),
            target.get("event_into"),
        )
    } else {
        (
            call.arguments.get("function_id"),
            call.arguments.pointer("/metadata/payload"),
            call.arguments.pointer("/metadata/event_into"),
        )
    };

    function_id.and_then(Value::as_str) == Some("state::set")
        && payload
            .and_then(|payload| payload.get("scope"))
            .and_then(Value::as_str)
            == Some(names.scope.as_str())
        && payload
            .and_then(|payload| payload.get("key"))
            .and_then(Value::as_str)
            == Some(MIRROR_KEY)
        && event_into.and_then(Value::as_str) == Some("/value")
}

fn valid_mirror(mirror: &Value, names: &Names, source: &Value) -> bool {
    mirror.get("scope").and_then(Value::as_str) == Some(names.scope.as_str())
        && mirror.get("key").and_then(Value::as_str) == Some(SOURCE_KEY)
        && mirror.get("new_value") == Some(source)
        && mirror
            .get("event_type")
            .and_then(Value::as_str)
            .is_some_and(|event_type| event_type.starts_with("state:"))
}

fn source_value(seed: u64) -> Value {
    json!({ "message": "mirror me", "case_seed": seed })
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
        for key in [SOURCE_KEY, MIRROR_KEY] {
            let _: Value = context
                .trigger("state::delete", json!({ "scope": names.scope, "key": key }))
                .await?;
        }
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
            scope: format!("e2e:mechanical:{run_id}"),
            root_session: format!("e2e_{run_id}"),
        }
    }
}
