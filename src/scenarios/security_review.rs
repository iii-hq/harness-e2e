use anyhow::bail;
use serde_json::json;

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::{
    CriterionSpec, DeliverableContract, EvaluationFuture, ExecutionPolicy, MaterializedScenario,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "security_review";

pub fn scenario(_run_id: &str) -> ScenarioSpec {
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
        setup: None,
        evaluate: composite_only_evaluator,
        cleanup: None,
    }
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let spec = scenario(namespace);
    let case = ScenarioCase::new(
        ID,
        seed,
        json!({
            "variant": "full_local_security_scan",
            "repository": "iii-hq/security-scan-e2e-fixture",
            "fixture_source": "HARNESS_E2E_SECURITY_FIXTURE_PATH",
        }),
        vec![
            "e2e::control-plane-v1".to_string(),
            "security_scan::v1".to_string(),
            "github::security-read".to_string(),
            "security_scan::on-demand".to_string(),
        ],
        DeliverableContract::default(),
    )?;
    Ok(MaterializedScenario {
        spec,
        case,
        capture: None,
    })
}

fn composite_only_evaluator<'a>(
    _context: &'a E2eContext,
    _observation: &'a ScenarioObservation,
    _run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        bail!("security_review must be executed through the registered CompositeFlow driver")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materialized_contract_is_stable_and_complex() {
        let first = materialize("attempt-a", 42).unwrap();
        let retry = materialize("attempt-b", 42).unwrap();
        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert!(first
            .case
            .required_capabilities
            .contains(&"security_scan::on-demand".to_string()));
        assert!(!first
            .case
            .required_capabilities
            .iter()
            .any(|capability| capability.contains("cron")));
    }
}
