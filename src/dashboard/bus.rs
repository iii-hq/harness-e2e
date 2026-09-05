use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use iii_sdk::errors::Error;
use iii_sdk::protocol::{TriggerAction, TriggerRequest};
use iii_sdk::runtime::WorkerMetadata;
use iii_sdk::trigger::{TriggerConfig, TriggerHandler};
use iii_sdk::{register_worker, IIIClient, InitOptions, RegisterFunction, RegisterTriggerType};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::controller::Controller;
use super::plans::{PlanCreateRequest, PlanRunRequest, PlanUpdateRequest};
use super::presenter::{
    repository_url, stored_execution_detail, validate_execution_id, MAX_EXECUTIONS,
};
use super::read_model::{
    EvaluatedVersionsRequest, EvaluatedVersionsResponse, TestHistoryRequest, TestHistoryResponse,
    TestVersionGetRequest, TestVersionResult, TestsListRequest, TestsListResponse,
};
use super::store::read_stored_run;
use super::RunRequest;
use crate::catalog::CatalogModel;
use crate::context::E2eContext;
use crate::control::{LocalScenarioCreateRequest, LocalScenarioCreateResponse, ScenarioOrigin};

pub(super) const EXECUTIONS_LIST: &str = "e2e::dashboard::executions-list";
pub(super) const EXECUTION_GET: &str = "e2e::dashboard::execution-get";
pub(super) const EVALUATED_VERSIONS_LIST: &str = "e2e::dashboard::evaluated-versions-list";
pub(super) const TESTS_LIST: &str = "e2e::dashboard::tests-list";
pub(super) const TEST_VERSION_GET: &str = "e2e::dashboard::test-version-get";
pub(super) const TEST_HISTORY_GET: &str = "e2e::dashboard::test-history-get";
pub(super) const CATALOG_GET: &str = "e2e::dashboard::catalog-get";
pub(super) const LOCAL_SCENARIO_CREATE: &str = "e2e::dashboard::local-scenario-create";
pub(super) const PROFILE_PLAN: &str = "e2e::dashboard::profile-plan";
pub(super) const PLANS_LIST: &str = "e2e::dashboard::plans-list";
pub(super) const PLAN_GET: &str = "e2e::dashboard::plan-get";
pub(super) const PLAN_CREATE: &str = "e2e::dashboard::plan-create";
pub(super) const PLAN_UPDATE: &str = "e2e::dashboard::plan-update";
pub(super) const PLAN_RUN_START: &str = "e2e::dashboard::plan-run-start";
pub(super) const RUN_STATUS: &str = "e2e::dashboard::run-status";
pub(super) const RUN_START: &str = "e2e::dashboard::run-start";
pub(super) const RUN_CANCEL: &str = "e2e::dashboard::run-cancel";
pub(super) const CHANGED_TRIGGER: &str = "e2e::dashboard::changed";
pub(super) const BROWSER_FUNCTION_PREFIX: &str = "iii::harness-e2e-dashboard::";

const CONTRACT_NAME: &str = "harness-e2e-dashboard";
const DEFAULT_PAGE_SIZE: u16 = 25;

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub(super) struct ExecutionListRequest {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub ids: Vec<String>,
    #[serde(default)]
    pub ids_csv: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct Retention {
    summaries: usize,
    details: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct ExecutionListResponse {
    pub mode: String,
    pub last_update: String,
    pub repo_url: String,
    pub retention: Retention,
    pub executions: Vec<Value>,
    pub total: usize,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(super) struct ExecutionGetRequest {
    pub execution_id: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct ExecutionBundle {
    pub manifest: ExecutionListResponse,
    pub detail: Value,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub(super) struct CatalogRequest {
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
struct DashboardEmptyRequest {
    #[serde(rename = "_caller_worker_id", default)]
    #[schemars(skip)]
    _caller_worker_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct PlanGetRequest {
    #[serde(rename = "_caller_worker_id", default)]
    #[schemars(skip)]
    _caller_worker_id: Option<String>,
    plan_id: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct PlansListResponse {
    mode: String,
    plans: Vec<super::plans::LocalPlan>,
    master_plan: Value,
    profile_plans: Vec<Value>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(super) struct CatalogResponse {
    url: String,
    models: Vec<CatalogModel>,
    scenarios: Vec<String>,
    local_scenarios: Vec<LocalScenarioSummary>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(super) struct LocalScenarioSummary {
    id: String,
    title: String,
    version: u32,
    source_path: String,
    source_sha256: String,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub(super) struct RunStatusRequest {
    #[serde(default)]
    pub after: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EventBindingConfig {}

#[derive(Clone, Default)]
struct Subscribers {
    values: Arc<Mutex<HashMap<String, TriggerConfig>>>,
}

impl Subscribers {
    fn insert(&self, config: TriggerConfig) {
        self.values
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(config.id.clone(), config);
    }

    fn remove(&self, id: &str) {
        self.values
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(id);
    }

    fn targets(&self) -> Vec<String> {
        self.values
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .values()
            .map(|config| config.function_id.clone())
            .collect()
    }
}

struct ChangedHandler {
    subscribers: Subscribers,
}

#[async_trait]
impl TriggerHandler for ChangedHandler {
    async fn register_trigger(&self, config: TriggerConfig) -> std::result::Result<(), Error> {
        let raw = if config.config.is_null() {
            json!({})
        } else {
            config.config.clone()
        };
        serde_json::from_value::<EventBindingConfig>(raw)
            .map_err(|error| Error::Handler(format!("invalid dashboard event config: {error}")))?;
        self.subscribers.insert(config);
        Ok(())
    }

    async fn unregister_trigger(&self, config: TriggerConfig) -> std::result::Result<(), Error> {
        self.subscribers.remove(&config.id);
        Ok(())
    }
}

pub(super) struct DashboardEvents {
    iii: IIIClient,
    subscribers: Subscribers,
}

impl DashboardEvents {
    pub(super) fn register(iii: &IIIClient) -> Arc<Self> {
        let subscribers = Subscribers::default();
        let _ = iii.register_trigger_type(
            RegisterTriggerType::new(
                CHANGED_TRIGGER,
                "A local E2E execution started, produced progress, or reached a terminal state.",
                ChangedHandler {
                    subscribers: subscribers.clone(),
                },
            )
            .trigger_request_format::<EventBindingConfig>(),
        );
        Arc::new(Self {
            iii: iii.clone(),
            subscribers,
        })
    }

    pub(super) async fn emit(&self, kind: &str, execution_id: &str) {
        let payload = json!({
            "kind": kind,
            "execution_id": execution_id,
            "at": chrono::Utc::now().to_rfc3339(),
        });
        for function_id in self.subscribers.targets() {
            if let Err(error) = self
                .iii
                .trigger(TriggerRequest {
                    function_id: function_id.clone(),
                    payload: payload.clone(),
                    action: Some(TriggerAction::Void),
                    timeout_ms: None,
                })
                .await
            {
                tracing::debug!(%error, %function_id, "deliver dashboard change signal");
            }
        }
    }
}

pub(super) fn connect(url: &str) -> Arc<IIIClient> {
    Arc::new(register_worker(
        url,
        InitOptions {
            metadata: Some(WorkerMetadata {
                runtime: "rust".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                name: "harness-e2e-dashboard".into(),
                os: std::env::consts::OS.into(),
                pid: Some(std::process::id()),
                ..WorkerMetadata::default()
            }),
            ..InitOptions::default()
        },
    ))
}

pub(super) fn register_functions(iii: &IIIClient, controller: Arc<Controller>) {
    register(
        iii,
        EXECUTIONS_LIST,
        "List compact retained execution summaries with server-side filters and cursor pagination.",
        {
            let controller = controller.clone();
            RegisterFunction::new_async(move |request: ExecutionListRequest| {
                let controller = controller.clone();
                async move {
                    execution_list(&controller, request)
                        .await
                        .map_err(handler_error)
                }
            })
        },
    );
    register(
        iii,
        EXECUTION_GET,
        "Read one retained execution summary and its diagnostic report.",
        {
            let controller = controller.clone();
            RegisterFunction::new_async(move |request: ExecutionGetRequest| {
                let controller = controller.clone();
                async move {
                    execution_bundle(&controller, request)
                        .await
                        .map_err(handler_error)
                }
            })
        },
    );
    register(
        iii,
        EVALUATED_VERSIONS_LIST,
        "List immutable evaluated-system versions and their exact evaluation cohorts.",
        {
            let controller = controller.clone();
            RegisterFunction::new_async(move |request: EvaluatedVersionsRequest| {
                let controller = controller.clone();
                async move {
                    evaluated_versions(&controller, request)
                        .await
                        .map_err(handler_error)
                }
            })
        },
    );
    register(
        iii,
        TESTS_LIST,
        "List the versioned test catalog with compact A/B results and stable pagination.",
        {
            let controller = controller.clone();
            RegisterFunction::new_async(move |request: TestsListRequest| {
                let controller = controller.clone();
                async move {
                    tests_list(&controller, request)
                        .await
                        .map_err(handler_error)
                }
            })
        },
    );
    register(
        iii,
        TEST_VERSION_GET,
        "Read one test version across two immutable evaluated-system versions.",
        {
            let controller = controller.clone();
            RegisterFunction::new_async(move |request: TestVersionGetRequest| {
                let controller = controller.clone();
                async move {
                    test_version_get(&controller, request)
                        .await
                        .map_err(handler_error)
                }
            })
        },
    );
    register(
        iii,
        TEST_HISTORY_GET,
        "Read local metric history for one test version, with provider-grouped execution and judge models, without comparison actions.",
        {
            let controller = controller.clone();
            RegisterFunction::new_async(move |request: TestHistoryRequest| {
                let controller = controller.clone();
                async move {
                    test_history(&controller, request)
                        .await
                        .map_err(handler_error)
                }
            })
        },
    );
    register(
        iii,
        PLANS_LIST,
        "List local plans and their baseline/candidate lifecycle.",
        {
            let controller = controller.clone();
            RegisterFunction::new_async(move |_request: DashboardEmptyRequest| {
                let controller = controller.clone();
                async move {
                    let plans = controller.list_plans().await.map_err(handler_error)?;
                    Ok(PlansListResponse {
                        mode: "local".into(),
                        plans,
                        profile_plans: controller.profile_plans.list().map_err(handler_error)?,
                        master_plan: crate::test_plan::embedded()
                            .and_then(|plan| plan.catalog())
                            .map_err(handler_error)?,
                    })
                }
            })
        },
    );
    register(iii, PROFILE_PLAN, "Configure, export and execute pinned profile plans, and inspect or cancel their composed executions.", {
        let controller = controller.clone();
        RegisterFunction::new_async(move |request: super::profile_plans::Request| {
            let controller = controller.clone();
            async move { controller.profile_plans.handle(request).await.map_err(handler_error) }
        })
    });
    register(iii, PLAN_GET, "Read one local plan.", {
        let controller = controller.clone();
        RegisterFunction::new_async(move |request: PlanGetRequest| {
            let controller = controller.clone();
            async move {
                controller
                    .get_plan(&request.plan_id)
                    .await
                    .map_err(handler_error)
            }
        })
    });
    register(
        iii,
        PLAN_CREATE,
        "Create a draft local plan with an explicit small test scope.",
        {
            let controller = controller.clone();
            RegisterFunction::new_async(move |request: PlanCreateRequest| {
                let controller = controller.clone();
                async move {
                    controller
                        .create_plan(request)
                        .await
                        .map_err(|error| Error::Handler(error.message))
                }
            })
        },
    );
    register(
        iii,
        PLAN_UPDATE,
        "Update an unlocked local plan or rename retained candidates.",
        {
            let controller = controller.clone();
            RegisterFunction::new_async(move |request: PlanUpdateRequest| {
                let controller = controller.clone();
                async move {
                    let id = request
                        .plan_id
                        .clone()
                        .ok_or_else(|| handler_error("plan_id is required"))?;
                    controller
                        .update_plan(&id, request)
                        .await
                        .map_err(|error| Error::Handler(error.message))
                }
            })
        },
    );
    register(
        iii,
        PLAN_RUN_START,
        "Start a baseline or candidate from a locked local plan.",
        {
            let controller = controller.clone();
            RegisterFunction::new_async(move |request: PlanRunRequest| {
                let controller = controller.clone();
                async move {
                    let id = request
                        .plan_id
                        .as_deref()
                        .ok_or_else(|| handler_error("plan_id is required"))?;
                    controller
                        .start_plan(id, request.role)
                        .await
                        .map_err(|error| Error::Handler(error.message))
                }
            })
        },
    );
    register(
        iii,
        CATALOG_GET,
        "Read models and scenarios when the execution dialog opens.",
        {
            let controller = controller.clone();
            let iii = iii.clone();
            RegisterFunction::new_async(move |request: CatalogRequest| {
                let controller = controller.clone();
                let iii = iii.clone();
                async move {
                    catalog(&controller, request, Some(&iii))
                        .await
                        .map_err(handler_error)
                }
            })
        },
    );
    register(
        iii,
        LOCAL_SCENARIO_CREATE,
        "Validate and save one local-only Markdown scenario, then return its compiled identity.",
        {
            let controller = controller.clone();
            RegisterFunction::new_async(move |request: LocalScenarioCreateRequest| {
                let controller = controller.clone();
                async move {
                    controller
                        .create_local_scenario(request)
                        .await
                        .map_err(handler_error)
                }
            })
        },
    );
    register(
        iii,
        RUN_STATUS,
        "Read local execution state and only the unread log suffix.",
        {
            let controller = controller.clone();
            RegisterFunction::new_async(move |request: RunStatusRequest| {
                let controller = controller.clone();
                async move {
                    controller
                        .snapshot(request.after)
                        .await
                        .map_err(handler_error)
                }
            })
        },
    );
    register(iii, RUN_START, "Start one local E2E execution.", {
        let controller = controller.clone();
        RegisterFunction::new_async(move |request: RunRequest| {
            let controller = controller.clone();
            async move {
                controller
                    .start(request)
                    .await
                    .map_err(|error| Error::Handler(error.message))?;
                controller.snapshot(Some(0)).await.map_err(handler_error)
            }
        })
    });
    register(iii, RUN_CANCEL, "Cancel the active local E2E execution.", {
        let controller = controller.clone();
        RegisterFunction::new_async(move |_request: DashboardEmptyRequest| {
            let controller = controller.clone();
            async move {
                controller
                    .cancel()
                    .await
                    .map_err(|error| Error::Handler(error.message))?;
                controller.snapshot(None).await.map_err(handler_error)
            }
        })
    });
}

fn register(iii: &IIIClient, id: &str, description: &str, function: RegisterFunction) {
    iii.register_function(
        id,
        function.description(description).metadata(json!({
            "internal": true,
            "contract": {
                "name": CONTRACT_NAME,
                "capability": id.trim_start_matches("e2e::dashboard::"),
            }
        })),
    );
}

pub(super) async fn execution_list(
    controller: &Controller,
    request: ExecutionListRequest,
) -> Result<ExecutionListResponse> {
    let limit = request.limit.unwrap_or(DEFAULT_PAGE_SIZE);
    if limit == 0 || usize::from(limit) > MAX_EXECUTIONS {
        bail!("execution list limit must be between 1 and {MAX_EXECUTIONS}");
    }
    let offset = request
        .cursor
        .as_deref()
        .unwrap_or("0")
        .parse::<usize>()
        .context("execution list cursor is invalid")?;
    let query = request.query.unwrap_or_default().trim().to_lowercase();
    let status = normalized_filter(request.status);
    let event = normalized_filter(request.event);
    let mut ids = request.ids;
    ids.extend(
        request
            .ids_csv
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    );
    if ids.len() > MAX_EXECUTIONS {
        bail!("execution id filter exceeds {MAX_EXECUTIONS} entries");
    }
    for id in &ids {
        validate_execution_id(id).map_err(anyhow::Error::msg)?;
    }

    let all = controller.execution_summaries().await?;
    let filtered = all
        .iter()
        .filter(|execution| {
            ((ids.is_empty() && execution.get("parent_plan_execution_id").is_none())
                || execution
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| ids.iter().any(|candidate| candidate == id)))
                && status
                    .as_deref()
                    .is_none_or(|wanted| string_field(execution, "status") == wanted)
                && event
                    .as_deref()
                    .is_none_or(|wanted| string_field(execution, "event") == wanted)
                && (query.is_empty() || execution_haystack(execution).contains(&query))
        })
        .cloned()
        .collect::<Vec<_>>();
    let total = filtered.len();
    let executions = filtered
        .into_iter()
        .skip(offset)
        .take(usize::from(limit))
        .collect::<Vec<_>>();
    let end = offset.saturating_add(executions.len());
    let last_update = all
        .first()
        .and_then(|value| value.get("completed_at"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(ExecutionListResponse {
        mode: "local".into(),
        last_update,
        repo_url: repository_url(),
        retention: Retention {
            summaries: MAX_EXECUTIONS,
            details: MAX_EXECUTIONS,
        },
        executions,
        total,
        next_cursor: (end < total).then(|| end.to_string()),
    })
}

pub(super) async fn execution_bundle(
    controller: &Controller,
    request: ExecutionGetRequest,
) -> Result<ExecutionBundle> {
    validate_execution_id(&request.execution_id).map_err(anyhow::Error::msg)?;
    let manifest = execution_list(
        controller,
        ExecutionListRequest {
            ids: vec![request.execution_id.clone()],
            limit: Some(1),
            ..ExecutionListRequest::default()
        },
    )
    .await?;
    if manifest.executions.is_empty() {
        bail!("execution not found");
    }
    let run_dir = controller.runs_dir().join(&request.execution_id);
    let run = tokio::task::spawn_blocking(move || read_stored_run(&run_dir))
        .await
        .context("read execution task")??
        .context("execution not found")?;
    let detail = stored_execution_detail(&run)?;
    Ok(ExecutionBundle { manifest, detail })
}

pub(super) async fn evaluated_versions(
    controller: &Controller,
    request: EvaluatedVersionsRequest,
) -> Result<EvaluatedVersionsResponse> {
    Ok(controller.read_model().await?.evaluated_versions(request))
}

pub(super) async fn tests_list(
    controller: &Controller,
    request: TestsListRequest,
) -> Result<TestsListResponse> {
    controller.read_model().await?.tests_list(request)
}

pub(super) async fn test_version_get(
    controller: &Controller,
    request: TestVersionGetRequest,
) -> Result<TestVersionResult> {
    controller.read_model().await?.test_version_get(request)
}

pub(super) async fn test_history(
    controller: &Controller,
    request: TestHistoryRequest,
) -> Result<TestHistoryResponse> {
    controller.read_model().await?.test_history(request)
}

pub(super) async fn catalog(
    controller: &Controller,
    request: CatalogRequest,
    connected: Option<&IIIClient>,
) -> Result<CatalogResponse> {
    let url = request
        .url
        .unwrap_or_else(|| controller.default_url().to_string());
    super::controller::validate_stack_url(&url)?;
    if url == controller.default_url() {
        if let Some(iii) = connected {
            let functions = iii
                .trigger(TriggerRequest {
                    function_id: "engine::functions::list".into(),
                    payload: json!({ "include_internal": true }),
                    action: None,
                    timeout_ms: Some(5_000),
                })
                .await?;
            let ids = function_ids(&functions).collect::<Vec<_>>();
            if !ids.contains(&"harness::send") {
                bail!("connected iii stack does not expose harness::send");
            }
            if !ids.contains(&"router::models::list") {
                bail!("connected Harness stack does not expose router::models::list; start its llm-router");
            }
            let models = crate::catalog::list_with_client(iii, None).await?;
            if models.is_empty() {
                bail!("the running Harness has no registered models");
            }
            let scenario_catalog = controller.scenario_catalog().await?;
            let scenarios = scenario_catalog
                .scenarios
                .iter()
                .map(|scenario| scenario.scenario_id.to_string())
                .collect();
            let local_scenarios = local_scenario_summaries(&scenario_catalog.scenarios);
            return Ok(CatalogResponse {
                url,
                models,
                scenarios,
                local_scenarios,
            });
        }
    }
    let context = E2eContext::connect(&url).await?;
    let result = async {
        if !context.function_exists("harness::send").await? {
            bail!("connected iii stack does not expose harness::send");
        }
        if !context.function_exists("router::models::list").await? {
            bail!("connected Harness stack does not expose router::models::list; start its llm-router");
        }
        let models = crate::catalog::list(&context, None).await?;
        if models.is_empty() {
            bail!("the running Harness has no registered models");
        }
        let (scenarios, local_scenarios) = if url == controller.default_url() {
            let scenario_catalog = controller.scenario_catalog().await?;
            (
                scenario_catalog
                    .scenarios
                    .iter()
                    .map(|scenario| scenario.scenario_id.to_string())
                    .collect(),
                local_scenario_summaries(&scenario_catalog.scenarios),
            )
        } else {
            (
                crate::markdown::all_keys()?
                    .into_iter()
                    .map(|value| value.to_string())
                    .collect(),
                Vec::new(),
            )
        };
        Ok(CatalogResponse {
            url,
            models,
            scenarios,
            local_scenarios,
        })
    }
    .await;
    context.shutdown().await;
    result
}

fn local_scenario_summaries(
    scenarios: &[crate::control::ScenarioDescriptor],
) -> Vec<LocalScenarioSummary> {
    scenarios
        .iter()
        .filter(|scenario| scenario.origin == ScenarioOrigin::Local)
        .map(|scenario| LocalScenarioSummary {
            id: scenario.scenario_id.to_string(),
            title: scenario
                .title
                .clone()
                .unwrap_or_else(|| scenario.scenario_id.to_string()),
            version: scenario.scenario_version,
            source_path: scenario.source_path.clone().unwrap_or_default(),
            source_sha256: scenario.source_sha256.clone().unwrap_or_default(),
        })
        .collect()
}

pub(super) async fn local_scenario_create(
    controller: &Controller,
    request: LocalScenarioCreateRequest,
) -> Result<LocalScenarioCreateResponse> {
    controller.create_local_scenario(request).await
}

pub(super) fn function_ids(listed: &Value) -> impl Iterator<Item = &str> {
    listed
        .as_array()
        .or_else(|| listed.as_object()?.values().find_map(Value::as_array))
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.as_str()
                .or_else(|| item.get("function_id").and_then(Value::as_str))
                .or_else(|| item.get("id").and_then(Value::as_str))
        })
}

fn normalized_filter(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty() && value != "all")
}

fn string_field<'a>(execution: &'a Value, field: &str) -> &'a str {
    execution
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn execution_haystack(execution: &Value) -> String {
    let source = execution.get("source").unwrap_or(&Value::Null);
    [
        string_field(execution, "label"),
        string_field(execution, "id"),
        string_field(execution, "run_id"),
        source
            .get("sha")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        source
            .get("ref")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        string_field(execution, "completed_at"),
        string_field(execution, "started_at"),
    ]
    .join(" ")
    .to_lowercase()
}

fn handler_error(error: impl std::fmt::Display) -> Error {
    Error::Handler(error.to_string())
}
