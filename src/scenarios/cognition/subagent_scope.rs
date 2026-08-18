//! Three narrow jobs, three children. The orchestrator delegates and does
//! not quietly do the work itself.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::common;
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "cognition.subagent_scope";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "subagent_scope_evidence";
const JOBS: [(&str, &str); 3] = [
    ("digest:north", "north-ready"),
    ("digest:south", "south-ready"),
    ("digest:west", "west-ready"),
];

const CHILDREN_SPAWNED: AssessmentSpec = AssessmentSpec::hard_gated(
    "children_spawned",
    35,
    "One child was spawned per job, using the session ids the task named.",
);
const CHILDREN_DELIVERED: AssessmentSpec = AssessmentSpec::hard_gated(
    "children_delivered",
    40,
    "Every job's result is present with the exact value that job was given.",
);
const ORCHESTRATOR_DELEGATED: AssessmentSpec = AssessmentSpec::hard_gated(
    "orchestrator_delegated",
    25,
    "The orchestrator wrote none of the job results itself.",
);
const ASSESSMENTS: &[AssessmentSpec] =
    &[CHILDREN_SPAWNED, CHILDREN_DELIVERED, ORCHESTRATOR_DELEGATED];

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let scope = kit::scope(run_id);
    let jobs = JOBS
        .iter()
        .enumerate()
        .map(|(index, (key, value))| {
            format!(
                "- child `{}` writes key `{key}` with the exact string value `{value}`",
                super::child_session(run_id, index + 1)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "You are the orchestrator for three independent jobs. You do not perform them.\n\n\
             Each job is one `state::set` into scope `{scope}`:\n{jobs}\n\n\
             1. Spawn exactly three children with `harness::spawn`, one per job, using exactly \
             the session ids listed above. Give each child only its own job, including the exact \
             key and value, and allow it `state::set`.\n\
             2. Do not write any of those three keys yourself, before or after spawning. If a \
             child fails, report that rather than doing its work.\n\
             3. Spawn returns immediately. End your turn after spawning; you will be woken when \
             there is more to do.\n\
             4. When you reply, use exactly one line: `DELEGATED:3`."
        ),
        filesystem_root: None,
        execution: kit::policy(20, 320_000, 420),
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
            "jobs": JOBS
                .iter()
                .map(|(key, value)| json!({ "key": key, "value": value }))
                .collect::<Vec<_>>(),
        }),
        super::delegation_profile(3, 3),
        &["iii::sessions"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["spawns", "delivered", "orchestrator_writes", "response"],
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

fn spawns(observation: &ScenarioObservation, run_id: &str) -> usize {
    let calls = common::function_calls(&observation.transcript);
    (1..=JOBS.len())
        .filter(|index| {
            let session = super::child_session(run_id, *index);
            calls.iter().any(|call| {
                call.function_id == "harness::spawn"
                    && call.arguments.get("session_id").and_then(Value::as_str)
                        == Some(session.as_str())
            })
        })
        .count()
}

fn orchestrator_writes(observation: &ScenarioObservation) -> usize {
    common::function_calls(&observation.transcript)
        .iter()
        .filter(|call| {
            call.function_id == "state::set"
                && call
                    .arguments
                    .get("key")
                    .and_then(Value::as_str)
                    .is_some_and(|key| JOBS.iter().any(|(job_key, _)| *job_key == key))
        })
        .count()
}

async fn delivered(context: &E2eContext, scope: &str) -> usize {
    let mut delivered = 0;
    for (key, value) in JOBS {
        if kit::state_get(context, scope, key)
            .await
            .ok()
            .as_ref()
            .and_then(Value::as_str)
            == Some(value)
        {
            delivered += 1;
        }
    }
    delivered
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let scope = kit::scope(run_id);
        let spawned = spawns(observation, run_id);
        let delivered = delivered(context, &scope).await;
        let own_writes = orchestrator_writes(observation);

        Ok(assessment::build_evaluation([
            CHILDREN_SPAWNED.full_or_zero(
                spawned == JOBS.len(),
                format!(
                    "observed {spawned} named child spawn(s), expected {}",
                    JOBS.len()
                ),
            ),
            CHILDREN_DELIVERED.full_or_zero(
                delivered == JOBS.len(),
                format!("observed {delivered} delivered job(s) of {}", JOBS.len()),
            ),
            ORCHESTRATOR_DELEGATED.full_or_zero(
                own_writes == 0,
                format!("orchestrator wrote {own_writes} job key(s) itself"),
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
        let scope = kit::scope(run_id);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "spawns": spawns(observation, run_id),
                "delivered": delivered(context, &scope).await,
                "orchestrator_writes": orchestrator_writes(observation),
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_subagent_scope_evidence",
            )],
        )])
    })
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let keys: Vec<String> = JOBS.iter().map(|(key, _)| (*key).to_string()).collect();
        kit::state_delete(context, &kit::scope(run_id), &keys).await
    })
}
