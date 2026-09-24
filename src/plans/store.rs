//! One saved-plan lifecycle with durable orchestration receipts and native Results.
//! Every planned child and its idempotency key is durable before admission.
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::{LocalPlan, PlanRunRole as Role};
use crate::artifact;
use crate::control::{execution_id_for_key, ControlPlane, ExecutionRecord, RunRequest};
use crate::persistence::Persistence;
use crate::report::{E2eReport, ReportState};
use crate::test_plan::{self, ProfileSnapshot};

mod github;
mod stack;

pub(crate) use github::{GithubRunContractsRequest, GithubRunImportRequest, GithubRunsListRequest};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct SavedPlan {
    #[serde(flatten)]
    pub plan: LocalPlan,
    pub snapshot: ProfileSnapshot,
    pub snapshot_sha256: String,
    pub configuration_sha256: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum Request {
    Requirements { plan_id: String },
    Export { plan_id: String },
    Execution { execution_id: String },
    Cancel { execution_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Check {
    pub id: String,
    pub status: String,
    pub message: String,
}
fn check(id: &str, ok: bool, message: impl Into<String>) -> Check {
    Check {
        id: id.into(),
        status: if ok { "ready" } else { "blocked" }.into(),
        message: message.into(),
    }
}

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
}

/// One execution, whatever produced it: a saved plan run here or a run
/// imported from GitHub. Where it came from is data (`source`), never a
/// different record.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct PlanExecution {
    pub id: String,
    /// The saved plan it ran; absent for an imported execution.
    #[serde(default)]
    pub plan_id: Option<String>,
    pub idempotency_key: String,
    pub configuration_sha256: String,
    /// Baseline or candidate within its plan; absent without a plan.
    #[serde(default)]
    pub role: Option<Role>,
    /// Editable name; the plan label (or the id) is shown when absent.
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
    pub baseline_eligible: bool,
    pub slots: Vec<Slot>,
    pub measurements: Option<Value>,
    #[serde(default)]
    pub system_under_test: Option<Value>,
}

/// What an execution ran, to run it again. Always the canonical cases: a
/// `seed` sent or stored by an older Console is ignored, so every execution
/// pairs with any other by scenario and repetition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ExecutionParameters {
    pub scenarios: Vec<String>,
    pub runs: u32,
    pub technical_retries: u8,
    pub model: String,
    pub provider: String,
    /// Agent profile the subject ran under.
    pub agent: Option<String>,
    /// Suite of the master plan an imported GitHub execution ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite: Option<String>,
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

pub(crate) fn validate_saved_plan(plan: &SavedPlan) -> Result<()> {
    ensure!(!plan.plan.id.is_empty(), "Unsupported plan identity");
    ensure!(
        artifact::sha256_value(&plan.snapshot)? == plan.snapshot_sha256,
        "Saved plan snapshot hash does not match"
    );
    ensure!(
        configuration_digest(&plan.plan, &plan.snapshot_sha256)? == plan.configuration_sha256,
        "Saved plan configuration digest does not match"
    );
    Ok(())
}

pub(crate) fn validate_saved_plan_execution(execution: &PlanExecution) -> Result<()> {
    ensure!(
        !execution.id.is_empty()
            && execution.plan_id.as_ref().is_none_or(|id| !id.is_empty())
            && !execution.idempotency_key.is_empty(),
        "Unsupported plan execution identity"
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
    async fn requirements(&self, config: &LocalPlan) -> Result<Vec<Check>>;
    async fn active(&self) -> Option<Value>;
    async fn reserve(&self, owner: &str) -> Result<()>;
    async fn release(&self, owner: &str);
    async fn submit(&self, owner: &str, request: RunRequest) -> Result<String>;
    async fn record(&self, id: &str) -> Option<ExecutionRecord>;
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
    async fn requirements(&self, config: &LocalPlan) -> Result<Vec<Check>> {
        let models =
            serde_json::to_value(crate::catalog::list_with_client(self.client(), None).await?)?;
        let contains = |model: &str, provider: &str| {
            models.as_array().is_some_and(|models| {
                models
                    .iter()
                    .any(|m| m["model"] == model && m["provider"] == provider)
            })
        };
        let mut checks = vec![check(
            "model",
            contains(&config.model, &config.provider),
            "Execution model must be available in this stack's catalog.",
        )];
        let functions = self
            .client()
            .trigger(iii_sdk::protocol::TriggerRequest {
                function_id: "engine::functions::list".into(),
                payload: json!({ "include_internal": true }),
                action: None,
                timeout_ms: Some(15_000),
            })
            .await?;
        let has_send = crate::context::function_ids(&functions).any(|id| id == "harness::send");
        checks.push(check(
            "harness",
            has_send,
            "The Harness execution function must be registered.",
        ));
        let info = self.client().trigger(iii_sdk::protocol::TriggerRequest {
            function_id: "engine::functions::info".into(), payload: json!({"function_ids": crate::wire::control_plane_function_ids().collect::<Vec<_>>()}), action: None, timeout_ms: Some(15_000),
        }).await?;
        let contracts = crate::wire::validate_control_plane(&info);
        checks.push(check(
            "contracts",
            contracts.is_ok(),
            contracts
                .err()
                .map(|e| format!("Native control-plane contracts are incompatible: {e:#}"))
                .unwrap_or_else(|| "Native control-plane contracts are compatible.".into()),
        ));
        if config.scenario_ids.iter().any(|id| {
            matches!(
                id.as_str(),
                "shell_coder_sandbox" | "chess_engine_build" | "trend_blog"
            )
        }) {
            let git = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                tokio::process::Command::new("git")
                    .arg("--version")
                    .kill_on_drop(true)
                    .output(),
            )
            .await;
            checks.push(check(
                "shared_fixture",
                matches!(git, Ok(Ok(output)) if output.status.success()),
                "Git is required to prepare the embedded shared fixture automatically during setup.",
            ));
        }
        Ok(checks)
    }
    async fn active(&self) -> Option<Value> {
        if let Some(id) = self.active_plan().await {
            return Some(json!({"id": id, "kind": "plan"}));
        }
        self.records()
            .await
            .ok()?
            .into_iter()
            .find(|r| !r.phase.terminal())
            .map(|r| json!({"id": r.execution_id, "kind": "native"}))
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
    url: String,
    persistence: Option<Persistence>,
    runner: Option<Arc<dyn Runner>>,
    github: github::GithubCli,
    // Serializes receipt transitions against cancellation and admission.
    lock: Mutex<()>,
}
impl PlanStore {
    pub(crate) async fn new(
        root: PathBuf,
        url: String,
        control: Option<ControlPlane>,
    ) -> Result<Arc<Self>> {
        #[cfg(test)]
        if control.is_none() {
            fs::create_dir_all(root.join("plan-store/plans"))?;
            fs::create_dir_all(root.join("plan-store/executions"))?;
        }
        let manager = Arc::new(Self {
            root,
            url,
            persistence: control.as_ref().map(ControlPlane::persistence),
            runner: control.map(|c| Arc::new(c) as Arc<dyn Runner>),
            github: github::GithubCli::default(),
            lock: Mutex::new(()),
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
    #[cfg(not(test))]
    fn persistence(&self) -> Result<&Persistence> {
        self.persistence
            .as_ref()
            .context("the E2E control-plane persistence is not available")
    }
    #[cfg(test)]
    fn plan_path(&self, id: &str) -> Result<PathBuf> {
        safe_id(id)?;
        Ok(self
            .root
            .join("plan-store/plans")
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
    pub(crate) async fn read_plan(&self, id: &str) -> Result<SavedPlan> {
        safe_id(id)?;
        if let Some(persistence) = &self.persistence {
            return persistence
                .saved_plan(id)
                .await?
                .context("unknown saved plan");
        }
        #[cfg(not(test))]
        anyhow::bail!("the E2E control-plane persistence is not available");
        #[cfg(test)]
        let plan: SavedPlan = serde_json::from_slice(&fs::read(self.plan_path(id)?)?)?;
        #[cfg(test)]
        ensure!(plan.plan.id == id, "Unsupported plan identity");
        #[cfg(test)]
        Ok(plan)
    }
    async fn write_plan(&self, plan: &SavedPlan) -> Result<()> {
        if let Some(persistence) = &self.persistence {
            return persistence.save_plan(plan).await;
        }
        #[cfg(not(test))]
        anyhow::bail!("the E2E control-plane persistence is not available");
        #[cfg(test)]
        write_json(&self.plan_path(&plan.plan.id)?, plan)
    }
    pub(crate) async fn read_execution(&self, id: &str) -> Result<PlanExecution> {
        safe_id(id)?;
        if let Some(persistence) = &self.persistence {
            return persistence
                .saved_plan_execution(id)
                .await?
                .context("unknown saved plan execution");
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
            return persistence.save_plan_execution(execution).await;
        }
        #[cfg(not(test))]
        anyhow::bail!("the E2E control-plane persistence is not available");
        #[cfg(test)]
        write_json(&self.execution_path(&execution.id)?, execution)
    }
    pub(crate) async fn executions(&self) -> Result<Vec<PlanExecution>> {
        if let Some(persistence) = &self.persistence {
            return persistence.saved_plan_executions(None).await;
        }
        #[cfg(not(test))]
        anyhow::bail!("the E2E control-plane persistence is not available");
        #[cfg(test)]
        read_json_directory(&self.root.join("plan-store/executions"))
    }
    async fn history(&self, plan_id: &str) -> Result<Vec<PlanExecution>> {
        #[cfg(not(test))]
        let mut history = self
            .persistence()?
            .saved_plan_executions(Some(plan_id))
            .await?;
        #[cfg(test)]
        let mut history: Vec<_> = self
            .executions()
            .await?
            .into_iter()
            .filter(|e| e.plan_id.as_deref() == Some(plan_id))
            .collect();
        history.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(history)
    }
    pub(crate) async fn get_local(&self, id: &str) -> Result<super::LocalPlan> {
        self.canonical(&self.read_plan(id).await?).await
    }
    pub(crate) async fn list_local(&self) -> Result<Vec<super::LocalPlan>> {
        if let Some(persistence) = &self.persistence {
            let saved = persistence.saved_plans().await?;
            let mut plans = Vec::new();
            for plan in saved {
                plans.push(self.canonical(&plan).await?);
            }
            plans.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then_with(|| b.id.cmp(&a.id))
            });
            return Ok(plans);
        }
        #[cfg(not(test))]
        anyhow::bail!("the E2E control-plane persistence is not available");
        #[cfg(test)]
        {
            let mut plans = Vec::new();
            for entry in fs::read_dir(self.root.join("plan-store/plans"))? {
                let entry = entry?;
                let path = entry.path();
                if !entry.file_type()?.is_file()
                    || path.extension().and_then(|value| value.to_str()) != Some("json")
                {
                    continue;
                }
                let id = path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default();
                match self.read_plan(id).await {
                    Ok(plan) => plans.push(self.canonical(&plan).await?),
                    Err(error) => tracing::warn!(path = %path.display(), error = %error,
                    "ignoring an unsupported or corrupt local E2E plan"),
                }
            }
            plans.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then_with(|| b.id.cmp(&a.id))
            });
            Ok(plans)
        }
    }
    async fn canonical(&self, saved: &SavedPlan) -> Result<super::LocalPlan> {
        use super::PlanState;
        let mut plan = saved.plan.clone();
        let history = self.history(&saved.plan.id).await?;
        for execution in history.iter().rev() {
            if execution.active() {
                continue;
            }
            if execution.baseline_eligible {
                if plan.baseline_execution_id.is_none() {
                    plan.baseline_execution_id = Some(execution.id.clone());
                } else if plan.baseline_execution_id.as_ref() != Some(&execution.id)
                    && !plan.candidate_execution_ids.contains(&execution.id)
                {
                    plan.candidate_execution_ids.push(execution.id.clone());
                }
            } else if !plan.incomplete_execution_ids.contains(&execution.id) {
                plan.incomplete_execution_ids.push(execution.id.clone());
            }
        }
        if let Some(last) = history.first() {
            plan.last_attempt_id = Some(last.id.clone());
            plan.updated_at = plan.updated_at.max(last.updated_at.clone());
            plan.state = if last.active() {
                if plan.baseline_execution_id.is_some() {
                    PlanState::CandidateRunning
                } else {
                    PlanState::BaselineRunning
                }
            } else if !plan.candidate_execution_ids.is_empty() {
                PlanState::ComparisonReady
            } else if plan.baseline_execution_id.is_some() {
                PlanState::BaselineReady
            } else {
                PlanState::Draft
            };
        }
        plan.locked = plan.locked || !history.is_empty();
        plan.compatible = verify_snapshot(saved).is_ok();
        Ok(plan)
    }
    pub(crate) async fn create_local(
        &self,
        request: super::PlanCreateRequest,
    ) -> Result<super::LocalPlan> {
        let _guard = self.lock.lock().await;
        let id = format!(
            "plan-{}-{}",
            Utc::now().format("%Y%m%dT%H%M%S"),
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let mut plan = super::new_plan(&request, id)?;
        let snapshot = if let Some(source) = &request.duplicate_of {
            let source = self.read_plan(source).await?;
            let original = self.canonical(&source).await?;
            ensure!(
                plan.scenario_ids == original.scenario_ids
                    && plan.runs == original.runs
                    && plan.technical_retries == original.technical_retries
                    && plan.seed == original.seed,
                "A duplicate must preserve scope and execution policy."
            );
            plan.template_id = original.template_id;
            plan.scenarios = original.scenarios;
            Some(source.snapshot)
        } else {
            None
        };
        let prepared = prepared_plan(plan, snapshot)?;
        validate_config(&prepared.plan, &self.url)?;
        self.write_plan(&prepared).await?;
        self.canonical(&prepared).await
    }
    pub(crate) async fn update_local(
        &self,
        id: &str,
        update: super::PlanUpdateRequest,
    ) -> Result<super::LocalPlan> {
        let _guard = self.lock.lock().await;
        let saved = self.read_plan(id).await?;
        let mut plan = self.canonical(&saved).await?;
        let old_scope = plan.scope_hash.clone();
        super::apply_update(&mut plan, &update)?;
        let snapshot = if old_scope == plan.scope_hash {
            saved.snapshot
        } else {
            snapshot_for_plan(&plan, Some(saved.snapshot.profile))?
        };
        let prepared = prepared_plan(plan.clone(), Some(snapshot))?;
        validate_config(&prepared.plan, &self.url)?;
        self.write_plan(&prepared).await?;
        self.canonical(&prepared).await
    }
    pub(crate) async fn delete_local(&self, id: &str) -> Result<()> {
        loop {
            let active = {
                let _guard = self.lock.lock().await;
                self.read_plan(id).await?;
                self.history(id)
                    .await?
                    .into_iter()
                    .filter(PlanExecution::active)
                    .map(|execution| execution.id)
                    .collect::<Vec<_>>()
            };
            if !active.is_empty() {
                for execution_id in active {
                    self.cancel(&execution_id).await?;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            let deleted = {
                let _guard = self.lock.lock().await;
                self.read_plan(id).await?;
                let executions = self.history(id).await?;
                if executions.iter().any(PlanExecution::active) {
                    false
                } else {
                    if let Some(persistence) = &self.persistence {
                        persistence.delete_plan_and_executions(id).await?;
                    } else {
                        #[cfg(not(test))]
                        anyhow::bail!("the E2E control-plane persistence is not available");
                        #[cfg(test)]
                        {
                            for execution in executions {
                                fs::remove_file(self.execution_path(&execution.id)?)?;
                            }
                            fs::remove_file(self.plan_path(id)?)?;
                        }
                    }
                    true
                }
            };
            if deleted {
                return Ok(());
            }
        }
    }
    pub(crate) async fn start_local(
        self: &Arc<Self>,
        id: &str,
        idempotency_key: &str,
        role: Role,
    ) -> Result<super::LocalPlan> {
        let admitted = self.start(id, idempotency_key, role).await?;
        ensure!(
            admitted["blocked"] != true,
            "Plan admission is blocked: {}",
            admitted["requirements"]
        );
        self.get_local(id).await
    }

    /// Start an execution from its parameters alone, on this stack: no saved
    /// plan, no role. A scenario this runner does not know gets a slot that
    /// says so; every other slot runs. A scenario of a sequential group brings
    /// the whole group, and the execution says what was added.
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
        for (name, value) in [
            ("model", Some(&parameters.model)),
            ("provider", Some(&parameters.provider)),
            ("agent", parameters.agent.as_ref()),
        ] {
            let Some(value) = value else { continue };
            ensure!(
                value.chars().count() <= 200 && !value.chars().any(char::is_control),
                "{name} must be at most 200 characters, without control characters"
            );
        }
        let groups = sequential_groups(&test_plan::embedded()?);
        let mut scenarios: Vec<String> = Vec::new();
        let mut warnings = Vec::new();
        for scenario in &parameters.scenarios {
            let group = groups.iter().find(|group| group.contains(scenario));
            for member in group.map_or(std::slice::from_ref(scenario), Vec::as_slice) {
                if !scenarios.contains(member) {
                    scenarios.push(member.clone());
                }
            }
            if let Some(group) = group.filter(|group| {
                group
                    .iter()
                    .any(|member| !parameters.scenarios.contains(member))
            }) {
                let note = format!(
                    "{} run only together, in this order; the whole group was added.",
                    group.join(" then ")
                );
                if !warnings.contains(&note) {
                    warnings.push(note);
                }
            }
        }
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
        let execution = PlanExecution {
            slots: parameter_slots(&parameters, &id, label.as_deref())?,
            id,
            plan_id: None,
            idempotency_key: key,
            configuration_sha256: artifact::sha256_value(&parameters)?,
            role: None,
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
            baseline_eligible: false,
            measurements: None,
            system_under_test: None,
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

    /// A runner that refused a reservation is busy: say which execution holds
    /// it, by name, with its id in parentheses for the Console to open.
    async fn busy(&self, runner: &Arc<dyn Runner>, error: anyhow::Error) -> anyhow::Error {
        let Some(id) = runner
            .active()
            .await
            .and_then(|active| active["id"].as_str().map(str::to_owned))
        else {
            return error;
        };
        let execution = self.read_execution(&id).await.ok();
        let mut name = execution.as_ref().and_then(|e| e.label.clone());
        if let (None, Some(plan_id)) = (&name, execution.and_then(|e| e.plan_id)) {
            name = self
                .read_plan(&plan_id)
                .await
                .ok()
                .map(|plan| plan.plan.label);
        }
        let holder = name.map_or_else(
            || format!("Another execution ({id})"),
            |name| format!("\"{name}\" ({id})"),
        );
        anyhow::anyhow!("{holder} is still running; wait for it to finish or cancel it.")
    }

    /// Delete a finished execution that ran no saved plan (a plan's executions
    /// go with the plan): its native runs first, then the execution.
    pub(crate) async fn delete_execution(&self, id: &str) -> Result<()> {
        let _guard = self.lock.lock().await;
        let execution = self.read_execution(id).await?;
        ensure!(
            execution.plan_id.is_none(),
            "The executions of a saved plan are deleted with the plan."
        );
        ensure!(
            !execution.active() && execution.state != "importing",
            "Only a finished execution can be deleted."
        );
        let runner = self.runner()?;
        let children = execution
            .slots
            .iter()
            .map(|slot| slot.execution_id.as_str())
            .filter(|child| !child.is_empty())
            .collect::<BTreeSet<_>>();
        for child in children {
            if runner.record(child).await.is_some() {
                runner.remove(child).await?;
            }
        }
        if let Some(persistence) = &self.persistence {
            return persistence.delete_plan_execution(id).await;
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

    pub(crate) async fn handle(self: &Arc<Self>, request: Request) -> Result<Value> {
        match request {
            Request::Requirements { plan_id } => {
                self.requirements(&self.read_plan(&plan_id).await?).await
            }
            Request::Export { plan_id } => export(&self.read_plan(&plan_id).await?),
            Request::Execution { execution_id } => Ok(serde_json::to_value(
                self.read_execution(&execution_id).await?,
            )?),
            Request::Cancel { execution_id } => self.cancel(&execution_id).await,
        }
    }
    async fn requirements(&self, plan: &SavedPlan) -> Result<Value> {
        let snapshot = &plan.snapshot;
        let config = &plan.plan;
        let validation = validate_config(config, &self.url);
        let mut checks = vec![check(
            "configuration",
            validation.is_ok(),
            validation
                .err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "Model and evaluator configuration is complete.".into()),
        )];
        {
            let identity = verify_snapshot(plan);
            checks.push(check(
                "revision",
                identity.is_ok(),
                identity
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "Saved cases and contracts match this runner.".into()),
            ));
        }
        let active = if let Some(runner) = &self.runner {
            match runner.requirements(config).await {
                Ok(runtime) => checks.extend(runtime),
                Err(error) => checks.push(check(
                    "stack",
                    false,
                    format!("Cannot verify this stack: {error:#}"),
                )),
            }
            runner.active().await
        } else {
            checks.push(check(
                "stack",
                false,
                "This dashboard cannot execute plans.",
            ));
            None
        };
        let mut active = active;
        if let Some(value) = active.as_mut() {
            if value["kind"] == "plan" {
                if let Some(id) = value["id"].as_str() {
                    if let Ok(execution) = self.read_execution(id).await {
                        value["plan_id"] = json!(execution.plan_id);
                    }
                }
            }
        }
        checks.push(check(
            "admission",
            active.is_none(),
            "One plan execution at a time. Follow active work before starting another plan.",
        ));
        let mut requirements = BTreeSet::new();
        for case in &snapshot.cases {
            if let Some(items) = case["requirements"].as_array() {
                for item in items {
                    if let Some(item) = item.as_str() {
                        requirements.insert(item.to_owned());
                    }
                }
            }
        }
        for requirement in requirements {
            checks.push(Check {
                id: format!("fixture:{requirement}"),
                status: "pending".into(),
                message: format!("{requirement}: verified by the native scenario during setup."),
            });
        }
        let ready = checks.iter().all(|c| c.status != "blocked");
        Ok(
            json!({"ready": ready, "checks": checks, "active_execution": active, "snapshot": snapshot}),
        )
    }
    async fn start(self: &Arc<Self>, plan_id: &str, key: &str, role: Role) -> Result<Value> {
        ensure!(
            !key.trim().is_empty() && key.len() <= 200,
            "A bounded idempotency key is required."
        );
        let _guard = self.lock.lock().await;
        let mut plan = self.read_plan(plan_id).await?;
        let id = format!("plan-{}", &artifact::sha256_bytes(key.as_bytes())[7..39]);
        #[cfg(test)]
        let existing = if self.persistence.is_none() && self.execution_path(&id)?.exists() {
            Some(self.read_execution(&id).await?)
        } else if let Some(db) = &self.persistence {
            db.saved_plan_execution(&id).await?
        } else {
            None
        };
        #[cfg(not(test))]
        let existing = self.persistence()?.saved_plan_execution(&id).await?;
        if let Some(existing) = existing {
            ensure!(
                existing.plan_id.as_deref() == Some(plan_id)
                    && existing.configuration_sha256 == plan.configuration_sha256
                    && existing.role == Some(role),
                "Idempotency key already belongs to a different plan request."
            );
            return Ok(json!({"execution_id": id, "duplicate": true, "execution": existing}));
        }
        verify_snapshot(&plan)?;
        let canonical = self.canonical(&plan).await?;
        let has_baseline = canonical.baseline_execution_id.is_some();
        ensure!(
            matches!(
                (&role, has_baseline),
                (Role::Baseline, false) | (Role::Candidate, true)
            ),
            "A plan captures one valid baseline before running candidates."
        );
        let preflight = self.requirements(&plan).await?;
        if preflight["ready"] != true {
            return Ok(json!({"blocked": true, "requirements": preflight}));
        }
        let runner = self.runner()?;
        if let Err(error) = runner.reserve(&id).await {
            return Err(self.busy(runner, error).await);
        }
        let config = &plan.plan;
        let mut execution = PlanExecution {
            id: id.clone(),
            plan_id: Some(plan_id.into()),
            idempotency_key: key.into(),
            configuration_sha256: plan.configuration_sha256.clone(),
            role: Some(role),
            label: None,
            parameters: Some(ExecutionParameters {
                suite: None,
                scenarios: config.scenario_ids.clone(),
                runs: config.runs,
                technical_retries: config.technical_retries,
                model: config.model.clone(),
                provider: config.provider.clone(),
                agent: None,
            }),
            source: ExecutionSource::Local,
            stack: Vec::new(),
            warnings: Vec::new(),
            state: "running".into(),
            started_at: now(),
            updated_at: now(),
            finished_at: None,
            cancel_requested: false,
            error: None,
            baseline_eligible: false,
            slots: Vec::new(),
            measurements: None,
            system_under_test: None,
        };
        let persist = async {
            execution.slots = materialize_slots(&plan, &id)?;
            // Lock first: if receipt persistence fails, no child has started.
            plan.plan.locked = true;
            plan.plan.updated_at = now();
            if let Some(persistence) = &self.persistence {
                persistence
                    .save_plan_and_execution(&plan, &execution)
                    .await?;
            } else {
                self.write_plan(&plan).await?;
                self.write_execution(&execution).await?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = persist {
            runner.release(&id).await;
            return Err(error);
        }
        self.spawn_drive(&id);
        Ok(json!({"execution_id": id, "duplicate": false, "execution": execution}))
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
        // is a warning on the execution.
        let (stack, warnings) = runner.stack().await;
        let count = {
            let _guard = self.lock.lock().await;
            let mut execution = self.read_execution(id).await?;
            execution.stack = stack;
            execution.warnings.extend(warnings);
            self.write_execution(&execution).await?;
            execution.slots.len()
        };
        for index in 0..count {
            let execution = self.read_execution(id).await?;
            // A slot without a native run (an unknown scenario) has nothing
            // to admit; a grouped slot shares its group's run.
            if execution.slots[index].execution_id.is_empty()
                || execution.slots[..index]
                    .iter()
                    .any(|slot| slot.execution_id == execution.slots[index].execution_id)
            {
                continue;
            }
            let child = {
                let _guard = self.lock.lock().await;
                let mut execution = self.read_execution(id).await?;
                if execution.cancel_requested {
                    break;
                }
                if let Some(plan) = self.execution_plan(&execution).await? {
                    verify_snapshot(&plan)?;
                    ensure!(
                        plan.configuration_sha256 == execution.configuration_sha256,
                        "Plan identity changed during execution."
                    );
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
                let admitted = runner
                    .submit(id, serde_json::from_value(slot.request.clone())?)
                    .await?;
                ensure!(
                    admitted == slot.execution_id,
                    "Native child identity differs from the persisted slot."
                );
                admitted
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
                    let plan = self.execution_plan(&execution).await?;
                    if let Some(report) = record.report.as_ref().filter(|_| terminal) {
                        verify_system_identity(&mut execution.system_under_test, report)?;
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
                            update_slot(slot, &record, plan.as_ref(), &self.root)?;
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
        {
            let _guard = self.lock.lock().await;
            let mut execution = self.read_execution(id).await?;
            finish(&mut execution, None, &self.root)?;
            self.write_execution(&execution).await?;
        }
        runner.release(id).await;
        Ok(())
    }
    async fn cancel(&self, id: &str) -> Result<Value> {
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
        if let Ok(plan) = self.execution_plan(&execution).await {
            for slot in &mut execution.slots {
                if let Some(record) = runner.record(&slot.execution_id).await {
                    let _ = update_slot(slot, &record, plan.as_ref(), &self.root);
                }
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
    /// The saved plan an execution ran, if it ran one.
    async fn execution_plan(&self, execution: &PlanExecution) -> Result<Option<SavedPlan>> {
        match &execution.plan_id {
            Some(plan_id) => Ok(Some(self.read_plan(plan_id).await?)),
            None => Ok(None),
        }
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
            let plan = self.execution_plan(&execution).await?;
            if let Some(runner) = &self.runner {
                for slot in &mut execution.slots {
                    if let Some(record) = runner.record(&slot.execution_id).await {
                        ensure!(
                            record.phase.terminal(),
                            "Native child is still active during plan recovery"
                        );
                        if let Err(error) = update_slot(slot, &record, plan.as_ref(), &self.root) {
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
pub(crate) fn prepared_plan(
    plan: LocalPlan,
    snapshot: Option<ProfileSnapshot>,
) -> Result<SavedPlan> {
    let snapshot = snapshot
        .map(Ok)
        .unwrap_or_else(|| snapshot_for_plan(&plan, None))?;
    let snapshot_sha256 = artifact::sha256_value(&snapshot)?;
    Ok(SavedPlan {
        configuration_sha256: configuration_digest(&plan, &snapshot_sha256)?,
        plan,
        snapshot,
        snapshot_sha256,
    })
}
fn snapshot_for_plan(
    plan: &super::LocalPlan,
    base: Option<test_plan::Suite>,
) -> Result<ProfileSnapshot> {
    let master = test_plan::embedded()?;
    let mut profile = base
        .or_else(|| {
            master
                .suites
                .iter()
                .find(|profile| profile.id == plan.template_id.as_deref().unwrap_or("pr"))
                .cloned()
        })
        .context("Unknown plan template")?;
    profile.id = "saved-plan".into();
    profile.label = plan.label.clone();
    profile.purpose = plan.purpose.clone();
    profile.modules.clear();
    profile.scenarios = plan.scenario_ids.clone();
    profile
        .scenario_groups
        .retain(|group| group.iter().all(|id| plan.scenario_ids.contains(id)));
    profile.repetitions = plan.runs;
    profile.technical_retries = plan.technical_retries;
    profile.lane = "local".into();
    master.materialize_scope(profile, plan.seed)
}

fn configuration_digest(config: &LocalPlan, snapshot_digest: &str) -> Result<String> {
    let value = json!({
        "label": config.label, "purpose": config.purpose, "url": config.url,
        "model": config.model, "provider": config.provider,
        "scenarios": config.scenarios, "scenario_ids": config.scenario_ids,
        "runs": config.runs, "technical_retries": config.technical_retries, "seed": config.seed,
        "template_id": config.template_id, "scope_hash": config.scope_hash,
        "reference_execution_id": config.reference_execution_id,
        "reference_differences": config.reference_differences,
        "snapshot_sha256": snapshot_digest,
    });
    artifact::sha256_value(&value)
}
fn validate_config(config: &LocalPlan, url: &str) -> Result<()> {
    ensure!(
        !config.label.trim().is_empty() && config.label.len() <= 160,
        "Enter a plan name (up to 160 characters)."
    );
    ensure!(
        !config.model.trim().is_empty() && !config.provider.trim().is_empty(),
        "Select an execution model."
    );
    ensure!(
        config.url == url,
        "This plan belongs to a different stack endpoint."
    );
    Ok(())
}
fn verify_snapshot(plan: &SavedPlan) -> Result<()> {
    validate_saved_plan(plan)?;
    for expected in &plan.snapshot.cases {
        let id = expected["scenario_id"]
            .as_str()
            .context("Saved scenario missing")?;
        let seed = expected["seed"].as_u64().context("Saved seed missing")?;
        let current = super::resolve_scope(&[id.into()], Some(seed))?;
        let current = &current[0];
        ensure!(current.case_id == expected["case_id"] && current.inputs_sha256 == expected["inputs_sha256"]
            && current.contract_sha256 == expected["contract_sha256"],
            "Saved scenario contract is unavailable in this runner. Consultation and export remain available.");
    }
    Ok(())
}
fn materialize_slots(plan: &SavedPlan, owner: &str) -> Result<Vec<Slot>> {
    let c = &plan.plan;
    campaign_slots(
        &plan.snapshot,
        owner,
        &c.label,
        &c.model,
        &c.provider,
        None,
        c.seed,
    )
}
/// Slots of an execution without a saved plan: its scenarios materialized
/// like a plan's (the master plan's sequential groups kept together), then
/// one slot per round for each scenario this runner does not know, carrying
/// the reason instead of a native run.
fn parameter_slots(
    parameters: &ExecutionParameters,
    owner: &str,
    label: Option<&str>,
) -> Result<Vec<Slot>> {
    let (known, unknown): (Vec<_>, Vec<_>) = parameters
        .scenarios
        .iter()
        .cloned()
        .partition(|id| id.parse::<crate::scenarios::ScenarioId>().is_ok());
    let mut slots = Vec::new();
    if !known.is_empty() {
        let master = test_plan::embedded()?;
        let scenario_groups = sequential_groups(&master)
            .into_iter()
            .filter(|group| group.iter().all(|id| known.contains(id)))
            .collect();
        let label = label.unwrap_or("Execution");
        let profile = test_plan::Suite {
            id: "execution".into(),
            label: label.into(),
            purpose: String::new(),
            metrics: Vec::new(),
            modules: Vec::new(),
            scenarios: known,
            scenario_groups,
            repetitions: parameters.runs,
            technical_retries: parameters.technical_retries,
            lane: "local".into(),
        };
        let snapshot = master.materialize_scope(profile, None)?;
        slots = campaign_slots(
            &snapshot,
            owner,
            label,
            &parameters.model,
            &parameters.provider,
            parameters.agent.as_deref(),
            None,
        )?;
    }
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
    seed: Option<u64>,
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
                "scenarios": scenario_ids, "runs": 1, "seed": seed, "technical_retries": group["technical_retries"]}),
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
/// Project one terminal native run into its slot. With a saved plan, the run
/// must also match the plan's model and pinned cases.
fn update_slot(
    slot: &mut Slot,
    record: &ExecutionRecord,
    plan: Option<&SavedPlan>,
    root: &Path,
) -> Result<()> {
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
    if let Some(plan) = plan {
        ensure!(
            report.subject.model == plan.plan.model
                && report.subject.provider == plan.plan.provider,
            "Execution model identity differs"
        );
    }
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
    if let Some(plan) = plan {
        let expected = plan
            .snapshot
            .cases
            .iter()
            .find(|c| c["scenario_id"] == slot.scenario_id)
            .context("Case absent from pinned profile")?;
        ensure!(
            scenario.scenario_id == slot.scenario_id && scenario.case_id == expected["case_id"],
            "Native case identity differs from the pinned profile"
        );
        let case = scenario
            .case
            .as_ref()
            .context("Native materialized case is absent")?;
        ensure!(
            case.seed == expected["seed"].as_u64().context("Pinned seed missing")?
                && case.inputs_sha256 == expected["inputs_sha256"]
                && crate::scenarios::scenario_contract_sha256(case, scenario.execution_policy)?
                    == expected["contract_sha256"],
            "Native scenario contract or seed differs from the pinned profile"
        );
    }
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
        let mut left = identity.clone();
        let mut right = observed.clone();
        left.as_object_mut().unwrap().remove("contract_hashes");
        right.as_object_mut().unwrap().remove("contract_hashes");
        let mut changed = Vec::new();
        identity_changes("", &left, &right, &mut changed);
        ensure!(
            changed.is_empty(),
            "Stack or runner identity changed during the composed execution: {}",
            changed.join("; ")
        );
        for (id, digest) in observed["contract_hashes"]
            .as_object()
            .context("Native contract hashes are absent")?
        {
            if let Some(previous) = identity["contract_hashes"].get(id) {
                ensure!(
                    previous == digest,
                    "Native function contract {id} changed during execution"
                );
            }
            identity["contract_hashes"][id] = digest.clone();
        }
    } else {
        *pinned = Some(observed);
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
    execution.state = if execution.cancel_requested {
        "cancelled"
    } else if error.is_some() {
        "interrupted"
    } else {
        "completed"
    }
    .into();
    execution.error = error;
    execution.updated_at = now();
    execution.finished_at = Some(now());
    for slot in &mut execution.slots {
        if matches!(slot.state.as_str(), "pending" | "admitting" | "running") {
            slot.state = "not_run".into();
        }
    }
    execution.baseline_eligible = execution.state == "completed"
        && !execution.slots.is_empty()
        && execution.slots.iter().all(|s| s.eligible);
    let paths = result_paths(execution, root);
    if !paths.is_empty() {
        match test_plan::measure(&paths) {
            Ok(value) => execution.measurements = Some(value),
            Err(error) => {
                execution.baseline_eligible = false;
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
    json!({"id": execution.id, "plan_id": execution.plan_id, "state": execution.state, "role": execution.role, "started_at": execution.started_at, "finished_at": execution.finished_at, "planned": execution.slots.len(),
        "finished": execution.slots.iter().filter(|s| s.state == "finished").count(), "observed": execution.slots.iter().map(|s| s.observed).sum::<u32>(), "completed": execution.slots.iter().map(|s| s.completed).sum::<u32>(), "passed": execution.slots.iter().map(|s| s.passed).sum::<u32>(), "technical_valid": execution.slots.iter().map(|s| s.technical_valid).sum::<u32>(), "baseline_eligible": execution.baseline_eligible, "error": execution.error,
        "active_slot": execution.slots.iter().find(|s| matches!(s.state.as_str(), "running" | "admitting"))})
}
fn export(plan: &SavedPlan) -> Result<Value> {
    let suites: Vec<_> = plan.snapshot.campaigns.iter().map(|campaign| {
        let groups: Vec<_> = campaign["groups"].as_array().into_iter().flatten().cloned().collect();
        json!({"id": campaign["campaign_id"], "label": plan.snapshot.profile.label, "lane": campaign["lane"], "seed": null, "subject": {"model": plan.plan.model, "provider": plan.plan.provider}, "groups": groups})
    }).collect();
    Ok(
        json!({"schema": "harness-e2e-profile-campaigns", "plan_id": plan.snapshot.plan_id, "definition_sha256": plan.snapshot.definition_sha256,
        "profile": {"id": plan.snapshot.profile.id, "profile_sha256": plan.snapshot.profile_sha256, "campaigns": plan.snapshot.campaigns}, "saved_plan": plan, "release_control_suites": suites}),
    )
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
            let system = SystemUnderTestIdentity {
                stack: StackIdentity::Source {
                    workers_repository: "iii-hq/workers".into(),
                    workers_revision: "0123456789abcdef0123456789abcdef01234567".into(),
                },
                engine_version: "0.22.0".into(),
                engine_revision: None,
                harness_version: "1.8.0".into(),
                e2e_repository: "iii-hq/harness-e2e".into(),
                e2e_revision: "0123456789abcdef0123456789abcdef01234567".into(),
                contract_hashes: BTreeMap::from([("harness::status".into(), digest.clone())]),
            };
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
        async fn requirements(&self, _: &LocalPlan) -> Result<Vec<Check>> {
            Ok(Vec::new())
        }
        async fn active(&self) -> Option<Value> {
            self.owner
                .lock()
                .await
                .as_ref()
                .map(|id| json!({"id": id, "kind": "plan"}))
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
            }
            self.records.lock().await.insert(id.clone(), record);
            Ok(if self.wrong_identity.load(Ordering::SeqCst) {
                "different-child".into()
            } else {
                id
            })
        }
        async fn record(&self, id: &str) -> Option<ExecutionRecord> {
            self.records.lock().await.get(id).cloned()
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
                    dirty: Some(true),
                    groups: Vec::new(),
                }],
                vec!["Worker sources were not recorded: compose is unavailable".into()],
            )
        }
    }
    fn request(profile: &str) -> super::super::PlanCreateRequest {
        let snapshot = test_plan::embedded().unwrap().materialize(profile).unwrap();
        serde_json::from_value(json!({"label": "Plan test", "purpose": snapshot.profile.purpose,
            "url": "ws://localhost:49134", "model": "model", "provider": "provider",
            "scenarios": snapshot.scenario_ids,
            "runs": snapshot.profile.repetitions, "technical_retries": snapshot.profile.technical_retries,
            "template_id": profile})).unwrap()
    }
    fn manager(root: &Path, runner: Arc<FakeRunner>) -> Arc<PlanStore> {
        manager_with_gh(root, runner, github::GithubCli::default())
    }
    fn manager_with_gh(
        root: &Path,
        runner: Arc<FakeRunner>,
        github: github::GithubCli,
    ) -> Arc<PlanStore> {
        fs::create_dir_all(root.join("plan-store/plans")).unwrap();
        fs::create_dir_all(root.join("plan-store/executions")).unwrap();
        Arc::new(PlanStore {
            root: root.into(),
            url: request("pr").url,
            persistence: None,
            runner: Some(runner),
            github,
            lock: Mutex::new(()),
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
    async fn admitted(manager: &Arc<PlanStore>, profile: &str, key: &str) -> (String, String) {
        let plan = manager.create_local(request(profile)).await.unwrap();
        let id = plan.id.as_str().to_string();
        let response = manager.start(&id, key, Role::Baseline).await.unwrap();
        (id, response["execution_id"].as_str().unwrap().into())
    }
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires HARNESS_E2E_TEST_DATABASE_URL and an isolated database worker; run serially"]
    async fn real_database_start_recreates_a_stale_plan_table_keeping_readable_plans() {
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
            json!({"sql":"DELETE FROM saved_plan_executions WHERE origin = 'local'","params":[]}),
            json!({"sql":"DELETE FROM saved_plans WHERE origin = 'local'","params":[]}),
        ])
        .await
        .unwrap();
        let manager = Arc::new(PlanStore {
            root: root.path().into(),
            url: request("pr").url,
            persistence: Some(db.clone()),
            runner: Some(runner),
            github: github::GithubCli::default(),
            lock: Mutex::new(()),
        });
        let key = uuid::Uuid::new_v4().to_string();
        let (plan_id, baseline_id) = admitted(&manager, "pr", &key).await;
        let baseline = terminal(&manager, &baseline_id).await;
        let candidate_id = manager
            .start(&plan_id, &format!("{key}-candidate"), Role::Candidate)
            .await
            .unwrap()["execution_id"]
            .as_str()
            .unwrap()
            .to_owned();
        terminal(&manager, &candidate_id).await;
        let before = manager.get_local(&plan_id).await.unwrap();

        // A plan written under another layout: its row hash matches, its
        // snapshot digest does not.
        let foreign_id = manager
            .create_local(request("pr"))
            .await
            .unwrap()
            .id
            .as_str()
            .to_owned();
        let mut foreign =
            serde_json::to_value(db.saved_plan(&foreign_id).await.unwrap().unwrap()).unwrap();
        foreign["snapshot_sha256"] = json!("invalid");
        let payload = serde_json::to_string(&foreign).unwrap();
        let hash = artifact::sha256_bytes(payload.as_bytes());
        db.transaction(vec![json!({
            "sql": "UPDATE saved_plans SET payload_json = ?, payload_sha256 = ? WHERE id = ?",
            "params": [payload, hash, foreign_id]
        })])
        .await
        .unwrap();

        // The plan tables were written under another layout: the next start
        // recreates them with the plans and receipts it can still read.
        db.transaction(vec![json!({
            "sql": "UPDATE harness_e2e_storage SET fingerprint = 'sha256:foreign' WHERE name IN ('saved_plans', 'saved_plan_executions')",
            "params": []
        })])
        .await
        .unwrap();
        db.initialize(root.path()).await.unwrap();
        assert!(db.saved_plan(&foreign_id).await.unwrap().is_none());
        let after = manager.get_local(&plan_id).await.unwrap();
        assert_eq!(after.baseline_execution_id, before.baseline_execution_id);
        assert_eq!(
            after.candidate_execution_ids,
            before.candidate_execution_ids
        );
        assert_eq!(
            db.saved_plan_execution(&baseline_id)
                .await
                .unwrap()
                .unwrap()
                .slots
                .len(),
            baseline.slots.len()
        );
        assert_eq!(
            manager.start(&plan_id, &key, Role::Baseline).await.unwrap()["duplicate"],
            true
        );
        assert_eq!(manager.list_local().await.unwrap().len(), 1);

        // Reads delete a row this runner cannot read instead of failing.
        let mut stale =
            serde_json::to_value(db.saved_plan(&plan_id).await.unwrap().unwrap()).unwrap();
        stale["snapshot_sha256"] = json!("invalid");
        let payload = serde_json::to_string(&stale).unwrap();
        let hash = artifact::sha256_bytes(payload.as_bytes());
        db.transaction(vec![json!({
            "sql": "UPDATE saved_plans SET payload_json = ?, payload_sha256 = ? WHERE id = ?",
            "params": [payload, hash, plan_id]
        })])
        .await
        .unwrap();
        assert!(db.saved_plans().await.unwrap().is_empty());
        assert!(db.saved_plan(&plan_id).await.unwrap().is_none());
        assert!(db
            .saved_plan_executions(Some(&plan_id))
            .await
            .unwrap()
            .is_empty());
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
    async fn templates_use_the_shared_scope_edits_and_baseline_candidate_lifecycle() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let request = serde_json::from_value(json!({"label": "Edited PR", "purpose": "Custom scope from a template", "url": request("pr").url,
            "model": "model", "provider": "provider",
            "scenarios": ["minimal_path", "persistent_state"], "runs": 2, "technical_retries": 0, "template_id": "pr"})).unwrap();
        let plan = manager.create_local(request).await.unwrap();
        assert_eq!(plan.scenario_ids.len(), 2);
        assert_eq!(plan.template_id.as_deref(), Some("pr"));
        let mut saved = manager.read_plan(&plan.id).await.unwrap();
        // A removed template remains provenance for a saved plan.
        saved.plan.template_id = Some("smoke".into());
        saved.snapshot.definition_sha256 = "historical-template-revision".into();
        saved.snapshot_sha256 = artifact::sha256_value(&saved.snapshot).unwrap();
        saved.configuration_sha256 =
            configuration_digest(&saved.plan, &saved.snapshot_sha256).unwrap();
        manager.write_plan(&saved).await.unwrap();
        let started = manager
            .start_local(&plan.id, "baseline", Role::Baseline)
            .await
            .unwrap();
        let baseline = terminal(&manager, started.last_attempt_id.as_deref().unwrap()).await;
        assert_eq!(baseline.slots.len(), 4);
        crate::dashboard::presenter::validate_execution_id(&baseline.id).unwrap();
        for invalid in [
            "plan-../results",
            "plan-short",
            "plan-0000000000000000000000000000000/",
        ] {
            assert!(crate::dashboard::presenter::validate_execution_id(invalid).is_err());
        }
        let detail = manager
            .execution_detail(&baseline.id, &[])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail["id"], baseline.id);
        assert_eq!(detail["assessment_summary"]["run_count"], 4);
        assert_eq!(detail["native_execution_ids"].as_array().unwrap().len(), 4);
        let reports = detail["reports"].as_array().unwrap();
        assert_eq!(reports.len(), 4);
        assert!(reports.iter().all(|report| report["available"] == true));
        for (report, slot) in reports.iter().zip(&baseline.slots) {
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
        assert!(baseline.baseline_eligible);
        let ready = manager.get_local(&plan.id).await.unwrap();
        assert_eq!(
            ready.baseline_execution_id.as_deref(),
            Some(baseline.id.as_str())
        );
        assert_eq!(
            manager
                .update_local(
                    &plan.id,
                    serde_json::from_value(json!({"runs": 3})).unwrap()
                )
                .await
                .unwrap()
                .runs,
            3
        );
        let started = manager
            .start_local(&plan.id, "candidate", Role::Candidate)
            .await
            .unwrap();
        let candidate = terminal(&manager, started.last_attempt_id.as_deref().unwrap()).await;
        assert_eq!(
            manager
                .get_local(&plan.id)
                .await
                .unwrap()
                .candidate_execution_ids,
            vec![candidate.id]
        );
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
            json!({"profile": {"label": "Smoke"}}).to_string(),
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
            plan_id: None,
            idempotency_key: "github:iii-hq/harness-e2e#42".into(),
            configuration_sha256: String::new(),
            role: None,
            label: None,
            parameters: None,
            source: ExecutionSource::Github {
                repository: "iii-hq/harness-e2e".into(),
                run_id: 42,
                run_attempt: 1,
                url: "https://github.com/iii-hq/harness-e2e/actions/runs/42".into(),
                release_control_execution_id: Some("rc-execution".into()),
            },
            stack: Vec::new(),
            warnings: Vec::new(),
            state: "importing".into(),
            started_at: "2026-09-20T10:00:00Z".into(),
            updated_at: now(),
            finished_at: Some("2026-09-20T11:00:00Z".into()),
            cancel_requested: false,
            error: None,
            baseline_eligible: false,
            slots: Vec::new(),
            measurements: None,
            system_under_test: None,
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
                suite: Some("smoke".into()),
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
            plan_id: None,
            idempotency_key: "github:owner/repo#1".into(),
            configuration_sha256: String::new(),
            role: None,
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
            baseline_eligible: false,
            slots: Vec::new(),
            measurements: None,
            system_under_test: None,
        }
    }

    #[test]
    fn plans_that_reproduced_a_reference_keep_validating() {
        let mut plan = super::super::new_plan(&request("pr"), "plan-rc-legacy".into()).unwrap();
        plan.reference_execution_id = Some("1b8e60cc-7818-4dcb-8cbc-c76790c271af".into());
        plan.reference_differences = vec!["context_pressure: case identity differs".into()];
        let saved = prepared_plan(plan, None).unwrap();
        let stored: SavedPlan =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        validate_saved_plan(&stored).unwrap();
        assert_eq!(stored.configuration_sha256, saved.configuration_sha256);
        let fresh = prepared_plan(
            super::super::new_plan(&request("pr"), "plan-new".into()).unwrap(),
            None,
        )
        .unwrap();
        assert!(serde_json::to_value(&fresh.plan)
            .unwrap()
            .get("reference_execution_id")
            .is_none());
        let schema = serde_json::to_value(schemars::schema_for!(LocalPlan)).unwrap();
        assert!(schema["properties"].get("reference_execution_id").is_none());
    }

    #[tokio::test]
    async fn deletes_drafts_and_admitted_plans() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner);
        let draft = manager.create_local(request("pr")).await.unwrap();
        manager.delete_local(&draft.id).await.unwrap();
        assert!(manager.get_local(&draft.id).await.is_err());

        let (id, execution_id) = admitted(&manager, "pr", "delete-locked").await;
        terminal(&manager, &execution_id).await;
        manager.delete_local(&id).await.unwrap();
        assert!(manager.get_local(&id).await.is_err());
        assert!(manager.history(&id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn current_store_rejects_old_formats_and_does_not_load_old_history() {
        let root = tempfile::tempdir().unwrap();
        // Previous directories are outside the current store, including interrupted receipts.
        fs::create_dir_all(root.path().join("plans")).unwrap();
        fs::create_dir_all(root.path().join("plan-executions")).unwrap();
        fs::write(root.path().join("plans/old.json"), b"old plan").unwrap();
        fs::write(root.path().join("plan-executions/old.json"), b"old receipt").unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner);
        manager.reconcile().await.unwrap();
        assert!(manager.list_local().await.unwrap().is_empty());
        assert!(manager.executions().await.unwrap().is_empty());
        let plan = manager.create_local(request("pr")).await.unwrap();
        let saved = manager.read_plan(&plan.id).await.unwrap();
        let encoded = serde_json::to_value(&saved).unwrap();
        assert!(encoded.get("base").is_none());
        assert!(encoded.get("configuration").is_none());
        manager.write_plan(&saved).await.unwrap();
        fs::write(
            root.path().join("plan-store/plans/corrupt.json"),
            b"invalid json",
        )
        .unwrap();
        assert_eq!(manager.list_local().await.unwrap().len(), 1);
        fs::write(
            root.path().join("plan-store/executions/corrupt.json"),
            b"invalid json",
        )
        .unwrap();
        assert!(manager.list_local().await.is_err());
        for action in ["create", "update", "get", "duplicate", "start", "compare"] {
            assert!(serde_json::from_value::<Request>(json!({"action": action})).is_err());
        }
    }

    #[tokio::test]
    async fn dashboard_summaries_skip_bad_receipts_without_hiding_valid_children() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner);
        let plan = manager.create_local(request("pr")).await.unwrap();
        let saved = manager.read_plan(&plan.id).await.unwrap();
        let execution = PlanExecution {
            id: "plan-good".into(),
            plan_id: Some(plan.id.clone()),
            idempotency_key: "good".into(),
            configuration_sha256: saved.configuration_sha256,
            role: Some(Role::Baseline),
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
            baseline_eligible: false,
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
                },
            ],
            measurements: Some(json!({"cohorts": [{
                "scenario_id": "direct_answer", "identity": {},
                "aggregate": {"observed_runs": 1, "planned_runs": 1, "completed_runs": 1,
                    "total_tokens_consumed": 20, "cost": {"total_usd": 0.1}},
            }]})),
            system_under_test: None,
        };
        manager.write_execution(&execution).await.unwrap();
        fs::write(
            root.path().join("plan-store/executions/corrupt.json"),
            b"not json",
        )
        .unwrap();
        let missing = PlanExecution {
            id: "plan-missing".into(),
            plan_id: Some("missing-plan".into()),
            ..execution
        };
        manager.write_execution(&missing).await.unwrap();

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
    async fn profile_contracts_are_pinned_and_duplicates_have_no_execution_state() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner);
        for profile in &test_plan::embedded().unwrap().suites {
            let profile = profile.id.as_str();
            let value = manager.create_local(request(profile)).await.unwrap();
            let id = value.id.as_str();
            let mut plan = manager.read_plan(id).await.unwrap();
            verify_snapshot(&plan).unwrap();
            let round_trip: SavedPlan =
                serde_json::from_slice(&fs::read(manager.plan_path(id).unwrap()).unwrap()).unwrap();
            assert_eq!(round_trip.snapshot_sha256, plan.snapshot_sha256);
            let mut duplicate = request(profile);
            duplicate.duplicate_of = Some(id.into());
            duplicate.label = "Copy".into();
            duplicate.model = "another".into();
            duplicate.provider = "another-provider".into();
            let copy = manager.create_local(duplicate).await.unwrap();
            assert_ne!(copy.id, id);
            assert!(manager.history(&copy.id).await.unwrap().is_empty());
            assert!(!copy.locked);
            assert_eq!(
                manager.read_plan(&copy.id).await.unwrap().snapshot_sha256,
                plan.snapshot_sha256
            );
            assert!(copy.baseline_execution_id.is_none());
            assert!(copy.candidate_execution_ids.is_empty());
            let mut changed = plan.clone();
            changed.plan.model = "tampered-model".into();
            assert!(verify_snapshot(&changed).is_err());
            plan.snapshot.profile.repetitions += 1;
            assert!(verify_snapshot(&plan).is_err());
            manager.write_plan(&plan).await.unwrap();
            assert!(!manager.get_local(id).await.unwrap().compatible);
            assert!(export(&plan).is_ok());
        }
        let mut missing = request("pr");
        missing.model.clear();
        assert!(manager.create_local(missing).await.is_err());
    }
    #[tokio::test]
    async fn native_coordination_covers_all_profile_slots() {
        for (profile, expected_slots, expected_submissions) in [
            ("regression", 9, 9),
            ("software-engineering", 15, 14),
            ("pr", 4, 4),
            ("after-release", 5, 5),
        ] {
            let root = tempfile::tempdir().unwrap();
            let runner = Arc::new(FakeRunner::new(root.path().into()));
            let manager = manager(root.path(), runner.clone());
            runner.fail_next.store(true, Ordering::SeqCst);
            let (plan_id, id) = admitted(&manager, profile, profile).await;
            let execution = terminal(&manager, &id).await;
            assert_eq!(
                execution.state, "completed",
                "{profile}: {:?}",
                execution.error
            );
            assert_eq!(execution.slots.len(), expected_slots);
            assert_eq!(
                runner.submitted.load(Ordering::SeqCst),
                expected_submissions
            );
            assert!(execution.baseline_eligible); // Objective failure is independent from validity.
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
                manager
                    .read_plan(&plan_id)
                    .await
                    .unwrap()
                    .snapshot
                    .scenario_ids
                    .len()
            );
            let repeated = manager
                .start(&plan_id, profile, Role::Baseline)
                .await
                .unwrap();
            assert_eq!(repeated["duplicate"], true);
            let updated = manager
                .update_local(
                    &plan_id,
                    serde_json::from_value(json!({"model": "changed"})).unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(updated.model, "changed");
            assert_eq!(
                updated.baseline_execution_id.as_deref(),
                Some(execution.id.as_str())
            );
            assert_eq!(
                manager
                    .read_execution(&execution.id)
                    .await
                    .unwrap()
                    .configuration_sha256,
                execution.configuration_sha256,
            );
        }
    }
    #[tokio::test]
    async fn grouped_registry_cases_share_one_child_without_duplicate_measurements() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let registry_request = || {
            let mut request = request("software-engineering");
            request.scenarios = vec![
                "registry_planning".into(),
                "registry_implementation".into(),
                "registry_environment".into(),
                "registry_verification".into(),
            ];
            request.runs = 1;
            request
        };
        let plan = manager.create_local(registry_request()).await.unwrap();
        let plan_id = plan.id;
        let response = manager
            .start(&plan_id, "registry-group", Role::Baseline)
            .await
            .unwrap();
        let id = response["execution_id"].as_str().unwrap().to_string();
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
        assert!(execution.baseline_eligible);

        let snapshot = &manager.read_plan(&plan_id).await.unwrap().snapshot;
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
            let expected = snapshot
                .cases
                .iter()
                .find(|case| case["scenario_id"] == slot.scenario_id)
                .unwrap();
            assert_eq!(scenario.case.as_ref().unwrap().seed, expected["seed"]);
        }

        let mut partial = registry_request();
        partial.scenarios = vec!["registry_implementation".into()];
        let partial = manager.create_local(partial).await.unwrap();
        let snapshot = &manager.read_plan(&partial.id).await.unwrap().snapshot;
        let groups = snapshot.campaigns[0]["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["scenarios"], json!(["registry_implementation"]));

        let mut seeded = registry_request();
        seeded.seed = Some(7);
        let seeded = manager.create_local(seeded).await.unwrap();
        let seeded = manager.read_plan(&seeded.id).await.unwrap();
        assert!(materialize_slots(&seeded, "seeded")
            .unwrap()
            .iter()
            .all(|slot| slot.request["seed"] == 7));
    }
    #[tokio::test]
    async fn cancellation_reserves_admission_and_prevents_next_groups() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        runner.hold.store(true, Ordering::SeqCst);
        let manager = manager(root.path(), runner.clone());
        let (plan, id) = admitted(&manager, "pr", "cancel").await;
        let duplicate = manager
            .start(&plan, "cancel", Role::Baseline)
            .await
            .unwrap();
        assert_eq!(duplicate["duplicate"], true);
        let other = manager
            .start(&plan, "another", Role::Baseline)
            .await
            .unwrap();
        assert_eq!(other["blocked"], true);
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
        assert!(!execution.baseline_eligible);
        assert!(runner.owner.lock().await.is_none());
    }
    #[tokio::test]
    async fn missing_artifacts_and_identity_divergence_interrupt_without_promoting_reference() {
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
            let (plan, id) = admitted(&manager, "software-engineering", "missing").await;
            let execution = terminal(&manager, &id).await;
            assert_eq!(execution.state, "interrupted");
            assert!(!execution.baseline_eligible);
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
            assert!(manager
                .get_local(&plan)
                .await
                .unwrap()
                .baseline_execution_id
                .is_none());
        }
    }
    #[tokio::test]
    async fn persistence_failure_never_dispatches_a_child() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let plan = manager.create_local(request("pr")).await.unwrap();
        runner.fail_receipt.store(true, Ordering::SeqCst);
        assert!(manager
            .start(plan.id.as_str(), "fail-write", Role::Baseline)
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
        let (plan, id) = admitted(&manager, "pr", "restart").await;
        let mut receipt = terminal(&manager, &id).await;
        receipt.state = "running".into();
        receipt.baseline_eligible = false;
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
        assert!(manager
            .get_local(&plan)
            .await
            .unwrap()
            .baseline_execution_id
            .is_none());
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

        assert_eq!(execution.plan_id, None);
        assert_eq!(execution.role, None);
        assert_eq!(execution.source, ExecutionSource::Local);
        assert_eq!(execution.label.as_deref(), Some("Again"));
        let parameters = execution.parameters.as_ref().unwrap();
        assert_eq!(parameters.model, "model");
        assert_eq!(
            parameters.scenarios,
            vec!["context_pressure", "retired_scenario", "minimal_path"]
        );
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
        assert!(!execution.baseline_eligible);
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
        let parameters = ExecutionParameters {
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
        let slots = parameter_slots(&parameters, "plan-grouped", None).unwrap();
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
                "registry_implementation",
                "registry_verification",
                "minimal_path"
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
    async fn a_completed_plan_execution_names_the_slot_that_did_not_run_cleanly() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        runner.crash_next.store(true, Ordering::SeqCst);
        let (_, id) = admitted(&manager, "pr", "crash").await;
        let execution = terminal(&manager, &id).await;
        // No safety stop: every slot ran, and the reason is the execution's.
        assert_eq!(execution.state, "completed");
        assert_eq!(runner.submitted.load(Ordering::SeqCst), 4);
        assert!(!execution.baseline_eligible);
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
    async fn a_finished_execution_without_a_plan_is_deleted_with_its_runs() {
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

        // A plan's executions stay until the plan is deleted.
        let (_, planned) = admitted(&manager, "pr", "kept").await;
        terminal(&manager, &planned).await;
        let error = manager.delete_execution(&planned).await.unwrap_err();
        assert!(
            error.to_string().contains("deleted with the plan"),
            "{error}"
        );
        assert!(manager.read_execution(&planned).await.is_ok());
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
