//! Build a working system from a prompt, then verify it by using it.
//!
//! These scenarios do not grade a description of a system, a plan for one, or
//! a file that looks like one. The runner takes what the session produced,
//! runs it against inputs planted after the session ended, and compares the
//! behaviour against its own reference. A system that hard-codes the sample it
//! was shown fails on first contact with the held-out one.

pub(in crate::scenarios) mod repo;
pub mod security_scanner;

use serde_json::Value;

use crate::scenarios::assessment::AssessmentSpec;
use crate::scenarios::kit;
use crate::scenarios::{ComplexityProfile, DeliverableContract, ScenarioCase};

pub(in crate::scenarios) const DELIVERABLE_KIND: &str = "built_system";
pub(in crate::scenarios) const CAPABILITIES: &[&str] =
    &["e2e::control-plane-v1", "iii::functions", "iii::filesystem"];

/// Building a system is planning-heavy, long-running, and produces one
/// deliverable: the system itself.
pub(in crate::scenarios) fn system_profile(
    dependency_depth: u8,
    ambiguity_level: u8,
) -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 4,
        dependency_depth,
        external_systems: 1,
        state_transitions: 8,
        artifact_count: 1,
        coordination_edges: 2,
        ambiguity_level,
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
        131_072,
    )
}
