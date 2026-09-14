//! Adaptive recovery of an immutable release train and a stale promotion.

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::{
    async_trait, Capability, CriterionSpec, DeliverableContract, ExecutionPolicy, ExecutionRealism,
    HumanHorizon, ObjectiveEvaluation, Scenario, ScenarioCase, ScenarioCharacterization,
    ScenarioExecutionKind, ScenarioObservation, ScenarioSpec, ShadowMode,
};

pub const ID: &str = "release_train_recovery";
pub const CANONICAL_SEED: u64 = 0x7265_6c65_6173_0001;

const IMMUTABLE_RECOVERY: CriterionSpec = CriterionSpec::scored(
    "immutable_release_recovery",
    25,
    "The same immutable tag, version, and run id recover through a later run attempt without retagging or version drift.",
    EvaluationDimension::StructuralIntegrity,
);
const PUBLICATION_INTEGRITY: CriterionSpec = CriterionSpec::scored(
    "exact_publication_integrity",
    20,
    "All expected assets exist and the exact Registry version resolves before promotion planning continues.",
    EvaluationDimension::Deliverable,
);
const EVIDENCE_BOUND_REPLAN: CriterionSpec = CriterionSpec::scored(
    "evidence_bound_replan",
    20,
    "The incompatible latest graph invalidates plan one and plan two cites the trusted preview evidence.",
    EvaluationDimension::StructuralIntegrity,
);
const SAFE_CAS_PROMOTION: CriterionSpec = CriterionSpec::scored(
    "safe_cas_promotion",
    25,
    "A fresh gated operation preserves the real latest pointer and performs one authorized CAS without retrying the stale operation.",
    EvaluationDimension::Deliverable,
);
const RELEASE_RECONCILIATION: CriterionSpec = CriterionSpec::scored(
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

pub struct ReleaseTrainRecovery;

#[async_trait]
impl Scenario for ReleaseTrainRecovery {
    fn id(&self) -> &'static str {
        ID
    }

    fn execution_kind(&self) -> ScenarioExecutionKind {
        ScenarioExecutionKind::AdaptiveFlow
    }

    fn canonical_seed(&self) -> u64 {
        CANONICAL_SEED
    }

    fn canonical_seed_only(&self) -> bool {
        true
    }

    fn characterization(&self) -> Result<ScenarioCharacterization> {
        ScenarioCharacterization::new(
            HumanHorizon::author_estimate(120, 240)?,
            ExecutionRealism::RealisticSimulator,
            ShadowMode::ReadOnly,
        )
    }

    fn case(&self, _seed: u64) -> Result<ScenarioCase> {
        let initial: Value = serde_json::from_str(include_str!(
            "../../fixtures/release_train_recovery/initial_state.json"
        ))?;
        let fixture_sha256 = crate::artifact::sha256_value(&initial)?;
        ScenarioCase::new(
            ID,
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
            vec![
                Capability::E2eAdaptiveFlowV1,
                Capability::E2eWorkflowResumeV1,
                Capability::ReleaseTrainSimulatorV1,
                Capability::ReleaseShadowReadOnlyV1,
            ],
            DeliverableContract::default(),
        )
    }

    fn spec(&self, _run_id: &str) -> ScenarioSpec {
        ScenarioSpec {
            id: ID,
            prompt: "Recover a partially published immutable Workers release, verify exact publication, then safely replan a promotion when the historical latest graph is incompatible. Preserve the original tag/version/run identity, use evidence-gated operations, never mutate latest directly, and reconcile the final state.".into(),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(1),
                max_output_tokens: None,
                max_total_tokens: Some(900_000),
                stuck_timeout_seconds: Some(900),
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: CRITERIA.to_vec(),
        }
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        _observation: &ScenarioObservation,
        _run_id: &str,
    ) -> Result<ObjectiveEvaluation> {
        bail!("release_train_recovery must run through the registered AdaptiveFlow driver")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::scenarios::ScenarioId;

    #[test]
    fn canonical_case_is_realistic_and_shadowed() {
        let case = ScenarioId::ReleaseTrainRecovery
            .materialize("attempt", 99)
            .unwrap()
            .case;
        assert_eq!(case.seed, CANONICAL_SEED);
        assert_eq!(case.characterization.human_horizon.min_minutes, Some(120));
        assert_eq!(case.characterization.realism.shadow, ShadowMode::ReadOnly);
        assert!(case.deliverable_contract.artifacts.is_empty());
    }
}
