//! Adaptive recovery of an immutable release train and a stale promotion.

use anyhow::bail;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::{
    ComplexityProfile, CriterionSpec, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    ExecutionRealism, HumanHorizon, MaterializedScenario, ScenarioCase, ScenarioCharacterization,
    ScenarioObservation, ScenarioSpec, ShadowMode,
};

pub const ID: &str = "release_train_recovery";
pub const VERSION: u32 = 1;
pub const CANONICAL_SEED: u64 = 0x7265_6c65_6173_0001;

const IMMUTABLE_RECOVERY: CriterionSpec = CriterionSpec::required_deterministic(
    "immutable_release_recovery",
    25,
    "The same immutable tag, version, and run id recover through a later run attempt without retagging or version drift.",
    EvaluationDimension::StructuralIntegrity,
);
const PUBLICATION_INTEGRITY: CriterionSpec = CriterionSpec::required_deterministic(
    "exact_publication_integrity",
    20,
    "All expected assets exist and the exact Registry version resolves before promotion planning continues.",
    EvaluationDimension::Deliverable,
);
const EVIDENCE_BOUND_REPLAN: CriterionSpec = CriterionSpec::required_deterministic(
    "evidence_bound_replan",
    20,
    "The incompatible latest graph invalidates plan one and plan two cites the trusted preview evidence.",
    EvaluationDimension::StructuralIntegrity,
);
const SAFE_CAS_PROMOTION: CriterionSpec = CriterionSpec::required_deterministic(
    "safe_cas_promotion",
    25,
    "A fresh gated operation preserves the real latest pointer and performs one authorized CAS without retrying the stale operation.",
    EvaluationDimension::Deliverable,
);
const RELEASE_RECONCILIATION: CriterionSpec = CriterionSpec::required_deterministic(
    "release_reconciliation",
    10,
    "Canary convergence, locks, audit state, secret hygiene, and cleanup reconcile after the single terminal promotion.",
    EvaluationDimension::StructuralIntegrity,
);

pub const CRITERIA: [CriterionSpec; 5] = [
    IMMUTABLE_RECOVERY,
    PUBLICATION_INTEGRITY,
    EVIDENCE_BOUND_REPLAN,
    SAFE_CAS_PROMOTION,
    RELEASE_RECONCILIATION,
];

pub fn scenario(_run_id: &str) -> ScenarioSpec {
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: "Recover a partially published immutable Workers release, verify exact publication, then safely replan a promotion when the historical latest graph is incompatible. Preserve the original tag/version/run identity, use evidence-gated operations, never mutate latest directly, and reconcile the final state.".into(),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 1,
            max_output_tokens: None,
            max_total_tokens: Some(900_000),
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
    let initial: Value = serde_json::from_str(include_str!(
        "../../fixtures/release_train_recovery/initial_state.json"
    ))?;
    let fixture_sha256 = crate::artifact::sha256_value(&initial)?;
    let case = ScenarioCase::new(
        ID,
        VERSION,
        CANONICAL_SEED,
        json!({
            "variant": "partial_publication_then_incompatible_latest",
            "fixture_sha256": fixture_sha256,
            "initial_identity": {
                "tag": initial["immutable_tag"],
                "version": initial["version"],
                "run_id": initial["github_run"]["run_id"],
                "run_attempt": initial["github_run"]["run_attempt"],
            },
            "invalidation_evidence_id": crate::workflow::release_train_recovery::INVALIDATION_EVIDENCE_ID,
            "maximum_plan_revisions": 2,
            "shadow": {
                "mode": "read_only",
                "objective_authority": false,
                "missing_outcome": "not_evaluated",
            },
            "workflow_resource_budgets": {
                "max_parallel": 3,
                "max_nodes": 24,
                "step_timeout_seconds": 900,
                "workflow_timeout_seconds": 7200,
                "max_total_tokens": 836000,
                "planner_max_total_tokens": 64000,
                "max_cost_usd": 30.0,
                "technical_retries": 0,
            },
        }),
        complexity_profile(),
        vec![
            "e2e::adaptive-flow-v1".into(),
            "e2e::workflow-resume-v1".into(),
            "release_train_simulator::v1".into(),
            "release_shadow::read-only-v1".into(),
        ],
        DeliverableContract::default(),
    )?
    .with_characterization(ScenarioCharacterization::new(
        HumanHorizon::author_estimate(120, 240)?,
        ExecutionRealism::RealisticSimulator,
        ShadowMode::ReadOnly,
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
        dependency_depth: 9,
        parallel_branches: 3,
        external_systems: 4,
        state_transitions: 16,
        wake_cycles: 2,
        validation_loops: 3,
        artifact_count: 10,
        coordination_edges: 18,
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
        bail!("release_train_recovery must run through the registered AdaptiveFlow driver")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_case_is_l5_realistic_and_shadowed() {
        let case = materialize("attempt", 99).unwrap().case;
        assert_eq!(case.seed, CANONICAL_SEED);
        assert_eq!(
            case.complexity.tier,
            super::super::ComplexityTier::L5Adaptive
        );
        assert_eq!(case.characterization.human_horizon.min_minutes, Some(120));
        assert_eq!(case.characterization.realism.shadow, ShadowMode::ReadOnly);
        assert!(case.deliverable_contract.artifacts.is_empty());
    }
}
