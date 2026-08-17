use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::artifact;
use crate::context::E2eContext;
use crate::identity::{self, ExecutionIdentity, SystemUnderTestIdentity};
use crate::report::{E2eManifest, E2eReport, ModelArtifact, ObservedWorkerContract};
use crate::wire::Model;

use super::security_scan::{self, register_security_scan_steps};
use super::{execute_workflow, StepCatalog, WorkflowDefinitionV1, WorkflowExecutionRequest};

pub struct SecurityReviewRunConfig {
    pub url: String,
    pub model: String,
    pub provider: String,
    pub runs_dir: PathBuf,
    pub lane: String,
}

pub struct SecurityReviewRunOutcome {
    pub report: E2eReport,
    pub manifest: E2eManifest,
    pub report_path: PathBuf,
}

pub async fn run_security_review(
    config: SecurityReviewRunConfig,
) -> Result<SecurityReviewRunOutcome> {
    if config.model.trim().is_empty() || config.provider.trim().is_empty() {
        bail!("workflow model and provider cannot be empty");
    }
    let execution_id = Uuid::new_v4().simple().to_string();
    let output = config.runs_dir.join(&execution_id).join("results");
    let started_at = timestamp();
    let context = Arc::new(
        E2eContext::connect(&config.url)
            .await
            .context("connect workflow runner")?,
    );
    let control_plane = context
        .preflight_control_plane()
        .await
        .context("preflight Harness control-plane contract")?;
    let runtime_versions = context.runtime_versions().await?;
    let system_under_test = SystemUnderTestIdentity::from_environment(
        runtime_versions.engine,
        runtime_versions.harness,
        &control_plane,
    )?;
    let model = resolve_model(&context, &config.model, &config.provider).await?;
    let subject = ModelArtifact::from(model);
    let mut catalog = StepCatalog::new();
    let cleanup_hook = register_security_scan_steps(&mut catalog, context.clone())?;
    let definition = security_scan::definition();
    definition.validate(&catalog)?;
    let worker_contracts =
        observe_worker_contracts(&context, &catalog, std::slice::from_ref(&definition)).await?;
    let catalog = Arc::new(catalog);
    let mut workflow_runs = Vec::new();
    let run_id = Uuid::new_v4().simple().to_string();
    for attempt in 0..=definition.limits.technical_retries {
        context
            .bind_turn_completed()
            .await
            .with_context(|| format!("bind turn observation for workflow '{}'", definition.id))?;
        let (cancel_sender, cancellation) = tokio::sync::watch::channel(false);
        let signal_task = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                let _ = cancel_sender.send(true);
            }
        });
        let outcome = execute_workflow(
            &definition,
            catalog.clone(),
            WorkflowExecutionRequest {
                output_dir: output.clone(),
                run_id: run_id.clone(),
                attempt_number: u32::from(attempt) + 1,
                cancellation,
                cleanup_hook: cleanup_hook.clone(),
            },
        )
        .await;
        signal_task.abort();
        let unbind = context.unbind_turn_completed().await;
        let report = outcome.with_context(|| {
            format!(
                "execute workflow '{}' attempt {}",
                definition.id,
                attempt + 1
            )
        })?;
        unbind.context("unbind workflow turn observation")?;
        let retry = report.technical_failure && attempt < definition.limits.technical_retries;
        workflow_runs.push(report);
        if !retry {
            break;
        }
    }

    let execution = ExecutionIdentity {
        execution_id,
        lane: if config.lane.trim().is_empty() {
            "local-workflow".into()
        } else {
            config.lane
        },
        started_at,
        completed_at: timestamp(),
    };
    let manifest = E2eManifest {
        execution: execution.clone(),
        system_under_test: system_under_test.clone(),
        subject: subject.clone(),
        judge: None,
        control_plane,
        worker_contracts,
    };
    let mut report = E2eReport::new_workflows(
        execution,
        system_under_test,
        subject,
        identity::nonempty_env("HARNESS_E2E_ENGINE_REVISION"),
        workflow_runs,
    );
    let report_path = report.write_to(&output, &manifest)?;
    context.shutdown().await;
    Ok(SecurityReviewRunOutcome {
        report,
        manifest,
        report_path,
    })
}

async fn resolve_model(context: &E2eContext, model: &str, provider: &str) -> Result<Model> {
    let response = context
        .trigger_value(
            "router::models::get",
            json!({"id": model, "provider": provider}),
        )
        .await?;
    if response.is_null() {
        bail!("model {provider}/{model} is not registered in the router catalog");
    }
    let resolved: Model = serde_json::from_value(
        response
            .get("model")
            .cloned()
            .context("router::models::get response is missing model")?,
    )?;
    if resolved.id != model || resolved.provider != provider {
        bail!("router resolved a different model identity");
    }
    if !context
        .function_exists(&format!("provider::{provider}::stream"))
        .await?
    {
        bail!("provider::{provider}::stream is unavailable");
    }
    Ok(resolved)
}

async fn observe_worker_contracts(
    context: &E2eContext,
    catalog: &StepCatalog,
    definitions: &[WorkflowDefinitionV1],
) -> Result<Vec<ObservedWorkerContract>> {
    let used = definitions
        .iter()
        .flat_map(|definition| &definition.nodes)
        .map(|node| (node.step_type.as_str(), node.step_version))
        .collect::<BTreeSet<_>>();
    let mut expected_contracts = HashMap::new();
    for (step_type, version) in used {
        let registered = catalog
            .get(step_type, version)
            .context("validated step type disappeared from catalog")?;
        for required in &registered.descriptor.required_functions {
            let expectation = (
                required.request_schema_sha256.clone(),
                required.response_schema_sha256.clone(),
            );
            if let Some(previous) =
                expected_contracts.insert(required.function_id.clone(), expectation.clone())
            {
                if previous != expectation {
                    bail!(
                        "step catalog declares conflicting contract hashes for '{}'",
                        required.function_id
                    );
                }
            }
        }
    }
    let harness = crate::wire::control_plane_function_ids().collect::<BTreeSet<_>>();
    let function_ids = expected_contracts
        .keys()
        .filter(|function_id| !harness.contains(function_id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if function_ids.is_empty() {
        return Ok(Vec::new());
    }
    let info = context
        .trigger_value(
            "engine::functions::info",
            json!({"function_ids": function_ids}),
        )
        .await?;
    let details = info
        .get("functions")
        .and_then(Value::as_array)
        .context("functions::info response is missing functions[]")?;
    let by_id = details
        .iter()
        .filter_map(|detail| Some((detail.get("function_id")?.as_str()?, detail)))
        .collect::<HashMap<_, _>>();
    function_ids
        .into_iter()
        .map(|function_id| {
            let detail = by_id
                .get(function_id.as_str())
                .with_context(|| format!("functions::info omitted '{function_id}'"))?;
            let request_schema = detail
                .get("request_schema")
                .filter(|schema| schema.is_object())
                .with_context(|| format!("'{function_id}' has no request schema"))?;
            let response_schema = detail
                .get("response_schema")
                .filter(|schema| schema.is_object())
                .with_context(|| format!("'{function_id}' has no response schema"))?;
            let request_schema_sha256 = artifact::sha256_value(request_schema)?;
            let response_schema_sha256 = artifact::sha256_value(response_schema)?;
            let expected = &expected_contracts[&function_id];
            if expected
                .0
                .as_ref()
                .is_some_and(|hash| hash != &request_schema_sha256)
                || expected
                    .1
                    .as_ref()
                    .is_some_and(|hash| hash != &response_schema_sha256)
            {
                bail!("observed contract for '{function_id}' differs from the registered exact contract");
            }
            Ok(ObservedWorkerContract {
                function_id,
                request_schema_sha256,
                response_schema_sha256,
            })
        })
        .collect()
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
