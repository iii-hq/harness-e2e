//! Three independent branches and one join that may only run once every
//! branch has landed.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::common;
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "orchestration.fanout_join";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "fanout_join_evidence";
const BRANCHES: [(&str, &str); 3] = [("1", "alpha"), ("2", "beta"), ("3", "gamma")];
const JOIN_KEY: &str = "join";
const JOIN_VALUE: &str = "alpha|beta|gamma";

const BRANCHES_COMPLETED: AssessmentSpec = AssessmentSpec::hard_gated(
    "branches_completed",
    30,
    "All three branch values are present in the run scope.",
);
const JOIN_AFTER_BRANCHES: AssessmentSpec = AssessmentSpec::hard_gated(
    "join_after_branches",
    40,
    "The join was written only after every branch write.",
);
const JOIN_VALUE_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "join_value_exact",
    30,
    "The join value is the branch values in the declared order.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[BRANCHES_COMPLETED, JOIN_AFTER_BRANCHES, JOIN_VALUE_EXACT];

fn branch_key(index: &str) -> String {
    format!("branch:{index}")
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let scope = kit::scope(run_id);
    let branches = BRANCHES
        .iter()
        .map(|(index, value)| format!("- key `branch:{index}` with the string value `{value}`"))
        .collect::<Vec<_>>()
        .join("\n");
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Fan out three independent branches, then join them, in state scope `{scope}`.\n\n\
             1. Write each branch, in any order:\n{branches}\n\
             2. Read all three branch values back from state. Do not use the values you \
             remember writing.\n\
             3. Only after all three reads succeed, write key `{JOIN_KEY}` with the three values \
             you read joined by a single `|` character in branch order 1, 2, 3.\n\
             4. Reply with exactly one line: `JOIN:<the joined value>`."
        ),
        filesystem_root: None,
        execution: kit::policy(14, 160_000, 300),
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
            "branches": BRANCHES
                .iter()
                .map(|(index, value)| json!({ "key": branch_key(index), "value": value }))
                .collect::<Vec<_>>(),
            "join_key": JOIN_KEY,
            "join_value": JOIN_VALUE,
        }),
        super::graph_profile(2, 3, 4),
        &[],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["branch_values", "join_value", "write_order", "response"],
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

async fn branch_values(context: &E2eContext, scope: &str) -> Vec<Value> {
    let mut values = Vec::new();
    for (index, _) in BRANCHES {
        values.push(
            kit::state_get(context, scope, &branch_key(index))
                .await
                .unwrap_or(Value::Null),
        );
    }
    values
}

fn join_is_last(write_order: &[String]) -> bool {
    let Some(join_position) = write_order.iter().position(|key| key == JOIN_KEY) else {
        return false;
    };
    let branch_positions: Vec<usize> = BRANCHES
        .iter()
        .filter_map(|(index, _)| write_order.iter().position(|key| *key == branch_key(index)))
        .collect();
    branch_positions.len() == BRANCHES.len()
        && branch_positions
            .iter()
            .all(|position| *position < join_position)
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let scope = kit::scope(run_id);
        let write_order =
            super::written_keys(&common::function_calls(&observation.transcript), &scope);
        let values = branch_values(context, &scope).await;
        let expected: Vec<Value> = BRANCHES
            .iter()
            .map(|(_, value)| Value::String((*value).to_string()))
            .collect();
        let join = kit::state_get(context, &scope, JOIN_KEY)
            .await
            .unwrap_or(Value::Null);

        Ok(assessment::build_evaluation([
            BRANCHES_COMPLETED.full_or_zero(
                values == expected,
                format!("expected branch values {expected:?}, observed {values:?}"),
            ),
            JOIN_AFTER_BRANCHES.full_or_zero(
                join_is_last(&write_order),
                format!("observed write order {write_order:?}"),
            ),
            JOIN_VALUE_EXACT.full_or_zero(
                join.as_str() == Some(JOIN_VALUE)
                    && observation.response.contains(&format!("JOIN:{JOIN_VALUE}")),
                format!("expected join `{JOIN_VALUE}`, observed {join}"),
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
                "branch_values": branch_values(context, &scope).await,
                "join_value": kit::state_get(context, &scope, JOIN_KEY).await.unwrap_or(Value::Null),
                "write_order": super::written_keys(
                    &common::function_calls(&observation.transcript),
                    &scope,
                ),
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_fanout_join_evidence",
            )],
        )])
    })
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let mut keys: Vec<String> = BRANCHES
            .iter()
            .map(|(index, _)| branch_key(index))
            .collect();
        keys.push(JOIN_KEY.to_string());
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
    fn a_join_written_before_a_branch_is_rejected() {
        assert!(!join_is_last(&order(&[
            "branch:1", "branch:2", "join", "branch:3"
        ])));
        assert!(!join_is_last(&order(&["branch:1", "branch:2", "join"])));
    }

    #[test]
    fn a_join_after_every_branch_is_accepted() {
        assert!(join_is_last(&order(&[
            "branch:3", "branch:1", "branch:2", "join"
        ])));
    }
}
