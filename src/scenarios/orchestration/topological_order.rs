//! A six-node pipeline with declared dependencies. Every node must be
//! materialised, and never before the nodes it depends on.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::common;
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "orchestration.topological_order";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "topological_order_evidence";

const DEPENDENCIES: &[(&str, &[&str])] = &[
    ("ingest", &[]),
    ("normalize", &["ingest"]),
    ("enrich", &["normalize"]),
    ("score", &["normalize"]),
    ("merge", &["enrich", "score"]),
    ("publish", &["merge"]),
];

const ALL_NODES_MATERIALIZED: AssessmentSpec = AssessmentSpec::hard_gated(
    "all_nodes_materialized",
    30,
    "Every declared node exists in the run's state scope.",
);
const ORDER_RESPECTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "order_respected",
    40,
    "No node was written before one of its declared dependencies.",
);
const NO_EXTRA_WRITES: AssessmentSpec = AssessmentSpec::hard_gated(
    "no_extra_writes",
    15,
    "The scope holds exactly one write per node and nothing else.",
);
const ORDER_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "order_reported",
    15,
    "The final response lists the execution order that was actually used.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    ALL_NODES_MATERIALIZED,
    ORDER_RESPECTED,
    NO_EXTRA_WRITES,
    ORDER_REPORTED,
];

fn node_key(node: &str) -> String {
    format!("node:{node}")
}

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
            "Run a six-stage pipeline in dependency order. The stages are:\n\n{}\n\n\
             1. Work out an execution order that never runs a stage before the stages it \
             depends on.\n\
             2. Execute the stages in that order. Executing a stage means one `state::set` in \
             scope `{scope}` with key `node:<stage>` and value {{\"stage\": \"<stage>\", \
             \"position\": <1-based position in your order>}}.\n\
             3. Write each stage exactly once and write nothing else into that scope.\n\
             4. Reply with exactly one line: `ORDER:<stage>,<stage>,...` listing the stages in \
             the order you executed them.",
            declaration()
        ),
        filesystem_root: None,
        execution: kit::policy(16, 200_000, 600),
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
        }),
        super::graph_profile(3, 2, 6),
        &[],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["written_order", "materialized_nodes", "response"],
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

fn written_order(observation: &ScenarioObservation, scope: &str) -> Vec<String> {
    super::written_keys(&common::function_calls(&observation.transcript), scope)
        .into_iter()
        .filter_map(|key| key.strip_prefix("node:").map(str::to_owned))
        .collect()
}

async fn materialized_nodes(context: &E2eContext, scope: &str) -> Vec<String> {
    let mut present = Vec::new();
    for (node, _) in DEPENDENCIES {
        let value = kit::state_get(context, scope, &node_key(node))
            .await
            .unwrap_or(Value::Null);
        if value.get("stage").and_then(Value::as_str) == Some(*node) {
            present.push((*node).to_string());
        }
    }
    present
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let scope = kit::scope(run_id);
        let order = written_order(observation, &scope);
        let present = materialized_nodes(context, &scope).await;
        let all_writes =
            super::written_keys(&common::function_calls(&observation.transcript), &scope);
        let reported = format!("ORDER:{}", order.join(","));

        Ok(assessment::build_evaluation([
            ALL_NODES_MATERIALIZED.full_or_zero(
                present.len() == DEPENDENCIES.len(),
                format!(
                    "observed {} materialized node(s) of {}: {present:?}",
                    present.len(),
                    DEPENDENCIES.len()
                ),
            ),
            ORDER_RESPECTED.full_or_zero(
                super::respects_dependencies(&order, DEPENDENCIES),
                format!("observed execution order {order:?}"),
            ),
            NO_EXTRA_WRITES.full_or_zero(
                all_writes.len() == DEPENDENCIES.len(),
                format!(
                    "observed {} write(s) into `{scope}`, expected {}",
                    all_writes.len(),
                    DEPENDENCIES.len()
                ),
            ),
            ORDER_REPORTED.full_or_zero(
                !order.is_empty() && observation.response.contains(&reported),
                format!("expected `{reported}` in the response"),
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
                "written_order": written_order(observation, &scope),
                "materialized_nodes": materialized_nodes(context, &scope).await,
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_topological_order_evidence",
            )],
        )])
    })
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let keys: Vec<String> = DEPENDENCIES
            .iter()
            .map(|(node, _)| node_key(node))
            .collect();
        kit::state_delete(context, &kit::scope(run_id), &keys).await
    })
}
