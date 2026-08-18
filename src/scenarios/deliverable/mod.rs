//! Build-shaped deliverables: scenes, simulations, sites, diagrams, charts,
//! contracts, worlds, and reports. Nothing here is graded on taste; every
//! gate is a structural fact recomputed from the produced files.

pub mod anomaly_report;
pub mod api_contract;
pub mod architecture_diagram;
pub mod game_simulation;
pub mod scene_graph;
pub mod static_site;
pub mod svg_chart;
pub(in crate::scenarios) mod workspace;
pub mod world_bible;

use serde_json::Value;

use crate::scenarios::assessment::AssessmentSpec;
use crate::scenarios::kit;
use crate::scenarios::{ComplexityProfile, DeliverableContract, ScenarioCase};

pub(in crate::scenarios) const DELIVERABLE_KIND: &str = "workspace_artifact";

pub(in crate::scenarios) fn capabilities(extra: &[&str]) -> Vec<String> {
    let mut capabilities = vec![
        "e2e::control-plane-v1".to_string(),
        "iii::functions".to_string(),
        "iii::filesystem".to_string(),
    ];
    capabilities.extend(extra.iter().map(|capability| (*capability).to_string()));
    capabilities
}

/// `files` is how many files the scenario asks for; the deliverable itself is
/// always the single captured evidence artifact.
pub(in crate::scenarios) fn build_profile(files: u16, ambiguity_level: u8) -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 3,
        dependency_depth: 2,
        external_systems: 1,
        state_transitions: 3 + files,
        artifact_count: 1,
        coordination_edges: 1,
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
        65_536,
    )
}
