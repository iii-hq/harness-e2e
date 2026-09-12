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
    ControlPlane, ExecutionPhase, ExecutionRecord, LocalScenarioCreateRequest,
    LocalScenarioCreateResponse, RunRequest as ControlRunRequest, ScenariosListRequest,
    ScenariosListResponse,
};
use crate::markdown::ScenarioKey;
use crate::plans::{LocalPlan, PlanCreateRequest, PlanRunRole, PlanUpdateRequest};

const MAX_LOG_TAIL_BYTES: u64 = 256 * 1024;
const MAX_LOG_CHUNK_BYTES: u64 = 64 * 1024;

struct ControllerState {
    job: Option<RunMetadata>,
}

pub(super) struct Controller {
    pub(super) plan_store: Arc<crate::plans::store::PlanStore>,
    runs_dir: PathBuf,
    defaults: Defaults,
    control: Option<ControlPlane>,
    state: Mutex<ControllerState>,
    read_model: RwLock<Option<Arc<DashboardReadModel>>>,
    events: Option<Arc<DashboardEvents>>,
}

impl Controller {
    pub(super) async fn open_history_evidence(
        &self,
        request: crate::history::evidence::EvidenceRequest,
    ) -> Result<Value> {
        self.control
            .as_ref()
            .context("E2E control database is unavailable")?
            .persistence()
            .open_history_evidence(request)
            .await
    }
    pub(super) async fn new(
        url: String,
        runs_dir: PathBuf,
        events: Option<Arc<DashboardEvents>>,
        control: Option<ControlPlane>,
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
            crate::plans::store::PlanStore::new(runs_dir.clone(), url.clone(), control.clone())
                .await?;
        let controller = Arc::new(Self {
            plan_store,
            runs_dir,
            defaults: Defaults {
                url,
                model: env::var("HARNESS_E2E_MODEL").unwrap_or_default(),
                provider: env::var("HARNESS_E2E_PROVIDER").unwrap_or_default(),
                judge_model: env::var("HARNESS_E2E_JUDGE_MODEL").unwrap_or_default(),
                judge_provider: env::var("HARNESS_E2E_JUDGE_PROVIDER").unwrap_or_default(),
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
        Ok(controller)
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
        if let Some(detail) = control.persistence().imported_execution_detail(id).await? {
            return Ok(detail);
        }
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
        if let Some(model) = self.read_model.write().await.as_mut() {
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

    pub(super) async fn create_local_scenario(
        &self,
        request: LocalScenarioCreateRequest,
    ) -> Result<LocalScenarioCreateResponse> {
        self.control
            .as_ref()
            .context("the E2E control plane is not available")?
            .create_local_scenario(request)
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
        if let Some(control) = &self.control {
            summaries.extend(control.persistence().imported_execution_summaries().await?);
        }
        summaries.sort_by(|a, b| b["started_at"].as_str().cmp(&a["started_at"].as_str()));
        Ok(Arc::new(summaries))
    }

    pub(super) async fn read_model(&self) -> Result<Arc<DashboardReadModel>> {
        if let Some(model) = self.read_model.read().await.as_ref() {
            return Ok(model.clone());
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
            *self.read_model.write().await = Some(model.clone());
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
        let model = Arc::new(DashboardReadModel::from_records(records)?);
        *self.read_model.write().await = Some(model.clone());
        Ok(model)
    }

    async fn invalidate_summaries(&self) {
        self.read_model.write().await.take();
    }

    pub(super) async fn list_plans(&self) -> Result<Vec<Value>> {
        let mut plans = self
            .plan_store
            .list_local()
            .await?
            .into_iter()
            .map(|plan| {
                let mut value = serde_json::to_value(plan)?;
                value["origin"] = json!("local");
                Ok(value)
            })
            .collect::<Result<Vec<Value>>>()?;
        if let Some(control) = &self.control {
            plans.extend(control.persistence().imported_plans().await?);
        }
        plans.sort_by(|a, b| b["updated_at"].as_str().cmp(&a["updated_at"].as_str()));
        Ok(plans)
    }

    pub(super) async fn get_plan(&self, id: &str) -> Result<Value> {
        validate_plan_id(id)?;
        if let Some(control) = &self.control {
            if let Some(plan) = control.persistence().imported_plan(id).await? {
                return Ok(plan);
            }
        }
        let mut plan = serde_json::to_value(self.plan_store.get_local(id).await?)?;
        plan["origin"] = json!("local");
        Ok(plan)
    }

    pub(super) async fn create_plan(&self, request: PlanCreateRequest) -> Result<LocalPlan> {
        self.require_current_url(&request.url)?;
        self.plan_store.create_local(request).await
    }

    pub(super) async fn update_plan(
        &self,
        id: &str,
        update: PlanUpdateRequest,
    ) -> Result<LocalPlan> {
        validate_plan_id(id)?;
        self.plan_store.update_local(id, update).await
    }

    pub(super) async fn delete_plan(&self, id: &str) -> Result<()> {
        validate_plan_id(id)?;
        self.plan_store.get_local(id).await?;
        self.plan_store.delete_local(id).await
    }

    pub(super) async fn delete_execution(&self, id: &str) -> Result<()> {
        super::presenter::validate_execution_id(id).map_err(anyhow::Error::msg)?;
        if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("only native control-plane executions can be deleted");
        }
        let control = self
            .control
            .as_ref()
            .context("the E2E control plane is not available")?;
        control.delete(id).await?;
        let mut state = self.state.lock().await;
        if state.job.as_ref().is_some_and(|job| job.id == id) {
            state.job = None;
        }
        drop(state);
        self.invalidate_summaries().await;
        self.emit_change("deleted", id).await;
        Ok(())
    }

    pub(super) async fn start_plan(
        self: &Arc<Self>,
        id: &str,
        role: PlanRunRole,
        idempotency_key: &str,
    ) -> Result<LocalPlan> {
        validate_plan_id(id)?;
        self.plan_store.start_local(id, idempotency_key, role).await
    }

    async fn emit_change(&self, kind: &str, execution_id: &str) {
        if let Some(events) = &self.events {
            events.emit(kind, execution_id).await;
        }
    }

    pub(super) async fn start(self: &Arc<Self>, mut request: RunRequest) -> Result<String> {
        validate_request(&mut request).map_err(anyhow::Error::msg)?;
        self.require_current_url(&request.url)?;
        let control = self
            .control
            .as_ref()
            .context("the E2E control plane is not available")?;
        let control_request = control_request(&request).map_err(anyhow::Error::msg)?;
        let accepted = control.run(control_request).await?;
        let record = control.record(&accepted.execution_id).await?;
        let mut metadata = metadata_from_record(&record);
        metadata.request = request;
        self.set_current_job(metadata).await;
        self.invalidate_summaries().await;
        self.emit_change("started", &accepted.execution_id).await;
        Ok(accepted.execution_id)
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

    fn require_current_url(&self, url: &str) -> Result<()> {
        if url.trim() != self.defaults.url {
            bail!(
                "execution URL must match the worker stack {}",
                self.defaults.url
            );
        }
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

pub(super) fn control_request(
    request: &RunRequest,
) -> std::result::Result<ControlRunRequest, String> {
    let scenarios = request
        .scenarios
        .iter()
        .map(|value| {
            value
                .parse::<ScenarioKey>()
                .map_err(|_| format!("unknown scenario '{value}'"))
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let (judge_model, judge_provider) = match (
        request.judge_model.is_empty(),
        request.judge_provider.is_empty(),
    ) {
        (true, true) => (None, None),
        (false, false) => (
            Some(request.judge_model.clone()),
            Some(request.judge_provider.clone()),
        ),
        _ => return Err("judge_model and judge_provider must be supplied together".into()),
    };
    Ok(ControlRunRequest {
        _caller_worker_id: None,
        idempotency_key: format!("dashboard:{}", uuid::Uuid::new_v4().simple()),
        label: request.label.clone(),
        lane: "local".into(),
        model: request.model.clone(),
        provider: request.provider.clone(),
        judge_model,
        judge_provider,
        audit_model: None,
        audit_provider: None,
        scenarios,
        local_markdown_scenarios: Vec::new(),
        runs: request.runs,
        seed: request.seed,
        rotating_seeds: Vec::new(),
        technical_retries: request.technical_retries,
        progress_interval_seconds: 15,
        slot_start_deadline_seconds: None,
        run_contract: None,
    })
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
            judge_model: record.request.judge_model.clone().unwrap_or_default(),
            judge_provider: record.request.judge_provider.clone().unwrap_or_default(),
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

fn validate_plan_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("plan id is invalid");
    }
    Ok(())
}

pub(super) fn validate_request(request: &mut RunRequest) -> std::result::Result<(), String> {
    request.label = request.label.trim().to_string();
    request.url = request.url.trim().to_string();
    request.model = request.model.trim().to_string();
    request.provider = request.provider.trim().to_string();
    request.judge_model = request.judge_model.trim().to_string();
    request.judge_provider = request.judge_provider.trim().to_string();
    validate_stack_url(&request.url).map_err(|error| error.to_string())?;
    if request.label.len() > 120 || request.label.chars().any(char::is_control) {
        return Err("label is invalid".into());
    }
    for (name, value) in [("model", &request.model), ("provider", &request.provider)] {
        if value.is_empty() || value.len() > 200 || value.chars().any(char::is_control) {
            return Err(format!("{name} is invalid"));
        }
    }
    if request.judge_model.is_empty() != request.judge_provider.is_empty() {
        return Err("judge_model and judge_provider must be supplied together".into());
    }
    if !(1..=20).contains(&request.runs) {
        return Err("runs must be between 1 and 20".into());
    }
    if request.technical_retries > 3 {
        return Err("technical_retries must be between 0 and 3".into());
    }
    if request.scenarios.is_empty() || request.scenarios.len() > 256 {
        return Err("select at least one valid scenario".into());
    }
    request.scenarios.sort();
    request.scenarios.dedup();
    let selected = request
        .scenarios
        .iter()
        .map(|value| {
            value
                .parse::<ScenarioKey>()
                .map_err(|_| "request contains an unknown scenario".to_string())
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if selected
        .iter()
        .any(|scenario| scenario.built_in().is_none())
        && request.judge_model.is_empty()
    {
        return Err("Markdown scenarios require an explicit judge model and provider".into());
    }
    Ok(())
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
