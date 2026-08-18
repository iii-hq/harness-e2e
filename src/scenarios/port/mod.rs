//! Rewrites: take a working library in one language and produce one in
//! another that behaves identically and runs faster.
//!
//! These are the long ones. The work does not fit in a context window, so it
//! is the harness that decides whether the session still knows what it was
//! doing after hours of it: what survived compaction, what was delegated,
//! what had to be re-read. Correctness is not a matter of opinion here,
//! because the original is right there to be run: parity is byte-for-byte
//! against inputs the session never saw, and speed is measured rather than
//! claimed.

pub mod python_to_rust;

use serde_json::Value;

use crate::scenarios::assessment::AssessmentSpec;
use crate::scenarios::kit;
use crate::scenarios::{ComplexityProfile, DeliverableContract, ScenarioCase};

pub(in crate::scenarios) const DELIVERABLE_KIND: &str = "ported_system";
pub(in crate::scenarios) const CAPABILITIES: &[&str] = &[
    "e2e::control-plane-v1",
    "iii::functions",
    "iii::filesystem",
    "iii::shell",
];

/// A port is the deepest planning in the suite and produces one deliverable:
/// the rewritten system.
pub(in crate::scenarios) fn port_profile() -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 5,
        dependency_depth: 4,
        parallel_branches: 2,
        external_systems: 2,
        state_transitions: 40,
        validation_loops: 3,
        artifact_count: 1,
        coordination_edges: 4,
        ambiguity_level: 5,
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
