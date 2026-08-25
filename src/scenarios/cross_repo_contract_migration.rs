//! Adaptive, three-repository compatibility migration with a hidden consumer.

use anyhow::bail;
use serde_json::json;

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::{
    ComplexityProfile, CriterionSpec, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    ExecutionRealism, HumanHorizon, MaterializedScenario, ScenarioCase, ScenarioCharacterization,
    ScenarioObservation, ScenarioSpec, ShadowMode,
};

pub const ID: &str = "cross_repo_contract_migration";
pub const VERSION: u32 = 1;
pub const CANONICAL_SEED: u64 = 0x6372_6f73_7372_0001;

const VISIBLE_MIGRATION: CriterionSpec = CriterionSpec::required_deterministic(
    "visible_contract_migration",
    20,
    "The producer and initially visible consumer migrate without breaking the old contract.",
    EvaluationDimension::Deliverable,
);
const CANARY_INVALIDATION: CriterionSpec = CriterionSpec::required_deterministic(
    "hidden_consumer_invalidation",
    20,
    "The trusted canary reveals consumer B only after plan one and produces evidence that requires plan two.",
    EvaluationDimension::StructuralIntegrity,
);
const COMPATIBILITY_MATRIX: CriterionSpec = CriterionSpec::required_deterministic(
    "three_repo_compatibility",
    30,
    "Old and new clients pass against the final producer contract across all three repositories.",
    EvaluationDimension::Deliverable,
);
const WORKSPACE_BOUNDARIES: CriterionSpec = CriterionSpec::required_deterministic(
    "workspace_boundaries",
    20,
    "Only allowed paths change; Git provenance is deterministic and no network or outside-root write occurs.",
    EvaluationDimension::StructuralIntegrity,
);
const MIGRATION_RECONCILIATION: CriterionSpec = CriterionSpec::required_deterministic(
    "migration_reconciliation",
    10,
    "One terminal rollout, a complete migration manifest, and cleanup reconcile the dedicated fixture repositories.",
    EvaluationDimension::StructuralIntegrity,
);

pub const CRITERIA: [CriterionSpec; 5] = [
    VISIBLE_MIGRATION,
    CANARY_INVALIDATION,
    COMPATIBILITY_MATRIX,
    WORKSPACE_BOUNDARIES,
    MIGRATION_RECONCILIATION,
];

pub fn scenario(_run_id: &str) -> ScenarioSpec {
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: "Migrate a versioned producer contract across the repositories currently available to you while preserving backwards compatibility. Treat trusted canary evidence as authoritative, replan when a later consumer is revealed, validate the full compatibility matrix, and leave repository cleanup to the runner.".into(),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 1,
            max_output_tokens: None,
            max_total_tokens: Some(700_000),
            stuck_timeout_seconds: 900,
            max_validation_retries: None,
        },
        denied_functions: &[],
        criteria: CRITERIA.to_vec(),
        judge_reference: None,
        setup: None,
        evaluate: adaptive_only_evaluator,
        cleanup: None,
    }
}

pub fn materialize(namespace: &str, _seed: u64) -> anyhow::Result<MaterializedScenario> {
    let fixture_manifest = json!({
        "producer_contract_sha256": crate::artifact::sha256_value(&include_str!(
            "../../fixtures/cross_repo_contract_migration/producer/contract.json"
        ))?,
        "consumer_a_expectation_sha256": crate::artifact::sha256_value(&include_str!(
            "../../fixtures/cross_repo_contract_migration/consumer-a/expectation.json"
        ))?,
        "consumer_b_expectation_sha256": crate::artifact::sha256_value(&include_str!(
            "../../fixtures/cross_repo_contract_migration/hidden/consumer-b/expectation.json"
        ))?,
    });
    let case = ScenarioCase::new(
        ID,
        VERSION,
        CANONICAL_SEED,
        json!({
            "variant": "producer_two_consumers_hidden_canary",
            "fixture_manifest": fixture_manifest,
            "initially_visible_repositories": ["producer", "consumer-a"],
            "trusted_canary_evidence_id": crate::workflow::cross_repo_contract_migration::CANARY_EVIDENCE_ID,
            "maximum_plan_revisions": 2,
            "network_allowed": false,
            "workflow_resource_budgets": {
                "max_parallel": 3,
                "max_nodes": 20,
                "step_timeout_seconds": 900,
                "workflow_timeout_seconds": 5400,
                "max_total_tokens": 636000,
                "planner_max_total_tokens": 64000,
                "max_cost_usd": 25.0,
                "technical_retries": 0,
            },
        }),
        complexity_profile(),
        vec![
            "e2e::adaptive-flow-v1".into(),
            "e2e::workflow-resume-v1".into(),
            "git::deterministic-fixture-v1".into(),
            "cross_repo_contract_simulator::v1".into(),
        ],
        DeliverableContract::default(),
    )?
    .with_characterization(ScenarioCharacterization::new(
        HumanHorizon::author_estimate(90, 180)?,
        ExecutionRealism::RealisticSimulator,
        ShadowMode::None,
    )?)?;
    Ok(MaterializedScenario {
        spec: scenario(namespace),
        case,
        capture: None,
    })
}

pub fn complexity_profile() -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 7,
        dependency_depth: 8,
        parallel_branches: 3,
        external_systems: 3,
        state_transitions: 14,
        wake_cycles: 1,
        validation_loops: 3,
        artifact_count: 9,
        coordination_edges: 16,
        ambiguity_level: 8,
        agent_owned_decomposition: true,
        material_invalidation_events: 1,
        replan_loops: 1,
        compensable_mutations: 1,
        durable_resume_cycles: 1,
        coherent_long_horizon: true,
    }
}

fn adaptive_only_evaluator<'a>(
    _context: &'a E2eContext,
    _observation: &'a ScenarioObservation,
    _run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        bail!("cross_repo_contract_migration must run through the registered AdaptiveFlow driver")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_case_is_l5_and_hides_consumer_b() {
        let case = materialize("attempt", 99).unwrap().case;
        assert_eq!(case.seed, CANONICAL_SEED);
        assert_eq!(
            case.complexity.tier,
            super::super::ComplexityTier::L5Adaptive
        );
        assert_eq!(case.characterization.human_horizon.min_minutes, Some(90));
        assert_eq!(
            case.inputs["initially_visible_repositories"],
            json!(["producer", "consumer-a"])
        );
    }
}
