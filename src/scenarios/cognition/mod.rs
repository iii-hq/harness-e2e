//! Context, memory, delegation, and trust: the failures that come from what
//! the agent believed rather than from what the stack did.

pub mod goal_drift;
pub mod injection_resistance;
pub mod instruction_precedence;
pub mod stale_memory_refresh;
pub mod subagent_context_handoff;
pub mod subagent_scope;

use serde_json::Value;

use crate::scenarios::assessment::AssessmentSpec;
use crate::scenarios::kit;
use crate::scenarios::{ComplexityProfile, DeliverableContract, ScenarioCase};

pub(in crate::scenarios) const DELIVERABLE_KIND: &str = "cognition_evidence";

pub(in crate::scenarios) const CAPABILITIES: &[&str] =
    &["e2e::control-plane-v1", "iii::functions", "iii::state"];

pub(in crate::scenarios) fn context_profile(
    ambiguity_level: u8,
    state_transitions: u16,
) -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 2,
        dependency_depth: 1,
        external_systems: 1,
        state_transitions,
        artifact_count: 1,
        ambiguity_level,
        ..ComplexityProfile::default()
    }
}

pub(in crate::scenarios) fn delegation_profile(
    parallel_branches: u8,
    coordination_edges: u16,
) -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 3,
        dependency_depth: 3,
        parallel_branches,
        external_systems: 1,
        state_transitions: 4,
        artifact_count: 1,
        coordination_edges,
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

pub(in crate::scenarios) fn child_session(run_id: &str, index: usize) -> String {
    format!("e2e_{run_id}-child-{index}")
}
