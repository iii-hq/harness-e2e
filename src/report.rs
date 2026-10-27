use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::artifact::{self, ArtifactReference};
use crate::assessment::{
    AssessmentContract, AssessmentResult, AssessmentTargetKind, AssetValidationResult,
    EvidenceReference,
};
use crate::identity::{ExecutionIdentity, StackIdentity, SystemUnderTestIdentity};
#[cfg(test)]
use crate::scenarios::DeliverableContract;
use crate::scenarios::{
    CapturedDeliverable, CapturedDeliverableContent, CapturedInvariant, ExecutionPolicy,
    ProvenanceEvidence, ScenarioCase, ScenarioId,
};
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

/// Why a run ended as it did. It decides whether the run counts toward the
/// mean: a failure of the subject or a spent budget counts, keeping the points
/// already measured; one of the provider, the stack, the evaluator's machine
/// or the operator does not, and is counted apart.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RunCause {
    /// No failure: the run reached evaluation.
    Evaluated,
    /// A token, cost or turn budget, or no progress, stopped the subject.
    ResourceLimit,
    /// The provider refused a request the Harness built.
    InvalidProviderRequest,
    /// The conversation outgrew the model's context.
    ContextOverflow,
    /// The adaptive planner answered in the wrong format.
    PlannerFormat,
    /// Any other failure of the subject.
    SubjectFailure,
    /// The provider was unreachable, rate limited, out of balance or not
    /// authorized.
    ProviderInfrastructure,
    /// The stack lacked a function or database the test relies on, or does
    /// not route the configured provider or model.
    TestEnvironmentIncomplete,
    /// The evaluator could not validate the delivery for a reason of its own
    /// machine, such as a Docker daemon that stopped answering.
    EvaluationEnvironment,
    /// Any other failure of the runner or its fixtures.
    E2eInfrastructure,
    /// The operator cancelled the execution.
    OperatorCancelled,
    /// Nobody can be blamed from the evidence: another worker timed out, the
    /// turn was cancelled without an error, or the provider rejected a request
    /// for a reason no rule recognises.
    Undetermined,
}

impl RunCause {
    /// Counted runs are technically valid and enter the mean.
    pub fn counted(self) -> bool {
        matches!(
            self,
            Self::Evaluated
                | Self::ResourceLimit
                | Self::InvalidProviderRequest
                | Self::ContextOverflow
                | Self::PlannerFormat
                | Self::SubjectFailure
        )
    }

    fn label(self) -> &'static str {
        match self {
            Self::Evaluated => "evaluated",
            Self::ResourceLimit => "resource limit",
            Self::InvalidProviderRequest => "invalid provider request",
            Self::ContextOverflow => "context overflow",
            Self::PlannerFormat => "planner format",
            Self::SubjectFailure => "subject failure",
            Self::ProviderInfrastructure => "provider infrastructure",
            Self::TestEnvironmentIncomplete => "test environment incomplete",
            Self::EvaluationEnvironment => "evaluation environment",
            Self::E2eInfrastructure => "E2E infrastructure",
            Self::OperatorCancelled => "cancelled by the operator",
            Self::Undetermined => "undetermined",
        }
    }
}

/// The runner's copy for an operator's cancellation. A composite flow's ends
/// with the same words; the adaptive flow tags its own `execution_cancelled`.
pub(crate) const EXECUTION_CANCELLED: &str = "E2E execution was cancelled";

/// The code of what the subject left running past cleanup, such as a
/// function it registered that outlived its workers. It is the subject's,
/// recorded beside the run, and changes neither the run's status, its cause
/// nor its score.
pub(crate) const SUBJECT_LEFTOVER: &str = "subject_leftover";

/// What the test environment owes a run. `functions` are the ones the run
/// registers itself and needs, as its preflight observed them; `prompt` is
/// the task the scenario gave, for the databases it names and the functions
/// of the workers a stack brings for the measurement. A name the prompt
/// alone mentions, such as a validator the scenario breaks on purpose, is
/// owed by nobody.
struct Owed<'a> {
    prompt: &'a str,
    functions: Vec<&'a str>,
}

/// Functions of the worker every stack declares for the measurement itself
/// (`fp` in `stacks/default.yaml`).
// ponytail: one worker; read the stack's lock if a scenario relies on another.
const MEASUREMENT_FUNCTIONS: &str = "fp::";

/// The cause one failure points to. The failure's code decides when the
/// runner tagged it; else the error the Harness recorded for the failed turn
/// (`custom_type: error`, matched by its summary); else the runner's and the
/// Harness's fixed copy, which reports older than the codes carry.
fn failure_cause(failure: &FailureRecord, transcripts: &[&Value], owed: &Owed) -> RunCause {
    let message = failure.message.as_str();
    if failure.code == "execution_cancelled" || message.ends_with("execution was cancelled") {
        return RunCause::OperatorCancelled;
    }
    match failure.status {
        RunStatus::Passed | RunStatus::HardGateFailed => RunCause::Evaluated,
        RunStatus::InfrastructureError if failure.code == "provider_transport_failed" => {
            RunCause::ProviderInfrastructure
        }
        RunStatus::InfrastructureError if failure.phase == FailurePhase::Evaluate => {
            RunCause::EvaluationEnvironment
        }
        RunStatus::InfrastructureError => RunCause::E2eInfrastructure,
        RunStatus::SubjectError
            if failure.code == "adaptive_plan_invalid"
                || message.starts_with("adaptive planning failed:") =>
        {
            RunCause::PlannerFormat
        }
        RunStatus::SubjectError | RunStatus::ResourceLimit => turn_error(message, transcripts)
            .and_then(|error| turn_error_cause(error, owed))
            .or_else(|| fixed_copy_cause(message, owed))
            .unwrap_or(if failure.status == RunStatus::ResourceLimit {
                RunCause::ResourceLimit
            } else {
                RunCause::SubjectFailure
            }),
    }
}

/// The error entry the Harness wrote for the turn the failure reports.
fn turn_error<'a>(message: &str, transcripts: &[&'a Value]) -> Option<&'a Value> {
    transcripts.iter().find_map(|transcript| {
        transcript
            .get("messages")?
            .as_array()?
            .iter()
            .filter_map(|entry| entry.get("custom"))
            .filter(|custom| custom.get("custom_type").and_then(Value::as_str) == Some("error"))
            .filter_map(|custom| custom.get("data"))
            .rfind(|data| {
                data.get("summary")
                    .and_then(Value::as_str)
                    .is_some_and(|summary| !summary.is_empty() && message.ends_with(summary))
            })
    })
}

fn turn_error_cause(error: &Value, owed: &Owed) -> Option<RunCause> {
    let text = |key: &str| error.get(key).and_then(Value::as_str).unwrap_or_default();
    let (code, class, detail) = (text("code"), text("class"), text("detail"));
    // Only the final failure decides: a function or database missing earlier
    // in the turn, which the turn then got past, does not.
    if missing_test_environment(detail, owed)
        || matches!(
            code,
            "router/unknown_provider"
                | "router/no_provider_for_model"
                | "router/ambiguous_model"
                | "router/structured_output_unsupported"
                | "harness.unknown_model"
                | "harness.budget_unavailable"
        )
    {
        return Some(RunCause::TestEnvironmentIncomplete);
    }
    let cause = if class == "llm.context_overflow" || code.ends_with("context_overflow") {
        RunCause::ContextOverflow
    } else if matches!(
        class,
        "llm.rate_limited" | "llm.transient" | "llm.auth_expired"
    ) || matches!(
        code,
        "router/not_configured" | "router/provider_auth_expired" | "router/provider_unavailable"
    ) {
        RunCause::ProviderInfrastructure
    } else if code == "router/provider_rejected" {
        provider_refusal_cause(&detail.to_ascii_lowercase())
    } else if code.ends_with("invalid_request") {
        RunCause::InvalidProviderRequest
    } else if detail.starts_with("max_turns") {
        RunCause::ResourceLimit
    } else {
        // Unrecognized: the failure's status decides; a Harness dependency
        // repeats its code in the summary, which the fixed copy reads.
        return None;
    };
    Some(cause)
}

/// A provider's refusal, read from the provider's own words, which the
/// Harness keeps as the error's detail.
fn provider_refusal_cause(detail: &str) -> RunCause {
    let says = |needles: &[&str]| needles.iter().any(|needle| detail.contains(needle));
    if says(&[
        "exceeded model token limit",
        "prompt exceeds max length",
        "exceeds the maximum number of tokens",
        "maximum context length",
        "context_length_exceeded",
        "prompt is too long",
    ]) {
        RunCause::ContextOverflow
    } else if says(&[
        // A conversation the Harness sent malformed: a tool call without its
        // result, or a function schema the provider cannot read.
        "ids were found without `tool_result`",
        "must be followed by tool messages",
        "invalid schema for function",
        "invalid_function_parameters",
    ]) {
        RunCause::InvalidProviderRequest
    } else if says(&[
        "insufficient balance",
        "run out of balance",
        "insufficient_quota",
        "insufficient credits",
        "credit balance",
        "exceeded your current quota",
        "exceeded_current_quota",
        "requires more credits",
        "usage limit",
        "key limit exceeded",
        "api key",
        "余额不足",
    ]) {
        RunCause::ProviderInfrastructure
    } else if says(&[
        "model_not_found",
        "model not found",
        "does not exist or you do not have access",
        "is not found for api version",
    ]) {
        // The stack names a model its provider does not serve.
        RunCause::TestEnvironmentIncomplete
    } else {
        // A refusal no rule above explains may be the account's as well as the
        // request's: nobody is blamed.
        RunCause::Undetermined
    }
}

/// The failure says the stack lacks a function or a database the run is
/// owed. One the agent mistyped or made up stays the agent's, and so does
/// one the scenario never provides.
fn missing_test_environment(text: &str, owed: &Owed) -> bool {
    fn names<'a>(text: &'a str, start: &'a str, end: &'a str) -> impl Iterator<Item = &'a str> {
        text.split(start)
            .skip(1)
            .filter_map(move |rest| rest.split_once(end))
            .map(|(name, _)| name.trim_matches(['\'', '"', '`']))
    }
    let named = |name: &str| {
        !name.is_empty()
            && owed
                .prompt
                .split(|c: char| !(c.is_ascii_alphanumeric() || "_:.-".contains(c)))
                .any(|token| token.trim_end_matches('.') == name)
    };
    names(text, "Function ", " not found").any(|name| {
        owed.functions.contains(&name) || name.starts_with(MEASUREMENT_FUNCTIONS) && named(name)
    }) || text.contains("UNKNOWN_DB") && names(text, "\"db\":\"", "\"").any(named)
}

/// Failures that kept no error entry: the Harness's fixed summary for a
/// context overflow, a provider or model the stack does not serve, a
/// provider's refusal or a dependency, and the runner's copy for a turn
/// cancelled without an error. The Harness's `failure_presentation` maps
/// each router code to one summary.
fn fixed_copy_cause(message: &str, owed: &Owed) -> Option<RunCause> {
    let reason = message
        .split_once("turn ended as ")
        .map_or(message, |(_, reason)| reason);
    let ends = |copies: &[&str]| copies.iter().any(|copy| reason.ends_with(copy));
    if missing_test_environment(reason, owed)
        || ends(&[
            "No configured provider can serve the selected model.",
            "The selected provider is no longer registered.",
            "The selected model is not in the model catalog.",
            "The selected model is available from multiple providers.",
            "The selected model does not support structured output.",
        ])
        // A cost cap the catalog cannot price, at send or at a step.
        || reason.contains("no pricing is configured in the model catalog")
    {
        Some(RunCause::TestEnvironmentIncomplete)
    } else if ends(&[
        "The conversation is too large for the selected model.",
        "The conversation could not be compacted to fit the selected model.",
    ]) {
        Some(RunCause::ContextOverflow)
    } else if ends(&[
        "The provider authentication needs attention.",
        "The selected provider is not configured.",
    ]) {
        Some(RunCause::ProviderInfrastructure)
    } else if reason.ends_with("The model request is invalid.") {
        Some(RunCause::InvalidProviderRequest)
    } else if reason.contains("harness/dependency")
        || reason == "Cancelled: no error was reported"
        // Without its code, a refusal is the account's as much as the request's.
        || reason == "Failed: The provider rejected this request."
    {
        Some(RunCause::Undetermined)
    } else {
        None
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
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
    /// The raw points the run's criteria of this dimension awarded. The
    /// dimensions' points add up to the run score: a breakdown of it, not a
    /// second score. Absent when the run has no score or a scored criterion
    /// declared no dimension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<u16>,
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
    /// The screenshots embedded in the content, so a reader can list them
    /// without opening the artifact. Empty when there are none.
    #[serde(default)]
    pub screenshots: Vec<crate::screenshot::ScreenshotReference>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The criterion as a short statement; absent in reports older than the
    /// field and for criteria that state none. Readers fall back to `description`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub possible: u8,
    pub awarded: Option<u8>,
    pub reason: String,
    /// A gate criterion decides completion: the task counts as completed only
    /// when every gate passed. Always written, so a reader can tell "no gate"
    /// from a report older than the field.
    #[serde(default)]
    pub gate: bool,
    /// The points a gate needs to count as passed when fewer than `possible`
    /// already mean the task's purpose was met. Absent: full points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_minimum: Option<u8>,
}

/// The run score is the plain sum of the points the evaluated criteria
/// awarded. A criterion nobody evaluated adds nothing, and a run with no
/// evaluated criterion has no score at all; nothing is normalized or rescaled.
pub fn criteria_score(criteria: &[CriterionReport]) -> Option<u8> {
    let mut evaluated = false;
    let total = criteria
        .iter()
        .filter_map(|criterion| criterion.awarded)
        .inspect(|_| evaluated = true)
        .map(u16::from)
        .sum::<u16>();
    evaluated.then(|| total.min(100) as u8)
}

/// Completed means the subject met the task's purpose: every gate criterion
/// earned its passing points (`gate_minimum`, else full points). A gate that
/// fell short leaves the task incomplete. A gate nobody evaluated leaves it incomplete when another
/// criterion already fell short, and undetermined otherwise, as does a rubric
/// without gates.
pub fn gate_completion(criteria: &[CriterionReport]) -> CompletionState {
    let mut gates = criteria
        .iter()
        .filter(|criterion| criterion.gate)
        .peekable();
    if gates.peek().is_none() {
        return CompletionState::Undetermined;
    }
    let mut unreached = false;
    for gate in gates {
        match gate.awarded {
            Some(awarded) if awarded < gate.gate_minimum.unwrap_or(gate.possible) => {
                return CompletionState::TaskIncomplete
            }
            Some(_) => {}
            None => unreached = true,
        }
    }
    if !unreached {
        CompletionState::Completed
    } else if criteria.iter().any(|criterion| {
        criterion
            .awarded
            .is_some_and(|awarded| awarded < criterion.possible)
    }) {
        CompletionState::TaskIncomplete
    } else {
        CompletionState::Undetermined
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CostReport {
    pub subject_usd: Option<f64>,
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
    /// Prompt tokens served from the provider's cache.
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    /// Every prompt token the provider processed: input, cache reads and
    /// cache writes. `input_tokens` alone leaves the cache out, and the cache
    /// is most of a long session's prompt.
    #[serde(default)]
    pub processed_prompt_tokens: Option<u64>,
    /// The largest prompt one step of the root session sent, from the usage
    /// each assistant message recorded in the transcript. Given only when the
    /// steps add up to the root session's totals.
    #[serde(default)]
    pub peak_context_tokens: Option<u64>,
    /// The prompt budget the Harness fit the root session's window into
    /// (its context snapshot's `usable`).
    #[serde(default)]
    pub usable_context_tokens: Option<u64>,
    /// Compactions of the root session: the `compaction` records the Harness
    /// wrote to its transcript, automatic or asked for. The snapshot's
    /// `compacted` only says the session holds a summary now.
    #[serde(default)]
    pub compactions: Option<u64>,
    /// The Harness prompt-surface digest of each session role (`root`,
    /// `child`): data, not drift, since every Harness release changes it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub prompt_surface_digests: BTreeMap<String, Vec<String>>,
    /// False when a session's provider reported a prompt that does not match
    /// the Harness's own count of what it sent (claude-code adds a prompt of
    /// its own): its prompt and cache figures are not comparable with another
    /// provider's.
    // Judged only against a heuristic estimate (a provider-counted total is the
    // billed value), null when no session has one; the schema text above is
    // left as it was so the result contract's fingerprint does not move.
    #[serde(default)]
    pub cache_comparable: Option<bool>,
    pub cost_usd: Option<f64>,
    pub observed_work: Option<u64>,
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
}

impl RunStatus {
    pub fn is_technical_failure(self) -> bool {
        matches!(
            self,
            Self::SubjectError | Self::ResourceLimit | Self::InfrastructureError
        )
    }

    fn label(self) -> &'static str {
        match self {
            Self::Passed => "PASS",
            Self::HardGateFailed => "HARD GATE FAIL",
            Self::SubjectError => "SUBJECT ERROR",
            Self::ResourceLimit => "RESOURCE LIMIT",
            Self::InfrastructureError => "INFRA ERROR",
        }
    }

    fn failure_domain(self) -> FailureDomain {
        match self {
            Self::SubjectError => FailureDomain::Subject,
            Self::ResourceLimit => FailureDomain::Resource,
            Self::Passed | Self::HardGateFailed | Self::InfrastructureError => {
                FailureDomain::E2eInfrastructure
            }
        }
    }
}

pub(crate) fn classify_failure(status: RunStatus, phase: FailurePhase) -> String {
    match status {
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
    /// The points the attempt's evaluated criteria awarded, as on the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<u8>,
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
    #[serde(skip)]
    #[schemars(skip)]
    pub assessment_results: Vec<AssessmentResult>,
    #[serde(skip)]
    #[schemars(skip)]
    pub asset_assessments: Vec<AssetValidationResult>,
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
            score: report.score,
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
    pub status: RunStatus,
    /// Why the run ended as it did; `technical` follows from it. Absent only
    /// in reports written before causes, which keep the earlier rule: every
    /// subject error left the mean, and an evaluator's own failure could
    /// count as a valid zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<RunCause>,
    pub completion: CompletionState,
    pub technical: TechnicalState,
    pub evaluators: EvaluatorStates,
    pub criteria: Vec<CriterionReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<SessionMetricsResponse>,
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
    /// Advisory behavioral audit over the captured transcript and metrics.
    /// Never contributes to score, status, gates, or longitudinal inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit: Option<crate::audit::AuditReport>,
    /// Harness invariants the run broke; README Completion says what each
    /// costs the run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harness_violations: Vec<crate::harness_invariants::HarnessViolation>,
    #[serde(skip)]
    #[schemars(skip)]
    pub terminal_status: Option<StatusReport>,
    #[serde(skip)]
    #[schemars(skip)]
    pub assessment_results: Vec<AssessmentResult>,
    #[serde(skip)]
    #[schemars(skip)]
    pub asset_assessments: Vec<AssetValidationResult>,
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
            // Set when the run finishes or fails; a run never classified
            // reads as one written before causes.
            cause: None,
            completion: CompletionState::Undetermined,
            technical: TechnicalState::TechnicalInvalid,
            evaluators: EvaluatorStates::default(),
            criteria: Vec::new(),
            transcript: None,
            metrics: None,
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
            audit: None,
            harness_violations: Vec::new(),
            terminal_status: None,
            assessment_results: Vec::new(),
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
        let code = code.into();
        // A subject's leftover never becomes the run's status.
        let is_primary = code != SUBJECT_LEFTOVER
            && self
                .failures
                .iter()
                .all(|failure| failure.code == SUBJECT_LEFTOVER);
        self.failures.push(FailureRecord {
            status,
            domain: status.failure_domain(),
            phase,
            code,
            retry_scope,
            contamination,
            message,
        });
        if is_primary {
            self.status = status;
        }
        self.classify();
    }

    pub fn finish(&mut self, status: RunStatus) {
        self.status = status;
        self.classify();
    }

    /// Read the run's cause from its failures and seal the outcome from it.
    /// Only the producer classifies: a report read back keeps the cause it was
    /// written with, and one written before causes keeps the earlier rule.
    fn classify(&mut self) {
        let transcripts = self
            .transcript
            .iter()
            .chain(
                self.semantic_tests
                    .iter()
                    .filter_map(|test| test.transcript.as_ref()),
            )
            .collect::<Vec<_>>();
        let owed = Owed {
            prompt: &self.prompt,
            functions: self
                .worker_contracts
                .iter()
                .map(|contract| contract.function_id.as_str())
                .collect(),
        };
        let mut causes = Vec::new();
        // A composite step the flow stopped: another failure is the cause. A
        // subject's leftover is recorded and decides nothing.
        for failure in self
            .failures
            .iter_mut()
            .filter(|failure| !matches!(failure.code.as_str(), "step_aborted" | SUBJECT_LEFTOVER))
        {
            // A composite step's failure is read from its own step's
            // transcript (`semantic test '<node>': …`), never from another
            // step whose error has the same summary.
            let step = failure
                .message
                .strip_prefix("semantic test '")
                .and_then(|rest| rest.split_once("': "))
                .map(|(node, _)| node);
            let transcripts = match step {
                Some(node) => self
                    .semantic_tests
                    .iter()
                    .filter(|test| test.node_id == node)
                    .filter_map(|test| test.transcript.as_ref())
                    .collect(),
                None => transcripts.clone(),
            };
            let cause = failure_cause(failure, &transcripts, &owed);
            // The domain says whose failure it is, as the cause does.
            failure.domain = match cause {
                RunCause::Evaluated => failure.status.failure_domain(),
                RunCause::ResourceLimit => FailureDomain::Resource,
                cause if cause.counted() => FailureDomain::Subject,
                _ => FailureDomain::E2eInfrastructure,
            };
            causes.push(cause);
        }
        // A cancellation explains everything after it; any failure the subject
        // did not cause takes the run out; else the first failure decides.
        let cause = causes
            .iter()
            .find(|cause| **cause == RunCause::OperatorCancelled)
            .or_else(|| causes.iter().find(|cause| !cause.counted()))
            .or(causes.first())
            .copied()
            .unwrap_or(match self.status {
                RunStatus::Passed | RunStatus::HardGateFailed => RunCause::Evaluated,
                RunStatus::ResourceLimit => RunCause::ResourceLimit,
                RunStatus::SubjectError => RunCause::SubjectFailure,
                RunStatus::InfrastructureError => RunCause::E2eInfrastructure,
            });
        self.cause = Some(cause);
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
    /// projection for existing scenario implementations. The score is never
    /// touched here: it is what the evaluated criteria awarded, and `technical`
    /// says whether that measurement can be trusted, which the cause decides.
    pub fn seal_outcome_from_status(&mut self) {
        let Some(cause) = self.cause else {
            return self.seal_by_earlier_rule();
        };
        self.technical = if cause.counted() {
            TechnicalState::Valid
        } else {
            TechnicalState::TechnicalInvalid
        };
        if self.evaluators.completion == EvaluatorAvailability::Available {
            return;
        }
        match self.status {
            RunStatus::Passed => {
                self.completion = CompletionState::Completed;
                self.evaluators.completion = EvaluatorAvailability::Available;
            }
            RunStatus::HardGateFailed => {}
            _ if cause.counted() => {
                self.completion = CompletionState::TaskIncomplete;
                self.evaluators.completion = EvaluatorAvailability::Available;
            }
            _ => {
                self.completion = CompletionState::Undetermined;
                self.evaluators.completion = EvaluatorAvailability::Unavailable;
            }
        }
    }

    /// Reports without a cause: a subject error invalidated the run.
    fn seal_by_earlier_rule(&mut self) {
        match self.status {
            RunStatus::Passed => {
                self.technical = TechnicalState::Valid;
                if self.evaluators.completion != EvaluatorAvailability::Available {
                    self.completion = CompletionState::Completed;
                    self.evaluators.completion = EvaluatorAvailability::Available;
                }
            }
            RunStatus::HardGateFailed => {
                self.technical = TechnicalState::Valid;
            }
            RunStatus::ResourceLimit => {
                self.technical = TechnicalState::Valid;
                if self.evaluators.completion != EvaluatorAvailability::Available {
                    self.completion = CompletionState::TaskIncomplete;
                    self.evaluators.completion = EvaluatorAvailability::Available;
                }
            }
            RunStatus::SubjectError | RunStatus::InfrastructureError => {
                self.technical = TechnicalState::TechnicalInvalid;
                if self.evaluators.completion != EvaluatorAvailability::Available {
                    self.completion = CompletionState::Undetermined;
                    self.evaluators.completion = EvaluatorAvailability::Unavailable;
                }
            }
        }
        // The compatibility status retains the first failure, but subsequent
        // infrastructure/cleanup failures still invalidate the technical axis.
        if self.failures.iter().any(|failure| {
            failure.phase == FailurePhase::Cleanup
                || matches!(
                    failure.status,
                    RunStatus::SubjectError | RunStatus::InfrastructureError
                )
        }) {
            self.technical = TechnicalState::TechnicalInvalid;
        }
    }

    pub fn update_cost(&mut self) {
        let subject_usd = self
            .metrics
            .as_ref()
            .and_then(|metrics| metrics.totals.cost_usd);
        self.cost = CostReport {
            subject_usd,
            total_usd: subject_usd,
        };
    }

    pub fn update_efficiency(&mut self) {
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
                "cache_read_tokens",
                "processed_prompt_tokens",
                "peak_context_tokens",
                "usable_context_tokens",
                "compactions",
                "cache_comparable",
                "cost_usd",
                "critical_path_ms",
                "observed_work",
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
                cache_read_tokens: None,
                processed_prompt_tokens: None,
                peak_context_tokens: None,
                usable_context_tokens: None,
                compactions: None,
                prompt_surface_digests: BTreeMap::new(),
                cache_comparable: None,
                cost_usd: self.cost.total_usd,
                observed_work: None,
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
        }
        let context = context_usage(metrics, self.transcript.as_ref(), &mut unavailable);
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
            cache_read_tokens: metrics.totals.cache_read_tokens,
            processed_prompt_tokens: context.processed_prompt_tokens,
            peak_context_tokens: context.peak_context_tokens,
            usable_context_tokens: context.usable_context_tokens,
            compactions: context.compactions,
            prompt_surface_digests: context.prompt_surface_digests,
            cache_comparable: context.cache_comparable,
            cost_usd: self.cost.total_usd,
            observed_work,
            technical_attempts: 1,
            observed_complexity,
            unavailable,
        });
    }

    /// The awarded points per criterion dimension, which add up to the score.
    fn points_by_dimension(&self) -> Option<BTreeMap<EvaluationDimension, u16>> {
        self.score?;
        let mut points = BTreeMap::new();
        for criterion in &self.criteria {
            let Some(awarded) = criterion.awarded else {
                continue;
            };
            let dimension = self
                .assessment_results
                .iter()
                .find(|result| result.criterion_id == criterion.id)?
                .dimension;
            *points.entry(dimension).or_insert(0) += u16::from(awarded);
        }
        Some(points)
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
        aggregate.cache_read_tokens = sum_optional(&attempts, |value| value.cache_read_tokens);
        aggregate.processed_prompt_tokens =
            sum_optional(&attempts, |value| value.processed_prompt_tokens);
        aggregate.peak_context_tokens = max_optional(&attempts, |value| value.peak_context_tokens);
        aggregate.usable_context_tokens =
            max_optional(&attempts, |value| value.usable_context_tokens);
        aggregate.compactions = sum_optional(&attempts, |value| value.compactions);
        for attempt in &attempts {
            for (role, digests) in &attempt.prompt_surface_digests {
                let merged = aggregate
                    .prompt_surface_digests
                    .entry(role.clone())
                    .or_default();
                merged.extend(digests.iter().cloned());
                merged.sort();
                merged.dedup();
            }
        }
        aggregate.cache_comparable = attempts
            .iter()
            .filter_map(|attempt| attempt.cache_comparable)
            .reduce(|all, comparable| all && comparable);
        aggregate.cost_usd = self.cost.total_usd;
        aggregate.observed_work = sum_optional(&attempts, |value| value.observed_work);
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
        let efficiency = self.efficiency.as_ref().map_or_else(
            || serde_json::json!({ "available": false, "wall_time_ms": self.wall_time_ms }),
            |efficiency| serde_json::to_value(efficiency).unwrap_or(Value::Null),
        );
        let infrastructure_failures = self
            .failures
            .iter()
            .filter(|failure| failure.domain == FailureDomain::E2eInfrastructure)
            .count();
        let points = self.points_by_dimension();
        let points = |dimension| {
            points
                .as_ref()
                .map(|points| points.get(&dimension).copied().unwrap_or(0))
        };
        self.dimensions = vec![
            DimensionReport {
                dimension: EvaluationDimension::Deliverable,
                points: points(EvaluationDimension::Deliverable),
                passed: deliverable_passed,
                signals: serde_json::json!({
                    "expected": expects_deliverables,
                    "captured": self.deliverables.len(),
                    "valid": self.deliverables.iter().filter(|item| item.passed()).count(),
                }),
            },
            DimensionReport {
                dimension: EvaluationDimension::StructuralIntegrity,
                points: points(EvaluationDimension::StructuralIntegrity),
                passed: None,
                signals: serde_json::json!({
                    "criteria": self.criteria.iter().filter(|criterion| criterion.awarded.is_some()).count(),
                }),
            },
            DimensionReport {
                dimension: EvaluationDimension::Efficiency,
                points: points(EvaluationDimension::Efficiency),
                passed: None,
                signals: efficiency,
            },
            DimensionReport {
                dimension: EvaluationDimension::Robustness,
                points: points(EvaluationDimension::Robustness),
                passed: None,
                signals: serde_json::json!({
                    "available": false,
                    "reason": "robustness requires a repeated comparable cohort",
                }),
            },
            DimensionReport {
                dimension: EvaluationDimension::E2eInfrastructure,
                points: points(EvaluationDimension::E2eInfrastructure),
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

/// A provider whose reported prompt differs from the Harness's count of the
/// same step by more than this does not report a comparable prompt.
pub(crate) const PROMPT_RECONCILIATION_TOLERANCE: f64 = 0.10;

#[derive(Default)]
struct ContextUsage {
    processed_prompt_tokens: Option<u64>,
    peak_context_tokens: Option<u64>,
    usable_context_tokens: Option<u64>,
    compactions: Option<u64>,
    prompt_surface_digests: BTreeMap<String, Vec<String>>,
    cache_comparable: Option<bool>,
}

/// A step's prompt as its provider billed it: input, cache reads and writes.
pub(crate) fn step_prompt(usage: &Value) -> u64 {
    ["input", "cache_read", "cache_write"]
        .iter()
        .filter_map(|key| usage.get(key).and_then(Value::as_u64))
        .fold(0, u64::saturating_add)
}

fn context_usage(
    metrics: &SessionMetricsResponse,
    transcript: Option<&Value>,
    unavailable: &mut BTreeMap<String, String>,
) -> ContextUsage {
    let totals = &metrics.totals;
    let mut usage = ContextUsage {
        processed_prompt_tokens: totals.input_tokens.zip(totals.cache_read_tokens).map(
            |(input, read)| {
                input
                    .saturating_add(read)
                    .saturating_add(totals.cache_write_tokens.unwrap_or(0))
            },
        ),
        ..ContextUsage::default()
    };
    if totals.cache_read_tokens.is_none() {
        unavailable.insert(
            "cache_read_tokens".into(),
            "provider did not report cache reads".into(),
        );
        unavailable.insert(
            "processed_prompt_tokens".into(),
            "provider did not report cache reads".into(),
        );
    }
    let root = metrics.by_session.iter().find(|session| session.depth == 0);
    usage.usable_context_tokens =
        root.and_then(|session| session.context.as_ref()?.get("usable")?.as_u64());
    if usage.usable_context_tokens.is_none() {
        unavailable.insert(
            "usable_context_tokens".into(),
            "the root session has no context snapshot".into(),
        );
    }
    for session in &metrics.by_session {
        let Some(context) = session.context.as_ref() else {
            continue;
        };
        if let Some(digest) = context.get("prompt_surface_digest").and_then(Value::as_str) {
            let role = if session.depth == 0 { "root" } else { "child" };
            let digests = usage.prompt_surface_digests.entry(role.into()).or_default();
            if !digests.iter().any(|known| known == digest) {
                digests.push(digest.into());
                digests.sort();
            }
        }
        // The snapshot keeps the last step's billed usage beside the count
        // the Harness made of the same prompt.
        // Only a heuristic count is the Harness's own: a provider-counted
        // total is overwritten with the billed value and always agrees.
        let billed = context.get("usage").map_or(0, step_prompt);
        let counted = context.get("total").and_then(Value::as_u64).unwrap_or(0);
        let heuristic = context.get("estimator").and_then(Value::as_str) == Some("heuristic");
        if heuristic && billed > 0 && counted > 0 {
            let reconciles = (billed as f64 - counted as f64).abs()
                <= PROMPT_RECONCILIATION_TOLERANCE * counted as f64;
            usage.cache_comparable = Some(usage.cache_comparable.unwrap_or(true) && reconciles);
        }
    }
    if usage.cache_comparable.is_none() {
        unavailable.insert(
            "cache_comparable".into(),
            "no session has a heuristic context estimate to hold against the billed step".into(),
        );
    }
    let Some(messages) = transcript
        .and_then(|transcript| transcript.get("messages"))
        .and_then(Value::as_array)
    else {
        for field in ["peak_context_tokens", "compactions"] {
            unavailable.insert(field.into(), "transcript unavailable".into());
        }
        return usage;
    };
    let recorded = messages.iter().any(|entry| entry.get("custom").is_some());
    let snapshot_compacted = metrics.by_session.iter().any(|session| {
        session
            .context
            .as_ref()
            .and_then(|context| context.get("compacted"))
            == Some(&Value::Bool(true))
    });
    if recorded || snapshot_compacted {
        usage.compactions = Some(
            messages
                .iter()
                .filter(|entry| {
                    entry.pointer("/custom/custom_type").and_then(Value::as_str)
                        == Some("compaction")
                })
                .count() as u64,
        );
    } else {
        unavailable.insert(
            "compactions".into(),
            "the transcript records no custom entries and no snapshot says compacted".into(),
        );
    }
    usage.peak_context_tokens = root_peak_prompt(metrics, messages);
    if usage.peak_context_tokens.is_none() {
        unavailable.insert(
            "peak_context_tokens".into(),
            "the transcript's per-step usage does not add up to the root session's totals".into(),
        );
    }
    usage
}

/// The largest prompt a root step sent, only when the transcript's per-step
/// usage adds up to the root session's totals: a step without usage could be
/// the largest.
pub(crate) fn root_peak_prompt(
    metrics: &SessionMetricsResponse,
    messages: &[Value],
) -> Option<u64> {
    let steps = messages
        .iter()
        .filter_map(|entry| entry.get("message"))
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .filter_map(|message| message.get("usage"))
        .map(step_prompt)
        .collect::<Vec<_>>();
    let root = metrics
        .by_session
        .iter()
        .find(|session| session.depth == 0)?;
    let total = root.input_tokens.map(|input| {
        input
            .saturating_add(root.cache_read_tokens.unwrap_or(0))
            .saturating_add(root.cache_write_tokens.unwrap_or(0))
    })?;
    let step_sum = steps.iter().copied().fold(0, u64::saturating_add);
    (step_sum.abs_diff(total) as f64 <= 0.01 * total as f64)
        .then(|| steps.iter().copied().max())
        .flatten()
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
    /// Observed runs per cause: the counted ones are the technically valid
    /// runs. Runs from reports written before causes are left out of it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub causes: BTreeMap<RunCause, u32>,
    pub execution_reliability: Option<f64>,
    pub completion_evidence_coverage: Option<f64>,
    pub completion_rate: Option<f64>,
    /// Technically valid runs that have a score.
    pub scored_runs: u32,
    /// Mean score of the technically valid runs that have one; null when none.
    pub mean_score: Option<f64>,
    pub total_tokens_consumed: Option<u64>,
    pub tokens_completed_p50: Option<f64>,
    pub failed_attempt_tokens: Option<u64>,
    pub tokens_per_completion: Option<f64>,
    pub passed_runs: u32,
    pub required_passes: u32,
    pub pass_rate: f64,
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
        if self.scored_runs > self.technical_valid_runs {
            bail!("scenario aggregate scored runs exceed the technically valid runs");
        }
        for (name, value) in [
            ("execution_reliability", self.execution_reliability),
            (
                "completion_evidence_coverage",
                self.completion_evidence_coverage,
            ),
            ("completion_rate", self.completion_rate),
        ] {
            if value.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
                bail!("scenario aggregate {name} must be null or a finite ratio");
            }
        }
        for (name, value) in [
            ("mean_score", self.mean_score),
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct E2eScenarioReport {
    pub scenario_id: String,
    /// Presentation captured when this result was produced; absent in older results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// What the test protects and what breaks when it fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protects: Option<String>,
    /// Workers, area and disciplines; presentation like `protects`.
    #[serde(flatten)]
    pub tags: crate::scenarios::tags::ScenarioTags,
    #[serde(default)]
    pub case_id: String,
    /// Digest of the definition the case was materialized from. Absent only
    /// when no case could be materialized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior_sha256: Option<String>,
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
        execution_policy: ExecutionPolicy,
        runs: Vec<E2eRunReport>,
    ) -> Self {
        let case = ScenarioCase::new(
            scenario_id,
            0,
            serde_json::json!({ "variant": "canonical" }),
            Vec::new(),
            DeliverableContract::default(),
        )
        .expect("canonical scenario case is valid")
        .seal(Self::canonical_test_behavior_sha256())
        .expect("canonical scenario case seals");
        Self::aggregate_case(case, execution_policy, runs)
    }

    /// The definition digest `aggregate` seals into its canonical test case.
    #[cfg(test)]
    pub(crate) fn canonical_test_behavior_sha256() -> String {
        artifact::sha256_bytes(b"canonical scenario definition")
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
            Some(case.behavior_sha256.clone()),
            Some(case),
            execution_policy,
            planned_runs,
            runs,
        )
    }

    /// Preserve requested slots even when no executable case could be materialized.
    /// The definition digest stays absent rather than fabricating a case identity.
    pub fn deferred(
        scenario_id: String,
        case_id: String,
        execution_policy: ExecutionPolicy,
        planned_runs: u32,
        reason: String,
    ) -> Self {
        let mut report = Self::aggregate_with_planned(
            scenario_id,
            case_id,
            None,
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
        behavior_sha256: Option<String>,
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
        let mut causes = BTreeMap::new();
        for cause in runs.iter().filter_map(|run| run.cause) {
            *causes.entry(cause).or_insert(0_u32) += 1;
        }
        let scores = runs
            .iter()
            .filter(|run| run.technical == TechnicalState::Valid)
            .filter_map(|run| run.score)
            .collect::<Vec<_>>();
        let scored_runs = scores.len() as u32;
        let mean_score = mean(&scores);
        let total_tokens_consumed =
            sum_u64(runs.iter().map(|run| run.efficiency.as_ref()?.total_tokens));
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
        let technical_failures = technical_invalid_runs;
        let required_passes = required_passes(planned_runs);
        let cost = CostReport {
            subject_usd: sum_cost(runs.iter().map(|run| run.cost.subject_usd)),
            total_usd: sum_cost(runs.iter().map(|run| run.cost.total_usd)),
        };
        let robustness = robustness_report(&runs);
        let passed = planned_runs > 0
            && run_count == planned_runs
            && technical_failures == 0
            && undetermined_runs == 0
            && passed_runs >= required_passes;
        let metadata = scenario_id.parse::<ScenarioId>().ok();
        Self {
            case_id,
            scenario_id,
            title: metadata.and_then(ScenarioId::title).map(str::to_owned),
            summary: metadata.and_then(ScenarioId::summary).map(str::to_owned),
            protects: metadata.and_then(ScenarioId::protects).map(str::to_owned),
            tags: metadata.map(ScenarioId::tags).unwrap_or_default(),
            behavior_sha256,
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
                causes,
                execution_reliability: ratio(technical_valid_runs, planned_runs),
                completion_evidence_coverage: ratio(determined_runs, planned_runs),
                completion_rate: ratio(completed_runs, determined_runs),
                scored_runs,
                mean_score,
                total_tokens_consumed,
                tokens_completed_p50,
                failed_attempt_tokens,
                tokens_per_completion,
                passed_runs,
                required_passes,
                pass_rate: if run_count == 0 {
                    0.0
                } else {
                    f64::from(passed_runs) / f64::from(run_count)
                },
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
        let title = self.title.take();
        let summary = self.summary.take();
        let protects = self.protects.take();
        let tags = std::mem::take(&mut self.tags);
        let case = self.case.take();
        let runs = std::mem::take(&mut self.runs);
        let deferral_reason = self.deferral_reason.take();
        *self = Self::aggregate_with_planned(
            self.scenario_id.clone(),
            self.case_id.clone(),
            self.behavior_sha256.clone(),
            case,
            self.execution_policy,
            planned_runs,
            runs,
        );
        self.title = title;
        self.summary = summary;
        self.protects = protects;
        self.tags = tags;
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
pub struct AgentProfileArtifact {
    pub id: String,
    pub configuration_sha256: String,
    /// Functions the profile preloads that this stack does not register. The
    /// Harness skips them, as it does in any project without their worker;
    /// listed so a report says the session ran without them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unavailable_functions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelArtifact {
    pub model: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentProfileArtifact>,
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub supports_tools: Option<bool>,
    pub supports_vision: Option<bool>,
}

pub const OBSERVATION_SCHEMA: &str = "e2e-observation";
pub const CATALOG_SCHEMA: &str = "e2e-scenario-catalog";
pub use crate::result_contract::RESULT_CONTRACT_SHA256;

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
    pub scenario_id: crate::scenarios::ScenarioId,
    pub behavior_sha256: String,
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
            if case.case_id.trim().is_empty() {
                bail!("run_contract contains an invalid selected case identity");
            }
            validate_observation_sha256(&case.behavior_sha256, "selected case definition")?;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior_sha256: Option<String>,
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
            agent: None,
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
    pub result_contract_sha256: String,
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
    pub engine_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_contract: Option<ObservationRunContract>,
    pub passed: bool,
    #[serde(default)]
    pub redaction: crate::redaction::RedactionReport,
    pub assessment_contract: AssessmentContract,
    pub scenarios: Vec<E2eScenarioReport>,
}

/// What a reader is told about runs written before causes.
pub const EARLIER_RULE: &str = "runs written before run causes were classified by the earlier rule: every subject error left the mean, and an evaluator's own failure could count as a valid zero";

impl E2eReport {
    /// Runs written before causes, which keep the earlier rule.
    pub fn runs_without_cause(&self) -> usize {
        self.scenarios
            .iter()
            .flat_map(|scenario| &scenario.runs)
            .filter(|run| run.cause.is_none())
            .count()
    }

    pub fn execution_succeeded(&self) -> bool {
        self.report_state == ReportState::Complete
            && self.persistence_errors.is_empty()
            && !self.scenarios.is_empty()
            && self.scenarios.iter().all(|scenario| {
                scenario.aggregate.planned_runs > 0
                    && scenario.aggregate.technical_valid_runs == scenario.aggregate.planned_runs
                    && scenario.aggregate.observed_runs == scenario.aggregate.planned_runs
                    && scenario.runs.iter().all(|run| {
                        matches!(run.status, RunStatus::Passed | RunStatus::HardGateFailed)
                    })
            })
    }

    pub fn new(
        execution: ExecutionIdentity,
        system_under_test: SystemUnderTestIdentity,
        subject: ModelArtifact,
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
            result_contract_sha256: RESULT_CONTRACT_SHA256.into(),
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
        let contract = value.get("result_contract_sha256").and_then(Value::as_str);
        if contract != Some(RESULT_CONTRACT_SHA256) {
            tracing::warn!(
                path = %path.display(),
                contract = contract.unwrap_or("missing"),
                current = RESULT_CONTRACT_SHA256,
                "reading a results report written under another results contract"
            );
        }
        let report: Self = serde_json::from_value(value)
            .with_context(|| format!("decode typed E2E report {}", path.display()))?;
        let earlier = report.runs_without_cause();
        if earlier > 0 {
            tracing::warn!(
                path = %path.display(),
                runs = earlier,
                "{EARLIER_RULE}"
            );
        }
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
        // Contract drift is a warning: the report says which contract and
        // scoring profile it was written under, and both stay visible.
        if self.result_contract_sha256 != RESULT_CONTRACT_SHA256 {
            tracing::warn!(
                contract = %self.result_contract_sha256,
                current = RESULT_CONTRACT_SHA256,
                "results report was written under another results contract"
            );
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
                if scenario.case_id != case.case_id
                    || scenario.scenario_id != case.scenario_id
                    || scenario.behavior_sha256.as_deref() != Some(case.behavior_sha256.as_str())
                {
                    bail!("scenario identity differs from its materialized case");
                }
            }
            for run in &scenario.runs {
                validate_attempt_identity(run)?;
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
                scenario.behavior_sha256.clone(),
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
                let preserved_evidence = run
                    .evidence
                    .drain(..)
                    .filter(|reference| reference.kind == "runtime_diagnostics")
                    .collect::<Vec<_>>();
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
                append_semantic_references(
                    &mut run.evidence,
                    run.scenario_flow.as_ref(),
                    &run.semantic_tests,
                );
                append_verified_references(output, &mut run.evidence, preserved_evidence)?;
                bind_assessment_evidence(&mut run.assessment_results, &run.evidence);
                for attempt in &mut run.retry_attempts {
                    let preserved_evidence = attempt
                        .evidence
                        .drain(..)
                        .filter(|reference| reference.kind == "runtime_diagnostics")
                        .collect::<Vec<_>>();
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
                    append_semantic_references(
                        &mut attempt.evidence,
                        attempt.scenario_flow.as_ref(),
                        &attempt.semantic_tests,
                    );
                    append_verified_references(output, &mut attempt.evidence, preserved_evidence)?;
                    bind_assessment_evidence(&mut attempt.assessment_results, &attempt.evidence);
                }
            }
        }
        Ok(())
    }
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
                "preserved evidence conflicts with an existing artifact reference: {}",
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
    criteria: &mut [CriterionReport],
    dimensions: &mut [DimensionReport],
    deliverables: &mut [DeliverableReport],
) {
    for failure in failures {
        redact_string(policy, redaction, &mut failure.message);
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
    assets: &mut Vec<AssetValidationResult>,
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
            // Everything a reader needs to tell the three cases apart without
            // reproducing the run: the artifact was written from different
            // content, the artifact was rewritten after the report was built,
            // or the report's hash was taken over a value the artifact never
            // held. The raw file digest separates "the bytes on disk changed"
            // from "the same bytes canonicalize differently".
            bail!(
                "deliverable '{}' content hash does not match its artifact\n                   artifact      : {}\n                   expected      : {}\n                   observed      : {}\n                   file digest   : {} (reference says {})\n                   file size     : {} bytes (reference says {})",
                deliverable.id,
                reference.path,
                deliverable.content_sha256,
                observed_hash,
                artifact::sha256_bytes(&bytes),
                reference.sha256,
                bytes.len(),
                reference.size_bytes,
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

fn mean(values: &[u8]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some(values.iter().map(|value| f64::from(*value)).sum::<f64>() / values.len() as f64)
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
        AssessmentKind, AssessmentOutcome, AssessmentPolicy, AssessmentScore, AssessmentTarget,
        SystemStatus,
    };
    use crate::identity::StackIdentity;
    use crate::scenarios::{ArtifactExpectation, Capability, InvariantSpec};
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
                scenario_id: crate::scenarios::ScenarioId::ContextPressure,
                behavior_sha256: format!("sha256:{}", "d".repeat(64)),
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
        E2eReport::new(execution(), system(), model(), None, scenarios)
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
            ExecutionPolicy {
                max_turns: Some(1),
                max_output_tokens: Some(1),
                max_total_tokens: Some(1),
                stuck_timeout_seconds: 1,
                max_validation_retries: None,
            },
            runs,
        )
    }

    #[test]
    fn partial_scores_round_trip_independently_of_completion_and_limits() {
        for status in [RunStatus::Passed, RunStatus::ResourceLimit] {
            let output = tempfile::tempdir().unwrap();
            let mut measured = run(65, true);
            measured.set_completion(
                CompletionState::TaskIncomplete,
                EvaluatorAvailability::Available,
            );
            measured.finish(status);
            assert_eq!(measured.score, Some(65));
            let mut result = report(vec![aggregate(vec![measured])]);
            assert_eq!(result.execution_succeeded(), status == RunStatus::Passed);
            let path = result.write_to(output.path(), &manifest()).unwrap();
            let (restored, _) = E2eReport::read_from(&path).unwrap();
            assert_eq!(restored.scenarios[0].runs[0].score, Some(65));
            assert_eq!(
                restored.scenarios[0].runs[0].completion,
                CompletionState::TaskIncomplete
            );
        }
        let mut unavailable = run(0, true);
        unavailable.score = None;
        unavailable.finish(RunStatus::ResourceLimit);
        assert_eq!(unavailable.score, None);
        let mut evaluator_error = run(65, true);
        evaluator_error.finish(RunStatus::InfrastructureError);
        assert!(!report(vec![aggregate(vec![evaluator_error])]).execution_succeeded());
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
        assert_eq!(report.aggregate.mean_score, Some(83.0));
    }

    #[test]
    fn costs_are_aggregated_without_hiding_unknown_values() {
        let mut first = run(90, true);
        first.cost = CostReport {
            subject_usd: Some(0.1),
            total_usd: Some(0.1),
        };
        let mut second = run(90, true);
        second.cost = CostReport {
            subject_usd: Some(0.2),
            total_usd: None,
        };
        let report = aggregate(vec![first, second]);
        assert!((report.aggregate.cost.subject_usd.unwrap() - 0.3).abs() < f64::EPSILON);
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
            RunStatus::InfrastructureError,
            FailurePhase::Evaluate,
            "evaluator unavailable",
        );
        let report = aggregate(vec![run(90, true), run(90, true), error]);
        assert!(!report.passed);
        assert_eq!(report.aggregate.scored_runs, 2);
        assert_eq!(report.aggregate.technical_invalid_runs, 1);
        assert_eq!(report.aggregate.mean_score, Some(90.0));
    }

    /// The 26 subject errors of the local history before causes, as their
    /// reports recorded them: failure, the Harness's error entry, and the
    /// gate's nudges. Each run carries its scenario's own prompt and ends as
    /// the suite ends it: a counted run always has a score.
    #[test]
    fn local_history_subject_errors_each_fall_in_one_cause() {
        #[derive(Deserialize)]
        struct Case {
            scenario: String,
            expected: RunCause,
            failure: FailureRecord,
            transcript: Value,
        }
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../tests/fixtures/results/run-causes.json"))
                .unwrap();
        let mut counted = BTreeMap::<RunCause, u32>::new();
        let mut excluded = BTreeMap::<RunCause, u32>::new();
        for case in cases {
            let spec = case
                .scenario
                .parse::<crate::scenarios::ScenarioId>()
                .unwrap()
                .spec("run");
            let mut run = E2eRunReport::new(
                "run".into(),
                "attempt".into(),
                1,
                "session".into(),
                spec.prompt.clone(),
            );
            run.transcript = Some(case.transcript);
            let failure = case.failure;
            run.push_typed_failure(
                failure.status,
                failure.phase,
                failure.code,
                failure.retry_scope,
                failure.contamination,
                failure.message,
            );
            crate::suite::zero_unreached_criteria(&spec, &mut run);
            assert_eq!(run.cause, Some(case.expected), "{}", case.scenario);
            let valid = run.technical == TechnicalState::Valid;
            assert_eq!(valid, case.expected.counted(), "{}", case.scenario);
            if valid {
                assert_eq!(run.completion, CompletionState::TaskIncomplete);
                assert_eq!(run.score, Some(0), "{}", case.scenario);
            } else {
                assert_eq!(run.score, None, "{}", case.scenario);
            }
            *if valid { &mut counted } else { &mut excluded }
                .entry(case.expected)
                .or_default() += 1;
        }
        // validation_self_repair's database was missing only earlier in the
        // turn; its final failure is the turn limit.
        assert_eq!(
            counted,
            BTreeMap::from([
                (RunCause::ResourceLimit, 2),
                (RunCause::InvalidProviderRequest, 1),
                (RunCause::ContextOverflow, 1),
                (RunCause::PlannerFormat, 2),
            ])
        );
        assert_eq!(
            excluded,
            BTreeMap::from([
                (RunCause::ProviderInfrastructure, 10),
                (RunCause::TestEnvironmentIncomplete, 1),
                (RunCause::OperatorCancelled, 7),
                (RunCause::Undetermined, 2),
            ])
        );
    }

    #[test]
    fn a_provider_refusal_counts_only_when_the_request_was_wrong() {
        let refused = |code: &str, detail: &str| {
            let mut run = run(0, false);
            run.evaluators = EvaluatorStates::default();
            run.transcript = Some(serde_json::json!({"messages": [{"custom": {
                "custom_type": "error",
                "data": {"code": code, "class": "llm.permanent",
                         "summary": "The provider rejected this request.", "detail": detail}
            }}]}));
            run.push_failure(
                RunStatus::SubjectError,
                FailurePhase::Execute,
                "turn ended as Failed: The provider rejected this request.",
            );
            run.cause.unwrap()
        };
        for (detail, cause) in [
            // Moonshot (Kimi), Z.ai (code 1113) and OpenRouter out of credit.
            (
                r#"{"error":{"message":"Your account org-1 <ak-1> is suspended, please check your plan and billing details","type":"exceeded_current_quota_error"}}"#,
                RunCause::ProviderInfrastructure,
            ),
            (
                r#"{"error":{"code":"1113","message":"余额不足或无可用资源包,请充值。"}}"#,
                RunCause::ProviderInfrastructure,
            ),
            (
                r#"{"error":{"message":"This request requires more credits, or fewer max_tokens. You requested up to 32000 tokens, but can only afford 1234.","code":402}}"#,
                RunCause::ProviderInfrastructure,
            ),
            // DeepSeek, Anthropic and Z.ai (1308) usage limits, an OpenRouter
            // key limit and an expired Gemini key.
            (
                r#"{"error":{"message":"You have run out of balance","type":"unknown_error"}}"#,
                RunCause::ProviderInfrastructure,
            ),
            (
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"You have reached your specified API usage limits. You will regain access on 2026-11-01 at 00:00 UTC."}}"#,
                RunCause::ProviderInfrastructure,
            ),
            (
                r#"{"error":{"code":"1308","message":"Usage limit reached for 5 hour. Your limit will reset at 2026-10-24 20:00:00"}}"#,
                RunCause::ProviderInfrastructure,
            ),
            (
                r#"{"error":{"message":"Key limit exceeded (total limit). Manage it using https://openrouter.ai/settings/keys","code":403}}"#,
                RunCause::ProviderInfrastructure,
            ),
            (
                r#"{"error":{"code":400,"message":"API key expired. Please renew the API key.","status":"INVALID_ARGUMENT"}}"#,
                RunCause::ProviderInfrastructure,
            ),
            // Kimi, Z.ai (1261) and Gemini overflow in their own words.
            (
                r#"{"error":{"message":"Invalid request: Your request exceeded model token limit: 262144","type":"invalid_request_error"}}"#,
                RunCause::ContextOverflow,
            ),
            (
                r#"{"error":{"code":"1261","message":"Prompt exceeds max length"}}"#,
                RunCause::ContextOverflow,
            ),
            (
                r#"{"error":{"code":400,"message":"The input token count (1200000) exceeds the maximum number of tokens allowed (1048576).","status":"INVALID_ARGUMENT"}}"#,
                RunCause::ContextOverflow,
            ),
            // A conversation the Harness sent malformed.
            (
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"messages.3: `tool_use` ids were found without `tool_result` blocks immediately after: toolu_01"}}"#,
                RunCause::InvalidProviderRequest,
            ),
            (
                r#"{"error":{"message":"Invalid schema for function 'state__set': In context=(), 'required' is required to be supplied","type":"invalid_request_error","param":"tools[0].function.parameters","code":"invalid_function_parameters"}}"#,
                RunCause::InvalidProviderRequest,
            ),
            // A model the stack names and its provider does not serve.
            (
                r#"{"error":{"message":"The model `kimi-k3` does not exist or you do not have access to it."}}"#,
                RunCause::TestEnvironmentIncomplete,
            ),
            (
                r#"{"error":{"message":"The model `gpt-9` does not exist or you do not have access to it.","type":"invalid_request_error","param":null,"code":"model_not_found"}}"#,
                RunCause::TestEnvironmentIncomplete,
            ),
            // A refusal no rule explains blames nobody.
            (
                r#"{"error":{"message":"Invalid prompt: your prompt was flagged as potentially violating our usage policy.","type":"invalid_request_error"}}"#,
                RunCause::Undetermined,
            ),
        ] {
            assert_eq!(
                refused("router/provider_rejected", detail),
                cause,
                "{detail}"
            );
        }
        assert_eq!(
            refused(
                "provider/invalid_request",
                "remote error (provider/invalid_request): bad ProviderStreamInput"
            ),
            RunCause::InvalidProviderRequest
        );
        // A stack that does not route, single out or price the configured
        // provider or model.
        for code in [
            "router/unknown_provider",
            "router/no_provider_for_model",
            "router/ambiguous_model",
            "router/structured_output_unsupported",
            "harness.unknown_model",
            "harness.budget_unavailable",
        ] {
            assert_eq!(
                refused(code, "no route"),
                RunCause::TestEnvironmentIncomplete,
                "{code}"
            );
        }
        // Without its error entry (an adaptive planner turn, an unreadable
        // transcript), the Harness's copy for each router code decides.
        let bare = |message: &str| {
            let mut bare = run(0, false);
            bare.evaluators = EvaluatorStates::default();
            bare.push_failure(RunStatus::SubjectError, FailurePhase::Execute, message);
            bare.cause.unwrap()
        };
        for (reason, cause) in [
            // A refusal says no more.
            (
                "The provider rejected this request.",
                RunCause::Undetermined,
            ),
            (
                "The provider authentication needs attention.",
                RunCause::ProviderInfrastructure,
            ),
            (
                "The selected provider is not configured.",
                RunCause::ProviderInfrastructure,
            ),
            (
                "No configured provider can serve the selected model.",
                RunCause::TestEnvironmentIncomplete,
            ),
            (
                "The selected model is available from multiple providers.",
                RunCause::TestEnvironmentIncomplete,
            ),
            (
                "The selected model does not support structured output.",
                RunCause::TestEnvironmentIncomplete,
            ),
            (
                "cannot enforce max_cost_usd for model m because no pricing is configured in the model catalog; configure input/output pricing or remove max_cost_usd",
                RunCause::TestEnvironmentIncomplete,
            ),
            ("The model request is invalid.", RunCause::InvalidProviderRequest),
        ] {
            assert_eq!(bare(&format!("turn ended as Failed: {reason}")), cause, "{reason}");
        }
        // The Harness refuses a cost cap it cannot price at send as well.
        assert_eq!(
            bare("invoke harness::send: remote error (invocation_failed): cannot enforce max_cost_usd for model m because no pricing is configured in the model catalog; configure input/output pricing or remove max_cost_usd"),
            RunCause::TestEnvironmentIncomplete
        );
    }

    #[test]
    fn only_a_function_the_run_is_owed_makes_the_stack_incomplete() {
        let failed_with = |prompt: &str, required: &[&str], status, message: &str| {
            let mut run = E2eRunReport::new(
                "run".into(),
                "attempt".into(),
                1,
                "session".into(),
                prompt.into(),
            );
            run.worker_contracts = required
                .iter()
                .map(|function_id| ObservedWorkerContract {
                    function_id: (*function_id).into(),
                    request_schema_sha256: TEST_DIGEST.into(),
                    response_schema_sha256: TEST_DIGEST.into(),
                })
                .collect();
            run.push_failure(status, FailurePhase::Execute, message);
            run.cause.unwrap()
        };
        let failed = |prompt: &str, message: &str| {
            failed_with(prompt, &[], RunStatus::SubjectError, message)
        };
        // validation_chain's deliberately broken validator, which its prompt
        // names: a Harness that fails it closed is the regression the
        // scenario protects, never an incomplete stack.
        let chain = crate::scenarios::ScenarioId::ValidationChain
            .spec("run")
            .prompt;
        let broken = chain
            .split("function_id \"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap();
        assert!(broken.starts_with("e2e::does_not_exist_"), "{broken}");
        let hook = format!("hook {broken} failed: remote error (function_not_found): Function {broken} not found in namespace my-project.");
        for (status, message, cause) in [
            (
                RunStatus::ResourceLimit,
                format!("turn ended as Failed: max_turns reached with the post-turn gate unsatisfied: {hook}"),
                RunCause::ResourceLimit,
            ),
            (
                RunStatus::SubjectError,
                format!("turn ended as Failed: {hook}"),
                RunCause::SubjectFailure,
            ),
        ] {
            assert_eq!(failed_with(&chain, &[], status, &message), cause, "{message}");
        }
        // A function the run registers itself and needs is owed, named or not.
        let resolver = "turn ended as Failed: hook e2etest::contract_resolver_1 failed: remote error (function_not_found): Function e2etest::contract_resolver_1 not found in namespace my-project.";
        assert_eq!(
            failed_with(
                "Resolve the contract.",
                &["e2etest::contract_resolver_1"],
                RunStatus::SubjectError,
                resolver
            ),
            RunCause::TestEnvironmentIncomplete
        );
        assert_eq!(
            failed("Resolve the contract.", resolver),
            RunCause::SubjectFailure
        );
        let missing = "turn ended as Failed: max_turns reached with the post-turn gate unsatisfied: hook fp::pipe failed: remote error (function_not_found): Function fp::pipe not found in namespace my-project.";
        assert_eq!(
            failed(
                "Call engine::register_trigger with function_id: \"fp::pipe\".",
                missing
            ),
            RunCause::TestEnvironmentIncomplete
        );
        // The agent's typo: the scenario never named it.
        assert_eq!(
            failed(
                "Call engine::register_trigger with function_id: \"fp::pipeline\".",
                missing
            ),
            RunCause::SubjectFailure
        );
        let database = r#"turn ended as Failed: database::query failed: {"code":"UNKNOWN_DB","db":"primary","available":["harness_e2e"]}"#;
        assert_eq!(
            failed("Use database::execute (db `primary`).", database),
            RunCause::TestEnvironmentIncomplete
        );
        assert_eq!(
            failed("Use database::execute (db `harness_e2e`).", database),
            RunCause::SubjectFailure
        );
    }

    #[test]
    fn counted_causes_stay_in_the_mean_and_the_others_are_counted_apart() {
        let failed = |status, phase, code: &str, message: &str| {
            let mut run = run(0, false);
            run.score = Some(30);
            run.evaluators = EvaluatorStates::default();
            run.push_typed_failure(
                status,
                phase,
                code,
                RetryScope::None,
                ContaminationScope::None,
                message,
            );
            run
        };
        let turns = failed(
            RunStatus::ResourceLimit,
            FailurePhase::Execute,
            "subject_budget_exhausted",
            "turn ended as Failed: max_turns reached with the post-turn gate unsatisfied",
        );
        assert_eq!(turns.cause, Some(RunCause::ResourceLimit));
        assert_eq!(turns.technical, TechnicalState::Valid);
        assert_eq!(turns.completion, CompletionState::TaskIncomplete);
        assert_eq!(turns.score, Some(30));
        let overflow = failed(
            RunStatus::SubjectError,
            FailurePhase::Execute,
            "subject_execution_failed",
            "turn ended as Failed: The conversation is too large for the selected model.",
        );
        assert_eq!(overflow.cause, Some(RunCause::ContextOverflow));
        // An error code the classifier does not know leaves the status to decide.
        let mut budget = run(0, false);
        budget.evaluators = EvaluatorStates::default();
        budget.transcript = Some(serde_json::json!({"messages": [{"custom": {
            "custom_type": "error",
            "data": {"code": "harness.budget_exhausted", "summary": "token budget exhausted"}
        }}]}));
        budget.push_failure(
            RunStatus::ResourceLimit,
            FailurePhase::Execute,
            "turn ended as Failed: token budget exhausted",
        );
        assert_eq!(budget.cause, Some(RunCause::ResourceLimit));
        let transport = failed(
            RunStatus::InfrastructureError,
            FailurePhase::Execute,
            "provider_transport_failed",
            "provider transport failed, not the subject: turn ended as Failed: The provider is busy right now.",
        );
        assert_eq!(transport.cause, Some(RunCause::ProviderInfrastructure));
        let evaluator = failed(
            RunStatus::InfrastructureError,
            FailurePhase::Evaluate,
            "infrastructure_failed",
            "scenario 'registry_implementation' evaluator failed: validation incomplete: Cannot connect to the Docker daemon at unix:///var/run/docker.sock",
        );
        assert_eq!(evaluator.cause, Some(RunCause::EvaluationEnvironment));
        assert_eq!(evaluator.technical, TechnicalState::TechnicalInvalid);
        let cancelled = failed(
            RunStatus::SubjectError,
            FailurePhase::Execute,
            "execution_cancelled",
            EXECUTION_CANCELLED,
        );
        assert_eq!(cancelled.cause, Some(RunCause::OperatorCancelled));
        // A cancellation explains what failed before it, even a failure that
        // was not the subject's.
        let mut interrupted = transport.clone();
        interrupted.push_typed_failure(
            RunStatus::SubjectError,
            FailurePhase::Execute,
            "execution_cancelled",
            RetryScope::None,
            ContaminationScope::None,
            EXECUTION_CANCELLED,
        );
        assert_eq!(interrupted.cause, Some(RunCause::OperatorCancelled));
        // A later failure the subject did not cause takes a counted run out.
        let mut cleanup = turns.clone();
        cleanup.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Cleanup,
            "teardown failed",
        );
        assert_eq!(cleanup.cause, Some(RunCause::E2eInfrastructure));
        assert_eq!(cleanup.technical, TechnicalState::TechnicalInvalid);
        // Any other failure of the subject counts, as its own cause.
        let denied = failed(
            RunStatus::SubjectError,
            FailurePhase::Execute,
            "subject_execution_failed",
            "turn ended as Failed: pre_generate hook denied: tool budget",
        );
        assert_eq!(denied.cause, Some(RunCause::SubjectFailure));
        assert_eq!(denied.technical, TechnicalState::Valid);
        assert_eq!(denied.completion, CompletionState::TaskIncomplete);

        let report = aggregate(vec![
            run(90, true),
            turns,
            overflow,
            denied,
            transport,
            evaluator,
            cancelled,
        ]);
        assert_eq!(report.aggregate.technical_valid_runs, 4);
        assert_eq!(report.aggregate.mean_score, Some(45.0));
        assert_eq!(
            report.aggregate.causes,
            BTreeMap::from([
                (RunCause::Evaluated, 1),
                (RunCause::ResourceLimit, 1),
                (RunCause::ContextOverflow, 1),
                (RunCause::SubjectFailure, 1),
                (RunCause::ProviderInfrastructure, 1),
                (RunCause::EvaluationEnvironment, 1),
                (RunCause::OperatorCancelled, 1),
            ])
        );
    }

    /// A session keeps one error entry per failed turn; the failure reports
    /// the last turn, so the last entry with its summary decides.
    #[test]
    fn the_last_error_entry_of_the_failed_turn_decides() {
        let entry = |code: &str, detail: &str| {
            serde_json::json!({"custom": {"custom_type": "error", "data": {
                "code": code, "class": "llm.permanent",
                "summary": "The provider rejected this request.", "detail": detail}}})
        };
        let mut run = run(0, false);
        run.evaluators = EvaluatorStates::default();
        run.transcript = Some(serde_json::json!({"messages": [
            entry("router/provider_rejected", r#"{"error":{"message":"Insufficient Balance","type":"unknown_error","param":null,"code":"invalid_request_error"}}"#),
            entry("provider/invalid_request", "remote error (provider/invalid_request): bad ProviderStreamInput: data did not match any variant of untagged enum AgentMessage"),
        ]}));
        run.push_failure(
            RunStatus::SubjectError,
            FailurePhase::Execute,
            "turn ended as Failed: The provider rejected this request.",
        );
        assert_eq!(run.cause, Some(RunCause::InvalidProviderRequest));
    }

    #[test]
    fn a_report_without_causes_keeps_the_earlier_rule_and_says_so() {
        let output = tempfile::tempdir().unwrap();
        let mut overflow = run(0, false);
        overflow.run_id = "overflow".into();
        overflow.attempt_id = "overflow-attempt".into();
        overflow.score = None;
        overflow.evaluators = EvaluatorStates::default();
        overflow.push_failure(
            RunStatus::SubjectError,
            FailurePhase::Execute,
            "turn ended as Failed: The conversation is too large for the selected model.",
        );
        assert_eq!(overflow.technical, TechnicalState::Valid);
        // As written before causes.
        let mut runs = vec![run(80, true), overflow];
        for run in &mut runs {
            run.cause = None;
        }
        let mut result = report(vec![aggregate(runs)]);
        let path = result.write_to(output.path(), &manifest()).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!written.contains("\"cause\"") && !written.contains("\"causes\""));

        let (restored, _) = E2eReport::read_from(&path).unwrap();
        let scenario = &restored.scenarios[0];
        assert_eq!(scenario.runs[1].cause, None);
        // The subject error invalidated the run, as it did then.
        assert_eq!(scenario.runs[1].technical, TechnicalState::TechnicalInvalid);
        assert_eq!(scenario.aggregate.mean_score, Some(80.0));
        assert!(scenario.aggregate.causes.is_empty());
        assert_eq!(restored.runs_without_cause(), 2);
        assert!(restored.summary(false).contains(EARLIER_RULE));
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
            0,
            serde_json::json!({ "variant": "canonical" }),
            Vec::new(),
            DeliverableContract::default(),
        )
        .unwrap()
        .sealed_for_tests();
        let report = E2eScenarioReport::aggregate_case_with_planned(
            case,
            ExecutionPolicy {
                max_turns: Some(1),
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
        assert_eq!(report.aggregate.mean_score, Some(65.0));
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
        for primary in [RunStatus::HardGateFailed, RunStatus::ResourceLimit] {
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
                assert_eq!(aggregated.aggregate.technical_invalid_runs, 1);
                // A technically invalid run keeps its measured points but never counts as scored.
                assert_eq!(aggregated.aggregate.scored_runs, 0);
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
        assert_eq!(aggregated.runs[0].score, Some(0));
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
        assert_eq!(completed.score, Some(90));
    }

    #[test]
    fn profile_measurements_preserve_replications_and_reject_duplicate_evidence() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let mut one = report(vec![aggregate(vec![run(100, true)])]);
        let first_path = one.write_to(first.path(), &manifest()).unwrap();
        let mut second_run = run(0, false);
        second_run.run_id = "second-run".into();
        second_run.attempt_id = "second-attempt".into();
        let mut two = report(vec![aggregate(vec![second_run])]);
        let second_path = two.write_to(second.path(), &manifest()).unwrap();
        let measured = crate::test_plan::measure(&[first_path.clone(), second_path]).unwrap();
        assert_eq!(measured["cohorts"].as_array().unwrap().len(), 1);
        assert_eq!(measured["cohorts"][0]["aggregate"]["planned_runs"], 2);
        assert_eq!(measured["cohorts"][0]["aggregate"]["observed_runs"], 2);
        assert_eq!(measured["cohorts"][0]["consumption"]["observed_runs"], 2);
        assert_eq!(measured["interpretation"], "descriptive_only");
        assert!(crate::test_plan::measure(&[first_path.clone(), first_path]).is_err());
        let repeated = tempfile::tempdir().unwrap();
        let repeated_path = one.write_to(repeated.path(), &manifest()).unwrap();
        assert!(
            crate::test_plan::measure(&[first.path().join("results.json"), repeated_path]).is_err()
        );
    }

    #[test]
    fn wholly_deferred_scenario_has_no_fabricated_case_or_run() {
        let mut deferred = E2eScenarioReport::deferred(
            "unmaterialized".into(),
            "requested-case".into(),
            aggregate(Vec::new()).execution_policy,
            3,
            "scenario materialization failed".into(),
        );
        deferred.refresh_aggregate().unwrap();
        assert!(deferred.case.is_none());
        assert!(deferred.runs.is_empty());
        assert!(deferred.behavior_sha256.is_none());
        assert_eq!(deferred.case_id, "requested-case");
        assert_eq!(deferred.aggregate.planned_runs, 3);
        assert_eq!(deferred.aggregate.observed_runs, 0);
        assert_eq!(deferred.aggregate.deferred_runs, 3);
        assert_eq!(deferred.aggregate.total_tokens_consumed, Some(0));
        assert_eq!(deferred.aggregate.completion_rate, None);
        assert_eq!(deferred.aggregate.tokens_per_completion, None);
        assert_eq!(deferred.aggregate.mean_score, None);
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
                policy,
                planned,
                reason.into(),
            );
            assert!(deferred.refresh_aggregate().is_err());
        }
        let mut deferred = E2eScenarioReport::deferred(
            "unmaterialized".into(),
            "requested-case".into(),
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
    fn a_scenario_result_carries_its_card_and_older_results_still_read() {
        let mut scenario = E2eScenarioReport::deferred(
            "persistent_state".into(),
            "persistent_state@1".into(),
            aggregate(Vec::new()).execution_policy,
            1,
            "not run".into(),
        );
        scenario.refresh_aggregate().unwrap();
        let written = serde_json::to_value(&scenario).unwrap();
        for (field, text) in [
            ("title", ScenarioId::PersistentState.title()),
            ("summary", ScenarioId::PersistentState.summary()),
            ("protects", ScenarioId::PersistentState.protects()),
        ] {
            assert!(text.is_some(), "{field}");
            assert_eq!(written[field].as_str(), text, "{field}");
        }
        assert_eq!(written["workers"], serde_json::json!(["state"]));
        assert_eq!(written["area"], "data-safety");
        assert!(written.get("disciplines").is_none());

        let mut older = written;
        for field in ["protects", "workers", "area"] {
            older.as_object_mut().unwrap().remove(field);
        }
        let older: E2eScenarioReport = serde_json::from_value(older).unwrap();
        assert_eq!(older.protects, None);
        assert_eq!(older.tags, Default::default());
    }

    #[test]
    fn deferred_aggregate_rejects_invented_measurements() {
        let deferred = E2eScenarioReport::deferred(
            "unmaterialized".into(),
            "requested-case".into(),
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
        assert_eq!(report.scenarios[0].aggregate.mean_score, Some(90.0));
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
    fn numeric_scores_do_not_change_run_outcomes() {
        let mut outvoted = run(45, false);
        outvoted.status = RunStatus::Passed;
        let report = aggregate(vec![outvoted, run(90, true), run(90, true)]);
        assert!(report.passed);
        assert_eq!(report.aggregate.mean_score, Some(75.0));

        let mut decisive = run(45, false);
        decisive.status = RunStatus::Passed;
        let report = aggregate(vec![decisive, run(90, true)]);
        assert!(report.passed);
        assert_eq!(report.aggregate.mean_score, Some(67.5));
    }

    #[test]
    fn report_contains_current_execution_shape() {
        let report = report(vec![aggregate(vec![run(90, true)])]);
        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["scenarios"][0]["aggregate"]["mean_score"], 90.0);
        assert!(value["scenarios"][0].get("threshold").is_none());
        assert_eq!(value["scenarios"][0]["runs"][0]["status"], "passed");
        assert!(value["scenarios"][0]["aggregate"]["cost"].is_object());
    }

    #[test]
    fn criterion_description_is_optional_for_existing_reports() {
        let criterion: CriterionReport = serde_json::from_value(serde_json::json!({
            "id": "legacy",
            "possible": 20,
            "awarded": 0,
            "reason": "measured evidence"
        }))
        .unwrap();

        assert!(criterion.description.is_none());
        assert!(serde_json::to_value(criterion)
            .unwrap()
            .get("description")
            .is_none());
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

        attempt.update_efficiency();

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
        assert_eq!(efficiency.total_tokens, Some(1_500));
        assert!(efficiency.unavailable.contains_key("critical_path_ms"));
        // No cache figures: unavailable, never a zero.
        assert_eq!(efficiency.processed_prompt_tokens, None);
        assert!(efficiency
            .unavailable
            .contains_key("processed_prompt_tokens"));
        assert!(efficiency.unavailable.contains_key("peak_context_tokens"));
    }

    #[test]
    fn context_is_measured_per_step_with_its_cache_and_compactions() {
        let usage = |input: u64, read: u64| {
            serde_json::json!({"message": {"role": "assistant", "content": [],
                "usage": {"input": input, "cache_read": read, "output": 10}}})
        };
        let compaction =
            serde_json::json!({"custom": {"custom_type": "compaction", "data": {"summary": "…"}}});
        let session = |depth: u32,
                       digest: &str,
                       billed: u64,
                       counted: u64,
                       estimator: &str,
                       compacted: bool| {
            serde_json::json!({
                "session_id": format!("session-{depth}"), "depth": depth, "turns": 3,
                "function_calls": 0, "function_call_errors": 0,
                "input_tokens": 300, "cache_read_tokens": 6_000,
                "context": {"usable": 596_000, "total": counted, "compacted": compacted,
                    "prompt_surface_digest": digest, "estimator": estimator, "usage": {"input": 10, "cache_read": billed - 10}}
            })
        };
        let build = |child_counted: u64,
                     estimator: &str,
                     compacted: bool|
         -> SessionMetricsResponse {
            serde_json::from_value(serde_json::json!({
                "root_session_id": "session-0", "complete": true,
                "totals": {"sessions": 2, "turns": 6, "function_calls": 0, "function_call_errors": 0,
                    "input_tokens": 600, "output_tokens": 60, "cache_read_tokens": 12_000},
                "by_session": [session(0, "sha256:root", 3_100, 3_000, estimator, compacted),
                    session(1, "sha256:child", 2_000, child_counted, estimator, compacted)]
            }))
            .unwrap()
        };
        let metrics = |child_counted: u64| build(child_counted, "heuristic", true);
        let mut attempt = run(100, true);
        attempt.metrics = Some(metrics(2_000));
        attempt.transcript = Some(serde_json::json!({"messages": [
            usage(100, 1_000), compaction.clone(), usage(100, 2_000), compaction,
            usage(100, 3_000),
        ]}));
        attempt.update_efficiency();
        let efficiency = attempt.efficiency.clone().unwrap();
        assert_eq!(efficiency.processed_prompt_tokens, Some(12_600));
        assert_eq!(efficiency.cache_read_tokens, Some(12_000));
        // The steps add up to the root session's 300 + 6,000.
        assert_eq!(efficiency.peak_context_tokens, Some(3_100));
        assert_eq!(efficiency.usable_context_tokens, Some(596_000));
        // Two compaction records, whatever the snapshot's `compacted` says.
        assert_eq!(efficiency.compactions, Some(2));
        assert_eq!(
            efficiency.prompt_surface_digests,
            BTreeMap::from([
                ("child".into(), vec!["sha256:child".into()]),
                ("root".into(), vec!["sha256:root".into()]),
            ])
        );
        assert_eq!(efficiency.cache_comparable, Some(true));

        // A provider that bills a prompt the Harness never sent.
        attempt.metrics = Some(metrics(1_500));
        attempt.update_efficiency();
        assert_eq!(
            attempt.efficiency.clone().unwrap().cache_comparable,
            Some(false)
        );

        // A provider-counted total is the billed value: nothing to judge.
        attempt.metrics = Some(build(1_500, "provider", true));
        attempt.update_efficiency();
        let efficiency = attempt.efficiency.clone().unwrap();
        assert_eq!(efficiency.cache_comparable, None);
        assert!(efficiency.unavailable.contains_key("cache_comparable"));

        // Without custom entries and without a compacted snapshot, compaction
        // is unknown, not zero.
        attempt.metrics = Some(build(2_000, "heuristic", false));
        attempt.transcript = Some(serde_json::json!({"messages": [usage(100, 6_000)]}));
        attempt.update_efficiency();
        let efficiency = attempt.efficiency.unwrap();
        assert_eq!(efficiency.compactions, None);
        assert!(efficiency.unavailable.contains_key("compactions"));

        // Steps that do not add up give no peak rather than a wrong one.
        let mut short = run(100, true);
        short.metrics = Some(metrics(2_000));
        short.transcript = Some(serde_json::json!({"messages": [usage(100, 1_000)]}));
        short.update_efficiency();
        let efficiency = short.efficiency.unwrap();
        assert_eq!(efficiency.peak_context_tokens, None);
        assert!(efficiency.unavailable.contains_key("peak_context_tokens"));
    }

    #[test]
    fn retry_efficiency_and_cost_include_every_technical_attempt() {
        let mut failed = run(0, false);
        failed.status = RunStatus::SubjectError;
        failed.wall_time_ms = 500;
        failed.cost.total_usd = Some(0.10);
        failed.metrics = Some(metrics());
        failed.terminal_status = Some(status(1, 0));
        failed.update_efficiency();

        let mut passed = run(100, true);
        passed.wall_time_ms = 1_000;
        passed.cost.total_usd = Some(0.20);
        passed.metrics = Some(metrics());
        passed.terminal_status = Some(status(2, 1));
        passed.update_efficiency();
        passed.attach_retry_attempts(vec![RetryAttemptReport::from(&failed)]);

        let aggregate = aggregate(vec![passed.clone()]);
        let efficiency = passed.efficiency.unwrap();
        assert_eq!(efficiency.technical_attempts, 2);
        assert_eq!(efficiency.total_tokens, Some(3_000));
        assert_eq!(efficiency.observed_work, Some(33));
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
        failed.criteria.push(CriterionReport {
            id: "correctness".into(),
            description: None,
            label: None,
            possible: 100,
            awarded: Some(50),
            reason: "only half of the expected result was present".into(),
            gate: false,
            gate_minimum: None,
        });
        let report = report(vec![aggregate(vec![failed])]);

        let summary = report.summary(false);
        assert!(summary.contains("Harness E2E: subject="));
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
            outcome: AssessmentOutcome::Passed,
            score: Some(AssessmentScore {
                awarded: 100,
                possible: 100,
            }),
            summary: "deterministic evidence passed".into(),
            evidence: Vec::new(),
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
            value.get("result_contract_sha256"),
            Some(&serde_json::json!(RESULT_CONTRACT_SHA256))
        );
        assert!(value.get("assessment_contract").is_some());
        assert!(value["scenarios"][0]["runs"][0].get("attempt_id").is_some());
    }

    #[test]
    fn write_preserves_verified_runtime_diagnostics_without_a_transcript() {
        let output = tempfile::tempdir().unwrap();
        let mut run = run(0, false);
        let reference = artifact::write_json(
            output.path(),
            Path::new("evidence/kanban-controller.json"),
            "kanban_controller",
            "runtime_diagnostics",
            &serde_json::json!({"status":"infrastructure_failed"}),
        )
        .unwrap();
        run.evidence.push(reference.clone());
        let mut report = report(vec![aggregate(vec![run])]);
        report.write_to(output.path(), &manifest()).unwrap();
        let (decoded, _) = E2eReport::read_from(output.path()).unwrap();
        assert_eq!(
            decoded.scenarios[0].runs[0].evidence,
            vec![reference.clone()]
        );
        reference.verify(output.path()).unwrap();
        fs::write(output.path().join(&reference.path), "tampered").unwrap();
        assert!(report.write_to(output.path(), &manifest()).is_err());
    }

    #[test]
    fn a_credential_the_subject_echoed_is_redacted_before_anything_is_hashed() {
        const KEY: &str = "sk-echoed-0123456789";
        crate::redaction::with_test_credentials(&[("OPENAI_API_KEY", KEY)], || {
            let output = tempfile::tempdir().unwrap();
            let mut run = run(90, true);
            run.prompt = format!("cat .env printed OPENAI_API_KEY={KEY}");
            // Referenced by digest from results.json, as a transcript is.
            let reference = artifact::write_json(
                output.path(),
                Path::new("evidence/run/attempt/transcript.json"),
                "transcript",
                "transcript",
                &serde_json::json!({"tool_output": format!("OPENAI_API_KEY={KEY}\n")}),
            )
            .unwrap();
            run.evidence.push(reference);
            let mut report = report(vec![aggregate(vec![run])]);
            report.write_to(output.path(), &manifest()).unwrap();
            // Every digest still holds, as import and aggregation check them.
            let (decoded, _) = E2eReport::read_from(output.path()).unwrap();
            assert!(decoded.scenarios[0].runs[0]
                .prompt
                .starts_with("cat .env printed OPENAI_API_KEY=["));
            let mut folders = vec![output.path().to_owned()];
            while let Some(folder) = folders.pop() {
                for entry in fs::read_dir(folder).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        folders.push(path);
                    } else {
                        let text = fs::read_to_string(&path).unwrap();
                        assert!(!text.contains(KEY), "{} holds the key", path.display());
                    }
                }
            }
        });
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
    }

    #[test]
    fn score_is_the_plain_sum_of_evaluated_criteria() {
        let criterion = |awarded: Option<u8>, possible: u8| CriterionReport {
            id: format!("c{possible}"),
            description: None,
            label: None,
            possible,
            awarded,
            reason: "observed".into(),
            gate: false,
            gate_minimum: None,
        };
        // An unevaluated criterion adds nothing and does not null the score.
        assert_eq!(
            criteria_score(&[
                criterion(Some(40), 40),
                criterion(None, 30),
                criterion(Some(10), 30)
            ]),
            Some(50)
        );
        // Nothing evaluated: no score, not zero.
        assert_eq!(
            criteria_score(&[criterion(None, 60), criterion(None, 40)]),
            None
        );
        assert_eq!(criteria_score(&[]), None);
        // Zero points awarded is a score of zero.
        assert_eq!(criteria_score(&[criterion(Some(0), 100)]), Some(0));
        // The sum never exceeds the hundred-point rubric.
        assert_eq!(
            criteria_score(&[criterion(Some(80), 80), criterion(Some(80), 80)]),
            Some(100)
        );
    }

    #[test]
    fn completion_is_read_from_the_gate_criteria() {
        let criterion = |gate: bool, awarded: Option<u8>, possible: u8| CriterionReport {
            id: format!("c{possible}"),
            description: None,
            label: None,
            possible,
            awarded,
            reason: "observed".into(),
            gate,
            gate_minimum: None,
        };
        // A proportional gate passes at its minimum: the purpose was met.
        let mut goal_met = criterion(true, Some(20), 35);
        goal_met.gate_minimum = Some(20);
        assert_eq!(
            gate_completion(&[goal_met.clone()]),
            CompletionState::Completed
        );
        goal_met.awarded = Some(19);
        assert_eq!(
            gate_completion(&[goal_met]),
            CompletionState::TaskIncomplete
        );
        // Every gate at full points completes, whatever the other criteria scored.
        assert_eq!(
            gate_completion(&[criterion(true, Some(60), 60), criterion(false, Some(0), 40)]),
            CompletionState::Completed
        );
        // A gate short of its full points leaves the task incomplete.
        assert_eq!(
            gate_completion(&[
                criterion(true, Some(59), 60),
                criterion(false, Some(40), 40)
            ]),
            CompletionState::TaskIncomplete
        );
        // A gate never reached: incomplete after another failure, else undetermined.
        assert_eq!(
            gate_completion(&[criterion(true, None, 60), criterion(false, Some(0), 40)]),
            CompletionState::TaskIncomplete
        );
        assert_eq!(
            gate_completion(&[criterion(true, None, 60), criterion(false, Some(40), 40)]),
            CompletionState::Undetermined
        );
        assert_eq!(
            gate_completion(&[criterion(false, Some(100), 100)]),
            CompletionState::Undetermined
        );
    }

    #[test]
    fn read_keeps_a_foreign_result_contract_and_rejects_a_missing_one() {
        let output = tempfile::tempdir().unwrap();
        let mut report = report(vec![aggregate(vec![run(100, true)])]);
        let path = report.write_to(output.path(), &manifest()).unwrap();
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let foreign = format!("sha256:{}", "f".repeat(64));
        value["result_contract_sha256"] = serde_json::json!(foreign);
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        // Another contract is a warning, not a refusal: the report keeps saying
        // which contract it was written under.
        let (decoded, _) = E2eReport::read_from(&path).unwrap();
        assert_eq!(decoded.result_contract_sha256, foreign);

        value
            .as_object_mut()
            .unwrap()
            .remove("result_contract_sha256");
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        let error = E2eReport::read_from(&path).unwrap_err();
        assert!(format!("{error:#}").contains("decode typed E2E report"));
    }

    #[test]
    fn workflow_runtime_checks_do_not_become_objective_approvals() {
        let step = WorkflowStepReport {
            node_id: "assess".into(),
            step_type: "test.assess".into(),
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
        assert_eq!(contract.runs[0].assessments.len(), 1);
        assert_eq!(
            contract.runs[0].assessments[0].outcome,
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
            7,
            serde_json::json!({ "status": "ready" }),
            vec![Capability::IiiState],
            contract,
        )
        .unwrap()
        .sealed_for_tests();
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
                max_turns: Some(1),
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
            7,
            serde_json::json!({ "status": "ready" }),
            vec![],
            contract,
        )
        .unwrap()
        .sealed_for_tests();
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
                max_turns: Some(1),
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
            asset.outcome,
            crate::assessment::AssetValidationOutcome::RemovedDuringCleanup
        );
        assert_eq!(asset.evidence.len(), 1);
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
            serde_json::json!({}),
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
        .unwrap()
        .sealed_for_tests();
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
            agent: None,
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
                points: None,
                passed: Some(true),
                signals: serde_json::json!({}),
            },
            DimensionReport {
                dimension: EvaluationDimension::StructuralIntegrity,
                points: None,
                passed: Some(true),
                signals: serde_json::json!({}),
            },
        ];
        report
    }

    #[test]
    fn a_deliverable_hash_mismatch_names_everything_needed_to_tell_the_causes_apart() {
        let output = tempfile::tempdir().unwrap();
        let dir = output.path().join("deliverables");
        std::fs::create_dir_all(&dir).unwrap();
        let bytes = br#"{"observations":[]}"#;
        std::fs::write(dir.join("evidence.json"), bytes).unwrap();

        let deliverable = DeliverableReport {
            id: "linkly_evidence".into(),
            kind: "application_audit".into(),
            media_type: "application/json".into(),
            content_format: DeliverableContentFormat::Json,
            // A hash the artifact never had: the case the message has to explain.
            content_sha256:
                "sha256:0000000000000000000000000000000000000000000000000000000000000000".into(),
            content_size_bytes: bytes.len() as u64,
            schema_valid: true,
            provenance_valid: true,
            invariants: Vec::new(),
            provenance: Vec::new(),
            preview: serde_json::json!({}),
            screenshots: Vec::new(),
            artifact: Some(crate::artifact::ArtifactReference {
                id: "linkly_evidence".into(),
                kind: "application_audit".into(),
                path: "deliverables/evidence.json".into(),
                sha256: crate::artifact::sha256_bytes(bytes),
                size_bytes: bytes.len() as u64,
                media_type: "application/json".into(),
            }),
            content: CapturedDeliverableContent::Json(serde_json::json!({})),
        };

        let message = format!(
            "{:#}",
            verify_deliverables(output.path(), std::slice::from_ref(&deliverable)).unwrap_err()
        );
        for needle in [
            "linkly_evidence",
            "deliverables/evidence.json",
            "sha256:0000",
            &crate::artifact::sha256_bytes(bytes),
            &format!("{} bytes", bytes.len()),
        ] {
            assert!(
                message.contains(needle),
                "the mismatch must name {needle}, got:\n{message}"
            );
        }
    }
}
