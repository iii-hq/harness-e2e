use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, RwLock};
use url::Url;

use super::bus::DashboardEvents;
use super::plans::{self, LocalPlan, PlanCreateRequest, PlanRunRole, PlanState, PlanUpdateRequest};
use super::read_model::DashboardReadModel;
use super::store::{read_report, recover_interrupted_runs, write_metadata};
use super::{
    ApiError, DashboardArgs, Defaults, JobStatus, JobView, RunMetadata, RunRequest, RunSnapshot,
};
use crate::artifact;
use crate::report::{E2eReport, RunStatus};
use crate::scenarios::ScenarioId;

const MAX_LOG_TAIL_BYTES: u64 = 256 * 1024;
const MAX_LOG_CHUNK_BYTES: u64 = 64 * 1024;

struct ControllerState {
    job: Option<RunMetadata>,
    child: Option<Child>,
}

pub(super) struct Controller {
    runs_dir: PathBuf,
    plans_dir: PathBuf,
    executable: PathBuf,
    defaults: Defaults,
    state: Mutex<ControllerState>,
    plan_lock: Mutex<()>,
    read_model: RwLock<Option<Arc<DashboardReadModel>>>,
    events: Option<Arc<DashboardEvents>>,
}

impl Controller {
    pub(super) fn new(
        args: DashboardArgs,
        events: Option<Arc<DashboardEvents>>,
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
        Ok(Arc::new(Self {
            runs_dir: args.runs_dir,
            plans_dir,
            executable: env::current_exe().context("resolve harness-e2e executable")?,
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
            state: Mutex::new(ControllerState {
                job: None,
                child: None,
            }),
            plan_lock: Mutex::new(()),
            read_model: RwLock::new(None),
            events,
        }))
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
        if let Some(context) = request.plan_context.as_ref() {
            self.validate_plan_context(context).await?;
        }
        let mut state = self.state.lock().await;
        if state.job.as_ref().is_some_and(|job| job.status.active()) {
            return Err(ApiError::conflict("an E2E execution is already running"));
        }

        let now = Utc::now();
        let id = format!(
            "local-{}-{}",
            now.format("%Y%m%dT%H%M%S"),
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let run_dir = self.runs_dir.join(&id);
        let output_dir = run_dir.join("results");
        fs::create_dir_all(&output_dir).map_err(ApiError::internal)?;
        let log_path = run_dir.join("run.log");
        let stdout = File::create(&log_path).map_err(ApiError::internal)?;
        let stderr = stdout.try_clone().map_err(ApiError::internal)?;

        let mut command = build_run_command(&self.executable, &request, &output_dir);
        command.kill_on_drop(true);
        let child = command
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(ApiError::internal)?;
        let metadata = RunMetadata {
            id: id.clone(),
            label: request.label.clone(),
            status: JobStatus::Running,
            started_at: now.to_rfc3339_opts(SecondsFormat::Millis, true),
            completed_at: String::new(),
            returncode: None,
            error: String::new(),
            plan_context: request.plan_context.clone(),
            request,
        };
        write_metadata(&run_dir, &metadata).map_err(ApiError::internal)?;
        state.job = Some(metadata);
        state.child = Some(child);
        drop(state);

        self.invalidate_summaries().await;
        self.emit_change("started", &id).await;

        let execution_id = id.clone();
        let controller = Arc::clone(self);
        tokio::spawn(async move { controller.monitor(id).await });
        Ok(execution_id)
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
        let mut state = self.state.lock().await;
        let Some(job) = state.job.as_ref() else {
            return Err(ApiError::conflict("no E2E execution is running"));
        };
        if job.status != JobStatus::Running {
            return Err(ApiError::conflict("no E2E execution is running"));
        }
        let id = job.id.clone();
        if let Some(child) = state.child.as_mut() {
            child.start_kill().map_err(ApiError::internal)?;
        }
        let job = state.job.as_mut().expect("job checked above");
        job.status = JobStatus::Cancelling;
        write_metadata(&self.runs_dir.join(&id), job).map_err(ApiError::internal)?;
        drop(state);
        self.invalidate_summaries().await;
        self.emit_change("cancelling", &id).await;
        Ok(())
    }

    async fn monitor(self: Arc<Self>, id: String) {
        let mut last_progress = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let finished = {
                let mut state = self.state.lock().await;
                match state
                    .child
                    .as_mut()
                    .and_then(|child| child.try_wait().transpose())
                {
                    Some(Ok(status)) => {
                        state.child = None;
                        Some(Ok(status))
                    }
                    Some(Err(error)) => {
                        state.child = None;
                        Some(Err(error))
                    }
                    None => None,
                }
            };
            let Some(result) = finished else {
                if last_progress.elapsed() >= Duration::from_secs(1) {
                    self.emit_change("progress", &id).await;
                    last_progress = Instant::now();
                }
                continue;
            };
            let mut state = self.state.lock().await;
            let Some(job) = state.job.as_mut().filter(|job| job.id == id) else {
                return;
            };
            let cancelling = job.status == JobStatus::Cancelling;
            job.completed_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            match result {
                Ok(status) => {
                    job.returncode = status.code();
                    if cancelling {
                        job.status = JobStatus::Cancelled;
                    } else if read_report(&self.runs_dir.join(&id))
                        .ok()
                        .flatten()
                        .is_some()
                    {
                        job.status = JobStatus::Completed;
                    } else {
                        job.status = JobStatus::Failed;
                        job.error =
                            "E2E runner did not produce a result artifact; inspect the log".into();
                    }
                }
                Err(error) => {
                    job.status = JobStatus::Failed;
                    job.error = format!("cannot read E2E runner status: {error}");
                }
            }
            if let Err(error) = write_metadata(&self.runs_dir.join(&id), job) {
                tracing::error!(%error, %id, "write local E2E metadata");
            }
            let plan_context = job.plan_context.clone();
            let status = job.status;
            drop(state);
            if let Some(context) = plan_context {
                if let Err(error) = self.record_plan_attempt(&context, &id, status).await {
                    tracing::error!(%error, %id, "update local plan after execution");
                }
            }
            self.invalidate_summaries().await;
            self.emit_change("finished", &id).await;
            return;
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
        // A missing or malformed report is an incomplete attempt, not a stuck
        // plan. The execution metadata remains available for diagnosis.
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

pub(super) fn build_run_command(
    executable: &Path,
    request: &RunRequest,
    output_dir: &Path,
) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("run")
        .arg("--url")
        .arg(&request.url)
        .arg("--model")
        .arg(&request.model)
        .arg("--provider")
        .arg(&request.provider)
        .arg("--output")
        .arg(output_dir)
        .arg("--runs")
        .arg(request.runs.to_string())
        .arg("--technical-retries")
        .arg(request.technical_retries.to_string());
    if let Some(seed) = request.seed {
        command.arg("--seed").arg(seed.to_string());
    }
    if !request.judge_model.is_empty() {
        command.arg("--judge-model").arg(&request.judge_model);
    }
    if !request.judge_provider.is_empty() {
        command.arg("--judge-provider").arg(&request.judge_provider);
    }
    for scenario in &request.scenarios {
        command.arg("--scenario").arg(scenario);
    }
    command
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
        .map(|value| (value.as_str(), ()))
        .collect();
    request.scenarios.sort();
    request.scenarios.dedup();
    if request
        .scenarios
        .iter()
        .any(|value| !valid.contains_key(value.as_str()))
    {
        return Err("request contains an unknown scenario".into());
    }
    if request.scenarios.iter().any(|value| {
        ScenarioId::ALL
            .iter()
            .any(|scenario| scenario.as_str() == value && scenario.manual_cli_only())
    }) {
        return Err(
            "manually prepared composite scenarios must be started with the local CLI".into(),
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
