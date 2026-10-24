use std::collections::BTreeSet;
use std::env;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex, RwLock, Semaphore};
use url::Url;

use super::bus::DashboardEvents;
use super::overview::ReleaseOverview;
use super::read_model::DashboardReadModel;
use super::{Defaults, JobStatus, JobView, RunMetadata, RunRequest, RunSnapshot};
use crate::control::{
    ControlPlane, ExecutionPhase, ExecutionRecord, ScenariosListRequest, ScenariosListResponse,
};
use crate::plans::stacks::{StackCreateRequest, StackUpdateRequest, StackView};
use crate::plans::store::{
    import_id, left_no_results, native_runs, ExecutionParameters, GithubRunContractsRequest,
    GithubRunImportRequest, GithubRunsListRequest, SuiteView,
};
use crate::plans::{SuiteCreateRequest, SuiteUpdateRequest};

const MAX_LOG_TAIL_BYTES: u64 = 256 * 1024;
const MAX_LOG_CHUNK_BYTES: u64 = 64 * 1024;
/// GitHub imports downloading and installing at once. Each pulls a run's
/// evidence bundle (hundreds of MB); the others wait their turn, already
/// `importing`.
const GITHUB_IMPORTS: usize = 2;

struct ControllerState {
    job: Option<RunMetadata>,
}

/// The cached read model, and how often the slot changed: what was derived
/// from a model tells it from the one that replaced it, even when both left
/// out the same previous attempts.
#[derive(Default)]
struct ModelSlot {
    generation: u64,
    /// The previous-attempts revision the model left out, and the model.
    model: Option<(u64, Arc<DashboardReadModel>)>,
}

impl ModelSlot {
    fn replace(&mut self, model: Option<(u64, Arc<DashboardReadModel>)>) {
        self.model = model;
        self.generation += 1;
    }

    /// Caches a model built while the slot was at `generation`. Refused when
    /// the slot changed since: the model may lack that change.
    fn install(&mut self, generation: u64, model: (u64, Arc<DashboardReadModel>)) -> bool {
        if self.generation != generation {
            return false;
        }
        self.replace(Some(model));
        true
    }

    /// Sets a summary's availability when it differs; otherwise nothing
    /// changes, and what was derived from the model stays current.
    fn mark_availability(&mut self, id: &str, availability: &Value) {
        let Some((_, model)) = self.model.as_mut() else {
            return;
        };
        let Some(position) = model
            .summaries
            .iter()
            .position(|summary| summary["id"] == id && summary["availability"] != *availability)
        else {
            return;
        };
        Arc::make_mut(model).summaries[position]["availability"] = availability.clone();
        self.generation += 1;
    }
}

/// What `execution_summaries` and the overview were computed from: the read
/// model's generation and the saved executions' revision.
type Key = (u64, u64);

pub(super) struct Controller {
    pub(super) plan_store: Arc<crate::plans::store::PlanStore>,
    pub(super) github_repository: String,
    runs_dir: PathBuf,
    defaults: Defaults,
    control: Option<ControlPlane>,
    state: Mutex<ControllerState>,
    read_model: RwLock<ModelSlot>,
    /// The last merged `execution_summaries`. Held while one is merged, so
    /// callers that arrive meanwhile wait for it instead of merging again.
    summaries: Mutex<Option<(Key, Arc<Vec<Value>>)>>,
    /// The last `release_overview`, kept as `summaries` is.
    overview: Mutex<Option<(Key, Arc<ReleaseOverview>)>>,
    events: Option<Arc<DashboardEvents>>,
    /// Turns for the GitHub imports in the background (`GITHUB_IMPORTS`).
    github_imports: Semaphore,
    pub(super) ci_sync: super::ci_sync::CiSync,
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
            ci_sync: super::ci_sync::CiSync::load(&runs_dir),
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
            read_model: RwLock::default(),
            summaries: Mutex::new(None),
            overview: Mutex::new(None),
            events,
            github_imports: Semaphore::new(GITHUB_IMPORTS),
        });
        if let Some(control) = controller.control.as_ref() {
            for record in control.records().await? {
                controller.sync_control_record(record).await?;
            }
            controller.observe_control_plane();
        }
        controller.observe_plan_executions();
        if controller.control.is_some() {
            super::ci_sync::spawn(&controller);
        }
        Ok(controller)
    }

    /// A plan execution changes in the background. Each change refreshes the
    /// summaries and tells the Console.
    fn observe_plan_executions(self: &Arc<Self>) {
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
        self.mark_availability(id, &detail["availability"]).await;
        Ok(detail)
    }

    /// Records in the cached model what `execution_detail` found out about a
    /// run's evidence.
    async fn mark_availability(&self, id: &str, availability: &Value) {
        self.read_model
            .write()
            .await
            .mark_availability(id, availability);
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

    /// Every execution listed, the native runs and the saved plan executions
    /// together. The answer is kept until the read model or a saved
    /// execution changes.
    pub(super) async fn execution_summaries(&self) -> Result<Arc<Vec<Value>>> {
        let (key, model) = self.keyed_read_model().await?;
        self.summaries_of(key, &model).await
    }

    /// The read model with the key its derived answers are kept under.
    async fn keyed_read_model(&self) -> Result<(Key, Arc<DashboardReadModel>)> {
        // Read before the executions, so a write between the two merges
        // them again next time.
        let revision = self.plan_store.executions_revision();
        let (generation, model) = self.read_model_at().await?;
        Ok(((generation, revision), model))
    }

    async fn summaries_of(&self, key: Key, model: &DashboardReadModel) -> Result<Arc<Vec<Value>>> {
        let mut kept = self.summaries.lock().await;
        if let Some((at, summaries)) = kept.as_ref() {
            if *at == key {
                return Ok(summaries.clone());
            }
        }
        let summaries = self.merge_summaries(model).await?;
        #[cfg(test)]
        if self.reads_live_runs_again(&summaries) {
            return Ok(summaries);
        }
        *kept = Some((key, summaries.clone()));
        Ok(summaries)
    }

    /// Without a control plane, a test hears of a live run only by reading
    /// its files again at every call: nothing derived from one is kept.
    #[cfg(test)]
    fn reads_live_runs_again(&self, summaries: &[Value]) -> bool {
        self.control.is_none()
            && summaries
                .iter()
                .any(|summary| matches!(summary["status"].as_str(), Some("running" | "cancelling")))
    }

    async fn merge_summaries(&self, model: &DashboardReadModel) -> Result<Arc<Vec<Value>>> {
        let mut summaries = model.summaries.clone();
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
        Ok(self.read_model_at().await?.1)
    }

    /// The read model, and the generation of the slot it came from. A model
    /// built while the slot changed is not kept: it may lack that change.
    async fn read_model_at(&self) -> Result<(u64, Arc<DashboardReadModel>)> {
        let mut retried = false;
        loop {
            // Read before the previous attempts, so a change between the two
            // builds the model again next time.
            let attempts = self.plan_store.attempts_revision();
            let generation = {
                let slot = self.read_model.read().await;
                match &slot.model {
                    Some((revision, model)) if *revision == attempts => {
                        return Ok((slot.generation, model.clone()));
                    }
                    _ => slot.generation,
                }
            };
            let model = Arc::new(self.build_read_model().await?);
            let mut slot = self.read_model.write().await;
            if slot.install(generation, (attempts, model.clone())) {
                return Ok((slot.generation, model));
            }
            if retried {
                // Answered as built, under a generation the slot has left, so
                // nothing derived from it is reused.
                return Ok((generation, model));
            }
            retried = true;
        }
    }

    /// A read model of every record, previous attempts left out.
    async fn build_read_model(&self) -> Result<DashboardReadModel> {
        #[cfg(test)]
        if self.control.is_none() {
            let runs_dir = self.runs_dir.clone();
            return tokio::task::spawn_blocking(move || DashboardReadModel::load(&runs_dir))
                .await
                .map_err(|error| anyhow::anyhow!("load dashboard test model task: {error}"))?;
        }
        let mut records = self
            .control
            .as_ref()
            .context("the E2E control plane is not available")?
            .records()
            .await?;
        for record in &mut records {
            self.mark_unavailable(record);
        }
        let discarded = self.plan_store.previous_attempts().await?;
        DashboardReadModel::from_records(records, &discarded)
    }

    /// Marks a record's projection unavailable when its native results or
    /// their manifest are gone.
    fn mark_unavailable(&self, record: &mut ExecutionRecord) {
        if let Some(projection) = &mut record.dashboard_projection {
            let available = record.result_path.as_ref().is_some_and(|path| {
                let result = super::run_sessions::results_file(self.runs_dir.join(path));
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

    /// Adds the native runs a finished import installed to the cached read
    /// model, as a new model; `true` with no change for one that left no
    /// results. `false` when no model is cached, the previous attempts moved,
    /// the import failed otherwise, or the model changed meanwhile.
    async fn grow_read_model(&self, id: &str) -> Result<bool> {
        let execution = self.plan_store.read_execution(id).await?;
        // It failed before installing a run: whatever is cached is current.
        if left_no_results(&execution) {
            return Ok(true);
        }
        // A failed import may have installed runs its slots do not name.
        if execution.state == "failed" {
            return Ok(false);
        }
        let attempts = self.plan_store.attempts_revision();
        let base = match self.read_model.read().await.model.as_ref() {
            Some((revision, model)) if *revision == attempts => model.clone(),
            _ => return Ok(false),
        };
        let natives = execution
            .slots
            .iter()
            .map(|slot| slot.execution_id.as_str())
            .filter(|native| {
                !native.is_empty()
                    && !base
                        .summaries
                        .iter()
                        .any(|summary| summary["id"] == *native)
            })
            .collect::<BTreeSet<_>>();
        let Some(grown) = self.with_native_runs(&base, natives).await? else {
            return Ok(false);
        };
        // Two imports can finish at once: only the model this one grew is
        // replaced.
        let mut slot = self.read_model.write().await;
        let unchanged = slot
            .model
            .as_ref()
            .is_some_and(|(revision, model)| *revision == attempts && Arc::ptr_eq(model, &base));
        if !unchanged || self.plan_store.attempts_revision() != attempts {
            return Ok(false);
        }
        slot.replace(Some((attempts, Arc::new(grown))));
        Ok(true)
    }

    /// `base` with these native runs added, read as `read_model()` reads them.
    async fn with_native_runs(
        &self,
        base: &DashboardReadModel,
        natives: BTreeSet<&str>,
    ) -> Result<Option<DashboardReadModel>> {
        #[cfg(test)]
        if self.control.is_none() {
            let stored = natives
                .iter()
                .filter_map(|native| {
                    super::store::read_stored_run(&self.runs_dir.join(native)).transpose()
                })
                .collect::<Result<Vec<_>>>()?;
            return base.with_stored(stored);
        }
        let control = self
            .control
            .as_ref()
            .context("the E2E control plane is not available")?;
        let mut records = Vec::new();
        for native in natives {
            let mut record = control.stored_record(native).await?;
            self.mark_unavailable(&mut record);
            records.push(record);
        }
        base.with_records(records, &self.plan_store.previous_attempts().await?)
    }

    /// A page of a test's history, with each run's sessions read from its
    /// native results.
    pub(super) async fn test_history(
        &self,
        request: super::read_model::TestHistoryRequest,
    ) -> Result<super::read_model::TestHistoryResponse> {
        let model = self.read_model().await?;
        let mut history = model.test_history(request)?;
        // Without the plan store the runs still show, only unnamed.
        let summaries = self.execution_summaries().await.unwrap_or_else(|error| {
            tracing::warn!(
                error = %format!("{error:#}"),
                "a test history shows its runs without execution names"
            );
            Arc::new(Vec::new())
        });
        let runs_dir = self.runs_dir.clone();
        tokio::task::spawn_blocking(move || {
            model.attach_run_details(&mut history, &summaries, &runs_dir);
            history
        })
        .await
        .context("read the runs of a test history")
    }

    /// The trends of a series, from the executions listed and the runs the
    /// read model holds.
    pub(super) async fn trends(
        &self,
        request: super::trends::TrendsRequest,
    ) -> Result<super::trends::TrendsResponse> {
        let (key, model) = self.keyed_read_model().await?;
        let summaries = self.summaries_of(key, &model).await?;
        Ok(super::trends::trends(
            &request,
            &summaries,
            &model.trend_runs(),
        ))
    }

    /// The release overview over every execution listed: the Trends first
    /// screen. Kept until the read model or a saved execution changes.
    pub(super) async fn release_overview(&self) -> Result<Arc<ReleaseOverview>> {
        let (key, model) = self.keyed_read_model().await?;
        let mut kept = self.overview.lock().await;
        if let Some((at, overview)) = kept.as_ref() {
            if *at == key {
                return Ok(overview.clone());
            }
        }
        let summaries = self.summaries_of(key, &model).await?;
        let overview = Arc::new(super::overview::overview(&summaries, &model.trend_runs()));
        #[cfg(test)]
        if self.reads_live_runs_again(&summaries) {
            return Ok(overview);
        }
        *kept = Some((key, overview.clone()));
        Ok(overview)
    }

    async fn invalidate_summaries(&self) {
        self.read_model.write().await.replace(None);
    }

    pub(super) async fn suites(&self) -> Result<Vec<SuiteView>> {
        self.plan_store.suites().await
    }

    pub(super) async fn create_suite(&self, request: SuiteCreateRequest) -> Result<SuiteView> {
        if let Some(from) = &request.from {
            validate_suite_id(from)?;
        }
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
        if let Some(from) = &request.from {
            validate_stack_id(from)?;
        }
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
    /// continue in the background, `GITHUB_IMPORTS` at a time, and end in
    /// `completed`, `interrupted` (a run GitHub did not conclude `success`)
    /// or `failed`. Importing a run by hand forgets that its execution was
    /// deleted.
    pub(super) async fn github_run_import(
        self: &Arc<Self>,
        request: GithubRunImportRequest,
    ) -> Result<Value> {
        let repository = request
            .repository
            .unwrap_or_else(|| self.github_repository.clone());
        self.plan_store
            .clear_deleted_import(&import_id(&repository, request.run_id))
            .await?;
        let (execution, started) = self
            .plan_store
            .begin_github_import(&repository, request.run_id)
            .await?;
        if started {
            self.emit_change("started", &execution.id).await;
            let controller = Arc::clone(self);
            let id = execution.id.clone();
            tokio::spawn(async move {
                let _turn = controller.github_imports.acquire().await;
                controller.finish_import(&id).await;
            });
        }
        Ok(json!({"execution_id": execution.id, "state": execution.state}))
    }

    /// Import a run for CI sync: it waits its turn before it begins, so a
    /// run deleted or imported by hand meanwhile is left alone (`None`).
    /// Answers the execution once the import ended.
    pub(super) async fn sync_import(
        self: &Arc<Self>,
        repository: &str,
        run_id: u64,
    ) -> Result<Option<crate::plans::store::PlanExecution>> {
        let _turn = self.github_imports.acquire().await?;
        let id = import_id(repository, run_id);
        if self.plan_store.import_deleted(&id).await? {
            return Ok(None);
        }
        let (execution, started) = self
            .plan_store
            .begin_github_import(repository, run_id)
            .await?;
        if !started {
            return Ok(None);
        }
        let id = execution.id.clone();
        self.emit_change("started", &execution.id).await;
        self.finish_import(&id).await;
        self.plan_store.read_execution(&id).await.map(Some)
    }

    /// Download and install a begun import, grow the read model by its runs,
    /// then tell the Console.
    async fn finish_import(&self, id: &str) {
        // Imported again, its runs may replace or remove ones the model holds.
        let first = self
            .plan_store
            .read_execution(id)
            .await
            .is_ok_and(|execution| native_runs(&execution).all(str::is_empty));
        let finished = self.plan_store.finish_github_import(id).await;
        if let Err(error) = &finished {
            tracing::error!(execution_id = %id, error = %format!("{error:#}"), "record the GitHub import outcome");
        }
        let grown = first
            && finished.is_ok()
            && self.grow_read_model(id).await.unwrap_or_else(|error| {
                tracing::warn!(execution_id = %id, error = %format!("{error:#}"), "the read model is built again after an import");
                false
            });
        if !grown {
            self.invalidate_summaries().await;
        }
        self.emit_change("finished", id).await;
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

    pub(super) async fn emit_change(&self, kind: &str, execution_id: &str) {
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
        idempotency_key: Option<&str>,
    ) -> Result<Value> {
        let execution = self
            .plan_store
            .start_execution_in(
                parameters,
                label,
                Some(&self.github_repository),
                idempotency_key,
            )
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
        if !record.phase.terminal()
            || self
                .should_emit_terminal_change(&record.execution_id)
                .await?
        {
            self.emit_change(change_kind(&record), &record.execution_id)
                .await;
        }
        Ok(())
    }

    async fn should_emit_terminal_change(&self, execution_id: &str) -> Result<bool> {
        Ok(!self
            .plan_store
            .executions()
            .await?
            .iter()
            .any(|execution| native_runs(execution).any(|id| id == execution_id)))
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

#[cfg(test)]
pub(super) mod tests {
    use std::time::Duration;

    use super::*;
    use crate::plans::store::tests::{
        exact_stack_bundle, fake_gh, import_stub, manager, manager_with_gh, save_execution,
        FakeRunner,
    };
    use crate::plans::store::{ExecutionSource, PlanStore};

    /// A controller without a control plane, on this plan store.
    pub(in crate::dashboard) fn without_control_plane(
        plan_store: Arc<PlanStore>,
        runs_dir: &Path,
    ) -> Arc<Controller> {
        Arc::new(Controller {
            plan_store,
            github_repository: "o/r".into(),
            runs_dir: runs_dir.into(),
            defaults: Defaults {
                url: "ws://localhost:49134".into(),
                model: String::new(),
                provider: String::new(),
                runs: 1,
                technical_retries: 1,
                seed: None,
            },
            control: None,
            state: Mutex::new(ControllerState { job: None }),
            read_model: RwLock::default(),
            summaries: Mutex::new(None),
            overview: Mutex::new(None),
            events: None,
            github_imports: Semaphore::new(GITHUB_IMPORTS),
            ci_sync: crate::dashboard::ci_sync::CiSync::load(runs_dir),
        })
    }

    #[test]
    fn a_slot_refuses_a_model_built_before_it_changed() {
        let model =
            || Arc::new(DashboardReadModel::from_records(Vec::new(), &BTreeSet::new()).unwrap());
        let mut slot = ModelSlot::default();
        let built_at = slot.generation;
        // An import invalidated the slot while the model was built.
        slot.replace(None);
        assert!(!slot.install(built_at, (0, model())));
        assert!(slot.model.is_none());

        let built_at = slot.generation;
        assert!(slot.install(built_at, (0, model())));
        assert!(slot.model.is_some());
        assert_ne!(slot.generation, built_at);
    }

    #[tokio::test]
    async fn the_release_overview_of_an_empty_store_has_no_release() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let controller =
            without_control_plane(manager(root, Arc::new(FakeRunner::new(root.into()))), root);
        let overview = controller.release_overview().await.unwrap();
        assert!(overview.releases.is_empty() && overview.locals.is_empty());
        assert_eq!((&overview.verdict, overview.unversioned), (&None, 0));
    }

    #[tokio::test]
    async fn terminal_changes_emit_for_the_plan_and_standalone_run_not_its_native_child() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let store = manager(root.path(), runner);
        let controller = without_control_plane(store.clone(), root.path());
        controller.read_model().await.unwrap();
        assert!(controller.read_model.read().await.model.is_some());
        controller.observe_plan_executions();
        let plan = store
            .start_execution(
                ExecutionParameters {
                    suite: None,
                    scenarios: vec!["minimal_path".into()],
                    runs: 1,
                    technical_retries: 0,
                    model: "model".into(),
                    provider: "provider".into(),
                    agent: None,
                    r#where: crate::plans::store::Where::Harness,
                    stack: None,
                },
                "",
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while controller.read_model.read().await.model.is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            store.read_execution(&plan.id).await.unwrap().state,
            "completed"
        );
        let child = plan.slots[0].execution_id.as_str();

        assert!(!controller.should_emit_terminal_change(child).await.unwrap());
        assert!(controller
            .should_emit_terminal_change(&plan.id)
            .await
            .unwrap());
        assert!(controller
            .should_emit_terminal_change("standalone-native-run")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn github_imports_answer_at_once_and_download_two_at_a_time() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        // Every run answers; listing a run's artifacts holds until `release`
        // exists, then lists none, so each import ends failed.
        let mut gh = fake_gh(
            root.path(),
            r#"here="$(dirname "$0")"
case "$*" in
  *artifacts*) echo "$*" >> "$here/downloads"
    while [ ! -f "$here/release" ]; do sleep 0.02; done ;;
  *) printf '%s' '{"run_attempt":1,"display_title":"E2E · rc","html_url":"https://github.com/o/r/actions/runs/1","created_at":"2026-09-20T10:00:00Z"}' ;;
esac"#,
        );
        gh.api_timeout = Duration::from_secs(30);
        let controller = without_control_plane(
            manager_with_gh(&data, Arc::new(FakeRunner::new(data.clone())), gh),
            &data,
        );
        let downloads = || {
            fs::read_to_string(root.path().join("downloads"))
                .unwrap_or_default()
                .lines()
                .count()
        };

        let mut ids = Vec::new();
        for run_id in [1, 2, 3] {
            let accepted = controller
                .github_run_import(GithubRunImportRequest {
                    repository: None,
                    run_id,
                })
                .await
                .unwrap();
            assert_eq!(accepted["state"], "importing");
            ids.push(accepted["execution_id"].as_str().unwrap().to_owned());
        }

        // Two download; the third waits its turn, still importing.
        tokio::time::timeout(Duration::from_secs(10), async {
            while downloads() < GITHUB_IMPORTS {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("two imports should start downloading");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(downloads(), GITHUB_IMPORTS);
        let waiting = controller.plan_store.read_execution(&ids[2]).await.unwrap();
        assert_eq!(waiting.state, "importing");

        // Once one finishes, the third takes its turn.
        fs::write(root.path().join("release"), "").unwrap();
        for id in &ids {
            let ended = tokio::time::timeout(Duration::from_secs(20), async {
                loop {
                    let execution = controller.plan_store.read_execution(id).await.unwrap();
                    if execution.state != "importing" {
                        return execution;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("every import should end");
            assert_eq!(ended.state, "failed");
        }
        assert_eq!(downloads(), 3);
    }

    #[tokio::test]
    async fn a_manual_import_forgets_a_deleted_run() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        // Every run answers and lists no artifact, so each import ends failed.
        let gh = fake_gh(
            root.path(),
            r#"case "$*" in
  *artifacts*) ;;
  *) printf '%s' '{"run_attempt":1,"display_title":"E2E · rc","html_url":"https://github.com/o/r/actions/runs/1","created_at":"2026-09-20T10:00:00Z"}' ;;
esac"#,
        );
        let controller = without_control_plane(
            manager_with_gh(&data, Arc::new(FakeRunner::new(data.clone())), gh),
            &data,
        );
        let import = || {
            controller.github_run_import(GithubRunImportRequest {
                repository: None,
                run_id: 1,
            })
        };

        let accepted = import().await.unwrap();
        let id = accepted["execution_id"].as_str().unwrap().to_owned();
        tokio::time::timeout(Duration::from_secs(10), async {
            while controller
                .plan_store
                .read_execution(&id)
                .await
                .unwrap()
                .state
                == "importing"
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the import should end");
        controller.delete_execution(&id).await.unwrap();
        assert!(controller.plan_store.import_deleted(&id).await.unwrap());

        let again = import().await.unwrap();
        assert_eq!(again["state"], "importing");
        assert!(!controller.plan_store.import_deleted(&id).await.unwrap());
    }

    /// Puts the native run of a bundle named `name` in the data directory.
    fn place_native_run(root: &Path, data: &Path, name: &str) -> String {
        let (bundle, _, id) =
            exact_stack_bundle(&root.join(name), &format!("rc:e2e:{name}"), "1.8.8");
        fs::rename(
            bundle
                .join("smoke-r01/groups/case-context/native")
                .join(&id),
            data.join(&id),
        )
        .unwrap();
        id
    }

    #[tokio::test]
    async fn an_import_grows_the_read_model_instead_of_rebuilding_it() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let (bundle, contract, native) =
            exact_stack_bundle(&root.path().join("fixture"), "rc:e2e:grown", "1.8.8");
        let gh = fake_gh(
            root.path(),
            &format!(
                r#"case "$*" in
  "api repos/o/r/actions/runs/42") printf '%s' '{{"id":42,"run_attempt":1,"display_title":"E2E · rc-1","conclusion":"success","created_at":"2026-09-20T10:00:00Z","updated_at":"2026-09-20T11:00:00Z","html_url":"https://github.com/o/r/actions/runs/42"}}' ;;
  "api --paginate"*) printf 'e2e-contract-rc-1-gh-1\ne2e-observation-rc-1-gh-1\n' ;;
  "run download"*) case "$7" in
      e2e-contract-*) cp -R '{contract}' "$9" ;;
      *) cp -R '{bundle}' "$9" ;;
    esac ;;
  *) echo "unexpected: $*" >&2; exit 1 ;;
esac"#,
                contract = contract.display(),
                bundle = bundle.display(),
            ),
        );
        let controller = without_control_plane(
            manager_with_gh(&data, Arc::new(FakeRunner::new(data.clone())), gh),
            &data,
        );
        let place = |name: &str| place_native_run(root.path(), &data, name);
        // Held before the import, so the import's runs join a test version's.
        let held = place("held");
        let before = controller.read_model().await.unwrap();
        assert_eq!(before.summaries.len(), 1);
        // A run only a rebuild reads.
        let decoy = place("decoy");

        let imported = controller.sync_import("o/r", 42).await.unwrap().unwrap();
        assert_eq!(imported.state, "completed");
        let model = controller.read_model().await.unwrap();
        assert!(!Arc::ptr_eq(&model, &before));
        let mut ids = model
            .summaries
            .iter()
            .map(|summary| summary["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        ids.sort();
        let mut expected = [held.as_str(), native.as_str()];
        expected.sort();
        assert_eq!(ids, expected);

        fs::remove_dir_all(data.join(&decoy)).unwrap();
        let built = DashboardReadModel::load(&data).unwrap();
        assert_eq!(model.state(), built.state());
        assert_eq!(model.trend_runs(), built.trend_runs());
    }

    #[tokio::test]
    async fn a_second_overview_without_a_change_reuses_the_first() {
        let root = tempfile::tempdir().unwrap();
        let controller = importing_run_1(root.path(), "");
        let summaries = controller.execution_summaries().await.unwrap();
        let overview = controller.release_overview().await.unwrap();
        // Nothing was written: the same answers, not new ones.
        assert!(Arc::ptr_eq(
            &summaries,
            &controller.execution_summaries().await.unwrap()
        ));
        assert!(Arc::ptr_eq(
            &overview,
            &controller.release_overview().await.unwrap()
        ));
        controller
            .trends(super::super::trends::TrendsRequest::default())
            .await
            .unwrap();
        assert!(Arc::ptr_eq(
            &summaries,
            &controller.execution_summaries().await.unwrap()
        ));

        // A saved execution changes what is listed and what is counted.
        controller
            .plan_store
            .begin_github_import("o/r", 1)
            .await
            .unwrap();
        let changed = controller.execution_summaries().await.unwrap();
        let recounted = controller.release_overview().await.unwrap();
        assert_eq!(changed.len(), 1);
        assert!(!Arc::ptr_eq(&overview, &recounted));
        // Both equal what a controller that never saw the old answers builds.
        let fresh = without_control_plane(controller.plan_store.clone(), &root.path().join("data"));
        assert_eq!(*changed, *fresh.execution_summaries().await.unwrap());
        assert_eq!(*recounted, *fresh.release_overview().await.unwrap());
    }

    #[tokio::test]
    async fn a_changed_availability_reaches_the_summaries_and_an_unchanged_one_keeps_them() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let controller = without_control_plane(
            manager(&data, Arc::new(FakeRunner::new(data.clone()))),
            &data,
        );
        let id = place_native_run(root.path(), &data, "held");
        let listed = controller.execution_summaries().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_ne!(listed[0]["availability"], "unavailable");

        controller
            .mark_availability(&id, &json!("unavailable"))
            .await;
        let marked = controller.execution_summaries().await.unwrap();
        assert_eq!(marked[0]["availability"], "unavailable");

        controller
            .mark_availability(&id, &json!("unavailable"))
            .await;
        assert!(Arc::ptr_eq(
            &marked,
            &controller.execution_summaries().await.unwrap()
        ));
    }

    /// A controller whose `gh` answers run 1 and lists its artifacts as
    /// `artifacts` says.
    fn importing_run_1(root: &Path, artifacts: &str) -> Arc<Controller> {
        let data = root.join("data");
        let gh = fake_gh(
            root,
            &format!(
                r#"case "$*" in
  *artifacts*) {artifacts} ;;
  *) printf '%s' '{{"run_attempt":1,"display_title":"E2E · rc","html_url":"https://github.com/o/r/actions/runs/1","created_at":"2026-09-20T10:00:00Z","updated_at":"2026-09-20T11:00:00Z"}}' ;;
esac"#
            ),
        );
        without_control_plane(
            manager_with_gh(&data, Arc::new(FakeRunner::new(data.clone())), gh),
            &data,
        )
    }

    #[tokio::test]
    async fn a_sync_import_takes_its_turn_before_it_begins() {
        let root = tempfile::tempdir().unwrap();
        let controller = importing_run_1(root.path(), "");

        let turns = controller
            .github_imports
            .acquire_many(GITHUB_IMPORTS as u32)
            .await
            .unwrap();
        let importing = tokio::spawn({
            let controller = controller.clone();
            async move { controller.sync_import("o/r", 1).await }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(controller.plan_store.executions().await.unwrap().is_empty());

        drop(turns);
        let imported = tokio::time::timeout(Duration::from_secs(10), importing)
            .await
            .expect("the import should end")
            .unwrap()
            .unwrap()
            .expect("the run should be imported");
        assert_eq!(
            (imported.id, imported.state.as_str()),
            (import_id("o/r", 1), "failed")
        );
    }

    #[tokio::test]
    async fn a_sync_reimport_finishes_the_stable_execution_id() {
        let root = tempfile::tempdir().unwrap();
        let controller = importing_run_1(root.path(), "");
        let mut stable = import_stub();
        stable.id = "plan-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
        stable.idempotency_key = "eval-work-stable".into();
        stable.state = "failed".into();
        stable.source = ExecutionSource::Github {
            repository: "o/r".into(),
            run_id: 1,
            run_attempt: 1,
            url: "https://github.com/o/r/actions/runs/1".into(),
            release_control_execution_id: None,
            stack: None,
            run_updated_at: Some("2026-10-20T10:00:00Z".into()),
            sync_attempts: 1,
            conclusion: None,
            status: None,
            follow: Default::default(),
        };
        save_execution(&controller.plan_store, &stable).await;

        let imported = controller.sync_import("o/r", 1).await.unwrap().unwrap();
        assert_eq!(imported.id, stable.id);
        assert_ne!(imported.state, "importing");
        assert!(controller
            .plan_store
            .read_execution(&import_id("o/r", 1))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn deleting_a_stable_github_execution_blocks_ci_reimport() {
        let root = tempfile::tempdir().unwrap();
        let controller = importing_run_1(root.path(), "");
        let mut stable = import_stub();
        stable.id = "plan-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into();
        stable.state = "completed".into();
        stable.source = ExecutionSource::Github {
            repository: "o/r".into(),
            run_id: 1,
            run_attempt: 1,
            url: "https://github.com/o/r/actions/runs/1".into(),
            release_control_execution_id: None,
            stack: None,
            run_updated_at: Some("2026-10-20T11:00:00Z".into()),
            sync_attempts: 0,
            conclusion: Some("success".into()),
            status: None,
            follow: Default::default(),
        };
        save_execution(&controller.plan_store, &stable).await;

        controller.delete_execution(&stable.id).await.unwrap();
        let marker = import_id("o/r", 1);
        assert!(controller.plan_store.import_deleted(&marker).await.unwrap());
        assert!(!controller
            .plan_store
            .import_deleted(&stable.id)
            .await
            .unwrap());
        assert!(controller.sync_import("o/r", 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_run_deleted_while_its_sync_import_waits_stays_deleted() {
        let root = tempfile::tempdir().unwrap();
        let controller = importing_run_1(root.path(), "echo 'connection reset' >&2; exit 1");
        let id = import_id("o/r", 1);
        controller
            .plan_store
            .begin_github_import("o/r", 1)
            .await
            .unwrap();
        controller
            .plan_store
            .finish_github_import(&id)
            .await
            .unwrap();
        let failed = controller.plan_store.read_execution(&id).await.unwrap();
        assert_eq!(failed.state, "failed");

        let turns = controller
            .github_imports
            .acquire_many(GITHUB_IMPORTS as u32)
            .await
            .unwrap();
        let importing = tokio::spawn({
            let controller = controller.clone();
            async move { controller.sync_import("o/r", 1).await }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        controller.delete_execution(&id).await.unwrap();
        drop(turns);
        let imported = tokio::time::timeout(Duration::from_secs(10), importing)
            .await
            .expect("the import should end")
            .unwrap()
            .unwrap();
        assert!(imported.is_none(), "{imported:?}");
        assert!(controller.plan_store.read_execution(&id).await.is_err());
        assert!(controller.plan_store.import_deleted(&id).await.unwrap());

        let again = controller
            .github_run_import(GithubRunImportRequest {
                repository: None,
                run_id: 1,
            })
            .await
            .unwrap();
        assert_eq!(again["state"], "importing");
        assert!(!controller.plan_store.import_deleted(&id).await.unwrap());
    }
}
