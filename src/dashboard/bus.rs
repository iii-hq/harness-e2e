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
use super::presenter::{
    execution_detail_value, repository_url, validate_execution_id, MAX_EXECUTIONS,
};
use super::store::read_stored_run;
use super::RunRequest;
use crate::context::E2eContext;
use crate::scenarios::ScenarioId;

pub(super) const EXECUTIONS_LIST: &str = "e2e::dashboard::executions-list";
pub(super) const EXECUTION_GET: &str = "e2e::dashboard::execution-get";
pub(super) const CATALOG_GET: &str = "e2e::dashboard::catalog-get";
pub(super) const RUN_STATUS: &str = "e2e::dashboard::run-status";
pub(super) const RUN_START: &str = "e2e::dashboard::run-start";
pub(super) const RUN_CANCEL: &str = "e2e::dashboard::run-cancel";
pub(super) const CHANGED_TRIGGER: &str = "e2e::dashboard::changed";
pub(super) const BROWSER_FUNCTION_PREFIX: &str = "iii::harness-e2e-dashboard::";

const CONTRACT_NAME: &str = "harness-e2e-dashboard";
const CONTRACT_VERSION: u32 = 1;
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
    pub schema_version: u32,
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
        RegisterFunction::new_async(move |_request: Value| {
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
                "version": CONTRACT_VERSION,
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
            (ids.is_empty()
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
        schema_version: 4,
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
    let report = run.report.context("execution report not found")?;
    let detail = execution_detail_value(&run.metadata, &report)?;
    Ok(ExecutionBundle { manifest, detail })
}

pub(super) async fn catalog(
    controller: &Controller,
    request: CatalogRequest,
    connected: Option<&IIIClient>,
) -> Result<Value> {
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
            let scenarios: Vec<_> = ScenarioId::ALL.iter().map(|value| value.as_str()).collect();
            return Ok(json!({ "url": url, "models": models, "scenarios": scenarios }));
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
        let scenarios: Vec<_> = ScenarioId::ALL.iter().map(|value| value.as_str()).collect();
        Ok(json!({ "url": url, "models": models, "scenarios": scenarios }))
    }
    .await;
    context.shutdown().await;
    result
}

fn function_ids(listed: &Value) -> impl Iterator<Item = &str> {
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
