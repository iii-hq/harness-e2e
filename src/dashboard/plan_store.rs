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

use crate::artifact;
use crate::control::{execution_id_for_key, ControlPlane, ExecutionRecord, RunRequest};
use crate::report::{E2eReport, ReportState};
use crate::test_plan::{self, ProfileSnapshot};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct Configuration {
    pub label: String,
    pub profile_id: String,
    pub url: String,
    pub model: String,
    pub provider: String,
    #[serde(default)]
    pub judge_model: String,
    #[serde(default)]
    pub judge_provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(super) struct SavedPlan {
    pub schema_version: u32,
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub locked: bool,
    pub configuration: Configuration,
    pub snapshot: ProfileSnapshot,
    pub snapshot_sha256: String,
    pub configuration_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<super::plans::LocalPlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum Role {
    Run,
    Baseline,
    Candidate,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(super) enum Request {
    Requirements {
        configuration: Option<Configuration>,
        plan_id: Option<String>,
    },
    Create {
        configuration: Configuration,
    },
    Update {
        plan_id: String,
        configuration: Configuration,
    },
    Get {
        plan_id: String,
    },
    Duplicate {
        plan_id: String,
        label: String,
        model: String,
        provider: String,
    },
    Export {
        plan_id: String,
    },
    Start {
        plan_id: String,
        idempotency_key: String,
        role: Role,
    },
    Execution {
        execution_id: String,
    },
    Cancel {
        execution_id: String,
    },
    Compare {
        plan_id: String,
        candidate_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(super) struct Check {
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
pub(super) struct Slot {
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(super) struct PlanExecution {
    pub schema: String,
    pub id: String,
    pub plan_id: String,
    pub idempotency_key: String,
    pub configuration_sha256: String,
    pub role: Role,
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
impl PlanExecution {
    fn active(&self) -> bool {
        matches!(self.state.as_str(), "running" | "cancelling")
    }
}

#[async_trait]
trait Runner: Send + Sync {
    async fn requirements(&self, config: &Configuration) -> Result<Vec<Check>>;
    async fn active(&self) -> Option<Value>;
    async fn reserve(&self, owner: &str) -> Result<()>;
    async fn release(&self, owner: &str);
    async fn submit(&self, owner: &str, request: RunRequest) -> Result<String>;
    async fn record(&self, id: &str) -> Option<ExecutionRecord>;
    async fn cancel(&self, id: &str) -> Result<()>;
}
#[async_trait]
impl Runner for ControlPlane {
    async fn requirements(&self, config: &Configuration) -> Result<Vec<Check>> {
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
        if !config.judge_model.is_empty() {
            checks.push(check(
                "judge",
                contains(&config.judge_model, &config.judge_provider),
                "Evaluator must be available in this stack's catalog.",
            ));
        }
        let functions = self
            .client()
            .trigger(iii_sdk::protocol::TriggerRequest {
                function_id: "engine::functions::list".into(),
                payload: json!({ "include_internal": true }),
                action: None,
                timeout_ms: Some(15_000),
            })
            .await?;
        let has_send = super::bus::function_ids(&functions).any(|id| id == "harness::send");
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
        if config.profile_id == "shell-coder-fixture" {
            let fixture = crate::scenarios::shell_coder_sandbox::validate_fixture();
            checks.push(check(
                "shared_fixture",
                fixture.is_ok(),
                fixture
                    .err()
                    .map(|e| format!("Pinned shared engineering fixture is unavailable: {e:#}"))
                    .unwrap_or_else(|| {
                        "Pinned shared engineering fixture revision and file digests match.".into()
                    }),
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
}

pub(super) struct PlanStore {
    root: PathBuf,
    url: String,
    runner: Option<Arc<dyn Runner>>,
    // Serializes receipt transitions against cancellation and admission.
    lock: Mutex<()>,
}
impl PlanStore {
    pub(super) async fn new(
        root: PathBuf,
        url: String,
        control: Option<ControlPlane>,
    ) -> Result<Arc<Self>> {
        let manager = Arc::new(Self {
            root,
            url,
            runner: control.map(|c| Arc::new(c) as Arc<dyn Runner>),
            lock: Mutex::new(()),
        });
        fs::create_dir_all(manager.root.join("plans"))?;
        fs::create_dir_all(manager.root.join("plan-executions"))?;
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
    fn plan_path(&self, id: &str) -> Result<PathBuf> {
        safe_id(id)?;
        Ok(self.root.join("plans").join(format!("{id}.json")))
    }
    fn execution_path(&self, id: &str) -> Result<PathBuf> {
        safe_id(id)?;
        Ok(self.root.join("plan-executions").join(format!("{id}.json")))
    }
    fn read_plan(&self, id: &str) -> Result<SavedPlan> {
        let value: Value = serde_json::from_slice(&fs::read(self.plan_path(id)?)?)?;
        if value["schema_version"] == 1 {
            return prepared_plan(serde_json::from_value(value)?, None);
        }
        let plan: SavedPlan = serde_json::from_value(value)?;
        ensure!(
            plan.schema_version == 2 && plan.id == id,
            "Unsupported plan identity or schema"
        );
        Ok(plan)
    }
    fn write_plan(&self, plan: &SavedPlan) -> Result<()> {
        write_json(&self.plan_path(&plan.id)?, plan)
    }
    fn read_execution(&self, id: &str) -> Result<PlanExecution> {
        let execution: PlanExecution =
            serde_json::from_slice(&fs::read(self.execution_path(id)?)?)?;
        ensure!(
            execution.schema == "harness-e2e-plan-execution/v1" && execution.id == id,
            "Unsupported execution identity"
        );
        Ok(execution)
    }
    fn write_execution(&self, execution: &PlanExecution) -> Result<()> {
        write_json(&self.execution_path(&execution.id)?, execution)
    }
    pub(super) fn executions(&self) -> Result<Vec<PlanExecution>> {
        read_json_directory(&self.root.join("plan-executions"))
    }
    pub(super) fn dashboard_summaries(
        &self,
    ) -> Result<(Vec<Value>, std::collections::BTreeMap<String, String>)> {
        let mut values = Vec::new();
        let mut children = std::collections::BTreeMap::new();
        for execution in self.executions()? {
            let plan = self.read_plan(&execution.plan_id)?;
            let summary = execution_summary(&execution);
            for slot in &execution.slots {
                children.insert(slot.execution_id.clone(), execution.id.clone());
            }
            let status = match execution.state.as_str() {
                "completed" if execution.slots.iter().all(|s| s.passed == 1) => "passed",
                "completed" => "failed",
                "interrupted" => "incomplete",
                other => other,
            };
            values.push(json!({"id": execution.id, "label": plan.configuration.label, "run_id": execution.id,
                "kind": "profile_plan", "plan_id": plan.id, "profile_id": plan.configuration.profile_id, "plan_execution": summary,
                "attempt": 1, "workflow_name": "Harness plan", "workflow_url": null, "event": "local", "actor": "local",
                "started_at": execution.started_at, "completed_at": execution.finished_at.as_deref().unwrap_or(""), "generated_at": execution.updated_at,
                "status": status, "conclusion": if status == "passed" { "success" } else { "" }, "availability": "available", "lane": plan.snapshot.profile.lane,
                "subjects": [{"id": plan.configuration.model, "model": plan.configuration.model, "provider": plan.configuration.provider, "scenarios": []}],
                "requested_runs": execution.slots.len(), "scenario_metrics": [], "execution": {"id": execution.id},
                "totals": {"expected_reports": execution.slots.len(), "received_reports": summary["observed"], "missing_reports": execution.slots.len() as u64 - summary["observed"].as_u64().unwrap_or(0),
                    "report_coverage": summary["observed"].as_f64().map(|observed| observed / execution.slots.len().max(1) as f64), "passed_scenarios": summary["passed"], "total_tokens": null, "total_cost_usd": null},
                "first_failure": execution.error.as_ref().map(|error| json!({"kind": "plan_execution", "message": error}))}));
        }
        for value in &mut values {
            let execution =
                self.read_execution(value["id"].as_str().context("Missing execution id")?)?;
            project_measurements(value, &execution, &self.root)?;
        }
        Ok((values, children))
    }
    pub(super) fn execution_detail(&self, id: &str) -> Result<Option<Value>> {
        if !self.execution_path(id)?.exists() {
            return Ok(None);
        }
        let (summaries, _) = self.dashboard_summaries()?;
        let mut summary = summaries
            .into_iter()
            .find(|value| value["id"] == id)
            .context("Plan execution missing")?;
        summary["reports"] = json!([]);
        summary["native_execution_ids"] = json!(self
            .read_execution(id)?
            .slots
            .iter()
            .map(|slot| &slot.execution_id)
            .collect::<Vec<_>>());
        Ok(Some(summary))
    }

    fn history(&self, plan_id: &str) -> Result<Vec<PlanExecution>> {
        let mut history: Vec<_> = self
            .executions()?
            .into_iter()
            .filter(|e| e.plan_id == plan_id)
            .collect();
        history.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(history)
    }
    fn view(&self, plan: &SavedPlan) -> Result<Value> {
        let history = self.history(&plan.id)?;
        let mut value = serde_json::to_value(plan)?;
        value["last_execution"] = history
            .first()
            .map(execution_summary)
            .unwrap_or(Value::Null);
        value["history"] = json!(history.iter().map(execution_summary).collect::<Vec<_>>());
        let canonical = self.canonical(plan)?;
        for (key, item) in serde_json::to_value(canonical)?.as_object().unwrap() {
            value[key] = item.clone();
        }
        Ok(value)
    }
    pub(super) fn get_local(&self, id: &str) -> Result<super::plans::LocalPlan> {
        self.canonical(&self.read_plan(id)?)
    }
    pub(super) fn list_local(&self) -> Result<Vec<super::plans::LocalPlan>> {
        let mut plans = Vec::new();
        for entry in fs::read_dir(self.root.join("plans"))? {
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
            match self.get_local(id) {
                Ok(plan) => plans.push(plan),
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
    fn canonical(&self, saved: &SavedPlan) -> Result<super::plans::LocalPlan> {
        use super::plans::PlanState;
        let mut plan = match &saved.base {
            Some(plan) => plan.clone(),
            None => serde_json::from_value(json!({
                "schema_version": 1, "id": saved.id, "label": saved.configuration.label,
                "purpose": saved.snapshot.profile.purpose, "created_at": saved.created_at, "updated_at": saved.updated_at,
                "state": "draft", "locked": saved.locked, "scope_hash": saved.configuration_sha256,
                "url": saved.configuration.url, "model": saved.configuration.model, "provider": saved.configuration.provider,
                "judge_model": saved.configuration.judge_model, "judge_provider": saved.configuration.judge_provider,
                "scenarios": saved.snapshot.cases.iter().map(|c| { let mut c = c.clone(); c["complexity_tier"] = json!("profile"); c }).collect::<Vec<_>>(),
                "scenario_ids": saved.snapshot.scenario_ids, "runs": saved.snapshot.profile.repetitions,
                "technical_retries": saved.snapshot.profile.technical_retries, "seed": null,
                "baseline_execution_id": null, "candidate_execution_ids": [], "incomplete_execution_ids": [], "last_attempt_id": null,
                "template_id": saved.snapshot.profile.id
            }))?,
        };
        let history = self.history(&saved.id)?;
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
            plan.updated_at = last.updated_at.clone();
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
        plan.locked = saved.locked || plan.locked || !history.is_empty();
        plan.compatible = verify_snapshot(saved).is_ok();
        plan.protected_executor_required = saved.snapshot.protected_supervisor_required;
        Ok(plan)
    }
    pub(super) async fn create_local(
        &self,
        request: super::plans::PlanCreateRequest,
    ) -> Result<super::plans::LocalPlan> {
        let _guard = self.lock.lock().await;
        let id = format!(
            "plan-{}-{}",
            Utc::now().format("%Y%m%dT%H%M%S"),
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let mut plan = super::plans::new_plan(&request, id)?;
        let snapshot = if let Some(source) = &request.duplicate_of {
            let source = self.read_plan(source)?;
            let original = self.canonical(&source)?;
            ensure!(
                plan.scenario_ids == original.scenario_ids
                    && plan.runs == original.runs
                    && plan.technical_retries == original.technical_retries
                    && plan.seed == original.seed
                    && plan.judge_model == original.judge_model
                    && plan.judge_provider == original.judge_provider,
                "A duplicate must preserve scope, evaluator and execution policy."
            );
            plan.template_id = original.template_id;
            plan.scenarios = original.scenarios;
            Some(source.snapshot)
        } else {
            None
        };
        let prepared = prepared_plan(plan, snapshot)?;
        validate_config(&prepared.configuration, &prepared.snapshot, &self.url)?;
        self.write_plan(&prepared)?;
        self.canonical(&prepared)
    }
    pub(super) async fn update_local(
        &self,
        id: &str,
        update: super::plans::PlanUpdateRequest,
    ) -> Result<super::plans::LocalPlan> {
        let _guard = self.lock.lock().await;
        let saved = self.read_plan(id)?;
        let mut plan = self.canonical(&saved)?;
        let old_scope = plan.scope_hash.clone();
        super::plans::apply_update(&mut plan, &update)?;
        let prepared = prepared_plan(
            plan.clone(),
            (old_scope == plan.scope_hash).then_some(saved.snapshot),
        )?;
        validate_config(&prepared.configuration, &prepared.snapshot, &self.url)?;
        self.write_plan(&prepared)?;
        self.canonical(&prepared)
    }
    pub(super) async fn start_local(
        self: &Arc<Self>,
        id: &str,
        role: super::plans::PlanRunRole,
    ) -> Result<super::plans::LocalPlan> {
        let role = match role {
            super::plans::PlanRunRole::Baseline => Role::Baseline,
            super::plans::PlanRunRole::Candidate => Role::Candidate,
        };
        let admitted = self
            .start(id, &uuid::Uuid::new_v4().to_string(), role)
            .await?;
        ensure!(
            admitted["blocked"] != true,
            "Plan admission is blocked: {}",
            admitted["requirements"]
        );
        self.get_local(id)
    }

    pub(super) async fn handle(self: &Arc<Self>, request: Request) -> Result<Value> {
        match request {
            Request::Requirements {
                configuration,
                plan_id,
            } => {
                let plan = plan_id.map(|id| self.read_plan(&id)).transpose()?;
                let config = configuration
                    .or_else(|| plan.as_ref().map(|p| p.configuration.clone()))
                    .context("Select a plan and execution model.")?;
                self.requirements(&config, plan.as_ref()).await
            }
            Request::Create { configuration } => {
                let snapshot = test_plan::embedded()?.materialize(&configuration.profile_id)?;
                let request = serde_json::from_value(
                    json!({"label": configuration.label, "purpose": snapshot.profile.purpose,
                    "url": configuration.url, "model": configuration.model, "provider": configuration.provider,
                    "judge_model": configuration.judge_model, "judge_provider": configuration.judge_provider,
                    "scenarios": snapshot.scenario_ids, "runs": snapshot.profile.repetitions, "technical_retries": snapshot.profile.technical_retries,
                    "template_id": configuration.profile_id}),
                )?;
                let plan = self.create_local(request).await?;
                self.view(&self.read_plan(&plan.id)?)
            }
            Request::Update {
                plan_id,
                configuration,
            } => {
                let update = serde_json::from_value(
                    json!({"label": configuration.label, "url": configuration.url,
                    "model": configuration.model, "provider": configuration.provider, "judge_model": configuration.judge_model, "judge_provider": configuration.judge_provider}),
                )?;
                self.update_local(&plan_id, update).await?;
                self.view(&self.read_plan(&plan_id)?)
            }
            Request::Get { plan_id } => self.view(&self.read_plan(&plan_id)?),
            Request::Duplicate {
                plan_id,
                label,
                model,
                provider,
            } => {
                let mut request = super::plans::plan_request(&self.get_local(&plan_id)?);
                request.label = label;
                request.model = model;
                request.provider = provider;
                request.duplicate_of = Some(plan_id);
                let plan = self.create_local(request).await?;
                self.view(&self.read_plan(&plan.id)?)
            }
            Request::Export { plan_id } => export(&self.read_plan(&plan_id)?),
            Request::Start {
                plan_id,
                idempotency_key,
                role,
            } => self.start(&plan_id, &idempotency_key, role).await,
            Request::Execution { execution_id } => {
                Ok(serde_json::to_value(self.read_execution(&execution_id)?)?)
            }
            Request::Cancel { execution_id } => self.cancel(&execution_id).await,
            Request::Compare {
                plan_id,
                candidate_id,
            } => {
                let history = self.history(&plan_id)?;
                let baseline = history
                    .iter()
                    .rev()
                    .find(|e| matches!(e.role, Role::Baseline) && e.baseline_eligible)
                    .context("No complete, technically valid baseline is available.")?;
                let candidate = history
                    .iter()
                    .find(|e| {
                        e.id == candidate_id && matches!(e.role, Role::Candidate) && !e.active()
                    })
                    .context("Candidate execution is unavailable.")?;
                ensure!(
                    baseline.configuration_sha256 == candidate.configuration_sha256,
                    "Plan configurations differ"
                );
                let mut comparison = test_plan::compare_measurements(
                    &result_paths(baseline, &self.root),
                    &result_paths(candidate, &self.root),
                )?;
                let complete = baseline.baseline_eligible && candidate.baseline_eligible;
                comparison["coverage_complete"] = json!(complete);
                if !complete {
                    comparison["unavailable"] = json!("Efficiency deltas are unavailable because this execution does not have complete, technically valid coverage.");
                    if let Some(cohorts) = comparison["comparisons"].as_array_mut() {
                        for cohort in cohorts {
                            cohort["metrics"]["delta"] = Value::Null;
                        }
                    }
                }
                Ok(comparison)
            }
        }
    }
    async fn requirements(
        &self,
        config: &Configuration,
        plan: Option<&SavedPlan>,
    ) -> Result<Value> {
        let snapshot = match plan {
            Some(plan) => plan.snapshot.clone(),
            None => test_plan::embedded()?.materialize(&config.profile_id)?,
        };
        let validation = validate_config(config, &snapshot, &self.url);
        let mut checks = vec![check(
            "configuration",
            validation.is_ok(),
            validation
                .err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "Model and evaluator configuration is complete.".into()),
        )];
        if let Some(plan) = plan {
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
        checks.push(check("executor", !snapshot.protected_supervisor_required, if snapshot.protected_supervisor_required { "Export this plan for the protected Release Control executor. Dashboard execution is unavailable." } else { "Native Harness executor." }));
        let active = if let Some(runner) = &self.runner {
            let mut runtime_config = config.clone();
            runtime_config.profile_id = if snapshot
                .scenario_ids
                .iter()
                .any(|id| id == "shell_coder_sandbox")
            {
                "shell-coder-fixture"
            } else {
                "saved-plan"
            }
            .into();
            match runner.requirements(&runtime_config).await {
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
                    if let Ok(execution) = self.read_execution(id) {
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
        let mut plan = self.read_plan(plan_id)?;
        let id = format!("plan-{}", &artifact::sha256_bytes(key.as_bytes())[7..39]);
        if self.execution_path(&id)?.exists() {
            let existing = self.read_execution(&id)?;
            ensure!(
                existing.plan_id == plan_id
                    && existing.configuration_sha256 == plan.configuration_sha256
                    && (matches!(role, Role::Run)
                        || serde_json::to_value(&existing.role)? == serde_json::to_value(&role)?),
                "Idempotency key already belongs to a different plan request."
            );
            return Ok(json!({"execution_id": id, "duplicate": true, "execution": existing}));
        }
        verify_snapshot(&plan)?;
        let canonical = self.canonical(&plan)?;
        let has_baseline = canonical.baseline_execution_id.is_some();
        let role = match role {
            Role::Run if has_baseline => Role::Candidate,
            Role::Run => Role::Baseline,
            role => role,
        };
        ensure!(
            matches!(
                (&role, has_baseline),
                (Role::Baseline, false) | (Role::Candidate, true)
            ),
            "A plan captures one valid baseline before running candidates."
        );
        let preflight = self.requirements(&plan.configuration, Some(&plan)).await?;
        if preflight["ready"] != true {
            return Ok(json!({"blocked": true, "requirements": preflight}));
        }
        let runner = self.runner()?;
        runner.reserve(&id).await?;
        let mut execution = PlanExecution {
            schema: "harness-e2e-plan-execution/v1".into(),
            id: id.clone(),
            plan_id: plan_id.into(),
            idempotency_key: key.into(),
            configuration_sha256: plan.configuration_sha256.clone(),
            role,
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
        let persist = (|| -> Result<()> {
            execution.slots = materialize_slots(&plan, &id)?;
            // Lock first: if receipt persistence fails, no child has started.
            plan.locked = true;
            plan.updated_at = now();
            self.write_plan(&plan)?;
            self.write_execution(&execution)
        })();
        if let Err(error) = persist {
            runner.release(&id).await;
            return Err(error);
        }
        let manager = self.clone();
        let worker_id = id.clone();
        tokio::spawn(async move {
            if let Err(error) = manager.drive(&worker_id).await {
                tracing::error!(execution_id = %worker_id, error = %error, "plan coordinator stopped");
                // Keep admission until the active child has actually terminated.
                manager
                    .interrupt_after_error(&worker_id, &format!("{error:#}"))
                    .await;
            }
        });
        Ok(json!({"execution_id": id, "duplicate": false, "execution": execution}))
    }
    async fn drive(&self, id: &str) -> Result<()> {
        let runner = self.runner()?;
        let count = self.read_execution(id)?.slots.len();
        for index in 0..count {
            let child = {
                let _guard = self.lock.lock().await;
                let mut execution = self.read_execution(id)?;
                if execution.cancel_requested {
                    break;
                }
                let plan = self.read_plan(&execution.plan_id)?;
                verify_snapshot(&plan)?;
                ensure!(
                    plan.configuration_sha256 == execution.configuration_sha256,
                    "Plan identity changed during execution."
                );
                // This write must succeed before invoking native admission.
                execution.slots[index].state = "admitting".into();
                execution.updated_at = now();
                self.write_execution(&execution)?;
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
                    let mut execution = self.read_execution(id)?;
                    let plan = self.read_plan(&execution.plan_id)?;
                    if let Some(report) = record.report.as_ref().filter(|_| terminal) {
                        verify_system_identity(&mut execution.system_under_test, report)?;
                    }
                    update_slot(&mut execution.slots[index], &record, &plan, &self.root)?;
                    execution.updated_at = now();
                    self.write_execution(&execution)?;
                    if execution.cancel_requested && !terminal {
                        runner.cancel(&child).await?;
                    }
                }
                if terminal {
                    // Objective failures with a complete native report continue.
                    ensure!(
                        record
                            .report
                            .as_ref()
                            .is_some_and(|report| report.report_state == ReportState::Complete
                                && report.scenarios.iter().all(|s| s
                                    .aggregate
                                    .technical_invalid_runs
                                    == 0
                                    && s.aggregate.undetermined_runs == 0)),
                        "Native execution cannot continue safely: {}",
                        record.error
                    );
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        {
            let _guard = self.lock.lock().await;
            let mut execution = self.read_execution(id)?;
            finish(&mut execution, None, &self.root)?;
            self.write_execution(&execution)?;
        }
        runner.release(id).await;
        Ok(())
    }
    async fn cancel(&self, id: &str) -> Result<Value> {
        let _guard = self.lock.lock().await;
        let mut execution = self.read_execution(id)?;
        if execution.active() {
            execution.cancel_requested = true;
            execution.state = "cancelling".into();
            execution.updated_at = now();
            self.write_execution(&execution)?;
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
        let Ok(mut execution) = self.read_execution(id) else {
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
        if let Ok(latest) = self.read_execution(id) {
            execution = latest;
        }
        if let Ok(plan) = self.read_plan(&execution.plan_id) {
            for slot in &mut execution.slots {
                if let Some(record) = runner.record(&slot.execution_id).await {
                    let _ = update_slot(slot, &record, &plan, &self.root);
                }
            }
        }
        if let Err(error) = finish(&mut execution, Some(error.into()), &self.root)
            .and_then(|()| self.write_execution(&execution))
        {
            tracing::error!(%error, "cannot persist interruption; admission retained");
            return;
        }
        runner.release(id).await;
    }
    async fn reconcile(&self) -> Result<()> {
        for mut execution in self.executions()? {
            let was_active = execution.active();
            if !was_active
                && (execution.measurements.is_some()
                    || !execution.slots.iter().any(|s| s.result_path.is_some()))
            {
                continue;
            }
            let finished_at = execution.finished_at.clone();
            let plan = self.read_plan(&execution.plan_id)?;
            if let Some(runner) = &self.runner {
                for slot in &mut execution.slots {
                    if let Some(record) = runner.record(&slot.execution_id).await {
                        ensure!(
                            record.phase.terminal(),
                            "Native child is still active during plan recovery"
                        );
                        if let Err(error) = update_slot(slot, &record, &plan, &self.root) {
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
            self.write_execution(&execution)?;
        }
        Ok(())
    }
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
fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    artifact::write_atomic(path, &serde_json::to_vec_pretty(value)?)
}
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
fn prepared_plan(
    base: super::plans::LocalPlan,
    snapshot: Option<ProfileSnapshot>,
) -> Result<SavedPlan> {
    let snapshot = snapshot
        .map(Ok)
        .unwrap_or_else(|| snapshot_for_plan(&base))?;
    let configuration = Configuration {
        label: base.label.clone(),
        profile_id: snapshot.profile.id.clone(),
        url: base.url.clone(),
        model: base.model.clone(),
        provider: base.provider.clone(),
        judge_model: base.judge_model.clone(),
        judge_provider: base.judge_provider.clone(),
    };
    let snapshot_sha256 = artifact::sha256_value(&snapshot)?;
    Ok(SavedPlan {
        schema_version: 2,
        id: base.id.clone(),
        created_at: base.created_at.clone(),
        updated_at: base.updated_at.clone(),
        locked: base.locked,
        configuration_sha256: configuration_digest(&configuration, &snapshot_sha256)?,
        configuration,
        snapshot,
        snapshot_sha256,
        base: Some(base),
    })
}
fn snapshot_for_plan(plan: &super::plans::LocalPlan) -> Result<ProfileSnapshot> {
    let master = test_plan::embedded()?;
    let mut snapshot = master.materialize(plan.template_id.as_deref().unwrap_or("smoke"))?;
    snapshot.profile.id = "saved-plan".into();
    snapshot.profile.label = plan.label.clone();
    snapshot.profile.purpose = plan.purpose.clone();
    snapshot.profile.modules.clear();
    snapshot.profile.scenarios = plan.scenario_ids.clone();
    snapshot.profile.repetitions = plan.runs;
    snapshot.profile.technical_retries = plan.technical_retries;
    snapshot.profile.lane = "local".into();
    if plan.template_id.is_none() {
        snapshot.profile.fault_groups.clear();
    }
    snapshot.scenario_ids = plan.scenario_ids.clone();
    snapshot.cases = plan
        .scenarios
        .iter()
        .map(|case| {
            let mut value = serde_json::to_value(case)?;
            let key = case
                .scenario_id
                .parse::<crate::markdown::ScenarioKey>()
                .ok();
            value["judge_required"] =
                json!(key.as_ref().is_none_or(|key| key.built_in().is_none()));
            value["requirements"] = json!(master
                .requirements
                .get(&case.scenario_id)
                .cloned()
                .unwrap_or_default());
            Ok(value)
        })
        .collect::<Result<_>>()?;
    let catalog =
        crate::control::scenarios_list(crate::control::ScenariosListRequest { seed: plan.seed })?;
    let mut groups = Vec::new();
    for id in &plan.scenario_ids {
        let key = id.parse::<crate::markdown::ScenarioKey>().ok();
        groups.push(json!({"id": format!("case-{}", id.replace('_', "-")), "scenarios": [id], "runs": 1,
            "technical_retries": if key.as_ref().is_some_and(|key| key.execution_kind().replay_safe()) { plan.technical_retries } else { 0 }, "difficulty_weight": catalog.scenarios.iter().find(|case| case.scenario_id.as_str() == id).map(|case| test_plan::weight(case.classification.tier)),
            "execution_kind": key.as_ref().map(test_plan::execution_kind).unwrap_or("harness_turn")}));
    }
    for fault in &snapshot.profile.fault_groups {
        groups.push(serde_json::to_value(fault)?);
    }
    snapshot.campaigns = (1..=plan.runs).map(|round| json!({"kind": "harness-e2e-campaign", "campaign_id": format!("saved-plan-r{round:02}"),
        "lane": "local", "failure_policy": "advisory", "scoring_profile": "difficulty-weighted-v1", "groups": groups})).collect();
    let fault_runs: u64 = snapshot
        .profile
        .fault_groups
        .iter()
        .map(|g| u64::from(g.runs))
        .sum::<u64>()
        * u64::from(plan.runs);
    snapshot.budget = json!({"planned_runs": plan.scenario_ids.len() as u64 * u64::from(plan.runs) + fault_runs, "scenario_runs": plan.scenario_ids.len() as u64 * u64::from(plan.runs), "fault_runs": fault_runs});
    snapshot.protected_supervisor_required = fault_runs > 0;
    snapshot.profile_sha256 =
        artifact::sha256_value(&json!({"profile": snapshot.profile, "cases": snapshot.cases}))?;
    Ok(snapshot)
}

fn configuration_digest(config: &Configuration, snapshot_digest: &str) -> Result<String> {
    artifact::sha256_value(&json!({"configuration": config, "snapshot_sha256": snapshot_digest}))
}
fn judge_required(snapshot: &ProfileSnapshot) -> bool {
    snapshot.protected_supervisor_required
        || snapshot.cases.iter().any(|c| c["judge_required"] == true)
}
fn validate_config(config: &Configuration, snapshot: &ProfileSnapshot, url: &str) -> Result<()> {
    ensure!(
        !config.label.trim().is_empty() && config.label.len() <= 160,
        "Enter a plan name (up to 160 characters)."
    );
    ensure!(
        !config.model.trim().is_empty() && !config.provider.trim().is_empty(),
        "Select an execution model."
    );
    ensure!(
        config.judge_model.is_empty() == config.judge_provider.is_empty(),
        "Select both evaluator model and provider."
    );
    ensure!(
        !judge_required(snapshot) || !config.judge_model.trim().is_empty(),
        "The selected work requires an evaluator."
    );
    ensure!(
        config.profile_id == snapshot.profile.id,
        "Profile identity differs from its snapshot."
    );
    ensure!(
        config.url == url,
        "This plan belongs to a different stack endpoint."
    );
    Ok(())
}
fn verify_snapshot(plan: &SavedPlan) -> Result<()> {
    ensure!(
        artifact::sha256_value(&plan.snapshot)? == plan.snapshot_sha256
            && configuration_digest(&plan.configuration, &plan.snapshot_sha256)?
                == plan.configuration_sha256,
        "Saved plan configuration or snapshot digest differs."
    );
    for expected in &plan.snapshot.cases {
        let id = expected["scenario_id"]
            .as_str()
            .context("Saved scenario missing")?;
        let seed = expected["seed"].as_u64().context("Saved seed missing")?;
        let current = super::plans::resolve_scope(&[id.into()], Some(seed))?;
        let current = &current[0];
        ensure!(current.case_id == expected["case_id"] && current.inputs_sha256 == expected["inputs_sha256"]
            && current.contract_sha256 == expected["contract_sha256"],
            "Saved scenario contract is unavailable in this runner. Consultation and export remain available.");
    }
    Ok(())
}
fn materialize_slots(plan: &SavedPlan, owner: &str) -> Result<Vec<Slot>> {
    ensure!(
        !plan.snapshot.protected_supervisor_required,
        "Protected executor required"
    );
    let mut slots = Vec::new();
    for (round, campaign) in plan.snapshot.campaigns.iter().enumerate() {
        for group in campaign["groups"]
            .as_array()
            .context("Missing campaign groups")?
        {
            let group_id = group["id"].as_str().context("Missing group identity")?;
            let scenario_id = group["scenarios"][0]
                .as_str()
                .context("Native scenario required")?;
            let key = format!("{owner}:round-{}:{group_id}", round + 1);
            let c = &plan.configuration;
            // Use the saved seed. Native materialization and envelopes are
            // checked again independently for every invocation.
            let request: RunRequest = serde_json::from_value(
                json!({"idempotency_key": key, "label": format!("{} · round {} · {}", c.label, round+1, scenario_id), "lane": campaign["lane"], "model": c.model, "provider": c.provider,
                "judge_model": if c.judge_model.is_empty() { None } else { Some(&c.judge_model) }, "judge_provider": if c.judge_provider.is_empty() { None } else { Some(&c.judge_provider) },
                "scenarios": [scenario_id], "runs": 1, "seed": plan.snapshot.cases.iter().find(|case| case["scenario_id"] == scenario_id).map(|case| &case["seed"]), "technical_retries": group["technical_retries"]}),
            )?;
            crate::control::validate_run_request(&request)?;
            slots.push(Slot {
                round: round as u32 + 1,
                group_id: group_id.into(),
                scenario_id: scenario_id.into(),
                execution_id: execution_id_for_key(&key),
                request: serde_json::to_value(request)?,
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
    ensure!(
        Some(slots.len() as u64) == plan.snapshot.budget["planned_runs"].as_u64(),
        "Materialized slot coverage differs"
    );
    Ok(slots)
}
fn update_slot(
    slot: &mut Slot,
    record: &ExecutionRecord,
    plan: &SavedPlan,
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
    let config = &plan.configuration;
    ensure!(
        report.subject.model == config.model && report.subject.provider == config.provider,
        "Execution model identity differs"
    );
    ensure!(
        report
            .judge
            .as_ref()
            .map(|j| (j.model.as_str(), j.provider.as_str()))
            == (!config.judge_model.is_empty())
                .then_some((config.judge_model.as_str(), config.judge_provider.as_str())),
        "Evaluator identity differs"
    );
    ensure!(
        report.scenarios.len() == 1,
        "A child must contain exactly one scenario"
    );
    let scenario = &report.scenarios[0];
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
    let aggregate = &scenario.aggregate;
    ensure!(
        aggregate.planned_runs == 1,
        "Native slot count differs from admission"
    );
    slot.observed = aggregate.observed_runs;
    slot.completed = aggregate.completed_runs;
    slot.passed = aggregate.passed_runs;
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
        ensure!(
            left == right,
            "Stack or runner identity changed during the composed execution"
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

fn project_measurements(value: &mut Value, execution: &PlanExecution, root: &Path) -> Result<()> {
    let Some(cohorts) = execution
        .measurements
        .as_ref()
        .and_then(|m| m["cohorts"].as_array())
    else {
        return Ok(());
    };
    let sum =
        |path: &str| -> Option<f64> { cohorts.iter().map(|c| c.pointer(path)?.as_f64()).sum() };
    let completed = sum("/aggregate/completed_runs");
    let tokens = sum("/aggregate/total_tokens_consumed");
    value["totals"]["total_tokens"] = json!(tokens);
    value["totals"]["failed_attempt_tokens"] = json!(sum("/aggregate/failed_attempt_tokens"));
    value["totals"]["total_cost_usd"] = json!(sum("/aggregate/cost/total_usd"));
    value["totals"]["tokens_per_completion"] = json!(tokens
        .zip(completed)
        .filter(|(_, n)| *n > 0.0)
        .map(|(tokens, n)| tokens / n));
    value["totals"]["scenario_pass_rate"] =
        json!(sum("/aggregate/passed_runs")
            .map(|passed| passed / execution.slots.len().max(1) as f64));
    value["totals"]["hard_gate_failures"] = json!(sum("/aggregate/hard_gate_failures"));
    value["totals"]["technical_failures"] = json!(sum("/aggregate/technical_failures"));
    // Reuse the native presenter for additive metrics; never derive totals from medians.
    let native = execution
        .slots
        .iter()
        .filter_map(|slot| {
            let run = super::store::read_stored_run(&root.join(&slot.execution_id))
                .ok()
                .flatten()?;
            super::presenter::stored_execution_summary(&run).ok()
        })
        .collect::<Vec<_>>();
    for key in [
        "wall_time_seconds",
        "function_calls",
        "function_call_errors",
        "turns",
    ] {
        let total: Option<f64> = (!native.is_empty())
            .then(|| native.iter().map(|v| v["totals"][key].as_f64()).sum())
            .flatten();
        value["totals"][key] = json!(total);
    }
    let mut scenarios = Vec::new();
    let mut metrics = Vec::new();
    for cohort in cohorts {
        let aggregate = &cohort["aggregate"];
        let id = &cohort["scenario_id"];
        let count = aggregate["observed_runs"].as_f64().unwrap_or(0.0);
        scenarios.push(json!({"id": id, "case_id": cohort["identity"]["case"]["case_id"], "runs": count,
            "status": if aggregate["passed_runs"] == aggregate["planned_runs"] { "passed" } else { "failed" },
            "passed": aggregate["passed_runs"] == aggregate["planned_runs"], "pass_rate": aggregate["pass_rate"],
            "hard_gate_failures": aggregate["hard_gate_failures"], "technical_failures": aggregate["technical_failures"],
            "median_score": aggregate["median_score"], "total_cost_usd": aggregate["cost"]["total_usd"]}));
        let contract = json!({"case_id": cohort["identity"]["case"]["case_id"], "case": cohort["identity"]["case"],
            "scenario_id": id, "scenario_version": cohort["identity"]["case"]["scenario_version"], "execution_policy": cohort["identity"]["execution_policy"]});
        metrics.push(json!({"subject_id": value["subjects"][0]["id"], "scenario_id": id,
            "scenario_version": cohort["identity"]["case"]["scenario_version"], "contract_fingerprint": super::presenter::contract_fingerprint(&contract), "run_count": count,
            "averages": {"tokens": aggregate["total_tokens_consumed"].as_f64().filter(|_| count > 0.0).map(|tokens| tokens / count), "tokens_per_completion": aggregate["tokens_per_completion"]},
            "samples": {"tokens": if aggregate["total_tokens_consumed"].is_number() { count } else { 0.0 }, "tokens_per_completion": aggregate["completed_runs"]}}));
    }
    value["subjects"][0]["scenarios"] = json!(scenarios);
    value["scenario_metrics"] = json!(metrics);
    value["baseline_comparable"] = json!(execution.baseline_eligible);
    Ok(())
}

fn result_paths(execution: &PlanExecution, root: &Path) -> Vec<PathBuf> {
    execution
        .slots
        .iter()
        .filter_map(|s| s.result_path.as_ref().map(|path| root.join(path)))
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
    Ok(())
}
pub(super) fn execution_summary(execution: &PlanExecution) -> Value {
    json!({"id": execution.id, "plan_id": execution.plan_id, "state": execution.state, "role": execution.role, "started_at": execution.started_at, "finished_at": execution.finished_at, "planned": execution.slots.len(),
        "finished": execution.slots.iter().filter(|s| s.state == "finished").count(), "observed": execution.slots.iter().map(|s| s.observed).sum::<u32>(), "completed": execution.slots.iter().map(|s| s.completed).sum::<u32>(), "passed": execution.slots.iter().map(|s| s.passed).sum::<u32>(), "technical_valid": execution.slots.iter().map(|s| s.technical_valid).sum::<u32>(), "baseline_eligible": execution.baseline_eligible, "error": execution.error,
        "active_slot": execution.slots.iter().find(|s| matches!(s.state.as_str(), "running" | "admitting"))})
}
fn export(plan: &SavedPlan) -> Result<Value> {
    let suites: Vec<_> = plan.snapshot.campaigns.iter().map(|campaign| {
        let groups: Vec<_> = campaign["groups"].as_array().into_iter().flatten().map(|g| { let mut g = g.clone(); if let Some(object) = g.as_object_mut() { if let Some(weight) = object.remove("difficulty_weight") { object.insert("weight".into(), weight); } } g }).collect();
        json!({"id": campaign["campaign_id"], "label": plan.snapshot.profile.label, "lane": campaign["lane"], "seed": null, "subject": {"model": plan.configuration.model, "provider": plan.configuration.provider}, "judge": {"model": plan.configuration.judge_model, "provider": plan.configuration.judge_provider}, "groups": groups, "test_plan": campaign["test_plan"]})
    }).collect();
    Ok(
        json!({"schema": "harness-e2e-profile-campaigns/v1", "plan_id": plan.snapshot.plan_id, "version": plan.snapshot.version, "definition_sha256": plan.snapshot.definition_sha256,
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
                fail_receipt: AtomicBool::new(false),
            }
        }
        fn native_record(&self, request: RunRequest) -> Result<ExecutionRecord> {
            let id = execution_id_for_key(&request.idempotency_key);
            let scenario = &request.scenarios[0];
            let (case, policy) = match scenario.built_in() {
                Some(key) => {
                    let materialized =
                        key.materialize("profile-test", scenario.canonical_seed())?;
                    (materialized.case, materialized.spec.execution)
                }
                None => {
                    let markdown = crate::markdown::embedded_catalog()?
                        .into_iter()
                        .find(|c| c.id == scenario.as_str())
                        .unwrap();
                    (
                        crate::suite::markdown_case(&markdown, scenario.canonical_seed())?,
                        crate::markdown::execution_policy(),
                    )
                }
            };
            let mut run = E2eRunReport::new(
                format!("{id}-run"),
                format!("{id}-attempt"),
                1,
                format!("{id}-session"),
                "prompt".into(),
            );
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
            let scenario = E2eScenarioReport::aggregate_case(case, policy, vec![run]);
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
                context_window: 128000,
                max_output_tokens: 4096,
                supports_tools: Some(true),
                supports_vision: None,
            };
            let subject = model(request.model.clone(), request.provider.clone());
            let judge = request
                .judge_model
                .clone()
                .zip(request.judge_provider.clone())
                .map(|(m, p)| model(m, p));
            let manifest = E2eManifest {
                execution: execution.clone(),
                system_under_test: system.clone(),
                subject: subject.clone(),
                judge: judge.clone(),
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
            let mut report = E2eReport::new(
                execution,
                system,
                subject,
                judge,
                Some("profile-test".into()),
                None,
                vec![scenario],
            );
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
                    max_cases: 1,
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
                manifest: Some(manifest),
                observation: None,
                observation_artifact: None,
                archive: None,
            })
        }
    }
    #[async_trait]
    impl Runner for FakeRunner {
        async fn requirements(&self, _: &Configuration) -> Result<Vec<Check>> {
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
                fs::remove_dir(self.root.join("plan-executions"))?;
                fs::write(self.root.join("plan-executions"), b"unwritable")?;
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
                    .join("plan-executions")
                    .join(format!("{owner}.json")),
            )?)?;
            let id = execution_id_for_key(&request.idempotency_key);
            ensure!(
                receipt.slots.iter().any(|slot| slot.execution_id == id
                    && slot.request == serde_json::to_value(&request).unwrap()),
                "child was not persisted before dispatch"
            );
            self.submitted.fetch_add(1, Ordering::SeqCst);
            let record = self.native_record(request)?;
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
    }
    fn config(profile: &str) -> Configuration {
        Configuration {
            label: "Profile test".into(),
            profile_id: profile.into(),
            url: "ws://localhost:49134".into(),
            model: "model".into(),
            provider: "provider".into(),
            judge_model: "judge".into(),
            judge_provider: "provider".into(),
        }
    }
    fn manager(root: &Path, runner: Arc<FakeRunner>) -> Arc<PlanStore> {
        fs::create_dir_all(root.join("plans")).unwrap();
        fs::create_dir_all(root.join("plan-executions")).unwrap();
        Arc::new(PlanStore {
            root: root.into(),
            url: config("smoke").url,
            runner: Some(runner),
            lock: Mutex::new(()),
        })
    }
    async fn terminal(manager: &PlanStore, id: &str) -> PlanExecution {
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let execution = manager.read_execution(id).unwrap();
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
        let plan = manager
            .handle(Request::Create {
                configuration: config(profile),
            })
            .await
            .unwrap();
        let id = plan["id"].as_str().unwrap().to_string();
        let response = manager
            .start(
                &id,
                key,
                if profile == "evolution" {
                    Role::Baseline
                } else {
                    Role::Run
                },
            )
            .await
            .unwrap();
        (id, response["execution_id"].as_str().unwrap().into())
    }
    #[test]
    fn native_measurements_count_retry_consumption_once_and_reject_reused_attempts() {
        let root = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new(root.path().into());
        let efficiency = |tokens| {
            serde_json::from_value(json!({"wall_time_ms": 0, "root_turns": 1, "child_turns": 0, "child_sessions": 0, "function_calls": 1, "function_call_errors": 0, "validation_retries": 0, "transient_resumes": 0, "wake_resumes": 0, "effective_fan_out": 0, "critical_path_ms": 0, "input_tokens": tokens, "output_tokens": 0, "total_tokens": tokens, "cost_usd": null, "minimum_expected_work": 1, "observed_work": 1, "work_amplification": 1.0, "technical_attempts": 1, "observed_complexity": {}})).unwrap()
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
    async fn templates_and_legacy_plans_share_scope_edits_and_baseline_candidate_lifecycle() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let request = serde_json::from_value(json!({"label": "Edited Smoke", "purpose": "Custom scope from a template", "url": config("smoke").url,
            "model": "model", "provider": "provider", "judge_model": "judge", "judge_provider": "provider",
            "scenarios": ["minimal_path", "persistent_state"], "runs": 2, "technical_retries": 0, "template_id": "smoke"})).unwrap();
        let plan = manager.create_local(request).await.unwrap();
        assert_eq!(plan.scenario_ids.len(), 2);
        assert_eq!(plan.template_id.as_deref(), Some("smoke"));
        let mut saved = manager.read_plan(&plan.id).unwrap();
        // A starting template's revision is provenance, not an execution dependency.
        saved.snapshot.version += 1;
        saved.snapshot.definition_sha256 = "historical-template-revision".into();
        saved.snapshot_sha256 = artifact::sha256_value(&saved.snapshot).unwrap();
        saved.configuration_sha256 =
            configuration_digest(&saved.configuration, &saved.snapshot_sha256).unwrap();
        manager.write_plan(&saved).unwrap();
        let started = manager
            .start_local(&plan.id, super::super::plans::PlanRunRole::Baseline)
            .await
            .unwrap();
        let baseline = terminal(&manager, started.last_attempt_id.as_deref().unwrap()).await;
        assert_eq!(baseline.slots.len(), 4);
        super::super::presenter::validate_execution_id(&baseline.id).unwrap();
        for invalid in [
            "plan-../results",
            "plan-short",
            "plan-0000000000000000000000000000000/",
        ] {
            assert!(super::super::presenter::validate_execution_id(invalid).is_err());
        }
        let detail = manager.execution_detail(&baseline.id).unwrap().unwrap();
        assert_eq!(detail["id"], baseline.id);
        assert_eq!(detail["native_execution_ids"].as_array().unwrap().len(), 4);
        assert!(detail["totals"]["scenario_pass_rate"].is_number());
        assert!(baseline.baseline_eligible);
        let ready = manager.get_local(&plan.id).unwrap();
        assert_eq!(
            ready.baseline_execution_id.as_deref(),
            Some(baseline.id.as_str())
        );
        assert!(manager
            .update_local(
                &plan.id,
                serde_json::from_value(json!({"runs": 3})).unwrap()
            )
            .await
            .is_err());
        let started = manager
            .start_local(&plan.id, super::super::plans::PlanRunRole::Candidate)
            .await
            .unwrap();
        let candidate = terminal(&manager, started.last_attempt_id.as_deref().unwrap()).await;
        assert_eq!(
            manager.get_local(&plan.id).unwrap().candidate_execution_ids,
            vec![candidate.id]
        );
        let mut legacy_request = super::super::plans::plan_request(&ready);
        legacy_request.template_id = None;
        legacy_request.runs = 1;
        legacy_request.scenarios = vec!["minimal_path".into()];
        let mut legacy =
            super::super::plans::new_plan(&legacy_request, "retained-native-plan".into()).unwrap();
        legacy.baseline_execution_id = Some("retained-native-baseline".into());
        legacy
            .candidate_execution_ids
            .push("retained-native-candidate".into());
        legacy.locked = true;
        legacy.state = super::super::plans::PlanState::ComparisonReady;
        super::super::plans::write_plan(&root.path().join("plans"), &legacy).unwrap();
        let started = manager
            .start_local(&legacy.id, super::super::plans::PlanRunRole::Candidate)
            .await
            .unwrap();
        let candidate = terminal(&manager, started.last_attempt_id.as_deref().unwrap()).await;
        let retained = manager.get_local(&legacy.id).unwrap();
        assert_eq!(
            retained.baseline_execution_id.as_deref(),
            Some("retained-native-baseline")
        );
        assert_eq!(
            retained.candidate_execution_ids,
            vec!["retained-native-candidate".to_owned(), candidate.id]
        );
        assert_eq!(manager.list_local().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn legacy_plan_bytes_and_history_are_preserved_beside_v2() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner);
        let request = serde_json::from_value(json!({"label": "Legacy", "url": config("smoke").url, "model": "model", "provider": "provider", "judge_model": "judge", "judge_provider": "provider", "scenarios": ["minimal_path"], "runs": 1, "technical_retries": 0})).unwrap();
        let mut legacy = super::super::plans::new_plan(&request, "legacy-retained".into()).unwrap();
        legacy.baseline_execution_id = Some("baseline-retained".into());
        legacy
            .candidate_execution_ids
            .push("candidate-retained".into());
        super::super::plans::write_plan(&root.path().join("plans"), &legacy).unwrap();
        let path = root.path().join("plans/legacy-retained.json");
        let original = fs::read(&path).unwrap();
        manager
            .handle(Request::Create {
                configuration: config("smoke"),
            })
            .await
            .unwrap();
        manager.reconcile().await.unwrap();
        fs::write(
            root.path().join("plans/unsupported.json"),
            br#"{"schema_version":3}"#,
        )
        .unwrap();
        fs::write(root.path().join("plans/corrupt.json"), b"invalid json").unwrap();
        let listed = manager.list_local().unwrap();
        assert_eq!(listed.len(), 2);
        let retained = listed.iter().find(|plan| plan.id == legacy.id).unwrap();
        assert_eq!(retained.candidate_execution_ids, vec!["candidate-retained"]);
        assert_eq!(
            retained.baseline_execution_id.as_deref(),
            Some("baseline-retained")
        );
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[tokio::test]
    async fn profile_contracts_are_pinned_and_duplicates_have_no_execution_state() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner);
        for profile in test_plan::PROFILE_IDS {
            let value = manager
                .handle(Request::Create {
                    configuration: config(profile),
                })
                .await
                .unwrap();
            let id = value["id"].as_str().unwrap();
            let mut plan = manager.read_plan(id).unwrap();
            verify_snapshot(&plan).unwrap();
            let round_trip: SavedPlan =
                serde_json::from_slice(&fs::read(manager.plan_path(id).unwrap()).unwrap()).unwrap();
            assert_eq!(round_trip.snapshot_sha256, plan.snapshot_sha256);
            let copy = manager
                .handle(Request::Duplicate {
                    plan_id: id.into(),
                    label: "Copy".into(),
                    model: "another".into(),
                    provider: "another-provider".into(),
                })
                .await
                .unwrap();
            assert_ne!(copy["id"], id);
            assert_eq!(copy["history"], json!([]));
            assert_eq!(copy["locked"], false);
            assert_eq!(copy["snapshot"], value["snapshot"]);
            assert_eq!(copy["configuration"]["judge_model"], "judge");
            assert!(copy["baseline_execution_id"].is_null());
            plan.snapshot.profile.repetitions += 1;
            assert!(verify_snapshot(&plan).is_err());
            manager.write_plan(&plan).unwrap();
            assert_eq!(manager.view(&plan).unwrap()["compatible"], false);
            assert!(export(&plan).is_ok());
        }
        let mut missing = config("smoke");
        missing.model.clear();
        assert!(manager
            .handle(Request::Create {
                configuration: missing
            })
            .await
            .is_err());
        let mut missing = config("smoke");
        missing.judge_model.clear();
        missing.judge_provider.clear();
        assert!(manager
            .handle(Request::Create {
                configuration: missing
            })
            .await
            .is_err());
    }
    #[tokio::test]
    async fn native_coordination_covers_all_slots_including_capability_and_evolution() {
        for (profile, expected) in [
            ("smoke", 5),
            ("regression", 12),
            ("capability", 47),
            ("evolution", 90),
            ("endurance", 5),
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
            assert_eq!(execution.slots.len(), expected);
            assert_eq!(runner.submitted.load(Ordering::SeqCst), expected);
            assert!(execution.baseline_eligible); // Objective failure is independent from validity.
            assert_eq!(
                execution.slots.iter().map(|s| s.passed).sum::<u32>(),
                expected as u32 - 1
            );
            let cohorts = execution.measurements.as_ref().unwrap()["cohorts"]
                .as_array()
                .unwrap()
                .clone();
            assert_eq!(
                cohorts.len(),
                manager
                    .read_plan(&plan_id)
                    .unwrap()
                    .snapshot
                    .scenario_ids
                    .len()
            );
            if profile == "evolution" {
                assert!(cohorts.iter().all(|c| c["aggregate"]["observed_runs"] == 5));
                let paths = result_paths(&execution, root.path());
                let comparison = test_plan::compare_measurements(&paths, &paths).unwrap();
                assert_eq!(comparison["comparisons"].as_array().unwrap().len(), 18);
                for cohort in comparison["comparisons"].as_array().unwrap() {
                    assert_eq!(
                        cohort["metrics"]["from_run_ids"].as_array().unwrap().len(),
                        5
                    );
                    assert!(
                        cohort["metrics"]["from"]["consumption"]["total_tokens_consumed"].is_null()
                    );
                }
                assert!(test_plan::measure(&[paths[0].clone(), paths[0].clone()]).is_err());
            }
            let repeated = manager
                .start(
                    &plan_id,
                    profile,
                    if profile == "evolution" {
                        Role::Baseline
                    } else {
                        Role::Run
                    },
                )
                .await
                .unwrap();
            assert_eq!(repeated["duplicate"], true);
            assert!(manager
                .handle(Request::Update {
                    plan_id,
                    configuration: config(profile)
                })
                .await
                .is_err());
        }
    }
    #[tokio::test]
    async fn cancellation_reserves_admission_and_prevents_next_groups() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        runner.hold.store(true, Ordering::SeqCst);
        let manager = manager(root.path(), runner.clone());
        let (plan, id) = admitted(&manager, "smoke", "cancel").await;
        let duplicate = manager.start(&plan, "cancel", Role::Run).await.unwrap();
        assert_eq!(duplicate["duplicate"], true);
        let other = manager.start(&plan, "another", Role::Run).await.unwrap();
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
            let (plan, id) = admitted(&manager, "evolution", "missing").await;
            let execution = terminal(&manager, &id).await;
            assert_eq!(execution.state, "interrupted");
            assert!(!execution.baseline_eligible);
            assert_eq!(runner.submitted.load(Ordering::SeqCst), 1);
            assert!(manager.view(&manager.read_plan(&plan).unwrap()).unwrap()
                ["baseline_execution_id"]
                .is_null());
        }
    }
    #[tokio::test]
    async fn persistence_failure_never_dispatches_a_child() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner.clone());
        let plan = manager
            .handle(Request::Create {
                configuration: config("smoke"),
            })
            .await
            .unwrap();
        runner.fail_receipt.store(true, Ordering::SeqCst);
        assert!(manager
            .start(plan["id"].as_str().unwrap(), "fail-write", Role::Run)
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
        let (plan, id) = admitted(&manager, "smoke", "restart").await;
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
        manager.write_execution(&receipt).unwrap();
        let before = runner.submitted.load(Ordering::SeqCst);
        manager.reconcile().await.unwrap();
        let recovered = manager.read_execution(&id).unwrap();
        assert_eq!(recovered.state, "interrupted");
        assert_eq!(recovered.slots[0].state, "finished");
        assert_eq!(recovered.slots[2].state, "not_run");
        assert_eq!(runner.submitted.load(Ordering::SeqCst), before);
        assert!(
            manager.view(&manager.read_plan(&plan).unwrap()).unwrap()["baseline_execution_id"]
                .is_null()
        );
    }
    #[tokio::test]
    async fn resilience_export_is_accepted_by_the_existing_protected_suite_validator() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new(root.path().into()));
        let manager = manager(root.path(), runner);
        let plan = manager
            .handle(Request::Create {
                configuration: config("resilience"),
            })
            .await
            .unwrap();
        let saved = manager.read_plan(plan["id"].as_str().unwrap()).unwrap();
        assert_eq!(saved.snapshot.budget["planned_runs"], 13);
        let export = export(&saved).unwrap();
        let path = root.path().join("export.json");
        write_json(&path, &export).unwrap();
        let result = std::process::Command::new("python3").args(["-c", "import json,sys; sys.path.insert(0,'scripts'); from exact_stack_campaign import validate_suite; v=json.load(open(sys.argv[1])); [validate_suite(s) for s in v['release_control_suites']]", path.to_str().unwrap()]).output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(materialize_slots(&saved, "protected").is_err());
        assert_eq!(
            manager
                .requirements(&saved.configuration, Some(&saved))
                .await
                .unwrap()["ready"],
            false
        );
    }
}
