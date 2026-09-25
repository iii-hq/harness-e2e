use std::env;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex, RwLock};
use url::Url;

use super::bus::DashboardEvents;
use super::read_model::DashboardReadModel;
use super::{Defaults, JobStatus, JobView, RunMetadata, RunRequest, RunSnapshot};
use crate::control::{
    ControlPlane, ExecutionPhase, ExecutionRecord, ScenariosListRequest, ScenariosListResponse,
};
use crate::plans::stacks::{StackCreateRequest, StackUpdateRequest, StackView};
use crate::plans::store::{
    ExecutionParameters, GithubRunContractsRequest, GithubRunImportRequest, GithubRunsListRequest,
    SuiteView,
};
use crate::plans::{SuiteCreateRequest, SuiteUpdateRequest};

const MAX_LOG_TAIL_BYTES: u64 = 256 * 1024;
const MAX_LOG_CHUNK_BYTES: u64 = 64 * 1024;

struct ControllerState {
    job: Option<RunMetadata>,
}

pub(super) struct Controller {
    pub(super) plan_store: Arc<crate::plans::store::PlanStore>,
    github_repository: String,
    runs_dir: PathBuf,
    defaults: Defaults,
    control: Option<ControlPlane>,
    state: Mutex<ControllerState>,
    /// The read model and the previous-attempts revision it left out.
    read_model: RwLock<Option<(u64, Arc<DashboardReadModel>)>>,
    events: Option<Arc<DashboardEvents>>,
}

impl Controller {
    pub(super) async fn new(
        url: String,
        runs_dir: PathBuf,
        events: Option<Arc<DashboardEvents>>,
        control: Option<ControlPlane>,
        github_repository: String,
        docker: crate::plans::store::DockerSettings,
    ) -> Result<Arc<Self>> {
        validate_stack_url(&url)?;
        fs::create_dir_all(&runs_dir).with_context(|| format!("create {}", runs_dir.display()))?;
        if let Some(control) = control.as_ref() {
            if control.url() != url {
                bail!(
                    "dashboard URL {} differs from the control-plane URL {}",
                    url,
                    control.url()
                );
            }
            if control.output_root() != runs_dir {
                bail!(
                    "dashboard runs directory {} differs from the control-plane data directory {}",
                    runs_dir.display(),
                    control.output_root().display()
                );
            }
        }
        let plan_store =
            crate::plans::store::PlanStore::new(runs_dir.clone(), control.clone(), docker).await?;
        let controller = Arc::new(Self {
            plan_store,
            github_repository,
            runs_dir,
            defaults: Defaults {
                url,
                model: env::var("HARNESS_E2E_MODEL").unwrap_or_default(),
                provider: env::var("HARNESS_E2E_PROVIDER").unwrap_or_default(),
                runs: 1,
                technical_retries: 1,
                seed: env::var("HARNESS_E2E_SEED")
                    .ok()
                    .and_then(|value| value.parse().ok()),
            },
            control,
            state: Mutex::new(ControllerState { job: None }),
            read_model: RwLock::new(None),
            events,
        });
        if let Some(control) = controller.control.as_ref() {
            for record in control.records().await? {
                controller.sync_control_record(record).await?;
            }
            controller.observe_control_plane();
        }
        controller.observe_docker_executions();
        Ok(controller)
    }

    /// A Docker execution changes in the background: its groups move, then
    /// its import installs native runs. Each change refreshes the summaries
    /// and tells the Console.
    fn observe_docker_executions(self: &Arc<Self>) {
        let mut changes = self.plan_store.changes();
        let controller = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let id = match changes.recv().await {
                    Ok(id) => id,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                };
                let Some(controller) = controller.upgrade() else {
                    return;
                };
                controller.invalidate_summaries().await;
                controller.emit_change("progress", &id).await;
            }
        });
    }

    pub(super) fn default_url(&self) -> &str {
        &self.defaults.url
    }

    pub(super) async fn scenario_catalog(&self) -> Result<ScenariosListResponse> {
        self.control
            .as_ref()
            .context("the E2E control plane is not available")?
            .scenario_catalog(ScenariosListRequest { seed: None })
            .await
    }

    pub(super) async fn execution_detail(&self, id: &str) -> Result<Value> {
        let control = self
            .control
            .as_ref()
            .context("the E2E control plane is not available")?;
        let record = control.stored_record(id).await?;
        let metadata = metadata_from_record(&record);
        let (hydrated, evidence_error) = match control.hydrate_native_evidence(record.clone()) {
            Ok(hydrated) => {
                let error = (hydrated.report.is_none() && record.dashboard_projection.is_some())
                    .then(|| "No native evidence path was retained for this execution".to_string());
                (hydrated, error)
            }
            Err(error) => (record, Some(format!("{error:#}"))),
        };
        let detail = if let Some(error) = evidence_error {
            let mut detail = match hydrated.dashboard_projection {
                Some(projection) => projection["summary"].clone(),
                None => super::presenter::execution_detail_value_optional(&metadata, None)?,
            };
            detail["reports"] = json!([]);
            detail["availability"] = json!("unavailable");
            detail["evidence_error"] = json!(error);
            detail
        } else {
            super::presenter::execution_detail_value_optional(&metadata, hydrated.report.as_ref())?
        };
        if let Some((_, model)) = self.read_model.write().await.as_mut() {
            if let Some(summary) = Arc::make_mut(model)
                .summaries
                .iter_mut()
                .find(|summary| summary["id"] == id)
            {
                summary["availability"] = detail["availability"].clone();
            }
        }
        Ok(detail)
    }

    pub(super) async fn attempt_get(
        &self,
        execution_id: &str,
        run_id: &str,
        attempt_id: &str,
    ) -> Result<Value> {
        self.control
            .as_ref()
            .context("the E2E control plane is not available")?
            .attempt_get(execution_id, run_id, attempt_id)
            .await
    }

    pub(super) async fn snapshot(&self, after: Option<u64>) -> Result<RunSnapshot> {
        let metadata = self.state.lock().await.job.clone();
        let job = metadata
            .map(|metadata| {
                let log = read_log_chunk(&self.runs_dir.join(&metadata.id).join("run.log"), after)?;
                Ok::<_, anyhow::Error>(JobView {
                    metadata,
                    log: log.content,
                    log_from: log.from,
                    log_offset: log.offset,
                    log_truncated: log.truncated,
                })
            })
            .transpose()?;
        Ok(RunSnapshot {
            job,
            defaults: self.defaults.clone(),
        })
    }

    pub(super) async fn execution_summaries(&self) -> Result<Arc<Vec<Value>>> {
        let mut summaries = self.read_model().await?.summaries.clone();
        let (parents, children) = self.plan_store.dashboard_summaries(&summaries).await?;
        #[cfg(test)]
        if self.control.is_none() {
            let runs_dir = self.runs_dir.clone();
            return tokio::task::spawn_blocking(move || {
                for summary in &mut summaries {
                    if matches!(summary["status"].as_str(), Some("running" | "cancelling")) {
                        if let Some(id) = summary["id"].as_str() {
                            if let Some(run) = super::store::read_stored_run(&runs_dir.join(id))? {
                                *summary = super::presenter::stored_execution_summary(&run)?;
                            }
                        }
                    }
                }
                for summary in &mut summaries {
                    if let Some(parent) = summary["id"].as_str().and_then(|id| children.get(id)) {
                        summary["parent_plan_execution_id"] = json!(parent);
                    }
                }
                summaries.extend(parents);
                summaries.sort_by(|a, b| b["started_at"].as_str().cmp(&a["started_at"].as_str()));
                Ok(Arc::new(summaries))
            })
            .await
            .context("refresh test execution summaries")?;
        }
        for summary in &mut summaries {
            if let Some(parent) = summary["id"].as_str().and_then(|id| children.get(id)) {
                summary["parent_plan_execution_id"] = json!(parent);
            }
        }
        summaries.extend(parents);
        summaries.sort_by(|a, b| b["started_at"].as_str().cmp(&a["started_at"].as_str()));
        Ok(Arc::new(summaries))
    }

    pub(super) async fn read_model(&self) -> Result<Arc<DashboardReadModel>> {
        // Read before the previous attempts, so a change between the two
        // builds the model again next time.
        let attempts = self.plan_store.attempts_revision();
        if let Some((revision, model)) = self.read_model.read().await.as_ref() {
            if *revision == attempts {
                return Ok(model.clone());
            }
        }
        #[cfg(test)]
        if self.control.is_none() {
            let runs_dir = self.runs_dir.clone();
            let model = Arc::new(
                tokio::task::spawn_blocking(move || DashboardReadModel::load(&runs_dir))
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!("load dashboard test model task: {error}")
                    })??,
            );
            *self.read_model.write().await = Some((attempts, model.clone()));
            return Ok(model);
        }
        let mut records = self
            .control
            .as_ref()
            .context("the E2E control plane is not available")?
            .records()
            .await?;
        for record in &mut records {
            if let Some(projection) = &mut record.dashboard_projection {
                let available = record.result_path.as_ref().is_some_and(|path| {
                    let path = self.runs_dir.join(path);
                    let result = if path.is_dir() {
                        path.join("results.json")
                    } else {
                        path
                    };
                    result.is_file()
                        && result
                            .parent()
                            .is_some_and(|parent| parent.join("manifest.json").is_file())
                });
                if !available {
                    projection["summary"]["availability"] = json!("unavailable");
                }
            }
        }
        let discarded = self.plan_store.previous_attempts().await?;
        let model = Arc::new(DashboardReadModel::from_records(records, &discarded)?);
        *self.read_model.write().await = Some((attempts, model.clone()));
        Ok(model)
    }

    async fn invalidate_summaries(&self) {
        self.read_model.write().await.take();
    }

    pub(super) async fn suites(&self) -> Result<Vec<SuiteView>> {
        self.plan_store.suites().await
    }

    pub(super) async fn create_suite(&self, request: SuiteCreateRequest) -> Result<SuiteView> {
        validate_suite_id(&request.from)?;
        self.plan_store.create_suite(request).await
    }

    pub(super) async fn update_suite(&self, request: SuiteUpdateRequest) -> Result<SuiteView> {
        validate_suite_id(&request.suite_id)?;
        self.plan_store.update_suite(request).await
    }

    pub(super) async fn delete_suite(&self, id: &str) -> Result<()> {
        validate_suite_id(id)?;
        self.plan_store.delete_suite(id).await
    }

    pub(super) async fn stacks(&self) -> Result<Vec<StackView>> {
        self.plan_store.stacks().await
    }

    pub(super) async fn create_stack(&self, request: StackCreateRequest) -> Result<StackView> {
        validate_stack_id(&request.from)?;
        self.plan_store.create_stack(request).await
    }

    pub(super) async fn update_stack(&self, request: StackUpdateRequest) -> Result<StackView> {
        validate_stack_id(&request.stack_id)?;
        self.plan_store.update_stack(request).await
    }

    pub(super) async fn delete_stack(&self, id: &str) -> Result<()> {
        validate_stack_id(id)?;
        self.plan_store.delete_stack(id).await
    }

    pub(super) async fn read_evidence(
        &self,
        request: super::bus::EvidenceReadRequest,
    ) -> Result<super::store::EvidenceFile> {
        super::presenter::validate_execution_id(&request.execution_id)
            .map_err(anyhow::Error::msg)?;
        let run_dir = self.runs_dir.join(&request.execution_id);
        tokio::task::spawn_blocking(move || {
            super::store::read_evidence(&run_dir, &request.path, request.pointer.as_deref())
        })
        .await
        .context("read evidence")?
    }

    pub(super) async fn rename_execution(&self, id: &str, label: &str) -> Result<Value> {
        let execution = self.plan_store.rename(id, label).await?;
        self.emit_change("renamed", id).await;
        Ok(serde_json::to_value(execution)?)
    }

    pub(super) async fn github_runs(&self, request: GithubRunsListRequest) -> Result<Value> {
        let repository = request
            .repository
            .unwrap_or_else(|| self.github_repository.clone());
        self.plan_store
            .github_runs(&repository, request.page.unwrap_or(1))
            .await
    }

    /// Whether `gh` on this worker's machine can dispatch to its repository.
    pub(super) async fn github_status(&self) -> Value {
        self.plan_store.github_status(&self.github_repository).await
    }

    /// Groups a Docker execution runs at once (worker configuration).
    pub(super) fn docker_parallel_groups(&self) -> usize {
        self.plan_store.docker_parallel_groups()
    }

    pub(super) async fn github_run_contracts(
        &self,
        request: GithubRunContractsRequest,
    ) -> Result<Value> {
        let repository = request
            .repository
            .unwrap_or_else(|| self.github_repository.clone());
        self.plan_store
            .github_run_contracts(&repository, &request.runs)
            .await
    }

    /// Answers with the execution at once; the download and installation
    /// continue in the background and end in `completed` or `failed`.
    pub(super) async fn github_run_import(
        self: &Arc<Self>,
        request: GithubRunImportRequest,
    ) -> Result<Value> {
        let repository = request
            .repository
            .unwrap_or_else(|| self.github_repository.clone());
        let (execution, started) = self
            .plan_store
            .begin_github_import(&repository, request.run_id)
            .await?;
        if started {
            self.emit_change("started", &execution.id).await;
            let controller = Arc::clone(self);
            let id = execution.id.clone();
            tokio::spawn(async move {
                if let Err(error) = controller.plan_store.finish_github_import(&id).await {
                    tracing::error!(execution_id = %id, error = %format!("{error:#}"), "record the GitHub import outcome");
                }
                controller.invalidate_summaries().await;
                controller.emit_change("finished", &id).await;
            });
        }
        Ok(json!({"execution_id": execution.id, "state": execution.state}))
    }

    pub(super) async fn delete_execution(&self, id: &str) -> Result<()> {
        super::presenter::validate_execution_id(id).map_err(anyhow::Error::msg)?;
        if id.starts_with("plan-") {
            self.plan_store.delete_execution(id).await?;
        } else {
            if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                bail!("only native runs and executions can be deleted");
            }
            self.control
                .as_ref()
                .context("the E2E control plane is not available")?
                .delete(id)
                .await?;
            let mut state = self.state.lock().await;
            if state.job.as_ref().is_some_and(|job| job.id == id) {
                state.job = None;
            }
        }
        self.invalidate_summaries().await;
        self.emit_change("deleted", id).await;
        Ok(())
    }

    async fn emit_change(&self, kind: &str, execution_id: &str) {
        if let Some(events) = &self.events {
            events.emit(kind, execution_id).await;
        }
    }

    /// Start an execution on this stack from its parameters; answers at once
    /// and runs its slots in the background.
    pub(super) async fn start_execution(
        &self,
        parameters: ExecutionParameters,
        label: &str,
    ) -> Result<Value> {
        let execution = self
            .plan_store
            .start_execution_in(parameters, label, Some(&self.github_repository))
            .await?;
        self.invalidate_summaries().await;
        self.emit_change("started", &execution.id).await;
        Ok(json!({"execution_id": execution.id}))
    }

    /// Run one scenario of a finished local execution again; answers at once
    /// and runs it in the background.
    pub(super) async fn rerun_scenario(&self, id: &str, scenario_id: &str) -> Result<Value> {
        super::presenter::validate_execution_id(id).map_err(anyhow::Error::msg)?;
        let execution = self.plan_store.rerun_scenario(id, scenario_id).await?;
        self.emit_change("started", &execution.id).await;
        Ok(json!({"execution_id": execution.id}))
    }

    /// Stop an execution: no next slot is admitted and the running one is cancelled.
    pub(super) async fn cancel_execution(&self, id: &str) -> Result<Value> {
        super::presenter::validate_execution_id(id).map_err(anyhow::Error::msg)?;
        let execution = self.plan_store.cancel(id).await?;
        self.emit_change("cancelling", id).await;
        Ok(execution)
    }

    pub(super) async fn cancel(&self) -> Result<()> {
        let control = self
            .control
            .as_ref()
            .context("the E2E control plane is not available")?;
        let current = self.state.lock().await.job.clone();
        let execution_id = if let Some(job) = current.filter(|job| job.status.active()) {
            job.id
        } else {
            control
                .records()
                .await?
                .into_iter()
                .find(|record| !record.phase.terminal())
                .map(|record| record.execution_id)
                .context("no E2E execution is running")?
        };
        let response = control.cancel(&execution_id).await?;
        if !response.accepted {
            bail!("no E2E execution is running");
        }
        let record = control.record(&execution_id).await?;
        self.sync_control_record(record).await?;
        Ok(())
    }

    fn observe_control_plane(self: &Arc<Self>) {
        let Some(control) = self.control.as_ref() else {
            return;
        };
        let mut updates = control.subscribe();
        let control = control.clone();
        let controller = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                match updates.recv().await {
                    Ok(update) => {
                        if let Err(error) = controller.sync_control_record(update.record).await {
                            tracing::error!(%error, "project control-plane execution into dashboard");
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        for record in control.records().await.unwrap_or_else(|error| {
                            tracing::error!(%error, "refresh persisted dashboard executions");
                            Vec::new()
                        }) {
                            if let Err(error) = controller.sync_control_record(record).await {
                                tracing::error!(%error, "resynchronize control-plane execution into dashboard");
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }

    async fn sync_control_record(&self, record: ExecutionRecord) -> Result<()> {
        let mut metadata = metadata_from_record(&record);
        metadata.request.url = self.defaults.url.clone();
        self.set_current_job(metadata.clone()).await;
        self.invalidate_summaries().await;
        self.emit_change(change_kind(&record), &record.execution_id)
            .await;
        Ok(())
    }

    async fn set_current_job(&self, metadata: RunMetadata) {
        let mut state = self.state.lock().await;
        let replace = state.job.as_ref().is_none_or(|current| {
            current.id == metadata.id || metadata.started_at >= current.started_at
        });
        if replace {
            state.job = Some(metadata);
        }
    }
}

pub(super) fn metadata_from_record(record: &ExecutionRecord) -> RunMetadata {
    let status = job_status(record);
    let label = if record.request.label.trim().is_empty() {
        "e2e::* control-plane run".into()
    } else {
        record.request.label.clone()
    };
    RunMetadata {
        id: record.execution_id.clone(),
        label: label.clone(),
        status,
        started_at: record.requested_at.clone(),
        completed_at: if record.phase.terminal() {
            record.updated_at.clone()
        } else {
            String::new()
        },
        returncode: match status {
            JobStatus::Completed => Some(0),
            JobStatus::Failed => Some(1),
            _ => None,
        },
        error: record.error.clone(),
        request: RunRequest {
            _caller_worker_id: None,
            label,
            url: String::new(),
            model: record.request.model.clone(),
            provider: record.request.provider.clone(),
            scenarios: record
                .request
                .scenarios
                .iter()
                .map(|scenario| scenario.as_str().to_string())
                .collect(),
            runs: record.request.runs,
            technical_retries: record.request.technical_retries,
            seed: record.request.seed,
        },
    }
}

fn job_status(record: &ExecutionRecord) -> JobStatus {
    match record.phase {
        ExecutionPhase::Completed => JobStatus::Completed,
        ExecutionPhase::Failed
        | ExecutionPhase::Unsupported
        | ExecutionPhase::NeedsReconciliation => JobStatus::Failed,
        ExecutionPhase::Cancelled => JobStatus::Cancelled,
        _ if record.cancel_requested => JobStatus::Cancelling,
        _ => JobStatus::Running,
    }
}

fn change_kind(record: &ExecutionRecord) -> &'static str {
    if record.phase.terminal() {
        "finished"
    } else if record.cancel_requested {
        "cancelling"
    } else if record.phase == ExecutionPhase::Requested {
        "started"
    } else {
        "progress"
    }
}

fn validate_suite_id(value: &str) -> Result<()> {
    if !valid_id(value) {
        bail!("suite id is invalid");
    }
    Ok(())
}

fn validate_stack_id(value: &str) -> Result<()> {
    if !valid_id(value) {
        bail!("stack id is invalid");
    }
    Ok(())
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(super) fn validate_stack_url(value: &str) -> Result<()> {
    let parsed = Url::parse(value).context("url must be a ws:// or wss:// endpoint")?;
    if !matches!(parsed.scheme(), "ws" | "wss") || parsed.host_str().is_none() {
        bail!("url must be a ws:// or wss:// endpoint");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("url must not contain credentials");
    }
    Ok(())
}

struct LogChunk {
    content: String,
    from: u64,
    offset: u64,
    truncated: bool,
}

fn read_log_chunk(path: &Path, after: Option<u64>) -> Result<LogChunk> {
    if !path.is_file() {
        return Ok(LogChunk {
            content: String::new(),
            from: 0,
            offset: 0,
            truncated: false,
        });
    }
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let requested = after.unwrap_or_else(|| length.saturating_sub(MAX_LOG_TAIL_BYTES));
    let bounded = if after.is_some() {
        length.saturating_sub(MAX_LOG_CHUNK_BYTES)
    } else {
        length.saturating_sub(MAX_LOG_TAIL_BYTES)
    };
    let from = requested.min(length).max(bounded);
    file.seek(SeekFrom::Start(from))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(LogChunk {
        content: String::from_utf8_lossy(&bytes).into_owned(),
        from,
        offset: length,
        truncated: from > requested,
    })
}
