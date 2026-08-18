//! The requested pipeline cannot be ordered. The graded behaviour is refusal
//! with the cycle named, not a partially built graph.

use serde_json::json;

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::common;
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "orchestration.cycle_refusal";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "cycle_refusal_evidence";

const DEPENDENCIES: &[(&str, &[&str])] = &[
    ("collect", &[]),
    ("reduce", &["publish"]),
    ("verify", &["reduce"]),
    ("publish", &["verify"]),
];
const CYCLE: &str = "publish,reduce,verify";

const NOTHING_BUILT: AssessmentSpec = AssessmentSpec::hard_gated(
    "nothing_built",
    40,
    "No stage was materialized from an unorderable specification.",
);
const CYCLE_NAMED: AssessmentSpec = AssessmentSpec::hard_gated(
    "cycle_named",
    40,
    "The response names exactly the stages that form the cycle.",
);
const REFUSAL_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "refusal_reported",
    20,
    "The response states that nothing was written.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[NOTHING_BUILT, CYCLE_NAMED, REFUSAL_REPORTED];

fn declaration() -> String {
    DEPENDENCIES
        .iter()
        .map(|(node, dependencies)| {
            if dependencies.is_empty() {
                format!("- `{node}` depends on nothing")
            } else {
                format!("- `{node}` depends on {}", dependencies.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let scope = kit::scope(run_id);
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Run a four-stage pipeline in dependency order. The stages are:\n\n{}\n\n\
             1. Work out an execution order that never runs a stage before the stages it \
             depends on. Executing a stage means one `state::set` in scope `{scope}` with key \
             `node:<stage>`.\n\
             2. If no such order exists, execute nothing at all. Do not write a partial \
             pipeline, and do not drop a dependency to make it fit.\n\
             3. Reply with exactly one line: `CYCLE:<stages> NODES_WRITTEN:<n>` where `<stages>` \
             is the alphabetically sorted, comma-separated list of the stages that form the \
             cycle, and `<n>` is how many stages you wrote.",
            declaration()
        ),
        filesystem_root: None,
        execution: kit::policy(10, 120_000, 240),
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
            "nodes": DEPENDENCIES
                .iter()
                .map(|(node, dependencies)| json!({ "node": node, "depends_on": dependencies }))
                .collect::<Vec<_>>(),
            "cycle": CYCLE,
        }),
        super::graph_profile(3, 1, 4),
        &[],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["scope_writes", "response"],
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

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let scope = kit::scope(run_id);
        let writes = super::written_keys(&common::function_calls(&observation.transcript), &scope);

        Ok(assessment::build_evaluation([
            NOTHING_BUILT.full_or_zero(
                writes.is_empty(),
                format!(
                    "observed {} write(s) into `{scope}`: {writes:?}",
                    writes.len()
                ),
            ),
            CYCLE_NAMED.full_or_zero(
                observation.response.contains(&format!("CYCLE:{CYCLE}")),
                format!("expected `CYCLE:{CYCLE}` in the response"),
            ),
            REFUSAL_REPORTED.full_or_zero(
                observation.response.contains("NODES_WRITTEN:0"),
                "expected `NODES_WRITTEN:0` in the response",
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
                "scope_writes": super::written_keys(
                    &common::function_calls(&observation.transcript),
                    &scope,
                ),
                "declared_cycle": CYCLE,
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_cycle_refusal_evidence",
            )],
        )])
    })
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let keys: Vec<String> = DEPENDENCIES
            .iter()
            .map(|(node, _)| format!("node:{node}"))
            .collect();
        kit::state_delete(context, &kit::scope(run_id), &keys).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_declared_specification_really_is_unorderable() {
        let order: Vec<String> = DEPENDENCIES
            .iter()
            .map(|(node, _)| (*node).to_string())
            .collect();
        assert!(!super::super::respects_dependencies(&order, DEPENDENCIES));
    }

    #[test]
    fn the_named_cycle_is_sorted_and_complete() {
        let mut cycle: Vec<&str> = DEPENDENCIES
            .iter()
            .filter(|(_, dependencies)| !dependencies.is_empty())
            .map(|(node, _)| *node)
            .collect();
        cycle.sort_unstable();
        assert_eq!(cycle.join(","), CYCLE);
    }
}
