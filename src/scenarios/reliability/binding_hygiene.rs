//! Bindings a run installs must not outlive it. Leaked timers are how a rig
//! ends up firing work nobody asked for hours later.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::common;
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    ComplexityProfile, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "reliability.binding_hygiene";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "binding_hygiene_evidence";
const BINDINGS: usize = 3;
const DELAY_MS: u64 = 600_000;

const BINDINGS_ARMED: AssessmentSpec = AssessmentSpec::hard_gated(
    "bindings_armed",
    30,
    "Exactly three wake-only timers were registered.",
);
const BINDINGS_RELEASED: AssessmentSpec = AssessmentSpec::hard_gated(
    "bindings_released",
    35,
    "Exactly three unregistrations were issued, one per subscription.",
);
const NONE_SURVIVED: AssessmentSpec = AssessmentSpec::hard_gated(
    "none_survived",
    35,
    "The session holds no active binding when the run ends.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[BINDINGS_ARMED, BINDINGS_RELEASED, NONE_SURVIVED];

pub fn scenario(_run_id: &str) -> ScenarioSpec {
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Prove that you can arm and fully release your own bindings inside one turn.\n\n\
             1. Register exactly {BINDINGS} timers with trigger type `timer`, each with \
             `in_ms: {DELAY_MS}`, each with a distinct non-empty label, and with no function \
             target so they would wake this session. Keep every subscription_id.\n\
             2. Immediately unregister all {BINDINGS} of them by subscription_id. None of them \
             may be left armed.\n\
             3. List your session's triggers to confirm nothing survived.\n\
             4. Reply with exactly one line: `REGISTERED:{BINDINGS} UNREGISTERED:{BINDINGS} BINDINGS:0`."
        ),
        filesystem_root: None,
        execution: kit::policy(10, 120_000, 240),
        assessments: ASSESSMENTS,
        setup: None,
        evaluate,
        cleanup: None,
    }
    .spec()
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = super::case(
        ID,
        VERSION,
        seed,
        json!({
            "bindings": BINDINGS,
            "delay_ms": DELAY_MS,
        }),
        ComplexityProfile {
            planning_depth: 2,
            dependency_depth: 1,
            external_systems: 1,
            state_transitions: 6,
            artifact_count: 1,
            ambiguity_level: 1,
            ..ComplexityProfile::default()
        },
        &["iii::triggers"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["registrations", "unregistrations", "active_bindings", "response"],
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

async fn active_bindings(context: &E2eContext, session_id: &str) -> usize {
    common::active_binding_count(context, session_id)
        .await
        .unwrap_or(usize::MAX)
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    _run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let calls = common::function_calls(&observation.transcript);
        let registrations = calls
            .iter()
            .filter(|call| {
                call.function_id == "engine::register_trigger"
                    && call.arguments.get("trigger_type").and_then(Value::as_str) == Some("timer")
            })
            .count();
        let unregistrations = kit::calls_of(&calls, "engine::unregister_trigger").len();
        let survivors = active_bindings(context, &observation.metrics.root_session_id).await;

        Ok(assessment::build_evaluation([
            BINDINGS_ARMED.full_or_zero(
                registrations == BINDINGS,
                format!("observed {registrations} timer registration(s), expected {BINDINGS}"),
            ),
            BINDINGS_RELEASED.full_or_zero(
                unregistrations == BINDINGS,
                format!("observed {unregistrations} unregistration(s), expected {BINDINGS}"),
            ),
            NONE_SURVIVED.full_or_zero(
                survivors == 0,
                format!("observed {survivors} active binding(s) after the run, expected none"),
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
        let calls = common::function_calls(&observation.transcript);
        let survivors = active_bindings(context, &observation.metrics.root_session_id).await;
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "registrations": kit::calls_of(&calls, "engine::register_trigger").len(),
                "unregistrations": kit::calls_of(&calls, "engine::unregister_trigger").len(),
                "active_bindings": survivors,
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_binding_hygiene_evidence",
            )],
        )])
    })
}
