use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::RunRequest;
use crate::artifact;
use crate::scenarios::ScenarioId;

const PLAN_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum PlanState {
    Draft,
    BaselineRunning,
    BaselineReady,
    CandidateRunning,
    ComparisonReady,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum PlanRunRole {
    Baseline,
    Candidate,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub(super) struct PlanContext {
    pub plan_id: String,
    pub role: PlanRunRole,
    pub plan_hash: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
pub(super) struct PlanScopeItem {
    pub scenario_id: String,
    pub scenario_version: u32,
    pub case_id: String,
    pub seed: u64,
    pub inputs_sha256: String,
    pub contract_sha256: String,
    pub complexity_tier: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
pub(super) struct LocalPlan {
    pub schema_version: u32,
    pub id: String,
    pub label: String,
    pub purpose: String,
    pub created_at: String,
    pub updated_at: String,
    pub state: PlanState,
    pub locked: bool,
    pub scope_hash: String,
    pub policy_hash: String,
    pub url: String,
    pub model: String,
    pub provider: String,
    pub judge_model: String,
    pub judge_provider: String,
    pub scenarios: Vec<PlanScopeItem>,
    pub scenario_ids: Vec<String>,
    pub runs: u32,
    pub technical_retries: u8,
    pub seed: Option<u64>,
    pub baseline_execution_id: Option<String>,
    pub candidate_execution_ids: Vec<String>,
    pub incomplete_execution_ids: Vec<String>,
    pub last_attempt_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct PlanCreateRequest {
    // iii adds this routing metadata when a browser/worker invokes the
    // function. It is accepted at the boundary but never enters the plan.
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    _caller_worker_id: Option<String>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub purpose: String,
    pub url: String,
    pub model: String,
    pub provider: String,
    #[serde(default)]
    pub judge_model: String,
    #[serde(default)]
    pub judge_provider: String,
    pub scenarios: Vec<String>,
    #[serde(default = "default_runs")]
    pub runs: u32,
    #[serde(default = "default_retries")]
    pub technical_retries: u8,
    #[serde(default)]
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct PlanUpdateRequest {
    // See PlanCreateRequest::_caller_worker_id. Keep the update DTO strict for
    // user fields while allowing the engine's internal invocation metadata.
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    _caller_worker_id: Option<String>,
    #[serde(default)]
    pub plan_id: Option<String>,
    pub label: Option<String>,
    pub purpose: Option<String>,
    pub url: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub judge_model: Option<String>,
    pub judge_provider: Option<String>,
    pub scenarios: Option<Vec<String>>,
    pub runs: Option<u32>,
    pub technical_retries: Option<u8>,
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub(super) struct PlanRunRequest {
    #[serde(default)]
    pub plan_id: Option<String>,
    pub role: PlanRunRole,
}

fn default_runs() -> u32 {
    1
}

fn default_retries() -> u8 {
    1
}

pub(super) fn plans_dir(runs_dir: &Path) -> PathBuf {
    runs_dir.join("plans")
}

pub(super) fn write_plan(plans_dir: &Path, plan: &LocalPlan) -> Result<()> {
    fs::create_dir_all(plans_dir)
        .with_context(|| format!("create plans directory {}", plans_dir.display()))?;
    let target = plans_dir.join(format!("{}.json", plan.id));
    let temporary = plans_dir.join(format!("{}.json.tmp", plan.id));
    let mut bytes = serde_json::to_vec_pretty(plan)?;
    bytes.push(b'\n');
    fs::write(&temporary, bytes).with_context(|| format!("write {}", temporary.display()))?;
    fs::rename(&temporary, &target).with_context(|| format!("replace {}", target.display()))?;
    Ok(())
}

pub(super) fn read_plan(plans_dir: &Path, id: &str) -> Result<Option<LocalPlan>> {
    let path = plans_dir.join(format!("{id}.json"));
    if !path.is_file() {
        return Ok(None);
    }
    let plan: LocalPlan = serde_json::from_slice(&fs::read(&path)?)
        .with_context(|| format!("decode {}", path.display()))?;
    if plan.schema_version != PLAN_SCHEMA_VERSION {
        bail!(
            "unsupported local plan schema version {}",
            plan.schema_version
        );
    }
    Ok(Some(plan))
}

pub(super) fn list_plans(plans_dir: &Path) -> Result<Vec<LocalPlan>> {
    if !plans_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut plans = Vec::new();
    for entry in fs::read_dir(plans_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file()
            || entry.path().extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }
        let path = entry.path();
        let id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if let Some(plan) = read_plan(plans_dir, id)? {
            plans.push(plan);
        }
    }
    plans.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    Ok(plans)
}

pub(super) fn new_plan(request: &PlanCreateRequest, id: String) -> Result<LocalPlan> {
    validate_values(request)?;
    let scenarios = resolve_scope(&request.scenarios, request.seed)?;
    let policy_hash = current_policy_hash()?;
    let scope_hash = scope_hash(request, &scenarios, &policy_hash)?;
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    Ok(LocalPlan {
        schema_version: PLAN_SCHEMA_VERSION,
        id,
        label: request.label.trim().to_string(),
        purpose: request.purpose.trim().to_string(),
        created_at: now.clone(),
        updated_at: now,
        state: PlanState::Draft,
        locked: false,
        scope_hash,
        policy_hash,
        url: request.url.trim().to_string(),
        model: request.model.trim().to_string(),
        provider: request.provider.trim().to_string(),
        judge_model: request.judge_model.trim().to_string(),
        judge_provider: request.judge_provider.trim().to_string(),
        scenario_ids: request.scenarios.clone(),
        scenarios,
        runs: request.runs,
        technical_retries: request.technical_retries,
        seed: request.seed,
        baseline_execution_id: None,
        candidate_execution_ids: Vec::new(),
        incomplete_execution_ids: Vec::new(),
        last_attempt_id: None,
    })
}

pub(super) fn apply_update(plan: &mut LocalPlan, update: &PlanUpdateRequest) -> Result<()> {
    if plan.locked {
        bail!("local plan is locked after its baseline started");
    }
    let mut request = plan_request(plan);
    if let Some(value) = &update.label {
        request.label = value.clone();
    }
    if let Some(value) = &update.purpose {
        request.purpose = value.clone();
    }
    if let Some(value) = &update.url {
        request.url = value.clone();
    }
    if let Some(value) = &update.model {
        request.model = value.clone();
    }
    if let Some(value) = &update.provider {
        request.provider = value.clone();
    }
    if let Some(value) = &update.judge_model {
        request.judge_model = value.clone();
    }
    if let Some(value) = &update.judge_provider {
        request.judge_provider = value.clone();
    }
    if let Some(value) = &update.scenarios {
        request.scenarios = value.clone();
    }
    if let Some(value) = update.runs {
        request.runs = value;
    }
    if let Some(value) = update.technical_retries {
        request.technical_retries = value;
    }
    if let Some(value) = update.seed {
        request.seed = Some(value);
    }
    validate_values(&request)?;
    let scenarios = resolve_scope(&request.scenarios, request.seed)?;
    plan.label = request.label.trim().to_string();
    plan.purpose = request.purpose.trim().to_string();
    plan.url = request.url.trim().to_string();
    plan.model = request.model.trim().to_string();
    plan.provider = request.provider.trim().to_string();
    plan.judge_model = request.judge_model.trim().to_string();
    plan.judge_provider = request.judge_provider.trim().to_string();
    plan.scenario_ids = request.scenarios.clone();
    plan.scenarios = scenarios;
    plan.runs = request.runs;
    plan.technical_retries = request.technical_retries;
    plan.seed = request.seed;
    plan.policy_hash = current_policy_hash()?;
    plan.scope_hash = scope_hash(&request, &plan.scenarios, &plan.policy_hash)?;
    plan.updated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    Ok(())
}

pub(super) fn plan_request(plan: &LocalPlan) -> PlanCreateRequest {
    PlanCreateRequest {
        _caller_worker_id: None,
        label: plan.label.clone(),
        purpose: plan.purpose.clone(),
        url: plan.url.clone(),
        model: plan.model.clone(),
        provider: plan.provider.clone(),
        judge_model: plan.judge_model.clone(),
        judge_provider: plan.judge_provider.clone(),
        scenarios: plan.scenario_ids.clone(),
        runs: plan.runs,
        technical_retries: plan.technical_retries,
        seed: plan.seed,
    }
}

pub(super) fn run_request(plan: &LocalPlan, role: PlanRunRole) -> RunRequest {
    RunRequest {
        _caller_worker_id: None,
        label: format!(
            "{} · {}",
            plan.label,
            match role {
                PlanRunRole::Baseline => "baseline",
                PlanRunRole::Candidate => "candidate",
            }
        ),
        url: plan.url.clone(),
        model: plan.model.clone(),
        provider: plan.provider.clone(),
        judge_model: plan.judge_model.clone(),
        judge_provider: plan.judge_provider.clone(),
        scenarios: plan.scenario_ids.clone(),
        runs: plan.runs,
        technical_retries: plan.technical_retries,
        seed: plan.seed,
        plan_context: Some(PlanContext {
            plan_id: plan.id.clone(),
            role,
            plan_hash: plan.scope_hash.clone(),
        }),
        workflow_definition: None,
        workflow_hash: None,
    }
}

pub(super) fn record_incomplete_attempt(
    plans_dir: &Path,
    context: &PlanContext,
    execution_id: &str,
) -> Result<()> {
    let Some(mut plan) = read_plan(plans_dir, &context.plan_id)? else {
        return Ok(());
    };
    if plan.scope_hash != context.plan_hash {
        bail!("plan context hash does not match the frozen scope");
    }
    if !plan
        .incomplete_execution_ids
        .iter()
        .any(|id| id == execution_id)
    {
        plan.incomplete_execution_ids.push(execution_id.to_string());
    }
    plan.last_attempt_id = Some(execution_id.to_string());
    plan.state = match context.role {
        PlanRunRole::Baseline => PlanState::Draft,
        PlanRunRole::Candidate if plan.baseline_execution_id.is_some() => PlanState::BaselineReady,
        PlanRunRole::Candidate => PlanState::Draft,
    };
    plan.updated_at = now();
    write_plan(plans_dir, &plan)
}

pub(super) fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn validate_values(request: &PlanCreateRequest) -> Result<()> {
    if request.scenarios.is_empty() {
        bail!("select at least one test for the local plan");
    }
    if request.runs == 0 || request.runs > 20 {
        bail!("runs must be between 1 and 20");
    }
    if request.technical_retries > 3 {
        bail!("technical_retries must be between 0 and 3");
    }
    if request.url.trim().is_empty()
        || request.model.trim().is_empty()
        || request.provider.trim().is_empty()
    {
        bail!("url, model, and provider are required");
    }
    let ids = request
        .scenarios
        .iter()
        .map(|value| value.trim())
        .collect::<std::collections::BTreeSet<_>>();
    if ids.len() != request.scenarios.len() {
        bail!("plan scenarios must be unique");
    }
    for value in ids {
        if !ScenarioId::ALL.iter().any(|id| id.as_str() == value) {
            bail!("unknown scenario '{value}'");
        }
    }
    Ok(())
}

fn resolve_scope(scenario_ids: &[String], seed: Option<u64>) -> Result<Vec<PlanScopeItem>> {
    scenario_ids
        .iter()
        .map(|value| {
            let id = ScenarioId::ALL
                .iter()
                .copied()
                .find(|candidate| candidate.as_str() == value)
                .with_context(|| format!("unknown scenario '{value}'"))?;
            let case_seed = seed.unwrap_or_else(|| id.canonical_seed());
            let materialized = id.materialize("local-plan", case_seed)?;
            let contract_sha256 = artifact::sha256_value(&json!({
                "scenario_id": materialized.case.scenario_id,
                "scenario_version": materialized.case.scenario_version,
                "case": materialized.case,
                "execution_policy": materialized.spec.execution,
            }))?;
            Ok(PlanScopeItem {
                scenario_id: id.as_str().into(),
                scenario_version: materialized.case.scenario_version,
                case_id: materialized.case.case_id,
                seed: materialized.case.seed,
                inputs_sha256: materialized.case.inputs_sha256,
                contract_sha256,
                complexity_tier: format!("{:?}", materialized.case.complexity.tier),
            })
        })
        .collect()
}

fn scope_hash(
    request: &PlanCreateRequest,
    scenarios: &[PlanScopeItem],
    policy_hash: &str,
) -> Result<String> {
    artifact::sha256_value(&json!({
        "url": request.url.trim(),
        "model": request.model.trim(),
        "provider": request.provider.trim(),
        "judge_model": request.judge_model.trim(),
        "judge_provider": request.judge_provider.trim(),
        "scenarios": scenarios,
        "runs": request.runs,
        "technical_retries": request.technical_retries,
        "policy_hash": policy_hash,
    }))
}

fn current_policy_hash() -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(include_str!("../../config/policies/capability-policy.json"))?;
    artifact::sha256_value(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> PlanCreateRequest {
        PlanCreateRequest {
            _caller_worker_id: None,
            label: "local plan".into(),
            purpose: "exercise the dashboard workflow".into(),
            url: "ws://127.0.0.1:49134".into(),
            model: "model".into(),
            provider: "provider".into(),
            judge_model: "judge".into(),
            judge_provider: "judge-provider".into(),
            scenarios: vec![ScenarioId::DirectAnswer.as_str().into()],
            runs: 1,
            technical_retries: 1,
            seed: None,
        }
    }

    #[test]
    fn creates_a_small_unlocked_scope_by_default() {
        let plan = new_plan(&request(), "plan-test".into()).expect("plan should materialize");
        assert_eq!(plan.state, PlanState::Draft);
        assert!(!plan.locked);
        assert_eq!(plan.runs, 1);
        assert_eq!(plan.scenarios.len(), 1);
        assert_eq!(plan.scenarios[0].scenario_version, 2);
        assert_eq!(
            plan.scenarios[0].seed,
            ScenarioId::DirectAnswer.canonical_seed()
        );
        assert!(plan.baseline_execution_id.is_none());
    }

    #[test]
    fn accepts_engine_caller_metadata_without_persisting_it() {
        let mut value = serde_json::to_value(request()).expect("request should serialize");
        value["_caller_worker_id"] = serde_json::json!("browser-worker");
        let decoded: PlanCreateRequest =
            serde_json::from_value(value).expect("engine metadata should be accepted");
        assert_eq!(decoded._caller_worker_id.as_deref(), Some("browser-worker"));

        let serialized = serde_json::to_value(decoded).expect("request should serialize");
        assert!(serialized.get("_caller_worker_id").is_none());
    }

    #[test]
    fn writes_and_reads_plans_atomically() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let plan = new_plan(&request(), "plan-round-trip".into()).expect("plan should materialize");
        write_plan(directory.path(), &plan).expect("plan should be written");
        assert!(!directory.path().join("plan-round-trip.json.tmp").exists());
        assert_eq!(
            read_plan(directory.path(), &plan.id).unwrap(),
            Some(plan.clone())
        );
        assert_eq!(list_plans(directory.path()).unwrap(), vec![plan]);
    }

    #[test]
    fn locked_plans_reject_scope_changes() {
        let mut plan = new_plan(&request(), "plan-locked".into()).expect("plan should materialize");
        plan.locked = true;
        let update = PlanUpdateRequest {
            label: Some("changed".into()),
            ..PlanUpdateRequest::default()
        };
        let error = apply_update(&mut plan, &update).expect_err("locked plan must be immutable");
        assert!(error.to_string().contains("locked"));
    }

    #[test]
    fn run_requests_carry_the_frozen_plan_context() {
        let plan = new_plan(&request(), "plan-context".into()).expect("plan should materialize");
        let run = run_request(&plan, PlanRunRole::Candidate);
        let context = run.plan_context.expect("plan context");
        assert_eq!(context.plan_id, plan.id);
        assert_eq!(context.plan_hash, plan.scope_hash);
        assert_eq!(context.role, PlanRunRole::Candidate);
    }

    #[test]
    fn recovery_records_an_incomplete_attempt_without_promoting_it() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut plan =
            new_plan(&request(), "plan-recovery".into()).expect("plan should materialize");
        plan.locked = true;
        plan.state = PlanState::BaselineRunning;
        write_plan(directory.path(), &plan).expect("plan should be written");
        let context = PlanContext {
            plan_id: plan.id.clone(),
            role: PlanRunRole::Baseline,
            plan_hash: plan.scope_hash.clone(),
        };
        record_incomplete_attempt(directory.path(), &context, "local-recovered")
            .expect("recovery should update the plan");
        let recovered = read_plan(directory.path(), &plan.id)
            .expect("plan should be readable")
            .expect("plan should exist");
        assert_eq!(recovered.state, PlanState::Draft);
        assert_eq!(recovered.incomplete_execution_ids, vec!["local-recovered"]);
        assert!(recovered.baseline_execution_id.is_none());
    }
}
