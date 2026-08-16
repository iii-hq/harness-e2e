use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::artifact::{self, ArtifactReference};
use crate::report::{DimensionReport, E2eReport, FailureRecord, RunStatus};

pub const ASSESSMENT_CONTRACT_VERSION: u32 = 1;
pub const ANALYSIS_BUNDLE_SCHEMA_VERSION: u32 = 1;
pub const ANALYSIS_RESPONSE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentKind {
    RequiredCheck,
    Signal,
    AssetValidation,
    AssetQuality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentPolicy {
    HardGate,
    Advisory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentSource {
    Deterministic,
    Judge,
    AssetAnalyzer,
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
    Asset,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnalyzerIdentity {
    pub analyzer: String,
    pub analyzer_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub prompt_version: String,
    pub input_sha256: String,
}

impl AnalyzerIdentity {
    pub fn validate(&self) -> Result<()> {
        required(&self.analyzer, "analyzer id")?;
        required(&self.analyzer_version, "analyzer version")?;
        required(&self.prompt_version, "analyzer prompt version")?;
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

impl AnalyzerUsage {
    fn validate(&self) -> Result<()> {
        if self
            .cost_usd
            .is_some_and(|cost| !cost.is_finite() || cost < 0.0)
        {
            bail!("analyzer cost must be finite and non-negative");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AssessmentResult {
    pub criterion_id: String,
    pub target: AssessmentTarget,
    pub kind: AssessmentKind,
    pub policy: AssessmentPolicy,
    pub dimension: crate::report::EvaluationDimension,
    pub source: AssessmentSource,
    pub outcome: AssessmentOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<AssessmentScore>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analyzer: Option<AnalyzerIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analyzer_usage: Option<AnalyzerUsage>,
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
        validate_confidence(self.confidence, "assessment confidence")?;
        for evidence in &self.evidence {
            evidence.validate()?;
        }
        if let Some(analyzer) = &self.analyzer {
            analyzer.validate()?;
        }
        if let Some(usage) = &self.analyzer_usage {
            usage.validate()?;
        }
        if self.analyzer_usage.is_some() && self.analyzer.is_none() {
            bail!(
                "assessment '{}' analyzer usage requires analyzer identity",
                self.criterion_id
            );
        }
        if self.source == AssessmentSource::Deterministic
            && (self.analyzer.is_some() || self.analyzer_usage.is_some())
        {
            bail!(
                "deterministic assessment '{}' cannot contain AI analyzer metadata",
                self.criterion_id
            );
        }
        let execution_was_attempted = !matches!(
            self.outcome,
            AssessmentOutcome::NotEvaluated | AssessmentOutcome::Unavailable
        );
        if self.source != AssessmentSource::Deterministic
            && execution_was_attempted
            && self.analyzer.is_none()
        {
            bail!(
                "AI assessment '{}' requires analyzer identity",
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
        for evidence in &self.evidence {
            evidence.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AssetAssessmentResult {
    pub validation: AssetValidationResult,
    pub qualitative_assessment: AssessmentResult,
}

impl AssetAssessmentResult {
    fn validate(&self) -> Result<()> {
        self.validation.validate()?;
        self.qualitative_assessment.validate()?;
        if self.qualitative_assessment.kind != AssessmentKind::AssetQuality
            || self.qualitative_assessment.target.kind != AssessmentTargetKind::Asset
            || self.qualitative_assessment.target.id != self.validation.asset_id
        {
            bail!(
                "asset '{}' qualitative assessment targets a different asset",
                self.validation.asset_id
            );
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveStatus {
    Unavailable,
    Passed,
    PassedWithConcerns,
    HardGateFailed,
    SubjectError,
    JudgeError,
    ResourceLimit,
    InfrastructureError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AiAssessmentAvailability {
    NotRequested,
    NotEvaluated,
    Available,
    Unavailable,
    Malformed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AiVerdict {
    Pass,
    PassWithConcerns,
    Fail,
    Inconclusive,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FinalAssessmentResult {
    pub verdict: AiVerdict,
    pub quality_score: u8,
    pub confidence: f64,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strengths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub concerns: Vec<String>,
    pub recommendation: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceReference>,
}

impl FinalAssessmentResult {
    fn validate(&self) -> Result<()> {
        if self.quality_score > 100 {
            bail!("final AI quality score must be in 0..=100");
        }
        validate_confidence(Some(self.confidence), "final AI confidence")?;
        required(&self.summary, "final AI summary")?;
        required(&self.recommendation, "final AI recommendation")?;
        for evidence in &self.evidence {
            evidence.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AiFinalAssessment {
    pub availability: AiAssessmentAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<FinalAssessmentResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analyzer: Option<AnalyzerIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analyzer_usage: Option<AnalyzerUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl AiFinalAssessment {
    pub fn not_evaluated(reason: impl Into<String>) -> Self {
        Self {
            availability: AiAssessmentAvailability::NotEvaluated,
            result: None,
            analyzer: None,
            analyzer_usage: None,
            reason: Some(reason.into()),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.analyzer_usage.is_some() && self.analyzer.is_none() {
            bail!("final AI analyzer usage requires analyzer identity");
        }
        if let Some(usage) = &self.analyzer_usage {
            usage.validate()?;
        }
        match self.availability {
            AiAssessmentAvailability::Available => {
                self.result
                    .as_ref()
                    .context("available final AI assessment has no result")?
                    .validate()?;
                self.analyzer
                    .as_ref()
                    .context("available final AI assessment has no analyzer identity")?
                    .validate()?;
            }
            AiAssessmentAvailability::Unavailable
            | AiAssessmentAvailability::Malformed
            | AiAssessmentAvailability::Failed => {
                if self.result.is_some() {
                    bail!("unavailable final AI assessment cannot contain a result");
                }
                required(
                    self.reason.as_deref().unwrap_or_default(),
                    "final AI unavailability reason",
                )?;
                if let Some(analyzer) = &self.analyzer {
                    analyzer.validate()?;
                }
            }
            AiAssessmentAvailability::NotRequested | AiAssessmentAvailability::NotEvaluated => {
                if self.result.is_some() {
                    bail!("non-evaluated final AI assessment cannot contain a result");
                }
                if let Some(analyzer) = &self.analyzer {
                    analyzer.validate()?;
                }
            }
        }
        Ok(())
    }
}

pub fn derive_effective_status(
    system_status: SystemStatus,
    ai_final_assessment: &AiFinalAssessment,
) -> EffectiveStatus {
    match system_status {
        SystemStatus::Unavailable => EffectiveStatus::Unavailable,
        SystemStatus::HardGateFailed => EffectiveStatus::HardGateFailed,
        SystemStatus::SubjectError => EffectiveStatus::SubjectError,
        SystemStatus::JudgeError => EffectiveStatus::JudgeError,
        SystemStatus::ResourceLimit => EffectiveStatus::ResourceLimit,
        SystemStatus::InfrastructureError => EffectiveStatus::InfrastructureError,
        SystemStatus::Passed => match ai_final_assessment
            .result
            .as_ref()
            .map(|value| value.verdict)
        {
            Some(AiVerdict::PassWithConcerns | AiVerdict::Fail | AiVerdict::Inconclusive) => {
                EffectiveStatus::PassedWithConcerns
            }
            Some(AiVerdict::Pass) | None => EffectiveStatus::Passed,
        },
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
    pub assets: Vec<AssetAssessmentResult>,
    pub ai_final_assessment: AiFinalAssessment,
    pub effective_status: EffectiveStatus,
}

impl RunAssessmentContract {
    fn legacy(run_id: String, attempt_id: String) -> Self {
        let ai_final_assessment = AiFinalAssessment::not_evaluated(
            "legacy result does not contain the assessment contract",
        );
        Self {
            run_id,
            attempt_id,
            system_status: SystemStatus::Unavailable,
            assessments: Vec::new(),
            assets: Vec::new(),
            effective_status: EffectiveStatus::Unavailable,
            ai_final_assessment,
        }
    }

    fn validate(&self) -> Result<()> {
        required(&self.run_id, "assessment run id")?;
        required(&self.attempt_id, "assessment attempt id")?;
        self.ai_final_assessment.validate()?;
        let derived = derive_effective_status(self.system_status, &self.ai_final_assessment);
        if self.effective_status != derived {
            bail!(
                "run '{}:{}' effective status {:?} differs from derived {:?}",
                self.run_id,
                self.attempt_id,
                self.effective_status,
                derived
            );
        }
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
            if !asset_ids.insert(asset.validation.asset_id.as_str()) {
                bail!(
                    "run '{}:{}' repeats asset '{}'",
                    self.run_id,
                    self.attempt_id,
                    asset.validation.asset_id
                );
            }
        }
        Ok(())
    }

    fn evidence_references(&self) -> Vec<&EvidenceReference> {
        let mut references = self
            .assessments
            .iter()
            .flat_map(|assessment| &assessment.evidence)
            .collect::<Vec<_>>();
        for asset in &self.assets {
            references.extend(&asset.validation.evidence);
            references.extend(&asset.qualitative_assessment.evidence);
        }
        if let Some(result) = &self.ai_final_assessment.result {
            references.extend(&result.evidence);
        }
        references
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AssessmentContract {
    pub contract_version: u32,
    pub runs: Vec<RunAssessmentContract>,
}

impl AssessmentContract {
    pub fn from_asset_evidence(report: &E2eReport) -> Self {
        Self {
            contract_version: ASSESSMENT_CONTRACT_VERSION,
            runs: report
                .scenarios
                .iter()
                .flat_map(|scenario| &scenario.runs)
                .map(|run| {
                    let system_status = SystemStatus::from(run.status);
                    let ai_final_assessment = AiFinalAssessment::not_evaluated(
                        "final AI assessment is owned by MOT-4447",
                    );
                    RunAssessmentContract {
                        run_id: run.run_id.clone(),
                        attempt_id: run.attempt_id.clone(),
                        system_status,
                        assessments: Vec::new(),
                        assets: run.asset_assessments.clone(),
                        effective_status: derive_effective_status(
                            system_status,
                            &ai_final_assessment,
                        ),
                        ai_final_assessment,
                    }
                })
                .collect(),
        }
    }

    pub fn normalize(report: &E2eReport) -> Self {
        report.assessment_contract.clone().unwrap_or_else(|| Self {
            contract_version: ASSESSMENT_CONTRACT_VERSION,
            runs: report
                .scenarios
                .iter()
                .enumerate()
                .flat_map(|(scenario_index, scenario)| {
                    scenario
                        .runs
                        .iter()
                        .enumerate()
                        .map(move |(run_index, run)| {
                            let run_id = if run.run_id.trim().is_empty() {
                                format!("legacy-run-{scenario_index}-{run_index}")
                            } else {
                                run.run_id.clone()
                            };
                            let attempt_id = if run.attempt_id.trim().is_empty() {
                                format!("legacy-attempt-{}", run.attempt_number)
                            } else {
                                run.attempt_id.clone()
                            };
                            RunAssessmentContract::legacy(run_id, attempt_id)
                        })
                })
                .collect(),
        })
    }

    pub fn validate(&self, report: &E2eReport) -> Result<()> {
        if self.contract_version != ASSESSMENT_CONTRACT_VERSION {
            bail!(
                "assessment contract version {} is unsupported; expected {}",
                self.contract_version,
                ASSESSMENT_CONTRACT_VERSION
            );
        }
        let expected = report
            .scenarios
            .iter()
            .flat_map(|scenario| &scenario.runs)
            .map(|run| ((run.run_id.as_str(), run.attempt_id.as_str()), run))
            .collect::<BTreeMap<_, _>>();
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
            let expected_run = expected.get(&identity).with_context(|| {
                format!(
                    "assessment contract contains unknown run '{}:{}'",
                    run.run_id, run.attempt_id
                )
            })?;
            let expected_status = SystemStatus::from(expected_run.status);
            if run.system_status != expected_status {
                bail!(
                    "assessment run '{}:{}' system status {:?} differs from E2E run status {:?}",
                    run.run_id,
                    run.attempt_id,
                    run.system_status,
                    expected_status
                );
            }
            for reference in run.evidence_references() {
                let matches_artifact = expected_run
                    .evidence
                    .iter()
                    .chain(
                        expected_run
                            .deliverables
                            .iter()
                            .filter_map(|deliverable| deliverable.artifact.as_ref()),
                    )
                    .any(|artifact| {
                        artifact.id == reference.artifact_id
                            && artifact.sha256 == reference.artifact_sha256
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisScope {
    Execution,
    Test,
    Comparison,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnalysisSubject {
    pub execution_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub scenario_id: String,
    pub scenario_version: u32,
    pub case_id: String,
    pub system_status: SystemStatus,
    pub effective_status: EffectiveStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AnalysisMetric {
    pub id: String,
    pub value: f64,
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnalysisExcerpt {
    pub kind: String,
    pub summary: String,
    pub evidence: EvidenceReference,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AnalysisBundle {
    pub schema_version: u32,
    pub scope: AnalysisScope,
    pub input_sha256: String,
    pub subjects: Vec<AnalysisSubject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assessments: Vec<AssessmentResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<AssetAssessmentResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dimensions: Vec<DimensionReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<FailureRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<ArtifactReference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metrics: Vec<AnalysisMetric>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excerpts: Vec<AnalysisExcerpt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
}

impl AnalysisBundle {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != ANALYSIS_BUNDLE_SCHEMA_VERSION {
            bail!("unsupported AnalysisBundle schema version");
        }
        validate_sha256(&self.input_sha256, "analysis bundle input hash")?;
        if self.subjects.is_empty() {
            bail!("analysis bundle requires at least one subject");
        }
        if self.scope == AnalysisScope::Comparison && self.subjects.len() != 2 {
            bail!("comparison analysis requires exactly two subjects");
        }
        let mut subjects = BTreeSet::new();
        for subject in &self.subjects {
            required(&subject.execution_id, "analysis subject execution id")?;
            required(&subject.run_id, "analysis subject run id")?;
            required(&subject.attempt_id, "analysis subject attempt id")?;
            required(&subject.scenario_id, "analysis subject scenario id")?;
            required(&subject.case_id, "analysis subject case id")?;
            if subject.scenario_version == 0 {
                bail!("analysis subject scenario version must be positive");
            }
            if !subjects.insert((
                subject.execution_id.as_str(),
                subject.run_id.as_str(),
                subject.attempt_id.as_str(),
            )) {
                bail!("analysis bundle repeats a subject identity");
            }
        }
        for assessment in &self.assessments {
            assessment.validate()?;
        }
        for asset in &self.assets {
            asset.validate()?;
        }
        for excerpt in &self.excerpts {
            required(&excerpt.kind, "analysis excerpt kind")?;
            required(&excerpt.summary, "analysis excerpt summary")?;
            excerpt.evidence.validate()?;
        }
        for metric in &self.metrics {
            required(&metric.id, "analysis metric id")?;
            required(&metric.unit, "analysis metric unit")?;
            if !metric.value.is_finite() {
                bail!("analysis metric value must be finite");
            }
        }
        for limitation in &self.limitations {
            required(limitation, "analysis bundle limitation")?;
        }
        Ok(())
    }

    pub fn sha256(&self) -> Result<String> {
        self.validate()?;
        artifact::sha256_value(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnalysisFact {
    pub summary: String,
    pub evidence: Vec<EvidenceReference>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AnalysisInterpretation {
    pub summary: String,
    pub confidence: f64,
    pub evidence: Vec<EvidenceReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnalysisOpportunity {
    pub priority: u8,
    pub summary: String,
    pub expected_impact: String,
    pub validation_method: String,
    pub evidence: Vec<EvidenceReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnalysisLimitation {
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceReference>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AnalysisResponse {
    pub schema_version: u32,
    pub input_sha256: String,
    pub analyzer: AnalyzerIdentity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<AnalysisFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interpretations: Vec<AnalysisInterpretation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opportunities: Vec<AnalysisOpportunity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<AnalysisLimitation>,
}

impl AnalysisResponse {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != ANALYSIS_RESPONSE_SCHEMA_VERSION {
            bail!("unsupported analysis response schema version");
        }
        validate_sha256(&self.input_sha256, "analysis response input hash")?;
        self.analyzer.validate()?;
        if self.input_sha256 != self.analyzer.input_sha256 {
            bail!("analysis response input hash differs from analyzer identity");
        }
        for fact in &self.facts {
            required(&fact.summary, "analysis fact")?;
            if fact.evidence.is_empty() {
                bail!("analysis facts require evidence");
            }
            for evidence in &fact.evidence {
                evidence.validate()?;
            }
        }
        for interpretation in &self.interpretations {
            required(&interpretation.summary, "analysis interpretation")?;
            validate_confidence(Some(interpretation.confidence), "interpretation confidence")?;
            if interpretation.evidence.is_empty() {
                bail!("analysis interpretations require evidence");
            }
            for evidence in &interpretation.evidence {
                evidence.validate()?;
            }
        }
        for opportunity in &self.opportunities {
            if !(1..=5).contains(&opportunity.priority) {
                bail!("analysis opportunity priority must be in 1..=5");
            }
            required(&opportunity.summary, "analysis opportunity")?;
            required(&opportunity.expected_impact, "analysis opportunity impact")?;
            required(
                &opportunity.validation_method,
                "analysis opportunity validation",
            )?;
            if opportunity.evidence.is_empty() {
                bail!("analysis opportunities require evidence");
            }
            for evidence in &opportunity.evidence {
                evidence.validate()?;
            }
        }
        for limitation in &self.limitations {
            required(&limitation.summary, "analysis limitation")?;
            for evidence in &limitation.evidence {
                evidence.validate()?;
            }
        }
        Ok(())
    }

    pub fn validate_for(&self, bundle: &AnalysisBundle) -> Result<()> {
        self.validate()?;
        let bundle_sha256 = bundle.sha256()?;
        if self.input_sha256 != bundle_sha256 {
            bail!("analysis response input hash differs from its AnalysisBundle");
        }
        Ok(())
    }
}

fn validate_confidence(value: Option<f64>, label: &str) -> Result<()> {
    if let Some(value) = value {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            bail!("{label} must be in 0..=1");
        }
    }
    Ok(())
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

    fn unavailable_ai() -> AiFinalAssessment {
        AiFinalAssessment {
            availability: AiAssessmentAvailability::Unavailable,
            result: None,
            analyzer: None,
            analyzer_usage: None,
            reason: Some("provider unavailable".into()),
        }
    }

    #[test]
    fn objective_failures_cannot_be_promoted_by_ai() {
        let ai = AiFinalAssessment {
            availability: AiAssessmentAvailability::Available,
            result: Some(FinalAssessmentResult {
                verdict: AiVerdict::Pass,
                quality_score: 100,
                confidence: 1.0,
                summary: "Excellent qualitative output.".into(),
                strengths: vec!["clear".into()],
                concerns: vec![],
                recommendation: "ship".into(),
                limitations: vec![],
                evidence: vec![],
            }),
            analyzer: Some(AnalyzerIdentity {
                analyzer: "final".into(),
                analyzer_version: "1".into(),
                provider: Some("provider".into()),
                model: Some("model".into()),
                prompt_version: "1".into(),
                input_sha256: format!("sha256:{}", "a".repeat(64)),
            }),
            analyzer_usage: None,
            reason: None,
        };

        for (system, expected) in [
            (
                SystemStatus::HardGateFailed,
                EffectiveStatus::HardGateFailed,
            ),
            (SystemStatus::SubjectError, EffectiveStatus::SubjectError),
            (SystemStatus::JudgeError, EffectiveStatus::JudgeError),
            (SystemStatus::ResourceLimit, EffectiveStatus::ResourceLimit),
            (
                SystemStatus::InfrastructureError,
                EffectiveStatus::InfrastructureError,
            ),
        ] {
            assert_eq!(derive_effective_status(system, &ai), expected);
        }
    }

    #[test]
    fn unavailable_ai_does_not_corrupt_a_passing_system_status() {
        let ai = unavailable_ai();
        ai.validate().unwrap();
        assert_eq!(
            derive_effective_status(SystemStatus::Passed, &ai),
            EffectiveStatus::Passed
        );
    }

    #[test]
    fn non_executed_asset_assessment_does_not_invent_analyzer_identity() {
        let assessment = AssessmentResult {
            criterion_id: "asset_quality".into(),
            target: AssessmentTarget {
                kind: AssessmentTargetKind::Asset,
                id: "result".into(),
            },
            kind: AssessmentKind::AssetQuality,
            policy: AssessmentPolicy::Advisory,
            dimension: crate::report::EvaluationDimension::Deliverable,
            source: AssessmentSource::AssetAnalyzer,
            outcome: AssessmentOutcome::NotEvaluated,
            score: None,
            confidence: None,
            summary: "Asset quality has not been evaluated.".into(),
            evidence: Vec::new(),
            analyzer: None,
            analyzer_usage: None,
        };

        assessment.validate().unwrap();
    }

    #[test]
    fn malformed_scores_and_confidence_are_rejected() {
        assert!(AssessmentScore {
            awarded: 11,
            possible: 10
        }
        .validate()
        .is_err());
        assert!(validate_confidence(Some(1.01), "confidence").is_err());
        assert!(validate_confidence(Some(f64::NAN), "confidence").is_err());
    }

    #[test]
    fn analysis_response_is_bound_to_the_canonical_bundle() {
        let mut bundle = AnalysisBundle {
            schema_version: ANALYSIS_BUNDLE_SCHEMA_VERSION,
            scope: AnalysisScope::Execution,
            input_sha256: format!("sha256:{}", "a".repeat(64)),
            subjects: vec![AnalysisSubject {
                execution_id: "execution-1".into(),
                run_id: "run-1".into(),
                attempt_id: "attempt-1".into(),
                scenario_id: "direct_answer".into(),
                scenario_version: 1,
                case_id: "case-1".into(),
                system_status: SystemStatus::Passed,
                effective_status: EffectiveStatus::Passed,
            }],
            assessments: Vec::new(),
            assets: Vec::new(),
            dimensions: Vec::new(),
            failures: Vec::new(),
            evidence: Vec::new(),
            metrics: Vec::new(),
            excerpts: Vec::new(),
            limitations: Vec::new(),
        };
        let bundle_sha256 = bundle.sha256().unwrap();
        let response = AnalysisResponse {
            schema_version: ANALYSIS_RESPONSE_SCHEMA_VERSION,
            input_sha256: bundle_sha256.clone(),
            analyzer: AnalyzerIdentity {
                analyzer: "manual-analysis".into(),
                analyzer_version: "1".into(),
                provider: Some("provider".into()),
                model: Some("model".into()),
                prompt_version: "analysis-v1".into(),
                input_sha256: bundle_sha256,
            },
            facts: Vec::new(),
            interpretations: Vec::new(),
            opportunities: Vec::new(),
            limitations: Vec::new(),
        };

        response.validate_for(&bundle).unwrap();
        bundle
            .limitations
            .push("No trace spans were available.".into());
        assert!(response.validate_for(&bundle).is_err());
    }

    #[test]
    fn shared_v3_fixture_decodes_and_validates() {
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/results/results-v3-assessment-contract.json"
        ))
        .unwrap();
        let contract: AssessmentContract =
            serde_json::from_value(value["assessment_contract"].clone()).unwrap();

        assert_eq!(contract.contract_version, ASSESSMENT_CONTRACT_VERSION);
        assert_eq!(contract.runs.len(), 1);
        contract.runs[0].validate().unwrap();
        assert_eq!(
            contract.runs[0].effective_status,
            EffectiveStatus::HardGateFailed
        );
    }
}
