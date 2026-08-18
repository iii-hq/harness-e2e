//! A three.js scene described by a specification. The gate is the exported
//! scene graph, not how the render looks.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

use super::workspace;

pub const ID: &str = "deliverable.scene_graph";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "scene_graph_artifact";
const GRAPH_FILE: &str = "scene/scene-graph.json";
const MODULE_FILE: &str = "scene/scene.js";

const NODES: [(&str, &str, &str); 7] = [
    ("world", "Group", ""),
    ("ground", "Mesh", "world"),
    ("tower", "Group", "world"),
    ("tower_base", "Mesh", "tower"),
    ("tower_top", "Mesh", "tower"),
    ("sun", "DirectionalLight", "world"),
    ("camera", "PerspectiveCamera", ""),
];
const ROOTS: usize = 2;

const GRAPH_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "graph_exact",
    40,
    "The exported scene graph declares exactly the specified nodes, types, and parents.",
);
const MODULE_CONSISTENT: AssessmentSpec = AssessmentSpec::hard_gated(
    "module_consistent",
    30,
    "The scene module builds a scene and names every node in the graph.",
);
const SELF_CONTAINED: AssessmentSpec = AssessmentSpec::hard_gated(
    "self_contained",
    15,
    "Neither file pulls anything from another host.",
);
const SUMMARY_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "summary_reported",
    15,
    "The response reports the node and root counts it produced.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    GRAPH_EXACT,
    MODULE_CONSISTENT,
    SELF_CONTAINED,
    SUMMARY_REPORTED,
];

fn specification() -> String {
    NODES
        .iter()
        .map(|(name, kind, parent)| {
            if parent.is_empty() {
                format!("- `{name}` of type `{kind}`, no parent")
            } else {
                format!("- `{name}` of type `{kind}`, child of `{parent}`")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn expected_graph() -> Vec<Value> {
    NODES
        .iter()
        .map(|(name, kind, parent)| {
            json!({
                "name": name,
                "type": kind,
                "parent": if parent.is_empty() { Value::Null } else { Value::String((*parent).to_string()) },
            })
        })
        .collect()
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Build a small three.js scene in this workspace.\n\n\
             The scene contains exactly these objects:\n\n{}\n\n\
             1. Write `{MODULE_FILE}`: an ES module that imports three.js from the bare specifier \
             `three`, builds that scene, and exports it. Do not fetch anything over the network \
             and do not reference a CDN.\n\
             2. Write `{GRAPH_FILE}`: the scene graph as JSON in the shape \
             {{\"nodes\": [{{\"name\": \"...\", \"type\": \"...\", \"parent\": \"...\" or null}}]}}. \
             Use exactly the names, types, and parents listed above, and null for an object with \
             no parent.\n\
             3. Reply with exactly one line: `NODES:{} ROOTS:{ROOTS}`.",
            specification(),
            NODES.len(),
        ),
        filesystem_root: Some(workspace::root(ID, run_id)),
        execution: kit::policy(20, 260_000, 900),
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
            "graph_file": GRAPH_FILE,
            "module_file": MODULE_FILE,
            "nodes": expected_graph(),
        }),
        super::build_profile(2, 3),
        &["iii::shell"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["nodes", "module_present", "response"],
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

fn observed_nodes(run_id: &str) -> Vec<Value> {
    workspace::read_json(&workspace::root(ID, run_id), GRAPH_FILE)
        .and_then(|graph| graph.get("nodes").and_then(Value::as_array).cloned())
        .unwrap_or_default()
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let root = workspace::root(ID, run_id);
        let nodes = observed_nodes(run_id);
        let module = workspace::read(&root, MODULE_FILE).unwrap_or_default();
        let graph_source = workspace::read(&root, GRAPH_FILE).unwrap_or_default();
        let named_in_module = NODES
            .iter()
            .filter(|(name, _, _)| module.contains(name))
            .count();
        let external = [
            workspace::external_references(&module),
            workspace::external_references(&graph_source),
        ]
        .concat();
        let summary = format!("NODES:{} ROOTS:{ROOTS}", NODES.len());

        Ok(assessment::build_evaluation([
            GRAPH_EXACT.full_or_zero(
                kit::sorted_by(&nodes, "name") == kit::sorted_by(&expected_graph(), "name"),
                format!("observed {} node(s) in `{GRAPH_FILE}`", nodes.len()),
            ),
            MODULE_CONSISTENT.full_or_zero(
                named_in_module == NODES.len() && module.contains("Scene"),
                format!(
                    "`{MODULE_FILE}` names {named_in_module} of {} node(s); builds a Scene: {}",
                    NODES.len(),
                    module.contains("Scene")
                ),
            ),
            SELF_CONTAINED.full_or_zero(
                external.is_empty(),
                format!("external reference(s): {external:?}"),
            ),
            SUMMARY_REPORTED.full_or_zero(
                observation.response.contains(&summary),
                format!("expected `{summary}` in the response"),
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
        let root = workspace::root(ID, run_id);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "nodes": observed_nodes(run_id),
                "module_present": workspace::read(&root, MODULE_FILE).is_some(),
                "files": workspace::files_under(&root, "scene"),
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_scene_graph_before_cleanup",
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
    fn the_expected_graph_has_two_roots() {
        let roots = expected_graph()
            .iter()
            .filter(|node| node.get("parent") == Some(&Value::Null))
            .count();
        assert_eq!(roots, ROOTS);
    }

    #[test]
    fn node_comparison_ignores_declaration_order() {
        let mut shuffled = expected_graph();
        shuffled.reverse();
        assert_eq!(
            kit::sorted_by(&shuffled, "name"),
            kit::sorted_by(&expected_graph(), "name")
        );
    }
}
