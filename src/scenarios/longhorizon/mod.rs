//! Long-horizon scenarios scored for comparison rather than pass or fail.
//!
//! A task benchmark measures a harness when the model is held fixed and the
//! harness varies: the delta between two executions is the signal. These
//! scenarios are built for that reading. Only the stack-attributable
//! properties are hard-gated, so a weaker model moves the score instead of
//! failing the gate, and the outcome and efficiency numbers are advisory so
//! `e2e::compare` can carry them as deltas between two subject revisions.

pub mod hidden_rule_world;
pub(in crate::scenarios) mod world;

use serde_json::Value;

use crate::scenarios::assessment::AssessmentSpec;
use crate::scenarios::kit;
use crate::scenarios::{ComplexityProfile, DeliverableContract, ScenarioCase};

pub(in crate::scenarios) const DELIVERABLE_KIND: &str = "exploration_record";
pub(in crate::scenarios) const CAPABILITIES: &[&str] = &["e2e::control-plane-v1", "iii::functions"];

/// Learning by acting: little planning depth, many state transitions, and the
/// highest ambiguity in the suite, because the rules are withheld by design.
pub(in crate::scenarios) fn exploration_profile() -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 3,
        dependency_depth: 2,
        external_systems: 1,
        state_transitions: 30,
        validation_loops: 2,
        artifact_count: 1,
        ambiguity_level: 9,
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
