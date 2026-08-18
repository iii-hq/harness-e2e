use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex, RwLock};
use url::Url;

use super::bus::DashboardEvents;
use super::plans::{self, LocalPlan, PlanCreateRequest, PlanRunRole, PlanState, PlanUpdateRequest};
use super::read_model::DashboardReadModel;
use super::store::{read_metadata, read_report, recover_interrupted_runs, write_metadata};
use super::{
    ApiError, DashboardArgs, Defaults, JobStatus, JobView, RunMetadata, RunRequest, RunSnapshot,
};
use crate::artifact;
use crate::control::{
    ControlPlane, ExecutionPhase, ExecutionRecord, RunRequest as ControlRunRequest,
};
use crate::report::{E2eReport, RunStatus};
use crate::scenarios::{ScenarioExecutionKind, ScenarioId};

const MAX_LOG_TAIL_BYTES: u64 = 256 * 1024;
const MAX_LOG_CHUNK_BYTES: u64 = 64 * 1024;

struct ControllerState {
    job: Option<RunMetadata>,
}

pub(super) struct Controller {
    runs_dir: PathBuf,
    plans_dir: PathBuf,
    defaults: Defaults,
    control: Option<ControlPlane>,
    state: Mutex<ControllerState>,
    pending_plan_contexts: RwLock<HashMap<String, plans::PlanContext>>,
    plan_lock: Mutex<()>,
    read_model: RwLock<Option<Arc<DashboardReadModel>>>,
    events: Option<Arc<DashboardEvents>>,
}

impl Controller {
    pub(super) async fn new(
        args: DashboardArgs,
        events: Option<Arc<DashboardEvents>>,
        control: Option<ControlPlane>,
    ) -> Result<Arc<Self>> {
        validate_stack_url(&args.url)?;
        fs::create_dir_all(&args.runs_dir)
            .with_context(|| format!("create {}", args.runs_dir.display()))?;
        let plans_dir = plans::plans_dir(&args.runs_dir);
        fs::create_dir_all(&plans_dir)
            .with_context(|| format!("create {}", plans_dir.display()))?;
        let recovered_runs = recover_interrupted_runs(&args.runs_dir)?;
        for metadata in &recovered_runs {
            if let Some(context) = metadata
                .plan_context
                .as_ref()
                .or(metadata.request.plan_context.as_ref())
            {
                plans::record_incomplete_attempt(&plans_dir, context, &metadata.id)?;
            }
        }
        if let Some(control) = control.as_ref() {
            if control.url() != args.url {
                bail!(
                    "dashboard URL {} differs from the control-plane URL {}",
                    args.url,
                    control.url()
                );
            }
            if control.output_root() != args.runs_dir {
                bail!(
                    "dashboard runs directory {} differs from the control-plane data directory {}",
                    args.runs_dir.display(),
                    control.output_root().display()
                );
            }
        }
        let controller = Arc::new(Self {
            runs_dir: args.runs_dir,
            plans_dir,
            defaults: Defaults {
                url: args.url,
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
            pending_plan_contexts: RwLock::new(HashMap::new()),
            plan_lock: Mutex::new(()),
            read_model: RwLock::new(None),
            events,
        });
        if let Some(control) = controller.control.as_ref() {
            for record in control.records().await {
                controller.sync_control_record(record).await?;
            }
            controller.observe_control_plane();
        }
        Ok(controller)
    }

    pub(super) fn runs_dir(&self) -> &Path {
        &self.runs_dir
    }

    pub(super) fn default_url(&self) -> &str {
        &self.defaults.url
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
        Ok(Arc::new(self.read_model().await?.summaries.clone()))
    }

    pub(super) async fn read_model(&self) -> Result<Arc<DashboardReadModel>> {
        if let Some(model) = self.read_model.read().await.as_ref() {
            return Ok(model.clone());
        }
        let runs_dir = self.runs_dir.clone();
        let model = Arc::new(
            tokio::task::spawn_blocking(move || DashboardReadModel::load(&runs_dir))
                .await
                .map_err(|error| anyhow::anyhow!("load dashboard read model task: {error}"))??,
        );
        *self.read_model.write().await = Some(model.clone());
        Ok(model)
    }

    async fn invalidate_summaries(&self) {
        self.read_model.write().await.take();
    }

    pub(super) async fn list_plans(&self) -> Result<Vec<LocalPlan>> {
        let plans_dir = self.plans_dir.clone();
        tokio::task::spawn_blocking(move || plans::list_plans(&plans_dir))
            .await
            .context("list local plans task")?
    }

    pub(super) async fn get_plan(&self, id: &str) -> Result<LocalPlan> {
        validate_plan_id(id)?;
        let plans_dir = self.plans_dir.clone();
        let id = id.to_string();
        let lookup_id = id.clone();
        tokio::task::spawn_blocking(move || plans::read_plan(&plans_dir, &lookup_id))
            .await
            .context("read local plan task")??
            .with_context(|| format!("local plan '{id}' not found"))
    }

    pub(super) async fn create_plan(
        &self,
        request: PlanCreateRequest,
    ) -> Result<LocalPlan, ApiError> {
        self.require_current_url(&request.url)?;
        let _plan_guard = self.plan_lock.lock().await;
        let id = format!(
            "plan-{}-{}",
            Utc::now().format("%Y%m%dT%H%M%S"),
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let plan = plans::new_plan(&request, id)
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
        plans::write_plan(&self.plans_dir, &plan).map_err(ApiError::internal)?;
        Ok(plan)
    }

    pub(super) async fn update_plan(
        &self,
        id: &str,
        update: PlanUpdateRequest,
    ) -> Result<LocalPlan, ApiError> {
        let _plan_guard = self.plan_lock.lock().await;
        validate_plan_id(id).map_err(|error| ApiError::bad_request(error.to_string()))?;
        let mut plan = self
            .get_plan(id)
            .await
            .map_err(|error| ApiError::not_found(error.to_string()))?;
        plans::apply_update(&mut plan, &update)
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
        self.require_current_url(&plan.url)?;
        plans::write_plan(&self.plans_dir, &plan).map_err(ApiError::internal)?;
        Ok(plan)
    }

    pub(super) async fn start_plan(
        self: &Arc<Self>,
        id: &str,
        role: PlanRunRole,
    ) -> Result<LocalPlan, ApiError> {
        let _plan_guard = self.plan_lock.lock().await;
        validate_plan_id(id).map_err(|error| ApiError::bad_request(error.to_string()))?;
        let mut plan = self
            .get_plan(id)
            .await
            .map_err(|error| ApiError::not_found(error.to_string()))?;
        self.require_current_url(&plan.url)?;
        match role {
            PlanRunRole::Baseline
                if plan.state != PlanState::Draft || plan.baseline_execution_id.is_some() =>
            {
                return Err(ApiError::conflict(
                    "the plan already has a baseline or baseline attempt",
                ));
            }
            PlanRunRole::Candidate
                if plan.baseline_execution_id.is_none() || plan.state == PlanState::Draft =>
            {
                return Err(ApiError::conflict(
                    "a completed baseline is required before a candidate",
                ));
            }
            _ => {}
        }
        let request = plans::run_request(&plan, role);
        let execution_id = self.start(request).await?;
        plan.locked = true;
        plan.last_attempt_id = Some(execution_id.clone());
        plan.state = match role {
            PlanRunRole::Baseline => PlanState::BaselineRunning,
            PlanRunRole::Candidate => PlanState::CandidateRunning,
        };
        plan.updated_at = plans::now();
        plans::write_plan(&self.plans_dir, &plan).map_err(ApiError::internal)?;
        Ok(plan)
    }

    async fn emit_change(&self, kind: &str, execution_id: &str) {
        if let Some(events) = &self.events {
            events.emit(kind, execution_id).await;
        }
    }

    pub(super) async fn start(
        self: &Arc<Self>,
        mut request: RunRequest,
    ) -> Result<String, ApiError> {
        validate_request(&mut request).map_err(ApiError::bad_request)?;
        self.require_current_url(&request.url)?;
        if let Some(context) = request.plan_context.as_ref() {
            self.validate_plan_context(context).await?;
        }
        let control = self
            .control
            .as_ref()
            .ok_or_else(|| ApiError::conflict("the E2E control plane is not available"))?;
        let control_request = control_request(&request).map_err(ApiError::bad_request)?;
        let idempotency_key = control_request.idempotency_key.clone();
        if let Some(context) = request.plan_context.clone() {
            self.pending_plan_contexts
                .write()
                .await
                .insert(idempotency_key.clone(), context);
        }
        let accepted = match control.run(control_request).await {
            Ok(accepted) => accepted,
            Err(error) => {
                self.pending_plan_contexts
                    .write()
                    .await
                    .remove(&idempotency_key);
                return Err(control_error(error));
            }
        };
        let record = control
            .record(&accepted.execution_id)
            .await
            .map_err(ApiError::internal)?;
        let mut metadata = metadata_from_record(&record, request.plan_context.clone());
        metadata.request = request;
        write_metadata(&self.runs_dir.join(&accepted.execution_id), &metadata)
            .map_err(ApiError::internal)?;
        self.set_current_job(metadata).await;
        self.invalidate_summaries().await;
        self.emit_change("started", &accepted.execution_id).await;
        Ok(accepted.execution_id)
    }

    async fn validate_plan_context(&self, context: &plans::PlanContext) -> Result<(), ApiError> {
        let plan = self
            .get_plan(&context.plan_id)
            .await
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
        if context.plan_hash != plan.scope_hash {
            return Err(ApiError::conflict(
                "plan context does not match the frozen scope",
            ));
        }
        match context.role {
            PlanRunRole::Baseline
                if plan.state != PlanState::Draft || plan.baseline_execution_id.is_some() =>
            {
                Err(ApiError::conflict(
                    "the plan already has a baseline or baseline attempt",
                ))
            }
            PlanRunRole::Candidate
                if plan.baseline_execution_id.is_none() || plan.state == PlanState::Draft =>
            {
                Err(ApiError::conflict(
                    "a completed baseline is required before a candidate",
                ))
            }
            _ => Ok(()),
        }
    }

    pub(super) async fn cancel(&self) -> Result<(), ApiError> {
        let control = self
            .control
            .as_ref()
            .ok_or_else(|| ApiError::conflict("the E2E control plane is not available"))?;
        let current = self.state.lock().await.job.clone();
        let execution_id = if let Some(job) = current.filter(|job| job.status.active()) {
            job.id
        } else {
            control
                .records()
                .await
                .into_iter()
                .find(|record| !terminal(record.phase))
                .map(|record| record.execution_id)
                .ok_or_else(|| ApiError::conflict("no E2E execution is running"))?
        };
        let response = control
            .cancel(&execution_id)
            .await
            .map_err(ApiError::internal)?;
        if !response.accepted {
            return Err(ApiError::conflict("no E2E execution is running"));
        }
        let record = control
            .record(&execution_id)
            .await
            .map_err(ApiError::internal)?;
        self.sync_control_record(record)
            .await
            .map_err(ApiError::internal)?;
        Ok(())
    }

    fn require_current_url(&self, url: &str) -> Result<(), ApiError> {
        if url.trim() != self.defaults.url {
            return Err(ApiError::conflict(format!(
                "execution URL must match the worker stack {}",
                self.defaults.url
            )));
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
                        for record in control.records().await {
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
        let run_dir = self.runs_dir.join(&record.execution_id);
        let previous = read_metadata(&run_dir)?;
        let previous_terminal = previous
            .as_ref()
            .is_some_and(|metadata| !metadata.status.active());
        let plan_context = self
            .pending_plan_contexts
            .read()
            .await
            .get(&record.request.idempotency_key)
            .cloned()
            .or_else(|| {
                previous.as_ref().and_then(|metadata| {
                    metadata
                        .plan_context
                        .clone()
                        .or_else(|| metadata.request.plan_context.clone())
                })
            });
        let mut metadata = metadata_from_record(&record, plan_context.clone());
        metadata.request.url = self.defaults.url.clone();
        write_metadata(&run_dir, &metadata)?;
        self.set_current_job(metadata.clone()).await;
        if !previous_terminal && terminal(record.phase) {
            if let Some(context) = plan_context {
                self.record_plan_attempt(&context, &record.execution_id, metadata.status)
                    .await?;
            }
            self.pending_plan_contexts
                .write()
                .await
                .remove(&record.request.idempotency_key);
        }
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

    async fn record_plan_attempt(
        &self,
        context: &plans::PlanContext,
        execution_id: &str,
        status: JobStatus,
    ) -> Result<()> {
        let _plan_guard = self.plan_lock.lock().await;
        let Some(mut plan) = plans::read_plan(&self.plans_dir, &context.plan_id)? else {
            return Ok(());
        };
        if context.plan_hash != plan.scope_hash {
            bail!(
                "plan context hash does not match the frozen scope for '{}'",
                context.plan_id
            );
        }
        plan.updated_at = plans::now();
        plan.last_attempt_id = Some(execution_id.into());
        let report = read_report(&self.runs_dir.join(execution_id))
            .ok()
            .flatten();
        let complete = status == JobStatus::Completed
            && report.as_ref().is_some_and(|report| {
                !report_has_infrastructure_failure(report) && report_matches_plan(report, &plan)
            });
        if complete {
            match context.role {
                PlanRunRole::Baseline => {
                    plan.baseline_execution_id = Some(execution_id.into());
                    plan.state = PlanState::BaselineReady;
                }
                PlanRunRole::Candidate => {
                    if !plan
                        .candidate_execution_ids
                        .iter()
                        .any(|id| id == execution_id)
                    {
                        plan.candidate_execution_ids.push(execution_id.into());
                    }
                    plan.state = PlanState::ComparisonReady;
                }
            }
        } else {
            if !plan
                .incomplete_execution_ids
                .iter()
                .any(|id| id == execution_id)
            {
                plan.incomplete_execution_ids.push(execution_id.into());
            }
            if context.role == PlanRunRole::Baseline {
                plan.state = PlanState::Draft;
            } else if plan.baseline_execution_id.is_some() {
                plan.state = PlanState::BaselineReady;
            }
        }
        plans::write_plan(&self.plans_dir, &plan)
    }
}

pub(super) fn control_request(
    request: &RunRequest,
) -> std::result::Result<ControlRunRequest, String> {
    let scenarios = request
        .scenarios
        .iter()
        .map(|value| {
            ScenarioId::ALL
                .iter()
                .copied()
                .find(|scenario| scenario.as_str() == value)
                .ok_or_else(|| format!("unknown scenario '{value}'"))
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
        idempotency_key: format!("dashboard:{}", uuid::Uuid::new_v4().simple()),
        label: request.label.clone(),
        lane: "local".into(),
        model: request.model.clone(),
        provider: request.provider.clone(),
        judge_model,
        judge_provider,
        scenarios,
        runs: request.runs,
        seed: request.seed,
        rotating_seeds: Vec::new(),
        technical_retries: request.technical_retries,
        progress_interval_seconds: 15,
        run_contract: None,
    })
}

fn metadata_from_record(
    record: &ExecutionRecord,
    plan_context: Option<plans::PlanContext>,
) -> RunMetadata {
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
        completed_at: if terminal(record.phase) {
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
            plan_context: plan_context.clone(),
        },
        plan_context,
    }
}

fn job_status(record: &ExecutionRecord) -> JobStatus {
    match record.phase {
        ExecutionPhase::Completed => JobStatus::Completed,
        ExecutionPhase::Failed | ExecutionPhase::Unsupported => JobStatus::Failed,
        ExecutionPhase::Cancelled => JobStatus::Cancelled,
        _ if record.cancel_requested => JobStatus::Cancelling,
        _ => JobStatus::Running,
    }
}

fn terminal(phase: ExecutionPhase) -> bool {
    matches!(
        phase,
        ExecutionPhase::Completed
            | ExecutionPhase::Failed
            | ExecutionPhase::Cancelled
            | ExecutionPhase::Unsupported
    )
}

fn change_kind(record: &ExecutionRecord) -> &'static str {
    if terminal(record.phase) {
        "finished"
    } else if record.cancel_requested {
        "cancelling"
    } else if record.phase == ExecutionPhase::Requested {
        "started"
    } else {
        "progress"
    }
}

fn control_error(error: anyhow::Error) -> ApiError {
    let message = format!("{error:#}");
    if message.contains("concurrency limit") || message.contains("idempotency key") {
        ApiError::conflict(message)
    } else {
        ApiError::bad_request(message)
    }
}

fn report_has_infrastructure_failure(report: &E2eReport) -> bool {
    report
        .scenarios
        .iter()
        .flat_map(|scenario| scenario.runs.iter())
        .any(|run| run.status == RunStatus::InfrastructureError)
}

fn report_matches_plan(report: &E2eReport, plan: &LocalPlan) -> bool {
    if report.scenarios.len() != plan.scenarios.len() {
        return false;
    }
    let expected = plan
        .scenarios
        .iter()
        .map(|item| (item.scenario_id.as_str(), item.case_id.as_str()))
        .collect::<BTreeSet<_>>();
    let observed = report
        .scenarios
        .iter()
        .map(|scenario| (scenario.scenario_id.as_str(), scenario.case_id.as_str()))
        .collect::<BTreeSet<_>>();
    expected == observed
        && plan.scenarios.iter().all(|item| {
            report.scenarios.iter().any(|scenario| {
                scenario.scenario_id == item.scenario_id
                    && scenario.scenario_version == item.scenario_version
                    && scenario.case_id == item.case_id
                    && scenario.runs.len() == plan.runs as usize
                    && scenario.case.as_ref().is_some_and(|case| {
                        case.seed == item.seed
                            && case.inputs_sha256 == item.inputs_sha256
                            && artifact::sha256_value(&json!({
                                "scenario_id": scenario.scenario_id,
                                "scenario_version": scenario.scenario_version,
                                "case": case,
                                "execution_policy": scenario.execution_policy,
                            }))
                            .ok()
                            .is_some_and(|hash| hash == item.contract_sha256)
                    })
            })
        })
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
    if request.scenarios.is_empty() || request.scenarios.len() > ScenarioId::ALL.len() {
        return Err("select at least one valid scenario".into());
    }
    let valid: BTreeMap<_, _> = ScenarioId::ALL
        .iter()
        .map(|value| (value.as_str(), *value))
        .collect();
    request.scenarios.sort();
    request.scenarios.dedup();
    let selected = request
        .scenarios
        .iter()
        .map(|value| {
            valid
                .get(value.as_str())
                .copied()
                .ok_or_else(|| "request contains an unknown scenario".to_string())
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if request.technical_retries > 0
        && selected
            .iter()
            .any(|scenario| scenario.execution_kind() == ScenarioExecutionKind::CompositeFlow)
    {
        return Err(
            "composite scenarios with non-repeatable steps require technical_retries=0".into(),
        );
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
