use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::artifact::{self, ArtifactReference};
use crate::assessment::{AssessmentContract, AssetAssessmentResult};
use crate::identity::{ExecutionIdentity, SystemUnderTestIdentity};
use crate::scenarios::{
    CapturedDeliverable, CapturedInvariant, ExecutionPolicy, ProvenanceEvidence, ScenarioCase,
    WorkExpectation,
};
#[cfg(test)]
use crate::scenarios::{ComplexityProfile, DeliverableContract};
use crate::schema;
use crate::wire::{ControlPlaneEvidence, Model, SessionMetricsResponseV1, StatusReport};

mod summary;

pub const RESULTS_SCHEMA_VERSION: u32 = 2;
pub const LATEST_RESULTS_SCHEMA_VERSION: u32 = 3;
pub const MANIFEST_SCHEMA_VERSION: u32 = 2;

fn legacy_schema_version() -> u32 {
    1
}

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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FailureRecord {
    #[serde(default = "legacy_failure_domain")]
    pub domain: FailureDomain,
    pub phase: FailurePhase,
    pub message: String,
}

fn legacy_failure_domain() -> FailureDomain {
    FailureDomain::E2eInfrastructure
}

fn default_attempt_number() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HardGateReport {
    pub id: String,
    #[serde(default = "default_structural_dimension")]
    pub dimension: EvaluationDimension,
    pub passed: bool,
    pub reason: String,
}

fn default_structural_dimension() -> EvaluationDimension {
    EvaluationDimension::StructuralIntegrity
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
    pub content: Value,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ObservedComplexityReport {
    pub policy_version: u32,
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
    pub policy_version: u32,
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RetryAttemptReport {
    pub run_id: String,
    #[serde(default)]
    pub attempt_id: String,
    #[serde(default = "default_attempt_number")]
    pub attempt_number: u32,
    pub session_id: String,
    pub wall_time_ms: u64,
    pub status: RunStatus,
    pub cost: CostReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<SessionMetricsResponseV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<ArtifactReference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deliverables: Vec<DeliverableReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dimensions: Vec<DimensionReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub efficiency: Option<EfficiencyReport>,
    pub failures: Vec<FailureRecord>,
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
            cost: report.cost.clone(),
            transcript: report.transcript.clone(),
            metrics: report.metrics.clone(),
            evidence: report.evidence.clone(),
            deliverables: report.deliverables.clone(),
            dimensions: report.dimensions.clone(),
            efficiency: report.efficiency.clone(),
            failures: report.failures.clone(),
            asset_assessments: report.asset_assessments.clone(),
            asset_capture_manifest: report.asset_capture_manifest.clone(),
            asset_redaction: report.asset_redaction.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct E2eRunReport {
    pub run_id: String,
    #[serde(default)]
    pub attempt_id: String,
    #[serde(default = "default_attempt_number")]
    pub attempt_number: u32,
    pub session_id: String,
    pub prompt: String,
    pub wall_time_ms: u64,
    pub score: Option<u8>,
    pub status: RunStatus,
    pub hard_gates: Vec<HardGateReport>,
    pub criteria: Vec<CriterionReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<SessionMetricsResponseV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_attempts: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_usage: Option<ModelUsageReport>,
    pub cost: CostReport,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<ArtifactReference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deliverables: Vec<DeliverableReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dimensions: Vec<DimensionReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub efficiency: Option<EfficiencyReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retry_attempts: Vec<RetryAttemptReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<FailureRecord>,
    #[serde(skip)]
    #[schemars(skip)]
    pub terminal_status: Option<StatusReport>,
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
            status: RunStatus::InfrastructureError,
            hard_gates: Vec::new(),
            criteria: Vec::new(),
            transcript: None,
            metrics: None,
            judge_attempts: None,
            judge_usage: None,
            cost: CostReport::default(),
            evidence: Vec::new(),
            deliverables: Vec::new(),
            dimensions: Vec::new(),
            efficiency: None,
            retry_attempts: Vec::new(),
            failures: Vec::new(),
            terminal_status: None,
            asset_assessments: Vec::new(),
            asset_capture_manifest: None,
            asset_redaction: crate::redaction::RedactionReport::default(),
        }
    }

    pub fn push_failure(
        &mut self,
        status: RunStatus,
        phase: FailurePhase,
        message: impl Into<String>,
    ) {
        let is_primary = self.failures.is_empty();
        self.failures.push(FailureRecord {
            domain: status.failure_domain(),
            phase,
            message: message.into(),
        });
        if is_primary {
            self.status = status;
        }
    }

    pub fn finish(&mut self, status: RunStatus) {
        self.status = status;
    }

    pub fn update_cost(&mut self, judge_expected: bool) {
        let subject_usd = self
            .metrics
            .as_ref()
            .and_then(|metrics| metrics.totals.cost_usd);
        let judge_skipped = !judge_expected || self.status == RunStatus::HardGateFailed;
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
                policy_version: work.policy_version,
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
            policy_version: work.policy_version,
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
        policy_version: 1,
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
    metrics: &SessionMetricsResponseV1,
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
        policy_version: 1,
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
    pub runs: u32,
    pub scored_runs: u32,
    pub passed_runs: u32,
    pub required_passes: u32,
    pub pass_rate: f64,
    pub median_score: Option<f64>,
    pub hard_gate_failures: u32,
    pub technical_failures: u32,
    pub cost: CostReport,
    pub robustness: RobustnessReport,
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
        let scenario_id = case.scenario_id.clone();
        let scenario_version = case.scenario_version;
        let run_count = runs.len() as u32;
        let scored_runs = runs.iter().filter(|run| run.score.is_some()).count() as u32;
        let passed_runs = runs
            .iter()
            .filter(|run| run.status == RunStatus::Passed)
            .count() as u32;
        let hard_gate_failures = runs
            .iter()
            .filter(|run| run.status == RunStatus::HardGateFailed)
            .count() as u32;
        let technical_failures = runs
            .iter()
            .filter(|run| run.status.is_technical_failure())
            .count() as u32;
        let required_passes = required_passes(run_count);
        let median_score = median(runs.iter().filter_map(|run| run.score));
        let cost = CostReport {
            subject_usd: sum_cost(runs.iter().map(|run| run.cost.subject_usd)),
            judge_usd: sum_cost(runs.iter().map(|run| run.cost.judge_usd)),
            total_usd: sum_cost(runs.iter().map(|run| run.cost.total_usd)),
        };
        let robustness = robustness_report(&runs);
        let passed = run_count > 0 && technical_failures == 0 && passed_runs >= required_passes;
        Self {
            case_id: case.case_id.clone(),
            scenario_id,
            scenario_version,
            case: Some(case),
            execution_policy,
            aggregate: ScenarioAggregate {
                runs: run_count,
                scored_runs,
                passed_runs,
                required_passes,
                pass_rate: if run_count == 0 {
                    0.0
                } else {
                    f64::from(passed_runs) / f64::from(run_count)
                },
                median_score,
                hard_gate_failures,
                technical_failures,
                cost,
                robustness,
            },
            passed,
            runs,
        }
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
pub struct E2eManifestV2 {
    pub schema_version: u32,
    pub execution: ExecutionIdentity,
    pub system_under_test: SystemUnderTestIdentity,
    pub subject: ModelArtifact,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge: Option<ModelArtifact>,
    pub control_plane: ControlPlaneEvidence,
}

impl E2eManifestV2 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != MANIFEST_SCHEMA_VERSION {
            bail!(
                "manifest schema version {} is unsupported; expected {}",
                self.schema_version,
                MANIFEST_SCHEMA_VERSION
            );
        }
        self.execution.validate()?;
        self.system_under_test.validate()?;
        if self.control_plane.functions.is_empty() {
            bail!("manifest control plane cannot be empty");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct E2eReport {
    #[serde(default = "legacy_schema_version")]
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_under_test: Option<SystemUnderTestIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<ArtifactReference>,
    pub subject: ModelArtifact,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge: Option<ModelArtifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine_revision: Option<String>,
    pub passed: bool,
    #[serde(default)]
    pub redaction: crate::redaction::RedactionReport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment_contract: Option<AssessmentContract>,
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
        Self {
            schema_version: RESULTS_SCHEMA_VERSION,
            execution: Some(execution),
            system_under_test: Some(system_under_test),
            manifest: None,
            subject,
            judge,
            judge_protocol,
            engine_revision,
            passed,
            redaction: crate::redaction::RedactionReport::default(),
            assessment_contract: None,
            scenarios,
        }
    }

    pub fn write_to(&mut self, output: &Path, manifest: &E2eManifestV2) -> Result<PathBuf> {
        fs::create_dir_all(output)
            .with_context(|| format!("create report directory {}", output.display()))?;
        for name in ["results-v3.json", "results-v2.json", "results.json"] {
            let path = output.join(name);
            if path.is_file() {
                fs::remove_file(&path)
                    .with_context(|| format!("remove stale result {}", path.display()))?;
            }
        }
        manifest.validate().context("validate E2E manifest")?;
        self.redact_sensitive_evidence()?;
        self.materialize_evidence(output)?;
        let manifest_reference = artifact::write_json(
            output,
            Path::new("manifest.json"),
            "execution_manifest",
            "manifest",
            manifest,
        )?;
        self.manifest = Some(manifest_reference);
        validate_against_schema(&schema::manifest_v2(), manifest, "manifest v2")?;
        match self.schema_version {
            RESULTS_SCHEMA_VERSION => {
                if self.assessment_contract.is_some() {
                    bail!("results v2 cannot carry the v3 assessment contract");
                }
                self.validate_versioned(manifest, output, RESULTS_SCHEMA_VERSION)
                    .context("validate E2E results v2")?;
                validate_against_schema(&schema::results_v2(), self, "results v2")?;
                write_json_value(&output.join("results.json"), &self.legacy_v1_value()?)?;
                let canonical_path = output.join("results-v2.json");
                write_json_value(&canonical_path, self)?;
                Ok(canonical_path)
            }
            LATEST_RESULTS_SCHEMA_VERSION => {
                let contract = self
                    .assessment_contract
                    .as_ref()
                    .context("results v3 are missing assessment_contract")?;
                contract.validate(self)?;
                self.validate_versioned(manifest, output, LATEST_RESULTS_SCHEMA_VERSION)
                    .context("validate E2E results v3")?;
                validate_against_schema(&schema::results_v3(), self, "results v3")?;
                let legacy_v2 = self.legacy_v2_value()?;
                validate_against_schema(
                    &schema::results_v2(),
                    &legacy_v2,
                    "results v2 projection",
                )?;
                write_json_value(&output.join("results.json"), &self.legacy_v1_value()?)?;
                write_json_value(&output.join("results-v2.json"), &legacy_v2)?;
                let canonical_path = output.join("results-v3.json");
                write_json_value(&canonical_path, self)?;
                Ok(canonical_path)
            }
            version => bail!("cannot write unsupported E2E results schema {version}"),
        }
    }

    pub fn read_from(input: &Path) -> Result<(Self, PathBuf)> {
        let path = if input.is_dir() {
            let latest = input.join("results-v3.json");
            let canonical = input.join("results-v2.json");
            if latest.is_file() {
                latest
            } else if canonical.is_file() {
                canonical
            } else {
                input.join("results.json")
            }
        } else {
            input.to_path_buf()
        };
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let value: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("decode E2E report {}", path.display()))?;
        let report: Self = serde_json::from_value(value.clone())
            .with_context(|| format!("decode typed E2E report {}", path.display()))?;
        if !matches!(
            report.schema_version,
            1 | RESULTS_SCHEMA_VERSION | LATEST_RESULTS_SCHEMA_VERSION
        ) {
            bail!(
                "unsupported E2E results schema {} in {}",
                report.schema_version,
                path.display()
            );
        }
        if matches!(
            report.schema_version,
            RESULTS_SCHEMA_VERSION | LATEST_RESULTS_SCHEMA_VERSION
        ) {
            if report.schema_version == RESULTS_SCHEMA_VERSION {
                if report.assessment_contract.is_some() {
                    bail!("results v2 cannot carry the v3 assessment contract");
                }
                validate_against_schema(&schema::results_v2(), &value, "results v2")?;
            } else {
                validate_against_schema(&schema::results_v3(), &value, "results v3")?;
                report
                    .assessment_contract
                    .as_ref()
                    .context("results v3 are missing assessment_contract")?
                    .validate(&report)?;
            }
            let output = path
                .parent()
                .context("results path has no parent directory")?;
            let manifest_path = output.join("manifest.json");
            let manifest_value: Value = serde_json::from_slice(
                &fs::read(&manifest_path)
                    .with_context(|| format!("read {}", manifest_path.display()))?,
            )
            .with_context(|| format!("decode {}", manifest_path.display()))?;
            validate_against_schema(&schema::manifest_v2(), &manifest_value, "manifest v2")?;
            let manifest: E2eManifestV2 = serde_json::from_value(manifest_value)
                .with_context(|| format!("decode typed manifest {}", manifest_path.display()))?;
            manifest.validate()?;
            report.validate_versioned(&manifest, output, report.schema_version)?;
        }
        Ok((report, path))
    }

    fn validate_versioned(
        &self,
        manifest: &E2eManifestV2,
        output: &Path,
        expected_version: u32,
    ) -> Result<()> {
        if self.schema_version != expected_version {
            bail!(
                "results schema version {} is unsupported; expected {}",
                self.schema_version,
                expected_version
            );
        }
        let execution = self
            .execution
            .as_ref()
            .context("results are missing execution identity")?;
        if execution.execution_id != manifest.execution.execution_id {
            bail!("results and manifest execution identities differ");
        }
        let system = self
            .system_under_test
            .as_ref()
            .context("results are missing system identity")?;
        system.validate()?;
        if serde_json::to_value(system)? != serde_json::to_value(&manifest.system_under_test)? {
            bail!("results and manifest system identities differ");
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
            if scenario.case_id.trim().is_empty() {
                bail!("scenario {} has an empty case_id", scenario.scenario_id);
            }
            let case = scenario
                .case
                .as_ref()
                .context("versioned scenario is missing its materialized case")?;
            case.validate()?;
            if scenario.case_id != case.case_id
                || scenario.scenario_id != case.scenario_id
                || scenario.scenario_version != case.scenario_version
            {
                bail!("scenario identity differs from its materialized case");
            }
            for run in &scenario.runs {
                validate_attempt_identity(run)?;
                for reference in &run.evidence {
                    reference.verify(output)?;
                }
                verify_deliverables(output, &run.deliverables)?;
                for attempt in &run.retry_attempts {
                    validate_retry_identity(run, attempt)?;
                    for reference in &attempt.evidence {
                        reference.verify(output)?;
                    }
                    verify_deliverables(output, &attempt.deliverables)?;
                }
            }
        }
        Ok(())
    }

    fn redact_sensitive_evidence(&mut self) -> Result<()> {
        let policy = crate::redaction::RedactionPolicy::from_environment();
        let mut redaction = self.redaction.clone();
        for scenario in &mut self.scenarios {
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
                scan_deliverable_content(&policy, &run.deliverables)?;
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
                    scan_deliverable_content(&policy, &retry.deliverables)?;
                }
            }
        }
        if let Some(contract) = &mut self.assessment_contract {
            let mut value = serde_json::to_value(&*contract)
                .context("serialize assessment contract before redaction")?;
            redaction.merge(policy.redact_value(&mut value));
            *contract = serde_json::from_value(value)
                .context("decode assessment contract after redaction")?;
        }
        self.redaction = redaction;
        policy.assert_clean(&serde_json::to_vec(&self).context("scan E2E report")?)?;
        Ok(())
    }

    fn legacy_v1_value(&self) -> Result<Value> {
        let mut value = self.legacy_v2_value()?;
        let object = value
            .as_object_mut()
            .context("E2E results must serialize as an object")?;
        for field in [
            "schema_version",
            "execution",
            "system_under_test",
            "manifest",
            "redaction",
        ] {
            object.remove(field);
        }
        let scenarios = object
            .get_mut("scenarios")
            .and_then(Value::as_array_mut)
            .context("E2E results scenarios must be an array")?;
        for scenario in scenarios {
            let scenario = scenario
                .as_object_mut()
                .context("E2E scenario must be an object")?;
            scenario.remove("case_id");
            scenario.remove("case");
            let runs = scenario
                .get_mut("runs")
                .and_then(Value::as_array_mut)
                .context("E2E scenario runs must be an array")?;
            for run in runs {
                remove_v2_attempt_fields(run)?;
                if let Some(retries) = run.get_mut("retry_attempts").and_then(Value::as_array_mut) {
                    for retry in retries {
                        remove_v2_attempt_fields(retry)?;
                        if let Some(retry) = retry.as_object_mut() {
                            retry.remove("transcript");
                            retry.remove("metrics");
                        }
                    }
                }
            }
        }
        Ok(value)
    }

    fn legacy_v2_value(&self) -> Result<Value> {
        let mut value = serde_json::to_value(self).context("serialize results v2 projection")?;
        let object = value
            .as_object_mut()
            .context("E2E results must serialize as an object")?;
        object.insert(
            "schema_version".to_string(),
            Value::from(RESULTS_SCHEMA_VERSION),
        );
        object.remove("assessment_contract");
        Ok(value)
    }

    fn materialize_evidence(&mut self, output: &Path) -> Result<()> {
        for scenario in &mut self.scenarios {
            for run in &mut scenario.runs {
                validate_attempt_identity(run)?;
                for attempt in &run.retry_attempts {
                    validate_retry_identity(run, attempt)?;
                }
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
                        references: &mut run.evidence,
                    },
                )?;
                for attempt in &mut run.retry_attempts {
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
                            references: &mut attempt.evidence,
                        },
                    )?;
                }
            }
        }
        Ok(())
    }
}

fn remove_v2_attempt_fields(value: &mut Value) -> Result<()> {
    let object = value
        .as_object_mut()
        .context("E2E attempt must be an object")?;
    for field in [
        "attempt_id",
        "attempt_number",
        "evidence",
        "deliverables",
        "dimensions",
        "efficiency",
    ] {
        object.remove(field);
    }
    if let Some(failures) = object.get_mut("failures").and_then(Value::as_array_mut) {
        for failure in failures {
            if let Some(failure) = failure.as_object_mut() {
                failure.remove("domain");
            }
        }
    }
    if let Some(gates) = object.get_mut("hard_gates").and_then(Value::as_array_mut) {
        for gate in gates {
            if let Some(gate) = gate.as_object_mut() {
                gate.remove("dimension");
            }
        }
    }
    Ok(())
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

fn scan_deliverable_content(
    policy: &crate::redaction::RedactionPolicy,
    deliverables: &[DeliverableReport],
) -> Result<()> {
    for deliverable in deliverables {
        policy
            .assert_clean(&serde_json::to_vec(&deliverable.content)?)
            .with_context(|| {
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
    metrics: Option<&'a SessionMetricsResponseV1>,
    deliverables: &'a mut [DeliverableReport],
    asset_capture_manifest: Option<&'a ArtifactReference>,
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
    let deliverable_root = PathBuf::from("deliverables").join(run_id).join(attempt_id);
    for deliverable in deliverables {
        let reference = artifact::write_json(
            output,
            &deliverable_root.join(format!("{}.json", deliverable.id)),
            deliverable.id.clone(),
            deliverable.kind.clone(),
            &deliverable.content,
        )?;
        deliverable.artifact = Some(reference);
    }
    Ok(())
}

fn verify_deliverables(output: &Path, deliverables: &[DeliverableReport]) -> Result<()> {
    for deliverable in deliverables {
        let reference = deliverable
            .artifact
            .as_ref()
            .with_context(|| format!("deliverable '{}' is missing its artifact", deliverable.id))?;
        reference.verify(output)?;
        let content: Value = serde_json::from_slice(
            &fs::read(output.join(&reference.path))
                .with_context(|| format!("read deliverable artifact {}", reference.path))?,
        )
        .with_context(|| format!("decode deliverable artifact {}", reference.path))?;
        if artifact::sha256_value(&content)? != deliverable.content_sha256 {
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
        AiFinalAssessment, EffectiveStatus, RunAssessmentContract, SystemStatus,
        ASSESSMENT_CONTRACT_VERSION,
    };
    use crate::identity::StackIdentity;
    use crate::scenarios::{ArtifactExpectation, InvariantSpec};
    use crate::wire::{
        ControlPlaneEvidence, FunctionContractEvidence, SessionMetricsResponseV1, StatusReport,
        CONTROL_PLANE_CONTRACT_NAME, CONTROL_PLANE_CONTRACT_VERSION,
    };

    const TEST_DIGEST: &str =
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

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

    fn manifest() -> E2eManifestV2 {
        E2eManifestV2 {
            schema_version: MANIFEST_SCHEMA_VERSION,
            execution: execution(),
            system_under_test: system(),
            subject: model(),
            judge: None,
            control_plane: ControlPlaneEvidence {
                name: CONTROL_PLANE_CONTRACT_NAME.into(),
                version: CONTROL_PLANE_CONTRACT_VERSION,
                functions: vec![FunctionContractEvidence {
                    function_id: "harness::status".into(),
                    contract: serde_json::json!({"name": CONTROL_PLANE_CONTRACT_NAME, "version": 1}),
                    request_schema: serde_json::json!({"type": "object"}),
                    response_schema: serde_json::json!({"type": "object"}),
                    sha256: TEST_DIGEST.into(),
                }],
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
        report.status = if passed {
            RunStatus::Passed
        } else {
            RunStatus::HardGateFailed
        };
        report
    }

    fn aggregate(runs: Vec<E2eRunReport>) -> E2eScenarioReport {
        E2eScenarioReport::aggregate(
            "case",
            1,
            ExecutionPolicy {
                max_turns: 1,
                max_output_tokens: Some(1),
                max_total_tokens: 1,
                stuck_timeout_seconds: 1,
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
        assert_eq!(report.aggregate.technical_failures, 1);
        assert_eq!(report.aggregate.median_score, Some(90.0));
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
        assert_eq!(report.aggregate.median_score, Some(67.5));
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
            policy_version: 1,
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
            policy_version: 1,
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

        let efficiency = passed.efficiency.unwrap();
        assert_eq!(efficiency.technical_attempts, 2);
        assert_eq!(efficiency.observed_work, Some(33));
        assert!((efficiency.work_amplification.unwrap() - 3.3).abs() < f64::EPSILON);
        assert!((passed.cost.total_usd.unwrap() - 0.30).abs() < f64::EPSILON);
        assert_eq!(passed.wall_time_ms, 1_500);
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
    fn v2_write_materializes_manifest_and_hashed_evidence() {
        let output = tempfile::tempdir().unwrap();
        let mut run = run(100, true);
        run.transcript = Some(serde_json::json!({"messages": ["done"]}));
        let mut report = report(vec![aggregate(vec![run])]);

        let path = report.write_to(output.path(), &manifest()).unwrap();
        assert_eq!(path, output.path().join("results-v2.json"));
        assert!(output.path().join("results.json").is_file());
        assert!(output.path().join("manifest.json").is_file());
        let evidence = &report.scenarios[0].runs[0].evidence;
        assert_eq!(evidence.len(), 1);
        assert!(output.path().join(&evidence[0].path).is_file());
        assert!(evidence[0].sha256.starts_with("sha256:"));
        let (decoded, _) = E2eReport::read_from(output.path()).unwrap();
        assert_eq!(decoded.schema_version, RESULTS_SCHEMA_VERSION);
        let (legacy, _) = E2eReport::read_from(&output.path().join("results.json")).unwrap();
        assert_eq!(legacy.schema_version, 1);
        assert!(legacy.execution.is_none());
        let legacy_value: Value =
            serde_json::from_slice(&std::fs::read(output.path().join("results.json")).unwrap())
                .unwrap();
        assert!(legacy_value.get("schema_version").is_none());
        assert!(legacy_value["scenarios"][0]["runs"][0]
            .get("attempt_id")
            .is_none());
    }

    #[test]
    fn v3_write_preserves_frozen_v2_and_v1_projections() {
        let output = tempfile::tempdir().unwrap();
        let mut report = report(vec![aggregate(vec![run(100, true)])]);
        let run_id = report.scenarios[0].runs[0].run_id.clone();
        let attempt_id = report.scenarios[0].runs[0].attempt_id.clone();
        report.schema_version = LATEST_RESULTS_SCHEMA_VERSION;
        report.assessment_contract = Some(AssessmentContract {
            contract_version: ASSESSMENT_CONTRACT_VERSION,
            runs: vec![RunAssessmentContract {
                run_id,
                attempt_id,
                system_status: SystemStatus::Passed,
                assessments: Vec::new(),
                assets: Vec::new(),
                ai_final_assessment: AiFinalAssessment::not_evaluated(
                    "final assessment is implemented by MOT-4447",
                ),
                effective_status: EffectiveStatus::Passed,
            }],
        });

        let path = report.write_to(output.path(), &manifest()).unwrap();
        assert_eq!(path, output.path().join("results-v3.json"));
        assert!(output.path().join("results-v2.json").is_file());
        assert!(output.path().join("results.json").is_file());

        let (decoded, selected) = E2eReport::read_from(output.path()).unwrap();
        assert_eq!(selected, output.path().join("results-v3.json"));
        assert_eq!(decoded.schema_version, LATEST_RESULTS_SCHEMA_VERSION);
        decoded
            .assessment_contract
            .as_ref()
            .unwrap()
            .validate(&decoded)
            .unwrap();

        let v2: Value =
            serde_json::from_slice(&std::fs::read(output.path().join("results-v2.json")).unwrap())
                .unwrap();
        assert_eq!(v2["schema_version"], RESULTS_SCHEMA_VERSION);
        assert!(v2.get("assessment_contract").is_none());
        let v1: Value =
            serde_json::from_slice(&std::fs::read(output.path().join("results.json")).unwrap())
                .unwrap();
        assert!(v1.get("schema_version").is_none());
        assert!(v1.get("assessment_contract").is_none());
    }

    #[test]
    fn v3_contract_cannot_override_the_run_system_status() {
        let output = tempfile::tempdir().unwrap();
        let mut report = report(vec![aggregate(vec![run(100, true)])]);
        let run_id = report.scenarios[0].runs[0].run_id.clone();
        let attempt_id = report.scenarios[0].runs[0].attempt_id.clone();
        report.schema_version = LATEST_RESULTS_SCHEMA_VERSION;
        report.assessment_contract = Some(AssessmentContract {
            contract_version: ASSESSMENT_CONTRACT_VERSION,
            runs: vec![RunAssessmentContract {
                run_id,
                attempt_id,
                system_status: SystemStatus::HardGateFailed,
                assessments: Vec::new(),
                assets: Vec::new(),
                ai_final_assessment: AiFinalAssessment::not_evaluated("not evaluated"),
                effective_status: EffectiveStatus::HardGateFailed,
            }],
        });

        let error = report.write_to(output.path(), &manifest()).unwrap_err();
        assert!(error.to_string().contains("differs from E2E run status"));
    }

    #[test]
    fn v3_contract_rejects_dangling_evidence_references() {
        let output = tempfile::tempdir().unwrap();
        let mut report = report(vec![aggregate(vec![run(0, false)])]);
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/results/results-v3-assessment-contract.json"
        ))
        .unwrap();
        let mut contract: AssessmentContract =
            serde_json::from_value(fixture["assessment_contract"].clone()).unwrap();
        contract.runs[0].run_id = report.scenarios[0].runs[0].run_id.clone();
        contract.runs[0].attempt_id = report.scenarios[0].runs[0].attempt_id.clone();
        report.schema_version = LATEST_RESULTS_SCHEMA_VERSION;
        report.assessment_contract = Some(contract);

        let error = report.write_to(output.path(), &manifest()).unwrap_err();
        assert!(error
            .to_string()
            .contains("assessment evidence 'transcript' is not present"));
    }

    #[test]
    fn v2_read_rejects_v3_assessment_fields() {
        let output = tempfile::tempdir().unwrap();
        let mut report = report(vec![aggregate(vec![run(100, true)])]);
        let path = report.write_to(output.path(), &manifest()).unwrap();
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value.as_object_mut().unwrap().insert(
            "assessment_contract".into(),
            serde_json::json!({"contract_version": 1, "runs": []}),
        );
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let error = E2eReport::read_from(&path).unwrap_err();
        assert!(error
            .to_string()
            .contains("results v2 cannot carry the v3 assessment contract"));
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
            content: serde_json::json!({ "status": "ready" }),
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
                max_total_tokens: 1,
                stuck_timeout_seconds: 1,
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
        let legacy: Value =
            serde_json::from_slice(&std::fs::read(output.path().join("results.json")).unwrap())
                .unwrap();
        assert!(legacy["scenarios"][0]["runs"][0]
            .get("deliverables")
            .is_none());
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
            content: serde_json::json!({ "status": "ready" }),
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
                max_total_tokens: 1,
                stuck_timeout_seconds: 1,
            },
            vec![attempt],
        )]);
        report.schema_version = LATEST_RESULTS_SCHEMA_VERSION;
        report.assessment_contract = Some(AssessmentContract::from_asset_evidence(&report));
        report.write_to(output.path(), &manifest()).unwrap();

        assert!(output.path().join(captured_path).is_file());
        let (decoded, _) = E2eReport::read_from(output.path()).unwrap();
        let asset = &decoded.assessment_contract.unwrap().runs[0].assets[0];
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
                content: serde_json::json!("wrong shape"),
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

    #[test]
    fn v1_results_remain_readable_with_absent_v2_identity() {
        let output = tempfile::tempdir().unwrap();
        let value = serde_json::json!({
            "subject": model(),
            "passed": false,
            "scenarios": []
        });
        std::fs::write(
            output.path().join("results.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();

        let (report, _) = E2eReport::read_from(output.path()).unwrap();
        assert_eq!(report.schema_version, 1);
        assert!(report.execution.is_none());
        assert!(report.system_under_test.is_none());
        let normalized = AssessmentContract::normalize(&report);
        assert_eq!(normalized.contract_version, ASSESSMENT_CONTRACT_VERSION);
        assert!(normalized.runs.is_empty());
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

    fn metrics() -> SessionMetricsResponseV1 {
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
