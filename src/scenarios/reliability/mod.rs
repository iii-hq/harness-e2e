//! Reliability regressions: dependencies that vanish, lie, fail transiently,
//! fail permanently, or redeliver. Each one reproduces a stack failure that
//! was observed on a live rig and grades the agent's recovery.

pub mod amplification_bound;
pub mod binding_hygiene;
pub mod idempotent_apply;
pub mod missing_function;
pub mod permanent_stop;
pub mod stale_counter;
pub mod transient_recovery;
pub mod vanishing_function;

use serde_json::Value;

use crate::scenarios::assessment::AssessmentSpec;
use crate::scenarios::kit;
use crate::scenarios::{ComplexityProfile, DeliverableContract, ScenarioCase};

pub(in crate::scenarios) const DELIVERABLE_KIND: &str = "reliability_evidence";

pub(in crate::scenarios) fn capabilities(extra: &[&str]) -> Vec<String> {
    let mut capabilities = vec![
        "e2e::control-plane-v1".to_string(),
        "iii::functions".to_string(),
    ];
    capabilities.extend(extra.iter().map(|capability| (*capability).to_string()));
    capabilities
}

pub(in crate::scenarios) fn probe_profile(
    external_systems: u8,
    state_transitions: u16,
) -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 2,
        dependency_depth: 1,
        external_systems,
        state_transitions,
        artifact_count: 1,
        ambiguity_level: 3,
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
    ScenarioCase::new(
        id,
        version,
        seed,
        inputs,
        profile,
        capabilities(extra_capabilities),
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
