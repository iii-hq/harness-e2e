use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

use crate::artifact::{self, ArtifactReference};
use crate::assessment::{
    AssessmentContract, AssessmentResult, AssessmentTargetKind, AssetAssessmentResult,
    EvidenceReference,
};
use crate::identity::{ExecutionIdentity, StackIdentity, SystemUnderTestIdentity};
use crate::scenarios::{
    CapturedDeliverable, CapturedDeliverableContent, CapturedInvariant, ExecutionPolicy,
    ProvenanceEvidence, ScenarioCase, WorkExpectation,
};
#[cfg(test)]
use crate::scenarios::{ComplexityProfile, DeliverableContract};
use crate::schema;
use crate::wire::{ControlPlaneEvidence, Model, SessionMetricsResponse, StatusReport};
use crate::workflow::{WorkflowCleanupReport, WorkflowStepReport};

mod summary;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailurePhase {
    Setup,
    Execute,
    Collect,
    Evaluate,
    Cleanup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureDomain {
    Subject,
    Judge,
    Resource,
    E2eInfrastructure,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RetryScope {
    #[default]
    None,
    SameSlot,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContaminationScope {
    #[default]
    None,
    Case,
    Execution,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FailureRecord {
    /// The failure's original classification, independent of the primary run status.
    pub status: RunStatus,
    pub domain: FailureDomain,
    pub phase: FailurePhase,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub retry_scope: RetryScope,
    #[serde(default)]
    pub contamination: ContaminationScope,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HardGateReport {
    pub id: String,
    pub dimension: EvaluationDimension,
    pub passed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationDimension {
    Deliverable,
    StructuralIntegrity,
    Efficiency,
    Robustness,
    E2eInfrastructure,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DimensionReport {
    pub dimension: EvaluationDimension,
    pub passed: Option<bool>,
    pub signals: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeliverableReport {
    pub id: String,
    pub kind: String,
    pub media_type: String,
    pub content_format: DeliverableContentFormat,
    pub content_sha256: String,
    pub content_size_bytes: u64,
    pub schema_valid: bool,
    pub provenance_valid: bool,
    pub invariants: Vec<CapturedInvariant>,
    pub provenance: Vec<ProvenanceEvidence>,
    pub preview: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactReference>,
    #[serde(skip)]
    #[schemars(skip)]
    pub content: CapturedDeliverableContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliverableContentFormat {
    Json,
    TextUtf8,
}

impl DeliverableReport {
    pub fn passed(&self) -> bool {
        self.schema_valid
            && self.provenance_valid
            && self.invariants.iter().all(|invariant| invariant.passed)
    }
}

pub fn evaluate_deliverables(
    case: &ScenarioCase,
    captured: Vec<CapturedDeliverable>,
) -> Result<Vec<DeliverableReport>> {
    crate::asset::evaluate_assets(case, captured, crate::asset::AssetCaptureLimits::default())
        .map(|evaluation| evaluation.deliverables)
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CriterionReport {
    pub id: String,
    pub possible: u8,
    pub awarded: Option<u8>,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MarkdownPhaseStatus {
    Pending,
    Completed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MarkdownPhaseReport {
    pub phase: String,
    pub status: MarkdownPhaseStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub input_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MarkdownExecutionReport {
    pub source_path: String,
    pub source_sha256: String,
    pub behavior_sha256: String,
    pub compiled_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialized_plan_sha256: Option<String>,
    pub prompt_sha256: String,
    pub pipeline_complete: bool,
    pub phases: Vec<MarkdownPhaseReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdherenceAvailability {
    Available,
    Unavailable,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AdherenceRequirement {
    pub id: String,
    pub instruction: String,
    pub followed: bool,
    pub reason: String,
    pub confidence: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InstructionAdherenceReport {
    pub availability: AdherenceAvailability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<u8>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirements: Vec<AdherenceRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analyzer: Option<crate::assessment::AnalyzerIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analyzer_usage: Option<crate::assessment::AnalyzerUsage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ModelUsageReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CostReport {
    pub subject_usd: Option<f64>,
    pub judge_usd: Option<f64>,
    pub total_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioMeasurement {
    pub id: String,
    pub value: f64,
    pub unit: String,
    pub origin: ObservationMetricOrigin,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ObservedComplexityReport {
    pub planning_depth: Option<u64>,
    pub dependency_depth: Option<u64>,
    pub parallel_branches: Option<u64>,
    pub external_systems: Option<u64>,
    pub state_transitions: Option<u64>,
    pub wake_cycles: Option<u64>,
    pub validation_loops: Option<u64>,
    pub artifact_count: Option<u64>,
    pub coordination_edges: Option<u64>,
    pub ambiguity_level: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EfficiencyReport {
    pub wall_time_ms: u64,
    pub root_turns: Option<u64>,
    pub child_turns: Option<u64>,
    pub child_sessions: Option<u64>,
    pub function_calls: Option<u64>,
    pub function_call_errors: Option<u64>,
    pub validation_retries: Option<u64>,
    pub transient_resumes: Option<u64>,
    pub wake_resumes: Option<u64>,
    pub effective_fan_out: Option<u64>,
    pub critical_path_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub minimum_expected_work: u64,
    pub observed_work: Option<u64>,
    pub work_amplification: Option<f64>,
    pub technical_attempts: u32,
    pub observed_complexity: ObservedComplexityReport,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<String, String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Passed,
    HardGateFailed,
    SubjectError,
    JudgeError,
    ResourceLimit,
    InfrastructureError,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompletionState {
    Completed,
    TaskIncomplete,
    #[default]
    Undetermined,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TechnicalState {
    Valid,
    #[default]
    TechnicalInvalid,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvaluatorAvailability {
    NotRequired,
    #[default]
    Pending,
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct EvaluatorStates {
    pub completion: EvaluatorAvailability,
    pub quality: EvaluatorAvailability,
    pub final_advisory: EvaluatorAvailability,
}

impl RunStatus {
    pub fn is_technical_failure(self) -> bool {
        matches!(
            self,
            Self::SubjectError | Self::JudgeError | Self::ResourceLimit | Self::InfrastructureError
        )
    }

    fn label(self) -> &'static str {
        match self {
            Self::Passed => "PASS",
            Self::HardGateFailed => "HARD GATE FAIL",
            Self::SubjectError => "SUBJECT ERROR",
            Self::JudgeError => "JUDGE ERROR",
            Self::ResourceLimit => "RESOURCE LIMIT",
            Self::InfrastructureError => "INFRA ERROR",
        }
    }

    fn failure_domain(self) -> FailureDomain {
        match self {
            Self::SubjectError => FailureDomain::Subject,
            Self::JudgeError => FailureDomain::Judge,
            Self::ResourceLimit => FailureDomain::Resource,
            Self::Passed | Self::HardGateFailed | Self::InfrastructureError => {
                FailureDomain::E2eInfrastructure
            }
        }
    }
}

pub(crate) fn classify_failure(status: RunStatus, phase: FailurePhase) -> String {
    match status {
        RunStatus::JudgeError => "judge_unavailable",
        RunStatus::ResourceLimit => "subject_budget_exhausted",
        RunStatus::SubjectError => "subject_execution_failed",
        RunStatus::InfrastructureError if phase == FailurePhase::Cleanup => "cleanup_failed",
        RunStatus::InfrastructureError => "infrastructure_failed",
        RunStatus::HardGateFailed => "objective_gate_failed",
        RunStatus::Passed => "unexpected_failure_on_passed_run",
    }
    .into()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RetryAttemptReport {
    pub run_id: String,
    pub attempt_id: String,
    pub attempt_number: u32,
    pub session_id: String,
    pub wall_time_ms: u64,
    pub status: RunStatus,
    pub completion: CompletionState,
    pub technical: TechnicalState,
    pub evaluators: EvaluatorStates,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective_score: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_score_completed: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_usage: Option<ModelUsageReport>,
    pub cost: CostReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<SessionMetricsResponse>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<ArtifactReference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deliverables: Vec<DeliverableReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub semantic_tests: Vec<WorkflowStepReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario_flow: Option<ScenarioFlowEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dimensions: Vec<DimensionReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub efficiency: Option<EfficiencyReport>,
    pub failures: Vec<FailureRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_score: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown_execution: Option<MarkdownExecutionReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction_adherence: Option<InstructionAdherenceReport>,
    #[serde(skip)]
    #[schemars(skip)]
    pub assessment_results: Vec<AssessmentResult>,
    #[serde(skip)]
    #[schemars(skip)]
    pub asset_assessments: Vec<AssetAssessmentResult>,
    #[serde(skip)]
    #[schemars(skip)]
    pub asset_capture_manifest: Option<ArtifactReference>,
    #[serde(skip)]
    #[schemars(skip)]
    pub asset_redaction: crate::redaction::RedactionReport,
}

impl From<&E2eRunReport> for RetryAttemptReport {
    fn from(report: &E2eRunReport) -> Self {
        Self {
            run_id: report.run_id.clone(),
            attempt_id: report.attempt_id.clone(),
            attempt_number: report.attempt_number,
            session_id: report.session_id.clone(),
            wall_time_ms: report.wall_time_ms,
            status: report.status,
            completion: report.completion,
            technical: report.technical,
            evaluators: report.evaluators.clone(),
            objective_score: report.objective_score,
            quality_score_completed: report.quality_score_completed,
            judge_usage: report.judge_usage.clone(),
            cost: report.cost.clone(),
            transcript: report.transcript.clone(),
            metrics: report.metrics.clone(),
            evidence: report.evidence.clone(),
            deliverables: report.deliverables.clone(),
            semantic_tests: report.semantic_tests.clone(),
            scenario_flow: report.scenario_flow.clone(),
            dimensions: report.dimensions.clone(),
            efficiency: report.efficiency.clone(),
            failures: report.failures.clone(),
            validation_score: report.validation_score,
            markdown_execution: report.markdown_execution.clone(),
            instruction_adherence: report.instruction_adherence.clone(),
            assessment_results: report.assessment_results.clone(),
            asset_assessments: report.asset_assessments.clone(),
            asset_capture_manifest: report.asset_capture_manifest.clone(),
            asset_redaction: report.asset_redaction.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct E2eRunReport {
    pub run_id: String,
    pub attempt_id: String,
    pub attempt_number: u32,
    pub session_id: String,
    pub prompt: String,
    pub wall_time_ms: u64,
    pub score: Option<u8>,
    /// Explicit Markdown validation score. Mirrors `score` for Markdown-authored
    /// scenarios while keeping the legacy aggregate field compatible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_score: Option<u8>,
    pub status: RunStatus,
    pub completion: CompletionState,
    pub technical: TechnicalState,
    pub evaluators: EvaluatorStates,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective_score: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_score_completed: Option<u8>,
    pub hard_gates: Vec<HardGateReport>,
    pub criteria: Vec<CriterionReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<SessionMetricsResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_attempts: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_usage: Option<ModelUsageReport>,
    pub cost: CostReport,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<ArtifactReference>,
    /// Exact request/response schemas observed after this scenario's setup.
    /// Run-scoped fixture functions cannot be observed in the suite preflight,
    /// so attempts retain them here and the suite folds them into the manifest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worker_contracts: Vec<ObservedWorkerContract>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenario_measurements: Vec<ScenarioMeasurement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deliverables: Vec<DeliverableReport>,
    /// Semantic tests executed inside a code-defined composite scenario. Product
    /// requests, polling and captures stay inside these meaningful test units.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub semantic_tests: Vec<WorkflowStepReport>,
    /// Evidence-only flow identity emitted by Rust. This value is never accepted
    /// as runner input and cannot reconstruct executable configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario_flow: Option<ScenarioFlowEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dimensions: Vec<DimensionReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub efficiency: Option<EfficiencyReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retry_attempts: Vec<RetryAttemptReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<FailureRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown_execution: Option<MarkdownExecutionReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction_adherence: Option<InstructionAdherenceReport>,
    /// Advisory behavioral audit over the captured transcript and metrics.
    /// Never contributes to score, status, gates, or longitudinal inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit: Option<crate::audit::AuditReport>,
    #[serde(skip)]
    #[schemars(skip)]
    pub terminal_status: Option<StatusReport>,
    #[serde(skip)]
    #[schemars(skip)]
    pub assessment_results: Vec<AssessmentResult>,
    #[serde(skip)]
    #[schemars(skip)]
    pub asset_assessments: Vec<AssetAssessmentResult>,
    #[serde(skip)]
    #[schemars(skip)]
    pub asset_capture_manifest: Option<ArtifactReference>,
    #[serde(skip)]
    #[schemars(skip)]
    pub final_assessment_input: Option<ArtifactReference>,
    #[serde(skip)]
    #[schemars(skip)]
    pub asset_redaction: crate::redaction::RedactionReport,
}

impl E2eRunReport {
    pub fn new(
        run_id: String,
        attempt_id: String,
        attempt_number: u32,
        session_id: String,
        prompt: String,
    ) -> Self {
        Self {
            run_id,
            attempt_id,
            attempt_number,
            session_id,
            prompt,
            wall_time_ms: 0,
            score: None,
            validation_score: None,
            status: RunStatus::InfrastructureError,
            completion: CompletionState::Undetermined,
            technical: TechnicalState::TechnicalInvalid,
            evaluators: EvaluatorStates::default(),
            objective_score: None,
            quality_score_completed: None,
            hard_gates: Vec::new(),
            criteria: Vec::new(),
            transcript: None,
            metrics: None,
            judge_attempts: None,
            judge_usage: None,
            cost: CostReport::default(),
            evidence: Vec::new(),
            worker_contracts: Vec::new(),
            scenario_measurements: Vec::new(),
            deliverables: Vec::new(),
            semantic_tests: Vec::new(),
            scenario_flow: None,
            dimensions: Vec::new(),
            efficiency: None,
            retry_attempts: Vec::new(),
            failures: Vec::new(),
            markdown_execution: None,
            instruction_adherence: None,
            audit: None,
            terminal_status: None,
            assessment_results: Vec::new(),
            asset_assessments: Vec::new(),
            asset_capture_manifest: None,
            final_assessment_input: None,
            asset_redaction: crate::redaction::RedactionReport::default(),
        }
    }

    pub fn push_failure(
        &mut self,
        status: RunStatus,
        phase: FailurePhase,
        message: impl Into<String>,
    ) {
        self.push_typed_failure(
            status,
            phase,
            classify_failure(status, phase),
            RetryScope::None,
            ContaminationScope::None,
            message,
        );
    }

    pub fn push_typed_failure(
        &mut self,
        status: RunStatus,
        phase: FailurePhase,
        code: impl Into<String>,
        retry_scope: RetryScope,
        contamination: ContaminationScope,
        message: impl Into<String>,
    ) {
        let message = message.into();
        let is_primary = self.failures.is_empty();
        self.failures.push(FailureRecord {
            status,
            domain: status.failure_domain(),
            phase,
            code: code.into(),
            retry_scope,
            contamination,
            message,
        });
        if is_primary {
            self.status = status;
        }
        self.seal_outcome_from_status();
    }

    pub fn finish(&mut self, status: RunStatus) {
        self.status = status;
        self.seal_outcome_from_status();
    }

    pub fn set_completion(
        &mut self,
        completion: CompletionState,
        availability: EvaluatorAvailability,
    ) {
        self.completion = completion;
        self.evaluators.completion = availability;
    }

    /// Seal the independent result axes while keeping `status` as a compatibility
    /// projection for existing scenario implementations.
    pub fn seal_outcome_from_status(&mut self) {
        match self.status {
            RunStatus::Passed => {
                self.technical = TechnicalState::Valid;
                if self.evaluators.completion != EvaluatorAvailability::Available {
                    self.completion = CompletionState::Completed;
                    self.evaluators.completion = EvaluatorAvailability::Available;
                }
                self.objective_score = if self.completion == CompletionState::TaskIncomplete {
                    Some(0)
                } else {
                    self.score
                };
                self.quality_score_completed = (self.completion == CompletionState::Completed)
                    .then_some(self.score)
                    .flatten();
                self.evaluators.quality =
                    if self.completion == CompletionState::Completed && self.score.is_some() {
                        EvaluatorAvailability::Available
                    } else {
                        EvaluatorAvailability::NotRequired
                    };
            }
            RunStatus::HardGateFailed => {
                self.technical = TechnicalState::Valid;
                self.objective_score = Some(0);
                self.quality_score_completed = None;
                self.evaluators.quality = EvaluatorAvailability::NotRequired;
            }
            RunStatus::ResourceLimit => {
                self.technical = TechnicalState::Valid;
                if self.evaluators.completion != EvaluatorAvailability::Available {
                    self.completion = CompletionState::TaskIncomplete;
                    self.evaluators.completion = EvaluatorAvailability::Available;
                }
                self.objective_score = Some(0);
                self.quality_score_completed = None;
                self.evaluators.quality = EvaluatorAvailability::NotRequired;
            }
            RunStatus::JudgeError => {
                self.technical = TechnicalState::Valid;
                if self.evaluators.completion != EvaluatorAvailability::Available {
                    self.completion = CompletionState::Undetermined;
                    self.evaluators.completion = EvaluatorAvailability::Unavailable;
                }
                self.evaluators.quality = EvaluatorAvailability::Unavailable;
                self.objective_score = None;
                if self.completion != CompletionState::Completed {
                    self.quality_score_completed = None;
                }
            }
            RunStatus::SubjectError | RunStatus::InfrastructureError => {
                self.technical = TechnicalState::TechnicalInvalid;
                if self.evaluators.completion != EvaluatorAvailability::Available {
                    self.completion = CompletionState::Undetermined;
                    self.evaluators.completion = EvaluatorAvailability::Unavailable;
                }
                self.objective_score = None;
                if self.completion != CompletionState::Completed {
                    self.quality_score_completed = None;
                }
            }
        }
        // The compatibility status retains the first failure, but subsequent
        // infrastructure/cleanup failures still invalidate the technical axis.
        // Derive this from durable evidence rather than the previous axis value:
        // newly created reports start invalid and must become valid on success.
        if self.failures.iter().any(|failure| {
            failure.phase == FailurePhase::Cleanup
                || matches!(
                    failure.status,
                    RunStatus::SubjectError | RunStatus::InfrastructureError
                )
        }) {
            self.technical = TechnicalState::TechnicalInvalid;
            self.objective_score = None;
        }
    }

    pub fn update_cost(&mut self, judge_expected: bool) {
        let subject_usd = self
            .metrics
            .as_ref()
            .and_then(|metrics| metrics.totals.cost_usd);
        let judge_skipped = !judge_expected;
        let judge_usd = if judge_skipped {
            Some(0.0)
        } else {
            self.judge_usage.as_ref().and_then(|usage| usage.cost_usd)
        };
        self.cost = CostReport {
            subject_usd,
            judge_usd,
            total_usd: subject_usd
                .zip(judge_usd)
                .map(|(subject, judge)| subject + judge),
        };
    }

    pub fn update_efficiency(&mut self, work: WorkExpectation) {
        let mut unavailable = BTreeMap::new();
        let Some(metrics) = self.metrics.as_ref() else {
            for field in [
                "root_turns",
                "child_turns",
                "child_sessions",
                "function_calls",
                "function_call_errors",
                "validation_retries",
                "transient_resumes",
                "wake_resumes",
                "effective_fan_out",
                "input_tokens",
                "output_tokens",
                "total_tokens",
                "cost_usd",
                "critical_path_ms",
                "observed_work",
                "work_amplification",
            ] {
                unavailable.insert(field.into(), "terminal Harness metrics unavailable".into());
            }
            self.efficiency = Some(EfficiencyReport {
                wall_time_ms: self.wall_time_ms,
                root_turns: None,
                child_turns: None,
                child_sessions: None,
                function_calls: None,
                function_call_errors: None,
                validation_retries: None,
                transient_resumes: None,
                wake_resumes: None,
                effective_fan_out: None,
                critical_path_ms: None,
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
                cost_usd: self.cost.total_usd,
                minimum_expected_work: work.minimum_expected_work,
                observed_work: None,
                work_amplification: None,
                technical_attempts: 1,
                observed_complexity: unavailable_complexity(),
                unavailable,
            });
            return;
        };

        let root_turns = metrics
            .by_session
            .iter()
            .filter(|session| session.depth == 0)
            .map(|session| session.turns)
            .sum::<u64>();
        let child_turns = metrics
            .by_session
            .iter()
            .filter(|session| session.depth > 0)
            .map(|session| session.turns)
            .sum::<u64>();
        let child_sessions = metrics
            .by_session
            .iter()
            .filter(|session| session.depth > 0)
            .count() as u64;
        let mut children_by_parent = HashMap::<&str, u64>::new();
        for session in &metrics.by_session {
            if let Some(parent) = session.parent_session_id.as_deref() {
                *children_by_parent.entry(parent).or_default() += 1;
            }
        }
        let tree_fan_out = children_by_parent.values().copied().max().unwrap_or(0);
        let effective_fan_out = observed_parallel_spawns(self.transcript.as_ref()).unwrap_or_else(|| {
            unavailable.insert(
                "effective_fan_out".into(),
                "transcript unavailable; using maximum child count per parent instead of temporal spawn fan-out".into(),
            );
            tree_fan_out
        });
        let validation_retries = metrics.totals.validation_retries.or_else(|| {
            self.terminal_status
                .as_ref()
                .map(|status| u64::from(status.validation_retries))
        });
        let transient_resumes = metrics.totals.transient_resumes.or_else(|| {
            self.terminal_status
                .as_ref()
                .map(|status| u64::from(status.transient_resumes))
        });
        let wake_resumes = metrics
            .totals
            .wake_resumes
            .or_else(|| observed_wake_resumes(self.transcript.as_ref()));
        if validation_retries.is_none() {
            unavailable.insert(
                "validation_retries".into(),
                "terminal status did not expose validation retries".into(),
            );
        }
        if wake_resumes.is_none() {
            unavailable.insert(
                "wake_resumes".into(),
                "terminal status did not expose transient resumes".into(),
            );
        }
        if transient_resumes.is_none() {
            unavailable.insert(
                "transient_resumes".into(),
                "terminal lifecycle metrics did not expose transient stream resumes".into(),
            );
        }
        let critical_path_ms = metrics
            .traces
            .as_ref()
            .and_then(|traces| traces.get("duration_ms"))
            .and_then(Value::as_u64);
        if critical_path_ms.is_none() {
            unavailable.insert(
                "critical_path_ms".into(),
                "trace aggregation unavailable for this stack".into(),
            );
        }
        let total_tokens = metrics
            .totals
            .input_tokens
            .zip(metrics.totals.output_tokens)
            .and_then(|(input, output)| input.checked_add(output));
        if total_tokens.is_none() {
            unavailable.insert(
                "total_tokens".into(),
                "provider did not report complete input and output token usage".into(),
            );
        }
        if metrics.totals.input_tokens.is_none() {
            unavailable.insert(
                "input_tokens".into(),
                "provider did not report input token usage".into(),
            );
        }
        if metrics.totals.output_tokens.is_none() {
            unavailable.insert(
                "output_tokens".into(),
                "provider did not report output token usage".into(),
            );
        }
        if metrics.totals.cost_usd.is_none() {
            unavailable.insert(
                "cost_usd".into(),
                "provider did not report monetary cost".into(),
            );
        }
        let observed_work = validation_retries.and_then(|retries| {
            metrics
                .totals
                .turns
                .checked_add(metrics.totals.function_calls)?
                .checked_add(retries)
        });
        if observed_work.is_none() {
            unavailable.insert(
                "observed_work".into(),
                "validation retry count is required by the work formula".into(),
            );
            unavailable.insert(
                "work_amplification".into(),
                "observed work is unavailable".into(),
            );
        }
        let work_amplification = observed_work
            .map(|observed| observed as f64 / work.minimum_expected_work.max(1) as f64);
        let observed_complexity = observed_complexity(
            metrics,
            self.transcript.as_ref(),
            self.deliverables.len() as u64,
            validation_retries,
            wake_resumes,
            effective_fan_out,
        );
        self.efficiency = Some(EfficiencyReport {
            wall_time_ms: self.wall_time_ms,
            root_turns: Some(root_turns),
            child_turns: Some(child_turns),
            child_sessions: Some(child_sessions),
            function_calls: Some(metrics.totals.function_calls),
            function_call_errors: Some(metrics.totals.function_call_errors),
            validation_retries,
            transient_resumes,
            wake_resumes,
            effective_fan_out: Some(effective_fan_out),
            critical_path_ms,
            input_tokens: metrics.totals.input_tokens,
            output_tokens: metrics.totals.output_tokens,
            total_tokens,
            cost_usd: self.cost.total_usd,
            minimum_expected_work: work.minimum_expected_work,
            observed_work,
            work_amplification,
            technical_attempts: 1,
            observed_complexity,
            unavailable,
        });
    }

    pub fn attach_retry_attempts(&mut self, retry_attempts: Vec<RetryAttemptReport>) {
        if retry_attempts.is_empty() {
            return;
        }
        self.wall_time_ms = retry_attempts
            .iter()
            .fold(self.wall_time_ms, |total, attempt| {
                total.saturating_add(attempt.wall_time_ms)
            });
        self.cost.subject_usd = sum_cost(
            retry_attempts
                .iter()
                .map(|attempt| attempt.cost.subject_usd)
                .chain([self.cost.subject_usd]),
        );
        self.cost.judge_usd = sum_cost(
            retry_attempts
                .iter()
                .map(|attempt| attempt.cost.judge_usd)
                .chain([self.cost.judge_usd]),
        );
        self.cost.total_usd = sum_cost(
            retry_attempts
                .iter()
                .map(|attempt| attempt.cost.total_usd)
                .chain([self.cost.total_usd]),
        );
        self.aggregate_retry_efficiency(&retry_attempts);
        let expects_deliverables = self
            .dimensions
            .iter()
            .find(|dimension| dimension.dimension == EvaluationDimension::Deliverable)
            .and_then(|dimension| dimension.signals.get("expected"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.retry_attempts = retry_attempts;
        self.refresh_dimensions(expects_deliverables);
    }

    fn aggregate_retry_efficiency(&mut self, retries: &[RetryAttemptReport]) {
        let Some(current) = self.efficiency.clone() else {
            return;
        };
        let mut aggregate = current.clone();
        let mut attempts = retries
            .iter()
            .filter_map(|attempt| attempt.efficiency.as_ref())
            .collect::<Vec<_>>();
        attempts.push(&current);
        let all_present = attempts.len() == retries.len() + 1;
        aggregate.wall_time_ms = self.wall_time_ms;
        aggregate.root_turns = sum_optional(&attempts, |value| value.root_turns);
        aggregate.child_turns = sum_optional(&attempts, |value| value.child_turns);
        aggregate.child_sessions = sum_optional(&attempts, |value| value.child_sessions);
        aggregate.function_calls = sum_optional(&attempts, |value| value.function_calls);
        aggregate.function_call_errors =
            sum_optional(&attempts, |value| value.function_call_errors);
        aggregate.validation_retries = sum_optional(&attempts, |value| value.validation_retries);
        aggregate.transient_resumes = sum_optional(&attempts, |value| value.transient_resumes);
        aggregate.wake_resumes = sum_optional(&attempts, |value| value.wake_resumes);
        aggregate.effective_fan_out = max_optional(&attempts, |value| value.effective_fan_out);
        aggregate.critical_path_ms = sum_optional(&attempts, |value| value.critical_path_ms);
        aggregate.input_tokens = sum_optional(&attempts, |value| value.input_tokens);
        aggregate.output_tokens = sum_optional(&attempts, |value| value.output_tokens);
        aggregate.total_tokens = sum_optional(&attempts, |value| value.total_tokens);
        aggregate.cost_usd = self.cost.total_usd;
        aggregate.observed_work = sum_optional(&attempts, |value| value.observed_work);
        aggregate.work_amplification = aggregate
            .observed_work
            .map(|observed| observed as f64 / aggregate.minimum_expected_work.max(1) as f64);
        aggregate.technical_attempts = attempts.len().try_into().unwrap_or(u32::MAX);
        if !all_present {
            aggregate.unavailable.insert(
                "retry_efficiency".into(),
                "one or more retry attempts lacked efficiency evidence".into(),
            );
        }
        self.efficiency = Some(aggregate);
    }

    pub fn refresh_dimensions(&mut self, expects_deliverables: bool) {
        let deliverable_passed = expects_deliverables.then(|| {
            !self.deliverables.is_empty() && self.deliverables.iter().all(|item| item.passed())
        });
        let structural_gates = self
            .hard_gates
            .iter()
            .filter(|gate| gate.dimension == EvaluationDimension::StructuralIntegrity)
            .collect::<Vec<_>>();
        let structural_passed =
            (!structural_gates.is_empty()).then(|| structural_gates.iter().all(|gate| gate.passed));
        let efficiency = self.efficiency.as_ref().map_or_else(
            || serde_json::json!({ "available": false, "wall_time_ms": self.wall_time_ms }),
            |efficiency| serde_json::to_value(efficiency).unwrap_or(Value::Null),
        );
        let infrastructure_failures = self
            .failures
            .iter()
            .filter(|failure| failure.domain == FailureDomain::E2eInfrastructure)
            .count();
        self.dimensions = vec![
            DimensionReport {
                dimension: EvaluationDimension::Deliverable,
                passed: deliverable_passed,
                signals: serde_json::json!({
                    "expected": expects_deliverables,
                    "captured": self.deliverables.len(),
                    "valid": self.deliverables.iter().filter(|item| item.passed()).count(),
                }),
            },
            DimensionReport {
                dimension: EvaluationDimension::StructuralIntegrity,
                passed: structural_passed,
                signals: serde_json::json!({
                    "gates": structural_gates.len(),
                    "failed_gates": structural_gates.iter().filter(|gate| !gate.passed).count(),
                }),
            },
            DimensionReport {
                dimension: EvaluationDimension::Efficiency,
                passed: None,
                signals: efficiency,
            },
            DimensionReport {
                dimension: EvaluationDimension::Robustness,
                passed: None,
                signals: serde_json::json!({
                    "available": false,
                    "reason": "robustness requires a repeated comparable cohort",
                }),
            },
            DimensionReport {
                dimension: EvaluationDimension::E2eInfrastructure,
                passed: Some(infrastructure_failures == 0),
                signals: serde_json::json!({ "failures": infrastructure_failures }),
            },
        ];
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioFlowEvidence {
    pub definition_sha256: String,
    pub snapshot: Value,
    pub checkpoint: ArtifactReference,
    pub cleanup: WorkflowCleanupReport,
}

fn unavailable_complexity() -> ObservedComplexityReport {
    let unavailable = [
        "planning_depth",
        "dependency_depth",
        "parallel_branches",
        "external_systems",
        "state_transitions",
        "wake_cycles",
        "validation_loops",
        "artifact_count",
        "coordination_edges",
        "ambiguity_level",
    ]
    .into_iter()
    .map(|field| (field.into(), "terminal observation unavailable".into()))
    .collect();
    ObservedComplexityReport {
        unavailable,
        ..ObservedComplexityReport::default()
    }
}

fn observed_wake_resumes(transcript: Option<&Value>) -> Option<u64> {
    let messages = transcript?.get("messages")?.as_array()?;
    Some(
        messages
            .iter()
            .filter_map(|entry| entry.get("custom"))
            .filter(|custom| {
                custom.get("custom_type").and_then(Value::as_str) == Some("trigger_fired")
                    && matches!(
                        custom.pointer("/data/target").and_then(Value::as_str),
                        Some("harness::send" | "notify")
                    )
                    && custom.pointer("/data/note").is_none_or(Value::is_null)
            })
            .count()
            .try_into()
            .unwrap_or(u64::MAX),
    )
}

fn observed_parallel_spawns(transcript: Option<&Value>) -> Option<u64> {
    let messages = transcript?.get("messages")?.as_array()?;
    Some(
        messages
            .iter()
            .filter_map(|entry| entry.get("message"))
            .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
            .map(|message| {
                message
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|block| {
                        block.get("type").and_then(Value::as_str) == Some("function_call")
                            && (block.get("function_id").and_then(Value::as_str)
                                == Some("harness::spawn")
                                || (block.get("function_id").and_then(Value::as_str)
                                    == Some("agent_trigger")
                                    && block
                                        .pointer("/arguments/function")
                                        .and_then(Value::as_str)
                                        == Some("harness::spawn")))
                    })
                    .count() as u64
            })
            .max()
            .unwrap_or(0),
    )
}

fn observed_complexity(
    metrics: &SessionMetricsResponse,
    transcript: Option<&Value>,
    artifact_count: u64,
    validation_retries: Option<u64>,
    wake_resumes: Option<u64>,
    effective_fan_out: u64,
) -> ObservedComplexityReport {
    let child_sessions = metrics
        .by_session
        .iter()
        .filter(|session| session.depth > 0)
        .count() as u64;
    let dependency_depth = metrics
        .by_session
        .iter()
        .map(|session| u64::from(session.depth))
        .max()
        .unwrap_or(0);
    let calls = transcript
        .map(crate::scenarios::common::function_calls)
        .unwrap_or_default();
    let namespaces = calls
        .iter()
        .filter_map(|call| {
            call.function_id
                .split_once("::")
                .map(|(namespace, _)| namespace)
        })
        .filter(|namespace| !matches!(*namespace, "harness" | "engine" | "router" | "session"))
        .collect::<HashSet<_>>();
    let root_state_transitions = calls
        .iter()
        .filter(|call| {
            matches!(
                call.function_id.as_str(),
                "state::set"
                    | "state::update"
                    | "state::delete"
                    | "database::execute"
                    | "database::query"
            )
        })
        .count() as u64;
    let mut unavailable = BTreeMap::from([
        (
            "planning_depth".into(),
            "planning intent is not observable from runtime evidence".into(),
        ),
        (
            "ambiguity_level".into(),
            "input ambiguity is declared by the case, not inferred from behavior".into(),
        ),
    ]);
    let (external_systems, state_transitions) = if child_sessions == 0 {
        (Some(namespaces.len() as u64), Some(root_state_transitions))
    } else {
        unavailable.insert(
            "external_systems".into(),
            "child transcripts are not yet included in per-function evidence".into(),
        );
        unavailable.insert(
            "state_transitions".into(),
            "child per-function state transitions are not yet observable".into(),
        );
        (None, None)
    };
    if validation_retries.is_none() {
        unavailable.insert(
            "validation_loops".into(),
            "terminal status did not expose validation retries".into(),
        );
    }
    if wake_resumes.is_none() {
        unavailable.insert(
            "wake_cycles".into(),
            "terminal status did not expose transient resumes".into(),
        );
    }
    ObservedComplexityReport {
        planning_depth: None,
        dependency_depth: Some(dependency_depth),
        parallel_branches: Some(effective_fan_out),
        external_systems,
        state_transitions,
        wake_cycles: wake_resumes,
        validation_loops: validation_retries,
        artifact_count: Some(artifact_count),
        coordination_edges: Some(child_sessions),
        ambiguity_level: None,
        unavailable,
    }
}

fn sum_optional(
    attempts: &[&EfficiencyReport],
    value: impl Fn(&EfficiencyReport) -> Option<u64>,
) -> Option<u64> {
    attempts
        .iter()
        .try_fold(0_u64, |total, attempt| total.checked_add(value(attempt)?))
}

fn max_optional(
    attempts: &[&EfficiencyReport],
    value: impl Fn(&EfficiencyReport) -> Option<u64>,
) -> Option<u64> {
    attempts
        .iter()
        .try_fold(0_u64, |maximum, attempt| Some(maximum.max(value(attempt)?)))
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScenarioAggregate {
    pub planned_runs: u32,
    pub observed_runs: u32,
    pub deferred_runs: u32,
    pub completed_runs: u32,
    pub task_incomplete_runs: u32,
    pub undetermined_runs: u32,
    pub technical_valid_runs: u32,
    pub technical_invalid_runs: u32,
    pub execution_reliability: Option<f64>,
    pub completion_evidence_coverage: Option<f64>,
    pub completion_rate: Option<f64>,
    pub objective_scored_runs: u32,
    pub objective_median_score: Option<f64>,
    pub objective_score_coverage: Option<f64>,
    pub quality_scored_completed_runs: u32,
    pub quality_score_completed: Option<f64>,
    pub quality_coverage: Option<f64>,
    pub total_tokens_consumed: Option<u64>,
    pub judge_tokens_consumed: Option<u64>,
    pub tokens_completed_p50: Option<f64>,
    pub failed_attempt_tokens: Option<u64>,
    pub tokens_per_completion: Option<f64>,
    /// Compatibility alias for `observed_runs`.
    pub runs: u32,
    /// Compatibility alias for `objective_scored_runs`.
    pub scored_runs: u32,
    pub passed_runs: u32,
    pub required_passes: u32,
    pub pass_rate: f64,
    /// Compatibility alias for `objective_median_score`.
    pub median_score: Option<f64>,
    pub hard_gate_failures: u32,
    pub technical_failures: u32,
    pub cost: CostReport,
    pub robustness: RobustnessReport,
}

impl ScenarioAggregate {
    fn validate(&self) -> Result<()> {
        if self.planned_runs != self.observed_runs.saturating_add(self.deferred_runs) {
            bail!("scenario aggregate violates planned = observed + deferred");
        }
        if self.observed_runs
            != self
                .completed_runs
                .saturating_add(self.task_incomplete_runs)
                .saturating_add(self.undetermined_runs)
        {
            bail!(
                "scenario aggregate violates observed = completed + task_incomplete + undetermined"
            );
        }
        if self.observed_runs
            != self
                .technical_valid_runs
                .saturating_add(self.technical_invalid_runs)
        {
            bail!("scenario aggregate violates observed = technical_valid + technical_invalid");
        }
        if self.objective_scored_runs > self.observed_runs
            || self.quality_scored_completed_runs > self.completed_runs
        {
            bail!("scenario aggregate score counts exceed their eligible populations");
        }
        for (name, value) in [
            ("execution_reliability", self.execution_reliability),
            (
                "completion_evidence_coverage",
                self.completion_evidence_coverage,
            ),
            ("completion_rate", self.completion_rate),
            ("objective_score_coverage", self.objective_score_coverage),
            ("quality_coverage", self.quality_coverage),
        ] {
            if value.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
                bail!("scenario aggregate {name} must be null or a finite ratio");
            }
        }
        for (name, value) in [
            ("objective_median_score", self.objective_median_score),
            ("quality_score_completed", self.quality_score_completed),
            ("tokens_completed_p50", self.tokens_completed_p50),
            ("tokens_per_completion", self.tokens_per_completion),
        ] {
            if value.is_some_and(|value| !value.is_finite() || value < 0.0) {
                bail!("scenario aggregate {name} must be null or finite and non-negative");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RobustnessReport {
    pub sample_size: u32,
    pub minimum_sample_size: u32,
    pub tail_minimum_sample_size: u32,
    pub eligible: bool,
    pub deliverable_success_rate: Option<f64>,
    pub structural_integrity_rate: Option<f64>,
    pub technical_failure_rate: Option<f64>,
    pub flaky_rate: Option<f64>,
    pub median_wall_time_ms: Option<f64>,
    pub wall_time_variance: Option<f64>,
    pub p95_wall_time_ms: Option<u64>,
    pub cost_per_successful_deliverable: Option<f64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<String, String>,
}

fn count_runs(runs: &[E2eRunReport], predicate: impl Fn(&E2eRunReport) -> bool) -> u32 {
    runs.iter()
        .filter(|run| predicate(run))
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

fn ratio(numerator: u32, denominator: u32) -> Option<f64> {
    (denominator > 0).then(|| f64::from(numerator) / f64::from(denominator))
}

fn sum_u64(values: impl IntoIterator<Item = Option<u64>>) -> Option<u64> {
    values
        .into_iter()
        .try_fold(0_u64, |total, value| total.checked_add(value?))
}

fn usage_tokens(usage: Option<&ModelUsageReport>) -> Option<u64> {
    let usage = usage?;
    usage.input_tokens?.checked_add(usage.output_tokens?)
}

fn run_judge_tokens(run: &E2eRunReport) -> Option<u64> {
    sum_u64(
        run.retry_attempts
            .iter()
            .map(|attempt| usage_tokens(attempt.judge_usage.as_ref()))
            .chain(std::iter::once(usage_tokens(run.judge_usage.as_ref()))),
    )
}

fn failed_attempt_tokens(run: &E2eRunReport) -> Option<u64> {
    let retry_tokens = sum_u64(
        run.retry_attempts
            .iter()
            .map(|attempt| attempt.efficiency.as_ref()?.total_tokens),
    )?;
    if run.completion == CompletionState::Completed {
        return Some(retry_tokens);
    }
    let all_tokens = run.efficiency.as_ref()?.total_tokens?;
    // The terminal run efficiency includes the retries after attach_retry_attempts.
    // Subtract them so each physical attempt is counted exactly once.
    let terminal_tokens = all_tokens.checked_sub(retry_tokens)?;
    retry_tokens.checked_add(terminal_tokens)
}

fn default_scenario_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct E2eScenarioReport {
    pub scenario_id: String,
    #[serde(default)]
    pub case_id: String,
    #[serde(default = "default_scenario_version")]
    pub scenario_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<ScenarioCase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferral_reason: Option<String>,
    pub execution_policy: ExecutionPolicy,
    pub aggregate: ScenarioAggregate,
    pub passed: bool,
    pub runs: Vec<E2eRunReport>,
}

impl E2eScenarioReport {
    #[cfg(test)]
    pub fn aggregate(
        scenario_id: impl Into<String>,
        scenario_version: u32,
        execution_policy: ExecutionPolicy,
        runs: Vec<E2eRunReport>,
    ) -> Self {
        let case = ScenarioCase::new(
            scenario_id,
            scenario_version,
            0,
            serde_json::json!({ "variant": "canonical" }),
            ComplexityProfile::default(),
            Vec::new(),
            DeliverableContract::default(),
        )
        .expect("canonical scenario case is valid");
        Self::aggregate_case(case, execution_policy, runs)
    }

    pub fn aggregate_case(
        case: ScenarioCase,
        execution_policy: ExecutionPolicy,
        runs: Vec<E2eRunReport>,
    ) -> Self {
        let planned_runs = runs.len() as u32;
        Self::aggregate_case_with_planned(case, execution_policy, planned_runs, runs)
    }

    pub fn aggregate_case_with_planned(
        case: ScenarioCase,
        execution_policy: ExecutionPolicy,
        planned_runs: u32,
        runs: Vec<E2eRunReport>,
    ) -> Self {
        Self::aggregate_with_planned(
            case.scenario_id.clone(),
            case.case_id.clone(),
            case.scenario_version,
            Some(case),
            execution_policy,
            planned_runs,
            runs,
        )
    }

    /// Preserve requested slots even when no executable case could be materialized.
    /// Version zero denotes an unknown version, not a fabricated case identity.
    pub fn deferred(
        scenario_id: String,
        case_id: String,
        scenario_version: u32,
        execution_policy: ExecutionPolicy,
        planned_runs: u32,
        reason: String,
    ) -> Self {
        let mut report = Self::aggregate_with_planned(
            scenario_id,
            case_id,
            scenario_version,
            None,
            execution_policy,
            planned_runs,
            Vec::new(),
        );
        report.deferral_reason = Some(reason);
        report
    }

    fn aggregate_with_planned(
        scenario_id: String,
        case_id: String,
        scenario_version: u32,
        case: Option<ScenarioCase>,
        execution_policy: ExecutionPolicy,
        planned_runs: u32,
        mut runs: Vec<E2eRunReport>,
    ) -> Self {
        for run in &mut runs {
            run.seal_outcome_from_status();
        }
        let run_count = runs.len() as u32;
        let completed_runs = count_runs(&runs, |run| run.completion == CompletionState::Completed);
        let task_incomplete_runs = count_runs(&runs, |run| {
            run.completion == CompletionState::TaskIncomplete
        });
        let undetermined_runs =
            count_runs(&runs, |run| run.completion == CompletionState::Undetermined);
        let technical_valid_runs = count_runs(&runs, |run| run.technical == TechnicalState::Valid);
        let technical_invalid_runs = count_runs(&runs, |run| {
            run.technical == TechnicalState::TechnicalInvalid
        });
        let deferred_runs = planned_runs.saturating_sub(run_count);
        let objective_scored_runs = count_runs(&runs, |run| run.objective_score.is_some());
        let quality_scored_completed_runs = count_runs(&runs, |run| {
            run.completion == CompletionState::Completed && run.quality_score_completed.is_some()
        });
        let objective_median_score = median(runs.iter().filter_map(|run| run.objective_score));
        let quality_score_completed = median(
            runs.iter()
                .filter(|run| run.completion == CompletionState::Completed)
                .filter_map(|run| run.quality_score_completed),
        );
        let total_tokens_consumed =
            sum_u64(runs.iter().map(|run| run.efficiency.as_ref()?.total_tokens));
        let judge_tokens_consumed = sum_u64(runs.iter().map(run_judge_tokens));
        let completed_token_values = runs
            .iter()
            .filter(|run| run.completion == CompletionState::Completed)
            .map(|run| run.efficiency.as_ref()?.total_tokens)
            .collect::<Option<Vec<_>>>();
        let tokens_completed_p50 = completed_token_values.as_ref().and_then(|values| {
            median_f64(&values.iter().map(|value| *value as f64).collect::<Vec<_>>())
        });
        let failed_attempt_tokens = sum_u64(runs.iter().map(failed_attempt_tokens));
        let tokens_per_completion = total_tokens_consumed
            .zip((completed_runs > 0).then_some(completed_runs))
            .map(|(tokens, completed)| tokens as f64 / f64::from(completed));
        let determined_runs = completed_runs + task_incomplete_runs;
        let passed_runs = runs
            .iter()
            .filter(|run| run.status == RunStatus::Passed)
            .count() as u32;
        let hard_gate_failures = runs
            .iter()
            .filter(|run| run.status == RunStatus::HardGateFailed)
            .count() as u32;
        let technical_failures = technical_invalid_runs;
        let required_passes = required_passes(planned_runs);
        let cost = CostReport {
            subject_usd: sum_cost(runs.iter().map(|run| run.cost.subject_usd)),
            judge_usd: sum_cost(runs.iter().map(|run| run.cost.judge_usd)),
            total_usd: sum_cost(runs.iter().map(|run| run.cost.total_usd)),
        };
        let robustness = robustness_report(&runs);
        let passed = planned_runs > 0
            && run_count == planned_runs
            && technical_failures == 0
            && undetermined_runs == 0
            && passed_runs >= required_passes;
        Self {
            case_id,
            scenario_id,
            scenario_version,
            case,
            deferral_reason: None,
            execution_policy,
            aggregate: ScenarioAggregate {
                planned_runs,
                observed_runs: run_count,
                deferred_runs,
                completed_runs,
                task_incomplete_runs,
                undetermined_runs,
                technical_valid_runs,
                technical_invalid_runs,
                execution_reliability: ratio(technical_valid_runs, planned_runs),
                completion_evidence_coverage: ratio(determined_runs, planned_runs),
                completion_rate: ratio(completed_runs, determined_runs),
                objective_scored_runs,
                objective_median_score,
                objective_score_coverage: ratio(objective_scored_runs, planned_runs),
                quality_scored_completed_runs,
                quality_score_completed,
                quality_coverage: ratio(quality_scored_completed_runs, completed_runs),
                total_tokens_consumed,
                judge_tokens_consumed,
                tokens_completed_p50,
                failed_attempt_tokens,
                tokens_per_completion,
                runs: run_count,
                scored_runs: objective_scored_runs,
                passed_runs,
                required_passes,
                pass_rate: if run_count == 0 {
                    0.0
                } else {
                    f64::from(passed_runs) / f64::from(run_count)
                },
                median_score: objective_median_score,
                hard_gate_failures,
                technical_failures,
                cost,
                robustness,
            },
            passed,
            runs,
        }
    }

    pub fn refresh_aggregate(&mut self) -> Result<()> {
        let planned_runs = self.aggregate.planned_runs;
        self.validate_deferral()?;
        let case = self.case.take();
        let runs = std::mem::take(&mut self.runs);
        let deferral_reason = self.deferral_reason.take();
        *self = Self::aggregate_with_planned(
            self.scenario_id.clone(),
            self.case_id.clone(),
            self.scenario_version,
            case,
            self.execution_policy,
            planned_runs,
            runs,
        );
        self.deferral_reason = deferral_reason;
        Ok(())
    }

    fn validate_deferral(&self) -> Result<()> {
        if let Some(reason) = &self.deferral_reason {
            if reason.trim().is_empty() || self.aggregate.deferred_runs == 0 {
                bail!("scenario deferral reason requires a nonempty reason and deferred slots");
            }
        }
        if self.case.is_none()
            && (self.deferral_reason.is_none()
                || self.aggregate.planned_runs == 0
                || self.aggregate.deferred_runs != self.aggregate.planned_runs
                || self.aggregate.observed_runs != 0
                || !self.runs.is_empty()
                || self.passed)
        {
            bail!("scenario without a materialized case must be wholly deferred with a reason");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelArtifact {
    pub model: String,
    pub provider: String,
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub supports_tools: Option<bool>,
    pub supports_vision: Option<bool>,
}

pub const OBSERVATION_SCHEMA: &str = "e2e-observation/v1";
pub const CATALOG_SCHEMA: &str = "e2e-scenario-catalog/v4";
pub use crate::result_contract::{
    RESULTS_SCHEMA_VERSION, RESULT_CONTRACT_SHA256, SCORING_PROFILE_SHA256,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunnerIdentity {
    pub name: String,
    pub version: String,
    pub revision: String,
}

impl RunnerIdentity {
    pub fn runtime() -> Self {
        Self {
            name: env!("CARGO_PKG_NAME").to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            revision: crate::identity::nonempty_env("HARNESS_E2E_REVISION")
                .unwrap_or_else(|| env!("HARNESS_E2E_BUILD_REVISION").to_string()),
        }
    }

    pub fn validate(&self) -> Result<()> {
        required_observation_value(&self.name, "runner name")?;
        required_observation_value(&self.version, "runner version")?;
        validate_observation_revision(&self.revision, "runner revision")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationTargetIdentity {
    pub application: String,
    pub version: String,
    pub stack: StackIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationPlanIdentity {
    pub id: String,
    pub revision: String,
    pub sha256: String,
    pub catalog_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationEnvironment {
    Demonstration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationDecision {
    ObserveOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationMode {
    pub environment: ObservationEnvironment,
    pub decision: ObservationDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationSelectedCase {
    pub scenario_id: crate::markdown::ScenarioKey,
    pub scenario_version: u32,
    pub case_id: String,
    pub seed: u64,
    pub inputs_sha256: String,
    pub contract_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationCorrelation {
    pub system: String,
    pub deployment_id: String,
    pub operation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationRunContract {
    pub mode: ObservationMode,
    pub target: ObservationTargetIdentity,
    pub plan: ObservationPlanIdentity,
    pub runner: RunnerIdentity,
    pub attempt: u32,
    pub selected_cases: Vec<ObservationSelectedCase>,
    pub correlation: ObservationCorrelation,
}

impl ObservationRunContract {
    pub fn validate(&self) -> Result<()> {
        if self.target.application != "harness" {
            bail!("run_contract target application must be 'harness'");
        }
        required_observation_value(&self.target.version, "target version")?;
        validate_observation_stack(&self.target.stack)?;
        required_observation_value(&self.plan.id, "plan id")?;
        required_observation_value(&self.plan.revision, "plan revision")?;
        validate_observation_sha256(&self.plan.sha256, "plan digest")?;
        validate_observation_sha256(&self.plan.catalog_sha256, "catalog digest")?;
        self.runner.validate()?;
        if self.attempt == 0 {
            bail!("run_contract attempt must be at least 1");
        }
        if self.selected_cases.is_empty() {
            bail!("run_contract selected_cases cannot be empty");
        }
        let mut identities = HashSet::new();
        for case in &self.selected_cases {
            if case.scenario_version == 0 || case.case_id.trim().is_empty() {
                bail!("run_contract contains an invalid selected case identity");
            }
            validate_observation_sha256(&case.inputs_sha256, "selected case inputs")?;
            validate_observation_sha256(&case.contract_sha256, "selected case contract")?;
            if !identities.insert((case.scenario_id.as_str(), case.case_id.as_str())) {
                bail!("run_contract contains duplicate selected cases");
            }
        }
        required_observation_value(&self.correlation.system, "correlation system")?;
        required_observation_value(&self.correlation.deployment_id, "deployment id")?;
        required_observation_value(&self.correlation.operation_id, "operation id")?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationExecutionIdentity {
    pub id: String,
    pub attempt: u32,
    pub request_sha256: String,
    pub run_contract_sha256: String,
    pub started_at: String,
    pub completed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationIdentity {
    pub target: ObservationTargetIdentity,
    pub plan: ObservationPlanIdentity,
    pub runner: RunnerIdentity,
    pub execution: ObservationExecutionIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationObjective {
    Passed,
    QualityAdvisory,
    HardGateFailed,
    TechnicalFailed,
    InfrastructureFailed,
    Cancelled,
    UnsupportedPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationDataAvailability {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationOutcome {
    pub control_phase: String,
    pub objective: ObservationObjective,
    pub passed: Option<bool>,
    pub data_availability: ObservationDataAvailability,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationMetricOrigin {
    Observed,
    DerivedFromObserved,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationMetricDerivation {
    pub metric: String,
    pub origin: ObservationMetricOrigin,
    pub formula: String,
    pub formula_version: String,
}

/// A metric is explicit about its unit and availability so a recorded zero
/// cannot be mistaken for a missing measurement. The legacy `metrics` object
/// remains alongside this additive projection for current consumers.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationMetric {
    pub value: Option<f64>,
    pub unit: String,
    pub availability: ObservationDataAvailability,
    pub origin: ObservationMetricOrigin,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationSample {
    pub scenario_id: String,
    pub scenario_version: u32,
    pub case_id: String,
    pub seed: u64,
    pub run_id: String,
    pub attempt_id: String,
    pub status: RunStatus,
    pub origin: ObservationMetricOrigin,
    pub data_availability: ObservationDataAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<EfficiencyReport>,
    #[serde(default)]
    pub metric_values: BTreeMap<String, ObservationMetric>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derivations: Vec<ObservationMetricDerivation>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationEvidence {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationProvenance {
    pub subject_model: String,
    pub subject_provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_under_test: Option<SystemUnderTestIdentity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct E2eObservationEnvelope {
    pub schema: String,
    pub identity: ObservationIdentity,
    pub mode: ObservationMode,
    pub outcome: ObservationOutcome,
    pub samples: Vec<ObservationSample>,
    pub evidence: ObservationEvidence,
    pub provenance: ObservationProvenance,
}

impl E2eObservationEnvelope {
    pub fn validate(&self) -> Result<()> {
        if self.schema != OBSERVATION_SCHEMA {
            bail!("observation schema must be {OBSERVATION_SCHEMA}");
        }
        self.identity.runner.validate()?;
        if self.identity.target.application != "harness" {
            bail!("observation target application must be 'harness'");
        }
        required_observation_value(&self.identity.target.version, "target version")?;
        validate_observation_stack(&self.identity.target.stack)?;
        required_observation_value(&self.identity.plan.id, "plan id")?;
        required_observation_value(&self.identity.plan.revision, "plan revision")?;
        validate_observation_sha256(&self.identity.plan.sha256, "plan digest")?;
        validate_observation_sha256(&self.identity.plan.catalog_sha256, "catalog digest")?;
        validate_observation_sha256(
            &self.identity.execution.request_sha256,
            "observation request digest",
        )?;
        validate_observation_sha256(
            &self.identity.execution.run_contract_sha256,
            "observation run contract digest",
        )?;
        ExecutionIdentity {
            execution_id: self.identity.execution.id.clone(),
            lane: "observation".into(),
            started_at: self.identity.execution.started_at.clone(),
            completed_at: self.identity.execution.completed_at.clone(),
        }
        .validate()?;
        if let Some(digest) = &self.evidence.results_sha256 {
            validate_observation_sha256(digest, "results evidence")?;
        }
        if let Some(digest) = &self.evidence.manifest_sha256 {
            validate_observation_sha256(digest, "manifest evidence")?;
        }
        if let Some(system) = &self.provenance.system_under_test {
            system.validate()?;
        }
        Ok(())
    }

    pub fn write_to(&self, output: &Path) -> Result<ArtifactReference> {
        self.validate()?;
        artifact::write_json(
            output,
            Path::new("observation.json"),
            "terminal_observation",
            OBSERVATION_SCHEMA,
            self,
        )
    }

    pub fn read_from(output: &Path) -> Result<Self> {
        let path = if output.is_dir() {
            output.join("observation.json")
        } else {
            output.to_path_buf()
        };
        let observation: Self = serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
        )
        .with_context(|| format!("decode {}", path.display()))?;
        observation.validate()?;
        let root = path.parent().context("observation path has no parent")?;
        for reference in &observation.evidence.artifacts {
            reference.verify(root)?;
        }
        Ok(observation)
    }
}

fn required_observation_value(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{label} cannot be empty");
    }
    Ok(())
}

fn validate_observation_revision(value: &str, label: &str) -> Result<()> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{label} must be a full immutable Git SHA");
    }
    Ok(())
}

fn validate_observation_sha256(value: &str, label: &str) -> Result<()> {
    if value.len() != 71
        || !value.starts_with("sha256:")
        || !value[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        bail!("{label} must be a sha256:<64 hex characters> digest");
    }
    Ok(())
}

fn validate_observation_stack(stack: &StackIdentity) -> Result<()> {
    match stack {
        StackIdentity::Source {
            workers_repository,
            workers_revision,
        } => {
            required_observation_value(workers_repository, "workers repository")?;
            validate_observation_revision(workers_revision, "workers revision")
        }
        StackIdentity::Registry {
            stack_versions,
            stack_lock_digest,
        } => {
            if stack_versions.is_empty() {
                bail!("target registry stack_versions cannot be empty");
            }
            for (worker, version) in stack_versions {
                required_observation_value(worker, "registry worker")?;
                required_observation_value(version, "registry worker version")?;
            }
            validate_observation_sha256(stack_lock_digest, "stack lock digest")
        }
    }
}

impl From<Model> for ModelArtifact {
    fn from(model: Model) -> Self {
        let model = model.into_normalized();
        Self {
            model: model.id,
            provider: model.provider,
            context_window: model.context_window,
            max_output_tokens: model.max_output_tokens,
            supports_tools: model.supports_tools,
            supports_vision: model.supports_vision,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct E2eManifest {
    pub execution: ExecutionIdentity,
    pub system_under_test: SystemUnderTestIdentity,
    pub subject: ModelArtifact,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge: Option<ModelArtifact>,
    pub control_plane: ControlPlaneEvidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_contract: Option<ObservationRunContract>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worker_contracts: Vec<ObservedWorkerContract>,
}

impl E2eManifest {
    pub fn validate(&self) -> Result<()> {
        self.execution.validate()?;
        self.system_under_test.validate()?;
        if self.control_plane.functions.is_empty() {
            bail!("manifest control plane cannot be empty");
        }
        if let Some(contract) = &self.observation_contract {
            contract.validate()?;
        }
        let mut observed = HashSet::new();
        for contract in &self.worker_contracts {
            if contract.function_id.trim().is_empty() {
                bail!("manifest worker contract function_id cannot be empty");
            }
            if !observed.insert(contract.function_id.as_str()) {
                bail!(
                    "manifest has duplicate worker contract '{}'",
                    contract.function_id
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservedWorkerContract {
    pub function_id: String,
    pub request_schema_sha256: String,
    pub response_schema_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReportState {
    Complete,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObjectiveOutcome {
    Passed,
    Failed,
    Inconclusive,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct E2eReport {
    pub schema_version: u32,
    pub result_contract_sha256: String,
    pub scoring_profile_sha256: String,
    pub report_state: ReportState,
    pub objective_outcome: ObjectiveOutcome,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub persistence_errors: Vec<String>,
    /// Soft scheduling deadline: prevents starting slots after this interval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_start_deadline_seconds: Option<u64>,
    pub execution: ExecutionIdentity,
    pub system_under_test: SystemUnderTestIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<ArtifactReference>,
    pub subject: ModelArtifact,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge: Option<ModelArtifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_contract: Option<ObservationRunContract>,
    pub passed: bool,
    #[serde(default)]
    pub redaction: crate::redaction::RedactionReport,
    pub assessment_contract: AssessmentContract,
    pub scenarios: Vec<E2eScenarioReport>,
}

impl E2eReport {
    pub fn new(
        execution: ExecutionIdentity,
        system_under_test: SystemUnderTestIdentity,
        subject: ModelArtifact,
        judge: Option<ModelArtifact>,
        judge_protocol: Option<String>,
        engine_revision: Option<String>,
        scenarios: Vec<E2eScenarioReport>,
    ) -> Self {
        let passed = !scenarios.is_empty() && scenarios.iter().all(|scenario| scenario.passed);
        let partial = scenarios
            .iter()
            .any(|scenario| scenario.aggregate.deferred_runs > 0);
        let inconclusive = scenarios.iter().any(|scenario| {
            scenario.aggregate.undetermined_runs > 0
                || scenario.aggregate.technical_invalid_runs > 0
        });
        let mut report = Self {
            schema_version: RESULTS_SCHEMA_VERSION,
            result_contract_sha256: RESULT_CONTRACT_SHA256.into(),
            scoring_profile_sha256: SCORING_PROFILE_SHA256.into(),
            report_state: if partial {
                ReportState::Partial
            } else {
                ReportState::Complete
            },
            objective_outcome: if inconclusive || partial {
                ObjectiveOutcome::Inconclusive
            } else if passed {
                ObjectiveOutcome::Passed
            } else {
                ObjectiveOutcome::Failed
            },
            persistence_errors: Vec::new(),
            slot_start_deadline_seconds: None,
            execution,
            system_under_test,
            manifest: None,
            subject,
            judge,
            judge_protocol,
            engine_revision,
            observation_contract: None,
            passed,
            redaction: crate::redaction::RedactionReport::default(),
            assessment_contract: AssessmentContract { runs: Vec::new() },
            scenarios,
        };
        report.assessment_contract = AssessmentContract::from_assessment_evidence(&report);
        report
    }

    pub fn record_persistence_error(&mut self, error: String) {
        self.persistence_errors.push(error);
        self.report_state = ReportState::Partial;
        self.objective_outcome = ObjectiveOutcome::Inconclusive;
        self.passed = false;
    }

    pub fn write_to(&mut self, output: &Path, manifest: &E2eManifest) -> Result<PathBuf> {
        self.schema_version = RESULTS_SCHEMA_VERSION;
        fs::create_dir_all(output)
            .with_context(|| format!("create report directory {}", output.display()))?;
        manifest.validate().context("validate E2E manifest")?;
        self.redact_sensitive_evidence()?;
        self.materialize_evidence(output)?;
        self.assessment_contract = AssessmentContract::from_assessment_evidence(self);
        let manifest_reference = artifact::write_json(
            output,
            Path::new("manifest.json"),
            "execution_manifest",
            "manifest",
            manifest,
        )?;
        self.manifest = Some(manifest_reference);
        validate_against_schema(&schema::manifest(), manifest, "manifest")?;
        self.assessment_contract.validate(self)?;
        self.validate(manifest, output)
            .context("validate E2E results")?;
        validate_against_schema(&schema::results(), self, "results")?;
        let path = output.join("results.json");
        write_json_value(&path, self)?;
        Ok(path)
    }

    pub fn read_from(input: &Path) -> Result<(Self, PathBuf)> {
        let path = if input.is_dir() {
            input.join("results.json")
        } else {
            input.to_path_buf()
        };
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let value: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("decode E2E report {}", path.display()))?;
        let version = value.get("schema_version").and_then(Value::as_u64);
        if version != Some(u64::from(RESULTS_SCHEMA_VERSION)) {
            bail!(
                "unsupported results schema_version {}; expected {} with result contract {}",
                version.map_or_else(|| "missing".into(), |value| value.to_string()),
                RESULTS_SCHEMA_VERSION,
                RESULT_CONTRACT_SHA256
            );
        }
        let report: Self = serde_json::from_value(value)
            .with_context(|| format!("decode typed E2E report {}", path.display()))?;
        validate_against_schema(&schema::results(), &report, "results")?;
        report.assessment_contract.validate(&report)?;
        let output = path
            .parent()
            .context("results path has no parent directory")?;
        let manifest_path = output.join("manifest.json");
        let manifest: E2eManifest = serde_json::from_slice(
            &fs::read(&manifest_path)
                .with_context(|| format!("read {}", manifest_path.display()))?,
        )
        .with_context(|| format!("decode {}", manifest_path.display()))?;
        validate_against_schema(&schema::manifest(), &manifest, "manifest")?;
        manifest.validate()?;
        report.validate(&manifest, output)?;
        Ok((report, path))
    }

    fn validate(&self, manifest: &E2eManifest, output: &Path) -> Result<()> {
        if self.schema_version != RESULTS_SCHEMA_VERSION {
            bail!("results schema_version must be {RESULTS_SCHEMA_VERSION}");
        }
        if self.result_contract_sha256 != RESULT_CONTRACT_SHA256 {
            bail!("results contract fingerprint is unsupported");
        }
        if self.scoring_profile_sha256 != SCORING_PROFILE_SHA256 {
            bail!("results scoring profile fingerprint is unsupported");
        }
        if self.execution.execution_id != manifest.execution.execution_id {
            bail!("results and manifest execution identities differ");
        }
        self.system_under_test.validate()?;
        if serde_json::to_value(&self.system_under_test)?
            != serde_json::to_value(&manifest.system_under_test)?
        {
            bail!("results and manifest system identities differ");
        }
        if self.observation_contract != manifest.observation_contract {
            bail!("results and manifest observation contracts differ");
        }
        if let Some(contract) = &self.observation_contract {
            contract.validate()?;
        }
        let reference = self
            .manifest
            .as_ref()
            .context("results are missing manifest reference")?;
        if reference.path != "manifest.json" {
            bail!("results manifest reference must point to manifest.json");
        }
        reference.verify(output)?;
        for scenario in &self.scenarios {
            scenario.aggregate.validate()?;
            scenario.validate_deferral()?;
            if scenario.scenario_id.trim().is_empty() {
                bail!("scenario has an empty scenario_id");
            }
            if scenario.case_id.trim().is_empty() {
                bail!("scenario {} has an empty case_id", scenario.scenario_id);
            }
            if let Some(case) = &scenario.case {
                case.validate()?;
                let expected_method = crate::scenarios::ComplexityMethod::CapabilityV2;
                if case.complexity.method != expected_method {
                    bail!(
                        "results schema_version {} requires {:?} complexity classification",
                        self.schema_version,
                        expected_method
                    );
                }
                if scenario.case_id != case.case_id
                    || scenario.scenario_id != case.scenario_id
                    || scenario.scenario_version != case.scenario_version
                {
                    bail!("scenario identity differs from its materialized case");
                }
            }
            for run in &scenario.runs {
                validate_attempt_identity(run)?;
                if (run.technical == TechnicalState::TechnicalInvalid
                    || run.completion == CompletionState::Undetermined)
                    && run.objective_score.is_some()
                {
                    bail!(
                        "run '{}' has an objective score without valid determined evidence",
                        run.run_id
                    );
                }
                if run.completion == CompletionState::TaskIncomplete
                    && run.technical == TechnicalState::Valid
                    && run.objective_score != Some(0)
                {
                    bail!(
                        "run '{}' task_incomplete objective score must be zero",
                        run.run_id
                    );
                }
                if run.completion != CompletionState::Completed
                    && run.quality_score_completed.is_some()
                {
                    bail!(
                        "run '{}' has a completed-task quality score without completion",
                        run.run_id
                    );
                }
                let mut measurement_ids = HashSet::new();
                for measurement in &run.scenario_measurements {
                    if measurement.id.trim().is_empty()
                        || measurement.unit.trim().is_empty()
                        || !measurement.value.is_finite()
                    {
                        bail!("run '{}' has an invalid scenario measurement", run.run_id);
                    }
                    if !measurement_ids.insert(measurement.id.as_str()) {
                        bail!(
                            "run '{}' repeats scenario measurement '{}'",
                            run.run_id,
                            measurement.id
                        );
                    }
                }
                let mut run_contract_ids = HashSet::new();
                for contract in &run.worker_contracts {
                    if !run_contract_ids.insert(contract.function_id.as_str()) {
                        bail!(
                            "run '{}' repeats observed worker contract '{}'",
                            run.run_id,
                            contract.function_id
                        );
                    }
                    if !manifest.worker_contracts.contains(contract) {
                        bail!(
                            "run '{}' observed worker contract '{}' that is absent from the manifest",
                            run.run_id,
                            contract.function_id
                        );
                    }
                }
                for reference in &run.evidence {
                    reference.verify(output)?;
                }
                verify_deliverables(output, &run.deliverables)?;
                validate_semantic_evidence(
                    output,
                    run.scenario_flow.as_ref(),
                    &run.semantic_tests,
                )?;
                for attempt in &run.retry_attempts {
                    validate_retry_identity(run, attempt)?;
                    for reference in &attempt.evidence {
                        reference.verify(output)?;
                    }
                    verify_deliverables(output, &attempt.deliverables)?;
                    validate_semantic_evidence(
                        output,
                        attempt.scenario_flow.as_ref(),
                        &attempt.semantic_tests,
                    )?;
                }
            }
            let expected = E2eScenarioReport::aggregate_with_planned(
                scenario.scenario_id.clone(),
                scenario.case_id.clone(),
                scenario.scenario_version,
                scenario.case.clone(),
                scenario.execution_policy,
                scenario.aggregate.planned_runs,
                scenario.runs.clone(),
            );
            if serde_json::to_value(&scenario.aggregate)?
                != serde_json::to_value(&expected.aggregate)?
                || scenario.passed != expected.passed
            {
                bail!(
                    "scenario '{}' aggregate differs from its run evidence",
                    scenario.scenario_id
                );
            }
        }
        let scenario_partial = self
            .scenarios
            .iter()
            .any(|scenario| scenario.aggregate.deferred_runs > 0);
        if self
            .persistence_errors
            .iter()
            .any(|error| error.trim().is_empty())
        {
            bail!("results persistence errors must be nonempty");
        }
        let expected_partial = scenario_partial || !self.persistence_errors.is_empty();
        if (self.report_state == ReportState::Partial) != expected_partial {
            bail!("results partial state must match deferred slots or persistence errors");
        }
        let expected_passed = !expected_partial
            && !self.scenarios.is_empty()
            && self.scenarios.iter().all(|scenario| scenario.passed);
        let expected_inconclusive = expected_partial
            || self.scenarios.iter().any(|scenario| {
                scenario.aggregate.undetermined_runs > 0
                    || scenario.aggregate.technical_invalid_runs > 0
            });
        let expected_objective_outcome = if expected_inconclusive {
            ObjectiveOutcome::Inconclusive
        } else if expected_passed {
            ObjectiveOutcome::Passed
        } else {
            ObjectiveOutcome::Failed
        };
        if self.passed != expected_passed || self.objective_outcome != expected_objective_outcome {
            bail!("results top-level outcome differs from scenario evidence");
        }
        Ok(())
    }

    fn redact_sensitive_evidence(&mut self) -> Result<()> {
        let policy = crate::redaction::RedactionPolicy::from_environment();
        let mut redaction = self.redaction.clone();
        for error in &mut self.persistence_errors {
            redact_string(&policy, &mut redaction, error);
        }
        for scenario in &mut self.scenarios {
            if let Some(reason) = &mut scenario.deferral_reason {
                redact_string(&policy, &mut redaction, reason);
            }
            for run in &mut scenario.runs {
                redaction.merge(run.asset_redaction.clone());
                redact_string(&policy, &mut redaction, &mut run.prompt);
                if let Some(transcript) = &mut run.transcript {
                    redaction.merge(policy.redact_value(transcript));
                }
                redact_attempt_annotations(
                    &policy,
                    &mut redaction,
                    &mut run.failures,
                    &mut run.hard_gates,
                    &mut run.criteria,
                    &mut run.dimensions,
                    &mut run.deliverables,
                );
                redact_structured_assessments(
                    &policy,
                    &mut redaction,
                    &mut run.assessment_results,
                    &mut run.asset_assessments,
                )?;
                scan_deliverable_content(&policy, &run.deliverables)?;
                redact_semantic_evidence(
                    &policy,
                    &mut redaction,
                    &mut run.scenario_flow,
                    &mut run.semantic_tests,
                );
                redact_optional_report_value(
                    &policy,
                    &mut redaction,
                    &mut run.markdown_execution,
                    "Markdown execution",
                )?;
                redact_optional_report_value(
                    &policy,
                    &mut redaction,
                    &mut run.instruction_adherence,
                    "instruction adherence",
                )?;
                for retry in &mut run.retry_attempts {
                    redaction.merge(retry.asset_redaction.clone());
                    if let Some(transcript) = &mut retry.transcript {
                        redaction.merge(policy.redact_value(transcript));
                    }
                    redact_attempt_annotations(
                        &policy,
                        &mut redaction,
                        &mut retry.failures,
                        &mut [],
                        &mut [],
                        &mut retry.dimensions,
                        &mut retry.deliverables,
                    );
                    redact_structured_assessments(
                        &policy,
                        &mut redaction,
                        &mut retry.assessment_results,
                        &mut retry.asset_assessments,
                    )?;
                    scan_deliverable_content(&policy, &retry.deliverables)?;
                    redact_semantic_evidence(
                        &policy,
                        &mut redaction,
                        &mut retry.scenario_flow,
                        &mut retry.semantic_tests,
                    );
                    redact_optional_report_value(
                        &policy,
                        &mut redaction,
                        &mut retry.markdown_execution,
                        "retry Markdown execution",
                    )?;
                    redact_optional_report_value(
                        &policy,
                        &mut redaction,
                        &mut retry.instruction_adherence,
                        "retry instruction adherence",
                    )?;
                }
            }
        }
        let mut value = serde_json::to_value(&self.assessment_contract)
            .context("serialize assessment contract before redaction")?;
        redaction.merge(policy.redact_value(&mut value));
        self.assessment_contract =
            serde_json::from_value(value).context("decode assessment contract after redaction")?;
        self.redaction = redaction;
        policy.assert_clean(&serde_json::to_vec(&self).context("scan E2E report")?)?;
        Ok(())
    }

    fn materialize_evidence(&mut self, output: &Path) -> Result<()> {
        for scenario in &mut self.scenarios {
            for run in &mut scenario.runs {
                validate_attempt_identity(run)?;
                for attempt in &run.retry_attempts {
                    validate_retry_identity(run, attempt)?;
                }
                let markdown_evidence = if run.markdown_execution.is_some() {
                    std::mem::take(&mut run.evidence)
                } else {
                    Vec::new()
                };
                run.evidence.clear();
                materialize_attempt_evidence(
                    output,
                    &run.run_id,
                    &run.attempt_id,
                    AttemptEvidence {
                        transcript: run.transcript.as_ref(),
                        metrics: run.metrics.as_ref(),
                        deliverables: &mut run.deliverables,
                        asset_capture_manifest: run.asset_capture_manifest.as_ref(),
                        final_assessment_input: run.final_assessment_input.as_ref(),
                        references: &mut run.evidence,
                    },
                )?;
                append_semantic_references(
                    &mut run.evidence,
                    run.scenario_flow.as_ref(),
                    &run.semantic_tests,
                );
                append_verified_references(output, &mut run.evidence, markdown_evidence)?;
                bind_assessment_evidence(&mut run.assessment_results, &run.evidence);
                for attempt in &mut run.retry_attempts {
                    let markdown_evidence = if attempt.markdown_execution.is_some() {
                        std::mem::take(&mut attempt.evidence)
                    } else {
                        Vec::new()
                    };
                    attempt.evidence.clear();
                    materialize_attempt_evidence(
                        output,
                        &attempt.run_id,
                        &attempt.attempt_id,
                        AttemptEvidence {
                            transcript: attempt.transcript.as_ref(),
                            metrics: attempt.metrics.as_ref(),
                            deliverables: &mut attempt.deliverables,
                            asset_capture_manifest: attempt.asset_capture_manifest.as_ref(),
                            final_assessment_input: None,
                            references: &mut attempt.evidence,
                        },
                    )?;
                    append_semantic_references(
                        &mut attempt.evidence,
                        attempt.scenario_flow.as_ref(),
                        &attempt.semantic_tests,
                    );
                    append_verified_references(output, &mut attempt.evidence, markdown_evidence)?;
                    bind_assessment_evidence(&mut attempt.assessment_results, &attempt.evidence);
                }
            }
        }
        Ok(())
    }
}

fn redact_optional_report_value<T>(
    policy: &crate::redaction::RedactionPolicy,
    redaction: &mut crate::redaction::RedactionReport,
    field: &mut Option<T>,
    label: &str,
) -> Result<()>
where
    T: Serialize + DeserializeOwned,
{
    let Some(current) = field.as_ref() else {
        return Ok(());
    };
    let mut value = serde_json::to_value(current)
        .with_context(|| format!("serialize {label} before redaction"))?;
    redaction.merge(policy.redact_value(&mut value));
    *field = Some(
        serde_json::from_value(value).with_context(|| format!("decode {label} after redaction"))?,
    );
    Ok(())
}

fn append_verified_references(
    output: &Path,
    references: &mut Vec<ArtifactReference>,
    additional: Vec<ArtifactReference>,
) -> Result<()> {
    for reference in additional {
        reference.verify(output)?;
        if references.iter().any(|existing| existing == &reference) {
            continue;
        }
        if references
            .iter()
            .any(|existing| existing.id == reference.id || existing.path == reference.path)
        {
            bail!(
                "Markdown evidence conflicts with an existing artifact reference: {}",
                reference.path
            );
        }
        references.push(reference);
    }
    Ok(())
}

fn validate_semantic_evidence(
    output: &Path,
    flow: Option<&ScenarioFlowEvidence>,
    tests: &[WorkflowStepReport],
) -> Result<()> {
    if tests.is_empty() {
        if flow.is_some() {
            bail!("scenario flow evidence requires at least one semantic test");
        }
        return Ok(());
    }
    let flow = flow.context("semantic tests require scenario flow evidence")?;
    if flow.definition_sha256.trim().is_empty()
        || flow.snapshot.get("executable").and_then(Value::as_bool) != Some(false)
    {
        bail!("scenario flow snapshot must be identified and explicitly non-executable");
    }
    flow.checkpoint.verify(output)?;
    for test in tests {
        for asset in &test.assets {
            asset.artifact.verify(output)?;
            if asset.artifact.sha256 != asset.content_sha256
                || asset.artifact.size_bytes != asset.size_bytes
            {
                bail!(
                    "semantic test asset '{}.{}' differs from its immutable reference",
                    test.node_id,
                    asset.id
                );
            }
        }
    }
    Ok(())
}

fn append_semantic_references(
    references: &mut Vec<ArtifactReference>,
    flow: Option<&ScenarioFlowEvidence>,
    tests: &[WorkflowStepReport],
) {
    let candidates = flow.into_iter().map(|flow| &flow.checkpoint).chain(
        tests
            .iter()
            .flat_map(|test| test.assets.iter().map(|asset| &asset.artifact)),
    );
    for candidate in candidates {
        if !references
            .iter()
            .any(|reference| reference.id == candidate.id && reference.sha256 == candidate.sha256)
        {
            references.push(candidate.clone());
        }
    }
}

fn redact_semantic_evidence(
    policy: &crate::redaction::RedactionPolicy,
    redaction: &mut crate::redaction::RedactionReport,
    flow: &mut Option<ScenarioFlowEvidence>,
    tests: &mut [WorkflowStepReport],
) {
    if let Some(flow) = flow {
        redaction.merge(policy.redact_value(&mut flow.snapshot));
        if let Some(failure) = &mut flow.cleanup.failure {
            redact_string(policy, redaction, failure);
        }
    }
    for test in tests {
        redaction.merge(test.redaction.clone());
        for output in test.outputs.values_mut() {
            redaction.merge(policy.redact_value(&mut output.value));
        }
        if let Some(transcript) = &mut test.transcript {
            redaction.merge(policy.redact_value(transcript));
        }
        if let Some(metrics) = &mut test.metrics {
            redaction.merge(policy.redact_value(metrics));
        }
        for asset in &mut test.assets {
            redaction.merge(policy.redact_value(&mut asset.preview));
        }
        for gate in &mut test.hard_gates {
            redact_string(policy, redaction, &mut gate.reason);
        }
        for evaluation in &mut test.evaluations {
            redact_string(policy, redaction, &mut evaluation.summary);
        }
        for failure in &mut test.failures {
            redact_string(policy, redaction, &mut failure.message);
        }
        if let Some(reason) = &mut test.skip_reason {
            redact_string(policy, redaction, reason);
        }
    }
}

fn validate_against_schema<T>(
    schema: &schemars::schema::RootSchema,
    value: &T,
    label: &str,
) -> Result<()>
where
    T: Serialize,
{
    let schema = serde_json::to_value(schema).context("serialize JSON Schema")?;
    let validator = jsonschema::JSONSchema::compile(&schema)
        .map_err(|error| anyhow::anyhow!("compile {label} schema: {error}"))?;
    let instance = serde_json::to_value(value).with_context(|| format!("serialize {label}"))?;
    if let Err(errors) = validator.validate(&instance) {
        let errors = errors.map(|error| error.to_string()).collect::<Vec<_>>();
        bail!(
            "{label} does not match its declared schema: {}",
            errors.join("; ")
        );
    }
    Ok(())
}

fn redact_string(
    policy: &crate::redaction::RedactionPolicy,
    report: &mut crate::redaction::RedactionReport,
    value: &mut String,
) {
    let (sanitized, nested) = policy.redact_text(value);
    *value = sanitized;
    report.merge(nested);
}

fn redact_attempt_annotations(
    policy: &crate::redaction::RedactionPolicy,
    redaction: &mut crate::redaction::RedactionReport,
    failures: &mut [FailureRecord],
    hard_gates: &mut [HardGateReport],
    criteria: &mut [CriterionReport],
    dimensions: &mut [DimensionReport],
    deliverables: &mut [DeliverableReport],
) {
    for failure in failures {
        redact_string(policy, redaction, &mut failure.message);
    }
    for gate in hard_gates {
        redact_string(policy, redaction, &mut gate.reason);
    }
    for criterion in criteria {
        redact_string(policy, redaction, &mut criterion.reason);
    }
    for dimension in dimensions {
        redaction.merge(policy.redact_value(&mut dimension.signals));
    }
    for deliverable in deliverables {
        redaction.merge(policy.redact_value(&mut deliverable.preview));
        for invariant in &mut deliverable.invariants {
            redact_string(policy, redaction, &mut invariant.reason);
        }
        for provenance in &mut deliverable.provenance {
            redact_string(policy, redaction, &mut provenance.source_id);
            redact_string(policy, redaction, &mut provenance.relation);
        }
    }
}

fn redact_structured_assessments(
    policy: &crate::redaction::RedactionPolicy,
    redaction: &mut crate::redaction::RedactionReport,
    assessments: &mut Vec<AssessmentResult>,
    assets: &mut Vec<AssetAssessmentResult>,
) -> Result<()> {
    let mut value = serde_json::to_value(&*assessments)
        .context("serialize per-assessment results before redaction")?;
    redaction.merge(policy.redact_value(&mut value));
    *assessments =
        serde_json::from_value(value).context("decode per-assessment results after redaction")?;

    let mut value =
        serde_json::to_value(&*assets).context("serialize asset assessments before redaction")?;
    redaction.merge(policy.redact_value(&mut value));
    *assets = serde_json::from_value(value).context("decode asset assessments after redaction")?;
    Ok(())
}

fn scan_deliverable_content(
    policy: &crate::redaction::RedactionPolicy,
    deliverables: &[DeliverableReport],
) -> Result<()> {
    for deliverable in deliverables {
        let bytes = match &deliverable.content {
            CapturedDeliverableContent::Json(value) => serde_json::to_vec(value)?,
            CapturedDeliverableContent::TextUtf8(value) => value.as_bytes().to_vec(),
        };
        policy.assert_clean(&bytes).with_context(|| {
            format!(
                "deliverable '{}' contains secret material and cannot be persisted",
                deliverable.id
            )
        })?;
    }
    Ok(())
}

fn write_json_value(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .with_context(|| format!("serialize {}", path.display()))?;
    bytes.push(b'\n');
    artifact::write_atomic(path, &bytes)
}

fn validate_attempt_identity(run: &E2eRunReport) -> Result<()> {
    validate_artifact_identifier(&run.run_id, "run id")?;
    validate_artifact_identifier(&run.attempt_id, "attempt id")?;
    if run.attempt_number == 0 {
        bail!("attempt_number must start at 1");
    }
    Ok(())
}

fn validate_artifact_identifier(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        bail!("{label} must be a safe non-empty identifier");
    }
    Ok(())
}

fn validate_retry_identity(run: &E2eRunReport, attempt: &RetryAttemptReport) -> Result<()> {
    if attempt.run_id != run.run_id {
        bail!("retry attempt belongs to a different run");
    }
    validate_artifact_identifier(&attempt.attempt_id, "retry attempt id")?;
    if attempt.attempt_id == run.attempt_id {
        bail!("retry and final attempts share an attempt_id");
    }
    if attempt.attempt_number == 0 || attempt.attempt_number >= run.attempt_number {
        bail!("retry attempt numbers must precede the final attempt");
    }
    Ok(())
}

struct AttemptEvidence<'a> {
    transcript: Option<&'a Value>,
    metrics: Option<&'a SessionMetricsResponse>,
    deliverables: &'a mut [DeliverableReport],
    asset_capture_manifest: Option<&'a ArtifactReference>,
    final_assessment_input: Option<&'a ArtifactReference>,
    references: &'a mut Vec<ArtifactReference>,
}

fn materialize_attempt_evidence(
    output: &Path,
    run_id: &str,
    attempt_id: &str,
    evidence: AttemptEvidence<'_>,
) -> Result<()> {
    let AttemptEvidence {
        transcript,
        metrics,
        deliverables,
        asset_capture_manifest,
        final_assessment_input,
        references,
    } = evidence;
    let root = PathBuf::from("evidence").join(run_id).join(attempt_id);
    if let Some(transcript) = transcript {
        references.push(artifact::write_json(
            output,
            &root.join("transcript.json"),
            "transcript",
            "transcript",
            transcript,
        )?);
    }
    if let Some(metrics) = metrics {
        references.push(artifact::write_json(
            output,
            &root.join("metrics.json"),
            "metrics",
            "metrics",
            metrics,
        )?);
    }
    if let Some(manifest) = asset_capture_manifest {
        manifest.verify(output)?;
        references.push(manifest.clone());
    }
    if let Some(input) = final_assessment_input {
        input.verify(output)?;
        references.push(input.clone());
    }
    let deliverable_root = PathBuf::from("deliverables").join(run_id).join(attempt_id);
    for deliverable in deliverables {
        let reference = match &deliverable.content {
            CapturedDeliverableContent::Json(value) => artifact::write_json(
                output,
                &deliverable_root.join(format!("{}.json", deliverable.id)),
                deliverable.id.clone(),
                deliverable.kind.clone(),
                value,
            )?,
            CapturedDeliverableContent::TextUtf8(value) => artifact::write_bytes(
                output,
                &deliverable_root.join(format!("{}.txt", deliverable.id)),
                deliverable.id.clone(),
                deliverable.kind.clone(),
                deliverable.media_type.clone(),
                value.as_bytes(),
            )?,
        };
        deliverable.artifact = Some(reference);
    }
    Ok(())
}

fn bind_assessment_evidence(
    assessments: &mut [AssessmentResult],
    references: &[ArtifactReference],
) {
    let Some(transcript) = references
        .iter()
        .find(|reference| reference.id == "transcript")
    else {
        return;
    };
    for assessment in assessments {
        if assessment.target.kind == AssessmentTargetKind::Criterion
            && assessment.evidence.is_empty()
            && !matches!(
                assessment.outcome,
                crate::assessment::AssessmentOutcome::NotEvaluated
                    | crate::assessment::AssessmentOutcome::Unavailable
            )
        {
            assessment
                .evidence
                .push(EvidenceReference::from(transcript));
        }
    }
}

fn verify_deliverables(output: &Path, deliverables: &[DeliverableReport]) -> Result<()> {
    for deliverable in deliverables {
        let reference = deliverable
            .artifact
            .as_ref()
            .with_context(|| format!("deliverable '{}' is missing its artifact", deliverable.id))?;
        reference.verify(output)?;
        let bytes = fs::read(output.join(&reference.path))
            .with_context(|| format!("read deliverable artifact {}", reference.path))?;
        let observed_hash = match deliverable.content_format {
            DeliverableContentFormat::Json => {
                let content: Value = serde_json::from_slice(&bytes)
                    .with_context(|| format!("decode deliverable artifact {}", reference.path))?;
                artifact::sha256_value(&content)?
            }
            DeliverableContentFormat::TextUtf8 => {
                std::str::from_utf8(&bytes).with_context(|| {
                    format!("decode UTF-8 deliverable artifact {}", reference.path)
                })?;
                artifact::sha256_bytes(&bytes)
            }
        };
        if observed_hash != deliverable.content_sha256 {
            bail!(
                "deliverable '{}' content hash does not match its artifact",
                deliverable.id
            );
        }
    }
    Ok(())
}

fn robustness_report(runs: &[E2eRunReport]) -> RobustnessReport {
    const MINIMUM_SAMPLE_SIZE: u32 = 5;
    const TAIL_MINIMUM_SAMPLE_SIZE: u32 = 20;
    let sample_size = runs.len().try_into().unwrap_or(u32::MAX);
    let mut unavailable = BTreeMap::new();
    if sample_size < MINIMUM_SAMPLE_SIZE {
        unavailable.insert(
            "robustness".into(),
            format!(
                "requires at least {MINIMUM_SAMPLE_SIZE} comparable runs; observed {sample_size}"
            ),
        );
    }
    let p95_wall_time_ms = if sample_size >= TAIL_MINIMUM_SAMPLE_SIZE {
        percentile_nearest_rank(runs.iter().map(|run| run.wall_time_ms), 95)
    } else {
        unavailable.insert(
            "p95_wall_time_ms".into(),
            format!(
                "requires at least {TAIL_MINIMUM_SAMPLE_SIZE} comparable runs; observed {sample_size}"
            ),
        );
        None
    };
    let deliverable_success_rate = dimension_success_rate(runs, EvaluationDimension::Deliverable);
    if deliverable_success_rate.is_none() {
        unavailable.insert(
            "deliverable_success_rate".into(),
            "one or more runs did not declare a deliverable outcome".into(),
        );
    }
    let structural_integrity_rate =
        dimension_success_rate(runs, EvaluationDimension::StructuralIntegrity);
    if structural_integrity_rate.is_none() {
        unavailable.insert(
            "structural_integrity_rate".into(),
            "one or more runs did not declare a structural outcome".into(),
        );
    }
    let technical_failure_rate = (!runs.is_empty()).then(|| {
        runs.iter()
            .filter(|run| run.status.is_technical_failure())
            .count() as f64
            / runs.len() as f64
    });
    let flaky_rate = if runs.len() >= 2 {
        let mut statuses = BTreeMap::<RunStatus, usize>::new();
        for run in runs {
            *statuses.entry(run.status).or_default() += 1;
        }
        let modal = statuses.values().copied().max().unwrap_or(0);
        Some((runs.len().saturating_sub(modal)) as f64 / runs.len() as f64)
    } else {
        unavailable.insert("flaky_rate".into(), "requires at least two runs".into());
        None
    };
    let wall_times = runs
        .iter()
        .map(|run| run.wall_time_ms as f64)
        .collect::<Vec<_>>();
    let median_wall_time_ms = median_f64(&wall_times);
    let wall_time_variance = if wall_times.len() >= 2 {
        let mean = wall_times.iter().sum::<f64>() / wall_times.len() as f64;
        Some(
            wall_times
                .iter()
                .map(|value| (value - mean).powi(2))
                .sum::<f64>()
                / wall_times.len() as f64,
        )
    } else {
        unavailable.insert(
            "wall_time_variance".into(),
            "requires at least two runs".into(),
        );
        None
    };
    let successful_deliverables = runs
        .iter()
        .filter(|run| dimension_outcome(run, EvaluationDimension::Deliverable) == Some(true))
        .count();
    let total_cost = sum_cost(runs.iter().map(|run| run.cost.total_usd));
    let cost_per_successful_deliverable = if successful_deliverables == 0 {
        unavailable.insert(
            "cost_per_successful_deliverable".into(),
            "no successful deliverable was observed".into(),
        );
        None
    } else {
        total_cost.map(|cost| cost / successful_deliverables as f64)
    };
    if successful_deliverables > 0 && total_cost.is_none() {
        unavailable.insert(
            "cost_per_successful_deliverable".into(),
            "one or more attempts did not report monetary cost".into(),
        );
    }
    RobustnessReport {
        sample_size,
        minimum_sample_size: MINIMUM_SAMPLE_SIZE,
        tail_minimum_sample_size: TAIL_MINIMUM_SAMPLE_SIZE,
        eligible: sample_size >= MINIMUM_SAMPLE_SIZE,
        deliverable_success_rate,
        structural_integrity_rate,
        technical_failure_rate,
        flaky_rate,
        median_wall_time_ms,
        wall_time_variance,
        p95_wall_time_ms,
        cost_per_successful_deliverable,
        unavailable,
    }
}

fn dimension_outcome(run: &E2eRunReport, dimension: EvaluationDimension) -> Option<bool> {
    run.dimensions
        .iter()
        .find(|report| report.dimension == dimension)
        .and_then(|report| report.passed)
}

fn dimension_success_rate(runs: &[E2eRunReport], dimension: EvaluationDimension) -> Option<f64> {
    if runs.is_empty() {
        return None;
    }
    let outcomes = runs
        .iter()
        .map(|run| dimension_outcome(run, dimension))
        .collect::<Option<Vec<_>>>()?;
    Some(outcomes.iter().filter(|passed| **passed).count() as f64 / outcomes.len() as f64)
}

fn percentile_nearest_rank(
    values: impl IntoIterator<Item = u64>,
    percentile: usize,
) -> Option<u64> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    if values.is_empty() || !(1..=100).contains(&percentile) {
        return None;
    }
    values.sort_unstable();
    let rank = percentile.saturating_mul(values.len()).saturating_add(99) / 100;
    values.get(rank.saturating_sub(1)).copied()
}

fn median_f64(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        Some(values[middle])
    } else {
        Some((values[middle - 1] + values[middle]) / 2.0)
    }
}

fn required_passes(runs: u32) -> u32 {
    runs.saturating_mul(2).saturating_add(2) / 3
}

fn median(values: impl IntoIterator<Item = u8>) -> Option<f64> {
    let mut values: Vec<_> = values.into_iter().collect();
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        Some(f64::from(values[middle]))
    } else {
        Some((f64::from(values[middle - 1]) + f64::from(values[middle])) / 2.0)
    }
}

fn sum_cost(values: impl IntoIterator<Item = Option<f64>>) -> Option<f64> {
    values
        .into_iter()
        .try_fold(0.0, |total, value| Some(total + value?))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::assessment::{
        AssessmentKind, AssessmentOutcome, AssessmentPolicy, AssessmentScore, AssessmentSource,
        AssessmentTarget, SystemStatus,
    };
    use crate::identity::StackIdentity;
    use crate::scenarios::{ArtifactExpectation, InvariantSpec};
    use crate::wire::{
        ControlPlaneEvidence, FunctionContractEvidence, SessionMetricsResponse, StatusReport,
    };
    use crate::workflow::{
        ActivationPolicy, DependencyPolicy, WorkflowEvaluationOutcome, WorkflowEvaluationResult,
        WorkflowGateResult, WorkflowStepStatus,
    };

    const TEST_DIGEST: &str =
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn scoring_profile_fingerprint_tracks_checked_in_bytes() {
        let profile: Value = serde_json::from_slice(include_bytes!(
            "../config/scoring/difficulty-weighted-v1.json"
        ))
        .unwrap();
        assert_eq!(
            artifact::sha256_value(&profile).unwrap(),
            SCORING_PROFILE_SHA256
        );
    }

    #[test]
    fn result_contract_fingerprint_tracks_checked_in_schema() {
        let schema: Value =
            serde_json::from_slice(include_bytes!("../schemas/results.json")).unwrap();
        assert_eq!(
            artifact::sha256_value(&schema).unwrap(),
            RESULT_CONTRACT_SHA256
        );
    }

    fn execution() -> ExecutionIdentity {
        ExecutionIdentity {
            execution_id: "execution".into(),
            lane: "test".into(),
            started_at: "2026-08-12T12:00:00Z".into(),
            completed_at: "2026-08-12T12:00:01Z".into(),
        }
    }

    fn system() -> SystemUnderTestIdentity {
        SystemUnderTestIdentity {
            stack: StackIdentity::Source {
                workers_repository: "iii-hq/workers".into(),
                workers_revision: "0123456789abcdef0123456789abcdef01234567".into(),
            },
            engine_version: "0.22.0".into(),
            engine_revision: None,
            harness_version: "1.8.0".into(),
            e2e_repository: "iii-hq/workers".into(),
            e2e_revision: "0123456789abcdef0123456789abcdef01234567".into(),
            contract_hashes: BTreeMap::from([("harness::status".into(), TEST_DIGEST.into())]),
        }
    }

    fn manifest() -> E2eManifest {
        E2eManifest {
            execution: execution(),
            system_under_test: system(),
            subject: model(),
            judge: None,
            control_plane: ControlPlaneEvidence {
                functions: vec![FunctionContractEvidence {
                    function_id: "harness::status".into(),
                    request_schema: serde_json::json!({"type": "object"}),
                    response_schema: serde_json::json!({"type": "object"}),
                    sha256: TEST_DIGEST.into(),
                }],
            },
            observation_contract: None,
            worker_contracts: Vec::new(),
        }
    }

    fn observation_contract() -> ObservationRunContract {
        ObservationRunContract {
            mode: ObservationMode {
                environment: ObservationEnvironment::Demonstration,
                decision: ObservationDecision::ObserveOnly,
            },
            target: ObservationTargetIdentity {
                application: "harness".into(),
                version: system().harness_version,
                stack: system().stack,
            },
            plan: ObservationPlanIdentity {
                id: "deployment-d0".into(),
                revision: "revision-1".into(),
                sha256: TEST_DIGEST.into(),
                catalog_sha256: format!("sha256:{}", "a".repeat(64)),
            },
            runner: RunnerIdentity {
                name: "harness-e2e".into(),
                version: "0.1.0".into(),
                revision: "0123456789abcdef0123456789abcdef01234567".into(),
            },
            attempt: 1,
            selected_cases: vec![ObservationSelectedCase {
                scenario_id: crate::scenarios::ScenarioId::ContextPressure.into(),
                scenario_version: 4,
                case_id: "context_pressure:v4:seed-0000000000000001".into(),
                seed: 1,
                inputs_sha256: format!("sha256:{}", "b".repeat(64)),
                contract_sha256: format!("sha256:{}", "c".repeat(64)),
            }],
            correlation: ObservationCorrelation {
                system: "release-control".into(),
                deployment_id: "deployment-1".into(),
                operation_id: "operation-1".into(),
            },
        }
    }

    fn report(scenarios: Vec<E2eScenarioReport>) -> E2eReport {
        E2eReport::new(execution(), system(), model(), None, None, None, scenarios)
    }

    fn run(score: u8, passed: bool) -> E2eRunReport {
        let mut report = E2eRunReport::new(
            "run".into(),
            "attempt".into(),
            1,
            "session".into(),
            "prompt".into(),
        );
        report.score = Some(score);
        report.finish(if passed {
            RunStatus::Passed
        } else {
            RunStatus::HardGateFailed
        });
        if !passed {
            report.set_completion(
                CompletionState::TaskIncomplete,
                EvaluatorAvailability::Available,
            );
        }
        report
    }

    fn aggregate(runs: Vec<E2eRunReport>) -> E2eScenarioReport {
        E2eScenarioReport::aggregate(
            "case",
            1,
            ExecutionPolicy {
                max_turns: 1,
                max_output_tokens: Some(1),
                max_total_tokens: Some(1),
                stuck_timeout_seconds: 1,
                max_validation_retries: None,
            },
            runs,
        )
    }

    #[test]
    fn one_run_requires_that_run_to_pass() {
        assert!(aggregate(vec![run(80, true)]).passed);
        assert!(!aggregate(vec![run(100, false)]).passed);
    }

    #[test]
    fn three_runs_require_two_passes() {
        let report = aggregate(vec![run(79, false), run(80, true), run(90, true)]);
        assert!(report.passed);
        assert_eq!(report.aggregate.required_passes, 2);
        assert_eq!(report.aggregate.pass_rate, 2.0 / 3.0);
        assert_eq!(report.aggregate.median_score, Some(80.0));
    }

    #[test]
    fn costs_are_aggregated_without_hiding_unknown_values() {
        let mut first = run(90, true);
        first.cost = CostReport {
            subject_usd: Some(0.1),
            judge_usd: Some(0.02),
            total_usd: Some(0.12),
        };
        let mut second = run(90, true);
        second.cost = CostReport {
            subject_usd: Some(0.2),
            judge_usd: None,
            total_usd: None,
        };
        let report = aggregate(vec![first, second]);
        assert!((report.aggregate.cost.subject_usd.unwrap() - 0.3).abs() < f64::EPSILON);
        assert_eq!(report.aggregate.cost.judge_usd, None);
        assert_eq!(report.aggregate.cost.total_usd, None);
    }

    #[test]
    fn technical_errors_are_not_quality_scores_and_fail_the_aggregate() {
        let mut error = E2eRunReport::new(
            "run".into(),
            "attempt".into(),
            1,
            "session".into(),
            "prompt".into(),
        );
        error.push_failure(
            RunStatus::JudgeError,
            FailurePhase::Evaluate,
            "judge unavailable",
        );
        let report = aggregate(vec![run(90, true), run(90, true), error]);
        assert!(!report.passed);
        assert_eq!(report.aggregate.scored_runs, 2);
        assert_eq!(report.aggregate.technical_invalid_runs, 0);
        assert_eq!(report.aggregate.undetermined_runs, 1);
        assert_eq!(report.aggregate.median_score, Some(90.0));
    }

    #[test]
    fn partial_aggregate_preserves_orthogonal_coverage_and_quality() {
        let completed = run(90, true);
        let mut incomplete = run(40, false);
        incomplete.set_completion(
            CompletionState::TaskIncomplete,
            EvaluatorAvailability::Available,
        );
        let case = ScenarioCase::new(
            "case",
            1,
            0,
            serde_json::json!({ "variant": "canonical" }),
            ComplexityProfile::default(),
            Vec::new(),
            DeliverableContract::default(),
        )
        .unwrap();
        let report = E2eScenarioReport::aggregate_case_with_planned(
            case,
            ExecutionPolicy {
                max_turns: 1,
                max_output_tokens: Some(1),
                max_total_tokens: Some(1),
                stuck_timeout_seconds: 1,
                max_validation_retries: None,
            },
            3,
            vec![completed, incomplete],
        );

        assert_eq!(report.aggregate.observed_runs, 2);
        assert_eq!(report.aggregate.deferred_runs, 1);
        assert_eq!(report.aggregate.completed_runs, 1);
        assert_eq!(report.aggregate.task_incomplete_runs, 1);
        assert_eq!(report.aggregate.objective_median_score, Some(45.0));
        assert_eq!(report.aggregate.quality_score_completed, Some(90.0));
        assert_eq!(report.aggregate.completion_rate, Some(0.5));
        assert_eq!(
            report.aggregate.completion_evidence_coverage,
            Some(2.0 / 3.0)
        );
        assert!(!report.passed);
    }

    #[test]
    fn cleanup_failure_does_not_hide_the_primary_failure_status() {
        let mut report = E2eRunReport::new(
            "run".into(),
            "attempt".into(),
            1,
            "session".into(),
            "prompt".into(),
        );
        report.push_failure(
            RunStatus::SubjectError,
            FailurePhase::Execute,
            "provider unavailable",
        );
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Cleanup,
            "cleanup unavailable",
        );

        assert_eq!(report.status, RunStatus::SubjectError);
        assert_eq!(report.failures.len(), 2);
    }

    #[test]
    fn secondary_infrastructure_failure_survives_sealing_and_aggregation() {
        for primary in [
            RunStatus::HardGateFailed,
            RunStatus::ResourceLimit,
            RunStatus::JudgeError,
        ] {
            for phase in [FailurePhase::Cleanup, FailurePhase::Collect] {
                let mut failed = run(80, true);
                failed.push_failure(primary, FailurePhase::Evaluate, "primary failure");
                failed.push_failure(
                    RunStatus::InfrastructureError,
                    phase,
                    "secondary infrastructure failure",
                );
                assert_eq!(failed.status, primary);
                assert_eq!(failed.technical, TechnicalState::TechnicalInvalid);
                assert_eq!(failed.objective_score, None);

                let bytes = serde_json::to_vec(&failed).unwrap();
                let decoded: E2eRunReport = serde_json::from_slice(&bytes).unwrap();
                let mut aggregated = aggregate(vec![decoded]);
                aggregated.refresh_aggregate().unwrap();

                assert_eq!(aggregated.runs[0].status, primary);
                assert_eq!(
                    aggregated.runs[0].technical,
                    TechnicalState::TechnicalInvalid
                );
                assert_eq!(aggregated.runs[0].completion, CompletionState::Completed);
                assert_eq!(aggregated.runs[0].objective_score, None);
                assert_eq!(aggregated.aggregate.technical_invalid_runs, 1);
                assert_eq!(aggregated.aggregate.objective_scored_runs, 0);
                assert!(!aggregated.passed);
            }
        }
    }

    #[test]
    fn hard_gate_failure_is_not_infrastructure_failure_despite_shared_domain() {
        let mut failed = run(0, false);
        failed.push_typed_failure(
            RunStatus::HardGateFailed,
            FailurePhase::Evaluate,
            "custom_objective_gate",
            RetryScope::None,
            ContaminationScope::None,
            "objective gate failed",
        );
        assert_eq!(failed.failures[0].domain, FailureDomain::E2eInfrastructure);
        let aggregated = aggregate(vec![failed]);
        assert_eq!(aggregated.runs[0].technical, TechnicalState::Valid);
        assert_eq!(aggregated.runs[0].objective_score, Some(0));
        assert_eq!(aggregated.aggregate.technical_invalid_runs, 0);
    }

    #[test]
    fn initially_invalid_run_becomes_valid_when_execution_succeeds() {
        let mut completed = E2eRunReport::new(
            "run".into(),
            "attempt".into(),
            1,
            "session".into(),
            "prompt".into(),
        );
        assert_eq!(completed.technical, TechnicalState::TechnicalInvalid);
        completed.score = Some(90);
        completed.finish(RunStatus::Passed);
        assert_eq!(completed.technical, TechnicalState::Valid);
        assert_eq!(completed.objective_score, Some(90));
    }

    #[test]
    fn wholly_deferred_scenario_has_no_fabricated_case_or_run() {
        let mut deferred = E2eScenarioReport::deferred(
            "unmaterialized".into(),
            "requested-case".into(),
            0,
            aggregate(Vec::new()).execution_policy,
            3,
            "scenario materialization failed".into(),
        );
        deferred.refresh_aggregate().unwrap();
        assert!(deferred.case.is_none());
        assert!(deferred.runs.is_empty());
        assert_eq!(deferred.scenario_version, 0);
        assert_eq!(deferred.case_id, "requested-case");
        assert_eq!(deferred.aggregate.planned_runs, 3);
        assert_eq!(deferred.aggregate.observed_runs, 0);
        assert_eq!(deferred.aggregate.deferred_runs, 3);
        assert_eq!(deferred.aggregate.total_tokens_consumed, Some(0));
        assert_eq!(deferred.aggregate.completion_rate, None);
        assert_eq!(deferred.aggregate.tokens_per_completion, None);
        assert_eq!(deferred.aggregate.quality_score_completed, None);
        assert!(!deferred.passed);
        assert_eq!(
            deferred.deferral_reason.as_deref(),
            Some("scenario materialization failed")
        );

        let output = tempfile::tempdir().unwrap();
        let mut report = report(vec![deferred]);
        report.write_to(output.path(), &manifest()).unwrap();
        let (decoded, _) = E2eReport::read_from(output.path()).unwrap();
        assert_eq!(decoded.report_state, ReportState::Partial);
        assert_eq!(decoded.objective_outcome, ObjectiveOutcome::Inconclusive);
        assert!(decoded.scenarios[0].case.is_none());
    }

    #[test]
    fn missing_case_requires_wholly_deferred_slots_and_reason() {
        let policy = aggregate(Vec::new()).execution_policy;
        for (planned, reason) in [(0, "failure"), (2, ""), (2, "   ")] {
            let mut deferred = E2eScenarioReport::deferred(
                "unmaterialized".into(),
                "requested-case".into(),
                0,
                policy,
                planned,
                reason.into(),
            );
            assert!(deferred.refresh_aggregate().is_err());
        }
        let mut deferred = E2eScenarioReport::deferred(
            "unmaterialized".into(),
            "requested-case".into(),
            0,
            policy,
            2,
            "materialization failed".into(),
        );
        deferred.runs.push(run(90, true));
        assert!(deferred.refresh_aggregate().is_err());
        deferred.runs.clear();
        deferred.deferral_reason = None;
        assert!(deferred.refresh_aggregate().is_err());
    }

    #[test]
    fn deferred_aggregate_rejects_invented_measurements() {
        let deferred = E2eScenarioReport::deferred(
            "unmaterialized".into(),
            "requested-case".into(),
            0,
            aggregate(Vec::new()).execution_policy,
            2,
            "materialization failed".into(),
        );
        let output = tempfile::tempdir().unwrap();
        let mut report = report(vec![deferred]);
        report.scenarios[0].aggregate.total_tokens_consumed = Some(100);
        let error = report.write_to(output.path(), &manifest()).unwrap_err();
        assert!(format!("{error:#}").contains("aggregate differs from its run evidence"));
    }

    #[test]
    fn persistence_error_preserves_completed_runs_but_prevents_complete_pass() {
        let output = tempfile::tempdir().unwrap();
        let mut report = report(vec![aggregate(vec![run(90, true)])]);
        report.slot_start_deadline_seconds = Some(3600);
        report.record_persistence_error("journal append failed".into());
        assert_eq!(report.report_state, ReportState::Partial);
        assert_eq!(report.objective_outcome, ObjectiveOutcome::Inconclusive);
        assert!(!report.passed);
        assert_eq!(report.scenarios[0].aggregate.completed_runs, 1);
        assert_eq!(report.scenarios[0].aggregate.deferred_runs, 0);
        assert_eq!(
            report.scenarios[0].aggregate.objective_median_score,
            Some(90.0)
        );
        report.write_to(output.path(), &manifest()).unwrap();
        let (decoded, _) = E2eReport::read_from(output.path()).unwrap();
        assert_eq!(decoded.persistence_errors, vec!["journal append failed"]);
        assert_eq!(decoded.slot_start_deadline_seconds, Some(3600));
        assert_eq!(
            decoded.scenarios[0].runs[0].technical,
            TechnicalState::Valid
        );
    }

    #[test]
    fn persistence_and_deferral_errors_are_redacted_before_persistence() {
        let secret = "Bearer test-secret-material-for-redaction";
        let deferred = E2eScenarioReport::deferred(
            "unmaterialized".into(),
            "requested-case".into(),
            0,
            aggregate(Vec::new()).execution_policy,
            2,
            format!("materialization failed: {secret}"),
        );
        let output = tempfile::tempdir().unwrap();
        let mut report = report(vec![deferred]);
        report.record_persistence_error(format!("journal append failed: {secret}"));
        report.write_to(output.path(), &manifest()).unwrap();
        let bytes = std::fs::read_to_string(output.path().join("results.json")).unwrap();
        assert!(!bytes.contains("test-secret-material-for-redaction"));
        assert!(report.redaction.redacted_values >= 2);
    }

    #[test]
    fn hard_gate_failures_count_as_scored_failed_runs() {
        let mut outvoted = run(45, false);
        outvoted.status = RunStatus::HardGateFailed;
        let report = aggregate(vec![outvoted, run(90, true), run(90, true)]);
        assert!(report.passed);
        assert_eq!(report.aggregate.hard_gate_failures, 1);
        assert_eq!(report.aggregate.median_score, Some(90.0));

        let mut decisive = run(45, false);
        decisive.status = RunStatus::HardGateFailed;
        let report = aggregate(vec![decisive, run(90, true)]);
        assert!(!report.passed);
        assert_eq!(report.aggregate.median_score, Some(45.0));
    }

    #[test]
    fn report_contains_current_execution_shape() {
        let report = report(vec![aggregate(vec![run(90, true)])]);
        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["scenarios"][0]["aggregate"]["median_score"], 90.0);
        assert!(value["scenarios"][0].get("threshold").is_none());
        assert_eq!(value["scenarios"][0]["runs"][0]["status"], "passed");
        assert!(value["scenarios"][0]["aggregate"]["cost"].is_object());
    }

    #[test]
    fn retry_attempts_preserve_failures_time_and_cost() {
        let mut failed = E2eRunReport::new(
            "run".into(),
            "retry".into(),
            1,
            "retry-session".into(),
            "prompt".into(),
        );
        failed.wall_time_ms = 2_000;
        failed.cost = CostReport {
            subject_usd: Some(0.10),
            judge_usd: Some(0.0),
            total_usd: Some(0.10),
        };
        failed.push_failure(
            RunStatus::SubjectError,
            FailurePhase::Execute,
            "stream ended without a terminal frame",
        );

        let mut passed = run(100, true);
        passed.attempt_id = "final".into();
        passed.attempt_number = 2;
        passed.wall_time_ms = 3_000;
        passed.cost = CostReport {
            subject_usd: Some(0.20),
            judge_usd: Some(0.0),
            total_usd: Some(0.20),
        };
        passed.attach_retry_attempts(vec![RetryAttemptReport::from(&failed)]);

        assert_eq!(passed.wall_time_ms, 5_000);
        assert_eq!(passed.retry_attempts.len(), 1);
        assert!((passed.cost.total_usd.unwrap() - 0.30).abs() < f64::EPSILON);
    }

    #[test]
    fn efficiency_uses_root_child_calls_and_validation_work() {
        let mut attempt = run(100, true);
        attempt.wall_time_ms = 1_500;
        attempt.cost.total_usd = Some(0.25);
        attempt.metrics = Some(metrics());
        attempt.terminal_status = Some(status(2, 1));
        attempt.transcript = Some(serde_json::json!({"messages": [{
            "message": {
                "role": "assistant",
                "content": [
                    {
                        "type": "function_call",
                        "function_id": "harness::spawn",
                        "arguments": {"session_id": "child-a"}
                    },
                    {
                        "type": "function_call",
                        "function_id": "agent_trigger",
                        "arguments": {
                            "function": "harness::spawn",
                            "payload": {"session_id": "child-b"}
                        }
                    }
                ]
            }
        }]}));

        attempt.update_efficiency(WorkExpectation {
            minimum_expected_work: 10,
        });

        let efficiency = attempt.efficiency.unwrap();
        assert_eq!(efficiency.root_turns, Some(3));
        assert_eq!(efficiency.child_turns, Some(4));
        assert_eq!(efficiency.child_sessions, Some(2));
        assert_eq!(efficiency.function_calls, Some(8));
        assert_eq!(efficiency.validation_retries, Some(2));
        assert_eq!(efficiency.transient_resumes, Some(1));
        assert_eq!(efficiency.wake_resumes, Some(1));
        assert_eq!(efficiency.effective_fan_out, Some(2));
        assert_eq!(efficiency.observed_work, Some(17));
        assert!((efficiency.work_amplification.unwrap() - 1.7).abs() < f64::EPSILON);
        assert_eq!(efficiency.total_tokens, Some(1_500));
        assert!(efficiency.unavailable.contains_key("critical_path_ms"));
    }

    #[test]
    fn retry_efficiency_and_cost_include_every_technical_attempt() {
        let work = WorkExpectation {
            minimum_expected_work: 10,
        };
        let mut failed = run(0, false);
        failed.status = RunStatus::SubjectError;
        failed.wall_time_ms = 500;
        failed.cost.total_usd = Some(0.10);
        failed.metrics = Some(metrics());
        failed.terminal_status = Some(status(1, 0));
        failed.update_efficiency(work);

        let mut passed = run(100, true);
        passed.wall_time_ms = 1_000;
        passed.cost.total_usd = Some(0.20);
        passed.metrics = Some(metrics());
        passed.terminal_status = Some(status(2, 1));
        passed.update_efficiency(work);
        passed.attach_retry_attempts(vec![RetryAttemptReport::from(&failed)]);

        let aggregate = aggregate(vec![passed.clone()]);
        let efficiency = passed.efficiency.unwrap();
        assert_eq!(efficiency.technical_attempts, 2);
        assert_eq!(efficiency.total_tokens, Some(3_000));
        assert_eq!(efficiency.observed_work, Some(33));
        assert!((efficiency.work_amplification.unwrap() - 3.3).abs() < f64::EPSILON);
        assert!((passed.cost.total_usd.unwrap() - 0.30).abs() < f64::EPSILON);
        assert_eq!(passed.wall_time_ms, 1_500);
        assert_eq!(aggregate.aggregate.total_tokens_consumed, Some(3_000));
        assert_eq!(aggregate.aggregate.tokens_completed_p50, Some(3_000.0));
        assert_eq!(aggregate.aggregate.failed_attempt_tokens, Some(1_500));
        assert_eq!(aggregate.aggregate.tokens_per_completion, Some(3_000.0));
    }

    #[test]
    fn robustness_never_reports_p95_below_the_tail_sample_minimum() {
        let mut nineteen = (1..=19).map(comparable_run).collect::<Vec<E2eRunReport>>();
        let report = aggregate(nineteen.clone());
        assert!(report.aggregate.robustness.eligible);
        assert_eq!(report.aggregate.robustness.p95_wall_time_ms, None);
        assert!(report
            .aggregate
            .robustness
            .unavailable
            .contains_key("p95_wall_time_ms"));

        nineteen.push(comparable_run(20));
        let report = aggregate(nineteen);
        assert_eq!(report.aggregate.robustness.sample_size, 20);
        assert_eq!(report.aggregate.robustness.p95_wall_time_ms, Some(1_900));
        assert_eq!(
            report.aggregate.robustness.deliverable_success_rate,
            Some(1.0)
        );
        assert_eq!(
            report.aggregate.robustness.structural_integrity_rate,
            Some(1.0)
        );
    }

    #[test]
    fn summary_surfaces_the_actionable_failure_details() {
        let mut failed = run(50, false);
        failed.status = RunStatus::HardGateFailed;
        failed.hard_gates.push(HardGateReport {
            id: "durable_effect".into(),
            dimension: EvaluationDimension::StructuralIntegrity,
            passed: false,
            reason: "expected row was missing".into(),
        });
        failed.criteria.push(CriterionReport {
            id: "correctness".into(),
            possible: 100,
            awarded: Some(50),
            reason: "only half of the expected result was present".into(),
        });
        let report = report(vec![aggregate(vec![failed])]);

        let summary = report.summary(false);
        assert!(summary.contains("Harness E2E: FAIL"));
        assert!(summary.contains("gate durable_effect: FAIL - expected row was missing"));
        assert!(summary.contains("criterion correctness: 50/100"));
    }

    #[test]
    fn an_empty_report_does_not_pass() {
        let report = report(vec![]);
        assert!(!report.passed);
    }

    #[test]
    fn observation_contract_is_persisted_in_report_and_manifest() {
        let output = tempfile::tempdir().unwrap();
        let contract = observation_contract();
        let mut manifest = manifest();
        manifest.observation_contract = Some(contract.clone());
        let mut report = report(vec![aggregate(vec![run(100, true)])]);
        report.observation_contract = Some(contract.clone());
        report.write_to(output.path(), &manifest).unwrap();

        let (decoded, _) = E2eReport::read_from(output.path()).unwrap();
        assert_eq!(decoded.observation_contract, Some(contract));
    }

    #[test]
    fn observation_records_requested_and_observed_versions_independently() {
        let output = tempfile::tempdir().unwrap();
        let mut contract = observation_contract();
        contract.target.version = "other".into();
        let mut manifest = manifest();
        manifest.observation_contract = Some(contract.clone());
        let mut report = report(vec![aggregate(vec![run(100, true)])]);
        report.observation_contract = Some(contract);

        report.write_to(output.path(), &manifest).unwrap();

        let (decoded, _) = E2eReport::read_from(output.path()).unwrap();
        assert_eq!(
            decoded.observation_contract.unwrap().target.version,
            "other"
        );
        assert_eq!(decoded.system_under_test.harness_version, "1.8.0");
    }

    #[test]
    fn write_materializes_the_single_result_and_hashed_evidence() {
        let output = tempfile::tempdir().unwrap();
        let mut run = run(100, true);
        run.transcript = Some(serde_json::json!({"messages": ["done"]}));
        run.assessment_results.push(AssessmentResult {
            criterion_id: "correctness".into(),
            target: AssessmentTarget {
                kind: AssessmentTargetKind::Criterion,
                id: "correctness".into(),
            },
            kind: AssessmentKind::RequiredCheck,
            policy: AssessmentPolicy::HardGate,
            dimension: EvaluationDimension::StructuralIntegrity,
            source: AssessmentSource::Deterministic,
            outcome: AssessmentOutcome::Passed,
            score: Some(AssessmentScore {
                awarded: 100,
                possible: 100,
            }),
            confidence: None,
            summary: "deterministic evidence passed".into(),
            evidence: Vec::new(),
            analyzer: None,
            analyzer_usage: None,
        });
        let mut report = report(vec![aggregate(vec![run])]);

        let path = report.write_to(output.path(), &manifest()).unwrap();
        assert_eq!(path, output.path().join("results.json"));
        assert!(!output.path().join("results-v2.json").exists());
        assert!(!output.path().join("results-v3.json").exists());
        assert!(output.path().join("manifest.json").is_file());
        let evidence = &report.scenarios[0].runs[0].evidence;
        assert_eq!(evidence.len(), 1);
        assert!(output.path().join(&evidence[0].path).is_file());
        assert!(evidence[0].sha256.starts_with("sha256:"));
        let assessment = &report.scenarios[0].runs[0].assessment_results[0];
        assert_eq!(assessment.evidence.len(), 1);
        assert_eq!(assessment.evidence[0].artifact_id, "transcript");
        assert_eq!(assessment.evidence[0].artifact_sha256, evidence[0].sha256);
        let contract = AssessmentContract::from_assessment_evidence(&report);
        assert_eq!(contract.runs[0].assessments, vec![assessment.clone()]);
        let (decoded, _) = E2eReport::read_from(output.path()).unwrap();
        decoded.assessment_contract.validate(&decoded).unwrap();
        let value: Value =
            serde_json::from_slice(&std::fs::read(output.path().join("results.json")).unwrap())
                .unwrap();
        assert_eq!(
            value.get("schema_version"),
            Some(&serde_json::json!(RESULTS_SCHEMA_VERSION))
        );
        assert!(value.get("assessment_contract").is_some());
        assert!(value["scenarios"][0]["runs"][0].get("attempt_id").is_some());
    }

    #[test]
    fn write_preserves_verified_markdown_phase_evidence() {
        let output = tempfile::tempdir().unwrap();
        let mut run = run(100, true);
        run.markdown_execution = Some(MarkdownExecutionReport {
            source_path: "case.md".into(),
            source_sha256: format!("sha256:{}", "a".repeat(64)),
            behavior_sha256: format!("sha256:{}", "b".repeat(64)),
            compiled_sha256: format!("sha256:{}", "c".repeat(64)),
            materialized_plan_sha256: Some(format!("sha256:{}", "d".repeat(64))),
            prompt_sha256: format!("sha256:{}", "e".repeat(64)),
            pipeline_complete: true,
            phases: Vec::new(),
        });
        let phase = artifact::write_json(
            output.path(),
            Path::new("evidence/run/attempt/setup.json"),
            "attempt-setup",
            "markdown-phase-evidence",
            &serde_json::json!({"session_id": "setup"}),
        )
        .unwrap();
        run.evidence.push(phase.clone());
        let mut report = report(vec![aggregate(vec![run])]);

        report.write_to(output.path(), &manifest()).unwrap();
        report.write_to(output.path(), &manifest()).unwrap();

        let evidence = &report.scenarios[0].runs[0].evidence;
        assert!(evidence.iter().any(|reference| reference == &phase));
        let (decoded, _) = E2eReport::read_from(output.path()).unwrap();
        assert!(decoded.scenarios[0].runs[0]
            .evidence
            .iter()
            .any(|reference| reference == &phase));
    }

    #[test]
    fn assessment_status_is_derived_from_the_run() {
        let output = tempfile::tempdir().unwrap();
        let mut report = report(vec![aggregate(vec![run(0, false)])]);
        report.write_to(output.path(), &manifest()).unwrap();

        let (decoded, _) = E2eReport::read_from(output.path()).unwrap();
        assert_eq!(
            decoded.assessment_contract.runs[0].system_status,
            crate::assessment::SystemStatus::HardGateFailed
        );
        assert_eq!(
            decoded.assessment_contract.runs[0].effective_status,
            crate::assessment::EffectiveStatus::HardGateFailed
        );
    }

    #[test]
    fn read_rejects_legacy_and_unknown_versions() {
        let output = tempfile::tempdir().unwrap();
        let mut report = report(vec![aggregate(vec![run(100, true)])]);
        let path = report.write_to(output.path(), &manifest()).unwrap();
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["schema_version"] = serde_json::json!(2);
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        let error = E2eReport::read_from(&path).unwrap_err();
        assert!(format!("{error:#}").contains("unsupported results schema_version 2"));

        value["schema_version"] = serde_json::json!(RESULTS_SCHEMA_VERSION + 1);
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let error = E2eReport::read_from(&path).unwrap_err();
        assert!(format!("{error:#}").contains(&format!(
            "unsupported results schema_version {}",
            RESULTS_SCHEMA_VERSION + 1
        )));
    }

    #[test]
    fn native_workflow_gates_and_evaluations_are_aggregated_into_the_assessment_contract() {
        let step = WorkflowStepReport {
            node_id: "assess".into(),
            step_type: "test.assess".into(),
            step_version: 1,
            required: true,
            dependencies: Vec::new(),
            dependency_policy: DependencyPolicy::Succeeded,
            activation: ActivationPolicy::Always,
            status: WorkflowStepStatus::HardGateFailed,
            started_at: Some("2026-08-12T12:00:00Z".into()),
            completed_at: Some("2026-08-12T12:00:01Z".into()),
            duration_ms: 1_000,
            harness_session_id: None,
            outputs: BTreeMap::new(),
            transcript: None,
            metrics: None,
            cost_usd: None,
            assets: Vec::new(),
            hard_gates: vec![WorkflowGateResult {
                id: "valid".into(),
                passed: false,
                reason: "deterministic validation failed".into(),
                evidence_ids: Vec::new(),
            }],
            evaluations: vec![WorkflowEvaluationResult {
                id: "quality".into(),
                outcome: WorkflowEvaluationOutcome::Advisory,
                summary: "half of the advisory signals matched".into(),
                score: Some(0.5),
                evidence_ids: Vec::new(),
            }],
            failures: Vec::new(),
            skip_reason: None,
            redaction: crate::redaction::RedactionReport::default(),
        };
        let mut attempt = run(50, false);
        attempt.semantic_tests = vec![step];
        attempt.assessment_results =
            crate::assessment::semantic_test_assessments(&attempt.semantic_tests, &[]);
        let report = report(vec![aggregate(vec![attempt])]);

        let contract = AssessmentContract::from_assessment_evidence(&report);
        contract.validate(&report).unwrap();
        assert_eq!(contract.runs.len(), 1);
        assert_eq!(contract.runs[0].system_status, SystemStatus::HardGateFailed);
        assert_eq!(contract.runs[0].assessments.len(), 2);
        assert_eq!(
            contract.runs[0].assessments[1].outcome,
            AssessmentOutcome::Partial
        );
    }

    #[test]
    fn deliverable_survives_cleanup_failure_as_a_verified_artifact() {
        let output = tempfile::tempdir().unwrap();
        let contract = DeliverableContract {
            artifacts: vec![ArtifactExpectation {
                id: "result".into(),
                kind: "state_value".into(),
                media_type: "application/json".into(),
                schema: serde_json::json!({
                    "type": "object",
                    "required": ["status"],
                    "properties": { "status": { "const": "ready" } },
                    "additionalProperties": false
                }),
                max_size_bytes: 1_024,
            }],
            invariants: vec![InvariantSpec {
                id: "ready".into(),
                description: "The materialized value is ready.".into(),
            }],
            provenance_required: true,
            capture_before_cleanup: true,
        };
        let case = ScenarioCase::new(
            "case",
            2,
            7,
            serde_json::json!({ "status": "ready" }),
            ComplexityProfile {
                external_systems: 1,
                state_transitions: 1,
                artifact_count: 1,
                ..ComplexityProfile::default()
            },
            vec!["iii::state".into()],
            contract,
        )
        .unwrap();
        let captured = CapturedDeliverable {
            id: "result".into(),
            kind: "state_value".into(),
            content: serde_json::json!({ "status": "ready" }).into(),
            invariants: vec![CapturedInvariant {
                id: "ready".into(),
                passed: true,
                reason: "status is ready".into(),
            }],
            provenance: vec![ProvenanceEvidence {
                kind: "function_call".into(),
                source_id: "call-1".into(),
                relation: "created".into(),
            }],
        };
        let mut attempt = run(100, true);
        attempt.deliverables = evaluate_deliverables(&case, vec![captured]).unwrap();
        attempt.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Cleanup,
            "teardown unavailable",
        );
        attempt.refresh_dimensions(true);
        let mut report = report(vec![E2eScenarioReport::aggregate_case(
            case,
            ExecutionPolicy {
                max_turns: 1,
                max_output_tokens: Some(1),
                max_total_tokens: Some(1),
                stuck_timeout_seconds: 1,
                max_validation_retries: None,
            },
            vec![attempt],
        )]);

        report.write_to(output.path(), &manifest()).unwrap();

        let deliverable = &report.scenarios[0].runs[0].deliverables[0];
        let reference = deliverable.artifact.as_ref().unwrap();
        assert!(output.path().join(&reference.path).is_file());
        assert!(deliverable.passed());
        let (decoded, _) = E2eReport::read_from(output.path()).unwrap();
        assert_eq!(
            decoded.scenarios[0].runs[0].failures[0].phase,
            FailurePhase::Cleanup
        );
        assert!(decoded.scenarios[0].runs[0].deliverables[0]
            .artifact
            .is_some());
        let persisted: Value =
            serde_json::from_slice(&std::fs::read(output.path().join("results.json")).unwrap())
                .unwrap();
        assert!(persisted["scenarios"][0]["runs"][0]
            .get("deliverables")
            .is_some());
    }

    #[test]
    fn deterministic_asset_evidence_survives_the_complete_cleanup_lifecycle() {
        let output = tempfile::tempdir().unwrap();
        let contract = DeliverableContract {
            artifacts: vec![ArtifactExpectation {
                id: "result".into(),
                kind: "state_value".into(),
                media_type: "application/json".into(),
                schema: serde_json::json!({
                    "type": "object",
                    "required": ["status"],
                    "properties": { "status": { "const": "ready" } },
                    "additionalProperties": false
                }),
                max_size_bytes: 1_024,
            }],
            invariants: vec![InvariantSpec {
                id: "ready".into(),
                description: "The materialized value is ready.".into(),
            }],
            provenance_required: true,
            capture_before_cleanup: true,
        };
        let case = ScenarioCase::new(
            "case",
            2,
            7,
            serde_json::json!({ "status": "ready" }),
            ComplexityProfile::default(),
            vec![],
            contract,
        )
        .unwrap();
        let captured = CapturedDeliverable {
            id: "result".into(),
            kind: "state_value".into(),
            content: serde_json::json!({ "status": "ready" }).into(),
            invariants: vec![CapturedInvariant {
                id: "ready".into(),
                passed: true,
                reason: "status is ready".into(),
            }],
            provenance: vec![ProvenanceEvidence {
                kind: "function_call".into(),
                source_id: "call-1".into(),
                relation: "created".into(),
            }],
        };
        let mut evaluation = crate::asset::evaluate_assets(
            &case,
            vec![captured],
            crate::asset::AssetCaptureLimits::default(),
        )
        .unwrap();
        let capture_manifest =
            crate::asset::persist_before_cleanup(output.path(), "run", "attempt", &mut evaluation)
                .unwrap();
        let captured_path = evaluation.deliverables[0]
            .artifact
            .as_ref()
            .unwrap()
            .path
            .clone();
        std::fs::remove_file(output.path().join(&captured_path)).unwrap();
        crate::asset::reconcile_after_cleanup(
            output.path(),
            &evaluation.deliverables,
            &mut evaluation.assessments,
        );
        let reconciliation_manifest = crate::asset::persist_after_cleanup(
            output.path(),
            &capture_manifest,
            &evaluation.assessments,
        )
        .unwrap();

        let mut attempt = run(0, false);
        attempt.deliverables = evaluation.deliverables;
        attempt.asset_assessments = evaluation.assessments;
        attempt.asset_capture_manifest = Some(reconciliation_manifest.clone());
        attempt.evidence.push(reconciliation_manifest.clone());
        attempt.asset_redaction = evaluation.redaction;
        let mut report = report(vec![E2eScenarioReport::aggregate_case(
            case,
            ExecutionPolicy {
                max_turns: 1,
                max_output_tokens: Some(1),
                max_total_tokens: Some(1),
                stuck_timeout_seconds: 1,
                max_validation_retries: None,
            },
            vec![attempt],
        )]);
        report.assessment_contract = AssessmentContract::from_assessment_evidence(&report);
        report.write_to(output.path(), &manifest()).unwrap();

        assert!(output.path().join(captured_path).is_file());
        let (decoded, _) = E2eReport::read_from(output.path()).unwrap();
        let asset = &decoded.assessment_contract.runs[0].assets[0];
        assert_eq!(
            asset.validation.outcome,
            crate::assessment::AssetValidationOutcome::RemovedDuringCleanup
        );
        assert_eq!(asset.validation.evidence.len(), 1);
        assert!(decoded.scenarios[0].runs[0]
            .evidence
            .iter()
            .any(|reference| reference.kind == "asset_capture_manifest"));
        let reconciliation: crate::asset::AssetCaptureManifest = serde_json::from_slice(
            &std::fs::read(output.path().join(reconciliation_manifest.path)).unwrap(),
        )
        .unwrap();
        assert!(reconciliation.reconciled_after_cleanup);
        assert_eq!(
            reconciliation.assets[0].validation.outcome,
            crate::assessment::AssetValidationOutcome::RemovedDuringCleanup
        );
        reconciliation
            .prior_capture
            .unwrap()
            .verify(output.path())
            .unwrap();
    }

    #[test]
    fn invalid_deliverable_content_is_quality_evidence_not_a_contract_error() {
        let case = ScenarioCase::new(
            "case",
            1,
            1,
            serde_json::json!({}),
            ComplexityProfile::default(),
            vec![],
            DeliverableContract {
                artifacts: vec![ArtifactExpectation {
                    id: "result".into(),
                    kind: "json".into(),
                    media_type: "application/json".into(),
                    schema: serde_json::json!({ "type": "object" }),
                    max_size_bytes: 1_024,
                }],
                invariants: vec![InvariantSpec {
                    id: "correct".into(),
                    description: "Content is correct.".into(),
                }],
                provenance_required: false,
                capture_before_cleanup: true,
            },
        )
        .unwrap();
        let reports = evaluate_deliverables(
            &case,
            vec![CapturedDeliverable {
                id: "result".into(),
                kind: "json".into(),
                content: serde_json::json!("wrong shape").into(),
                invariants: vec![CapturedInvariant {
                    id: "correct".into(),
                    passed: false,
                    reason: "wrong shape".into(),
                }],
                provenance: vec![],
            }],
        )
        .unwrap();

        assert!(!reports[0].schema_valid);
        assert!(!reports[0].passed());
    }

    fn model() -> ModelArtifact {
        ModelArtifact {
            model: "model".into(),
            provider: "provider".into(),
            context_window: 10_000,
            max_output_tokens: 2_000,
            supports_tools: Some(true),
            supports_vision: Some(false),
        }
    }

    fn metrics() -> SessionMetricsResponse {
        serde_json::from_value(serde_json::json!({
            "root_session_id": "root",
            "complete": true,
            "totals": {
                "sessions": 3,
                "turns": 7,
                "function_calls": 8,
                "function_call_errors": 0,
                "wake_resumes": 1,
                "input_tokens": 1000,
                "output_tokens": 500,
                "cost_usd": 0.25
            },
            "by_session": [
                {
                    "session_id": "root",
                    "depth": 0,
                    "turns": 3,
                    "function_calls": 4,
                    "function_call_errors": 0
                },
                {
                    "session_id": "child-a",
                    "parent_session_id": "root",
                    "depth": 1,
                    "turns": 2,
                    "function_calls": 2,
                    "function_call_errors": 0
                },
                {
                    "session_id": "child-b",
                    "parent_session_id": "root",
                    "depth": 1,
                    "turns": 2,
                    "function_calls": 2,
                    "function_call_errors": 0
                }
            ]
        }))
        .unwrap()
    }

    fn status(validation_retries: u32, transient_resumes: u32) -> StatusReport {
        serde_json::from_value(serde_json::json!({
            "session_id": "root",
            "turn_id": "turn",
            "status": "completed",
            "step": 1,
            "turn_count": 3,
            "max_turns": 8,
            "validation_retries": validation_retries,
            "transient_resumes": transient_resumes
        }))
        .unwrap()
    }

    fn comparable_run(index: u64) -> E2eRunReport {
        let mut report = run(100, true);
        report.wall_time_ms = index * 100;
        report.cost.total_usd = Some(0.01);
        report.dimensions = vec![
            DimensionReport {
                dimension: EvaluationDimension::Deliverable,
                passed: Some(true),
                signals: serde_json::json!({}),
            },
            DimensionReport {
                dimension: EvaluationDimension::StructuralIntegrity,
                passed: Some(true),
                signals: serde_json::json!({}),
            },
        ];
        report
    }
}
