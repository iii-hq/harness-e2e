//! Work that was already done must not be redone. The run starts with a
//! checkpoint already at item three and has to resume from four.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::probe;
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "orchestration.checkpoint_resume";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "checkpoint_resume_evidence";
const CHECKPOINT_KEY: &str = "checkpoint";
const SEEDED_CHECKPOINT: u64 = 3;
const TOTAL_ITEMS: u64 = 7;

const NO_REPROCESSING: AssessmentSpec = AssessmentSpec::hard_gated(
    "no_reprocessing",
    35,
    "No item at or below the seeded checkpoint was fetched again.",
);
const REMAINDER_PROCESSED: AssessmentSpec = AssessmentSpec::hard_gated(
    "remainder_processed",
    35,
    "Every item after the checkpoint was fetched and stored with its exact payload.",
);
const CHECKPOINT_ADVANCED: AssessmentSpec = AssessmentSpec::hard_gated(
    "checkpoint_advanced",
    30,
    "The checkpoint ends at the last item and the resume point is reported.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[NO_REPROCESSING, REMAINDER_PROCESSED, CHECKPOINT_ADVANCED];

fn items_id(run_id: &str) -> String {
    probe::id("items", run_id)
}

fn item_key(index: u64) -> String {
    format!("item:{index}")
}

fn payload(index: u64) -> String {
    format!("item-{index}")
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let id = items_id(run_id);
        probe::register(
            context,
            id.clone(),
            "E2E temporary item source: returns one item payload per index.",
            move |request: Value| {
                let id = id.clone();
                async move {
                    probe::record_hit(&id);
                    let index = request.get("index").and_then(Value::as_u64).unwrap_or(0);
                    if index == 0 || index > TOTAL_ITEMS {
                        return Err(probe::handler_error(format!(
                            "index {index} is outside 1..={TOTAL_ITEMS}"
                        )));
                    }
                    let ledger = probe::ledger(&id);
                    ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(format!("fetched:{index}"), Value::Bool(true));
                    Ok(json!({ "index": index, "payload": payload(index) }))
                }
            },
        );
        kit::state_set(
            context,
            &kit::scope(run_id),
            CHECKPOINT_KEY,
            Value::from(SEEDED_CHECKPOINT),
        )
        .await
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let items = items_id(run_id);
    let scope = kit::scope(run_id);
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Resume an interrupted batch of {TOTAL_ITEMS} items in state scope `{scope}`.\n\n\
             1. Read key `{CHECKPOINT_KEY}` from that scope. It holds the index of the last item \
             that was already processed by the previous run.\n\
             2. Process only the items after that checkpoint, in ascending order. Processing \
             item `n` means: call `{items}` with payload {{\"index\": n}}, then `state::set` key \
             `item:n` with the exact `payload` string it returned, then `state::set` key \
             `{CHECKPOINT_KEY}` to `n`.\n\
             3. Never fetch or rewrite an item at or below the checkpoint you read. That work is \
             already done and repeating it is a failure.\n\
             4. Reply with exactly one line: `RESUMED_FROM:<first index you processed> \
             PROCESSED:<comma-separated indexes>`."
        ),
        filesystem_root: None,
        execution: kit::policy(20, 260_000, 360),
        assessments: ASSESSMENTS,
        setup: Some(setup),
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
            "items_function": "e2etest::items_<run>",
            "checkpoint_key": CHECKPOINT_KEY,
            "seeded_checkpoint": SEEDED_CHECKPOINT,
            "total_items": TOTAL_ITEMS,
        }),
        super::graph_profile(2, 1, 3),
        &[],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["fetched_indexes", "stored_items", "checkpoint", "response"],
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

fn fetched_indexes(run_id: &str) -> Vec<u64> {
    let items = items_id(run_id);
    (1..=TOTAL_ITEMS)
        .filter(|index| probe::ledger_value(&items, &format!("fetched:{index}")).is_some())
        .collect()
}

async fn stored_items(context: &E2eContext, scope: &str) -> Vec<u64> {
    let mut stored = Vec::new();
    for index in 1..=TOTAL_ITEMS {
        let value = kit::state_get(context, scope, &item_key(index))
            .await
            .unwrap_or(Value::Null);
        if value.as_str() == Some(payload(index).as_str()) {
            stored.push(index);
        }
    }
    stored
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let scope = kit::scope(run_id);
        let fetched = fetched_indexes(run_id);
        let stored = stored_items(context, &scope).await;
        let expected: Vec<u64> = (SEEDED_CHECKPOINT + 1..=TOTAL_ITEMS).collect();
        let checkpoint = kit::state_get(context, &scope, CHECKPOINT_KEY)
            .await
            .ok()
            .as_ref()
            .and_then(Value::as_u64);
        let reported = format!(
            "RESUMED_FROM:{} PROCESSED:{}",
            SEEDED_CHECKPOINT + 1,
            expected
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );

        Ok(assessment::build_evaluation([
            NO_REPROCESSING.full_or_zero(
                fetched.iter().all(|index| *index > SEEDED_CHECKPOINT),
                format!("fetched indexes {fetched:?}, none may be at or below {SEEDED_CHECKPOINT}"),
            ),
            REMAINDER_PROCESSED.full_or_zero(
                fetched == expected && stored == expected,
                format!("expected {expected:?}; fetched {fetched:?}; stored {stored:?}"),
            ),
            CHECKPOINT_ADVANCED.full_or_zero(
                checkpoint == Some(TOTAL_ITEMS) && observation.response.contains(&reported),
                format!("checkpoint={checkpoint:?} (expected {TOTAL_ITEMS}); expected `{reported}` in the response"),
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
                "fetched_indexes": fetched_indexes(run_id),
                "stored_items": stored_items(context, &scope).await,
                "checkpoint": kit::state_get(context, &scope, CHECKPOINT_KEY)
                    .await
                    .unwrap_or(Value::Null),
                "response": observation.response,
            }),
            invariants,
            vec![
                kit::function_provenance(&items_id(run_id), "served_the_remaining_items"),
                kit::session_provenance(observation, "captured_checkpoint_resume_evidence"),
            ],
        )])
    })
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        probe::release(run_id);
        let mut keys: Vec<String> = (1..=TOTAL_ITEMS).map(item_key).collect();
        keys.push(CHECKPOINT_KEY.to_string());
        kit::state_delete(context, &kit::scope(run_id), &keys).await
    })
}
