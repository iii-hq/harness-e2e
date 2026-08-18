//! The child cannot succeed without a constraint only the parent was told.
//! Delegation that drops context is the failure being measured.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::common;
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "cognition.subagent_context_handoff";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "subagent_context_handoff_evidence";
const RELEASE_TAG: &str = "E2E-RELEASE-2f81";
const RESULT_KEY: &str = "release_note";

const SINGLE_CHILD: AssessmentSpec = AssessmentSpec::hard_gated(
    "single_child",
    30,
    "Exactly one child was spawned, with the session id the task named.",
);
const CONTEXT_HANDED_OVER: AssessmentSpec = AssessmentSpec::hard_gated(
    "context_handed_over",
    40,
    "The child's own transcript carries the tag it could not have known otherwise.",
);
const RESULT_CARRIES_CONTEXT: AssessmentSpec = AssessmentSpec::hard_gated(
    "result_carries_context",
    30,
    "The stored result contains the tag, proving the constraint survived delegation.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[SINGLE_CHILD, CONTEXT_HANDED_OVER, RESULT_CARRIES_CONTEXT];

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let scope = kit::scope(run_id);
    let child = super::child_session(run_id, 1);
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "You are preparing a release note through a sub-agent.\n\n\
             The release tag for this cycle is `{RELEASE_TAG}`. Only you have been told it.\n\n\
             1. Spawn exactly one child with `harness::spawn`, session id `{child}`, allowed \
             `state::set`.\n\
             2. The child's task: write a one-line release note to state scope `{scope}` key \
             `{RESULT_KEY}`. The note must start with the release tag followed by a space and a \
             short summary. The child has no other way to learn the tag, so its task text must \
             carry it.\n\
             3. Do not write `{RESULT_KEY}` yourself.\n\
             4. Reply with exactly one line: `DELEGATED:1 TAG:{RELEASE_TAG}`."
        ),
        filesystem_root: None,
        execution: kit::policy(18, 280_000, 900),
        assessments: ASSESSMENTS,
        setup: None,
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
            "release_tag": RELEASE_TAG,
            "result_key": RESULT_KEY,
        }),
        super::delegation_profile(1, 3),
        &["iii::sessions"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["spawns", "child_transcript_has_tag", "result", "response"],
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

fn spawns(observation: &ScenarioObservation) -> usize {
    common::function_calls(&observation.transcript)
        .iter()
        .filter(|call| call.function_id == "harness::spawn")
        .count()
}

async fn child_transcript_has_tag(context: &E2eContext, run_id: &str) -> bool {
    match context.transcript(&super::child_session(run_id, 1)).await {
        Ok(transcript) => common::transcript_contains(&transcript, RELEASE_TAG),
        Err(_) => false,
    }
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let scope = kit::scope(run_id);
        let spawns = spawns(observation);
        let handed_over = child_transcript_has_tag(context, run_id).await;
        let result = kit::state_get(context, &scope, RESULT_KEY)
            .await
            .unwrap_or(Value::Null);
        let result_carries_tag = result
            .as_str()
            .is_some_and(|note| note.trim_start().starts_with(RELEASE_TAG));

        Ok(assessment::build_evaluation([
            SINGLE_CHILD.full_or_zero(
                spawns == 1,
                format!("observed {spawns} spawn(s), expected exactly one"),
            ),
            CONTEXT_HANDED_OVER.full_or_zero(
                handed_over,
                format!("child transcript carries the release tag: {handed_over}"),
            ),
            RESULT_CARRIES_CONTEXT
                .full_or_zero(result_carries_tag, format!("observed stored note {result}")),
        ]))
    })
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let scope = kit::scope(run_id);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "spawns": spawns(observation),
                "child_transcript_has_tag": child_transcript_has_tag(context, run_id).await,
                "result": kit::state_get(context, &scope, RESULT_KEY).await.unwrap_or(Value::Null),
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_context_handoff_evidence",
            )],
        )])
    })
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        kit::state_delete(context, &kit::scope(run_id), &[RESULT_KEY.to_string()]).await
    })
}
