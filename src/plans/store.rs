//! Executions composed of native runs, and the suites this Console keeps.
//! Every planned child and its idempotency key is durable before admission.
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::{LocalSuite, SuiteCreateRequest, SuiteUpdateRequest};
use crate::artifact;
use crate::control::{execution_id_for_key, ControlPlane, ExecutionRecord, RunRequest};
use crate::persistence::Persistence;
use crate::report::{E2eReport, ReportState};
use crate::test_plan::{self, ProfileSnapshot};

mod github;
mod stack;

pub(crate) use github::{GithubRunContractsRequest, GithubRunImportRequest, GithubRunsListRequest};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Slot {
    pub round: u32,
    pub group_id: String,
    pub scenario_id: String,
    pub execution_id: String,
    pub request: Value,
    pub state: String,
    pub result_path: Option<String>,
    pub error: Option<String>,
    pub observed: u32,
    pub completed: u32,
    pub passed: u32,
    pub technical_valid: u32,
    pub eligible: bool,
    /// The runs this slot ran before its current one, oldest first. Only the
    /// last attempt counts; these stay visible, outside every total.
    #[serde(default)]
    pub previous_attempts: Vec<SlotAttempt>,
}

/// A run a slot ran before its current one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct SlotAttempt {
    pub execution_id: String,
    /// Why it ended as it did, when the run said.
    pub error: Option<String>,
}

/// One execution, whatever produced it: run here or imported from GitHub.
/// Where it came from is data (`source`), never a different record.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct PlanExecution {
    pub id: String,
    pub idempotency_key: String,
    /// Editable name; the Console titles it by model and date when absent.
    #[serde(default)]
    pub label: Option<String>,
    /// What running it again would need.
    #[serde(default)]
    pub parameters: Option<ExecutionParameters>,
    #[serde(default)]
    pub source: ExecutionSource,
    #[serde(default)]
    pub stack: Vec<StackWorker>,
    /// What could not be recorded about the stack, and scenarios added to
    /// complete a sequential group; shown, never blocking.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// `running`, `cancelling` or `importing` while active; `completed`,
    /// `interrupted`, `cancelled` or `failed` once done.
    pub state: String,
    pub started_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
    pub cancel_requested: bool,
    pub error: Option<String>,
    pub slots: Vec<Slot>,
    pub measurements: Option<Value>,
    #[serde(default)]
    pub system_under_test: Option<Value>,
    /// A scenario of this finished execution running again.
    #[serde(default)]
    pub rerun: Option<Rerun>,
}

/// A scenario of a finished execution running again, and the finished state
/// it came from. Each run it replaces becomes a previous attempt only when
/// its new run is admitted; if the rerun stops first, the slots keep their
/// runs and the execution returns to that state with a warning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Rerun {
    /// The scenario, or its whole sequential group.
    pub scenarios: Vec<String>,
    /// The native runs to replace.
    pub runs: Vec<String>,
    pub started_at: String,
    pub state: String,
    pub error: Option<String>,
    pub finished_at: Option<String>,
}

/// What an execution ran, to run it again. Always the canonical cases: a
/// `seed` sent or stored by an older Console is ignored, so every execution
/// pairs with any other by scenario and repetition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ExecutionParameters {
    /// The suite it ran; absent for an execution from before suites.
    #[serde(
        default,
        deserialize_with = "readable_suite",
        skip_serializing_if = "Option::is_none"
    )]
    pub suite: Option<ExecutionSuite>,
    pub scenarios: Vec<String>,
    pub runs: u32,
    pub technical_retries: u8,
    pub model: String,
    pub provider: String,
    /// Agent profile the subject ran under.
    pub agent: Option<String>,
}

/// The suite an execution ran: named when it was picked as it is (a suite of
/// the master plan or of this Console), unnamed when its scenarios were
/// ticked by hand, with the digest of the snapshot it materialized to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ExecutionSuite {
    /// Absent for an unnamed suite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default)]
    pub label: String,
    /// `profile_sha256` of the snapshot; the runner sets it, a request never does.
    #[serde(default)]
    pub sha256: String,
}

/// An import from before suites had a name and a digest stored only the
/// suite's id: it reads as that suite, named by its id, digest unknown.
fn readable_suite<'de, D: serde::Deserializer<'de>>(
    value: D,
) -> std::result::Result<Option<ExecutionSuite>, D::Error> {
    use serde::de::Error;
    match Option::<Value>::deserialize(value)? {
        Some(Value::String(id)) => Ok(Some(ExecutionSuite {
            label: id.clone(),
            id: Some(id),
            sha256: String::new(),
        })),
        value => serde_json::from_value(value.unwrap_or(Value::Null)).map_err(D::Error::custom),
    }
}

/// A suite as the Console lists it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct SuiteView {
    pub id: String,
    pub label: String,
    /// `repository` for a suite of the master plan (read-only), `local` for
    /// one this Console keeps.
    pub source: String,
    pub purpose: String,
    pub scenarios: Vec<String>,
    pub repetitions: u32,
    pub technical_retries: u8,
    /// Digest of the snapshot this runner materializes it to; what an
    /// execution of it records.
    pub sha256: Option<String>,
    pub updated_at: Option<String>,
}

/// Where an execution came from; shown and used to deduplicate imports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ExecutionSource {
    #[default]
    Local,
    Github {
        repository: String,
        run_id: u64,
        run_attempt: u32,
        url: String,
        release_control_execution_id: Option<String>,
        /// The stack its contract names (`default`, `inline`, …); absent
        /// before contracts stated their execution.
        #[serde(default)]
        stack: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkerSource {
    Package,
    Path,
}

/// One worker of the stack an execution ran on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct StackWorker {
    pub name: String,
    pub source: WorkerSource,
    pub requested: Option<String>,
    pub observed: Option<String>,
    pub commit: Option<String>,
    pub dirty: Option<bool>,
    /// Groups that ran this version, listed only when groups disagree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
}

pub(crate) fn validate_saved_execution(execution: &PlanExecution) -> Result<()> {
    ensure!(
        !execution.id.is_empty() && !execution.idempotency_key.is_empty(),
        "Unsupported execution identity"
    );
    Ok(())
}
impl PlanExecution {
    fn active(&self) -> bool {
        matches!(self.state.as_str(), "running" | "cancelling")
    }
}

#[async_trait]
trait Runner: Send + Sync {
    /// The execution or native run holding the runner, if any.
    async fn active(&self) -> Option<String>;
    async fn reserve(&self, owner: &str) -> Result<()>;
    async fn release(&self, owner: &str);
    async fn submit(&self, owner: &str, request: RunRequest) -> Result<String>;
    async fn record(&self, id: &str) -> Option<ExecutionRecord>;
    /// Whether a native run is stored, even when its evidence can no longer be read.
    async fn retained(&self, id: &str) -> bool;
    /// The identity a run starts on now: what `verify_system_identity` compares.
    async fn identity(&self) -> Result<Value>;
    async fn cancel(&self, id: &str) -> Result<()>;
    /// Retain a terminal native run produced elsewhere.
    async fn install(&self, record: ExecutionRecord) -> Result<()>;
    /// Delete a terminal native run and its evidence.
    async fn remove(&self, id: &str) -> Result<()>;
    /// The stack runs start on now, and what could not be read about it.
    async fn stack(&self) -> (Vec<StackWorker>, Vec<String>);
}
#[async_trait]
impl Runner for ControlPlane {
    async fn active(&self) -> Option<String> {
        if let Some(id) = self.active_plan().await {
            return Some(id);
        }
        self.records()
            .await
            .ok()?
            .into_iter()
            .find(|r| !r.phase.terminal())
            .map(|r| r.execution_id)
    }
    async fn reserve(&self, owner: &str) -> Result<()> {
        self.reserve_plan(owner).await
    }
    async fn release(&self, owner: &str) {
        self.release_plan(owner).await;
    }
    async fn submit(&self, owner: &str, request: RunRequest) -> Result<String> {
        Ok(self.run_plan_child(owner, request).await?.execution_id)
    }
    async fn record(&self, id: &str) -> Option<ExecutionRecord> {
        ControlPlane::record(self, id).await.ok()
    }
    async fn retained(&self, id: &str) -> bool {
        self.stored_record(id).await.is_ok()
    }
    async fn identity(&self) -> Result<Value> {
        let context = crate::context::E2eContext::from_client(self.client().clone());
        let contracts = context.preflight_control_plane().await?;
        let versions = context.runtime_versions().await?;
        Ok(serde_json::to_value(
            crate::identity::SystemUnderTestIdentity::from_environment(
                versions.engine,
                versions.harness,
                &contracts,
            )?,
        )?)
    }
    async fn cancel(&self, id: &str) -> Result<()> {
        ControlPlane::cancel(self, id).await?;
        Ok(())
    }
    async fn install(&self, record: ExecutionRecord) -> Result<()> {
        self.install_terminal(record).await
    }
    async fn remove(&self, id: &str) -> Result<()> {
        self.delete(id).await
    }
    async fn stack(&self) -> (Vec<StackWorker>, Vec<String>) {
        stack::observe(self.client()).await
    }
}

pub(crate) struct PlanStore {
    pub(crate) root: PathBuf,
    persistence: Option<Persistence>,
    runner: Option<Arc<dyn Runner>>,
    github: github::GithubCli,
    // Serializes receipt transitions against cancellation and admission.
    lock: Mutex<()>,
    /// Moves whenever a slot's previous attempts change, so views that leave
    /// them out know to read them again.
    attempts: AtomicU64,
}
impl PlanStore {
    pub(crate) async fn new(root: PathBuf, control: Option<ControlPlane>) -> Result<Arc<Self>> {
        #[cfg(test)]
        if control.is_none() {
            fs::create_dir_all(root.join("plan-store/suites"))?;
            fs::create_dir_all(root.join("plan-store/executions"))?;
        }
        let manager = Arc::new(Self {
            root,
            persistence: control.as_ref().map(ControlPlane::persistence),
            runner: control.map(|c| Arc::new(c) as Arc<dyn Runner>),
            github: github::GithubCli::default(),
            lock: Mutex::new(()),
            attempts: AtomicU64::new(0),
        });
        if manager.runner.is_some() {
            manager.reconcile().await?;
        }
        Ok(manager)
    }
    fn runner(&self) -> Result<&Arc<dyn Runner>> {
        self.runner
            .as_ref()
            .context("Execution is unavailable in this dashboard.")
    }
    #[cfg(test)]
    fn suite_path(&self, id: &str) -> Result<PathBuf> {
        safe_id(id)?;
        Ok(self
            .root
            .join("plan-store/suites")
            .join(format!("{id}.json")))
    }
    #[cfg(test)]
    pub(crate) fn execution_path(&self, id: &str) -> Result<PathBuf> {
        safe_id(id)?;
        Ok(self
            .root
            .join("plan-store/executions")
            .join(format!("{id}.json")))
    }
    async fn read_suite(&self, id: &str) -> Result<LocalSuite> {
        safe_id(id)?;
        if let Some(persistence) = &self.persistence {
            return persistence
                .local_suite(id)
                .await?
                .with_context(|| format!("unknown suite {id}"));
        }
        #[cfg(not(test))]
        anyhow::bail!("the E2E control-plane persistence is not available");
        #[cfg(test)]
        serde_json::from_slice(
            &fs::read(self.suite_path(id)?).with_context(|| format!("unknown suite {id}"))?,
        )
        .context("decode suite")
    }
    async fn write_suite(&self, suite: &LocalSuite) -> Result<()> {
        if let Some(persistence) = &self.persistence {
            return persistence.save_local_suite(suite).await;
        }
        #[cfg(not(test))]
        anyhow::bail!("the E2E control-plane persistence is not available");
        #[cfg(test)]
        write_json(&self.suite_path(&suite.id)?, suite)
    }
    async fn local_suites(&self) -> Result<Vec<LocalSuite>> {
        let mut suites = if let Some(persistence) = &self.persistence {
            persistence.local_suites().await?
        } else {
            #[cfg(not(test))]
            anyhow::bail!("the E2E control-plane persistence is not available");
            #[cfg(test)]
            read_json_directory(&self.root.join("plan-store/suites"))?
        };
        suites.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(suites)
    }
    pub(crate) async fn read_execution(&self, id: &str) -> Result<PlanExecution> {
        safe_id(id)?;
        if let Some(persistence) = &self.persistence {
            return persistence
                .saved_execution(id)
                .await?
                .context("unknown execution");
        }
        #[cfg(not(test))]
        anyhow::bail!("the E2E control-plane persistence is not available");
        #[cfg(test)]
        let execution: PlanExecution =
            serde_json::from_slice(&fs::read(self.execution_path(id)?)?)?;
        #[cfg(test)]
        ensure!(execution.id == id, "Unsupported execution identity");
        #[cfg(test)]
        Ok(execution)
    }
    async fn write_execution(&self, execution: &PlanExecution) -> Result<()> {
        if let Some(persistence) = &self.persistence {
            return persistence.save_execution_receipt(execution).await;
        }
        #[cfg(not(test))]
        anyhow::bail!("the E2E control-plane persistence is not available");
        #[cfg(test)]
        write_json(&self.execution_path(&execution.id)?, execution)
    }
    /// Changes whenever a slot's previous attempts do.
    pub(crate) fn attempts_revision(&self) -> u64 {
        self.attempts.load(Ordering::SeqCst)
    }
    /// The native runs a later attempt replaced, in every execution.
    pub(crate) async fn previous_attempts(&self) -> Result<BTreeSet<String>> {
        Ok(self
            .executions()
            .await?
            .iter()
            .flat_map(|execution| &execution.slots)
            .flat_map(|slot| &slot.previous_attempts)
            .map(|attempt| attempt.execution_id.clone())
            .collect())
    }
    pub(crate) async fn executions(&self) -> Result<Vec<PlanExecution>> {
        if let Some(persistence) = &self.persistence {
            return persistence.saved_executions().await;
        }
        #[cfg(not(test))]
        anyhow::bail!("the E2E control-plane persistence is not available");
        #[cfg(test)]
        read_json_directory(&self.root.join("plan-store/executions"))
    }

    /// Every suite an execution can run: the master plan's, read-only, then
    /// this Console's, newest first, each with the digest it materializes to.
    pub(crate) async fn suites(&self) -> Result<Vec<SuiteView>> {
        let master = test_plan::embedded()?;
        let mut suites = Vec::new();
        for suite in &master.suites {
            let snapshot = master.materialize(&suite.id)?;
            suites.push(SuiteView {
                id: suite.id.clone(),
                label: suite.label.clone(),
                source: "repository".into(),
                purpose: suite.purpose.clone(),
                scenarios: snapshot.scenario_ids,
                repetitions: suite.repetitions,
                technical_retries: suite.technical_retries,
                sha256: Some(snapshot.profile_sha256),
                updated_at: None,
            });
        }
        for suite in self.local_suites().await? {
            let sha256 = materialize_known(
                &master,
                &suite.id,
                &suite.label,
                &suite.scenarios,
                suite.repetitions,
                suite.technical_retries,
            )
            .ok()
            .and_then(|(snapshot, _)| snapshot)
            .map(|snapshot| snapshot.profile_sha256);
            suites.push(SuiteView {
                id: suite.id,
                label: suite.label,
                source: "local".into(),
                purpose: String::new(),
                scenarios: suite.scenarios,
                repetitions: suite.repetitions,
                technical_retries: suite.technical_retries,
                sha256,
                updated_at: Some(suite.updated_at),
            });
        }
        Ok(suites)
    }

    /// A local suite that starts as a copy of another one, repository or local.
    pub(crate) async fn create_suite(&self, request: SuiteCreateRequest) -> Result<SuiteView> {
        let source = self
            .suites()
            .await?
            .into_iter()
            .find(|suite| suite.id == request.from)
            .with_context(|| format!("unknown suite {}", request.from))?;
        let label = match request.label.trim() {
            "" => format!("{} copy", source.label),
            label => label.to_owned(),
        };
        let id = format!("suite-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let suite = LocalSuite {
            id: id.clone(),
            label,
            scenarios: source.scenarios,
            repetitions: source.repetitions,
            technical_retries: source.technical_retries,
            created_at: now(),
            updated_at: now(),
        };
        self.save_suite(suite).await?;
        self.suite_view(&id).await
    }

    pub(crate) async fn update_suite(&self, update: SuiteUpdateRequest) -> Result<SuiteView> {
        local_only(&update.suite_id)?;
        let mut suite = self.read_suite(&update.suite_id).await?;
        suite.apply(&update);
        suite.updated_at = now();
        self.save_suite(suite).await?;
        self.suite_view(&update.suite_id).await
    }

    /// Only a suite of this Console; executions that ran it keep its name.
    pub(crate) async fn delete_suite(&self, id: &str) -> Result<()> {
        local_only(id)?;
        self.read_suite(id).await?;
        if let Some(persistence) = &self.persistence {
            return persistence.delete_local_suite(id).await;
        }
        #[cfg(not(test))]
        anyhow::bail!("the E2E control-plane persistence is not available");
        #[cfg(test)]
        Ok(fs::remove_file(self.suite_path(id)?)?)
    }

    /// Stored in the canonical order, with whole sequential groups, so the
    /// same scenarios always materialize to the same digest.
    async fn save_suite(&self, mut suite: LocalSuite) -> Result<()> {
        let groups = sequential_groups(&test_plan::embedded()?);
        suite.scenarios = whole_groups(&canonical(&suite.scenarios), &groups).0;
        suite.validate()?;
        self.write_suite(&suite).await
    }

    async fn suite_view(&self, id: &str) -> Result<SuiteView> {
        self.suites()
            .await?
            .into_iter()
            .find(|suite| suite.id == id)
            .with_context(|| format!("unknown suite {id}"))
    }

    /// Start an execution from its parameters alone, on this stack. What runs
    /// is what the parameters hold: their suite names it, and a suite of the
    /// master plan named with exactly what it holds materializes as reviewed
    /// (the digest the workflow records for it). Without a name the suite is
    /// unnamed. A scenario this runner does not know gets a slot that says
    /// so; every other slot runs. A scenario of a sequential group brings the
    /// whole group, and the execution says what was added.
    pub(crate) async fn start_execution(
        self: &Arc<Self>,
        mut parameters: ExecutionParameters,
        label: &str,
    ) -> Result<PlanExecution> {
        let label = clean_label(label)?;
        parameters.model = parameters.model.trim().into();
        parameters.provider = parameters.provider.trim().into();
        parameters.agent = parameters
            .agent
            .map(|agent| agent.trim().to_owned())
            .filter(|agent| !agent.is_empty());
        ensure!(
            !parameters.model.is_empty() && !parameters.provider.is_empty(),
            "Select an execution model."
        );
        let suite = parameters.suite.as_ref();
        for (name, value) in [
            ("model", Some(&parameters.model)),
            ("provider", Some(&parameters.provider)),
            ("agent", parameters.agent.as_ref()),
            ("suite", suite.and_then(|suite| suite.id.as_ref())),
            ("suite label", suite.map(|suite| &suite.label)),
        ] {
            let Some(value) = value else { continue };
            ensure!(
                value.chars().count() <= 200 && !value.chars().any(char::is_control),
                "{name} must be at most 200 characters, without control characters"
            );
        }
        let master = test_plan::embedded()?;
        let (scenarios, mut warnings) = whole_groups(
            &canonical(&parameters.scenarios),
            &sequential_groups(&master),
        );
        parameters.scenarios = scenarios;
        ensure!(
            !parameters.scenarios.is_empty() && parameters.scenarios.len() <= 256,
            "Select between 1 and 256 scenarios."
        );
        ensure!(
            parameters.scenarios.iter().all(|id| !id.is_empty()
                && id.len() <= 100
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))),
            "Scenario ids hold only letters, digits, '_', '.' and '-'."
        );
        ensure!(
            (1..=20).contains(&parameters.runs),
            "runs must be between 1 and 20"
        );
        ensure!(
            parameters.technical_retries <= 3,
            "technical_retries must be between 0 and 3"
        );
        let key = format!("execution:{}", uuid::Uuid::new_v4().simple());
        let id = format!("plan-{}", &artifact::sha256_bytes(key.as_bytes())[7..39]);
        let slots = parameter_slots(
            &master,
            &mut parameters,
            &id,
            label.as_deref(),
            &mut warnings,
        )?;
        let execution = PlanExecution {
            slots,
            id,
            idempotency_key: key,
            label,
            parameters: Some(parameters),
            source: ExecutionSource::Local,
            stack: Vec::new(),
            warnings,
            state: "running".into(),
            started_at: now(),
            updated_at: now(),
            finished_at: None,
            cancel_requested: false,
            error: None,
            measurements: None,
            system_under_test: None,
            rerun: None,
        };
        let _guard = self.lock.lock().await;
        let runner = self.runner()?;
        if let Err(error) = runner.reserve(&execution.id).await {
            return Err(self.busy(runner, error).await);
        }
        if let Err(error) = self.write_execution(&execution).await {
            runner.release(&execution.id).await;
            return Err(error);
        }
        self.spawn_drive(&execution.id);
        Ok(execution)
    }

    /// Run one scenario of a finished local execution again, on this stack,
    /// with the requests its slots ran (every round, the canonical case). The
    /// last attempt counts, as a re-run job does on GitHub. Everything is
    /// checked here and nothing changes yet: each run is replaced when its
    /// new run is admitted (see `drive`). A scenario of a sequential group
    /// runs again with its whole group.
    pub(crate) async fn rerun_scenario(
        self: &Arc<Self>,
        id: &str,
        scenario_id: &str,
    ) -> Result<PlanExecution> {
        let runner = self.runner()?;
        // The stack is read before the lock; the checks run again under it.
        let execution = self.read_execution(id).await?;
        rerun_runs(&execution, scenario_id)?;
        if let Some(pinned) = &execution.system_under_test {
            same_identity(runner, pinned).await?;
        }
        let _guard = self.lock.lock().await;
        let mut execution = self.read_execution(id).await?;
        let runs = rerun_runs(&execution, scenario_id)?;
        if let Err(error) = runner.reserve(id).await {
            return Err(self.busy(runner, error).await);
        }
        let mut scenarios = Vec::new();
        for slot in execution
            .slots
            .iter()
            .filter(|slot| runs.contains(&slot.execution_id))
        {
            if !scenarios.contains(&slot.scenario_id) {
                scenarios.push(slot.scenario_id.clone());
            }
        }
        if scenarios.len() > 1 {
            let note = format!(
                "{} run only together, in this order; running one again runs the whole group.",
                scenarios.join(" then ")
            );
            if !execution.warnings.contains(&note) {
                execution.warnings.push(note);
            }
        }
        execution.rerun = Some(Rerun {
            scenarios,
            runs: runs.into_iter().collect(),
            started_at: now(),
            state: std::mem::replace(&mut execution.state, "running".into()),
            error: execution.error.take(),
            finished_at: execution.finished_at.take(),
        });
        execution.cancel_requested = false;
        execution.updated_at = now();
        if let Err(error) = self.write_execution(&execution).await {
            runner.release(id).await;
            return Err(error);
        }
        self.spawn_drive(id);
        Ok(execution)
    }

    /// A runner that refused a reservation is busy: say which execution holds
    /// it, by name, with its id in parentheses for the Console to open.
    async fn busy(&self, runner: &Arc<dyn Runner>, error: anyhow::Error) -> anyhow::Error {
        let Some(id) = runner.active().await else {
            return error;
        };
        let name = self.read_execution(&id).await.ok().and_then(|e| e.label);
        let holder = name.map_or_else(
            || format!("Another execution ({id})"),
            |name| format!("\"{name}\" ({id})"),
        );
        anyhow::anyhow!("{holder} is still running; wait for it to finish or cancel it.")
    }

    /// Delete a finished execution: its native runs first, previous attempts
    /// included, then the execution.
    pub(crate) async fn delete_execution(&self, id: &str) -> Result<()> {
        let _guard = self.lock.lock().await;
        let execution = self.read_execution(id).await?;
        ensure!(
            !execution.active() && execution.state != "importing",
            "Only a finished execution can be deleted."
        );
        let runner = self.runner()?;
        let children = native_runs(&execution)
            .filter(|child| !child.is_empty())
            .collect::<BTreeSet<_>>();
        for child in children {
            if runner.record(child).await.is_some() {
                runner.remove(child).await?;
            }
        }
        if let Some(persistence) = &self.persistence {
            return persistence.delete_execution_receipt(id).await;
        }
        #[cfg(not(test))]
        anyhow::bail!("the E2E control-plane persistence is not available");
        #[cfg(test)]
        Ok(fs::remove_file(self.execution_path(id)?)?)
    }

    /// Name any execution; an empty label restores the default name.
    pub(crate) async fn rename(&self, id: &str, label: &str) -> Result<PlanExecution> {
        let label = clean_label(label)?;
        let _guard = self.lock.lock().await;
        let mut execution = self.read_execution(id).await?;
        execution.label = label;
        self.write_execution(&execution).await?;
        Ok(execution)
    }

    fn spawn_drive(self: &Arc<Self>, id: &str) {
        let manager = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            if let Err(error) = manager.drive(&id).await {
                tracing::error!(execution_id = %id, error = %error, "plan coordinator stopped");
                // Keep admission until the active child has actually terminated.
                manager
                    .interrupt_after_error(&id, &format!("{error:#}"))
                    .await;
            }
        });
    }
    async fn drive(&self, id: &str) -> Result<()> {
        let runner = self.runner()?;
        // The stack is recorded before the first slot; what cannot be read
        // is a warning on the execution. A scenario run again keeps the
        // recorded stack and says when it ran on another one.
        let (stack, warnings) = runner.stack().await;
        let count = {
            let _guard = self.lock.lock().await;
            let mut execution = self.read_execution(id).await?;
            match &execution.rerun {
                None => {
                    execution.stack = stack;
                    execution.warnings.extend(warnings);
                }
                Some(rerun) => {
                    let changed = stack_changes(&execution.stack, &stack);
                    let mut notes = warnings;
                    if !changed.is_empty() {
                        notes.push(format!(
                            "{} ran again on a stack that differs from the recorded one: {}.",
                            rerun.scenarios.join(", "),
                            changed.join(", ")
                        ));
                    }
                    for note in notes {
                        if !execution.warnings.contains(&note) {
                            execution.warnings.push(note);
                        }
                    }
                }
            }
            self.write_execution(&execution).await?;
            execution.slots.len()
        };
        for index in 0..count {
            let execution = self.read_execution(id).await?;
            let slot = &execution.slots[index];
            let replacing = execution
                .rerun
                .as_ref()
                .is_some_and(|rerun| rerun.runs.contains(&slot.execution_id));
            // A pending slot is admitted, and one whose run a rerun replaces.
            // One without a native run (an unknown scenario) has nothing to
            // admit, one of a group was admitted with the group's first slot.
            if slot.execution_id.is_empty() || !(replacing || slot.state == "pending") {
                continue;
            }
            let (child, previous) = {
                let _guard = self.lock.lock().await;
                let mut execution = self.read_execution(id).await?;
                if execution.cancel_requested {
                    break;
                }
                let previous = execution.slots.clone();
                if replacing {
                    // Nothing is spent on another stack than the one pinned.
                    if let Some(pinned) = &execution.system_under_test {
                        same_identity(runner, pinned).await?;
                    }
                    let run = execution.slots[index].execution_id.clone();
                    let retained = runner.retained(&run).await;
                    replace_run(&mut execution, &run, retained)?;
                    self.attempts.fetch_add(1, Ordering::SeqCst);
                }
                // This write must succeed before invoking native admission.
                let execution_id = execution.slots[index].execution_id.clone();
                for slot in execution
                    .slots
                    .iter_mut()
                    .filter(|slot| slot.execution_id == execution_id)
                {
                    slot.state = "admitting".into();
                }
                execution.updated_at = now();
                self.write_execution(&execution).await?;
                let slot = &execution.slots[index];
                let admitted = match runner
                    .submit(id, serde_json::from_value(slot.request.clone())?)
                    .await
                {
                    Ok(admitted) => admitted,
                    Err(error) if replacing => {
                        // Not admitted: the slots keep the run they had.
                        execution.slots = previous;
                        self.write_execution(&execution).await?;
                        self.attempts.fetch_add(1, Ordering::SeqCst);
                        return Err(error);
                    }
                    Err(error) => return Err(error),
                };
                ensure!(
                    admitted == slot.execution_id,
                    "Native child identity differs from the persisted slot."
                );
                (admitted, replacing.then_some(previous))
            };
            loop {
                let record = runner
                    .record(&child)
                    .await
                    .context("Admitted native execution disappeared")?;
                let terminal = record.phase.terminal();
                {
                    let _guard = self.lock.lock().await;
                    let mut execution = self.read_execution(id).await?;
                    if let Some(report) = record.report.as_ref().filter(|_| terminal) {
                        if let Err(error) =
                            verify_system_identity(&mut execution.system_under_test, report)
                        {
                            // A run of another identity is never projected:
                            // the slots keep their run and this one goes.
                            if let Some(previous) = previous {
                                for (slot, kept) in execution.slots.iter_mut().zip(previous) {
                                    if slot.execution_id == child {
                                        *slot = kept;
                                    }
                                }
                                self.write_execution(&execution).await?;
                                self.attempts.fetch_add(1, Ordering::SeqCst);
                                runner.remove(&child).await?;
                            }
                            return Err(error);
                        }
                    }
                    for slot in execution
                        .slots
                        .iter_mut()
                        .filter(|slot| slot.execution_id == child)
                    {
                        if terminal && record.result_path.is_none() {
                            // The run ended without results: its slots keep
                            // the reason and the next slots still run.
                            slot.state = "finished".into();
                            slot.error = Some(if record.error.is_empty() {
                                "The native run ended without results.".into()
                            } else {
                                record.error.clone()
                            });
                        } else {
                            update_slot(slot, &record, &self.root)?;
                        }
                    }
                    execution.updated_at = now();
                    self.write_execution(&execution).await?;
                    if execution.cancel_requested && !terminal {
                        runner.cancel(&child).await?;
                    }
                }
                // A failed or technically invalid run fails only its slots.
                if terminal {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        // Released under the lock: no one sees the execution finished while
        // the runner is still reserved for it.
        let _guard = self.lock.lock().await;
        let mut execution = self.read_execution(id).await?;
        finish(&mut execution, None, &self.root)?;
        self.write_execution(&execution).await?;
        runner.release(id).await;
        Ok(())
    }
    /// Stop an execution: no next slot is admitted and the running one is cancelled.
    pub(crate) async fn cancel(&self, id: &str) -> Result<Value> {
        let _guard = self.lock.lock().await;
        let mut execution = self.read_execution(id).await?;
        if execution.active() {
            execution.cancel_requested = true;
            execution.state = "cancelling".into();
            execution.updated_at = now();
            self.write_execution(&execution).await?;
            for slot in &execution.slots {
                if matches!(slot.state.as_str(), "admitting" | "running") {
                    if let Some(record) = self.runner()?.record(&slot.execution_id).await {
                        if !record.phase.terminal() {
                            self.runner()?.cancel(&slot.execution_id).await?;
                        }
                    }
                }
            }
        }
        Ok(serde_json::to_value(execution)?)
    }
    async fn interrupt_after_error(&self, id: &str, error: &str) {
        let Ok(runner) = self.runner() else {
            return;
        };
        let Ok(mut execution) = self.read_execution(id).await else {
            return;
        };
        for slot in &execution.slots {
            if let Some(record) = runner.record(&slot.execution_id).await {
                if !record.phase.terminal() {
                    if let Err(error) = runner.cancel(&slot.execution_id).await {
                        tracing::error!(%error, "cannot cancel interrupted child; admission retained");
                        return;
                    }
                    loop {
                        match runner.record(&slot.execution_id).await {
                            Some(record) if record.phase.terminal() => break,
                            None => return,
                            _ => tokio::time::sleep(Duration::from_millis(250)).await,
                        }
                    }
                }
            }
        }
        let _guard = self.lock.lock().await;
        if let Ok(latest) = self.read_execution(id).await {
            execution = latest;
        }
        for slot in &mut execution.slots {
            if let Some(record) = runner.record(&slot.execution_id).await {
                let _ = update_slot(slot, &record, &self.root);
            }
        }
        if let Err(error) = async {
            finish(&mut execution, Some(error.into()), &self.root)?;
            self.write_execution(&execution).await
        }
        .await
        {
            tracing::error!(%error, "cannot persist interruption; admission retained");
            return;
        }
        runner.release(id).await;
    }
    async fn reconcile(&self) -> Result<()> {
        // No import runs at start: drop whatever an interrupted one left.
        let imports = self.root.join(".imports");
        if let Err(error) = fs::remove_dir_all(&imports) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %imports.display(), %error, "cannot clear interrupted imports");
            }
        }
        for mut execution in self.executions().await? {
            if execution.state == "importing" {
                execution.state = "failed".into();
                execution.error =
                    Some("The worker stopped before this import finished. Import it again.".into());
                execution.updated_at = now();
                execution.finished_at = Some(now());
                self.write_execution(&execution).await?;
                continue;
            }
            let was_active = execution.active();
            if !was_active
                && (execution.measurements.is_some()
                    || !execution.slots.iter().any(|s| s.result_path.is_some()))
            {
                continue;
            }
            let finished_at = execution.finished_at.clone();
            if let Some(runner) = &self.runner {
                for slot in &mut execution.slots {
                    if let Some(record) = runner.record(&slot.execution_id).await {
                        ensure!(
                            record.phase.terminal(),
                            "Native child is still active during plan recovery"
                        );
                        if let Err(error) = update_slot(slot, &record, &self.root) {
                            slot.error = Some(error.to_string());
                        }
                    }
                }
            }
            let reason = if was_active {
                Some("Worker restarted. Retained child evidence was reconciled; no work was resumed automatically.".into())
            } else {
                execution.error.clone()
            };
            finish(&mut execution, reason, &self.root)?;
            if !was_active {
                execution.finished_at = finished_at;
            }
            self.write_execution(&execution).await?;
        }
        Ok(())
    }
}

/// Only a suite of this Console changes; the master plan's are read-only.
fn local_only(id: &str) -> Result<()> {
    ensure!(
        !test_plan::embedded()?
            .suites
            .iter()
            .any(|suite| suite.id == id),
        "{id} is a repository suite, read-only; copy it to edit a suite of this Console."
    );
    Ok(())
}
/// A label as typed: trimmed, at most 80 characters; empty means none.
fn clean_label(label: &str) -> Result<Option<String>> {
    let label = label.trim();
    ensure!(
        label.chars().count() <= 80,
        "execution label must be at most 80 characters"
    );
    ensure!(
        !label.chars().any(char::is_control),
        "execution label must not contain control characters"
    );
    Ok((!label.is_empty()).then(|| label.to_owned()))
}
fn safe_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 100
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "Invalid identity"
    );
    Ok(())
}
fn now() -> String {
    Utc::now().to_rfc3339()
}
#[cfg(test)]
fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    artifact::write_atomic(path, &serde_json::to_vec_pretty(value)?)
}
#[cfg(test)]
fn read_json_directory<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let mut result = Vec::new();
    for entry in fs::read_dir(path)? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            result.push(
                serde_json::from_slice(&fs::read(&path)?)
                    .with_context(|| format!("read {}", path.display()))?,
            );
        }
    }
    Ok(result)
}
/// Each scenario once, in one order whatever order they were ticked in, so
/// the same scenarios always make the same suite.
fn canonical(scenarios: &[String]) -> Vec<String> {
    scenarios
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
/// A scenario of a sequential group brings its whole group, in the group's
/// order; a note says what was added.
fn whole_groups(scenarios: &[String], groups: &[Vec<String>]) -> (Vec<String>, Vec<String>) {
    let mut whole: Vec<String> = Vec::new();
    let mut notes = Vec::new();
    for scenario in scenarios {
        let group = groups.iter().find(|group| group.contains(scenario));
        for member in group.map_or(std::slice::from_ref(scenario), Vec::as_slice) {
            if !whole.contains(member) {
                whole.push(member.clone());
            }
        }
        if let Some(group) =
            group.filter(|group| group.iter().any(|member| !scenarios.contains(member)))
        {
            let note = format!(
                "{} run only together, in this order; the whole group was added.",
                group.join(" then ")
            );
            if !notes.contains(&note) {
                notes.push(note);
            }
        }
    }
    (whole, notes)
}
/// A suite this runner materializes from what it holds: its known scenarios
/// under its name, the master plan's sequential groups among them kept
/// together, and apart the scenarios this runner does not know.
fn materialize_known(
    master: &test_plan::MasterPlan,
    id: &str,
    label: &str,
    scenarios: &[String],
    runs: u32,
    technical_retries: u8,
) -> Result<(Option<ProfileSnapshot>, Vec<String>)> {
    let (known, unknown): (Vec<_>, Vec<_>) = scenarios
        .iter()
        .cloned()
        .partition(|id| id.parse::<crate::scenarios::ScenarioId>().is_ok());
    if known.is_empty() {
        return Ok((None, unknown));
    }
    let scenario_groups = sequential_groups(master)
        .into_iter()
        .filter(|group| group.iter().all(|id| known.contains(id)))
        .collect();
    let suite = test_plan::Suite {
        id: id.into(),
        label: label.into(),
        purpose: String::new(),
        metrics: Vec::new(),
        modules: Vec::new(),
        scenarios: known,
        scenario_groups,
        repetitions: runs,
        technical_retries,
        lane: format!("local-{id}"),
    };
    Ok((Some(master.materialize_scope(suite, None)?), unknown))
}
/// Slots of an execution: its suite materialized, then one slot per round
/// for each scenario this runner does not know, carrying the reason instead
/// of a native run. Records the suite in the parameters, with its digest.
fn parameter_slots(
    master: &test_plan::MasterPlan,
    parameters: &mut ExecutionParameters,
    owner: &str,
    label: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<Vec<Slot>> {
    let mut named = parameters.suite.take().and_then(|suite| {
        let id = suite.id?;
        let label = match suite.label.trim() {
            "" => id.clone(),
            label => label.to_owned(),
        };
        Some((id, label))
    });
    // A suite of the master plan named with exactly what it holds runs as
    // reviewed; named with anything else, what the parameters hold runs.
    let reviewed = match &named {
        Some((id, _)) if master.suites.iter().any(|suite| &suite.id == id) => {
            let snapshot = master.materialize(id)?;
            let same = canonical(&snapshot.scenario_ids) == canonical(&parameters.scenarios)
                && snapshot.profile.repetitions == parameters.runs
                && snapshot.profile.technical_retries == parameters.technical_retries;
            if !same {
                warnings.push(format!(
                    "This runner's suite {id} holds other scenarios, runs or retries; the ones given ran under its name."
                ));
            }
            same.then_some(snapshot)
        }
        _ => None,
    };
    let (snapshot, unknown) = match reviewed {
        Some(snapshot) => {
            parameters.scenarios = snapshot.scenario_ids.clone();
            if let Some((_, label)) = named.as_mut() {
                label.clone_from(&snapshot.profile.label);
            }
            (Some(snapshot), Vec::new())
        }
        None => {
            let (id, label) = named
                .as_ref()
                .map_or(("unnamed", ""), |(id, label)| (id.as_str(), label.as_str()));
            materialize_known(
                master,
                id,
                label,
                &parameters.scenarios,
                parameters.runs,
                parameters.technical_retries,
            )?
        }
    };
    let (id, suite_label) = named.map_or((None, String::new()), |(id, label)| (Some(id), label));
    parameters.suite = Some(ExecutionSuite {
        id,
        label: suite_label,
        sha256: snapshot
            .as_ref()
            .map(|snapshot| snapshot.profile_sha256.clone())
            .unwrap_or_default(),
    });
    let mut slots = match &snapshot {
        Some(snapshot) => campaign_slots(
            snapshot,
            owner,
            label.unwrap_or("Execution"),
            &parameters.model,
            &parameters.provider,
            parameters.agent.as_deref(),
        )?,
        None => Vec::new(),
    };
    for round in 1..=parameters.runs {
        for scenario in &unknown {
            let mut slot = github::slot(round, scenario, scenario);
            slot.state = "not_run".into();
            slot.error = Some(format!(
                "This runner does not know the scenario '{scenario}'."
            ));
            slots.push(slot);
        }
    }
    Ok(slots)
}
/// Scenarios the master plan runs only together, in order, in one session.
pub(crate) fn sequential_groups(master: &test_plan::MasterPlan) -> Vec<Vec<String>> {
    master
        .suites
        .iter()
        .flat_map(|suite| &suite.scenario_groups)
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
/// One slot per scenario of every campaign group.
fn campaign_slots(
    snapshot: &ProfileSnapshot,
    owner: &str,
    label: &str,
    model: &str,
    provider: &str,
    agent: Option<&str>,
) -> Result<Vec<Slot>> {
    let mut slots = Vec::new();
    for (round, campaign) in snapshot.campaigns.iter().enumerate() {
        for group in campaign["groups"]
            .as_array()
            .context("Missing campaign groups")?
        {
            let group_id = group["id"].as_str().context("Missing group identity")?;
            let scenario_ids = group["scenarios"]
                .as_array()
                .context("Native scenarios required")?;
            let key = format!("{owner}:round-{}:{group_id}", round + 1);
            let request: RunRequest = serde_json::from_value(
                json!({"idempotency_key": key, "label": format!("{label} · round {} · {group_id}", round + 1), "lane": campaign["lane"],
                "model": model, "provider": provider, "agent": agent,
                "scenarios": scenario_ids, "runs": 1, "technical_retries": group["technical_retries"]}),
            )?;
            crate::control::validate_run_request(&request)?;
            for scenario_id in scenario_ids {
                slots.push(Slot {
                    round: round as u32 + 1,
                    group_id: group_id.into(),
                    scenario_id: scenario_id
                        .as_str()
                        .context("Native scenario required")?
                        .into(),
                    execution_id: execution_id_for_key(&key),
                    request: serde_json::to_value(&request)?,
                    state: "pending".into(),
                    result_path: None,
                    error: None,
                    observed: 0,
                    completed: 0,
                    passed: 0,
                    technical_valid: 0,
                    eligible: false,
                    previous_attempts: Vec::new(),
                });
            }
        }
    }
    ensure!(
        Some(slots.len() as u64) == snapshot.budget["planned_runs"].as_u64(),
        "Materialized slot coverage differs"
    );
    Ok(slots)
}
/// Project one terminal native run into its slot.
fn update_slot(slot: &mut Slot, record: &ExecutionRecord, root: &Path) -> Result<()> {
    ensure!(
        record.execution_id == slot.execution_id
            && record.idempotency_key == slot.request["idempotency_key"],
        "Native execution identity mismatch"
    );
    slot.state = if record.phase.terminal() {
        "finished"
    } else {
        "running"
    }
    .into();
    slot.error = (!record.error.is_empty()).then(|| record.error.clone());
    if !record.phase.terminal() {
        return Ok(());
    }
    let path = record
        .result_path
        .as_ref()
        .context("Native Results artifact is unavailable")?;
    artifact::validate_relative_path(Path::new(path))?;
    ensure!(
        Path::new(path) == Path::new(&slot.execution_id).join("results.json"),
        "Native result path belongs to a different child"
    );
    slot.result_path = Some(path.clone());
    let (report, _) = E2eReport::read_from(&root.join(path))?;
    ensure!(
        report.execution.execution_id == slot.execution_id,
        "Native artifact belongs to a different execution"
    );
    let requested: Vec<_> = slot.request["scenarios"]
        .as_array()
        .context("Native request scenarios are absent")?
        .iter()
        .map(|id| id.as_str().context("Native request scenario is invalid"))
        .collect::<Result<_>>()?;
    ensure!(
        report
            .scenarios
            .iter()
            .map(|scenario| scenario.scenario_id.as_str())
            .eq(requested.iter().copied()),
        "Native report group differs from admission"
    );
    let scenario = report
        .scenarios
        .iter()
        .find(|scenario| scenario.scenario_id == slot.scenario_id)
        .context("Native report omitted the planned scenario")?;
    let aggregate = &scenario.aggregate;
    ensure!(
        aggregate.planned_runs == 1,
        "Native slot count differs from admission"
    );
    slot.observed = aggregate.observed_runs;
    slot.completed = aggregate.completed_runs;
    slot.passed = aggregate.completed_runs;
    slot.technical_valid = aggregate.technical_valid_runs;
    slot.eligible = report.report_state == ReportState::Complete
        && aggregate.observed_runs == 1
        && aggregate.technical_invalid_runs == 0
        && aggregate.undetermined_runs == 0;
    Ok(())
}

fn verify_system_identity(pinned: &mut Option<Value>, report: &E2eReport) -> Result<()> {
    let observed = serde_json::to_value(&report.system_under_test)?;
    if let Some(identity) = pinned.as_mut() {
        let changed = identity_differences(identity, &observed);
        ensure!(
            changed.is_empty(),
            "Stack or runner identity changed during the composed execution: {}",
            changed.join("; ")
        );
        for (id, digest) in observed["contract_hashes"]
            .as_object()
            .context("Native contract hashes are absent")?
        {
            identity["contract_hashes"][id] = digest.clone();
        }
    } else {
        *pinned = Some(observed);
    }
    Ok(())
}

/// What differs between the identity an execution pinned and another one:
/// every field but the contract hashes, then each contract both hold.
fn identity_differences(pinned: &Value, observed: &Value) -> Vec<String> {
    let without_contracts = |identity: &Value| {
        let mut identity = identity.clone();
        if let Some(fields) = identity.as_object_mut() {
            fields.remove("contract_hashes");
        }
        identity
    };
    let mut changed = Vec::new();
    identity_changes(
        "",
        &without_contracts(pinned),
        &without_contracts(observed),
        &mut changed,
    );
    for (id, digest) in observed["contract_hashes"]
        .as_object()
        .into_iter()
        .flatten()
    {
        if pinned["contract_hashes"]
            .get(id)
            .is_some_and(|previous| previous != digest)
        {
            changed.push(format!("native function contract {id} changed"));
        }
    }
    changed
}

/// Refuse to run on a stack whose identity differs from the pinned one.
async fn same_identity(runner: &Arc<dyn Runner>, pinned: &Value) -> Result<()> {
    let current = runner
        .identity()
        .await
        .context("Cannot read the identity of this stack")?;
    let changed = identity_differences(pinned, &current);
    ensure!(
        changed.is_empty(),
        "This stack is not the one the execution ran on ({}); running a scenario again here would mix them. Use Run again to start a new execution on this stack.",
        changed.join("; ")
    );
    Ok(())
}

/// The native runs running a scenario of this execution again replaces,
/// after every check that needs nothing but the execution.
fn rerun_runs(execution: &PlanExecution, scenario_id: &str) -> Result<BTreeSet<String>> {
    if let ExecutionSource::Github { url, .. } = &execution.source {
        anyhow::bail!(
            "This execution was imported from GitHub; running a scenario here would mix this stack with the one it ran on. Re-run its job on GitHub ({url}) and import the run again: the import takes the highest attempt."
        );
    }
    ensure!(
        !execution.active() && execution.state != "importing",
        "Only a finished execution can run a scenario again."
    );
    let runs = execution
        .slots
        .iter()
        .filter(|slot| slot.scenario_id == scenario_id && !slot.execution_id.is_empty())
        .map(|slot| slot.execution_id.clone())
        .collect::<BTreeSet<_>>();
    if runs.is_empty() {
        anyhow::bail!(execution
            .slots
            .iter()
            .find_map(|slot| (slot.scenario_id == scenario_id)
                .then(|| slot.error.clone())
                .flatten())
            .unwrap_or_else(|| format!(
                "This execution did not run the scenario '{scenario_id}'."
            )));
    }
    for slot in execution
        .slots
        .iter()
        .filter(|slot| runs.contains(&slot.execution_id))
    {
        attempt_request(&execution.id, slot, slot.previous_attempts.len() + 2)?;
    }
    Ok(runs)
}

/// The request of a slot's attempt: the one it ran, under its own key.
fn attempt_request(owner: &str, slot: &Slot, attempt: usize) -> Result<Value> {
    let mut request: RunRequest = serde_json::from_value(slot.request.clone())
        .with_context(|| format!("The recorded request of {} is unreadable", slot.scenario_id))?;
    request.idempotency_key = format!(
        "{owner}:round-{}:{}:attempt-{attempt}",
        slot.round, slot.group_id
    );
    crate::control::validate_run_request(&request)?;
    Ok(serde_json::to_value(&request)?)
}

/// Admit a new attempt for the slots of one run: that run, if it was ever
/// admitted, becomes their previous attempt, and they start over.
fn replace_run(execution: &mut PlanExecution, run: &str, retained: bool) -> Result<()> {
    let owner = execution.id.clone();
    for slot in execution
        .slots
        .iter_mut()
        .filter(|slot| slot.execution_id == run)
    {
        if retained {
            slot.previous_attempts.push(SlotAttempt {
                execution_id: slot.execution_id.clone(),
                error: slot.error.clone(),
            });
        }
        slot.request = attempt_request(&owner, slot, slot.previous_attempts.len() + 1)?;
        slot.execution_id = execution_id_for_key(
            slot.request["idempotency_key"]
                .as_str()
                .context("attempt key")?,
        );
        slot.state = "pending".into();
        (slot.result_path, slot.error) = (None, None);
        (
            slot.observed,
            slot.completed,
            slot.passed,
            slot.technical_valid,
        ) = (0, 0, 0, 0);
        slot.eligible = false;
    }
    Ok(())
}

/// Each field that differs between two identities, as `path: before → after`
/// (`harness_version: 1.8.8 → 1.8.9`, `stack.stack_versions.state: …`).
fn identity_changes(path: &str, before: &Value, after: &Value, changed: &mut Vec<String>) {
    match (before, after) {
        (Value::Object(left), Value::Object(right)) => {
            for key in left.keys().chain(right.keys()).collect::<BTreeSet<_>>() {
                let path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let missing = Value::Null;
                identity_changes(
                    &path,
                    left.get(key).unwrap_or(&missing),
                    right.get(key).unwrap_or(&missing),
                    changed,
                );
            }
        }
        _ if before != after => changed.push(format!("{path}: {before} → {after}")),
        _ => {}
    }
}
/// The workers whose recorded entry differs between two observations of the
/// stack; nothing when either could not be read.
fn stack_changes(recorded: &[StackWorker], now: &[StackWorker]) -> Vec<String> {
    if recorded.is_empty() || now.is_empty() {
        return Vec::new();
    }
    let entries = |stack: &[StackWorker], name: &str| {
        stack
            .iter()
            .filter(|worker| worker.name == name)
            .cloned()
            .collect::<Vec<_>>()
    };
    recorded
        .iter()
        .chain(now)
        .map(|worker| worker.name.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|name| entries(recorded, name) != entries(now, name))
        .map(str::to_owned)
        .collect()
}
/// Every native run an execution's slots ran, previous attempts included.
pub(crate) fn native_runs(execution: &PlanExecution) -> impl Iterator<Item = &str> {
    execution.slots.iter().flat_map(|slot| {
        std::iter::once(slot.execution_id.as_str()).chain(
            slot.previous_attempts
                .iter()
                .map(|attempt| attempt.execution_id.as_str()),
        )
    })
}
fn result_paths(execution: &PlanExecution, root: &Path) -> Vec<PathBuf> {
    execution
        .slots
        .iter()
        .filter_map(|s| s.result_path.as_ref().map(|path| root.join(path)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
fn finish(execution: &mut PlanExecution, error: Option<String>, root: &Path) -> Result<()> {
    for slot in &mut execution.slots {
        if matches!(slot.state.as_str(), "pending" | "admitting" | "running") {
            slot.state = "not_run".into();
        }
    }
    // A slot with a native run that never ran keeps the execution from
    // being complete (a slot of an unknown scenario has none).
    let never_ran = execution
        .slots
        .iter()
        .any(|slot| slot.state == "not_run" && !slot.execution_id.is_empty());
    let stopped = if execution.cancel_requested {
        Some("was cancelled".to_owned())
    } else {
        error.as_ref().map(|error| format!("stopped: {error}"))
    };
    execution.updated_at = now();
    execution.finished_at = Some(now());
    match execution.rerun.take() {
        // A rerun that stopped, or that leaves slots that never ran, returns
        // the execution to the finished state it came from, reason included.
        Some(rerun) if stopped.is_some() || never_ran => {
            if let Some(reason) = stopped {
                let note = format!(
                    "Running {} again {reason}; what it did not run keeps its previous attempt.",
                    rerun.scenarios.join(", ")
                );
                if !execution.warnings.contains(&note) {
                    execution.warnings.push(note);
                }
            }
            if rerun
                .runs
                .iter()
                .all(|run| execution.slots.iter().any(|slot| &slot.execution_id == run))
            {
                // Nothing was replaced: it finished when it did.
                execution.finished_at = rerun.finished_at;
            }
            execution.cancel_requested = rerun.state == "cancelled";
            execution.state = rerun.state;
            execution.error = rerun.error;
        }
        _ => {
            execution.state = if execution.cancel_requested {
                "cancelled"
            } else if error.is_some() || never_ran {
                "interrupted"
            } else {
                "completed"
            }
            .into();
            execution.error = error;
        }
    }
    let paths = result_paths(execution, root);
    // Only the current attempts are measured.
    if paths.is_empty() {
        execution.measurements = None;
    } else {
        match test_plan::measure(&paths) {
            Ok(value) => execution.measurements = Some(value),
            Err(error) => {
                execution.error =
                    Some(format!("Native evidence cannot be consolidated: {error:#}"));
                execution.measurements = None;
                if execution.state == "completed" {
                    execution.state = "interrupted".into();
                }
            }
        }
    }
    // Every slot ran, but not all cleanly: the first reason is the
    // execution's, so a completed execution never hides why.
    if execution.state == "completed" && execution.error.is_none() {
        execution.error = execution
            .slots
            .iter()
            .find(|slot| !slot.eligible)
            .map(|slot| {
                format!(
                    "{}: {}",
                    slot.scenario_id,
                    slot.error
                        .as_deref()
                        .unwrap_or("its run is technically invalid or undetermined")
                )
            });
    }
    Ok(())
}
pub(crate) fn execution_summary(execution: &PlanExecution) -> Value {
    json!({"id": execution.id, "state": execution.state, "started_at": execution.started_at, "finished_at": execution.finished_at, "planned": execution.slots.len(),
        "finished": execution.slots.iter().filter(|s| s.state == "finished").count(), "observed": execution.slots.iter().map(|s| s.observed).sum::<u32>(), "completed": execution.slots.iter().map(|s| s.completed).sum::<u32>(), "passed": execution.slots.iter().map(|s| s.passed).sum::<u32>(), "technical_valid": execution.slots.iter().map(|s| s.technical_valid).sum::<u32>(), "error": execution.error,
        "active_slot": execution.slots.iter().find(|s| matches!(s.state.as_str(), "running" | "admitting"))})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{ExecutionPhase, LaneBudget};
    use crate::identity::{ExecutionIdentity, StackIdentity, SystemUnderTestIdentity};
    use crate::report::E2eManifest;
    use crate::report::{E2eRunReport, E2eScenarioReport, ModelArtifact, RunStatus};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct FakeRunner {
        root: PathBuf,
        owner: Mutex<Option<String>>,
        records: Mutex<HashMap<String, ExecutionRecord>>,
        submitted: AtomicUsize,
        hold: AtomicBool,
        lose_artifact: AtomicBool,
        wrong_identity: AtomicBool,
        fail_next: AtomicBool,
        crash_next: AtomicBool,
        fail_receipt: AtomicBool,
        /// The stack now runs another Harness than the runs recorded.
        new_harness: AtomicBool,
        /// The next run reports another Harness than the stack did.
        diverge_next: AtomicBool,
        /// Runs whose evidence can no longer be read; still stored.
        unreadable: std::sync::Mutex<BTreeSet<String>>,
        /// Whether the stack's path worker has uncommitted changes.
        dirty: AtomicBool,
    }
    impl FakeRunner {
        fn new(root: PathBuf) -> Self {
            Self {
                root,
                owner: Mutex::new(None),
                records: Mutex::new(HashMap::new()),
                submitted: AtomicUsize::new(0),
                hold: AtomicBool::new(false),
                lose_artifact: AtomicBool::new(false),
                wrong_identity: AtomicBool::new(false),
                fail_next: AtomicBool::new(false),
                crash_next: AtomicBool::new(false),
                fail_receipt: AtomicBool::new(false),
                new_harness: AtomicBool::new(false),
                diverge_next: AtomicBool::new(false),
                unreadable: std::sync::Mutex::new(BTreeSet::new()),
                dirty: AtomicBool::new(true),
            }
        }
        fn system(&self, harness: &str) -> SystemUnderTestIdentity {
            SystemUnderTestIdentity {
                stack: StackIdentity::Source {
                    workers_repository: "iii-hq/workers".into(),
                    workers_revision: "0123456789abcdef0123456789abcdef01234567".into(),
                },
                engine_version: "0.22.0".into(),
                engine_revision: None,
                harness_version: harness.into(),
                e2e_repository: "iii-hq/harness-e2e".into(),
                e2e_revision: "0123456789abcdef0123456789abcdef01234567".into(),
                contract_hashes: BTreeMap::from([(
                    "harness::status".into(),
                    artifact::sha256_bytes(b"contract"),
                )]),
            }
        }
        fn native_record(&self, request: RunRequest) -> Result<ExecutionRecord> {
            let id = execution_id_for_key(&request.idempotency_key);
            let max_cases = request.scenarios.len() as u16;
            let scenarios = request
                .scenarios
                .iter()
                .map(|scenario| {
                    let seed = request.seed.unwrap_or_else(|| scenario.canonical_seed());
                    let materialized = scenario.materialize("profile-test", seed)?;
                    let (case, policy) = (materialized.case, materialized.spec.execution);
                    let mut run = E2eRunReport::new(
                        format!("{id}-{scenario}-run"),
                        format!("{id}-{scenario}-attempt"),
                        1,
                        format!("{id}-{scenario}-session"),
                        "prompt".into(),
                    );
                    run.wall_time_ms = 100;
                    run.cost = crate::report::CostReport {
                        subject_usd: Some(0.25),
                        total_usd: Some(0.25),
                    };
                    run.score = Some(80);
                    run.set_completion(
                        crate::report::CompletionState::Completed,
                        crate::report::EvaluatorAvailability::Available,
                    );
                    run.finish(if self.fail_next.swap(false, Ordering::SeqCst) {
                        RunStatus::HardGateFailed
                    } else {
                        RunStatus::Passed
                    });
                    Ok(E2eScenarioReport::aggregate_case(case, policy, vec![run]))
                })
                .collect::<Result<Vec<_>>>()?;
            let execution = ExecutionIdentity {
                execution_id: id.clone(),
                lane: request.lane.clone(),
                started_at: now(),
                completed_at: now(),
            };
            let digest = artifact::sha256_bytes(b"contract");
            let system = self.system(if self.diverge_next.swap(false, Ordering::SeqCst) {
                "1.9.0"
            } else {
                "1.8.0"
            });
            let model = |model: String, provider: String| ModelArtifact {
                model,
                provider,
                agent: None,
                context_window: 128000,
                max_output_tokens: 4096,
                supports_tools: Some(true),
                supports_vision: None,
            };
            let subject = model(request.model.clone(), request.provider.clone());
            let manifest = E2eManifest {
                execution: execution.clone(),
                system_under_test: system.clone(),
                subject: subject.clone(),
                control_plane: crate::wire::ControlPlaneEvidence {
                    functions: vec![crate::wire::FunctionContractEvidence {
                        function_id: "harness::status".into(),
                        request_schema: json!({"type": "object"}),
                        response_schema: json!({"type": "object"}),
                        sha256: digest,
                    }],
                },
                observation_contract: None,
                worker_contracts: Vec::new(),
            };
            let mut report = E2eReport::new(execution, system, subject, None, scenarios);
            let output = self.root.join(&id);
            fs::create_dir_all(&output)?;
            let path = report.write_to(&output, &manifest)?;
            if self.lose_artifact.load(Ordering::SeqCst) {
                fs::remove_file(&path)?;
            }
            Ok(ExecutionRecord {
                execution_id: id,
                idempotency_key: request.idempotency_key.clone(),
                phase: if self.hold.load(Ordering::SeqCst) {
                    ExecutionPhase::Executing
                } else {
                    ExecutionPhase::Completed
                },
                requested_at: now(),
                updated_at: now(),
                request,
                request_sha256: String::new(),
                run_contract_sha256: None,
                lane_budget: LaneBudget {
                    max_cases,
                    max_runs_per_case: 1,
                    max_technical_retries: 1,
                    max_declared_turns: 100,
                },
                transitions: Vec::new(),
                journal_progress: Default::default(),
                active_attempt: None,
                resume_state_path: None,
                resume_state_sha256: None,
                cancel_requested: false,
                error: String::new(),
                result_path: Some(path.strip_prefix(&self.root)?.to_string_lossy().into()),
                report: Some(report),
                dashboard_projection: None,
                manifest: Some(manifest),
                observation: None,
                observation_artifact: None,
                archive: None,
            })
        }
    }
    #[async_trait]
    impl Runner for FakeRunner {
        async fn active(&self) -> Option<String> {
            self.owner.lock().await.clone()
        }
        async fn reserve(&self, owner: &str) -> Result<()> {
            let mut active = self.owner.lock().await;
            ensure!(active.is_none(), "busy");
            *active = Some(owner.into());
            if self.fail_receipt.load(Ordering::SeqCst) {
                fs::remove_dir(self.root.join("plan-store/executions"))?;
                fs::write(self.root.join("plan-store/executions"), b"unwritable")?;
            }
            Ok(())
        }
        async fn release(&self, owner: &str) {
            let mut active = self.owner.lock().await;
            if active.as_deref() == Some(owner) {
                *active = None;
            }
        }
        async fn submit(&self, owner: &str, request: RunRequest) -> Result<String> {
            ensure!(
                self.owner.lock().await.as_deref() == Some(owner),
                "missing reservation"
            );
            // The whole receipt and every deterministic child are already on disk.
            let receipt: PlanExecution = serde_json::from_slice(&fs::read(
                self.root
                    .join("plan-store/executions")
                    .join(format!("{owner}.json")),
            )?)?;
            let id = execution_id_for_key(&request.idempotency_key);
            ensure!(
                receipt.slots.iter().any(|slot| slot.execution_id == id
                    && slot.request == serde_json::to_value(&request).unwrap()),
                "child was not persisted before dispatch"
            );
            self.submitted.fetch_add(1, Ordering::SeqCst);
            let mut record = self.native_record(request)?;
            if self.crash_next.swap(false, Ordering::SeqCst) {
                // The native run failed before it had results.
                record.phase = ExecutionPhase::Failed;
                record.error = "fixture repository unavailable".into();
                (record.report, record.manifest, record.result_path) = (None, None, None);
                fs::remove_dir_all(self.root.join(&id))?;
            }
            self.records.lock().await.insert(id.clone(), record);
            Ok(if self.wrong_identity.load(Ordering::SeqCst) {
                "different-child".into()
            } else {
                id
            })
        }
        async fn record(&self, id: &str) -> Option<ExecutionRecord> {
            if self.unreadable.lock().unwrap().contains(id) {
                return None;
            }
            self.records.lock().await.get(id).cloned()
        }
        async fn retained(&self, id: &str) -> bool {
            self.records.lock().await.contains_key(id)
        }
        async fn identity(&self) -> Result<Value> {
            Ok(serde_json::to_value(self.system(
                if self.new_harness.load(Ordering::SeqCst) {
                    "1.9.0"
                } else {
                    "1.8.0"
                },
            ))?)
        }
        async fn cancel(&self, id: &str) -> Result<()> {
            if let Some(record) = self.records.lock().await.get_mut(id) {
                record.phase = ExecutionPhase::Cancelled;
                record.cancel_requested = true;
            }
            Ok(())
        }
        async fn install(&self, record: ExecutionRecord) -> Result<()> {
            self.records
                .lock()
                .await
                .insert(record.execution_id.clone(), record);
            Ok(())
        }
        async fn remove(&self, id: &str) -> Result<()> {
            ensure!(
                self.records.lock().await.remove(id).is_some(),
                "unknown E2E execution {id}"
            );
            let evidence = self.root.join(id);
            if evidence.exists() {
                fs::remove_dir_all(evidence)?;
            }
            Ok(())
        }
        async fn stack(&self) -> (Vec<StackWorker>, Vec<String>) {
            (
                vec![StackWorker {
                    name: "queue".into(),
                    source: WorkerSource::Path,
                    requested: None,
                    observed: Some("0.4.1".into()),
                    commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
                    dirty: Some(self.dirty.load(Ordering::SeqCst)),
                    groups: Vec::new(),
                }],
                vec!["Worker sources were not recorded: compose is unavailable".into()],
            )
        }
    }
    /// What running a suite of the master plan as it is sends.
    fn suite_parameters(suite: &str) -> ExecutionParameters {
        let snapshot = test_plan::embedded().unwrap().materialize(suite).unwrap();
        ExecutionParameters {
            suite: Some(ExecutionSuite {
                id: Some(suite.into()),
                label: String::new(),
                sha256: String::new(),
            }),
            scenarios: snapshot.scenario_ids,
            runs: snapshot.profile.repetitions,
            technical_retries: snapshot.profile.technical_retries,
            model: "model".into(),
            provider: "provider".into(),
            agent: None,
        }
    }
    fn manager(root: &Path, runner: Arc<FakeRunner>) -> Arc<PlanStore> {
        manager_with_gh(root, runner, github::GithubCli::default())
    }
    fn manager_with_gh(
        root: &Path,
        runner: Arc<FakeRunner>,
        github: github::GithubCli,
    ) -> Arc<PlanStore> {
        fs::create_dir_all(root.join("plan-store/suites")).unwrap();
        fs::create_dir_all(root.join("plan-store/executions")).unwrap();
        Arc::new(PlanStore {
            root: root.into(),
            persistence: None,
            runner: Some(runner),
            github,
            lock: Mutex::new(()),
            attempts: AtomicU64::new(0),
        })
    }
    /// A stand-in `gh`: a shell script, with a short deadline.
    fn fake_gh(directory: &Path, script: &str) -> github::GithubCli {
        use std::os::unix::fs::PermissionsExt;
        let program = directory.join("gh");
        fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        github::GithubCli {
            program,
            api_timeout: Duration::from_millis(500),
            download_timeout: Duration::from_millis(500),
        }
    }
    async fn terminal(manager: &PlanStore, id: &str) -> PlanExecution {
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let execution = manager.read_execution(id).await.unwrap();
                if !execution.active() {
                    return execution;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("coordinator did not finish")
    }
    /// An execution of a suite of the master plan, as it is.
    async fn started(manager: &Arc<PlanStore>, suite: &str) -> String {
        manager
            .start_execution(suite_parameters(suite), "")
            .await
            .unwrap()
            .id
    }
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires HARNESS_E2E_TEST_DATABASE_URL and an isolated database worker; run serially"]
    async fn real_database_start_recreates_stale_suite_and_execution_tables_keeping_their_rows() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let url = std::env::var("HARNESS_E2E_TEST_DATABASE_URL").unwrap();
        let client = iii_sdk::register_worker(&url, iii_sdk::InitOptions::default());
        tokio::time::timeout(Duration::from_secs(10), async {
            while client.get_connection_state() != iii_sdk::runtime::IIIConnectionState::Connected {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let db = Persistence::new(client.clone(), "harness_e2e".into(), "default".into());
        db.initialize(root.path()).await.unwrap();
        db.transaction(vec![
            json!({"sql":"DELETE FROM saved_plan_executions","params":[]}),
            json!({"sql":"DELETE FROM local_suites","params":[]}),
        ])
        .await
        .unwrap();
        let manager = Arc::new(PlanStore {
            root: root.path().into(),
            persistence: Some(db.clone()),
            runner: Some(runner),
            github: github::GithubCli::default(),
            lock: Mutex::new(()),
            attempts: AtomicU64::new(0),
        });
        let suite = manager
            .create_suite(SuiteCreateRequest {
                from: "pr".into(),
                label: String::new(),
            })
            .await
            .unwrap();
        let id = started(&manager, "pr").await;
        let execution = terminal(&manager, &id).await;

        // Both tables were written under another layout: the next start
        // recreates them with the rows it can still read.
        db.transaction(vec![json!({
            "sql": "UPDATE harness_e2e_storage SET fingerprint = 'sha256:foreign' WHERE name IN ('local_suites', 'saved_plan_executions')",
            "params": []
        })])
        .await
        .unwrap();
        db.initialize(root.path()).await.unwrap();
        assert_eq!(
            db.local_suite(&suite.id).await.unwrap().unwrap().label,
            "PR copy"
        );
        assert_eq!(
            db.saved_execution(&id).await.unwrap().unwrap().slots.len(),
            execution.slots.len()
        );

        // Reads delete a row this runner cannot read instead of failing.
        let payload = json!({"id": suite.id}).to_string();
        let hash = artifact::sha256_bytes(payload.as_bytes());
        db.transaction(vec![json!({
            "sql": "UPDATE local_suites SET payload_json = ?, payload_sha256 = ? WHERE id = ?",
            "params": [payload, hash, suite.id]
        })])
        .await
        .unwrap();
        assert!(db.local_suites().await.unwrap().is_empty());
        assert!(db.local_suite(&suite.id).await.unwrap().is_none());
        client.shutdown_async().await;
    }
    #[test]
    fn native_measurements_count_retry_consumption_once_and_reject_reused_attempts() {
        let root = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new(root.path().into());
        let efficiency = |tokens| {
            serde_json::from_value(json!({"wall_time_ms": 0, "root_turns": 1, "child_turns": 0, "child_sessions": 0, "function_calls": 1, "function_call_errors": 0, "validation_retries": 0, "transient_resumes": 0, "wake_resumes": 0, "effective_fan_out": 0, "critical_path_ms": 0, "input_tokens": tokens, "output_tokens": 0, "total_tokens": tokens, "cost_usd": null, "observed_work": 1, "technical_attempts": 1, "observed_complexity": {}})).unwrap()
        };
        let mut failed = E2eRunReport::new(
            "retry-run".into(),
            "retry-attempt".into(),
            1,
            "retry-session".into(),
            "prompt".into(),
        );
        failed.finish(RunStatus::InfrastructureError);
        failed.efficiency = Some(efficiency(20));
        let mut paths = Vec::new();
        for id in ["first", "second"] {
            let request: RunRequest = serde_json::from_value(json!({"idempotency_key": id, "model": "model", "provider": "provider", "scenarios": ["tool_contract_recovery"], "runs": 1, "technical_retries": 0})).unwrap();
            let record = runner.native_record(request).unwrap();
            let mut report = record.report.unwrap();
            let scenario = report.scenarios.pop().unwrap();
            let mut run = scenario.runs[0].clone();
            run.run_id = failed.run_id.clone();
            run.attempt_number = 2;
            run.efficiency = Some(efficiency(100));
            run.attach_retry_attempts(vec![crate::report::RetryAttemptReport::from(&failed)]);
            report.scenarios = vec![E2eScenarioReport::aggregate_case(
                scenario.case.unwrap(),
                scenario.execution_policy,
                vec![run],
            )];
            let path = report
                .write_to(&root.path().join(id), &record.manifest.unwrap())
                .unwrap();
            paths.push(path);
        }
        let measurement = test_plan::measure(&paths[..1]).unwrap();
        assert_eq!(measurement["cohorts"][0]["aggregate"]["observed_runs"], 1);
        assert_eq!(
            measurement["cohorts"][0]["consumption"]["total_tokens_consumed"],
            120
        );
        assert_eq!(
            measurement["cohorts"][0]["aggregate"]["failed_attempt_tokens"],
            20
        );
        assert!(test_plan::measure(&paths)
            .unwrap_err()
            .to_string()
            .contains("duplicate retry"));
    }

    #[tokio::test]
    async fn a_local_suite_runs_under_its_name_with_the_digest_it_lists() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let created = manager
            .create_suite(SuiteCreateRequest {
                from: "pr".into(),
                label: "  ".into(),
            })
            .await
            .unwrap();
        assert_eq!(
            (created.label.as_str(), created.source.as_str()),
            ("PR copy", "local")
        );
        let suite = manager
            .update_suite(SuiteUpdateRequest {
                suite_id: created.id.clone(),
                label: Some(" Edited PR ".into()),
                scenarios: Some(vec!["persistent_state".into(), "minimal_path".into()]),
                repetitions: Some(2),
                technical_retries: Some(0),
            })
            .await
            .unwrap();
        // Stored in one order, whatever order it was edited in.
        assert_eq!(suite.scenarios, vec!["minimal_path", "persistent_state"]);
        assert_eq!(suite.label, "Edited PR");
        let parameters = ExecutionParameters {
            suite: Some(ExecutionSuite {
                id: Some(suite.id.clone()),
                label: suite.label.clone(),
                sha256: "ignored".into(),
            }),
            scenarios: suite.scenarios.clone(),
            runs: suite.repetitions,
            technical_retries: suite.technical_retries,
            model: "model".into(),
            provider: "provider".into(),
            agent: None,
        };
        let id = manager.start_execution(parameters, "").await.unwrap().id;
        let execution = terminal(&manager, &id).await;
        assert_eq!(
            execution.parameters.as_ref().unwrap().suite,
            Some(ExecutionSuite {
                id: Some(suite.id.clone()),
                label: "Edited PR".into(),
                sha256: suite.sha256.clone().unwrap(),
            })
        );
        assert_eq!(
            execution.slots[0].request["lane"],
            format!("local-{}", suite.id)
        );
        assert_eq!(execution.slots.len(), 4);
        crate::dashboard::presenter::validate_execution_id(&execution.id).unwrap();
        for invalid in [
            "plan-../results",
            "plan-short",
            "plan-0000000000000000000000000000000/",
        ] {
            assert!(crate::dashboard::presenter::validate_execution_id(invalid).is_err());
        }
        let detail = manager
            .execution_detail(&execution.id, &[])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail["id"], execution.id);
        assert_eq!(detail["parameters"]["suite"]["id"], json!(suite.id));
        assert_eq!(detail["assessment_summary"]["run_count"], 4);
        assert_eq!(detail["native_execution_ids"].as_array().unwrap().len(), 4);
        let reports = detail["reports"].as_array().unwrap();
        assert_eq!(reports.len(), 4);
        assert!(reports.iter().all(|report| report["available"] == true));
        for (report, slot) in reports.iter().zip(&execution.slots) {
            assert_eq!(report["native_execution_id"], slot.execution_id);
            assert_eq!(
                report["report"]["scenarios"][0]["scenario_id"],
                slot.scenario_id
            );
            assert_eq!(report["round"], slot.round);
        }
        assert_eq!(
            detail["plan_execution"]["slots"].as_array().unwrap().len(),
            4
        );
        assert!(detail["totals"]["scenario_pass_rate"].is_number());

        // Deleting the suite leaves the execution, and its name, as they ran.
        manager.delete_suite(&suite.id).await.unwrap();
        assert!(manager
            .suites()
            .await
            .unwrap()
            .iter()
            .all(|listed| listed.id != suite.id));
        assert_eq!(
            manager
                .read_execution(&id)
                .await
                .unwrap()
                .parameters
                .unwrap()
                .suite
                .unwrap()
                .label,
            "Edited PR"
        );
    }

    #[tokio::test]
    async fn suites_list_the_master_plan_then_this_console_and_copy_either() {
        let root = tempfile::tempdir().unwrap();
        let manager = manager(root.path(), Arc::new(FakeRunner::new(root.path().into())));
        let master = test_plan::embedded().unwrap();
        let listed = manager.suites().await.unwrap();
        assert_eq!(listed.len(), master.suites.len());
        for (view, suite) in listed.iter().zip(&master.suites) {
            let snapshot = master.materialize(&suite.id).unwrap();
            assert_eq!(view.id, suite.id);
            assert_eq!(view.source, "repository");
            assert_eq!(view.scenarios, snapshot.scenario_ids);
            assert_eq!(
                view.sha256.as_deref(),
                Some(snapshot.profile_sha256.as_str())
            );
        }
        let copy = manager
            .create_suite(SuiteCreateRequest {
                from: "software-engineering".into(),
                label: "Mine".into(),
            })
            .await
            .unwrap();
        let expected = &listed[1];
        assert_eq!(expected.id, "software-engineering");
        assert_eq!(copy.scenarios.len(), expected.scenarios.len());
        assert_eq!(
            (copy.repetitions, copy.technical_retries),
            (expected.repetitions, expected.technical_retries)
        );
        // Named apart from the master plan's, so its digest is its own.
        assert!(copy.sha256.is_some() && copy.sha256 != expected.sha256);
        let again = manager
            .create_suite(SuiteCreateRequest {
                from: copy.id.clone(),
                label: String::new(),
            })
            .await
            .unwrap();
        assert_eq!(again.label, "Mine copy");
        assert_eq!(again.scenarios, copy.scenarios);
        assert_eq!(
            manager.suites().await.unwrap().len(),
            master.suites.len() + 2
        );

        // A scenario of a sequential group brings the group.
        let grouped = manager
            .update_suite(SuiteUpdateRequest {
                suite_id: again.id.clone(),
                scenarios: Some(vec!["registry_verification".into()]),
                ..SuiteUpdateRequest::default()
            })
            .await
            .unwrap();
        assert_eq!(
            grouped.scenarios,
            vec!["registry_implementation", "registry_verification"]
        );
        // What a suite may not hold is refused and changes nothing.
        let error = manager
            .update_suite(SuiteUpdateRequest {
                suite_id: again.id.clone(),
                repetitions: Some(0),
                ..SuiteUpdateRequest::default()
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("between 1 and 20"), "{error}");
        // The master plan's suites are read-only and every suite is known by id.
        for (id, action, reason) in [
            (
                "pr",
                "update",
                "pr is a repository suite, read-only; copy it",
            ),
            (
                "pr",
                "delete",
                "pr is a repository suite, read-only; copy it",
            ),
            ("unknown", "create", "unknown suite"),
            ("suite-unknown", "update", "unknown suite"),
        ] {
            let error = match action {
                "update" => manager
                    .update_suite(SuiteUpdateRequest {
                        suite_id: id.into(),
                        label: Some("Renamed".into()),
                        ..SuiteUpdateRequest::default()
                    })
                    .await
                    .map(|_| ()),
                "delete" => manager.delete_suite(id).await,
                _ => manager
                    .create_suite(SuiteCreateRequest {
                        from: id.into(),
                        label: String::new(),
                    })
                    .await
                    .map(|_| ()),
            }
            .unwrap_err();
            assert!(error.to_string().contains(reason), "{error}");
        }
    }

    #[tokio::test]
    async fn an_execution_records_its_suite_named_as_reviewed_or_unnamed() {
        let root = tempfile::tempdir().unwrap();
        let manager = manager(root.path(), Arc::new(FakeRunner::new(root.path().into())));
        let reviewed = test_plan::embedded().unwrap().materialize("pr").unwrap();
        let start = |parameters: ExecutionParameters| {
            let manager = manager.clone();
            async move {
                let execution = manager.start_execution(parameters, "").await.unwrap();
                terminal(&manager, &execution.id).await
            }
        };

        // A suite of the master plan, as it is: the digest the workflow
        // records for it, under the master plan's name.
        let execution = start(suite_parameters("pr")).await;
        let parameters = execution.parameters.unwrap();
        assert_eq!(
            parameters.suite,
            Some(ExecutionSuite {
                id: Some("pr".into()),
                label: "PR".into(),
                sha256: reviewed.profile_sha256.clone(),
            })
        );
        assert_eq!(parameters.scenarios, reviewed.scenario_ids);
        assert_eq!(execution.slots[0].request["lane"], "local-pr");

        // Named with other scenarios: those run, under its name, and it says so.
        let mut changed = suite_parameters("pr");
        changed.scenarios.pop();
        let execution = start(changed).await;
        let suite = execution.parameters.unwrap().suite.unwrap();
        assert_eq!(suite.id.as_deref(), Some("pr"));
        assert_ne!(suite.sha256, reviewed.profile_sha256);
        assert!(execution.warnings[0].contains("suite pr holds other scenarios"));

        // Ticked by hand: unnamed, and the same scenarios in any order are
        // the same suite.
        let mut unnamed = parameters.clone();
        unnamed.suite = None;
        let one = start(unnamed.clone())
            .await
            .parameters
            .unwrap()
            .suite
            .unwrap();
        unnamed.scenarios.reverse();
        let two = start(unnamed).await.parameters.unwrap().suite.unwrap();
        assert_eq!((one.id, one.label.as_str()), (None, ""));
        assert!(!one.sha256.is_empty());
        assert_eq!(one.sha256, two.sha256);
        assert_ne!(one.sha256, reviewed.profile_sha256);
    }

    #[test]
    fn a_suite_an_older_import_stored_by_id_reads_as_that_suite() {
        let mut stored = serde_json::to_value(suite_parameters("pr")).unwrap();
        stored["suite"] = json!("pr");
        let read: ExecutionParameters = serde_json::from_value(stored.clone()).unwrap();
        assert_eq!(
            read.suite,
            Some(ExecutionSuite {
                id: Some("pr".into()),
                label: "pr".into(),
                sha256: String::new(),
            })
        );
        assert_eq!(read.scenarios, suite_parameters("pr").scenarios);
        // Absent is no suite; any other shape is not a suite this runner reads.
        stored.as_object_mut().unwrap().remove("suite");
        let read: ExecutionParameters = serde_json::from_value(stored.clone()).unwrap();
        assert_eq!(read.suite, None);
        stored["suite"] = json!(42);
        assert!(serde_json::from_value::<ExecutionParameters>(stored).is_err());
    }

    /// A campaign bundle as the exact-stack workflow uploads it: one group
    /// with its native run, one that left only `failure.json`.
    fn exact_stack_bundle(root: &Path, key: &str, observed: &str) -> (PathBuf, PathBuf, String) {
        let campaign = root.join("bundle/smoke-r01");
        let group = campaign.join("groups/case-context");
        fs::create_dir_all(group.join("stack")).unwrap();
        let request: RunRequest = serde_json::from_value(json!({
            "idempotency_key": key, "label": "Smoke · case-context", "lane": "local",
            "model": "model", "provider": "provider", "agent": "tech-lead",
            "scenarios": ["context_pressure"], "runs": 1, "technical_retries": 0,
        }))
        .unwrap();
        let native = FakeRunner::new(group.join("native"))
            .native_record(request.clone())
            .unwrap();
        fs::write(
            group.join("run-request.json"),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        fs::write(
            group.join("stack/worker-compose.lock"),
            "containers:\n  harness:\n    worker: package://harness\n    requested: latest\n    resolved:\n      version: 1.8.31\n",
        )
        .unwrap();
        fs::write(
            group.join("stack/workers.json"),
            json!({"workers": [
                {"name": "harness", "version": observed, "runtime": "rust"},
                {"name": "configuration", "version": "0.24.2", "runtime": "engine"},
            ]})
            .to_string(),
        )
        .unwrap();
        let failed = campaign.join("groups/case-registry");
        fs::create_dir_all(&failed).unwrap();
        fs::write(
            failed.join("failure.json"),
            json!({"phase": "workflow_artifact_download", "error": "group observation artifact was not available"})
                .to_string(),
        )
        .unwrap();
        fs::write(
            campaign.join("stack-lock.json"),
            json!({"suite": {"groups": [
                {"id": "case-context", "scenarios": ["context_pressure"]},
                {"id": "case-registry", "scenarios": ["registry_planning"]},
            ]}})
            .to_string(),
        )
        .unwrap();
        let contract = root.join("contract");
        fs::create_dir_all(&contract).unwrap();
        fs::write(
            contract.join("plan.json"),
            json!({"profile": {"id": "smoke"}, "subject": {"provider": "provider", "model": "model"}, "agent_profile": "tech-lead"})
                .to_string(),
        )
        .unwrap();
        fs::write(
            contract.join("profile.json"),
            json!({"profile": {"label": "Smoke"}, "profile_sha256": "sha256:smoke"}).to_string(),
        )
        .unwrap();
        (root.join("bundle"), contract, native.execution_id)
    }

    #[tokio::test]
    async fn github_bundle_becomes_one_execution_whose_runs_a_new_attempt_replaces() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let runner = Arc::new(FakeRunner::new(data.clone()));
        let manager = manager(&data, runner.clone());
        let id = github::import_id("iii-hq/harness-e2e", 42);
        assert_eq!(id, github::import_id("iii-hq/harness-e2e", 42));
        assert_ne!(id, github::import_id("iii-hq/other", 42));
        let mut execution = PlanExecution {
            id: id.clone(),
            idempotency_key: "github:iii-hq/harness-e2e#42".into(),
            label: None,
            parameters: None,
            source: ExecutionSource::Github {
                repository: "iii-hq/harness-e2e".into(),
                run_id: 42,
                run_attempt: 1,
                url: "https://github.com/iii-hq/harness-e2e/actions/runs/42".into(),
                release_control_execution_id: Some("rc-execution".into()),
                stack: None,
            },
            stack: Vec::new(),
            warnings: Vec::new(),
            state: "importing".into(),
            started_at: "2026-09-20T10:00:00Z".into(),
            updated_at: now(),
            finished_at: Some("2026-09-20T11:00:00Z".into()),
            cancel_requested: false,
            error: None,
            slots: Vec::new(),
            measurements: None,
            system_under_test: None,
            rerun: None,
        };
        manager.write_execution(&execution).await.unwrap();

        let (bundle, contract, first) =
            exact_stack_bundle(&root.path().join("attempt-1"), "rc:e2e:first", "1.8.8");
        manager
            .install_bundle(&mut execution, &bundle, &contract)
            .await
            .unwrap();
        let installed = manager.read_execution(&id).await.unwrap();
        assert_eq!(installed.state, "completed");
        assert_eq!(
            installed.finished_at.as_deref(),
            Some("2026-09-20T11:00:00Z")
        );
        assert_eq!(installed.label.as_deref(), Some("Smoke"));
        assert_eq!(
            installed.parameters,
            Some(ExecutionParameters {
                suite: Some(ExecutionSuite {
                    id: Some("smoke".into()),
                    label: "Smoke".into(),
                    sha256: "sha256:smoke".into(),
                }),
                scenarios: vec!["context_pressure".into(), "registry_planning".into()],
                runs: 1,
                technical_retries: 0,
                model: "model".into(),
                provider: "provider".into(),
                agent: Some("tech-lead".into()),
            })
        );
        assert_eq!(
            installed.stack,
            vec![StackWorker {
                name: "harness".into(),
                source: WorkerSource::Package,
                requested: Some("1.8.31".into()),
                observed: Some("1.8.8".into()),
                commit: None,
                dirty: None,
                groups: Vec::new(),
            }]
        );
        let [run, failure] = installed.slots.as_slice() else {
            panic!("one slot per group scenario: {:?}", installed.slots);
        };
        assert_eq!(
            (run.execution_id.as_str(), run.state.as_str()),
            (first.as_str(), "finished")
        );
        assert_eq!(
            (run.observed, run.completed, run.error.as_deref()),
            (1, 1, None)
        );
        assert_eq!(run.request["idempotency_key"], "rc:e2e:first");
        assert!(failure.execution_id.is_empty());
        assert_eq!(failure.state, "not_run");
        assert_eq!(
            failure.error.as_deref(),
            Some("group observation artifact was not available")
        );
        // The native run is an ordinary retained run: installed through the
        // runner and read back from the data directory.
        assert!(runner.record(&first).await.is_some());
        assert!(data.join(&first).join("results.json").is_file());
        assert!(installed.measurements.is_some());

        // Importing the run again takes its highest attempt: the runs the first
        // import installed are replaced, the name is kept and nothing doubles.
        let mut execution = installed;
        manager.rename(&id, "Mine").await.unwrap();
        let (bundle, contract, second) =
            exact_stack_bundle(&root.path().join("attempt-2"), "rc:e2e:second", "1.8.9");
        manager
            .install_bundle(&mut execution, &bundle, &contract)
            .await
            .unwrap();
        let replaced = manager.read_execution(&id).await.unwrap();
        assert_eq!(replaced.slots[0].execution_id, second);
        assert_eq!(replaced.label.as_deref(), Some("Mine"));
        assert_eq!(replaced.stack[0].observed.as_deref(), Some("1.8.9"));
        assert!(runner.record(&first).await.is_none());
        assert!(!data.join(&first).exists());
        assert!(runner.record(&second).await.is_some());
        assert_eq!(manager.executions().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_bundle_that_holds_only_a_failure_fails_the_import() {
        let root = tempfile::tempdir().unwrap();
        let manager = manager(root.path(), Arc::new(FakeRunner::new(root.path().into())));
        let bundle = root.path().join("bundle");
        fs::create_dir_all(&bundle).unwrap();
        fs::write(
            bundle.join("failure.json"),
            json!({"phase": "artifact_packaging", "error": "Root artifact validation failed"})
                .to_string(),
        )
        .unwrap();
        let mut execution = import_stub();
        let error = manager
            .install_bundle(&mut execution, &bundle, &root.path().join("contract"))
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("Root artifact validation failed"));
    }

    #[tokio::test]
    async fn a_run_being_imported_is_not_imported_twice_and_a_failed_download_fails_it() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        // Every call answers with the run; its artifact list then holds no
        // contract, so the download fails after the import began.
        let gh = fake_gh(
            root.path(),
            r#"printf '%s' '{"id":42,"run_attempt":1,"display_title":"E2E · rc-1","html_url":"https://github.com/o/r/actions/runs/42","run_started_at":"2026-09-20T10:00:00Z"}'"#,
        );
        let manager = manager_with_gh(&data, Arc::new(FakeRunner::new(data.clone())), gh);
        let (first, started) = manager.begin_github_import("o/r", 42).await.unwrap();
        assert!(started);
        assert_eq!(first.state, "importing");
        assert_eq!(first.id, github::import_id("o/r", 42));
        let (second, started) = manager.begin_github_import("o/r", 42).await.unwrap();
        assert!(!started);
        assert_eq!(second.id, first.id);

        manager.finish_github_import(&first.id).await.unwrap();
        let failed = manager.read_execution(&first.id).await.unwrap();
        assert_eq!(failed.state, "failed");
        assert!(failed
            .error
            .as_deref()
            .unwrap()
            .contains("no e2e-contract artifact"));
        // A failed import can be started again.
        assert!(manager.begin_github_import("o/r", 42).await.unwrap().1);
    }

    #[tokio::test]
    async fn gh_runs_with_its_temporary_files_in_the_data_directory_and_a_deadline() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let runner = Arc::new(FakeRunner::new(data.clone()));
        let failing = manager_with_gh(
            &data,
            runner.clone(),
            fake_gh(
                root.path(),
                r#"echo "HTTP 404 with TMPDIR=$TMPDIR" >&2; exit 1"#,
            ),
        );
        let error = failing
            .begin_github_import("o/r", 42)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("HTTP 404"), "{error}");
        assert!(error.contains(&format!("TMPDIR={}", data.join(".imports").display())));
        assert!(error.contains("gh auth login"));

        let mut execution = import_stub();
        execution.source = ExecutionSource::Github {
            repository: "o/r".into(),
            run_id: 42,
            run_attempt: 1,
            url: String::new(),
            release_control_execution_id: None,
            stack: None,
        };
        let slow = manager_with_gh(&data, runner, fake_gh(root.path(), "sleep 5"));
        slow.write_execution(&execution).await.unwrap();
        slow.finish_github_import(&execution.id).await.unwrap();
        let failed = slow.read_execution(&execution.id).await.unwrap();
        assert_eq!(failed.state, "failed");
        assert!(failed
            .error
            .as_deref()
            .unwrap()
            .contains("did not finish within 500ms"));
    }

    #[tokio::test]
    async fn restart_fails_an_interrupted_import_and_clears_its_files() {
        let root = tempfile::tempdir().unwrap();
        let manager = manager(root.path(), Arc::new(FakeRunner::new(root.path().into())));
        let execution = import_stub();
        manager.write_execution(&execution).await.unwrap();
        let leftover = root.path().join(".imports/github-left/bundle");
        fs::create_dir_all(&leftover).unwrap();
        fs::write(leftover.join("results.json"), b"{}").unwrap();
        manager.reconcile().await.unwrap();
        let failed = manager.read_execution(&execution.id).await.unwrap();
        assert_eq!(failed.state, "failed");
        assert!(failed.error.as_deref().unwrap().contains("Import it again"));
        assert!(!root.path().join(".imports").exists());
    }

    #[tokio::test]
    async fn a_rename_during_the_import_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let manager = manager(&data, Arc::new(FakeRunner::new(data.clone())));
        let mut snapshot = import_stub();
        manager.write_execution(&snapshot).await.unwrap();
        manager.rename(&snapshot.id, "  Mine  ").await.unwrap();
        let (bundle, contract, _) =
            exact_stack_bundle(&root.path().join("bundle"), "rc:e2e:renamed", "1.8.8");
        manager
            .install_bundle(&mut snapshot, &bundle, &contract)
            .await
            .unwrap();
        let installed = manager.read_execution(&snapshot.id).await.unwrap();
        assert_eq!(installed.label.as_deref(), Some("Mine"));
        assert_eq!(installed.state, "completed");
        manager.rename(&snapshot.id, "").await.unwrap();
        assert!(manager
            .read_execution(&snapshot.id)
            .await
            .unwrap()
            .label
            .is_none());
    }

    #[tokio::test]
    async fn a_new_attempt_without_a_readable_group_keeps_the_previous_import() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let runner = Arc::new(FakeRunner::new(data.clone()));
        let manager = manager(&data, runner.clone());
        let mut execution = import_stub();
        manager.write_execution(&execution).await.unwrap();
        let (bundle, contract, first) =
            exact_stack_bundle(&root.path().join("attempt-1"), "rc:e2e:first", "1.8.8");
        manager
            .install_bundle(&mut execution, &bundle, &contract)
            .await
            .unwrap();
        let before = manager.read_execution(&execution.id).await.unwrap();

        let failed = root
            .path()
            .join("attempt-2/bundle/smoke-r01/groups/case-context");
        fs::create_dir_all(&failed).unwrap();
        fs::write(
            failed.join("failure.json"),
            json!({"error": "group observation artifact was not available"}).to_string(),
        )
        .unwrap();
        let error = manager
            .install_bundle(
                &mut execution.clone(),
                &root.path().join("attempt-2/bundle"),
                &contract,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("No group of this run"), "{error}");
        let after = manager.read_execution(&execution.id).await.unwrap();
        assert_eq!(
            serde_json::to_value(&after.slots).unwrap(),
            serde_json::to_value(&before.slots).unwrap()
        );
        assert!(runner.record(&first).await.is_some());
        assert!(data.join(&first).join("results.json").is_file());
    }

    #[tokio::test]
    async fn native_runs_need_an_execution_id_and_never_replace_another_executions_evidence() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let runner = Arc::new(FakeRunner::new(data.clone()));
        let manager = manager(&data, runner.clone());
        let mut execution = import_stub();
        manager.write_execution(&execution).await.unwrap();

        let (bundle, contract, id) =
            exact_stack_bundle(&root.path().join("renamed"), "rc:e2e:renamed", "1.8.8");
        let natives = bundle.join("smoke-r01/groups/case-context/native");
        fs::rename(natives.join(&id), natives.join("not-an-id")).unwrap();
        let error = manager
            .install_bundle(&mut execution, &bundle, &contract)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("not an E2E execution id"), "{error}");

        let (bundle, contract, id) =
            exact_stack_bundle(&root.path().join("taken"), "rc:e2e:taken", "1.8.8");
        fs::create_dir_all(data.join(&id)).unwrap();
        fs::write(data.join(&id).join("marker"), b"another execution").unwrap();
        let error = manager
            .install_bundle(&mut execution, &bundle, &contract)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("already retained by another execution"),
            "{error}"
        );
        assert_eq!(
            fs::read(data.join(&id).join("marker")).unwrap(),
            b"another execution"
        );
        assert!(runner.record(&id).await.is_none());
    }

    fn import_stub() -> PlanExecution {
        PlanExecution {
            id: "plan-stub".into(),
            idempotency_key: "github:owner/repo#1".into(),
            label: None,
            parameters: None,
            source: ExecutionSource::Local,
            stack: Vec::new(),
            warnings: Vec::new(),
            state: "importing".into(),
            started_at: now(),
            updated_at: now(),
            finished_at: None,
            cancel_requested: false,
            error: None,
            slots: Vec::new(),
            measurements: None,
            system_under_test: None,
            rerun: None,
        }
    }

    #[tokio::test]
    async fn dashboard_summaries_skip_bad_receipts_without_hiding_valid_children() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner);
        let execution = PlanExecution {
            id: "plan-good".into(),
            idempotency_key: "good".into(),
            label: None,
            parameters: None,
            source: ExecutionSource::Local,
            stack: Vec::new(),
            warnings: Vec::new(),
            state: "running".into(),
            started_at: "2026-09-11T00:00:00Z".into(),
            updated_at: "2026-09-11T00:00:00Z".into(),
            finished_at: None,
            cancel_requested: false,
            error: None,
            slots: vec![
                Slot {
                    round: 1,
                    group_id: "group".into(),
                    scenario_id: "direct_answer".into(),
                    execution_id: "native-good".into(),
                    request: json!({}),
                    state: "pending".into(),
                    result_path: None,
                    error: None,
                    observed: 0,
                    completed: 0,
                    passed: 0,
                    technical_valid: 0,
                    eligible: false,
                    previous_attempts: Vec::new(),
                },
                // A group that failed before it had a native run.
                Slot {
                    round: 1,
                    group_id: "failed-group".into(),
                    scenario_id: "context_pressure".into(),
                    execution_id: String::new(),
                    request: Value::Null,
                    state: "not_run".into(),
                    result_path: None,
                    error: Some("group observation artifact was not available".into()),
                    observed: 0,
                    completed: 0,
                    passed: 0,
                    technical_valid: 0,
                    eligible: false,
                    previous_attempts: Vec::new(),
                },
            ],
            measurements: Some(json!({"cohorts": [{
                "scenario_id": "direct_answer", "identity": {},
                "aggregate": {"observed_runs": 1, "planned_runs": 1, "completed_runs": 1,
                    "total_tokens_consumed": 20, "cost": {"total_usd": 0.1}},
            }]})),
            system_under_test: None,
            rerun: None,
        };
        manager.write_execution(&execution).await.unwrap();
        fs::write(
            root.path().join("plan-store/executions/corrupt.json"),
            b"not json",
        )
        .unwrap();

        let (summaries, children) = manager
            .dashboard_summaries(&[json!({
                "id": "native-good", "totals": {"wall_time_seconds": 3.5, "function_calls": 2},
            })])
            .await
            .unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0]["id"], "plan-good");
        assert_eq!(summaries[0]["totals"]["wall_time_seconds"], 3.5);
        assert_eq!(summaries[0]["totals"]["function_calls"], 2.0);
        assert!(
            manager.dashboard_summaries(&[]).await.unwrap().0[0]["totals"]["wall_time_seconds"]
                .is_null()
        );
        assert_eq!(
            children.get("native-good").map(String::as_str),
            Some("plan-good")
        );
        assert!(!children.contains_key(""));
    }

    #[tokio::test]
    async fn native_coordination_covers_every_suite_of_the_master_plan() {
        for (suite, expected_slots, expected_submissions) in [
            ("regression", 9, 9),
            ("software-engineering", 15, 14),
            ("pr", 4, 4),
            ("after-release", 5, 5),
        ] {
            let root = tempfile::tempdir().unwrap();
            let runner = Arc::new(FakeRunner::new(root.path().into()));
            let manager = manager(root.path(), runner.clone());
            runner.fail_next.store(true, Ordering::SeqCst);
            let id = started(&manager, suite).await;
            let execution = terminal(&manager, &id).await;
            assert_eq!(
                execution.state, "completed",
                "{suite}: {:?}",
                execution.error
            );
            assert_eq!(execution.slots.len(), expected_slots);
            assert_eq!(
                runner.submitted.load(Ordering::SeqCst),
                expected_submissions
            );
            // Objective failure is independent from validity.
            assert!(execution.slots.iter().all(|slot| slot.eligible));
            assert_eq!(
                execution.slots.iter().map(|s| s.passed).sum::<u32>(),
                expected_slots as u32
            );
            let cohorts = execution.measurements.as_ref().unwrap()["cohorts"]
                .as_array()
                .unwrap()
                .clone();
            assert_eq!(
                cohorts.len(),
                execution.parameters.as_ref().unwrap().scenarios.len()
            );
        }
    }
    #[tokio::test]
    async fn grouped_registry_cases_share_one_child_without_duplicate_measurements() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let id = manager
            .start_execution(
                parameters(&[
                    "registry_planning",
                    "registry_implementation",
                    "registry_environment",
                    "registry_verification",
                ]),
                "",
            )
            .await
            .unwrap()
            .id;
        let execution = terminal(&manager, &id).await;

        assert_eq!(execution.slots.len(), 4);
        assert_eq!(runner.submitted.load(Ordering::SeqCst), 3);
        assert_eq!(result_paths(&execution, root.path()).len(), 3);
        let grouped = execution
            .slots
            .iter()
            .filter(|slot| slot.group_id == "case-registry-implementation")
            .collect::<Vec<_>>();
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].execution_id, grouped[1].execution_id);
        assert_eq!(
            grouped[0].request["scenarios"],
            json!(["registry_implementation", "registry_verification"])
        );
        assert!(grouped[0].request["seed"].is_null());

        let native_summaries = crate::dashboard::read_model::DashboardReadModel::load(root.path())
            .unwrap()
            .summaries;
        let detail = manager
            .execution_detail(&id, &native_summaries)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail["native_execution_ids"].as_array().unwrap().len(), 3);
        assert_eq!(detail["reports"].as_array().unwrap().len(), 4);
        assert_eq!(detail["scenario_metrics"].as_array().unwrap().len(), 4);
        assert_eq!(detail["totals"]["wall_time_seconds"], json!(0.4));
        assert_eq!(detail["totals"]["total_cost_usd"], json!(1.0));
        assert!(execution.slots.iter().all(|slot| slot.eligible));

        // Every scenario ran its canonical case.
        for slot in &execution.slots {
            let report =
                E2eReport::read_from(&root.path().join(slot.result_path.as_ref().unwrap()))
                    .unwrap()
                    .0;
            let scenario = report
                .scenarios
                .iter()
                .find(|scenario| scenario.scenario_id == slot.scenario_id)
                .unwrap();
            assert_eq!(
                scenario.case.as_ref().unwrap().seed,
                slot.scenario_id
                    .parse::<crate::scenarios::ScenarioId>()
                    .unwrap()
                    .canonical_seed()
            );
        }
    }
    #[tokio::test]
    async fn cancellation_reserves_admission_and_prevents_next_groups() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        runner.hold.store(true, Ordering::SeqCst);
        let manager = manager(root.path(), runner.clone());
        let id = started(&manager, "pr").await;
        let other = manager
            .start_execution(suite_parameters("pr"), "")
            .await
            .unwrap_err();
        assert!(other.to_string().contains("is still running"), "{other}");
        tokio::time::timeout(Duration::from_secs(5), async {
            while runner.submitted.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        manager.cancel(&id).await.unwrap();
        let execution = terminal(&manager, &id).await;
        assert_eq!(execution.state, "cancelled");
        assert_eq!(runner.submitted.load(Ordering::SeqCst), 1);
        assert!(execution.slots[1..].iter().all(|s| s.state == "not_run"));
        assert!(runner.owner.lock().await.is_none());
    }
    #[tokio::test]
    async fn missing_artifacts_and_identity_divergence_interrupt_the_execution() {
        for wrong_identity in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let runner = Arc::new(FakeRunner::new(root.path().into()));
            runner
                .wrong_identity
                .store(wrong_identity, Ordering::SeqCst);
            runner
                .lose_artifact
                .store(!wrong_identity, Ordering::SeqCst);
            let manager = manager(root.path(), runner.clone());
            let id = started(&manager, "software-engineering").await;
            let execution = terminal(&manager, &id).await;
            assert_eq!(execution.state, "interrupted");
            assert_eq!(runner.submitted.load(Ordering::SeqCst), 1);
            let detail = manager.execution_detail(&id, &[]).await.unwrap().unwrap();
            let reports = detail["reports"].as_array().unwrap();
            assert_eq!(reports.len(), 15);
            // Reconciliation retains evidence from the persisted child even
            // when admission returned a different identity; remaining slots stay explicit.
            assert_eq!(reports[0]["available"], wrong_identity);
            assert!(reports[1..]
                .iter()
                .all(|report| report["available"] == false));
        }
    }
    #[tokio::test]
    async fn persistence_failure_never_dispatches_a_child() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        runner.fail_receipt.store(true, Ordering::SeqCst);
        assert!(manager
            .start_execution(suite_parameters("pr"), "")
            .await
            .is_err());
        assert_eq!(runner.submitted.load(Ordering::SeqCst), 0);
        assert!(runner.owner.lock().await.is_none());
    }
    #[tokio::test]
    async fn restart_reconciles_persisted_children_and_never_resumes() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let id = started(&manager, "pr").await;
        let mut receipt = terminal(&manager, &id).await;
        receipt.state = "running".into();
        receipt.slots[2..].iter_mut().for_each(|s| {
            s.state = "pending".into();
            s.result_path = None;
            s.observed = 0;
        });
        for slot in &receipt.slots[2..] {
            runner.records.lock().await.remove(&slot.execution_id);
        }
        manager.write_execution(&receipt).await.unwrap();
        let before = runner.submitted.load(Ordering::SeqCst);
        manager.reconcile().await.unwrap();
        let recovered = manager.read_execution(&id).await.unwrap();
        assert_eq!(recovered.state, "interrupted");
        assert_eq!(recovered.slots[0].state, "finished");
        assert_eq!(recovered.slots[2].state, "not_run");
        assert_eq!(runner.submitted.load(Ordering::SeqCst), before);
    }

    #[tokio::test]
    async fn an_execution_starts_from_parameters_and_records_the_stack() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        runner.crash_next.store(true, Ordering::SeqCst);
        let parameters = ExecutionParameters {
            suite: None,
            scenarios: vec![
                "context_pressure".into(),
                "retired_scenario".into(),
                "minimal_path".into(),
                "context_pressure".into(),
            ],
            runs: 1,
            technical_retries: 0,
            model: " model ".into(),
            provider: "provider".into(),
            agent: Some("tech-lead".into()),
        };
        let started = manager
            .start_execution(parameters, "  Again  ")
            .await
            .unwrap();
        assert_eq!(started.state, "running");
        let execution = terminal(&manager, &started.id).await;

        assert_eq!(execution.source, ExecutionSource::Local);
        assert_eq!(execution.label.as_deref(), Some("Again"));
        let parameters = execution.parameters.as_ref().unwrap();
        assert_eq!(parameters.model, "model");
        assert_eq!(
            parameters.scenarios,
            vec!["context_pressure", "minimal_path", "retired_scenario"]
        );
        // Ticked by hand: an unnamed suite, with the digest of what it holds.
        let suite = parameters.suite.as_ref().unwrap();
        assert_eq!(suite.id, None);
        assert!(suite.sha256.starts_with("sha256:"));
        // The stack is recorded before the first slot, with what could not be.
        assert_eq!(execution.stack[0].name, "queue");
        assert_eq!(execution.stack[0].dirty, Some(true));
        assert_eq!(
            execution.warnings,
            vec!["Worker sources were not recorded: compose is unavailable"]
        );
        // The unknown scenario and the run that failed without results fail
        // only their slots; the other scenario still runs.
        assert_eq!(runner.submitted.load(Ordering::SeqCst), 2);
        assert_eq!(execution.state, "completed", "{:?}", execution.error);
        let slot = |id: &str| {
            execution
                .slots
                .iter()
                .find(|slot| slot.scenario_id == id)
                .unwrap()
        };
        let crashed = slot("context_pressure");
        assert_eq!(
            crashed.error.as_deref(),
            Some("fixture repository unavailable")
        );
        assert_eq!(crashed.observed, 0);
        let unknown = slot("retired_scenario");
        assert!(unknown.execution_id.is_empty());
        assert_eq!(unknown.state, "not_run");
        assert!(unknown
            .error
            .as_deref()
            .unwrap()
            .contains("does not know the scenario 'retired_scenario'"));
        let ran = slot("minimal_path");
        assert_eq!((ran.state.as_str(), ran.observed), ("finished", 1));
        assert_eq!(ran.request["agent"], "tech-lead");
        assert!(ran.request["seed"].is_null());
        assert!(execution.measurements.is_some());
        assert!(runner.owner.lock().await.is_none());

        // Nothing without a model or a scenario, or with a model, provider or
        // agent the old run form refused, starts.
        let long = "m".repeat(201);
        for (model, provider, agent, scenarios, reason) in [
            (
                "",
                "provider",
                None,
                vec!["minimal_path".into()],
                "Select an execution model",
            ),
            (
                "model",
                "provider",
                None,
                vec![],
                "Select between 1 and 256 scenarios",
            ),
            (
                long.as_str(),
                "provider",
                None,
                vec!["minimal_path".into()],
                "model must be at most 200",
            ),
            (
                "model",
                "pro\u{7}vider",
                None,
                vec!["minimal_path".into()],
                "provider must be at most 200",
            ),
            (
                "model",
                "provider",
                Some("tech\nlead"),
                vec!["minimal_path".into()],
                "agent must be at most 200",
            ),
        ] {
            let parameters = ExecutionParameters {
                suite: None,
                scenarios,
                runs: 1,
                technical_retries: 0,
                model: model.into(),
                provider: provider.into(),
                agent: agent.map(str::to_owned),
            };
            let error = manager
                .start_execution(parameters, "")
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(reason), "{error}");
        }
    }

    #[test]
    fn parameters_keep_the_sequential_groups_of_the_master_plan() {
        let mut parameters = ExecutionParameters {
            suite: None,
            scenarios: vec![
                "registry_implementation".into(),
                "registry_verification".into(),
                "minimal_path".into(),
            ],
            runs: 2,
            technical_retries: 1,
            model: "model".into(),
            provider: "provider".into(),
            agent: None,
        };
        let slots = parameter_slots(
            &test_plan::embedded().unwrap(),
            &mut parameters,
            "plan-grouped",
            None,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(slots.len(), 6);
        for round in [1, 2] {
            let ids = slots
                .iter()
                .filter(|slot| slot.round == round)
                .map(|slot| slot.execution_id.as_str())
                .collect::<BTreeSet<_>>();
            assert_eq!(ids.len(), 2, "one run for the group, one for minimal_path");
        }
        // The canonical cases, so every execution pairs with any other.
        assert!(slots.iter().all(|slot| slot.request["seed"].is_null()));
    }

    #[tokio::test]
    async fn a_busy_runner_names_the_execution_that_holds_it() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        runner.hold.store(true, Ordering::SeqCst);
        let manager = manager(root.path(), runner.clone());
        let parameters = ExecutionParameters {
            suite: None,
            scenarios: vec!["minimal_path".into()],
            runs: 1,
            technical_retries: 0,
            model: "model".into(),
            provider: "provider".into(),
            agent: None,
        };
        let running = manager
            .start_execution(parameters.clone(), "Nightly")
            .await
            .unwrap();
        let error = manager.start_execution(parameters, "").await.unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "\"Nightly\" ({}) is still running; wait for it to finish or cancel it.",
                running.id
            )
        );
    }

    #[tokio::test]
    async fn a_scenario_of_a_sequential_group_brings_the_whole_group() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let parameters = ExecutionParameters {
            suite: None,
            scenarios: vec!["registry_verification".into(), "minimal_path".into()],
            runs: 1,
            technical_retries: 0,
            model: "model".into(),
            provider: "provider".into(),
            agent: None,
        };
        let started = manager.start_execution(parameters, "").await.unwrap();
        assert_eq!(
            started.parameters.as_ref().unwrap().scenarios,
            vec![
                "minimal_path",
                "registry_implementation",
                "registry_verification"
            ]
        );
        assert_eq!(
            started.warnings,
            vec![
                "registry_implementation then registry_verification run only together, in this order; the whole group was added."
            ]
        );
        let grouped = started
            .slots
            .iter()
            .filter(|slot| slot.scenario_id.starts_with("registry_"))
            .map(|slot| slot.execution_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(grouped.len(), 1, "one session for the group");
        let execution = terminal(&manager, &started.id).await;
        assert_eq!(runner.submitted.load(Ordering::SeqCst), 2);
        // The note stays next to the stack warnings.
        assert_eq!(execution.warnings[0], started.warnings[0]);
    }

    #[tokio::test]
    async fn a_completed_execution_names_the_slot_that_did_not_run_cleanly() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        runner.crash_next.store(true, Ordering::SeqCst);
        let id = started(&manager, "pr").await;
        let execution = terminal(&manager, &id).await;
        // No safety stop: every slot ran, and the reason is the execution's.
        assert_eq!(execution.state, "completed");
        assert_eq!(runner.submitted.load(Ordering::SeqCst), 4);
        let first = &execution.slots[0];
        assert_eq!(
            execution.error,
            Some(format!(
                "{}: fixture repository unavailable",
                first.scenario_id
            ))
        );
        let (summaries, _) = manager.dashboard_summaries(&[]).await.unwrap();
        let summary = summaries
            .iter()
            .find(|summary| summary["id"] == id)
            .unwrap();
        assert_eq!(
            summary["first_failure"]["message"],
            json!(execution.error.as_deref().unwrap())
        );
    }

    #[tokio::test]
    async fn a_finished_execution_is_deleted_with_its_runs() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let parameters = ExecutionParameters {
            suite: None,
            scenarios: vec!["minimal_path".into(), "retired_scenario".into()],
            runs: 1,
            technical_retries: 0,
            model: "model".into(),
            provider: "provider".into(),
            agent: None,
        };
        let started = manager.start_execution(parameters, "").await.unwrap();
        let execution = terminal(&manager, &started.id).await;
        let child = execution.slots[0].execution_id.clone();
        assert!(root.path().join(&child).join("results.json").is_file());

        manager.delete_execution(&execution.id).await.unwrap();
        assert!(manager.read_execution(&execution.id).await.is_err());
        assert!(runner.record(&child).await.is_none());
        assert!(!root.path().join(&child).exists());
    }

    fn parameters(scenarios: &[&str]) -> ExecutionParameters {
        ExecutionParameters {
            scenarios: scenarios.iter().map(|id| (*id).into()).collect(),
            runs: 1,
            technical_retries: 0,
            model: "model".into(),
            provider: "provider".into(),
            agent: None,
            suite: None,
        }
    }

    #[tokio::test]
    async fn a_scenario_runs_again_and_only_its_last_attempt_counts() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        // The first attempt has results: it fails its task.
        runner.fail_next.store(true, Ordering::SeqCst);
        let started = manager
            .start_execution(parameters(&["minimal_path", "persistent_state"]), "")
            .await
            .unwrap();
        let first = terminal(&manager, &started.id).await;
        assert_eq!(first.slots[0].scenario_id, "minimal_path");
        let (failed, other) = (first.slots[0].clone(), first.slots[1].clone());

        // Asking changes no slot: a run is replaced when its new one is admitted.
        runner.crash_next.store(true, Ordering::SeqCst);
        let rerun = manager
            .rerun_scenario(&first.id, "minimal_path")
            .await
            .unwrap();
        assert_eq!(rerun.state, "running");
        assert_eq!(rerun.slots[0].execution_id, failed.execution_id);
        assert_eq!(rerun.rerun.as_ref().unwrap().state, "completed");
        assert!(rerun.measurements.is_some());
        // The second ends without results: it still replaces the first.
        let second = terminal(&manager, &first.id).await;
        assert_eq!(second.state, "completed");
        assert!(second.rerun.is_none());
        let slot = &second.slots[0];
        assert_ne!(slot.execution_id, failed.execution_id);
        assert!(slot.request["idempotency_key"]
            .as_str()
            .unwrap()
            .ends_with(":attempt-2"));
        assert_eq!(
            slot.previous_attempts,
            vec![SlotAttempt {
                execution_id: failed.execution_id.clone(),
                error: None,
            }]
        );
        assert_eq!(
            slot.error.as_deref(),
            Some("fixture repository unavailable")
        );
        // The other scenario kept its run, and the measurements hold only
        // the current attempts: the failed one is out.
        assert_eq!(
            serde_json::to_value(&second.slots[1]).unwrap(),
            serde_json::to_value(&other).unwrap()
        );
        assert_eq!(
            second.measurements.as_ref().unwrap()["cohorts"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let revision = manager.attempts_revision();
        manager
            .rerun_scenario(&first.id, "minimal_path")
            .await
            .unwrap();
        let third = terminal(&manager, &first.id).await;
        assert!(manager.attempts_revision() > revision);
        assert_eq!(runner.submitted.load(Ordering::SeqCst), 4);
        let slot = &third.slots[0];
        assert_eq!((slot.observed, slot.error.as_deref()), (1, None));
        assert_eq!(
            slot.previous_attempts
                .iter()
                .map(|attempt| (attempt.execution_id.as_str(), attempt.error.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                (failed.execution_id.as_str(), None),
                (
                    second.slots[0].execution_id.as_str(),
                    Some("fixture repository unavailable")
                ),
            ]
        );
        // What the test history and pass rate leave out.
        assert_eq!(
            manager.previous_attempts().await.unwrap(),
            BTreeSet::from([
                failed.execution_id.clone(),
                second.slots[0].execution_id.clone()
            ])
        );

        // Totals, assessment and runtime count the current attempts only;
        // the previous ones are listed apart, with their reason.
        let natives = crate::dashboard::read_model::DashboardReadModel::load(root.path())
            .unwrap()
            .summaries;
        let detail = manager
            .execution_detail(&third.id, &natives)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail["reports"].as_array().unwrap().len(), 2);
        assert_eq!(detail["assessment_summary"]["run_count"], 2);
        assert_eq!(detail["totals"]["total_cost_usd"], json!(0.5));
        assert_eq!(detail["totals"]["wall_time_seconds"], json!(0.2));
        let previous = detail["previous_reports"].as_array().unwrap();
        assert_eq!(previous.len(), 2);
        assert_eq!(
            previous[0]["native_execution_id"],
            json!(failed.execution_id)
        );
        assert_eq!(previous[0]["available"], true);
        assert_eq!(previous[1]["available"], false);
        assert_eq!(previous[1]["error"], "fixture repository unavailable");
        // Previous attempts are the execution's, never runs of their own.
        let (_, children) = manager.dashboard_summaries(&natives).await.unwrap();
        assert_eq!(children.get(&failed.execution_id), Some(&third.id));

        // Deleting the execution deletes every attempt.
        manager.delete_execution(&third.id).await.unwrap();
        for attempt in &third.slots[0].previous_attempts {
            assert!(runner.record(&attempt.execution_id).await.is_none());
            assert!(!root.path().join(&attempt.execution_id).exists());
        }
    }

    #[tokio::test]
    async fn a_scenario_of_a_sequential_group_runs_again_with_its_group() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let started = manager
            .start_execution(
                parameters(&[
                    "registry_implementation",
                    "registry_verification",
                    "minimal_path",
                ]),
                "",
            )
            .await
            .unwrap();
        let before = terminal(&manager, &started.id).await;
        let group = before
            .slots
            .iter()
            .find(|slot| slot.scenario_id == "registry_verification")
            .unwrap()
            .execution_id
            .clone();
        manager
            .rerun_scenario(&before.id, "registry_verification")
            .await
            .unwrap();
        let after = terminal(&manager, &before.id).await;
        assert_eq!(runner.submitted.load(Ordering::SeqCst), 3);
        let grouped = after
            .slots
            .iter()
            .filter(|slot| slot.scenario_id.starts_with("registry_"))
            .collect::<Vec<_>>();
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].execution_id, grouped[1].execution_id);
        assert_ne!(grouped[0].execution_id, group);
        assert!(grouped
            .iter()
            .all(|slot| slot.previous_attempts[0].execution_id == group && slot.observed == 1));
        let other = after
            .slots
            .iter()
            .find(|slot| slot.scenario_id == "minimal_path")
            .unwrap();
        assert!(other.previous_attempts.is_empty());
        assert!(after.warnings.contains(
            &"registry_implementation then registry_verification run only together, in this order; running one again runs the whole group."
                .to_string()
        ));
    }

    #[tokio::test]
    async fn only_a_finished_local_execution_runs_a_scenario_again() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let mut imported = import_stub();
        imported.state = "completed".into();
        imported.source = ExecutionSource::Github {
            repository: "o/r".into(),
            run_id: 42,
            run_attempt: 1,
            url: "https://github.com/o/r/actions/runs/42".into(),
            release_control_execution_id: None,
            stack: None,
        };
        imported.slots = vec![github::slot(1, "case-minimal", "minimal_path")];
        imported.slots[0].execution_id = "0123456789abcdef0123456789abcdef".into();
        manager.write_execution(&imported).await.unwrap();
        let error = manager
            .rerun_scenario(&imported.id, "minimal_path")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("imported from GitHub"), "{error}");
        assert!(
            error.contains("https://github.com/o/r/actions/runs/42"),
            "{error}"
        );
        assert!(error.contains("import the run again"), "{error}");

        let started = manager
            .start_execution(parameters(&["minimal_path", "retired_scenario"]), "Nightly")
            .await
            .unwrap();
        let finished = terminal(&manager, &started.id).await;
        for (scenario, reason) in [
            (
                "retired_scenario",
                "does not know the scenario 'retired_scenario'",
            ),
            ("trend_blog", "did not run the scenario 'trend_blog'"),
        ] {
            let error = manager
                .rerun_scenario(&finished.id, scenario)
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(reason), "{error}");
        }
        // A recorded request this runner refuses is refused before anything changes.
        let mut unreadable = finished.clone();
        unreadable.slots[0].request["lane"] = json!("");
        manager.write_execution(&unreadable).await.unwrap();
        let error = format!(
            "{:#}",
            manager
                .rerun_scenario(&finished.id, "minimal_path")
                .await
                .unwrap_err()
        );
        assert!(error.contains("lane cannot be empty"), "{error}");
        manager.write_execution(&finished).await.unwrap();

        // A busy runner names what holds it, as a start does; a running
        // execution cannot run a scenario again.
        runner.hold.store(true, Ordering::SeqCst);
        let running = manager
            .start_execution(parameters(&["minimal_path"]), "Other")
            .await
            .unwrap();
        let error = manager
            .rerun_scenario(&finished.id, "minimal_path")
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            format!(
                "\"Other\" ({}) is still running; wait for it to finish or cancel it.",
                running.id
            )
        );
        let error = manager
            .rerun_scenario(&running.id, "minimal_path")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Only a finished execution"), "{error}");
        let unchanged = manager.read_execution(&finished.id).await.unwrap();
        assert_eq!(
            serde_json::to_value(&unchanged).unwrap(),
            serde_json::to_value(&finished).unwrap()
        );
    }

    #[tokio::test]
    async fn another_identity_is_refused_before_spending_and_never_projected() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let started = manager
            .start_execution(parameters(&["minimal_path", "persistent_state"]), "")
            .await
            .unwrap();
        let before = terminal(&manager, &started.id).await;

        // The stack now runs another Harness: refused with what differs.
        runner.new_harness.store(true, Ordering::SeqCst);
        let error = manager
            .rerun_scenario(&before.id, "minimal_path")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(r#"harness_version: "1.8.0" → "1.9.0""#),
            "{error}"
        );
        assert!(error.contains("Use Run again"), "{error}");
        assert_eq!(runner.submitted.load(Ordering::SeqCst), 2);
        assert_eq!(
            serde_json::to_value(manager.read_execution(&before.id).await.unwrap()).unwrap(),
            serde_json::to_value(&before).unwrap()
        );

        // A run that reports another identity than the stack did is removed,
        // and the execution returns to where it was, saying why.
        runner.new_harness.store(false, Ordering::SeqCst);
        runner.diverge_next.store(true, Ordering::SeqCst);
        manager
            .rerun_scenario(&before.id, "minimal_path")
            .await
            .unwrap();
        let after = terminal(&manager, &before.id).await;
        assert_eq!(runner.submitted.load(Ordering::SeqCst), 3);
        assert_eq!(after.state, "completed");
        assert_eq!(after.finished_at, before.finished_at);
        assert_eq!(
            serde_json::to_value(&after.slots).unwrap(),
            serde_json::to_value(&before.slots).unwrap()
        );
        assert_eq!(runner.records.lock().await.len(), 2);
        assert!(
            after.warnings.iter().any(|warning| warning.starts_with(
                "Running minimal_path again stopped: Stack or runner identity changed"
            )),
            "{:?}",
            after.warnings
        );
        assert!(runner.owner.lock().await.is_none());
    }

    #[tokio::test]
    async fn cancelling_a_rerun_keeps_the_rounds_it_did_not_admit() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let mut three = parameters(&["minimal_path"]);
        three.runs = 3;
        let started = manager.start_execution(three, "").await.unwrap();
        let before = terminal(&manager, &started.id).await;
        assert_eq!(before.slots.len(), 3);

        runner.hold.store(true, Ordering::SeqCst);
        manager
            .rerun_scenario(&before.id, "minimal_path")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while runner.submitted.load(Ordering::SeqCst) < 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // Admitted or waiting, each round reads as running until it finishes.
        let detail = manager
            .execution_detail(&before.id, &[])
            .await
            .unwrap()
            .unwrap();
        assert!(detail["reports"]
            .as_array()
            .unwrap()
            .iter()
            .all(|report| report["state"] == "running" && report["available"] == false));
        manager.cancel(&before.id).await.unwrap();
        let after = terminal(&manager, &before.id).await;
        assert_eq!(runner.submitted.load(Ordering::SeqCst), 4);
        // The admitted round is the current attempt; the others keep theirs.
        assert_eq!(
            after.slots[0].previous_attempts[0].execution_id,
            before.slots[0].execution_id
        );
        assert_eq!(after.slots[0].observed, 1);
        assert_eq!(
            serde_json::to_value(&after.slots[1..]).unwrap(),
            serde_json::to_value(&before.slots[1..]).unwrap()
        );
        assert_eq!(after.state, "completed");
        assert!(!after.cancel_requested);
        assert!(after
            .warnings
            .iter()
            .any(|warning| warning.starts_with("Running minimal_path again was cancelled")));
        assert!(runner.owner.lock().await.is_none());
    }

    #[tokio::test]
    async fn a_restart_during_a_rerun_returns_the_execution_to_where_it_was() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let started = manager
            .start_execution(parameters(&["minimal_path"]), "")
            .await
            .unwrap();
        let before = terminal(&manager, &started.id).await;
        // Accepted, then the worker stopped before its run was admitted.
        let mut receipt = before.clone();
        receipt.rerun = Some(Rerun {
            scenarios: vec!["minimal_path".into()],
            runs: vec![before.slots[0].execution_id.clone()],
            started_at: now(),
            state: before.state.clone(),
            error: before.error.clone(),
            finished_at: before.finished_at.clone(),
        });
        receipt.state = "running".into();
        receipt.finished_at = None;
        manager.write_execution(&receipt).await.unwrap();
        manager.reconcile().await.unwrap();
        let recovered = manager.read_execution(&before.id).await.unwrap();
        assert_eq!(recovered.state, "completed");
        assert_eq!(recovered.error, before.error);
        assert_eq!(recovered.finished_at, before.finished_at);
        assert!(recovered.rerun.is_none());
        assert_eq!(
            serde_json::to_value(&recovered.slots).unwrap(),
            serde_json::to_value(&before.slots).unwrap()
        );
        assert!(recovered
            .warnings
            .iter()
            .any(|warning| warning
                .starts_with("Running minimal_path again stopped: Worker restarted")));
    }

    #[tokio::test]
    async fn a_cancelled_execution_completes_only_once_every_slot_ran() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        runner.hold.store(true, Ordering::SeqCst);
        let started = manager
            .start_execution(parameters(&["minimal_path", "persistent_state"]), "")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while runner.submitted.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        manager.cancel(&started.id).await.unwrap();
        let cancelled = terminal(&manager, &started.id).await;
        assert_eq!(cancelled.state, "cancelled");
        assert_eq!(cancelled.slots[1].state, "not_run");
        runner.hold.store(false, Ordering::SeqCst);

        // Running the scenario that ran again leaves one that never did.
        manager
            .rerun_scenario(&started.id, "minimal_path")
            .await
            .unwrap();
        let partial = terminal(&manager, &started.id).await;
        assert_eq!(partial.state, "cancelled");
        assert_eq!(partial.error, cancelled.error);
        assert_eq!(partial.slots[0].previous_attempts.len(), 1);
        assert_eq!(partial.slots[1].state, "not_run");

        // A run never admitted is no attempt; once every slot ran, it completes.
        manager
            .rerun_scenario(&started.id, "persistent_state")
            .await
            .unwrap();
        let complete = terminal(&manager, &started.id).await;
        assert_eq!(complete.state, "completed", "{:?}", complete.error);
        assert!(complete.slots[1].previous_attempts.is_empty());
        assert_eq!(complete.slots[1].observed, 1);
        assert!(complete.slots.iter().all(|slot| slot.eligible));
    }

    #[tokio::test]
    async fn a_run_whose_evidence_is_gone_is_still_a_previous_attempt() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let started = manager
            .start_execution(parameters(&["minimal_path"]), "")
            .await
            .unwrap();
        let before = terminal(&manager, &started.id).await;
        let old = before.slots[0].execution_id.clone();
        runner.unreadable.lock().unwrap().insert(old.clone());
        manager
            .rerun_scenario(&before.id, "minimal_path")
            .await
            .unwrap();
        let after = terminal(&manager, &before.id).await;
        assert_eq!(after.slots[0].previous_attempts[0].execution_id, old);
    }

    #[tokio::test]
    async fn a_rerun_keeps_the_recorded_stack_and_warns_once() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let started = manager
            .start_execution(parameters(&["minimal_path"]), "")
            .await
            .unwrap();
        let mut before = terminal(&manager, &started.id).await;
        before.warnings.clear();
        manager.write_execution(&before).await.unwrap();
        runner.dirty.store(false, Ordering::SeqCst);
        for _ in 0..2 {
            manager
                .rerun_scenario(&before.id, "minimal_path")
                .await
                .unwrap();
            terminal(&manager, &before.id).await;
        }
        let after = manager.read_execution(&before.id).await.unwrap();
        assert_eq!(after.stack, before.stack);
        assert_eq!(
            after.warnings,
            vec![
                "Worker sources were not recorded: compose is unavailable",
                "minimal_path ran again on a stack that differs from the recorded one: queue.",
            ]
        );
    }

    #[test]
    fn a_scenario_run_again_names_the_workers_that_changed() {
        let worker = |name: &str, dirty| StackWorker {
            name: name.into(),
            source: WorkerSource::Path,
            requested: None,
            observed: Some("0.4.1".into()),
            commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
            dirty: Some(dirty),
            groups: Vec::new(),
        };
        let recorded = [worker("queue", true), worker("state", false)];
        assert!(stack_changes(&recorded, &recorded).is_empty());
        assert_eq!(
            stack_changes(
                &recorded,
                &[
                    worker("queue", false),
                    worker("state", false),
                    worker("stream", false)
                ]
            ),
            vec!["queue", "stream"]
        );
        // What could not be read is not a change.
        assert!(stack_changes(&recorded, &[]).is_empty());
        assert!(stack_changes(&[], &recorded).is_empty());
    }

    #[test]
    fn a_stack_change_during_an_execution_names_what_changed() {
        let runner = FakeRunner::new(tempfile::tempdir().unwrap().path().into());
        let request: RunRequest = serde_json::from_value(json!({
            "idempotency_key": "identity", "lane": "local", "model": "model",
            "provider": "provider", "scenarios": ["minimal_path"], "runs": 1,
        }))
        .unwrap();
        let report = runner.native_record(request).unwrap().report.unwrap();
        let mut pinned = Some(serde_json::to_value(&report.system_under_test).unwrap());
        verify_system_identity(&mut pinned, &report).unwrap();
        let identity = pinned.as_mut().unwrap();
        identity["harness_version"] = json!("1.7.0");
        identity["stack"]["workers_revision"] = json!("abc");
        let error = verify_system_identity(&mut pinned, &report)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(r#"harness_version: "1.7.0" → "1.8.0""#),
            "{error}"
        );
        assert!(error.contains("stack.workers_revision"), "{error}");
    }

    #[test]
    fn a_seed_from_an_older_console_or_row_is_ignored() {
        let parameters: ExecutionParameters = serde_json::from_value(json!({
            "scenarios": ["minimal_path"], "runs": 1, "technical_retries": 0,
            "seed": "18446744073709551615", "model": "m", "provider": "p", "agent": null,
        }))
        .unwrap();
        assert!(serde_json::to_value(&parameters)
            .unwrap()
            .get("seed")
            .is_none());
    }

    #[tokio::test]
    async fn runs_list_with_one_call_and_contracts_are_read_apart_and_cached() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let gh = fake_gh(
            root.path(),
            r#"echo "$*" >> "$(dirname "$0")/calls"
case "$1" in
  api) printf '%s' '{"total_count":1,"workflow_runs":[{"id":41,"run_attempt":2,"display_title":"E2E · 366030b3-5f55","created_at":"2026-09-20T10:00:00Z","run_started_at":"2026-09-21T09:00:00Z","conclusion":"failure","html_url":"https://github.com/o/r/actions/runs/41"}]}' ;;
  run) echo "no valid artifacts found to download" >&2; exit 1 ;;
esac"#,
        );
        let calls = || fs::read_to_string(root.path().join("calls")).unwrap();
        let manager = manager_with_gh(&data, Arc::new(FakeRunner::new(data.clone())), gh);

        let listed = manager.github_runs("o/r", 1).await.unwrap();
        let run = &listed["runs"][0];
        // Dated by the run's creation; the latest attempt's start apart.
        assert_eq!(run["created_at"], "2026-09-20T10:00:00Z");
        assert_eq!(run["attempt_started_at"], "2026-09-21T09:00:00Z");
        assert_eq!(run["release_control_execution_id"], "366030b3-5f55");
        assert_eq!(run["contract_pending"], true);
        assert_eq!(calls().lines().count(), 1, "only the list: {}", calls());

        let read = manager
            .github_run_contracts(
                "o/r",
                &[github::GithubRunAttempt {
                    run_id: 41,
                    run_attempt: 2,
                }],
            )
            .await
            .unwrap();
        assert_eq!(read["runs"][0]["run_id"], 41);
        assert!(read["runs"][0]["contract_error"]
            .as_str()
            .unwrap()
            .contains("90 days"));

        // Read once: the next list carries it and downloads nothing.
        let listed = manager.github_runs("o/r", 1).await.unwrap();
        assert!(listed["runs"][0].get("contract_pending").is_none());
        assert_eq!(
            listed["runs"][0]["contract_error"],
            read["runs"][0]["contract_error"]
        );
        assert_eq!(
            calls()
                .lines()
                .filter(|call| call.starts_with("run download"))
                .count(),
            1
        );
    }
}
