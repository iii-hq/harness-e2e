use std::collections::BTreeSet;

use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{json, Value};

use crate::artifact;
use crate::assessment::{AssessmentOutcome, RunAssessmentContract, SystemStatus};
use crate::report::{E2eReport, E2eScenarioReport};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub(super) struct AssessmentOutcomeCounts {
    pub passed: usize,
    pub failed: usize,
    pub partial: usize,
    pub not_evaluated: usize,
    pub unavailable: usize,
    pub error: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub(super) struct AssetValidationCounts {
    pub valid: usize,
    pub invalid: usize,
    pub malformed: usize,
    pub oversized: usize,
    pub not_produced: usize,
    pub unreadable: usize,
    pub unsafe_path: usize,
    pub removed_during_cleanup: usize,
    pub unexpected: usize,
    pub not_evaluated: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub(super) struct StatusCounts {
    pub unavailable: usize,
    pub passed: usize,
    pub hard_gate_failed: usize,
    pub subject_error: usize,
    pub judge_error: usize,
    pub resource_limit: usize,
    pub infrastructure_error: usize,
}

/// Objective assessment totals for a set of runs. Every count is derived from
/// deterministic evidence: hard gates, deterministic criteria, and asset
/// validation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub(super) struct AssessmentSummary {
    pub run_count: usize,
    pub assessment_count: usize,
    pub asset_count: usize,
    pub evidence_reference_count: usize,
    pub system_statuses: StatusCounts,
    pub assessment_outcomes: AssessmentOutcomeCounts,
    pub asset_validation_outcomes: AssetValidationCounts,
}

pub(super) fn summarize<'a>(
    contracts: impl IntoIterator<Item = &'a RunAssessmentContract>,
) -> AssessmentSummary {
    let mut summary = AssessmentSummary::default();
    let mut evidence = BTreeSet::new();

    for contract in contracts {
        summary.run_count += 1;
        increment_system_status(&mut summary.system_statuses, contract.system_status);

        for assessment in &contract.assessments {
            summary.assessment_count += 1;
            increment_assessment_outcome(&mut summary.assessment_outcomes, assessment.outcome);
            for reference in &assessment.evidence {
                evidence.insert(evidence_identity(reference));
            }
        }
        for asset in &contract.assets {
            summary.asset_count += 1;
            increment_asset_validation(&mut summary.asset_validation_outcomes, asset.outcome);
            for reference in &asset.evidence {
                evidence.insert(evidence_identity(reference));
            }
        }
    }

    summary.evidence_reference_count = evidence.len();
    summary
}

pub(super) fn contracts_for_scenario<'a>(
    report: &'a E2eReport,
    scenario: &E2eScenarioReport,
) -> Vec<&'a RunAssessmentContract> {
    let identities = scenario
        .runs
        .iter()
        .map(|run| (run.run_id.as_str(), run.attempt_id.as_str()))
        .collect::<BTreeSet<_>>();
    report
        .assessment_contract
        .runs
        .iter()
        .filter(|contract| {
            identities.contains(&(contract.run_id.as_str(), contract.attempt_id.as_str()))
        })
        .collect()
}

pub(super) fn assessment_profile_sha256(
    scenario_version: u32,
    contracts: &[&RunAssessmentContract],
) -> Result<String> {
    let mut definitions = BTreeSet::new();
    for contract in contracts {
        for assessment in &contract.assessments {
            definitions.insert(serde_json::to_string(&json!({
                "criterion_id": assessment.criterion_id,
                "target": assessment.target,
                "kind": assessment.kind,
                "policy": assessment.policy,
                "dimension": assessment.dimension,
            }))?);
        }
    }
    artifact::sha256_value(&json!({
        "scenario_version": scenario_version,
        "assessments": definitions,
    }))
}

pub(super) fn project_scenario_report(
    report: &E2eReport,
    scenario: &E2eScenarioReport,
) -> Result<Value> {
    let contracts = contracts_for_scenario(report, scenario);
    let mut value = serde_json::to_value(report)?;
    value["passed"] = json!(scenario.passed);
    value["scenarios"] = json!([scenario]);
    value["assessment_contract"] = json!({
        "runs": contracts,
    });
    let contract_by_run = contracts
        .iter()
        .map(|contract| {
            (
                (contract.run_id.as_str(), contract.attempt_id.as_str()),
                *contract,
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    if let Some(runs) = value
        .pointer_mut("/scenarios/0/runs")
        .and_then(Value::as_array_mut)
    {
        for run in runs {
            let identity = run
                .get("run_id")
                .and_then(Value::as_str)
                .zip(run.get("attempt_id").and_then(Value::as_str));
            if let Some(contract) = identity.and_then(|identity| contract_by_run.get(&identity)) {
                run["assessment"] = serde_json::to_value(contract)?;
            }
        }
    }
    value["assessment_summary"] = serde_json::to_value(summarize(contracts))?;
    Ok(value)
}

fn evidence_identity(reference: &crate::assessment::EvidenceReference) -> String {
    format!(
        "{}\0{}\0{}",
        reference.artifact_id,
        reference.artifact_sha256,
        reference.locator.as_deref().unwrap_or_default()
    )
}

fn increment_assessment_outcome(counts: &mut AssessmentOutcomeCounts, value: AssessmentOutcome) {
    match value {
        AssessmentOutcome::Passed => counts.passed += 1,
        AssessmentOutcome::Failed => counts.failed += 1,
        AssessmentOutcome::Partial => counts.partial += 1,
        AssessmentOutcome::NotEvaluated => counts.not_evaluated += 1,
        AssessmentOutcome::Unavailable => counts.unavailable += 1,
        AssessmentOutcome::Error => counts.error += 1,
    }
}

fn increment_asset_validation(
    counts: &mut AssetValidationCounts,
    value: crate::assessment::AssetValidationOutcome,
) {
    use crate::assessment::AssetValidationOutcome as Outcome;
    match value {
        Outcome::Valid => counts.valid += 1,
        Outcome::Invalid => counts.invalid += 1,
        Outcome::Malformed => counts.malformed += 1,
        Outcome::Oversized => counts.oversized += 1,
        Outcome::NotProduced => counts.not_produced += 1,
        Outcome::Unreadable => counts.unreadable += 1,
        Outcome::UnsafePath => counts.unsafe_path += 1,
        Outcome::RemovedDuringCleanup => counts.removed_during_cleanup += 1,
        Outcome::Unexpected => counts.unexpected += 1,
        Outcome::NotEvaluated => counts.not_evaluated += 1,
    }
}

fn increment_system_status(counts: &mut StatusCounts, value: SystemStatus) {
    match value {
        SystemStatus::Unavailable => counts.unavailable += 1,
        SystemStatus::Passed => counts.passed += 1,
        SystemStatus::HardGateFailed => counts.hard_gate_failed += 1,
        SystemStatus::SubjectError => counts.subject_error += 1,
        SystemStatus::JudgeError => counts.judge_error += 1,
        SystemStatus::ResourceLimit => counts.resource_limit += 1,
        SystemStatus::InfrastructureError => counts.infrastructure_error += 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assessment::AssessmentContract;

    #[test]
    fn shared_fixture_matches_dashboard_summary_and_profiles() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/results/results-assessment-contract.json"
        ))
        .unwrap();
        let contract: AssessmentContract =
            serde_json::from_value(fixture["assessment_contract"].clone()).unwrap();
        let expected = &fixture["dashboard_projection"];
        let runs = contract.runs.iter().collect::<Vec<_>>();

        assert_eq!(
            serde_json::to_value(summarize(runs.iter().copied())).unwrap(),
            expected["summary"]
        );
        assert_eq!(
            assessment_profile_sha256(
                expected["scenario_version"].as_u64().unwrap() as u32,
                &runs,
            )
            .unwrap(),
            expected["assessment_profile_sha256"]
        );
    }
}
