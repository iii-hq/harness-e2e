use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::artifact::ArtifactReference;
use crate::report::{E2eReport, RunStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentKind {
    RequiredCheck,
    Signal,
    AssetValidation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentPolicy {
    HardGate,
    Advisory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentOutcome {
    Passed,
    Failed,
    Partial,
    NotEvaluated,
    Unavailable,
    Error,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentTargetKind {
    Criterion,
}

/// Canonical assessment metadata retained from the scenario declaration until
/// the per-attempt result is materialized. Every assessment is deterministic:
/// built-in scenarios never delegate a criterion to a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredAssessment {
    pub criterion_id: String,
    pub possible: u8,
    pub description: String,
    pub kind: AssessmentKind,
    pub policy: AssessmentPolicy,
    pub dimension: crate::report::EvaluationDimension,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AssessmentTarget {
    pub kind: AssessmentTargetKind,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AssessmentScore {
    pub awarded: u8,
    pub possible: u8,
}

impl AssessmentScore {
    pub fn validate(&self) -> Result<()> {
        if self.possible == 0 {
            bail!("assessment score possible must be at least 1");
        }
        if self.awarded > self.possible {
            bail!(
                "assessment score awarded {} exceeds possible {}",
                self.awarded,
                self.possible
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceReference {
    pub artifact_id: String,
    pub artifact_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
}

impl EvidenceReference {
    fn validate(&self) -> Result<()> {
        required(&self.artifact_id, "evidence artifact id")?;
        validate_sha256(&self.artifact_sha256, "evidence artifact hash")?;
        if self
            .locator
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            bail!("evidence locator cannot be empty when present");
        }
        Ok(())
    }
}

impl From<&ArtifactReference> for EvidenceReference {
    fn from(reference: &ArtifactReference) -> Self {
        Self {
            artifact_id: reference.id.clone(),
            artifact_sha256: reference.sha256.clone(),
            locator: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnalyzerIdentity {
    pub analyzer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub input_sha256: String,
}

impl AnalyzerIdentity {
    pub fn validate(&self) -> Result<()> {
        required(&self.analyzer, "analyzer id")?;
        validate_sha256(&self.input_sha256, "analyzer input hash")?;
        if self
            .provider
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            bail!("analyzer provider cannot be empty when present");
        }
        if self
            .model
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            bail!("analyzer model cannot be empty when present");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AnalyzerUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AssessmentResult {
    pub criterion_id: String,
    pub target: AssessmentTarget,
    pub kind: AssessmentKind,
    pub policy: AssessmentPolicy,
    pub dimension: crate::report::EvaluationDimension,
    pub outcome: AssessmentOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<AssessmentScore>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceReference>,
}

impl AssessmentResult {
    pub fn validate(&self) -> Result<()> {
        required(&self.criterion_id, "assessment criterion id")?;
        required(&self.target.id, "assessment target id")?;
        required(&self.summary, "assessment summary")?;
        if let Some(score) = &self.score {
            score.validate()?;
        }
        if matches!(
            self.outcome,
            AssessmentOutcome::NotEvaluated
                | AssessmentOutcome::Unavailable
                | AssessmentOutcome::Error
        ) && self.score.is_some()
        {
            bail!(
                "unavailable assessment '{}' cannot have a score",
                self.criterion_id
            );
        }
        if self.outcome == AssessmentOutcome::Partial && self.score.is_none() {
            bail!(
                "partial assessment '{}' requires a score",
                self.criterion_id
            );
        }
        let mut evidence_ids = BTreeSet::new();
        for evidence in &self.evidence {
            evidence.validate()?;
            if !evidence_ids.insert((
                evidence.artifact_id.as_str(),
                evidence.artifact_sha256.as_str(),
                evidence.locator.as_deref(),
            )) {
                bail!(
                    "assessment '{}' repeats an evidence identity",
                    self.criterion_id
                );
            }
        }
        if self.policy == AssessmentPolicy::HardGate && self.kind != AssessmentKind::RequiredCheck {
            bail!(
                "hard-gated assessment '{}' must be a required check",
                self.criterion_id
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssetValidationOutcome {
    Valid,
    Invalid,
    Malformed,
    Oversized,
    NotProduced,
    Unreadable,
    UnsafePath,
    RemovedDuringCleanup,
    Unexpected,
    NotEvaluated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AssetValidationResult {
    pub asset_id: String,
    pub outcome: AssetValidationOutcome,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceReference>,
}

impl AssetValidationResult {
    fn validate(&self) -> Result<()> {
        required(&self.asset_id, "asset validation id")?;
        required(&self.summary, "asset validation summary")?;
        let mut evidence_ids = BTreeSet::new();
        for evidence in &self.evidence {
            evidence.validate()?;
            if !evidence_ids.insert((
                evidence.artifact_id.as_str(),
                evidence.artifact_sha256.as_str(),
                evidence.locator.as_deref(),
            )) {
                bail!(
                    "asset validation '{}' repeats an evidence identity",
                    self.asset_id
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SystemStatus {
    Unavailable,
    Passed,
    HardGateFailed,
    SubjectError,
    JudgeError,
    ResourceLimit,
    InfrastructureError,
}

impl From<RunStatus> for SystemStatus {
    fn from(status: RunStatus) -> Self {
        match status {
            RunStatus::Passed => Self::Passed,
            RunStatus::HardGateFailed => Self::HardGateFailed,
            RunStatus::SubjectError => Self::SubjectError,
            RunStatus::JudgeError => Self::JudgeError,
            RunStatus::ResourceLimit => Self::ResourceLimit,
            RunStatus::InfrastructureError => Self::InfrastructureError,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunAssessmentContract {
    pub run_id: String,
    pub attempt_id: String,
    pub system_status: SystemStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assessments: Vec<AssessmentResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<AssetValidationResult>,
}

impl RunAssessmentContract {
    fn validate(&self) -> Result<()> {
        required(&self.run_id, "assessment run id")?;
        required(&self.attempt_id, "assessment attempt id")?;
        let mut assessment_ids = BTreeSet::new();
        for assessment in &self.assessments {
            assessment.validate()?;
            let identity = (
                assessment.target.kind,
                assessment.target.id.as_str(),
                assessment.criterion_id.as_str(),
            );
            if !assessment_ids.insert(identity) {
                bail!(
                    "run '{}:{}' repeats an assessment result",
                    self.run_id,
                    self.attempt_id
                );
            }
        }
        let mut asset_ids = BTreeSet::new();
        for asset in &self.assets {
            asset.validate()?;
            if !asset_ids.insert(asset.asset_id.as_str()) {
                bail!(
                    "run '{}:{}' repeats asset '{}'",
                    self.run_id,
                    self.attempt_id,
                    asset.asset_id
                );
            }
        }
        Ok(())
    }

    fn evidence_references(&self) -> Vec<&EvidenceReference> {
        self.assessments
            .iter()
            .flat_map(|assessment| &assessment.evidence)
            .chain(self.assets.iter().flat_map(|asset| &asset.evidence))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssessmentContract {
    pub runs: Vec<RunAssessmentContract>,
}

type ExpectedRunEvidence<'a> = (SystemStatus, Vec<(&'a str, &'a str)>);
type ExpectedRuns<'a> = BTreeMap<(&'a str, &'a str), ExpectedRunEvidence<'a>>;

impl AssessmentContract {
    pub fn from_assessment_evidence(report: &E2eReport) -> Self {
        let runs = report
            .scenarios
            .iter()
            .flat_map(|scenario| &scenario.runs)
            .map(|run| RunAssessmentContract {
                run_id: run.run_id.clone(),
                attempt_id: run.attempt_id.clone(),
                system_status: SystemStatus::from(run.status),
                assessments: run.assessment_results.clone(),
                assets: run.asset_assessments.clone(),
            })
            .collect::<Vec<_>>();
        Self { runs }
    }

    fn expected_runs(report: &E2eReport) -> ExpectedRuns<'_> {
        let mut expected = BTreeMap::new();
        for run in report.scenarios.iter().flat_map(|scenario| &scenario.runs) {
            let evidence = run
                .evidence
                .iter()
                .chain(
                    run.deliverables
                        .iter()
                        .filter_map(|deliverable| deliverable.artifact.as_ref()),
                )
                .map(|artifact| (artifact.id.as_str(), artifact.sha256.as_str()))
                .collect();
            expected.insert(
                (run.run_id.as_str(), run.attempt_id.as_str()),
                (SystemStatus::from(run.status), evidence),
            );
        }
        expected
    }
    pub fn validate(&self, report: &E2eReport) -> Result<()> {
        let expected = Self::expected_runs(report);
        let expected_run_count = report
            .scenarios
            .iter()
            .map(|scenario| scenario.runs.len())
            .sum::<usize>();
        if expected.len() != expected_run_count {
            bail!("E2E report repeats a run/attempt identity");
        }
        let mut observed = BTreeSet::new();
        for run in &self.runs {
            run.validate()?;
            let identity = (run.run_id.as_str(), run.attempt_id.as_str());
            if !observed.insert(identity) {
                bail!(
                    "assessment contract repeats run '{}:{}'",
                    run.run_id,
                    run.attempt_id
                );
            }
            let (expected_status, expected_evidence) =
                expected.get(&identity).with_context(|| {
                    format!(
                        "assessment contract contains unknown run '{}:{}'",
                        run.run_id, run.attempt_id
                    )
                })?;
            if run.system_status != *expected_status {
                bail!(
                    "assessment run '{}:{}' system status {:?} differs from E2E run status {:?}",
                    run.run_id,
                    run.attempt_id,
                    run.system_status,
                    expected_status
                );
            }
            for reference in run.evidence_references() {
                let matches_artifact = expected_evidence.iter().any(|(id, sha256)| {
                    *id == reference.artifact_id && *sha256 == reference.artifact_sha256
                });
                if !matches_artifact {
                    bail!(
                        "assessment evidence '{}' is not present in run '{}:{}'",
                        reference.artifact_id,
                        run.run_id,
                        run.attempt_id
                    );
                }
            }
        }
        if observed.len() != expected.len() {
            bail!("assessment contract run identities differ from the E2E report");
        }
        Ok(())
    }
}

pub(crate) fn semantic_test_assessments(
    tests: &[crate::workflow::WorkflowStepReport],
    criteria: &[crate::workflow::WorkflowCriterionResult],
) -> Vec<AssessmentResult> {
    let evidence = tests
        .iter()
        .flat_map(|step| &step.assets)
        .map(|asset| (asset.artifact.id.as_str(), &asset.artifact))
        .collect::<BTreeMap<_, _>>();
    let references = |ids: &[String]| {
        ids.iter()
            .filter_map(|id| evidence.get(id.as_str()).copied())
            .map(EvidenceReference::from)
            .collect::<Vec<_>>()
    };
    let mut assessments = Vec::new();
    for step in tests {
        assessments.extend(step.evaluations.iter().map(|evaluation| {
            let score = evaluation.score.and_then(|value| {
                value.is_finite().then(|| AssessmentScore {
                    awarded: (value.clamp(0.0, 1.0) * 100.0).round() as u8,
                    possible: 100,
                })
            });
            AssessmentResult {
                criterion_id: format!("{}.{}", step.node_id, evaluation.id),
                target: AssessmentTarget {
                    kind: AssessmentTargetKind::Criterion,
                    id: step.node_id.clone(),
                },
                kind: AssessmentKind::Signal,
                policy: AssessmentPolicy::Advisory,
                dimension: crate::report::EvaluationDimension::Deliverable,
                outcome: match evaluation.outcome {
                    crate::workflow::WorkflowEvaluationOutcome::Passed => AssessmentOutcome::Passed,
                    crate::workflow::WorkflowEvaluationOutcome::Failed => AssessmentOutcome::Failed,
                    crate::workflow::WorkflowEvaluationOutcome::Advisory => {
                        if score.is_some() {
                            AssessmentOutcome::Partial
                        } else {
                            AssessmentOutcome::Passed
                        }
                    }
                    crate::workflow::WorkflowEvaluationOutcome::NotEvaluated => {
                        AssessmentOutcome::NotEvaluated
                    }
                },
                score,
                summary: evaluation.summary.clone(),
                evidence: references(&evaluation.evidence_ids),
            }
        }));
    }
    assessments.extend(criteria.iter().map(|criterion| {
        let score = criterion.score.and_then(|value| {
            value.is_finite().then(|| AssessmentScore {
                awarded: (value.clamp(0.0, 1.0) * f64::from(criterion.weight)).round() as u8,
                possible: criterion.weight,
            })
        });
        AssessmentResult {
            criterion_id: criterion.id.clone(),
            target: AssessmentTarget {
                kind: AssessmentTargetKind::Criterion,
                id: criterion.producer_node_id.clone(),
            },
            kind: AssessmentKind::Signal,
            policy: AssessmentPolicy::Advisory,
            dimension: crate::report::EvaluationDimension::Deliverable,
            outcome: match criterion.outcome {
                crate::workflow::WorkflowEvaluationOutcome::Passed => AssessmentOutcome::Passed,
                crate::workflow::WorkflowEvaluationOutcome::Failed => AssessmentOutcome::Failed,
                crate::workflow::WorkflowEvaluationOutcome::Advisory => {
                    if score.is_some() {
                        AssessmentOutcome::Partial
                    } else {
                        AssessmentOutcome::Passed
                    }
                }
                crate::workflow::WorkflowEvaluationOutcome::NotEvaluated => {
                    AssessmentOutcome::NotEvaluated
                }
            },
            score,
            summary: criterion.summary.clone(),
            evidence: references(&criterion.evidence_ids),
        }
    }));
    assessments
}

fn validate_sha256(value: &str, label: &str) -> Result<()> {
    let Some(digest) = value.strip_prefix("sha256:") else {
        bail!("{label} must use the sha256:<hex> format");
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("{label} is not a SHA-256 digest");
    }
    Ok(())
}

fn required(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{label} cannot be empty");
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    fn deterministic_result() -> AssessmentResult {
        AssessmentResult {
            criterion_id: "durable_result".into(),
            target: AssessmentTarget {
                kind: AssessmentTargetKind::Criterion,
                id: "durable_result".into(),
            },
            kind: AssessmentKind::RequiredCheck,
            policy: AssessmentPolicy::HardGate,
            dimension: crate::report::EvaluationDimension::StructuralIntegrity,
            outcome: AssessmentOutcome::Failed,
            score: Some(AssessmentScore {
                awarded: 0,
                possible: 70,
            }),
            summary: "The expected durable result was not observed.".into(),
            evidence: Vec::new(),
        }
    }

    #[test]
    fn workflow_criteria_preserve_points_without_approval_gates() {
        let criteria = [crate::workflow::WorkflowCriterionResult {
            id: "quality".into(),
            weight: 100,
            producer_node_id: "assess".into(),
            output_port: "quality".into(),
            advisory: false,
            outcome: crate::workflow::WorkflowEvaluationOutcome::Failed,
            summary: "Partial credit".into(),
            score: Some(0.65),
            evidence_ids: Vec::new(),
        }];
        let results = semantic_test_assessments(&[], &criteria);
        assert_eq!(results[0].kind, AssessmentKind::Signal);
        assert_eq!(results[0].policy, AssessmentPolicy::Advisory);
        assert_eq!(results[0].score.as_ref().unwrap().awarded, 65);
        assert_eq!(results[0].outcome, AssessmentOutcome::Failed);
    }

    #[test]
    fn malformed_scores_are_rejected() {
        assert!(AssessmentScore {
            awarded: 11,
            possible: 10
        }
        .validate()
        .is_err());
        assert!(AssessmentScore {
            awarded: 0,
            possible: 0
        }
        .validate()
        .is_err());
    }

    #[test]
    fn hard_gates_must_be_required_checks() {
        let mut result = deterministic_result();
        result.validate().unwrap();
        result.kind = AssessmentKind::Signal;
        assert!(result.validate().is_err());
    }

    #[test]
    fn unavailable_assessments_cannot_carry_a_score() {
        let mut result = deterministic_result();
        result.outcome = AssessmentOutcome::NotEvaluated;
        assert!(result.validate().is_err());
        result.score = None;
        result.validate().unwrap();
    }

    #[test]
    fn shared_result_fixture_decodes_and_validates() {
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/results/results-assessment-contract.json"
        ))
        .unwrap();
        let contract: AssessmentContract =
            serde_json::from_value(value["assessment_contract"].clone()).unwrap();

        assert_eq!(contract.runs.len(), 1);
        contract.runs[0].validate().unwrap();
        assert_eq!(contract.runs[0].system_status, SystemStatus::HardGateFailed);
        assert_eq!(contract.runs[0].assets.len(), 1);
    }
}
