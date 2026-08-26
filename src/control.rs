use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use iii_sdk::errors::Error;
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::{IIIClient, RegisterFunction};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc, watch, Mutex, RwLock};

use crate::artifact::{self, ArtifactReference};
use crate::context::E2eContext;
use crate::durable::{
    ArchiveHeadResponse, ArchiveResponse, ArchiveRestoreResponse, DurableArchiveReference,
    DurableHistory, HistoryListRequest, RetentionClass, RetentionSweepRequest, ARCHIVE_HEAD_ID,
    ARCHIVE_ID, ARCHIVE_RESTORE_ID, HISTORY_LIST_ID, RETENTION_SWEEP_ID,
};
use crate::fault::{
    ExpectedTerminalOutcome, FaultEvaluation, FaultJournal, FaultPlan, FaultProfile,
};
use crate::judge::JudgeConfig;
use crate::longitudinal::{self, ComparisonPolicy, ComparisonResponse};
use crate::markdown::{
    MarkdownScenarioSource, ScenarioKey, LOCAL_SCENARIO_DIRECTORY, LOCAL_SCENARIO_MAX_BYTES,
    LOCAL_SCENARIO_PLAN_ID, LOCAL_SCENARIO_REQUIRED_SECTIONS, LOCAL_SCENARIO_TEMPLATE,
};
use crate::report::{
    E2eManifest, E2eObservationEnvelope, E2eReport, ObservationDataAvailability,
    ObservationEvidence, ObservationExecutionIdentity, ObservationIdentity, ObservationMetric,
    ObservationMetricDerivation, ObservationMetricOrigin, ObservationObjective, ObservationOutcome,
    ObservationProvenance, ObservationRunContract, ObservationSample, ObservationSelectedCase,
    RunnerIdentity, CATALOG_SCHEMA, OBSERVATION_SCHEMA,
};
#[cfg(test)]
use crate::scenarios::ScenarioId;
use crate::scenarios::{
    scenario_contract_sha256, ComplexityClassification, ComplexityProfile, DeliverableContract,
    ExecutionPolicy, ScenarioCharacterization,
};
use crate::suite::{
    run_suite, AdaptiveResumeAttempt, SubjectConfig, SuiteControl, SuiteEvent, SuiteEventEnvelope,
    SuitePhase, SuiteRunConfig,
};

pub const CONTROL_CONTRACT_NAME: &str = "e2e-control-plane";
pub const RUN_ID: &str = "e2e::run";
pub const STATUS_ID: &str = "e2e::status";
pub const CANCEL_ID: &str = "e2e::cancel";
pub const RESULTS_GET_ID: &str = "e2e::results-get";
pub const RESULTS_LIST_ID: &str = "e2e::results-list";
pub const COMPARE_ID: &str = "e2e::compare";
pub const SCENARIOS_LIST_ID: &str = "e2e::scenarios-list";
pub const SCENARIOS_CREATE_ID: &str = "e2e::scenarios-create";
pub const SCENARIOS_AUTHORING_GUIDE_ID: &str = "e2e::scenarios-authoring-guide";
pub const FAULT_PLAN_ID: &str = "e2e::fault-plan";
pub const FAULT_EVALUATE_ID: &str = "e2e::fault-evaluate";

const RECORD_SCOPE: &str = "harness_e2e_execution";
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;
const MAX_CONCURRENT_EXECUTIONS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPhase {
    Requested,
    Admitted,
    Preflighting,
    Materializing,
    SettingUp,
    Executing,
    Collecting,
    Evaluating,
    Persisting,
    CleaningUp,
    Finalizing,
    Completed,
    Failed,
    Cancelled,
    NeedsReconciliation,
    Unsupported,
}

impl ExecutionPhase {
    fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Failed
                | Self::Cancelled
                | Self::NeedsReconciliation
                | Self::Unsupported
        )
    }
}

impl From<SuitePhase> for ExecutionPhase {
    fn from(value: SuitePhase) -> Self {
        match value {
            SuitePhase::Preflighting => Self::Preflighting,
            SuitePhase::Materializing => Self::Materializing,
            SuitePhase::SettingUp => Self::SettingUp,
            SuitePhase::Executing => Self::Executing,
            SuitePhase::Collecting => Self::Collecting,
            SuitePhase::Evaluating => Self::Evaluating,
            SuitePhase::Persisting => Self::Persisting,
            SuitePhase::CleaningUp => Self::CleaningUp,
            SuitePhase::Finalizing => Self::Finalizing,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PhaseTransition {
    pub phase: ExecutionPhase,
    pub at: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ActiveAttempt {
    pub scenario_id: ScenarioKey,
    #[serde(default)]
    pub run_id: String,
    pub attempt_id: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_state_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_state_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRecord {
    pub execution_id: String,
    pub idempotency_key: String,
    pub phase: ExecutionPhase,
    pub requested_at: String,
    pub updated_at: String,
    pub request: RunRequest,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_contract_sha256: Option<String>,
    pub lane_budget: LaneBudget,
    pub transitions: Vec<PhaseTransition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_attempt: Option<ActiveAttempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_state_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_state_sha256: Option<String>,
    pub cancel_requested: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<E2eReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<E2eManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<E2eObservationEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_artifact: Option<ArtifactReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<DurableArchiveReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunRequest {
    // iii injects this routing metadata for worker-to-worker invocations.
    // It is accepted at the wire boundary but is never persisted or hashed.
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    pub(super) _caller_worker_id: Option<String>,
    pub idempotency_key: String,
    #[serde(default)]
    pub label: String,
    #[serde(default = "default_lane")]
    pub lane: String,
    pub model: String,
    pub provider: String,
    #[serde(default)]
    pub judge_model: Option<String>,
    #[serde(default)]
    pub judge_provider: Option<String>,
    /// Opt-in behavioral audit analyzer; supply model and provider together.
    #[serde(default)]
    pub audit_model: Option<String>,
    #[serde(default)]
    pub audit_provider: Option<String>,
    #[serde(default)]
    pub scenarios: Vec<ScenarioKey>,
    /// Immutable local Markdown definitions resolved by the control plane at
    /// admission time. This is persisted for restart-safe execution and is
    /// not part of the public function schema.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(skip)]
    pub local_markdown_scenarios: Vec<MarkdownScenarioSource>,
    #[serde(default = "default_runs")]
    pub runs: u32,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub rotating_seeds: Vec<u64>,
    #[serde(default = "default_technical_retries")]
    pub technical_retries: u8,
    #[serde(default = "default_progress_interval_seconds")]
    pub progress_interval_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_contract: Option<ObservationRunContract>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LaneBudget {
    pub max_cases: u16,
    pub max_runs_per_case: u32,
    pub max_technical_retries: u8,
    pub max_declared_turns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunAccepted {
    pub execution_id: String,
    pub phase: ExecutionPhase,
    pub duplicate: bool,
    pub request_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_contract_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionRequest {
    pub execution_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StatusResponse {
    pub execution_id: String,
    pub phase: ExecutionPhase,
    pub terminal: bool,
    pub cancel_requested: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_attempt: Option<ActiveAttempt>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    pub updated_at: String,
    pub transitions: Vec<PhaseTransition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CancelResponse {
    pub execution_id: String,
    pub phase: ExecutionPhase,
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResultsGetResponse {
    pub execution_id: String,
    pub phase: ExecutionPhase,
    pub result_path: Option<String>,
    pub report: Option<E2eReport>,
    pub manifest: Option<E2eManifest>,
    pub archive: Option<DurableArchiveReference>,
    pub request_sha256: String,
    pub run_contract_sha256: Option<String>,
    pub observation: Option<E2eObservationEnvelope>,
    pub observation_artifact: Option<ArtifactReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveExecutionRequest {
    pub execution_id: String,
    pub retention_class: RetentionClass,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FaultPlanRequest {
    pub profile: FaultProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FaultEvaluateRequest {
    pub execution_id: String,
    pub profile: FaultProfile,
    pub plan: FaultPlan,
    pub journal: FaultJournal,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ResultsListRequest {
    #[serde(default)]
    pub lane: Option<String>,
    #[serde(default)]
    pub scenario_id: Option<ScenarioKey>,
    #[serde(default = "default_results_limit")]
    pub limit: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResultsListResponse {
    pub executions: Vec<StatusResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompareRequest {
    pub from_execution_id: String,
    pub to_execution_id: String,
    /// Explicit policy override. Omit to gate on the reviewed baseline.
    #[serde(default)]
    pub policy: Option<ComparisonPolicy>,
    /// Reviewed baseline file to load thresholds from. Omit to use the
    /// checked-in `config/baselines/default.json` when it exists, falling back
    /// to the code-default thresholds. Ignored when `policy` is set.
    #[serde(default)]
    pub baseline: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScenariosListRequest {
    /// Optional deterministic seed used to materialize each listed test case.
    /// Omit it to use each scenario's canonical seed.
    #[serde(default)]
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioAuthoringGuideRequest {
    // iii injects this routing metadata for worker and CLI invocations.
    // Accept it at the wire boundary without exposing or persisting it.
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    _caller_worker_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScenarioAuthoringGuideResponse {
    /// Versioned shape of this guidance response.
    pub schema: String,
    /// Short statement of what local authoring is for.
    pub summary: String,
    /// Always false: persistence and execution are separate operations.
    pub creation_starts_execution: bool,
    /// Existing files are never silently replaced.
    pub overwrites_existing_file: bool,
    /// Function that returns the complete built-in and local scenario catalog.
    pub list_function: String,
    /// Function that validates and persists one new local definition.
    pub create_function: String,
    /// Separate function used only when the user explicitly asks to execute.
    pub run_function: String,
    /// Worker-data-relative directory where definitions are persisted.
    pub storage_directory: String,
    /// Maximum accepted UTF-8 Markdown source size.
    pub max_source_bytes: usize,
    /// File-name requirements enforced by the worker.
    pub file_name_rules: Vec<String>,
    /// Required H2 headings, in their exact order.
    pub required_h2_sections: Vec<String>,
    /// Only accepted entry under the Plans heading for local definitions.
    pub required_plan: String,
    /// Rules for weighted H3 validation criteria.
    pub validation_rules: Vec<String>,
    /// Recommended safe sequence for an agent authoring a local test.
    pub workflow: Vec<String>,
    /// Copy-ready valid Markdown source.
    pub template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LocalScenarioCreateRequest {
    // iii injects this routing metadata for Console-to-worker invocations.
    // Accept it at the wire boundary without exposing or persisting it.
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    _caller_worker_id: Option<String>,
    /// One UTF-8 `.md` file name, without a directory. Use letters, numbers,
    /// spaces, hyphens, or underscores; its stem becomes `local_<safe_id>`.
    /// Existing files are rejected instead of overwritten.
    pub file_name: String,
    /// Complete Markdown test definition. It must contain one H1 followed by
    /// the required H2 sections in order, use only `- local` under Plans, and
    /// contain positive `### Name (N%)` validations totaling exactly 100%.
    pub source: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct LocalScenarioCreateResponse {
    pub scenario: ScenarioDescriptor,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScenarioDescriptor {
    pub scenario_id: ScenarioKey,
    pub origin: ScenarioOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plans: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiled_sha256: Option<String>,
    pub scenario_version: u32,
    pub case_id: String,
    pub seed: u64,
    pub inputs_sha256: String,
    pub contract_sha256: String,
    pub classification: ComplexityClassification,
    pub complexity: ComplexityProfile,
    pub characterization: ScenarioCharacterization,
    pub resource_envelope: ScenarioResourceEnvelope,
    pub required_capabilities: Vec<String>,
    pub deliverable_contract: DeliverableContract,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioOrigin {
    #[default]
    BuiltIn,
    Markdown,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScenarioResourceEnvelope {
    pub execution: ExecutionPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<WorkflowResourceEnvelope>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowResourceEnvelope {
    pub max_parallel: u16,
    pub max_nodes: u16,
    pub step_timeout_seconds: u64,
    pub workflow_timeout_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    pub technical_retries: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScenariosListResponse {
    pub schema: String,
    pub runner: RunnerIdentity,
    pub catalog_sha256: String,
    pub scenarios: Vec<ScenarioDescriptor>,
}

#[derive(Debug, Clone)]
pub struct ControlPlaneUpdate {
    pub record: ExecutionRecord,
}

#[derive(Clone)]
pub struct ControlPlane {
    inner: Arc<ControlPlaneInner>,
}

struct ControlPlaneInner {
    iii: IIIClient,
    url: String,
    output_root: PathBuf,
    admission: Mutex<()>,
    records: RwLock<HashMap<String, ExecutionRecord>>,
    cancellations: Mutex<HashMap<String, watch::Sender<bool>>>,
    scenario_lock: Mutex<()>,
    durable: DurableHistory,
    updates: broadcast::Sender<ControlPlaneUpdate>,
}

fn default_lane() -> String {
    "local".into()
}

const fn default_runs() -> u32 {
    1
}

const fn default_technical_retries() -> u8 {
    1
}

const fn default_progress_interval_seconds() -> u64 {
    15
}

const fn default_results_limit() -> u16 {
    50
}

impl ControlPlane {
    pub async fn new(iii: IIIClient, url: String, output_root: PathBuf) -> Result<Self> {
        let (updates, _) = broadcast::channel(256);
        let control = Self {
            inner: Arc::new(ControlPlaneInner {
                durable: DurableHistory::from_client(iii.clone()),
                iii,
                url,
                output_root,
                admission: Mutex::new(()),
                records: RwLock::new(HashMap::new()),
                cancellations: Mutex::new(HashMap::new()),
                scenario_lock: Mutex::new(()),
                updates,
            }),
        };
        control.restore().await?;
        Ok(control)
    }

    pub fn register(&self) {
        register_function(
            &self.inner.iii,
            FAULT_PLAN_ID,
            "Materialize a deterministic, versioned fault plan for a protected supervisor.",
            RegisterFunction::new_async(move |request: FaultPlanRequest| async move {
                request.profile.materialize().map_err(handler_error)
            }),
        );
        register_function(
            &self.inner.iii,
            FAULT_EVALUATE_ID,
            "Classify a protected supervisor's fault journal against canonical execution evidence.",
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: FaultEvaluateRequest| {
                    let control = control.clone();
                    async move { control.fault_evaluate(request).await.map_err(handler_error) }
                })
            },
        );
        register_function(
            &self.inner.iii,
            ARCHIVE_ID,
            "Persist a completed execution through private iii storage and idempotent history.",
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: ArchiveExecutionRequest| {
                    let control = control.clone();
                    async move { control.archive(request).await.map_err(handler_error) }
                })
            },
        );
        register_function(
            &self.inner.iii,
            ARCHIVE_HEAD_ID,
            "Verify availability and immutable metadata for an archived execution.",
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: ExecutionRequest| {
                    let control = control.clone();
                    async move {
                        control
                            .archive_head(&request.execution_id)
                            .await
                            .map_err(handler_error)
                    }
                })
            },
        );
        register_function(
            &self.inner.iii,
            ARCHIVE_RESTORE_ID,
            "Restore and verify an archived execution from its immutable manifest.",
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: ExecutionRequest| {
                    let control = control.clone();
                    async move {
                        control
                            .archive_restore(&request.execution_id)
                            .await
                            .map_err(handler_error)
                    }
                })
            },
        );
        register_function(
            &self.inner.iii,
            HISTORY_LIST_ID,
            "List hash-validated E2E history records from the iii database worker.",
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: HistoryListRequest| {
                    let control = control.clone();
                    async move {
                        control
                            .inner
                            .durable
                            .history_list(request)
                            .await
                            .map_err(handler_error)
                    }
                })
            },
        );
        register_function(
            &self.inner.iii,
            RETENTION_SWEEP_ID,
            "Delete expired E2E objects and tombstone their history records.",
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: RetentionSweepRequest| {
                    let control = control.clone();
                    async move {
                        control
                            .inner
                            .durable
                            .retention_sweep(request)
                            .await
                            .map_err(handler_error)
                    }
                })
            },
        );
        register_function(
            &self.inner.iii,
            RUN_ID,
            "Admit an asynchronous, idempotent E2E execution.",
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: RunRequest| {
                    let control = control.clone();
                    async move { control.run(request).await.map_err(handler_error) }
                })
            },
        );
        register_function(
            &self.inner.iii,
            STATUS_ID,
            "Read the durable phase and progress of an E2E execution.",
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: ExecutionRequest| {
                    let control = control.clone();
                    async move {
                        control
                            .status(&request.execution_id)
                            .await
                            .map_err(handler_error)
                    }
                })
            },
        );
        register_function(
            &self.inner.iii,
            CANCEL_ID,
            "Request idempotent cancellation and cleanup of an E2E execution.",
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: ExecutionRequest| {
                    let control = control.clone();
                    async move {
                        control
                            .cancel(&request.execution_id)
                            .await
                            .map_err(handler_error)
                    }
                })
            },
        );
        register_function(
            &self.inner.iii,
            RESULTS_GET_ID,
            "Get the immutable result and manifest of an E2E execution.",
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: ExecutionRequest| {
                    let control = control.clone();
                    async move {
                        control
                            .results_get(&request.execution_id)
                            .await
                            .map_err(handler_error)
                    }
                })
            },
        );
        register_function(
            &self.inner.iii,
            RESULTS_LIST_ID,
            "List durable E2E executions using comparable identity filters.",
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: ResultsListRequest| {
                    let control = control.clone();
                    async move { control.results_list(request).await.map_err(handler_error) }
                })
            },
        );
        register_function(
            &self.inner.iii,
            COMPARE_ID,
            "Compare two completed executions when their identities are eligible.",
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: CompareRequest| {
                    let control = control.clone();
                    async move { control.compare(request).await.map_err(handler_error) }
                })
            },
        );
        register_agent_function(
            &self.inner.iii,
            SCENARIOS_LIST_ID,
            "List every built-in, committed Markdown, and local E2E test with its immutable contract, origin, capabilities, complexity, and materialized case. Call e2e::scenarios-authoring-guide before authoring a new local test.",
            "list_tests",
            false,
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: ScenariosListRequest| {
                    let control = control.clone();
                    async move {
                        control
                            .scenario_catalog(request)
                            .await
                            .map_err(handler_error)
                    }
                })
            },
        );
        register_agent_function(
            &self.inner.iii,
            SCENARIOS_CREATE_ID,
            "Create one local Markdown E2E test after validating its complete authoring contract. This persists a definition outside Git and never starts an execution; call e2e::run separately only when execution is explicitly requested.",
            "create_local_test",
            true,
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: LocalScenarioCreateRequest| {
                    let control = control.clone();
                    async move {
                        control
                            .create_local_scenario(request)
                            .await
                            .map_err(handler_error)
                    }
                })
            },
        );
        register_agent_function(
            &self.inner.iii,
            SCENARIOS_AUTHORING_GUIDE_ID,
            "Explain how the Harness can author a local Markdown E2E test, including the exact template, validation rules, persistence behavior, and separate create/list/run workflow. Call this before e2e::scenarios-create.",
            "understand_local_test_authoring",
            false,
            RegisterFunction::new_async(
                move |_request: ScenarioAuthoringGuideRequest| async move {
                    Ok::<_, Error>(scenario_authoring_guide())
                },
            ),
        );
    }

    pub fn url(&self) -> &str {
        &self.inner.url
    }

    pub fn output_root(&self) -> &Path {
        &self.inner.output_root
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ControlPlaneUpdate> {
        self.inner.updates.subscribe()
    }

    pub async fn records(&self) -> Vec<ExecutionRecord> {
        let mut records = self
            .inner
            .records
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| right.requested_at.cmp(&left.requested_at));
        records
    }

    pub async fn scenario_catalog(
        &self,
        request: ScenariosListRequest,
    ) -> Result<ScenariosListResponse> {
        let _lock = self.inner.scenario_lock.lock().await;
        let output_root = self.inner.output_root.clone();
        tokio::task::spawn_blocking(move || scenarios_list_with_local(&output_root, request))
            .await
            .context("load local scenario catalog task")?
    }

    pub async fn create_local_scenario(
        &self,
        request: LocalScenarioCreateRequest,
    ) -> Result<LocalScenarioCreateResponse> {
        let _lock = self.inner.scenario_lock.lock().await;
        let output_root = self.inner.output_root.clone();
        let definition = tokio::task::spawn_blocking(move || {
            crate::markdown::create_local_scenario(
                &output_root,
                &request.file_name,
                &request.source,
            )
        })
        .await
        .context("create local scenario task")??;
        let seed = ScenarioKey::Markdown(definition.scenario.id.clone()).canonical_seed();
        Ok(LocalScenarioCreateResponse {
            scenario: materialize_markdown_descriptor(
                definition.scenario,
                seed,
                ScenarioOrigin::Local,
            )?,
        })
    }

    async fn resolve_local_markdown_scenarios(
        &self,
        scenarios: &[ScenarioKey],
        capture_catalog: bool,
    ) -> Result<Vec<MarkdownScenarioSource>> {
        let _lock = self.inner.scenario_lock.lock().await;
        let output_root = self.inner.output_root.clone();
        let scenarios = scenarios.to_vec();
        tokio::task::spawn_blocking(move || {
            resolve_local_markdown_scenarios(&output_root, &scenarios, capture_catalog)
        })
        .await
        .context("load local scenarios for execution task")?
    }

    pub async fn run(&self, mut request: RunRequest) -> Result<RunAccepted> {
        request.local_markdown_scenarios = self
            .resolve_local_markdown_scenarios(&request.scenarios, request.run_contract.is_some())
            .await?;
        let lane_budget = validate_run_request(&request)?;
        let request_sha256 = artifact::sha256_value(&request)?;
        let run_contract_sha256 = request
            .run_contract
            .as_ref()
            .map(artifact::sha256_value)
            .transpose()?;
        let _admission = self.inner.admission.lock().await;
        let execution_id = execution_id_for_key(&request.idempotency_key);
        if let Some(record) = self.inner.records.read().await.get(&execution_id).cloned() {
            if record.request != request {
                bail!(
                    "idempotency key '{}' already belongs to execution {} with a different request",
                    request.idempotency_key,
                    execution_id
                );
            }
            return Ok(RunAccepted {
                execution_id,
                phase: record.phase,
                duplicate: true,
                request_sha256: if record.request_sha256.is_empty() {
                    artifact::sha256_value(&record.request)?
                } else {
                    record.request_sha256
                },
                run_contract_sha256: record.run_contract_sha256,
            });
        }

        let active = self
            .inner
            .records
            .read()
            .await
            .values()
            .filter(|record| !record.phase.terminal())
            .count();
        if active >= MAX_CONCURRENT_EXECUTIONS {
            bail!("E2E concurrency limit of {MAX_CONCURRENT_EXECUTIONS} is reached");
        }

        let now = now();
        let record = ExecutionRecord {
            execution_id: execution_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            phase: ExecutionPhase::Requested,
            requested_at: now.clone(),
            updated_at: now.clone(),
            request: request.clone(),
            request_sha256: request_sha256.clone(),
            run_contract_sha256: run_contract_sha256.clone(),
            lane_budget,
            transitions: vec![PhaseTransition {
                phase: ExecutionPhase::Requested,
                at: now,
                reason: "request accepted for admission".into(),
            }],
            active_attempt: None,
            resume_state_path: None,
            resume_state_sha256: None,
            cancel_requested: false,
            error: String::new(),
            result_path: None,
            report: None,
            manifest: None,
            observation: None,
            observation_artifact: None,
            archive: None,
        };
        self.persist_record(record).await?;
        self.transition(
            &execution_id,
            ExecutionPhase::Admitted,
            "execution admitted",
        )
        .await?;
        self.spawn_execution(execution_id.clone(), request, None)
            .await;
        Ok(RunAccepted {
            execution_id,
            phase: ExecutionPhase::Admitted,
            duplicate: false,
            request_sha256,
            run_contract_sha256,
        })
    }

    async fn spawn_execution(
        &self,
        execution_id: String,
        request: RunRequest,
        adaptive_resume: Option<AdaptiveResumeAttempt>,
    ) {
        let (cancellation, receiver) = watch::channel(false);
        self.inner
            .cancellations
            .lock()
            .await
            .insert(execution_id.clone(), cancellation);
        let control = self.clone();
        tokio::spawn(async move {
            control
                .execute(execution_id, request, receiver, adaptive_resume)
                .await;
        });
    }

    async fn execute(
        &self,
        execution_id: String,
        request: RunRequest,
        cancellation: watch::Receiver<bool>,
        adaptive_resume: Option<AdaptiveResumeAttempt>,
    ) {
        if let Err(error) = preflight_run_contract(&request) {
            let result = self
                .finish(
                    &execution_id,
                    ExecutionPhase::Unsupported,
                    format!("{error:#}"),
                    None,
                    None,
                    None,
                )
                .await;
            if let Err(error) = result {
                tracing::error!(execution_id, %error, "failed to persist unsupported E2E observation");
            }
            return;
        }
        let (events, mut checkpoints) = mpsc::channel::<SuiteEventEnvelope>(16);
        let checkpoint_control = self.clone();
        let checkpoint_execution_id = execution_id.clone();
        let checkpoint_task = tokio::spawn(async move {
            while let Some(envelope) = checkpoints.recv().await {
                let result = checkpoint_control
                    .apply_event(&checkpoint_execution_id, &envelope.event)
                    .await;
                envelope.acknowledge(result);
            }
        });
        let output = self.inner.output_root.join(&execution_id);
        let scenarios = if request.scenarios.is_empty() {
            crate::markdown::default_keys()
        } else {
            unique_scenarios(&request.scenarios)
        };
        let judge = Some(judge_config(&request));
        let audit_analyzer = audit_config(&request);
        let outcome = run_suite(SuiteRunConfig {
            url: self.inner.url.clone(),
            execution_id: None,
            subject: SubjectConfig {
                model: request.model.clone(),
                provider: request.provider.clone(),
            },
            judge,
            audit_analyzer,
            output: output.clone(),
            scenarios,
            local_markdown_scenarios: request.local_markdown_scenarios.clone(),
            runs: request.runs,
            seed: request.seed,
            rotating_seeds: request.rotating_seeds.clone(),
            technical_retries: request.technical_retries,
            progress_interval: (request.progress_interval_seconds > 0)
                .then(|| Duration::from_secs(request.progress_interval_seconds)),
            control: Some(SuiteControl {
                execution_id: execution_id.clone(),
                lane: request.lane.clone(),
                events,
                cancellation: cancellation.clone(),
                adaptive_resume,
            }),
            observation_contract: request.run_contract.clone(),
            materialized_markdown_plan: None,
        })
        .await;
        checkpoint_task.abort();

        let cancelled = *cancellation.borrow();
        let result = match outcome {
            Ok(outcome) => {
                let reconciliation_error = outcome
                    .report
                    .scenarios
                    .iter()
                    .flat_map(|scenario| &scenario.runs)
                    .flat_map(|run| &run.failures)
                    .find(|failure| failure.message.starts_with("needs_reconciliation:"))
                    .map(|failure| failure.message.clone());
                self.finish(
                    &execution_id,
                    if cancelled {
                        ExecutionPhase::Cancelled
                    } else if reconciliation_error.is_some() {
                        ExecutionPhase::NeedsReconciliation
                    } else {
                        ExecutionPhase::Completed
                    },
                    reconciliation_error.unwrap_or_default(),
                    Some(outcome.report),
                    Some(outcome.manifest),
                    Some(relative_result_path(
                        &self.inner.output_root,
                        &outcome.report_path,
                    )),
                )
                .await
            }
            Err(error) => {
                self.cleanup_active_attempt(&execution_id).await;
                let rendered = format!("{error:#}");
                self.finish(
                    &execution_id,
                    if cancelled || format!("{error:#}").contains("was cancelled") {
                        ExecutionPhase::Cancelled
                    } else if rendered.contains("E2E observation identity mismatch") {
                        ExecutionPhase::Unsupported
                    } else {
                        ExecutionPhase::Failed
                    },
                    rendered,
                    None,
                    None,
                    None,
                )
                .await
            }
        };
        if let Err(error) = result {
            tracing::error!(execution_id, %error, "failed to finalize E2E execution record");
        }
        self.inner.cancellations.lock().await.remove(&execution_id);
    }

    async fn apply_event(&self, execution_id: &str, event: &SuiteEvent) -> Result<()> {
        match event {
            SuiteEvent::Phase(phase) => {
                self.transition(execution_id, (*phase).into(), "suite checkpoint")
                    .await
            }
            SuiteEvent::AttemptStarted {
                scenario_id,
                run_id,
                attempt_id,
                session_id,
                resume_state_path,
            } => {
                self.update_record(execution_id, |record| {
                    record.active_attempt = Some(ActiveAttempt {
                        scenario_id: scenario_id.clone(),
                        run_id: run_id.clone(),
                        attempt_id: attempt_id.clone(),
                        session_id: session_id.clone(),
                        resume_state_path: resume_state_path.clone(),
                        resume_state_sha256: None,
                    });
                    record.resume_state_path = resume_state_path.clone();
                    record.resume_state_sha256 = None;
                })
                .await
            }
            SuiteEvent::AdaptiveResumeState {
                attempt_id,
                state_sha256,
            } => {
                self.update_record(execution_id, |record| {
                    if let Some(active) = record
                        .active_attempt
                        .as_mut()
                        .filter(|active| active.attempt_id == *attempt_id)
                    {
                        active.resume_state_sha256 = Some(state_sha256.clone());
                    }
                    record.resume_state_sha256 = Some(state_sha256.clone());
                })
                .await
            }
            SuiteEvent::AttemptFinished { attempt_id } => {
                self.update_record(execution_id, |record| {
                    if record
                        .active_attempt
                        .as_ref()
                        .map(|attempt| attempt.attempt_id.as_str())
                        == Some(attempt_id.as_str())
                    {
                        record.active_attempt = None;
                    }
                })
                .await
            }
        }
    }

    pub async fn status(&self, execution_id: &str) -> Result<StatusResponse> {
        Ok(status_response(&self.record(execution_id).await?))
    }

    pub async fn cancel(&self, execution_id: &str) -> Result<CancelResponse> {
        let record = self.record(execution_id).await?;
        if record.phase.terminal() {
            return Ok(CancelResponse {
                execution_id: execution_id.into(),
                phase: record.phase,
                accepted: false,
            });
        }
        self.update_record(execution_id, |record| record.cancel_requested = true)
            .await?;
        if let Some(cancellation) = self.inner.cancellations.lock().await.get(execution_id) {
            let _ = cancellation.send(true);
        }
        if let Some(active) = record.active_attempt {
            let _ = self
                .trigger(
                    "harness::stop",
                    json!({ "session_id": active.session_id, "reason": "E2E execution cancelled" }),
                )
                .await;
        }
        Ok(CancelResponse {
            execution_id: execution_id.into(),
            phase: self.record(execution_id).await?.phase,
            accepted: true,
        })
    }

    pub async fn results_get(&self, execution_id: &str) -> Result<ResultsGetResponse> {
        let record = self.record(execution_id).await?;
        Ok(ResultsGetResponse {
            execution_id: record.execution_id,
            phase: record.phase,
            result_path: record.result_path,
            report: record.report,
            manifest: record.manifest,
            archive: record.archive,
            request_sha256: if record.request_sha256.is_empty() {
                artifact::sha256_value(&record.request)?
            } else {
                record.request_sha256
            },
            run_contract_sha256: record.run_contract_sha256,
            observation: record.observation,
            observation_artifact: record.observation_artifact,
        })
    }

    async fn fault_evaluate(&self, request: FaultEvaluateRequest) -> Result<FaultEvaluation> {
        let record = self.record(&request.execution_id).await?;
        if !record.phase.terminal() {
            bail!("fault evaluation requires a terminal execution");
        }
        match request.profile.expected_outcome {
            ExpectedTerminalOutcome::Recovered if record.phase == ExecutionPhase::Cancelled => {
                bail!("a recovered fault profile cannot evaluate a cancelled execution");
            }
            ExpectedTerminalOutcome::Cancelled if record.phase != ExecutionPhase::Cancelled => {
                bail!("a cancellation fault profile requires a cancelled execution");
            }
            _ => {}
        }
        FaultEvaluation::evaluate(
            &request.profile,
            &request.plan,
            &request.journal,
            record.report.as_ref(),
        )
    }

    async fn archive(&self, request: ArchiveExecutionRequest) -> Result<ArchiveResponse> {
        let record = self.record(&request.execution_id).await?;
        if !record.phase.terminal() {
            bail!("only a terminal execution can be archived");
        }
        if record.report.is_none() && record.observation.is_none() {
            bail!("terminal execution has neither a report nor an observation envelope");
        }
        let output = self.inner.output_root.join(&record.execution_id);
        let response = self
            .inner
            .durable
            .archive(
                &output,
                record.report.as_ref(),
                record.observation.as_ref(),
                request.retention_class,
            )
            .await?;
        let archive = response.archive.clone();
        self.update_record(&request.execution_id, move |record| {
            record.archive = Some(archive);
        })
        .await?;
        Ok(response)
    }

    async fn archive_head(&self, execution_id: &str) -> Result<ArchiveHeadResponse> {
        let archive = self
            .record(execution_id)
            .await?
            .archive
            .context("execution has not been archived")?;
        self.inner.durable.head(archive).await
    }

    async fn archive_restore(&self, execution_id: &str) -> Result<ArchiveRestoreResponse> {
        let archive = self
            .record(execution_id)
            .await?
            .archive
            .context("execution has not been archived")?;
        self.inner
            .durable
            .restore(archive, &self.inner.output_root.join("restored"))
            .await
    }

    pub async fn results_list(&self, request: ResultsListRequest) -> Result<ResultsListResponse> {
        if request.limit == 0 || request.limit > 500 {
            bail!("results list limit must be between 1 and 500");
        }
        let mut records = self
            .inner
            .records
            .read()
            .await
            .values()
            .filter(|record| {
                request
                    .lane
                    .as_ref()
                    .is_none_or(|lane| &record.request.lane == lane)
                    && request.scenario_id.as_ref().is_none_or(|scenario| {
                        record.request.scenarios.is_empty()
                            || record.request.scenarios.contains(scenario)
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| right.requested_at.cmp(&left.requested_at));
        records.truncate(usize::from(request.limit));
        Ok(ResultsListResponse {
            executions: records.iter().map(status_response).collect(),
        })
    }

    async fn compare(&self, request: CompareRequest) -> Result<ComparisonResponse> {
        if request.from_execution_id == request.to_execution_id {
            bail!("comparison requires two distinct executions");
        }
        let from = self.record(&request.from_execution_id).await?;
        let to = self.record(&request.to_execution_id).await?;
        for (side, record) in [("from", &from), ("to", &to)] {
            if record.phase != ExecutionPhase::Completed {
                bail!("{side} execution must be completed before comparison");
            }
        }
        let policy = match request.policy {
            Some(policy) => policy,
            None => longitudinal::load_comparison_policy(request.baseline.as_deref())?,
        };
        let comparison = compare_records(&from, &to, policy)?;
        let artifacts = longitudinal::write_comparison(
            &self.inner.output_root.join(&to.execution_id),
            &comparison,
        )?;
        Ok(ComparisonResponse {
            comparison,
            artifacts,
        })
    }

    async fn restore(&self) -> Result<()> {
        let listed = self
            .trigger("state::list", json!({ "scope": RECORD_SCOPE }))
            .await
            .context("list persisted E2E executions")?;
        let records = listed
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| serde_json::from_value::<ExecutionRecord>(value).ok())
            .collect::<Vec<_>>();
        for mut record in records {
            let execution_id = record.execution_id.clone();
            if !record.phase.terminal() {
                let adaptive_resume = record
                    .active_attempt
                    .as_ref()
                    .and_then(|active| self.adaptive_restore_attempt(&record, active));
                if let Some(adaptive_resume) = adaptive_resume {
                    record.error.clear();
                    record.phase = ExecutionPhase::Admitted;
                    record.updated_at = now();
                    record.transitions.push(PhaseTransition {
                        phase: ExecutionPhase::Admitted,
                        at: record.updated_at.clone(),
                        reason: "restart recovery re-enqueued the trusted adaptive attempt".into(),
                    });
                    let request = record.request.clone();
                    self.persist_record(record).await?;
                    self.spawn_execution(execution_id, request, Some(adaptive_resume))
                        .await;
                    continue;
                }
                if let Some(active) = record.active_attempt.take() {
                    self.compensate_attempt(&active).await;
                }
                record.cancel_requested = true;
                record.error =
                    "worker restarted during execution; active work was compensated".into();
                record.phase = ExecutionPhase::Cancelled;
                record.updated_at = now();
                record.transitions.push(PhaseTransition {
                    phase: ExecutionPhase::Cancelled,
                    at: record.updated_at.clone(),
                    reason: "restart recovery compensated the active attempt".into(),
                });
                if record.request.run_contract.is_some() {
                    let observation = terminal_observation(
                        &record,
                        ExecutionPhase::Cancelled,
                        &record.error,
                        record.report.as_ref(),
                        record.manifest.as_ref(),
                        record.result_path.as_deref(),
                        &self.inner.output_root,
                    )?;
                    let artifact =
                        observation.write_to(&self.inner.output_root.join(&execution_id))?;
                    record.observation = Some(observation);
                    record.observation_artifact = Some(artifact);
                }
                self.persist_record(record).await?;
            } else if record.request.run_contract.is_some() && record.observation.is_none() {
                let observation = terminal_observation(
                    &record,
                    record.phase,
                    &record.error,
                    record.report.as_ref(),
                    record.manifest.as_ref(),
                    record.result_path.as_deref(),
                    &self.inner.output_root,
                )?;
                let artifact = observation.write_to(&self.inner.output_root.join(&execution_id))?;
                record.observation = Some(observation);
                record.observation_artifact = Some(artifact);
                self.persist_record(record).await?;
            } else {
                self.inner
                    .records
                    .write()
                    .await
                    .insert(execution_id, record);
            }
        }
        Ok(())
    }

    fn adaptive_restore_attempt(
        &self,
        record: &ExecutionRecord,
        active: &ActiveAttempt,
    ) -> Option<AdaptiveResumeAttempt> {
        let scenario_id = active.scenario_id.built_in()?;
        if record.cancel_requested
            || active.scenario_id.execution_kind()
                != crate::scenarios::ScenarioExecutionKind::AdaptiveFlow
            || active.run_id.is_empty()
            || record.request.runs != 1
            || record.request.technical_retries != 0
            || !record.request.rotating_seeds.is_empty()
            || record.request.scenarios.as_slice() != [active.scenario_id.clone()]
        {
            return None;
        }
        let expected = self
            .inner
            .output_root
            .join(".workflow-state")
            .join("workflow-resume")
            .join(&record.execution_id)
            .join(&active.run_id)
            .join(&active.attempt_id)
            .join("state-v1.json");
        if active.resume_state_path.as_deref() != Some(expected.to_string_lossy().as_ref()) {
            return None;
        }
        let planner_state = self
            .inner
            .output_root
            .join(".workflow-state")
            .join("adaptive-plans")
            .join(&record.execution_id)
            .join(&active.run_id)
            .join(&active.attempt_id)
            .join("plans-v1.json");
        if expected.is_file() && !planner_state.is_file() {
            return None;
        }
        Some(AdaptiveResumeAttempt {
            scenario_id,
            run_id: active.run_id.clone(),
            attempt_id: active.attempt_id.clone(),
            resume_existing: expected.is_file(),
            restore_planner: planner_state.is_file(),
        })
    }

    async fn cleanup_active_attempt(&self, execution_id: &str) {
        let active = self
            .inner
            .records
            .read()
            .await
            .get(execution_id)
            .and_then(|record| record.active_attempt.clone());
        if let Some(active) = active {
            self.compensate_attempt(&active).await;
        }
    }

    async fn compensate_attempt(&self, active: &ActiveAttempt) {
        let _ = self
            .trigger(
                "harness::teardown",
                json!({ "root_session_id": active.session_id }),
            )
            .await;
        if let Some(cleanup) = active
            .scenario_id
            .built_in()
            .and_then(|scenario| scenario.spec(&active.attempt_id).cleanup)
        {
            let context = E2eContext::from_client(self.inner.iii.clone());
            if let Err(error) = cleanup(&context, &active.attempt_id).await {
                tracing::warn!(
                    scenario = active.scenario_id.as_str(),
                    attempt_id = active.attempt_id,
                    %error,
                    "restart compensation could not complete scenario cleanup"
                );
            }
        }
    }

    async fn finish(
        &self,
        execution_id: &str,
        phase: ExecutionPhase,
        error: String,
        report: Option<E2eReport>,
        manifest: Option<E2eManifest>,
        result_path: Option<String>,
    ) -> Result<()> {
        let current = self.record(execution_id).await?;
        let (observation, observation_artifact) = if current.request.run_contract.is_some() {
            let observation = terminal_observation(
                &current,
                phase,
                &error,
                report.as_ref(),
                manifest.as_ref(),
                result_path.as_deref(),
                &self.inner.output_root,
            )?;
            let output = self.inner.output_root.join(execution_id);
            let artifact = observation.write_to(&output)?;
            (Some(observation), Some(artifact))
        } else {
            (None, None)
        };
        self.update_record(execution_id, move |record| {
            record.phase = phase;
            record.error = error;
            record.report = report;
            record.manifest = manifest;
            record.result_path = result_path;
            record.observation = observation;
            record.observation_artifact = observation_artifact;
            record.active_attempt = None;
            record.transitions.push(PhaseTransition {
                phase,
                at: now(),
                reason: "execution finalized".into(),
            });
        })
        .await
    }

    async fn transition(
        &self,
        execution_id: &str,
        phase: ExecutionPhase,
        reason: &str,
    ) -> Result<()> {
        let reason = reason.to_string();
        self.update_record(execution_id, move |record| {
            if !record.phase.terminal() {
                record.phase = phase;
                record.transitions.push(PhaseTransition {
                    phase,
                    at: now(),
                    reason,
                });
            }
        })
        .await
    }

    async fn update_record(
        &self,
        execution_id: &str,
        update: impl FnOnce(&mut ExecutionRecord),
    ) -> Result<()> {
        let mut record = self.record(execution_id).await?;
        update(&mut record);
        record.updated_at = now();
        self.persist_record(record).await
    }

    pub async fn record(&self, execution_id: &str) -> Result<ExecutionRecord> {
        self.inner
            .records
            .read()
            .await
            .get(execution_id)
            .cloned()
            .with_context(|| format!("unknown E2E execution {execution_id}"))
    }

    async fn persist_record(&self, record: ExecutionRecord) -> Result<()> {
        self.trigger(
            "state::set",
            json!({
                "scope": RECORD_SCOPE,
                "key": record.execution_id,
                "value": record,
            }),
        )
        .await
        .context("persist E2E execution record")?;
        self.inner
            .records
            .write()
            .await
            .insert(record.execution_id.clone(), record.clone());
        let _ = self.inner.updates.send(ControlPlaneUpdate { record });
        Ok(())
    }

    async fn trigger(&self, function_id: &str, payload: Value) -> Result<Value> {
        self.inner
            .iii
            .trigger(TriggerRequest {
                function_id: function_id.into(),
                payload,
                action: None,
                timeout_ms: Some(120_000),
            })
            .await
            .map_err(|error| anyhow::anyhow!("{function_id}: {error}"))
    }
}

fn register_function(iii: &IIIClient, id: &str, description: &str, registration: RegisterFunction) {
    iii.register_function(
        id,
        registration.description(description).metadata(json!({
            "internal": true,
            "contract": {
                "name": CONTROL_CONTRACT_NAME,
                "capabilities": [id.trim_start_matches("e2e::")],
            }
        })),
    );
}

fn register_agent_function(
    iii: &IIIClient,
    id: &str,
    description: &str,
    operation: &str,
    writes_local_state: bool,
    registration: RegisterFunction,
) {
    iii.register_function(
        id,
        registration.description(description).metadata(json!({
            "internal": false,
            "contract": {
                "name": CONTROL_CONTRACT_NAME,
                "capabilities": [id.trim_start_matches("e2e::")],
            },
            "agent": {
                "audience": "harness",
                "category": "e2e_local_test_authoring",
                "operation": operation,
                "writes_local_state": writes_local_state,
                "starts_execution": false,
            }
        })),
    );
}

fn scenario_authoring_guide() -> ScenarioAuthoringGuideResponse {
    ScenarioAuthoringGuideResponse {
        schema: "e2e-local-scenario-authoring/v1".into(),
        summary: "Author a reusable local Markdown test without changing the repository or starting an execution.".into(),
        creation_starts_execution: false,
        overwrites_existing_file: false,
        list_function: SCENARIOS_LIST_ID.into(),
        create_function: SCENARIOS_CREATE_ID.into(),
        run_function: RUN_ID.into(),
        storage_directory: LOCAL_SCENARIO_DIRECTORY.into(),
        max_source_bytes: LOCAL_SCENARIO_MAX_BYTES,
        file_name_rules: vec![
            "Pass one .md file name only; do not include a directory or path traversal.".into(),
            "Use a non-empty stem made from ASCII letters, numbers, spaces, hyphens, or underscores.".into(),
            "Choose a new name: an existing local file is rejected and never overwritten.".into(),
            "The file stem compiles to a stable local_<safe_id> scenario id.".into(),
        ],
        required_h2_sections: LOCAL_SCENARIO_REQUIRED_SECTIONS
            .into_iter()
            .map(str::to_string)
            .collect(),
        required_plan: LOCAL_SCENARIO_PLAN_ID.into(),
        validation_rules: vec![
            "Add at least one H3 criterion under Validations.".into(),
            "Format every criterion heading as ### Name (N%) with a positive integer weight."
                .into(),
            "Make all criterion weights total exactly 100%.".into(),
            "Provide non-empty instructions below every validation heading.".into(),
        ],
        workflow: vec![
            format!(
                "Draft source from this template and keep Plans set to {LOCAL_SCENARIO_PLAN_ID}."
            ),
            format!("Call {SCENARIOS_CREATE_ID} with file_name and the complete source."),
            format!(
                "Call {SCENARIOS_LIST_ID} to confirm the returned id appears with origin local."
            ),
            format!(
                "Call {RUN_ID} only later and only when the user explicitly asks to execute the test."
            ),
        ],
        template: LOCAL_SCENARIO_TEMPLATE.into(),
    }
}

fn handler_error(error: anyhow::Error) -> Error {
    Error::Handler(format!("{error:#}"))
}

fn execution_id_for_key(idempotency_key: &str) -> String {
    let digest = Sha256::digest(format!("{CONTROL_CONTRACT_NAME}:{idempotency_key}").as_bytes());
    format!("{:x}", digest)[..32].to_string()
}

fn validate_run_request(request: &RunRequest) -> Result<LaneBudget> {
    if request.idempotency_key.trim().is_empty()
        || request.idempotency_key.len() > MAX_IDEMPOTENCY_KEY_BYTES
    {
        bail!("idempotency_key must contain 1 to {MAX_IDEMPOTENCY_KEY_BYTES} bytes");
    }
    for (name, value) in [
        ("lane", request.lane.as_str()),
        ("model", request.model.as_str()),
        ("provider", request.provider.as_str()),
    ] {
        if value.trim().is_empty() {
            bail!("{name} cannot be empty");
        }
    }
    let budget = lane_budget(&request.lane);
    let scenarios = if request.scenarios.is_empty() {
        crate::markdown::default_keys()
    } else {
        unique_scenarios(&request.scenarios)
    };
    if request.technical_retries > 0
        && scenarios
            .iter()
            .any(|scenario| !scenario.execution_kind().replay_safe())
    {
        bail!("non-replayable scenarios require technical_retries=0");
    }
    let base_seed_count = 1_usize;
    let unique_rotating_seeds = request
        .rotating_seeds
        .iter()
        .copied()
        .filter(|seed| Some(*seed) != request.seed)
        .collect::<std::collections::HashSet<_>>()
        .len();
    let seed_count = base_seed_count.saturating_add(unique_rotating_seeds);
    let case_count = scenarios.len().saturating_mul(seed_count);
    if case_count > usize::from(budget.max_cases) {
        bail!(
            "lane '{}' permits at most {} cases, got {}",
            request.lane,
            budget.max_cases,
            case_count
        );
    }
    if !(1..=budget.max_runs_per_case).contains(&request.runs) {
        bail!(
            "lane '{}' permits 1 to {} runs per case",
            request.lane,
            budget.max_runs_per_case
        );
    }
    if request.technical_retries > budget.max_technical_retries {
        bail!(
            "lane '{}' permits at most {} technical retries",
            request.lane,
            budget.max_technical_retries
        );
    }
    let turns_per_run = scenarios
        .iter()
        .try_fold(0_u64, |total, scenario| -> Result<u64> {
            let max_turns = scenario.built_in().map_or_else(
                || {
                    markdown_scenario_for_request(request, scenario.as_str()).map(|compiled| {
                        16_u64
                            + u64::from(crate::markdown::execution_policy().max_turns)
                            + 8_u64.saturating_mul(compiled.validations.len() as u64)
                            + 16
                    })
                },
                |built_in| Ok(u64::from(built_in.spec("budget").execution.max_turns)),
            )?;
            total
                .checked_add(max_turns)
                .context("declared turn budget overflow")
        })?;
    let declared_turns = turns_per_run
        .checked_mul(seed_count.try_into().unwrap_or(u64::MAX))
        .and_then(|turns| turns.checked_mul(u64::from(request.runs)))
        .and_then(|turns| turns.checked_mul(u64::from(request.technical_retries) + 1))
        .context("declared turn budget overflow")?;
    if declared_turns > budget.max_declared_turns {
        bail!(
            "lane '{}' declared {} possible turns, above its {} turn budget",
            request.lane,
            declared_turns,
            budget.max_declared_turns
        );
    }
    if request.judge_model.is_some() != request.judge_provider.is_some() {
        bail!("judge_model and judge_provider must be supplied together");
    }
    if scenarios
        .iter()
        .any(|scenario| scenario.built_in().is_none())
        && (request.judge_model.is_none() || request.judge_provider.is_none())
    {
        bail!(
            "Markdown scenarios require an explicit judge_model and judge_provider for setup, validation, adherence, and cleanup"
        );
    }
    if request.audit_model.is_some() != request.audit_provider.is_some() {
        bail!("audit_model and audit_provider must be supplied together");
    }
    if let Some(contract) = &request.run_contract {
        contract.validate()?;
        let expected = observation_idempotency_key(request)?;
        if request.idempotency_key != expected {
            bail!("D0 idempotency_key must equal {expected}");
        }
    }
    Ok(budget)
}

fn observation_intent_sha256(request: &RunRequest) -> Result<String> {
    let contract = request
        .run_contract
        .as_ref()
        .context("D0 observation intent requires run_contract")?;
    artifact::sha256_value(&json!({
        "run_contract": contract,
        "lane": request.lane,
        "model": request.model,
        "provider": request.provider,
        "judge_model": request.judge_model,
        "judge_provider": request.judge_provider,
        "scenarios": request.scenarios,
        "runs": request.runs,
        "seed": request.seed,
        "rotating_seeds": request.rotating_seeds,
        "technical_retries": request.technical_retries,
    }))
}

fn observation_idempotency_key(request: &RunRequest) -> Result<String> {
    let digest = observation_intent_sha256(request)?;
    Ok(format!(
        "rc:d0:{}",
        digest.strip_prefix("sha256:").unwrap_or(&digest)
    ))
}

fn preflight_run_contract(request: &RunRequest) -> Result<()> {
    let Some(contract) = &request.run_contract else {
        return Ok(());
    };
    contract.validate()?;
    let runner = RunnerIdentity::runtime();
    runner.validate()?;
    if contract.runner != runner {
        bail!(
            "E2E observation identity mismatch: expected runner {:?}, observed {:?}",
            contract.runner,
            runner
        );
    }
    let catalog = scenarios_list_with_definitions(
        &request.local_markdown_scenarios,
        ScenariosListRequest { seed: request.seed },
    )?;
    if contract.plan.catalog_sha256 != catalog.catalog_sha256 {
        bail!(
            "E2E observation catalog mismatch: expected {}, observed {}",
            contract.plan.catalog_sha256,
            catalog.catalog_sha256
        );
    }
    let scenarios = if request.scenarios.is_empty() {
        crate::markdown::default_keys()
    } else {
        unique_scenarios(&request.scenarios)
    };
    let mut expected = Vec::new();
    for scenario in scenarios {
        for seed in observation_case_seeds(&scenario, request.seed, &request.rotating_seeds) {
            let descriptor = materialize_request_scenario_descriptor(request, &scenario, seed)?;
            expected.push(ObservationSelectedCase {
                scenario_id: descriptor.scenario_id,
                scenario_version: descriptor.scenario_version,
                case_id: descriptor.case_id,
                seed: descriptor.seed,
                inputs_sha256: descriptor.inputs_sha256,
                contract_sha256: descriptor.contract_sha256,
            });
        }
    }
    sort_selected_cases(&mut expected);
    let mut supplied = contract.selected_cases.clone();
    sort_selected_cases(&mut supplied);
    if supplied != expected {
        bail!("E2E observation selected cases differ from runtime materialization");
    }
    Ok(())
}

fn observation_case_seeds(
    scenario: &ScenarioKey,
    fixed: Option<u64>,
    rotating: &[u64],
) -> Vec<u64> {
    if scenario.canonical_seed_only() {
        return vec![scenario.canonical_seed()];
    }
    let mut seeds = vec![fixed.unwrap_or_else(|| scenario.canonical_seed())];
    for seed in rotating {
        if !seeds.contains(seed) {
            seeds.push(*seed);
        }
    }
    seeds
}

fn sort_selected_cases(cases: &mut [ObservationSelectedCase]) {
    cases.sort_by(|left, right| {
        left.scenario_id
            .as_str()
            .cmp(right.scenario_id.as_str())
            .then_with(|| left.case_id.cmp(&right.case_id))
    });
}

fn lane_budget(lane: &str) -> LaneBudget {
    let normalized = lane.to_ascii_lowercase();
    let (max_cases, max_runs_per_case, max_technical_retries, max_declared_turns) =
        if normalized == "ci" || normalized.contains("pr-gate") {
            (8, 1, 1, 1_000)
        } else if normalized.contains("main") {
            (24, 3, 1, 10_000)
        } else if normalized.contains("daily")
            || normalized.contains("deployed")
            || normalized.contains("release")
        {
            (32, 20, 2, 100_000)
        } else if normalized.contains("weekly")
            || normalized.contains("stress")
            || normalized.contains("local")
            || normalized.contains("smoke")
        {
            (32, 20, 3, 200_000)
        } else {
            (24, 1, 1, 10_000)
        };
    LaneBudget {
        max_cases,
        max_runs_per_case,
        max_technical_retries,
        max_declared_turns,
    }
}

fn audit_config(request: &RunRequest) -> Option<JudgeConfig> {
    request
        .audit_model
        .clone()
        .zip(request.audit_provider.clone())
        .map(|(model, provider)| JudgeConfig { model, provider })
}

fn judge_config(request: &RunRequest) -> JudgeConfig {
    JudgeConfig {
        model: request
            .judge_model
            .clone()
            .unwrap_or_else(|| request.model.clone()),
        provider: request
            .judge_provider
            .clone()
            .unwrap_or_else(|| request.provider.clone()),
    }
}

fn unique_scenarios(scenarios: &[ScenarioKey]) -> Vec<ScenarioKey> {
    scenarios
        .iter()
        .cloned()
        .fold(Vec::new(), |mut result, id| {
            if !result.contains(&id) {
                result.push(id);
            }
            result
        })
}

fn resolve_local_markdown_scenarios(
    output_root: &Path,
    scenarios: &[ScenarioKey],
    capture_catalog: bool,
) -> Result<Vec<MarkdownScenarioSource>> {
    let requested = scenarios
        .iter()
        .filter(|scenario| scenario.built_in().is_none())
        .map(|scenario| scenario.as_str().to_string())
        .collect::<std::collections::HashSet<_>>();
    if requested.is_empty() && !capture_catalog {
        return Ok(Vec::new());
    }
    let embedded = crate::markdown::embedded_catalog()?
        .into_iter()
        .map(|scenario| scenario.id)
        .collect::<std::collections::HashSet<_>>();
    let unresolved = requested
        .difference(&embedded)
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    if unresolved.is_empty() && !capture_catalog {
        return Ok(Vec::new());
    }
    let catalog = crate::markdown::local_catalog(output_root)?;
    if capture_catalog {
        return Ok(catalog);
    }
    let resolved = catalog
        .into_iter()
        .filter(|definition| unresolved.contains(&definition.scenario.id))
        .collect::<Vec<_>>();
    let resolved_ids = resolved
        .iter()
        .map(|definition| definition.scenario.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    if let Some(id) = unresolved
        .iter()
        .find(|id| !resolved_ids.contains(id.as_str()))
    {
        bail!("unknown E2E scenario '{id}'");
    }
    Ok(resolved)
}

pub fn scenarios_list(request: ScenariosListRequest) -> Result<ScenariosListResponse> {
    let scenarios = crate::markdown::all_keys()?
        .into_iter()
        .map(|scenario_id| {
            let seed = request.seed.unwrap_or_else(|| scenario_id.canonical_seed());
            materialize_scenario_descriptor(scenario_id, seed, "catalog")
        })
        .collect::<Result<Vec<_>>>()?;
    let runner = RunnerIdentity::runtime();
    runner.validate()?;
    let catalog_sha256 = artifact::sha256_value(&json!({
        "schema": CATALOG_SCHEMA,
        "runner": &runner,
        "scenarios": &scenarios,
    }))?;
    Ok(ScenariosListResponse {
        schema: CATALOG_SCHEMA.into(),
        runner,
        catalog_sha256,
        scenarios,
    })
}

fn scenarios_list_with_local(
    output_root: &Path,
    request: ScenariosListRequest,
) -> Result<ScenariosListResponse> {
    scenarios_list_with_definitions(&crate::markdown::local_catalog(output_root)?, request)
}

fn scenarios_list_with_definitions(
    definitions: &[MarkdownScenarioSource],
    request: ScenariosListRequest,
) -> Result<ScenariosListResponse> {
    let mut response = scenarios_list(request.clone())?;
    if definitions.is_empty() {
        return Ok(response);
    }
    for definition in definitions {
        crate::markdown::validate_local_definition(definition)?;
        let scenario_id = ScenarioKey::Markdown(definition.scenario.id.clone());
        let seed = request.seed.unwrap_or_else(|| scenario_id.canonical_seed());
        response.scenarios.push(materialize_markdown_descriptor(
            definition.scenario.clone(),
            seed,
            ScenarioOrigin::Local,
        )?);
    }
    response
        .scenarios
        .sort_by(|left, right| left.scenario_id.as_str().cmp(right.scenario_id.as_str()));
    response.catalog_sha256 = artifact::sha256_value(&json!({
        "schema": response.schema,
        "runner": response.runner,
        "scenarios": response.scenarios,
    }))?;
    Ok(response)
}

fn materialize_scenario_descriptor(
    scenario_id: ScenarioKey,
    seed: u64,
    label: &str,
) -> Result<ScenarioDescriptor> {
    match scenario_id.clone() {
        ScenarioKey::BuiltIn(id) => {
            let materialized = id.materialize(label, seed)?;
            let contract_sha256 =
                scenario_contract_sha256(&materialized.case, materialized.spec.execution)?;
            let complexity = materialized.case.complexity.profile;
            let resource_envelope = ScenarioResourceEnvelope {
                execution: materialized.spec.execution,
                workflow: serde_json::from_value(
                    materialized.case.inputs["workflow_resource_budgets"].clone(),
                )
                .ok(),
            };
            Ok(ScenarioDescriptor {
                scenario_id,
                origin: ScenarioOrigin::BuiltIn,
                title: None,
                plans: Vec::new(),
                author_version: None,
                source_path: None,
                source_sha256: None,
                behavior_sha256: None,
                compiled_sha256: None,
                scenario_version: materialized.case.scenario_version,
                case_id: materialized.case.case_id,
                seed: materialized.case.seed,
                inputs_sha256: materialized.case.inputs_sha256,
                contract_sha256,
                classification: materialized.case.complexity,
                complexity,
                characterization: materialized.case.characterization,
                resource_envelope,
                required_capabilities: materialized.case.required_capabilities,
                deliverable_contract: materialized.case.deliverable_contract,
            })
        }
        ScenarioKey::Markdown(id) => {
            let scenario = crate::markdown::embedded_scenario(&id)?;
            materialize_markdown_descriptor(scenario, seed, ScenarioOrigin::Markdown)
        }
    }
}

fn markdown_scenario_for_request(
    request: &RunRequest,
    id: &str,
) -> Result<crate::markdown::CompiledMarkdownScenario> {
    request
        .local_markdown_scenarios
        .iter()
        .find(|definition| definition.scenario.id == id)
        .map(|definition| definition.scenario.clone())
        .map(Ok)
        .unwrap_or_else(|| crate::markdown::embedded_scenario(id))
}

fn materialize_request_scenario_descriptor(
    request: &RunRequest,
    scenario_id: &ScenarioKey,
    seed: u64,
) -> Result<ScenarioDescriptor> {
    if scenario_id.built_in().is_some() {
        materialize_scenario_descriptor(scenario_id.clone(), seed, "run-contract")
    } else {
        materialize_markdown_descriptor(
            markdown_scenario_for_request(request, scenario_id.as_str())?,
            seed,
            if request
                .local_markdown_scenarios
                .iter()
                .any(|definition| definition.scenario.id == scenario_id.as_str())
            {
                ScenarioOrigin::Local
            } else {
                ScenarioOrigin::Markdown
            },
        )
    }
}

fn materialize_markdown_descriptor(
    scenario: crate::markdown::CompiledMarkdownScenario,
    seed: u64,
    origin: ScenarioOrigin,
) -> Result<ScenarioDescriptor> {
    let scenario_id = ScenarioKey::Markdown(scenario.id.clone());
    let case = crate::suite::markdown_case(&scenario, seed)?;
    let execution = crate::markdown::execution_policy();
    let contract_sha256 = scenario_contract_sha256(&case, execution)?;
    let complexity = case.complexity.profile;
    Ok(ScenarioDescriptor {
        scenario_id,
        origin,
        title: Some(scenario.title),
        plans: scenario.plans,
        author_version: Some(scenario.version),
        source_path: Some(scenario.source_path),
        source_sha256: Some(scenario.source_sha256),
        behavior_sha256: Some(scenario.behavior_sha256),
        compiled_sha256: Some(scenario.compiled_sha256),
        scenario_version: case.scenario_version,
        case_id: case.case_id,
        seed: case.seed,
        inputs_sha256: case.inputs_sha256,
        contract_sha256,
        classification: case.complexity,
        complexity,
        characterization: case.characterization,
        resource_envelope: ScenarioResourceEnvelope {
            execution,
            workflow: None,
        },
        required_capabilities: case.required_capabilities,
        deliverable_contract: case.deliverable_contract,
    })
}

fn terminal_observation(
    record: &ExecutionRecord,
    phase: ExecutionPhase,
    error: &str,
    report: Option<&E2eReport>,
    manifest: Option<&E2eManifest>,
    result_path: Option<&str>,
    output_root: &Path,
) -> Result<E2eObservationEnvelope> {
    let contract = record
        .request
        .run_contract
        .as_ref()
        .context("terminal D0 observation is missing run_contract")?;
    let request_sha256 = if record.request_sha256.is_empty() {
        artifact::sha256_value(&record.request)?
    } else {
        record.request_sha256.clone()
    };
    let run_contract_sha256 = record
        .run_contract_sha256
        .clone()
        .map(Ok)
        .unwrap_or_else(|| artifact::sha256_value(contract))?;
    let completed_at = report
        .map(|report| report.execution.completed_at.clone())
        .unwrap_or_else(|| record.updated_at.clone());
    let samples = report.map(observation_samples).unwrap_or_default();
    let data_availability = observation_data_availability(&samples);
    let objective = observation_objective(phase, report);
    let passed = (phase == ExecutionPhase::Completed)
        .then(|| report.map(|report| report.passed))
        .flatten();
    let result_sha256 = result_path
        .map(|path| output_root.join(path))
        .filter(|path| path.is_file())
        .map(|path| {
            std::fs::read(&path)
                .map(|bytes| artifact::sha256_bytes(&bytes))
                .with_context(|| format!("read observation result {}", path.display()))
        })
        .transpose()?;
    let mut artifacts = Vec::new();
    if let Some(report) = report {
        if let Some(reference) = &report.manifest {
            artifacts.push(reference.clone());
        }
        for run in report
            .scenarios
            .iter()
            .flat_map(|scenario| scenario.runs.iter())
        {
            artifacts.extend(run.evidence.iter().cloned());
            artifacts.extend(
                run.deliverables
                    .iter()
                    .filter_map(|deliverable| deliverable.artifact.clone()),
            );
        }
    }
    artifacts.sort_by(|left, right| left.path.cmp(&right.path));
    artifacts.dedup_by(|left, right| left.path == right.path && left.sha256 == right.sha256);
    let system_under_test = report
        .map(|report| report.system_under_test.clone())
        .or_else(|| manifest.map(|manifest| manifest.system_under_test.clone()));
    let (error, _) = crate::redaction::RedactionPolicy::from_environment().redact_text(error);
    let observation = E2eObservationEnvelope {
        schema: OBSERVATION_SCHEMA.into(),
        identity: ObservationIdentity {
            target: contract.target.clone(),
            plan: contract.plan.clone(),
            runner: contract.runner.clone(),
            execution: ObservationExecutionIdentity {
                id: record.execution_id.clone(),
                attempt: contract.attempt,
                request_sha256,
                run_contract_sha256,
                started_at: record.requested_at.clone(),
                completed_at,
            },
        },
        mode: contract.mode.clone(),
        outcome: ObservationOutcome {
            control_phase: serde_json::to_value(phase)?
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
            objective,
            passed,
            data_availability,
            error,
        },
        samples,
        evidence: ObservationEvidence {
            results_sha256: result_sha256,
            manifest_sha256: report
                .and_then(|report| report.manifest.as_ref())
                .map(|reference| reference.sha256.clone()),
            artifacts,
        },
        provenance: ObservationProvenance {
            subject_model: record.request.model.clone(),
            subject_provider: record.request.provider.clone(),
            system_under_test,
        },
    };
    observation.validate()?;
    Ok(observation)
}

fn observation_samples(report: &E2eReport) -> Vec<ObservationSample> {
    report
        .scenarios
        .iter()
        .flat_map(|scenario| {
            let seed = scenario.case.as_ref().map_or(0, |case| case.seed);
            scenario.runs.iter().map(move |run| {
                let data_availability = match &run.efficiency {
                    Some(metrics) if metrics.unavailable.is_empty() => {
                        ObservationDataAvailability::Complete
                    }
                    Some(_) => ObservationDataAvailability::Partial,
                    None => ObservationDataAvailability::Unavailable,
                };
                let derivations = run
                    .efficiency
                    .as_ref()
                    .is_some_and(|metrics| metrics.work_amplification.is_some())
                    .then(|| ObservationMetricDerivation {
                        metric: "work_amplification".into(),
                        origin: ObservationMetricOrigin::DerivedFromObserved,
                        formula: "observed_work / max(minimum_expected_work, 1)".into(),
                        formula_version: "1".into(),
                    })
                    .into_iter()
                    .collect();
                ObservationSample {
                    scenario_id: scenario.scenario_id.clone(),
                    scenario_version: scenario.scenario_version,
                    case_id: scenario.case_id.clone(),
                    seed,
                    run_id: run.run_id.clone(),
                    attempt_id: run.attempt_id.clone(),
                    status: run.status,
                    origin: ObservationMetricOrigin::Observed,
                    data_availability,
                    metrics: run.efficiency.clone(),
                    metric_values: observation_metric_values(
                        run.efficiency.as_ref(),
                        &run.scenario_measurements,
                    ),
                    derivations,
                }
            })
        })
        .collect()
}

fn observation_metric_values(
    metrics: Option<&crate::report::EfficiencyReport>,
    scenario_measurements: &[crate::report::ScenarioMeasurement],
) -> BTreeMap<String, ObservationMetric> {
    const METRICS: &[(&str, &str)] = &[
        ("wall_time_ms", "ms"),
        ("root_turns", "turns"),
        ("child_turns", "turns"),
        ("child_sessions", "sessions"),
        ("function_calls", "calls"),
        ("function_call_errors", "errors"),
        ("validation_retries", "retries"),
        ("transient_resumes", "resumes"),
        ("wake_resumes", "resumes"),
        ("effective_fan_out", "branches"),
        ("critical_path_ms", "ms"),
        ("input_tokens", "tokens"),
        ("output_tokens", "tokens"),
        ("total_tokens", "tokens"),
        ("cost_usd", "USD"),
        ("minimum_expected_work", "work_units"),
        ("observed_work", "work_units"),
        ("work_amplification", "ratio"),
        ("technical_attempts", "attempts"),
    ];
    let mut values = METRICS
        .iter()
        .map(|(name, unit)| {
            let (value, origin) = match (metrics, *name) {
                (Some(metrics), "wall_time_ms") => (
                    Some(metrics.wall_time_ms as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "root_turns") => (
                    metrics.root_turns.map(|value| value as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "child_turns") => (
                    metrics.child_turns.map(|value| value as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "child_sessions") => (
                    metrics.child_sessions.map(|value| value as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "function_calls") => (
                    metrics.function_calls.map(|value| value as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "function_call_errors") => (
                    metrics.function_call_errors.map(|value| value as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "validation_retries") => (
                    metrics.validation_retries.map(|value| value as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "transient_resumes") => (
                    metrics.transient_resumes.map(|value| value as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "wake_resumes") => (
                    metrics.wake_resumes.map(|value| value as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "effective_fan_out") => (
                    metrics.effective_fan_out.map(|value| value as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "critical_path_ms") => (
                    metrics.critical_path_ms.map(|value| value as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "input_tokens") => (
                    metrics.input_tokens.map(|value| value as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "output_tokens") => (
                    metrics.output_tokens.map(|value| value as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "total_tokens") => (
                    metrics.total_tokens.map(|value| value as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "cost_usd") => {
                    (metrics.cost_usd, ObservationMetricOrigin::Observed)
                }
                (Some(metrics), "minimum_expected_work") => (
                    Some(metrics.minimum_expected_work as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "observed_work") => (
                    metrics.observed_work.map(|value| value as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (Some(metrics), "work_amplification") => (
                    metrics.work_amplification,
                    ObservationMetricOrigin::DerivedFromObserved,
                ),
                (Some(metrics), "technical_attempts") => (
                    Some(metrics.technical_attempts as f64),
                    ObservationMetricOrigin::Observed,
                ),
                (None, _) => (None, ObservationMetricOrigin::Observed),
                _ => (None, ObservationMetricOrigin::Observed),
            };
            let availability = match (metrics, value) {
                (None, _) => ObservationDataAvailability::Unavailable,
                (Some(metrics), Some(_)) if !metrics.unavailable.contains_key(*name) => {
                    ObservationDataAvailability::Complete
                }
                (Some(_metrics), Some(_)) => ObservationDataAvailability::Partial,
                (Some(metrics), None) if metrics.unavailable.contains_key(*name) => {
                    ObservationDataAvailability::Unavailable
                }
                (Some(_), None) => ObservationDataAvailability::Partial,
            };
            (
                (*name).into(),
                ObservationMetric {
                    value,
                    unit: (*unit).into(),
                    availability,
                    origin,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    for measurement in scenario_measurements {
        values.insert(
            measurement.id.clone(),
            ObservationMetric {
                value: Some(measurement.value),
                unit: measurement.unit.clone(),
                availability: ObservationDataAvailability::Complete,
                origin: measurement.origin,
            },
        );
    }
    values
}

fn observation_data_availability(samples: &[ObservationSample]) -> ObservationDataAvailability {
    if samples.is_empty()
        || samples
            .iter()
            .all(|sample| sample.data_availability == ObservationDataAvailability::Unavailable)
    {
        ObservationDataAvailability::Unavailable
    } else if samples
        .iter()
        .all(|sample| sample.data_availability == ObservationDataAvailability::Complete)
    {
        ObservationDataAvailability::Complete
    } else {
        ObservationDataAvailability::Partial
    }
}

fn observation_objective(
    phase: ExecutionPhase,
    report: Option<&E2eReport>,
) -> ObservationObjective {
    match phase {
        ExecutionPhase::Cancelled => ObservationObjective::Cancelled,
        ExecutionPhase::Unsupported => ObservationObjective::UnsupportedPlan,
        ExecutionPhase::Failed => ObservationObjective::InfrastructureFailed,
        ExecutionPhase::Completed => {
            let Some(report) = report else {
                return ObservationObjective::InfrastructureFailed;
            };
            if report.passed {
                return ObservationObjective::Passed;
            }
            let statuses = report
                .scenarios
                .iter()
                .flat_map(|scenario| scenario.runs.iter().map(|run| run.status))
                .collect::<Vec<_>>();
            if statuses.contains(&crate::report::RunStatus::InfrastructureError) {
                ObservationObjective::InfrastructureFailed
            } else if statuses.iter().any(|status| status.is_technical_failure()) {
                ObservationObjective::TechnicalFailed
            } else {
                ObservationObjective::HardGateFailed
            }
        }
        _ => ObservationObjective::InfrastructureFailed,
    }
}

fn status_response(record: &ExecutionRecord) -> StatusResponse {
    StatusResponse {
        execution_id: record.execution_id.clone(),
        phase: record.phase,
        terminal: record.phase.terminal(),
        cancel_requested: record.cancel_requested,
        active_attempt: record.active_attempt.clone(),
        error: record.error.clone(),
        updated_at: record.updated_at.clone(),
        transitions: record.transitions.clone(),
    }
}

fn compare_records(
    from: &ExecutionRecord,
    to: &ExecutionRecord,
    policy: ComparisonPolicy,
) -> Result<crate::longitudinal::ComparisonSummary> {
    let from_report = from
        .report
        .as_ref()
        .context("from execution has no completed report")?;
    let to_report = to
        .report
        .as_ref()
        .context("to execution has no completed report")?;
    longitudinal::compare_reports(
        &from.execution_id,
        &from.request.lane,
        from_report,
        &to.execution_id,
        &to.request.lane,
        to_report,
        policy,
    )
}

fn relative_result_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> RunRequest {
        RunRequest {
            _caller_worker_id: None,
            idempotency_key: "gate:sha:case".into(),
            label: "PR gate".into(),
            lane: "pr-gate".into(),
            model: "model".into(),
            provider: "provider".into(),
            judge_model: None,
            judge_provider: None,
            audit_model: None,
            audit_provider: None,
            scenarios: vec![ScenarioId::ContextPressure.into()],
            local_markdown_scenarios: Vec::new(),
            runs: 1,
            seed: Some(42),
            rotating_seeds: Vec::new(),
            technical_retries: 1,
            progress_interval_seconds: 15,
            run_contract: None,
        }
    }

    #[test]
    fn run_request_accepts_engine_caller_metadata_without_persisting_it() {
        let mut value = serde_json::to_value(request()).unwrap();
        value["_caller_worker_id"] = serde_json::json!("worker-1");

        let with_caller: RunRequest = serde_json::from_value(value).unwrap();
        assert_eq!(with_caller._caller_worker_id.as_deref(), Some("worker-1"));
        assert!(!serde_json::to_value(&with_caller)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("_caller_worker_id"));
        assert_eq!(
            crate::artifact::sha256_value(&with_caller).unwrap(),
            crate::artifact::sha256_value(&request()).unwrap()
        );
    }

    #[test]
    fn local_scenario_create_accepts_engine_caller_metadata_without_serializing_it() {
        let request: LocalScenarioCreateRequest = serde_json::from_value(serde_json::json!({
            "_caller_worker_id": "console-1",
            "file_name": "console-draft.md",
            "source": "# Console draft"
        }))
        .unwrap();

        assert_eq!(request._caller_worker_id.as_deref(), Some("console-1"));
        assert_eq!(request.file_name, "console-draft.md");
        assert!(!serde_json::to_value(&request)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("_caller_worker_id"));
    }

    #[test]
    fn authoring_guide_separates_definition_creation_from_execution() {
        let guide = scenario_authoring_guide();

        assert_eq!(guide.schema, "e2e-local-scenario-authoring/v1");
        assert!(!guide.creation_starts_execution);
        assert!(!guide.overwrites_existing_file);
        assert_eq!(guide.create_function, SCENARIOS_CREATE_ID);
        assert_eq!(guide.list_function, SCENARIOS_LIST_ID);
        assert_eq!(guide.run_function, RUN_ID);
        assert_eq!(guide.required_plan, LOCAL_SCENARIO_PLAN_ID);
        assert_eq!(
            guide.required_h2_sections,
            LOCAL_SCENARIO_REQUIRED_SECTIONS.map(str::to_string)
        );
        crate::markdown::compile_local("local-scenarios/guide-example.md", &guide.template)
            .unwrap();
    }

    #[test]
    fn authoring_guide_request_is_closed() {
        let request = serde_json::from_value::<ScenarioAuthoringGuideRequest>(serde_json::json!({
            "_caller_worker_id": "cli"
        }))
        .unwrap();
        assert_eq!(request._caller_worker_id.as_deref(), Some("cli"));
        assert!(!serde_json::to_value(&request)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("_caller_worker_id"));
        assert!(
            serde_json::from_value::<ScenarioAuthoringGuideRequest>(serde_json::json!({
                "execute": true
            }))
            .is_err()
        );
    }

    fn d0_request() -> RunRequest {
        let mut request = request();
        let catalog = scenarios_list(ScenariosListRequest { seed: request.seed }).unwrap();
        let scenario = catalog
            .scenarios
            .iter()
            .find(|scenario| scenario.scenario_id == ScenarioId::ContextPressure.into())
            .unwrap()
            .clone();
        request.run_contract = Some(ObservationRunContract {
            schema_version: 1,
            mode: crate::report::ObservationMode {
                environment: crate::report::ObservationEnvironment::Demonstration,
                decision: crate::report::ObservationDecision::ObserveOnly,
            },
            target: crate::report::ObservationTargetIdentity {
                application: "harness".into(),
                version: "1.0.0".into(),
                stack: crate::identity::StackIdentity::Source {
                    workers_repository: "iii-hq/workers".into(),
                    workers_revision: "0123456789abcdef0123456789abcdef01234567".into(),
                },
            },
            plan: crate::report::ObservationPlanIdentity {
                id: "deployment-d0".into(),
                revision: "revision-1".into(),
                sha256: format!("sha256:{}", "a".repeat(64)),
                catalog_sha256: catalog.catalog_sha256,
            },
            runner: catalog.runner,
            attempt: 1,
            selected_cases: vec![ObservationSelectedCase {
                scenario_id: scenario.scenario_id,
                scenario_version: scenario.scenario_version,
                case_id: scenario.case_id.clone(),
                seed: scenario.seed,
                inputs_sha256: scenario.inputs_sha256.clone(),
                contract_sha256: scenario.contract_sha256.clone(),
            }],
            correlation: crate::report::ObservationCorrelation {
                system: "release-control".into(),
                deployment_id: "deployment-1".into(),
                operation_id: "operation-1".into(),
            },
        });
        request.idempotency_key = observation_idempotency_key(&request).unwrap();
        request
    }

    #[test]
    fn request_validation_requires_a_bounded_idempotency_key() {
        validate_run_request(&request()).unwrap();
        let mut invalid = request();
        invalid.idempotency_key.clear();
        assert!(validate_run_request(&invalid).is_err());
        invalid.idempotency_key = "x".repeat(MAX_IDEMPOTENCY_KEY_BYTES + 1);
        assert!(validate_run_request(&invalid).is_err());
    }

    #[test]
    fn execution_identity_is_stable_per_idempotency_key() {
        let first = execution_id_for_key("release:123:case");
        assert_eq!(first, execution_id_for_key("release:123:case"));
        assert_ne!(first, execution_id_for_key("release:124:case"));
        assert_eq!(first.len(), 32);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn scenarios_list_materializes_versioned_cases() {
        let response = scenarios_list(ScenariosListRequest { seed: Some(7) }).unwrap();
        assert_eq!(
            response.scenarios.len(),
            crate::markdown::all_keys().unwrap().len()
        );
        for scenario in &response.scenarios {
            let id = scenario.scenario_id.clone();
            let expected_seed = if id.canonical_seed_only() {
                id.canonical_seed()
            } else {
                7
            };
            assert_eq!(scenario.seed, expected_seed);
            assert!(scenario
                .case_id
                .contains(&format!("seed-{expected_seed:016x}")));
        }
        assert_eq!(response.schema, CATALOG_SCHEMA);
        assert_eq!(response.schema, "e2e-scenario-catalog/v4");
        assert!(response.catalog_sha256.starts_with("sha256:"));
        assert!(response.scenarios.iter().all(|scenario| {
            scenario.inputs_sha256.starts_with("sha256:")
                && scenario.contract_sha256.starts_with("sha256:")
                && scenario.classification.method
                    == crate::scenarios::ComplexityMethod::CapabilityV2
        }));
        let git = response
            .scenarios
            .iter()
            .find(|scenario| scenario.scenario_id == ScenarioId::GitRegressionForensics.into())
            .unwrap();
        assert_eq!(
            git.characterization.realism.execution,
            crate::scenarios::ExecutionRealism::FrozenRealArtifact
        );
        let incident = response
            .scenarios
            .iter()
            .find(|scenario| scenario.scenario_id == ScenarioId::IncidentResponse.into())
            .unwrap();
        let workflow = incident.resource_envelope.workflow.as_ref().unwrap();
        assert_eq!(workflow.max_parallel, 3);
        assert_eq!(workflow.max_total_tokens, Some(686_000));
        assert_eq!(workflow.max_cost_usd, Some(25.0));
        assert_eq!(workflow.technical_retries, 0);
        let markdown = response
            .scenarios
            .iter()
            .find(|scenario| scenario.scenario_id.as_str() == "insert_record")
            .unwrap();
        assert_eq!(markdown.origin, ScenarioOrigin::Markdown);
        assert_eq!(markdown.plans, ["daily", "weekly"]);
        assert_eq!(markdown.author_version, Some(1));
        assert!(markdown
            .source_sha256
            .as_deref()
            .unwrap()
            .starts_with("sha256:"));
    }

    #[test]
    fn scenarios_list_includes_local_markdown_from_the_worker_data_directory() {
        let root = tempfile::tempdir().unwrap();
        let source = "# Console draft\n\n## Plans\n\n- local\n\n## Version\n\n1\n\n## Before Test\n\nPrepare isolated state.\n\n## Prompt\n\nComplete the local task.\n\n## Validations\n\n### Correct result (100%)\n\nThe requested result exists.\n";
        crate::markdown::create_local_scenario(root.path(), "console-draft.md", source).unwrap();

        let response =
            scenarios_list_with_local(root.path(), ScenariosListRequest { seed: Some(9) }).unwrap();
        let local = response
            .scenarios
            .iter()
            .find(|scenario| scenario.scenario_id.as_str() == "local_console_draft")
            .unwrap();
        assert_eq!(local.origin, ScenarioOrigin::Local);
        assert_eq!(local.title.as_deref(), Some("Console draft"));
        assert_eq!(local.plans, ["local"]);
        assert_eq!(local.seed, 9);
        assert!(local
            .source_path
            .as_deref()
            .unwrap()
            .starts_with("local-scenarios/"));
        assert!(response.catalog_sha256.starts_with("sha256:"));
    }

    #[test]
    fn local_markdown_is_frozen_for_execution_and_unknown_ids_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let source = "# Frozen draft\n\n## Plans\n\n- local\n\n## Version\n\n1\n\n## Before Test\n\nPrepare isolated state.\n\n## Prompt\n\nComplete the frozen task.\n\n## Validations\n\n### Correct result (100%)\n\nThe requested result exists.\n";
        crate::markdown::create_local_scenario(root.path(), "frozen-draft.md", source).unwrap();
        let selected = vec!["local_frozen_draft".parse::<ScenarioKey>().unwrap()];

        let frozen = resolve_local_markdown_scenarios(root.path(), &selected, false).unwrap();
        assert_eq!(frozen.len(), 1);
        assert_eq!(frozen[0].source, source);
        assert_eq!(
            artifact::sha256_bytes(frozen[0].source.as_bytes()),
            frozen[0].scenario.source_sha256
        );
        let mut local_request = request();
        local_request.lane = "local".into();
        local_request.scenarios = selected;
        local_request.local_markdown_scenarios = frozen;
        local_request.judge_model = Some("judge".into());
        local_request.judge_provider = Some("provider".into());
        validate_run_request(&local_request).unwrap();
        assert!(resolve_local_markdown_scenarios(
            root.path(),
            &["local_missing".parse().unwrap()],
            false,
        )
        .is_err());
        assert!(resolve_local_markdown_scenarios(
            root.path(),
            &[ScenarioId::ContextPressure.into()],
            false,
        )
        .unwrap()
        .is_empty());
        assert_eq!(
            resolve_local_markdown_scenarios(
                root.path(),
                &[ScenarioId::ContextPressure.into()],
                true,
            )
            .unwrap()
            .len(),
            1
        );
    }

    #[test]
    fn catalog_digest_is_stable_for_the_same_runtime_materialization() {
        let first = scenarios_list(ScenariosListRequest { seed: Some(7) }).unwrap();
        let second = scenarios_list(ScenariosListRequest { seed: Some(7) }).unwrap();
        let changed = scenarios_list(ScenariosListRequest { seed: Some(8) }).unwrap();
        assert_eq!(first.catalog_sha256, second.catalog_sha256);
        assert_ne!(first.catalog_sha256, changed.catalog_sha256);
        assert_eq!(first.runner, RunnerIdentity::runtime());
    }

    #[test]
    fn d0_preflight_binds_runner_catalog_and_selected_cases() {
        let request = d0_request();
        validate_run_request(&request).unwrap();
        preflight_run_contract(&request).unwrap();

        let mut wrong_runner = request.clone();
        wrong_runner.run_contract.as_mut().unwrap().runner.version = "other".into();
        assert!(preflight_run_contract(&wrong_runner)
            .unwrap_err()
            .to_string()
            .contains("identity mismatch"));

        let mut wrong_case = request;
        wrong_case.run_contract.as_mut().unwrap().selected_cases[0].inputs_sha256 =
            format!("sha256:{}", "b".repeat(64));
        assert!(preflight_run_contract(&wrong_case)
            .unwrap_err()
            .to_string()
            .contains("selected cases"));
    }

    #[test]
    fn d0_idempotency_key_is_bound_to_the_canonical_intent() {
        let request = d0_request();
        assert_eq!(
            request.idempotency_key,
            observation_idempotency_key(&request).unwrap()
        );
        let mut tampered = request;
        tampered.model = "other-model".into();
        assert!(validate_run_request(&tampered)
            .unwrap_err()
            .to_string()
            .contains("D0 idempotency_key"));
    }

    #[test]
    fn request_and_run_contract_hashes_change_for_an_explicit_rerun() {
        let first = d0_request();
        let mut rerun = first.clone();
        rerun.run_contract.as_mut().unwrap().attempt = 2;
        assert_ne!(
            artifact::sha256_value(&first).unwrap(),
            artifact::sha256_value(&rerun).unwrap()
        );
        assert_ne!(
            artifact::sha256_value(first.run_contract.as_ref().unwrap()).unwrap(),
            artifact::sha256_value(rerun.run_contract.as_ref().unwrap()).unwrap()
        );
    }

    #[test]
    fn unsupported_d0_execution_still_produces_a_terminal_observation() {
        let request = d0_request();
        let record = ExecutionRecord {
            execution_id: "execution-1".into(),
            idempotency_key: request.idempotency_key.clone(),
            phase: ExecutionPhase::Admitted,
            requested_at: "2026-08-18T10:00:00Z".into(),
            updated_at: "2026-08-18T10:00:00Z".into(),
            request_sha256: artifact::sha256_value(&request).unwrap(),
            run_contract_sha256: Some(
                artifact::sha256_value(request.run_contract.as_ref().unwrap()).unwrap(),
            ),
            request,
            lane_budget: lane_budget("pr-gate"),
            transitions: Vec::new(),
            active_attempt: None,
            resume_state_path: None,
            resume_state_sha256: None,
            cancel_requested: false,
            error: String::new(),
            result_path: None,
            report: None,
            manifest: None,
            observation: None,
            observation_artifact: None,
            archive: None,
        };
        let output = tempfile::tempdir().unwrap();
        let observation = terminal_observation(
            &record,
            ExecutionPhase::Unsupported,
            "catalog mismatch",
            None,
            None,
            None,
            output.path(),
        )
        .unwrap();
        assert_eq!(observation.schema, OBSERVATION_SCHEMA);
        assert_eq!(
            observation.outcome.objective,
            ObservationObjective::UnsupportedPlan
        );
        assert_eq!(
            observation.outcome.data_availability,
            ObservationDataAvailability::Unavailable
        );
        observation.write_to(output.path()).unwrap();
        E2eObservationEnvelope::read_from(output.path()).unwrap();
    }

    #[test]
    fn observation_metrics_preserve_observed_zero_separately_from_unavailable() {
        let metrics: crate::report::EfficiencyReport = serde_json::from_value(json!({
            "wall_time_ms": 0,
            "root_turns": 0,
            "minimum_expected_work": 1,
            "technical_attempts": 1,
            "observed_complexity": {},
            "unavailable": {}
        }))
        .unwrap();
        let observed = ObservationSample {
            scenario_id: "direct_answer".into(),
            scenario_version: 1,
            case_id: "case".into(),
            seed: 1,
            run_id: "run".into(),
            attempt_id: "attempt".into(),
            status: crate::report::RunStatus::Passed,
            origin: ObservationMetricOrigin::Observed,
            data_availability: ObservationDataAvailability::Complete,
            metrics: Some(metrics.clone()),
            metric_values: observation_metric_values(Some(&metrics), &[]),
            derivations: Vec::new(),
        };
        let unavailable = ObservationSample {
            metrics: None,
            data_availability: ObservationDataAvailability::Unavailable,
            ..observed.clone()
        };
        let value = serde_json::to_value(&observed).unwrap();
        assert_eq!(value["metrics"]["root_turns"], 0);
        assert_eq!(value["metrics"]["input_tokens"], Value::Null);
        assert_eq!(value["metric_values"]["wall_time_ms"]["value"], 0.0);
        assert_eq!(value["metric_values"]["wall_time_ms"]["unit"], "ms");
        assert_eq!(
            value["metric_values"]["wall_time_ms"]["availability"],
            "complete"
        );
        assert_eq!(value["metric_values"]["input_tokens"]["value"], Value::Null);
        assert_eq!(
            value["metric_values"]["input_tokens"]["availability"],
            "partial"
        );
        assert_eq!(
            observation_data_availability(&[observed, unavailable]),
            ObservationDataAvailability::Partial
        );
    }

    #[test]
    fn terminal_phases_cannot_be_reopened_by_late_checkpoints() {
        assert!(ExecutionPhase::Completed.terminal());
        assert!(ExecutionPhase::Cancelled.terminal());
        assert!(ExecutionPhase::NeedsReconciliation.terminal());
        assert!(!ExecutionPhase::Finalizing.terminal());
    }

    #[test]
    fn lane_budget_rejects_pr_gate_repetitions() {
        let mut request = request();
        request.lane = "pr-gate".into();
        request.runs = 2;
        assert!(validate_run_request(&request)
            .unwrap_err()
            .to_string()
            .contains("1 to 1 runs"));
    }

    #[test]
    fn lane_budget_counts_rotating_seeds_as_distinct_cases() {
        let mut request = request();
        request.rotating_seeds = (100..108).collect();

        assert_eq!(
            validate_run_request(&request).unwrap_err().to_string(),
            "lane 'pr-gate' permits at most 8 cases, got 9"
        );
    }

    #[test]
    fn security_review_is_admitted_only_without_technical_retries() {
        let mut request = request();
        request.lane = "local".into();
        request.scenarios = vec![ScenarioId::SecurityReview.into()];
        request.technical_retries = 0;
        validate_run_request(&request).expect("security review should use the control plane");

        request.technical_retries = 1;
        assert_eq!(
            validate_run_request(&request).unwrap_err().to_string(),
            "non-replayable scenarios require technical_retries=0"
        );
    }

    #[test]
    fn todo_worker_scenarios_are_admitted_by_the_control_plane() {
        let mut simple = request();
        simple.lane = "local".into();
        simple.scenarios = vec![ScenarioId::TodoWorkerSimple.into()];
        simple.technical_retries = 0;
        validate_run_request(&simple).expect("todo_worker_simple should be Console-admitted");

        let mut planned = request();
        planned.lane = "local".into();
        planned.scenarios = vec![ScenarioId::TodoWorkerPlanned.into()];
        planned.technical_retries = 0;
        validate_run_request(&planned)
            .expect("planned Todo worker should be Console-admitted without technical retries");
    }

    #[test]
    fn subject_policy_hides_control_functions() {
        let spec = ScenarioId::ContextPressure.spec("policy");
        let policy = crate::suite::e2e_function_policy(&spec, "test-run");
        assert!(policy.deny.contains(&"e2e::*".to_string()));
    }

    #[test]
    fn execution_identity_is_stable_for_the_same_idempotency_key() {
        assert_eq!(
            execution_id_for_key("release:123:case"),
            "5d26613d37528e723253324a6bf5bd97"
        );
    }
}
