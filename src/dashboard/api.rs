use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{FromRef, Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use super::assets::{javascript_response, static_asset};
use super::bus::{
    self, CatalogRequest, ExecutionBundle, ExecutionGetRequest, ExecutionListRequest,
    ExecutionListResponse, RunStatusRequest,
};
use super::controller::{validate_stack_url, Controller};
use super::presenter::{
    execution_detail_value, repository_url, validate_execution_id, MAX_EXECUTIONS,
};
use super::store::read_stored_run;
use super::{ApiError, DashboardArgs, RunRequest, RunSnapshot};

const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Clone)]
struct AppState {
    controller: Arc<Controller>,
    view_only: bool,
    engine_url: Arc<String>,
}

impl FromRef<AppState> for Arc<String> {
    fn from_ref(state: &AppState) -> Self {
        state.engine_url.clone()
    }
}

#[derive(Debug, Deserialize)]
struct CatalogQuery {
    url: Option<String>,
}

pub(super) async fn serve(args: DashboardArgs) -> Result<()> {
    let listen = args.listen;
    let view_only = args.view_only;
    let engine_url = Arc::new(args.url.clone());
    let iii = (!view_only).then(|| bus::connect(&engine_url));
    let events = iii.as_deref().map(bus::DashboardEvents::register);
    let controller = Controller::new(args, events)?;
    if let Some(iii) = iii.as_deref() {
        bus::register_functions(iii, controller.clone());
    }
    let state = AppState {
        controller,
        view_only,
        engine_url,
    };
    let app = Router::new()
        .route("/data.js", get(benchmark_data))
        .route("/executions.js", get(execution_manifest))
        .route("/runs/:id", get(execution_detail))
        .route("/api/dashboard", get(dashboard_config))
        .route("/api/dashboard/executions", get(execution_page))
        .route("/api/dashboard/executions/:id", get(execution_bundle))
        .fallback(get(static_asset));
    let app = if view_only {
        app
    } else {
        app.route("/ws", get(super::proxy::ws_proxy))
            .route("/api/local/run", get(run_snapshot).post(start_run))
            .route("/api/local/run/cancel", axum::routing::post(cancel_run))
            .route("/api/local/catalog", get(catalog))
    }
    .layer(axum::extract::DefaultBodyLimit::max(MAX_REQUEST_BYTES))
    .with_state(state);

    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("bind dashboard on {listen}"))?;
    println!("dashboard: http://{listen}/#/overview");
    println!("press Ctrl+C to stop");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("serve local dashboard")?;
    if let Some(iii) = iii {
        iii.shutdown_async().await;
    }
    Ok(())
}

async fn dashboard_config(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "mode": if state.view_only { "observed" } else { "local" },
        "transport": if state.view_only { "static" } else { "iii" },
        "page_size": 25,
        "functions": {
            "executions_list": bus::EXECUTIONS_LIST,
            "execution_get": bus::EXECUTION_GET,
            "catalog_get": bus::CATALOG_GET,
            "run_status": bus::RUN_STATUS,
            "run_start": bus::RUN_START,
            "run_cancel": bus::RUN_CANCEL,
            "changed_trigger": bus::CHANGED_TRIGGER,
        }
    }))
}

async fn execution_page(
    State(state): State<AppState>,
    Query(request): Query<ExecutionListRequest>,
) -> Result<Json<ExecutionListResponse>, ApiError> {
    let mut response = bus::execution_list(&state.controller, request)
        .await
        .map_err(ApiError::internal)?;
    if state.view_only {
        response.mode = "observed".into();
    }
    Ok(Json(response))
}

async fn execution_bundle(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ExecutionBundle>, ApiError> {
    let mut response =
        bus::execution_bundle(&state.controller, ExecutionGetRequest { execution_id: id })
            .await
            .map_err(ApiError::internal)?;
    if state.view_only {
        response.manifest.mode = "observed".into();
    }
    Ok(Json(response))
}

async fn run_snapshot(
    State(state): State<AppState>,
    Query(request): Query<RunStatusRequest>,
) -> Result<Json<RunSnapshot>, ApiError> {
    state
        .controller
        .snapshot(request.after)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

async fn start_run(
    State(state): State<AppState>,
    Json(request): Json<RunRequest>,
) -> Result<(StatusCode, Json<RunSnapshot>), ApiError> {
    state.controller.start(request).await?;
    let snapshot = state
        .controller
        .snapshot(Some(0))
        .await
        .map_err(ApiError::internal)?;
    Ok((StatusCode::ACCEPTED, Json(snapshot)))
}

async fn cancel_run(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<RunSnapshot>), ApiError> {
    state.controller.cancel().await?;
    let snapshot = state
        .controller
        .snapshot(None)
        .await
        .map_err(ApiError::internal)?;
    Ok((StatusCode::ACCEPTED, Json(snapshot)))
}

async fn catalog(
    State(state): State<AppState>,
    Query(query): Query<CatalogQuery>,
) -> Result<Json<Value>, ApiError> {
    let url = query
        .url
        .unwrap_or_else(|| state.controller.default_url().to_string());
    validate_stack_url(&url).map_err(|error| ApiError::bad_request(error.to_string()))?;
    bus::catalog(&state.controller, CatalogRequest { url: Some(url) }, None)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

async fn benchmark_data() -> Response {
    javascript_response("window.BENCHMARK_DATA = {};\n".into())
}

async fn execution_manifest(State(state): State<AppState>) -> Result<Response, ApiError> {
    let executions = state
        .controller
        .execution_summaries()
        .await
        .map_err(ApiError::internal)?;
    let last_update = executions
        .first()
        .and_then(|value| value.get("completed_at"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(javascript_response(format!(
        "window.HARNESS_EXECUTIONS = {};\n",
        json!({
            "schema_version": 4,
            "mode": if state.view_only { "observed" } else { "local" },
            "last_update": last_update,
            "repo_url": repository_url(),
            "retention": { "summaries": MAX_EXECUTIONS, "details": MAX_EXECUTIONS },
            "executions": executions.as_ref(),
        })
    )))
}

async fn execution_detail(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Value>, ApiError> {
    let id = id
        .strip_suffix(".json")
        .ok_or_else(|| ApiError::bad_request("execution detail must end in .json"))?
        .to_string();
    validate_execution_id(&id).map_err(ApiError::bad_request)?;
    let run_dir = state.controller.runs_dir().join(&id);
    let run = read_stored_run(&run_dir)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            message: "execution not found".into(),
        })?;
    let report = run.report.ok_or_else(|| ApiError {
        status: StatusCode::NOT_FOUND,
        message: "execution report not found".into(),
    })?;
    execution_detail_value(&run.metadata, &report)
        .map(Json)
        .map_err(ApiError::internal)
}
