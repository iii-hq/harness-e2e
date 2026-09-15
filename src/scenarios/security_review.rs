use anyhow::{bail, Result};
use serde_json::json;

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::{
    async_trait, Capability, CriterionSpec, DeliverableContract, ExecutionPolicy,
    ObjectiveEvaluation, Scenario, ScenarioCase, ScenarioCharacterization, ScenarioExecutionKind,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "security_review";

pub struct SecurityReview;

#[async_trait]
impl Scenario for SecurityReview {
    fn id(&self) -> &'static str {
        ID
    }

    fn execution_kind(&self) -> ScenarioExecutionKind {
        ScenarioExecutionKind::CompositeFlow
    }

    fn characterization(&self) -> Result<ScenarioCharacterization> {
        Ok(ScenarioCharacterization::realistic())
    }

    fn case(&self, seed: u64) -> Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "variant": "full_local_security_scan",
                "repository": "iii-hq/security-scan-e2e-fixture",
                "fixture_source": "HARNESS_E2E_SECURITY_FIXTURE_PATH",
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::SecurityScanV1,
                Capability::GithubSecurityRead,
                Capability::SecurityScanOnDemand,
            ],
            DeliverableContract::default(),
        )
    }

    fn spec(&self, _run_id: &str) -> ScenarioSpec {
        ScenarioSpec {
            id: ID,
            // Composite scenarios do not send this text to Harness. It is retained as
            // the code-owned scenario purpose in the ordinary scenario contract.
            prompt: "Exercise the complete on-demand security-scan lifecycle against the manually prepared local fixture, including scan deduplication, optional suggestions, GitHub reconciliation, a second immediate exact-SHA scan, final listing, and repository integrity.".into(),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: 1,
                max_output_tokens: None,
                max_total_tokens: Some(500_000),
                stuck_timeout_seconds: 420,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: vec![
                CriterionSpec::scored(
                    "scan_a_detection",
                    60,
                    "The commit A scan identifies the seeded security capabilities while preserving every operational hard gate.",
                    EvaluationDimension::Deliverable,
                ),
                CriterionSpec::scored(
                    "suggest_a_quality",
                    20,
                    "When findings deterministically enable suggestions, the textual patches are useful and applicable without mutating the fixture.",
                    EvaluationDimension::Deliverable,
                ),
                CriterionSpec::scored(
                    "scan_b_detection",
                    20,
                    "An explicit request immediately creates and completes the commit B scan with coherent report evidence.",
                    EvaluationDimension::Deliverable,
                ),
            ],
        }
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        _observation: &ScenarioObservation,
        _run_id: &str,
    ) -> Result<ObjectiveEvaluation> {
        bail!("security_review must be executed through the registered CompositeFlow driver")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::ScenarioId;

    #[test]
    fn materialized_contract_is_stable_and_complex() {
        let first = ScenarioId::SecurityReview
            .materialize("attempt-a", 42)
            .unwrap();
        let retry = ScenarioId::SecurityReview
            .materialize("attempt-b", 42)
            .unwrap();
        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert!(first
            .case
            .required_capabilities
            .contains(&Capability::SecurityScanOnDemand));
        assert!(!first
            .case
            .required_capabilities
            .iter()
            .any(|capability| capability.as_str().contains("cron")));
    }
}
