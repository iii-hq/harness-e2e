//! A diamond: one source, two derived values, one merge that depends on both.
//! Every derived value must come from a read, not from memory.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::common;
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "orchestration.diamond_merge";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "diamond_merge_evidence";
const SOURCE: i64 = 7;
const DOUBLED: i64 = SOURCE * 2;
const OFFSET: i64 = SOURCE + 5;
const MERGED: i64 = DOUBLED + OFFSET;
const KEYS: [&str; 4] = ["source", "doubled", "offset", "merged"];

const VALUES_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "values_exact",
    40,
    "Every node in the diamond holds the value its formula produces.",
);
const ORDER_RESPECTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "order_respected",
    35,
    "Derived values follow the source, and the merge follows both derived values.",
);
const MERGE_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "merge_reported",
    25,
    "The final response reports the merged value.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[VALUES_EXACT, ORDER_RESPECTED, MERGE_REPORTED];

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let scope = kit::scope(run_id);
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Build a four-node calculation in state scope `{scope}`. Each node is one \
             `state::set` whose value is a plain integer.\n\n\
             1. `source` is {SOURCE}.\n\
             2. `doubled` is `source` multiplied by 2.\n\
             3. `offset` is `source` plus 5.\n\
             4. `merged` is `doubled` plus `offset`.\n\n\
             Read each input back from state before you use it: `doubled` and `offset` must be \
             computed from the stored `source`, and `merged` from the stored `doubled` and \
             `offset`. Write each key exactly once.\n\n\
             Reply with exactly one line: `MERGED:<value>`."
        ),
        filesystem_root: None,
        execution: kit::policy(16, 180_000, 300),
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
            "source": SOURCE,
            "doubled": DOUBLED,
            "offset": OFFSET,
            "merged": MERGED,
        }),
        super::graph_profile(3, 2, 4),
        &[],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["values", "write_order", "response"],
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

async fn values(context: &E2eContext, scope: &str) -> Vec<Option<i64>> {
    let mut values = Vec::new();
    for key in KEYS {
        values.push(
            kit::state_get(context, scope, key)
                .await
                .ok()
                .as_ref()
                .and_then(Value::as_i64),
        );
    }
    values
}

fn order_respected(write_order: &[String]) -> bool {
    let position = |key: &str| write_order.iter().position(|written| written == key);
    let (Some(source), Some(doubled), Some(offset), Some(merged)) = (
        position("source"),
        position("doubled"),
        position("offset"),
        position("merged"),
    ) else {
        return false;
    };
    source < doubled && source < offset && doubled < merged && offset < merged
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let scope = kit::scope(run_id);
        let observed = values(context, &scope).await;
        let expected = vec![Some(SOURCE), Some(DOUBLED), Some(OFFSET), Some(MERGED)];
        let write_order =
            super::written_keys(&common::function_calls(&observation.transcript), &scope);

        Ok(assessment::build_evaluation([
            VALUES_EXACT.full_or_zero(
                observed == expected,
                format!("expected {expected:?} for {KEYS:?}, observed {observed:?}"),
            ),
            ORDER_RESPECTED.full_or_zero(
                order_respected(&write_order),
                format!("observed write order {write_order:?}"),
            ),
            MERGE_REPORTED.full_or_zero(
                observation.response.contains(&format!("MERGED:{MERGED}")),
                format!("expected `MERGED:{MERGED}` in the response"),
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
                "values": values(context, &scope).await,
                "write_order": super::written_keys(
                    &common::function_calls(&observation.transcript),
                    &scope,
                ),
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_diamond_merge_evidence",
            )],
        )])
    })
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let keys: Vec<String> = KEYS.iter().map(|key| (*key).to_string()).collect();
        kit::state_delete(context, &kit::scope(run_id), &keys).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|key| (*key).to_string()).collect()
    }

    #[test]
    fn the_merge_must_follow_both_derived_values() {
        assert!(order_respected(&order(&[
            "source", "doubled", "offset", "merged"
        ])));
        assert!(order_respected(&order(&[
            "source", "offset", "doubled", "merged"
        ])));
        assert!(!order_respected(&order(&[
            "source", "doubled", "merged", "offset"
        ])));
        assert!(!order_respected(&order(&[
            "doubled", "source", "offset", "merged"
        ])));
    }
}
