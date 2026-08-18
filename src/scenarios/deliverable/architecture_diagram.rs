//! A diagram of a system that is specified, not imagined. Node and edge sets
//! are compared exactly; orphan nodes fail.

use serde_json::json;

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

use super::workspace;

pub const ID: &str = "deliverable.architecture_diagram";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "architecture_diagram_artifact";
const DIAGRAM_FILE: &str = "docs/architecture.mmd";
const NODES: [&str; 5] = ["client", "gateway", "queue", "store", "worker"];
const EDGES: [(&str, &str); 5] = [
    ("client", "gateway"),
    ("gateway", "queue"),
    ("gateway", "store"),
    ("queue", "worker"),
    ("worker", "store"),
];

const DIAGRAM_PARSES: AssessmentSpec = AssessmentSpec::hard_gated(
    "diagram_parses",
    15,
    "The file declares a left-to-right flowchart.",
);
const NODE_SET_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "node_set_exact",
    30,
    "The diagram names exactly the specified components.",
);
const EDGE_SET_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "edge_set_exact",
    40,
    "The diagram draws exactly the specified dependencies, with no invented ones.",
);
const NO_ORPHANS: AssessmentSpec = AssessmentSpec::hard_gated(
    "no_orphans",
    15,
    "Every component participates in at least one edge.",
);
const ASSESSMENTS: &[AssessmentSpec] =
    &[DIAGRAM_PARSES, NODE_SET_EXACT, EDGE_SET_EXACT, NO_ORPHANS];

fn specification() -> String {
    EDGES
        .iter()
        .map(|(from, to)| format!("- `{from}` sends work to `{to}`"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn expected_edges() -> Vec<(String, String)> {
    EDGES
        .iter()
        .map(|(from, to)| ((*from).to_string(), (*to).to_string()))
        .collect()
}

fn sorted_edges(mut edges: Vec<(String, String)>) -> Vec<(String, String)> {
    edges.sort();
    edges.dedup();
    edges
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Draw this system as a mermaid diagram in this workspace.\n\n\
             The system has exactly these dependencies:\n\n{}\n\n\
             1. Write `{DIAGRAM_FILE}`. The first line is `flowchart LR`.\n\
             2. Use one `-->` edge per dependency, with the component names exactly as written \
             above and one edge per line. Add no other components and no other edges.\n\
             3. Reply with exactly one line: `NODES:{} EDGES:{}`.",
            specification(),
            NODES.len(),
            EDGES.len(),
        ),
        filesystem_root: Some(workspace::root(ID, run_id)),
        execution: kit::policy(16, 200_000, 360),
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
            "diagram_file": DIAGRAM_FILE,
            "nodes": NODES,
            "edges": EDGES.iter().map(|(from, to)| json!([from, to])).collect::<Vec<_>>(),
        }),
        super::build_profile(1, 2),
        &["iii::shell"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["edges", "nodes", "response"],
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

fn diagram(run_id: &str) -> String {
    workspace::read(&workspace::root(ID, run_id), DIAGRAM_FILE).unwrap_or_default()
}

fn observed_nodes(edges: &[(String, String)]) -> Vec<String> {
    let mut nodes: Vec<String> = edges
        .iter()
        .flat_map(|(from, to)| [from.clone(), to.clone()])
        .collect();
    nodes.sort();
    nodes.dedup();
    nodes
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let source = diagram(run_id);
        let edges = sorted_edges(workspace::mermaid_edges(&source));
        let nodes = observed_nodes(&edges);
        let expected_nodes: Vec<String> = NODES.iter().map(|node| (*node).to_string()).collect();
        let summary = format!("NODES:{} EDGES:{}", NODES.len(), EDGES.len());
        let header = source
            .lines()
            .next()
            .map(str::trim)
            .is_some_and(|line| line == "flowchart LR" || line == "graph LR");

        Ok(assessment::build_evaluation([
            DIAGRAM_PARSES.full_or_zero(
                header && !edges.is_empty(),
                format!(
                    "flowchart header present: {header}; parsed {} edge(s)",
                    edges.len()
                ),
            ),
            NODE_SET_EXACT.full_or_zero(
                nodes == expected_nodes,
                format!("observed nodes {nodes:?}, expected {expected_nodes:?}"),
            ),
            EDGE_SET_EXACT.full_or_zero(
                edges == sorted_edges(expected_edges()),
                format!("observed edges {edges:?}"),
            ),
            NO_ORPHANS.full_or_zero(
                expected_nodes
                    .iter()
                    .all(|node| edges.iter().any(|(from, to)| from == node || to == node))
                    && observation.response.contains(&summary),
                format!("expected `{summary}` in the response and every node on an edge"),
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
        let edges = sorted_edges(workspace::mermaid_edges(&diagram(run_id)));
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "edges": edges.iter().map(|(from, to)| json!([from, to])).collect::<Vec<_>>(),
                "nodes": observed_nodes(&edges),
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_architecture_diagram_before_cleanup",
            )],
        )])
    })
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        workspace::remove(&workspace::root(ID, run_id));
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_specification_leaves_no_component_unconnected() {
        let edges = sorted_edges(expected_edges());
        assert_eq!(observed_nodes(&edges).len(), NODES.len());
    }
}
