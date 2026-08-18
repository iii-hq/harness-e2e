//! Graph and loop engineering: dependency order, refusal on impossible
//! topologies, fan-out joins, convergent repair, and knowing when to stop.

pub mod checkpoint_resume;
pub mod cycle_refusal;
pub mod diamond_merge;
pub mod exact_iteration_budget;
pub mod fanout_join;
pub mod impossible_stop;
pub mod repair_convergence;
pub mod topological_order;

use serde_json::Value;

use crate::scenarios::assessment::AssessmentSpec;
use crate::scenarios::common::ObservedFunctionCall;
use crate::scenarios::kit;
use crate::scenarios::{ComplexityProfile, DeliverableContract, ScenarioCase};

pub(in crate::scenarios) const DELIVERABLE_KIND: &str = "orchestration_evidence";

pub(in crate::scenarios) const CAPABILITIES: &[&str] =
    &["e2e::control-plane-v1", "iii::functions", "iii::state"];

pub(in crate::scenarios) fn graph_profile(
    dependency_depth: u8,
    parallel_branches: u8,
    coordination_edges: u16,
) -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 3,
        dependency_depth,
        parallel_branches,
        external_systems: 1,
        state_transitions: 6,
        artifact_count: 1,
        coordination_edges,
        ambiguity_level: 3,
        ..ComplexityProfile::default()
    }
}

pub(in crate::scenarios) fn loop_profile(validation_loops: u8) -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 3,
        dependency_depth: 2,
        external_systems: 1,
        state_transitions: 4,
        validation_loops,
        artifact_count: 1,
        coordination_edges: 1,
        ambiguity_level: 4,
        ..ComplexityProfile::default()
    }
}

pub(in crate::scenarios) fn case(
    id: &'static str,
    version: u32,
    seed: u64,
    inputs: Value,
    profile: ComplexityProfile,
    extra_capabilities: &[&str],
    contract: DeliverableContract,
) -> anyhow::Result<ScenarioCase> {
    kit::family_case(
        id,
        version,
        seed,
        inputs,
        profile,
        CAPABILITIES,
        extra_capabilities,
        contract,
    )
}

pub(in crate::scenarios) fn contract(
    deliverable_id: &str,
    schema: Value,
    assessments: &[AssessmentSpec],
) -> DeliverableContract {
    kit::contract(
        deliverable_id,
        DELIVERABLE_KIND,
        schema,
        assessments,
        32_768,
    )
}

/// The keys a run wrote into its own state scope, in transcript order.
pub(in crate::scenarios) fn written_keys(
    calls: &[ObservedFunctionCall],
    scope: &str,
) -> Vec<String> {
    calls
        .iter()
        .filter(|call| {
            call.function_id == "state::set"
                && call.arguments.get("scope").and_then(Value::as_str) == Some(scope)
        })
        .filter_map(|call| {
            call.arguments
                .get("key")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

/// Whether `order` lists every node exactly once and never places a node
/// before one it depends on.
pub(in crate::scenarios) fn respects_dependencies(
    order: &[String],
    dependencies: &[(&str, &[&str])],
) -> bool {
    if order.len() != dependencies.len() {
        return false;
    }
    let mut placed: Vec<&str> = Vec::with_capacity(order.len());
    for node in order {
        let Some((_, required)) = dependencies
            .iter()
            .find(|(candidate, _)| *candidate == node.as_str())
        else {
            return false;
        };
        if placed.contains(&node.as_str()) {
            return false;
        }
        if !required
            .iter()
            .all(|dependency| placed.contains(dependency))
        {
            return false;
        }
        placed.push(node.as_str());
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DEPENDENCIES: &[(&str, &[&str])] = &[("a", &[]), ("b", &["a"]), ("c", &["a", "b"])];

    fn order(nodes: &[&str]) -> Vec<String> {
        nodes.iter().map(|node| (*node).to_string()).collect()
    }

    #[test]
    fn a_valid_topological_order_is_accepted() {
        assert!(respects_dependencies(
            &order(&["a", "b", "c"]),
            DEPENDENCIES
        ));
    }

    #[test]
    fn an_order_that_runs_a_dependent_first_is_rejected() {
        assert!(!respects_dependencies(
            &order(&["b", "a", "c"]),
            DEPENDENCIES
        ));
        assert!(!respects_dependencies(&order(&["a", "b"]), DEPENDENCIES));
        assert!(!respects_dependencies(
            &order(&["a", "a", "b"]),
            DEPENDENCIES
        ));
    }

    #[test]
    fn only_writes_into_the_run_scope_are_counted() {
        let calls = vec![
            ObservedFunctionCall {
                function_id: "state::set".into(),
                arguments: json!({ "scope": "e2e:run", "key": "node:a" }),
            },
            ObservedFunctionCall {
                function_id: "state::set".into(),
                arguments: json!({ "scope": "other", "key": "node:b" }),
            },
        ];
        assert_eq!(written_keys(&calls, "e2e:run"), vec!["node:a".to_string()]);
    }
}
