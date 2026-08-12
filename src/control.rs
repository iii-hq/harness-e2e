use std::collections::HashMap;
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
use tokio::sync::{mpsc, watch, Mutex, RwLock};

use crate::context::E2eContext;
use crate::durable::{
    ArchiveHeadResponse, ArchiveResponse, ArchiveRestoreResponse, DurableArchiveReference,
    DurableHistory, HistoryListRequest, RetentionClass, RetentionSweepRequest, ARCHIVE_HEAD_ID,
    ARCHIVE_ID, ARCHIVE_RESTORE_ID, HISTORY_LIST_ID, RETENTION_SWEEP_ID,
};
use crate::judge::JudgeConfig;
use crate::longitudinal::{self, ComparisonPolicy, ComparisonResponse, PromotedBaselineIdentity};
use crate::report::{E2eManifestV2, E2eReport};
use crate::scenarios::{ComplexityProfile, DeliverableContract, ScenarioId};
use crate::suite::{
    run_suite, SubjectConfig, SuiteControl, SuiteEvent, SuiteEventEnvelope, SuitePhase,
    SuiteRunConfig,
};

pub const CONTROL_CONTRACT_NAME: &str = "e2e-control-plane";
pub const CONTROL_CONTRACT_VERSION: u32 = 3;
pub const RUN_ID: &str = "e2e::run";
pub const STATUS_ID: &str = "e2e::status";
pub const CANCEL_ID: &str = "e2e::cancel";
pub const RESULTS_GET_ID: &str = "e2e::results-get";
pub const RESULTS_LIST_ID: &str = "e2e::results-list";
pub const COMPARE_ID: &str = "e2e::compare";
pub const SCENARIOS_LIST_ID: &str = "e2e::scenarios-list";
pub const BASELINE_PROMOTE_ID: &str = "e2e::baseline-promote";
pub const BASELINE_GET_ID: &str = "e2e::baseline-get";

const RECORD_SCOPE: &str = "harness_e2e_execution_v1";
const BASELINE_SCOPE: &str = "harness_e2e_baseline_v1";
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
    Unsupported,
}

impl ExecutionPhase {
    fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Unsupported
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
    pub scenario_id: ScenarioId,
    pub attempt_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionRecord {
    pub schema_version: u32,
    pub execution_id: String,
    pub idempotency_key: String,
    pub phase: ExecutionPhase,
    pub requested_at: String,
    pub updated_at: String,
    pub request: RunRequest,
    pub lane_budget: LaneBudget,
    pub transitions: Vec<PhaseTransition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_attempt: Option<ActiveAttempt>,
    pub cancel_requested: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<E2eReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<E2eManifestV2>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<DurableArchiveReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunRequest {
    pub idempotency_key: String,
    #[serde(default = "default_lane")]
    pub lane: String,
    pub model: String,
    pub provider: String,
    #[serde(default)]
    pub judge_model: Option<String>,
    #[serde(default)]
    pub judge_provider: Option<String>,
    #[serde(default)]
    pub scenarios: Vec<ScenarioId>,
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
    #[serde(default)]
    pub allow_legacy_control_plane: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LaneBudget {
    pub policy_version: u32,
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
    pub manifest: Option<E2eManifestV2>,
    pub archive: Option<DurableArchiveReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveExecutionRequest {
    pub execution_id: String,
    pub retention_class: RetentionClass,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ResultsListRequest {
    #[serde(default)]
    pub lane: Option<String>,
    #[serde(default)]
    pub scenario_id: Option<ScenarioId>,
    #[serde(default = "default_results_limit")]
    pub limit: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResultsListResponse {
    pub executions: Vec<StatusResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompareRequest {
    pub current_execution_id: String,
    pub baseline: BaselineGetRequest,
    #[serde(default)]
    pub policy: ComparisonPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BaselinePromotionRequest {
    pub name: String,
    pub execution_id: String,
    pub expected_current_version: u32,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BaselineGetRequest {
    pub name: String,
    #[serde(default)]
    pub version: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BaselineRecord {
    pub schema_version: u32,
    pub baseline_id: String,
    pub name: String,
    pub version: u32,
    pub execution_id: String,
    pub report_sha256: String,
    pub promoted_at: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct BaselinePointer {
    name: String,
    version: u32,
    baseline_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BaselineGetResponse {
    pub baseline: BaselineRecord,
    pub current_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScenariosListRequest {
    #[serde(default)]
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScenarioDescriptor {
    pub scenario_id: ScenarioId,
    pub scenario_version: u32,
    pub case_id: String,
    pub seed: u64,
    pub complexity: ComplexityProfile,
    pub required_capabilities: Vec<String>,
    pub deliverable_contract: DeliverableContract,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScenariosListResponse {
    pub scenarios: Vec<ScenarioDescriptor>,
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
    durable: DurableHistory,
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
        let control = Self {
            inner: Arc::new(ControlPlaneInner {
                durable: DurableHistory::from_client(iii.clone()),
                iii,
                url,
                output_root,
                admission: Mutex::new(()),
                records: RwLock::new(HashMap::new()),
                cancellations: Mutex::new(HashMap::new()),
            }),
        };
        control.restore().await?;
        Ok(control)
    }

    pub fn register(&self) {
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
            BASELINE_PROMOTE_ID,
            "Promote an immutable, versioned E2E execution baseline.",
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: BaselinePromotionRequest| {
                    let control = control.clone();
                    async move {
                        control
                            .baseline_promote(request)
                            .await
                            .map_err(handler_error)
                    }
                })
            },
        );
        register_function(
            &self.inner.iii,
            BASELINE_GET_ID,
            "Read a current or versioned immutable E2E baseline.",
            {
                let control = self.clone();
                RegisterFunction::new_async(move |request: BaselineGetRequest| {
                    let control = control.clone();
                    async move { control.baseline_get(request).await.map_err(handler_error) }
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
        register_function(
            &self.inner.iii,
            SCENARIOS_LIST_ID,
            "List versioned E2E cases, contracts, capabilities and complexity.",
            RegisterFunction::new_async(move |request: ScenariosListRequest| async move {
                scenarios_list(request).map_err(handler_error)
            }),
        );
    }

    async fn run(&self, request: RunRequest) -> Result<RunAccepted> {
        let lane_budget = validate_run_request(&request)?;
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
            schema_version: CONTROL_CONTRACT_VERSION,
            execution_id: execution_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            phase: ExecutionPhase::Requested,
            requested_at: now.clone(),
            updated_at: now.clone(),
            request: request.clone(),
            lane_budget,
            transitions: vec![PhaseTransition {
                phase: ExecutionPhase::Requested,
                at: now,
                reason: "request accepted for admission".into(),
            }],
            active_attempt: None,
            cancel_requested: false,
            error: String::new(),
            result_path: None,
            report: None,
            manifest: None,
            archive: None,
        };
        self.persist_record(record).await?;
        self.transition(
            &execution_id,
            ExecutionPhase::Admitted,
            "execution admitted",
        )
        .await?;
        self.spawn_execution(execution_id.clone(), request).await;
        Ok(RunAccepted {
            execution_id,
            phase: ExecutionPhase::Admitted,
            duplicate: false,
        })
    }

    async fn spawn_execution(&self, execution_id: String, request: RunRequest) {
        let (cancellation, receiver) = watch::channel(false);
        self.inner
            .cancellations
            .lock()
            .await
            .insert(execution_id.clone(), cancellation);
        let control = self.clone();
        tokio::spawn(async move {
            control.execute(execution_id, request, receiver).await;
        });
    }

    async fn execute(
        &self,
        execution_id: String,
        request: RunRequest,
        cancellation: watch::Receiver<bool>,
    ) {
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
            ScenarioId::ALL.to_vec()
        } else {
            unique_scenarios(&request.scenarios)
        };
        let judge = judge_config(&request, &scenarios);
        let outcome = run_suite(SuiteRunConfig {
            url: self.inner.url.clone(),
            subject: SubjectConfig {
                model: request.model.clone(),
                provider: request.provider.clone(),
            },
            judge,
            output: output.clone(),
            scenarios,
            runs: request.runs,
            seed: request.seed,
            rotating_seeds: request.rotating_seeds.clone(),
            technical_retries: request.technical_retries,
            progress_interval: (request.progress_interval_seconds > 0)
                .then(|| Duration::from_secs(request.progress_interval_seconds)),
            allow_legacy_control_plane: request.allow_legacy_control_plane,
            control: Some(SuiteControl {
                execution_id: execution_id.clone(),
                lane: request.lane.clone(),
                events,
                cancellation: cancellation.clone(),
            }),
        })
        .await;
        checkpoint_task.abort();

        let cancelled = *cancellation.borrow();
        let result = match outcome {
            Ok(outcome) => {
                self.finish(
                    &execution_id,
                    if cancelled {
                        ExecutionPhase::Cancelled
                    } else {
                        ExecutionPhase::Completed
                    },
                    String::new(),
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
                self.finish(
                    &execution_id,
                    if cancelled || format!("{error:#}").contains("was cancelled") {
                        ExecutionPhase::Cancelled
                    } else {
                        ExecutionPhase::Failed
                    },
                    format!("{error:#}"),
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
                attempt_id,
                session_id,
            } => {
                self.update_record(execution_id, |record| {
                    record.active_attempt = Some(ActiveAttempt {
                        scenario_id: *scenario_id,
                        attempt_id: attempt_id.clone(),
                        session_id: session_id.clone(),
                    });
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

    async fn status(&self, execution_id: &str) -> Result<StatusResponse> {
        Ok(status_response(&self.record(execution_id).await?))
    }

    async fn cancel(&self, execution_id: &str) -> Result<CancelResponse> {
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

    async fn results_get(&self, execution_id: &str) -> Result<ResultsGetResponse> {
        let record = self.record(execution_id).await?;
        Ok(ResultsGetResponse {
            execution_id: record.execution_id,
            phase: record.phase,
            result_path: record.result_path,
            report: record.report,
            manifest: record.manifest,
            archive: record.archive,
        })
    }

    async fn archive(&self, request: ArchiveExecutionRequest) -> Result<ArchiveResponse> {
        let record = self.record(&request.execution_id).await?;
        if record.phase != ExecutionPhase::Completed {
            bail!("only a completed execution can be archived");
        }
        let report = record
            .report
            .as_ref()
            .context("completed execution has no report")?;
        let output = self.inner.output_root.join(&record.execution_id);
        let response = self
            .inner
            .durable
            .archive(&output, report, request.retention_class)
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

    async fn results_list(&self, request: ResultsListRequest) -> Result<ResultsListResponse> {
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
                    && request.scenario_id.is_none_or(|scenario| {
                        record.request.scenarios.is_empty()
                            || record.request.scenarios.contains(&scenario)
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
        let current = self.record(&request.current_execution_id).await?;
        let promoted = self.baseline_get(request.baseline).await?.baseline;
        let baseline = self.record(&promoted.execution_id).await?;
        let mut comparison = compare_records(&current, &baseline, request.policy)?;
        comparison.promoted_baseline = Some(PromotedBaselineIdentity {
            baseline_id: promoted.baseline_id,
            name: promoted.name,
            version: promoted.version,
            execution_id: promoted.execution_id,
            report_sha256: promoted.report_sha256,
            promoted_at: promoted.promoted_at,
        });
        longitudinal::refresh_comparison_id(&mut comparison)?;
        let artifacts = longitudinal::write_comparison(
            &self.inner.output_root.join(&current.execution_id),
            &comparison,
        )?;
        Ok(ComparisonResponse {
            comparison,
            artifacts,
        })
    }

    async fn baseline_promote(&self, request: BaselinePromotionRequest) -> Result<BaselineRecord> {
        validate_baseline_name(&request.name)?;
        let _admission = self.inner.admission.lock().await;
        let execution = self.record(&request.execution_id).await?;
        if execution.phase != ExecutionPhase::Completed {
            bail!("only a completed execution can be promoted as a baseline");
        }
        let report = execution
            .report
            .as_ref()
            .context("completed execution has no report")?;
        let current = self.baseline_pointer(&request.name).await?;
        let current_version = current.as_ref().map_or(0, |pointer| pointer.version);
        if request.expected_current_version != current_version {
            bail!(
                "baseline '{}' current version is {}, expected {}",
                request.name,
                current_version,
                request.expected_current_version
            );
        }
        let version = current_version
            .checked_add(1)
            .context("baseline version overflow")?;
        let report_sha256 = crate::artifact::sha256_value(report)?;
        let baseline_id = crate::artifact::sha256_value(&json!({
            "name": request.name,
            "version": version,
            "execution_id": request.execution_id,
            "report_sha256": report_sha256,
        }))?;
        let candidate = BaselineRecord {
            schema_version: 1,
            baseline_id: baseline_id.clone(),
            name: request.name.clone(),
            version,
            execution_id: request.execution_id,
            report_sha256,
            promoted_at: now(),
            note: request.note,
        };
        let immutable_key = baseline_version_key(&candidate.name, version);
        let existing = state_value(
            self.trigger(
                "state::get",
                json!({ "scope": BASELINE_SCOPE, "key": immutable_key }),
            )
            .await?,
        );
        let record = if existing.is_null() {
            self.trigger(
                "state::set",
                json!({
                    "scope": BASELINE_SCOPE,
                    "key": baseline_version_key(&candidate.name, version),
                    "value": candidate,
                }),
            )
            .await
            .context("persist immutable baseline record")?;
            candidate
        } else {
            let existing: BaselineRecord = serde_json::from_value(existing)
                .context("decode existing immutable baseline record")?;
            if existing.baseline_id != candidate.baseline_id {
                bail!(
                    "baseline '{}' version {} already exists with different evidence",
                    candidate.name,
                    version
                );
            }
            existing
        };
        self.trigger(
            "state::set",
            json!({
                "scope": BASELINE_SCOPE,
                "key": baseline_current_key(&record.name),
                "value": BaselinePointer {
                    name: record.name.clone(),
                    version,
                    baseline_id,
                },
            }),
        )
        .await
        .context("advance baseline pointer")?;
        Ok(record)
    }

    async fn baseline_get(&self, request: BaselineGetRequest) -> Result<BaselineGetResponse> {
        validate_baseline_name(&request.name)?;
        let current = self
            .baseline_pointer(&request.name)
            .await?
            .with_context(|| format!("unknown baseline '{}'", request.name))?;
        let version = request.version.unwrap_or(current.version);
        let value = state_value(
            self.trigger(
                "state::get",
                json!({
                    "scope": BASELINE_SCOPE,
                    "key": baseline_version_key(&request.name, version),
                }),
            )
            .await?,
        );
        if value.is_null() {
            bail!("unknown baseline '{}' version {}", request.name, version);
        }
        let baseline: BaselineRecord =
            serde_json::from_value(value).context("decode immutable baseline record")?;
        if baseline.name != request.name || baseline.version != version {
            bail!("baseline record identity does not match its immutable key");
        }
        if version == current.version && baseline.baseline_id != current.baseline_id {
            bail!("current baseline pointer does not match its immutable record");
        }
        let execution = self.record(&baseline.execution_id).await?;
        let report = execution
            .report
            .as_ref()
            .context("baseline execution has no report")?;
        if crate::artifact::sha256_value(report)? != baseline.report_sha256 {
            bail!("baseline report hash does not match the promoted evidence");
        }
        Ok(BaselineGetResponse {
            baseline,
            current_version: current.version,
        })
    }

    async fn baseline_pointer(&self, name: &str) -> Result<Option<BaselinePointer>> {
        let value = state_value(
            self.trigger(
                "state::get",
                json!({ "scope": BASELINE_SCOPE, "key": baseline_current_key(name) }),
            )
            .await?,
        );
        if value.is_null() {
            return Ok(None);
        }
        Ok(Some(
            serde_json::from_value(value).context("decode baseline pointer")?,
        ))
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
        if let Some(cleanup) = active.scenario_id.spec(&active.attempt_id).cleanup {
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
        manifest: Option<E2eManifestV2>,
        result_path: Option<String>,
    ) -> Result<()> {
        self.update_record(execution_id, move |record| {
            record.phase = phase;
            record.error = error;
            record.report = report;
            record.manifest = manifest;
            record.result_path = result_path;
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

    async fn record(&self, execution_id: &str) -> Result<ExecutionRecord> {
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
            .insert(record.execution_id.clone(), record);
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
                "version": CONTROL_CONTRACT_VERSION,
                "capabilities": [id.trim_start_matches("e2e::")],
            }
        })),
    );
}

fn handler_error(error: anyhow::Error) -> Error {
    Error::Handler(format!("{error:#}"))
}

fn execution_id_for_key(idempotency_key: &str) -> String {
    let digest = Sha256::digest(
        format!("{CONTROL_CONTRACT_NAME}:v{CONTROL_CONTRACT_VERSION}:{idempotency_key}").as_bytes(),
    );
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
        ScenarioId::ALL.to_vec()
    } else {
        unique_scenarios(&request.scenarios)
    };
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
    let turns_per_run = scenarios.iter().try_fold(0_u64, |total, scenario| {
        total.checked_add(u64::from(scenario.spec("budget").execution.max_turns))
    });
    let declared_turns = turns_per_run
        .and_then(|turns| turns.checked_mul(seed_count.try_into().unwrap_or(u64::MAX)))
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
    Ok(budget)
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
        policy_version: 2,
        max_cases,
        max_runs_per_case,
        max_technical_retries,
        max_declared_turns,
    }
}

fn judge_config(request: &RunRequest, scenarios: &[ScenarioId]) -> Option<JudgeConfig> {
    let required = scenarios
        .iter()
        .any(|scenario| scenario.spec("judge-check").needs_judge());
    if !required && request.judge_model.is_none() && request.judge_provider.is_none() {
        return None;
    }
    Some(JudgeConfig {
        model: request
            .judge_model
            .clone()
            .unwrap_or_else(|| request.model.clone()),
        provider: request
            .judge_provider
            .clone()
            .unwrap_or_else(|| request.provider.clone()),
    })
}

fn unique_scenarios(scenarios: &[ScenarioId]) -> Vec<ScenarioId> {
    scenarios
        .iter()
        .copied()
        .fold(Vec::new(), |mut result, id| {
            if !result.contains(&id) {
                result.push(id);
            }
            result
        })
}

fn scenarios_list(request: ScenariosListRequest) -> Result<ScenariosListResponse> {
    let scenarios = ScenarioId::ALL
        .into_iter()
        .map(|scenario_id| {
            let seed = request.seed.unwrap_or_else(|| scenario_id.canonical_seed());
            let materialized = scenario_id.materialize("catalog", seed)?;
            Ok(ScenarioDescriptor {
                scenario_id,
                scenario_version: materialized.case.scenario_version,
                case_id: materialized.case.case_id,
                seed,
                complexity: materialized.case.complexity.profile,
                required_capabilities: materialized.case.required_capabilities,
                deliverable_contract: materialized.case.deliverable_contract,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(ScenariosListResponse { scenarios })
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
    current: &ExecutionRecord,
    baseline: &ExecutionRecord,
    policy: ComparisonPolicy,
) -> Result<crate::longitudinal::ComparisonSummary> {
    let current_report = current
        .report
        .as_ref()
        .context("current execution has no completed report")?;
    let baseline_report = baseline
        .report
        .as_ref()
        .context("baseline execution has no completed report")?;
    longitudinal::compare_reports(
        &current.execution_id,
        &current.request.lane,
        current_report,
        &baseline.execution_id,
        &baseline.request.lane,
        baseline_report,
        policy,
    )
}

fn validate_baseline_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 80
        || !name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        bail!(
            "baseline name must contain 1 to 80 lowercase ASCII letters, digits, '.', '-' or '_'"
        );
    }
    Ok(())
}

fn baseline_version_key(name: &str, version: u32) -> String {
    format!("{name}:v{version}")
}

fn baseline_current_key(name: &str) -> String {
    format!("{name}:current")
}

fn state_value(response: Value) -> Value {
    match response {
        Value::Object(mut object)
            if object.get("ok").and_then(Value::as_bool) == Some(true)
                && object.contains_key("value") =>
        {
            object.remove("value").unwrap_or(Value::Null)
        }
        response => response,
    }
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
            idempotency_key: "gate:sha:case".into(),
            lane: "pr-gate".into(),
            model: "model".into(),
            provider: "provider".into(),
            judge_model: None,
            judge_provider: None,
            scenarios: vec![ScenarioId::PersistentState],
            runs: 1,
            seed: Some(42),
            rotating_seeds: Vec::new(),
            technical_retries: 1,
            progress_interval_seconds: 15,
            allow_legacy_control_plane: false,
        }
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
        assert_eq!(response.scenarios.len(), ScenarioId::ALL.len());
        assert!(response.scenarios.iter().all(|scenario| scenario.seed == 7));
        assert!(response
            .scenarios
            .iter()
            .all(|scenario| scenario.case_id.contains("seed-0000000000000007")));
    }

    #[test]
    fn terminal_phases_cannot_be_reopened_by_late_checkpoints() {
        assert!(ExecutionPhase::Completed.terminal());
        assert!(ExecutionPhase::Cancelled.terminal());
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
    fn subject_policy_hides_control_functions() {
        let spec = ScenarioId::DirectAnswer.spec("policy");
        let policy = crate::suite::e2e_function_policy(&spec);
        assert!(policy.deny.contains(&"e2e::*".to_string()));
    }

    #[test]
    fn baseline_names_and_keys_are_bounded_and_deterministic() {
        for name in ["daily", "coordination-l4", "release_1.2"] {
            validate_baseline_name(name).unwrap();
        }
        for name in ["", "Daily", "has space", "path/escape"] {
            assert!(validate_baseline_name(name).is_err());
        }
        assert!(validate_baseline_name(&"a".repeat(81)).is_err());
        assert_eq!(baseline_version_key("daily", 7), "daily:v7");
        assert_eq!(baseline_current_key("daily"), "daily:current");
    }

    #[test]
    fn state_values_accept_native_and_wrapped_iii_shapes() {
        assert_eq!(state_value(json!({"value": 1})), json!({"value": 1}));
        assert_eq!(
            state_value(json!({"ok": true, "value": {"version": 2}})),
            json!({"version": 2})
        );
        assert_eq!(state_value(Value::Null), Value::Null);
    }
}
