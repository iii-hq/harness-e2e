use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::watch;
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::artifact::{self, ArtifactReference};
use crate::redaction::{RedactionPolicy, RedactionReport};

use super::catalog::{
    CapturedWorkflowAsset, NoopWorkflowCleanupHook, StepCatalog, StepEvaluation,
    StepExecutorContext, StepExecutorOutput, TypedPortValue, WorkflowAssetContent,
    WorkflowCleanupContext, WorkflowCleanupHook, WorkflowEvaluationResult, WorkflowGateResult,
};
use super::{
    ActivationPolicy, DependencyPolicy, MaterializedWorkflow, WorkflowDefinitionV1,
    WorkflowInputBinding, WorkflowNodeV1,
};

const MAX_PORT_VALUE_BYTES: usize = 64 * 1024;
const MAX_WORKFLOW_ASSETS_PER_STEP: usize = 64;
const MAX_WORKFLOW_ASSET_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub struct WorkflowExecutionRequest {
    pub output_dir: PathBuf,
    pub run_id: String,
    pub attempt_number: u32,
    pub cancellation: watch::Receiver<bool>,
    pub cleanup_hook: Arc<dyn WorkflowCleanupHook>,
}

impl WorkflowExecutionRequest {
    pub fn local(output_dir: PathBuf) -> Self {
        let (_sender, cancellation) = watch::channel(false);
        Self {
            output_dir,
            run_id: Uuid::new_v4().simple().to_string(),
            attempt_number: 1,
            cancellation,
            cleanup_hook: Arc::new(NoopWorkflowCleanupHook),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStepStatus {
    Pending,
    Running,
    Succeeded,
    HardGateFailed,
    Failed,
    Skipped,
    Cancelled,
}

impl WorkflowStepStatus {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::HardGateFailed | Self::Failed | Self::Skipped | Self::Cancelled
        )
    }

    fn dependency_succeeded(self) -> bool {
        self == Self::Succeeded
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStepFailure {
    pub phase: WorkflowFailurePhase,
    pub message: String,
    pub technical: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowFailurePhase {
    Preflight,
    Execute,
    Capture,
    Persist,
    Evaluate,
    Cleanup,
    Cancel,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAssetReport {
    pub id: String,
    pub namespaced_id: String,
    pub kind: String,
    pub media_type: String,
    pub content_sha256: String,
    pub size_bytes: u64,
    pub preview: Value,
    pub preview_truncated: bool,
    pub artifact: ArtifactReference,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStepReport {
    pub node_id: String,
    pub step_type: String,
    pub step_version: u32,
    pub required: bool,
    pub dependencies: Vec<String>,
    pub dependency_policy: DependencyPolicy,
    pub activation: ActivationPolicy,
    pub status: WorkflowStepStatus,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, TypedPortValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<WorkflowAssetReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hard_gates: Vec<WorkflowGateResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evaluations: Vec<WorkflowEvaluationResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<WorkflowStepFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    #[serde(default)]
    pub redaction: RedactionReport,
}

impl WorkflowStepReport {
    fn pending(node: &WorkflowNodeV1) -> Self {
        Self {
            node_id: node.id.clone(),
            step_type: node.step_type.clone(),
            step_version: node.step_version,
            required: node.required,
            dependencies: node.depends_on.clone(),
            dependency_policy: node.dependency_policy,
            activation: node.activation.clone(),
            status: WorkflowStepStatus::Pending,
            started_at: None,
            completed_at: None,
            duration_ms: 0,
            harness_session_id: None,
            outputs: BTreeMap::new(),
            transcript: None,
            metrics: None,
            cost_usd: None,
            assets: Vec::new(),
            hard_gates: Vec::new(),
            evaluations: Vec::new(),
            failures: Vec::new(),
            skip_reason: None,
            redaction: RedactionReport::default(),
        }
    }

    fn skipped(node: &WorkflowNodeV1, reason: impl Into<String>) -> Self {
        let now = timestamp();
        let mut report = Self::pending(node);
        report.status = WorkflowStepStatus::Skipped;
        report.started_at = Some(now.clone());
        report.completed_at = Some(now);
        report.skip_reason = Some(reason.into());
        report
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAttemptReport {
    pub workflow_id: String,
    pub workflow_scenario_version: u32,
    pub workflow_sha256: String,
    pub run_id: String,
    pub attempt_id: String,
    pub attempt_number: u32,
    pub started_at: String,
    pub completed_at: String,
    pub duration_ms: u64,
    pub passed: bool,
    pub technical_failure: bool,
    /// Evidence-only projection generated by Rust. It is intentionally not a workflow definition.
    #[serde(default)]
    pub flow_snapshot: Value,
    pub steps: Vec<WorkflowStepReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<WorkflowCriterionResult>,
    pub aggregate_cost_usd: Option<f64>,
    pub checkpoint: ArtifactReference,
    #[serde(default)]
    pub cleanup: WorkflowCleanupReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCleanupReport {
    pub status: WorkflowCleanupStatus,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowCleanupStatus {
    Succeeded,
    Failed,
}

impl Default for WorkflowCleanupReport {
    fn default() -> Self {
        Self {
            status: WorkflowCleanupStatus::Succeeded,
            duration_ms: 0,
            failure: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCriterionResult {
    pub id: String,
    pub weight: u8,
    pub producer_node_id: String,
    pub output_port: String,
    pub advisory: bool,
    pub outcome: super::WorkflowEvaluationOutcome,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCheckpointV1 {
    pub schema_version: u32,
    pub workflow_id: String,
    pub workflow_sha256: String,
    pub run_id: String,
    pub attempt_id: String,
    /// Evidence-only projection generated by Rust. It is never accepted as runner input.
    #[serde(default)]
    pub flow_snapshot: Value,
    pub updated_at: String,
    pub terminal_nodes: Vec<String>,
    pub active_nodes: Vec<String>,
    pub steps: Vec<WorkflowStepReport>,
}

#[derive(Clone)]
pub struct CheckpointStore {
    output_dir: PathBuf,
    relative_path: PathBuf,
}

impl CheckpointStore {
    pub fn new(output_dir: &Path, run_id: &str, attempt_id: &str) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
            relative_path: PathBuf::from("checkpoints")
                .join(run_id)
                .join(attempt_id)
                .join("workflow-checkpoint.json"),
        }
    }

    pub fn persist(&self, checkpoint: &WorkflowCheckpointV1) -> Result<ArtifactReference> {
        artifact::write_json(
            &self.output_dir,
            &self.relative_path,
            "workflow-checkpoint",
            "workflow_checkpoint",
            checkpoint,
        )
    }
}

pub async fn execute_workflow(
    definition: &WorkflowDefinitionV1,
    catalog: Arc<StepCatalog>,
    request: WorkflowExecutionRequest,
) -> Result<WorkflowAttemptReport> {
    let materialized = definition.validate(&catalog)?;
    let attempt_id = Uuid::new_v4().simple().to_string();
    let cleanup_context = WorkflowCleanupContext {
        workflow_id: materialized.definition.id.clone(),
        workflow_sha256: materialized.sha256.clone(),
        run_id: request.run_id.clone(),
        attempt_id: attempt_id.clone(),
        output_dir: request.output_dir.clone(),
    };
    let result = execute_materialized_workflow(materialized, catalog, &request, attempt_id).await;
    let cleanup_started = Instant::now();
    let cleanup_result = request.cleanup_hook.cleanup(&cleanup_context).await;
    let cleanup_duration_ms = elapsed_ms(cleanup_started);
    match (result, cleanup_result) {
        (Ok(mut report), Ok(())) => {
            report.cleanup = WorkflowCleanupReport {
                status: WorkflowCleanupStatus::Succeeded,
                duration_ms: cleanup_duration_ms,
                failure: None,
            };
            Ok(report)
        }
        (Ok(mut report), Err(error)) => {
            report.passed = false;
            report.technical_failure = true;
            report.cleanup = WorkflowCleanupReport {
                status: WorkflowCleanupStatus::Failed,
                duration_ms: cleanup_duration_ms,
                failure: Some(format!("{error:#}")),
            };
            Ok(report)
        }
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup_error)) => Err(error.context(format!(
            "mandatory cleanup hook also failed: {cleanup_error:#}"
        ))),
    }
}

async fn execute_materialized_workflow(
    materialized: MaterializedWorkflow,
    catalog: Arc<StepCatalog>,
    request: &WorkflowExecutionRequest,
    attempt_id: String,
) -> Result<WorkflowAttemptReport> {
    let started = Instant::now();
    let workflow_deadline = tokio::time::Instant::now()
        + Duration::from_secs(materialized.definition.limits.workflow_timeout_seconds);
    let started_at = timestamp();
    let checkpoint_store = CheckpointStore::new(&request.output_dir, &request.run_id, &attempt_id);
    let (abort_sender, abort_receiver) = watch::channel(false);
    let mut external_cancellation = request.cancellation.clone();

    preflight_all(
        &materialized,
        &catalog,
        request,
        &attempt_id,
        abort_receiver.clone(),
    )
    .await?;

    let nodes = materialized
        .definition
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node.clone()))
        .collect::<HashMap<_, _>>();
    let mut reports = nodes
        .iter()
        .map(|(id, node)| (id.clone(), WorkflowStepReport::pending(node)))
        .collect::<HashMap<_, _>>();
    let mut outputs: HashMap<String, BTreeMap<String, TypedPortValue>> = HashMap::new();
    let mut active_contexts: HashMap<String, StepExecutorContext> = HashMap::new();
    let mut running = JoinSet::new();
    let mut technical_failure = false;
    let mut workflow_failure = None;

    loop {
        if *external_cancellation.borrow() && !technical_failure {
            technical_failure = true;
            workflow_failure = Some(WorkflowStepFailure {
                phase: WorkflowFailurePhase::Cancel,
                message: "workflow execution was cancelled".into(),
                technical: true,
            });
            let _ = abort_sender.send(true);
            cancel_active(&catalog, &active_contexts).await;
        }
        if tokio::time::Instant::now() >= workflow_deadline && !technical_failure {
            technical_failure = true;
            workflow_failure = Some(WorkflowStepFailure {
                phase: WorkflowFailurePhase::Execute,
                message: format!(
                    "workflow timed out after {} seconds",
                    materialized.definition.limits.workflow_timeout_seconds
                ),
                technical: true,
            });
            let _ = abort_sender.send(true);
            cancel_active(&catalog, &active_contexts).await;
        }

        let mut changed = false;
        let mut pending_ids = reports
            .iter()
            .filter_map(|(id, report)| {
                (report.status == WorkflowStepStatus::Pending).then_some(id.clone())
            })
            .collect::<Vec<_>>();
        pending_ids.sort();

        if technical_failure {
            for id in pending_ids {
                let node = &nodes[&id];
                reports.insert(
                    id,
                    WorkflowStepReport::skipped(
                        node,
                        "not started because another step had a technical failure",
                    ),
                );
                changed = true;
            }
        } else {
            for id in pending_ids.clone() {
                let node = &nodes[&id];
                if !dependencies_terminal(node, &reports) {
                    continue;
                }
                if node.dependency_policy == DependencyPolicy::Succeeded
                    && node
                        .depends_on
                        .iter()
                        .any(|dependency| !reports[dependency].status.dependency_succeeded())
                {
                    reports.insert(
                        id,
                        WorkflowStepReport::skipped(
                            node,
                            "a dependency did not succeed under dependency_policy=succeeded",
                        ),
                    );
                    changed = true;
                    continue;
                }
                if !activation_satisfied(node, &outputs)? {
                    reports.insert(
                        id,
                        WorkflowStepReport::skipped(node, "activation condition evaluated false"),
                    );
                    changed = true;
                }
            }

            let available = usize::from(materialized.definition.limits.max_parallel)
                .saturating_sub(running.len());
            if available > 0 {
                let mut ready = reports
                    .iter()
                    .filter_map(|(id, report)| {
                        if report.status != WorkflowStepStatus::Pending {
                            return None;
                        }
                        let node = &nodes[id];
                        (dependencies_ready(node, &reports)
                            && activation_satisfied(node, &outputs).ok() == Some(true))
                        .then_some(id.clone())
                    })
                    .collect::<Vec<_>>();
                ready.sort();
                for id in ready.into_iter().take(available) {
                    let node = nodes[&id].clone();
                    let registered = catalog
                        .get(&node.step_type, node.step_version)
                        .expect("validated catalog entry")
                        .clone();
                    let inputs = resolve_inputs(&node, &outputs)?;
                    let context = StepExecutorContext {
                        workflow_id: materialized.definition.id.clone(),
                        workflow_sha256: materialized.sha256.clone(),
                        run_id: request.run_id.clone(),
                        attempt_id: attempt_id.clone(),
                        node: node.clone(),
                        inputs,
                        output_dir: request.output_dir.clone(),
                        cancellation: abort_receiver.clone(),
                    };
                    let report = reports.get_mut(&id).expect("known report");
                    report.status = WorkflowStepStatus::Running;
                    report.started_at = Some(timestamp());
                    active_contexts.insert(id.clone(), context.clone());
                    let timeout =
                        Duration::from_secs(materialized.definition.limits.step_timeout_seconds);
                    running.spawn(async move {
                        let report = run_step(registered, context, timeout).await;
                        (id, report)
                    });
                    changed = true;
                }
            }
        }

        if changed {
            persist_checkpoint(
                &checkpoint_store,
                &materialized,
                &request.run_id,
                &attempt_id,
                &reports,
                active_contexts.keys(),
            )?;
        }

        if reports.values().all(|report| report.status.terminal()) {
            break;
        }

        if running.is_empty() {
            bail!("workflow scheduler reached a deadlock with pending nodes");
        }

        let joined = tokio::select! {
            changed = external_cancellation.changed() => {
                if changed.is_ok() && *external_cancellation.borrow() {
                    technical_failure = true;
                    workflow_failure.get_or_insert(WorkflowStepFailure {
                        phase: WorkflowFailurePhase::Cancel,
                        message: "workflow execution was cancelled".into(),
                        technical: true,
                    });
                    let _ = abort_sender.send(true);
                    cancel_active(&catalog, &active_contexts).await;
                }
                continue;
            }
            _ = tokio::time::sleep_until(workflow_deadline), if !technical_failure => {
                technical_failure = true;
                workflow_failure = Some(WorkflowStepFailure {
                    phase: WorkflowFailurePhase::Execute,
                    message: format!(
                        "workflow timed out after {} seconds",
                        materialized.definition.limits.workflow_timeout_seconds
                    ),
                    technical: true,
                });
                let _ = abort_sender.send(true);
                cancel_active(&catalog, &active_contexts).await;
                continue;
            }
            joined = running.join_next() => joined,
        };
        let Some(joined) = joined else {
            continue;
        };
        let (id, mut report) = joined.context("join workflow step task")?;
        active_contexts.remove(&id);
        if matches!(
            report.status,
            WorkflowStepStatus::Succeeded | WorkflowStepStatus::HardGateFailed
        ) {
            if let Some(message) = budget_violation(
                &materialized.definition.limits,
                reports.values().chain(std::iter::once(&report)),
            ) {
                report.failures.push(WorkflowStepFailure {
                    phase: WorkflowFailurePhase::Execute,
                    message: message.clone(),
                    technical: true,
                });
                report.status = WorkflowStepStatus::Failed;
                technical_failure = true;
                workflow_failure.get_or_insert(WorkflowStepFailure {
                    phase: WorkflowFailurePhase::Execute,
                    message,
                    technical: true,
                });
                let _ = abort_sender.send(true);
                cancel_active(&catalog, &active_contexts).await;
            }
        }
        if report.status == WorkflowStepStatus::Failed {
            technical_failure = true;
            let _ = abort_sender.send(true);
            cancel_active(&catalog, &active_contexts).await;
        }
        if report.status == WorkflowStepStatus::Succeeded
            || report.status == WorkflowStepStatus::HardGateFailed
        {
            outputs.insert(id.clone(), report.outputs.clone());
        }
        report.completed_at.get_or_insert_with(timestamp);
        reports.insert(id, report);
        persist_checkpoint(
            &checkpoint_store,
            &materialized,
            &request.run_id,
            &attempt_id,
            &reports,
            active_contexts.keys(),
        )?;
    }

    let mut steps = ordered_reports(&materialized, reports);
    if let Some(failure) = workflow_failure {
        let target_index = steps
            .iter()
            .position(|step| {
                matches!(
                    step.status,
                    WorkflowStepStatus::Failed | WorkflowStepStatus::Cancelled
                )
            })
            .or_else(|| (!steps.is_empty()).then_some(0));
        if let Some(target) = target_index.and_then(|index| steps.get_mut(index)) {
            let duplicate = target.failures.iter().any(|observed| {
                observed.phase == failure.phase && observed.message == failure.message
            });
            if !duplicate {
                target.failures.push(failure);
            }
        }
    }
    let criteria = materialize_criteria(&materialized.definition.criteria, &steps);
    let passed = !technical_failure
        && steps
            .iter()
            .all(|step| !step.required || step.status == WorkflowStepStatus::Succeeded)
        && steps
            .iter()
            .flat_map(|step| &step.hard_gates)
            .all(|gate| gate.passed)
        && criteria
            .iter()
            .filter(|criterion| !criterion.advisory)
            .all(|criterion| criterion.outcome == super::WorkflowEvaluationOutcome::Passed);
    let aggregate_cost_usd = aggregate_cost(&steps);
    let final_checkpoint = checkpoint_store.persist(&checkpoint_value(
        &materialized,
        &request.run_id,
        &attempt_id,
        &steps,
        std::iter::empty::<&String>(),
    ))?;
    let flow_snapshot = evidence_snapshot(&materialized);
    Ok(WorkflowAttemptReport {
        workflow_id: materialized.definition.id,
        workflow_scenario_version: materialized.definition.scenario_version,
        workflow_sha256: materialized.sha256,
        run_id: request.run_id.clone(),
        attempt_id,
        attempt_number: request.attempt_number,
        started_at,
        completed_at: timestamp(),
        duration_ms: elapsed_ms(started),
        passed,
        technical_failure,
        flow_snapshot,
        steps,
        criteria,
        aggregate_cost_usd,
        checkpoint: final_checkpoint,
        cleanup: WorkflowCleanupReport {
            status: WorkflowCleanupStatus::Succeeded,
            duration_ms: 0,
            failure: None,
        },
    })
}

async fn preflight_all(
    materialized: &MaterializedWorkflow,
    catalog: &StepCatalog,
    request: &WorkflowExecutionRequest,
    attempt_id: &str,
    cancellation: watch::Receiver<bool>,
) -> Result<()> {
    for id in &materialized.topological_order {
        let node = materialized
            .definition
            .nodes
            .iter()
            .find(|node| &node.id == id)
            .expect("validated node");
        let registered = catalog
            .get(&node.step_type, node.step_version)
            .expect("validated catalog entry");
        registered
            .executor
            .preflight(&StepExecutorContext {
                workflow_id: materialized.definition.id.clone(),
                workflow_sha256: materialized.sha256.clone(),
                run_id: request.run_id.clone(),
                attempt_id: attempt_id.to_string(),
                node: node.clone(),
                inputs: BTreeMap::new(),
                output_dir: request.output_dir.clone(),
                cancellation: cancellation.clone(),
            })
            .await
            .with_context(|| format!("preflight workflow node '{}'", node.id))?;
    }
    Ok(())
}

async fn run_step(
    registered: super::RegisteredStepType,
    context: StepExecutorContext,
    timeout: Duration,
) -> WorkflowStepReport {
    let started = Instant::now();
    let mut report = WorkflowStepReport::pending(&context.node);
    report.status = WorkflowStepStatus::Running;
    report.started_at = Some(timestamp());

    let mut execution =
        match tokio::time::timeout(timeout, registered.executor.execute(context.clone())).await {
            Ok(Ok(execution)) => execution,
            Ok(Err(error)) => {
                report
                    .failures
                    .push(step_failure(WorkflowFailurePhase::Execute, error));
                finish_failed_step(&registered, &context, &mut report, started).await;
                return report;
            }
            Err(_) => {
                report.failures.push(WorkflowStepFailure {
                    phase: WorkflowFailurePhase::Execute,
                    message: format!("step timed out after {} seconds", timeout.as_secs()),
                    technical: true,
                });
                if let Err(error) = registered.executor.cancel(&context).await {
                    report
                        .failures
                        .push(step_failure(WorkflowFailurePhase::Cancel, error));
                }
                finish_failed_step(&registered, &context, &mut report, started).await;
                return report;
            }
        };

    let assets = match registered.executor.capture(&context, &execution).await {
        Ok(assets) => assets,
        Err(error) => {
            report
                .failures
                .push(step_failure(WorkflowFailurePhase::Capture, error));
            finish_failed_step(&registered, &context, &mut report, started).await;
            return report;
        }
    };
    let assets_for_evaluation = assets.clone();
    match persist_assets(&context, assets) {
        Ok((assets, redaction)) => {
            report.assets = assets;
            report.redaction = redaction;
        }
        Err(error) => {
            report
                .failures
                .push(step_failure(WorkflowFailurePhase::Persist, error));
            finish_failed_step(&registered, &context, &mut report, started).await;
            return report;
        }
    }

    let evaluation = match registered
        .executor
        .evaluate(&context, &execution, &assets_for_evaluation)
        .await
    {
        Ok(evaluation) => evaluation,
        Err(error) => {
            report
                .failures
                .push(step_failure(WorkflowFailurePhase::Evaluate, error));
            finish_failed_step(&registered, &context, &mut report, started).await;
            return report;
        }
    };
    let declared_technical_failure = execution.technical_failure.clone();

    match validate_outputs(
        &registered.descriptor,
        std::mem::take(&mut execution.outputs),
    ) {
        Ok(outputs) => report.outputs = outputs,
        Err(error) => {
            report
                .failures
                .push(step_failure(WorkflowFailurePhase::Evaluate, error));
            finish_failed_step(&registered, &context, &mut report, started).await;
            return report;
        }
    }
    apply_execution(&mut report, execution, evaluation);
    if let Err(error) = registered.executor.cleanup(&context).await {
        report
            .failures
            .push(step_failure(WorkflowFailurePhase::Cleanup, error));
        report.status = WorkflowStepStatus::Failed;
    } else if let Some(message) = declared_technical_failure {
        let (message, _) = RedactionPolicy::from_environment().redact_text(&message);
        report.failures.push(WorkflowStepFailure {
            phase: WorkflowFailurePhase::Execute,
            message,
            technical: true,
        });
        report.status = WorkflowStepStatus::Failed;
    } else if report.hard_gates.iter().any(|gate| !gate.passed) {
        report.status = WorkflowStepStatus::HardGateFailed;
    } else {
        report.status = WorkflowStepStatus::Succeeded;
    }
    report.completed_at = Some(timestamp());
    report.duration_ms = elapsed_ms(started);
    report
}

fn apply_execution(
    report: &mut WorkflowStepReport,
    execution: StepExecutorOutput,
    mut evaluation: StepEvaluation,
) {
    let policy = RedactionPolicy::from_environment();
    report.harness_session_id = execution.harness_session_id;
    report.transcript = execution.transcript.map(|mut value| {
        report.redaction.merge(policy.redact_value(&mut value));
        value
    });
    report.metrics = execution.metrics.map(|mut value| {
        report.redaction.merge(policy.redact_value(&mut value));
        value
    });
    report.cost_usd = execution
        .cost_usd
        .filter(|cost| cost.is_finite() && *cost >= 0.0);
    for gate in &mut evaluation.hard_gates {
        let (reason, nested) = policy.redact_text(&gate.reason);
        gate.reason = reason;
        report.redaction.merge(nested);
        for evidence_id in &mut gate.evidence_ids {
            let (sanitized, nested) = policy.redact_text(evidence_id);
            *evidence_id = sanitized;
            report.redaction.merge(nested);
        }
    }
    for evaluation in &mut evaluation.evaluations {
        let (summary, nested) = policy.redact_text(&evaluation.summary);
        evaluation.summary = summary;
        report.redaction.merge(nested);
        for evidence_id in &mut evaluation.evidence_ids {
            let (sanitized, nested) = policy.redact_text(evidence_id);
            *evidence_id = sanitized;
            report.redaction.merge(nested);
        }
    }
    report.hard_gates = evaluation.hard_gates;
    report.evaluations = evaluation.evaluations;
}

async fn finish_failed_step(
    registered: &super::RegisteredStepType,
    context: &StepExecutorContext,
    report: &mut WorkflowStepReport,
    started: Instant,
) {
    if let Err(error) = registered.executor.cleanup(context).await {
        report
            .failures
            .push(step_failure(WorkflowFailurePhase::Cleanup, error));
    }
    report.status = if *context.cancellation.borrow() {
        WorkflowStepStatus::Cancelled
    } else {
        WorkflowStepStatus::Failed
    };
    report.completed_at = Some(timestamp());
    report.duration_ms = elapsed_ms(started);
}

fn validate_outputs(
    descriptor: &super::StepTypeDescriptor,
    mut outputs: BTreeMap<String, TypedPortValue>,
) -> Result<BTreeMap<String, TypedPortValue>> {
    for (name, port) in &descriptor.outputs {
        if !port.optional && !outputs.contains_key(name) {
            bail!("step did not materialize required output port '{name}'");
        }
    }
    let policy = RedactionPolicy::from_environment();
    for (name, output) in &mut outputs {
        let port = descriptor
            .outputs
            .get(name)
            .with_context(|| format!("step materialized undeclared output port '{name}'"))?;
        if output.kind != port.kind {
            bail!(
                "step output '{name}' declared {:?} but materialized {:?}",
                port.kind,
                output.kind
            );
        }
        output.validate()?;
        policy.redact_value(&mut output.value);
        let encoded = serde_json::to_vec(output).context("encode sanitized step output")?;
        if encoded.len() > MAX_PORT_VALUE_BYTES {
            bail!(
                "step output '{name}' exceeds the sanitized {} byte limit",
                MAX_PORT_VALUE_BYTES
            );
        }
    }
    Ok(outputs)
}

fn persist_assets(
    context: &StepExecutorContext,
    assets: Vec<CapturedWorkflowAsset>,
) -> Result<(Vec<WorkflowAssetReport>, RedactionReport)> {
    if assets.len() > MAX_WORKFLOW_ASSETS_PER_STEP {
        bail!(
            "step '{}' captured {} assets; limit is {}",
            context.node.id,
            assets.len(),
            MAX_WORKFLOW_ASSETS_PER_STEP
        );
    }
    let policy = RedactionPolicy::from_environment();
    let mut redaction = RedactionReport::default();
    let mut ids = HashSet::new();
    let mut reports = Vec::with_capacity(assets.len());
    for mut asset in assets {
        validate_asset_id(&asset.id)?;
        if !ids.insert(asset.id.clone()) {
            bail!(
                "step '{}' captured duplicate asset '{}'",
                context.node.id,
                asset.id
            );
        }
        if asset.kind.trim().is_empty() || !asset.content.media_type_compatible(&asset.media_type) {
            bail!(
                "step '{}' asset '{}' has incompatible kind or MIME '{}'",
                context.node.id,
                asset.id,
                asset.media_type
            );
        }
        let (bytes, preview, preview_truncated, extension) = match &mut asset.content {
            WorkflowAssetContent::Json(value) => {
                redaction.merge(policy.redact_value(value));
                let mut bytes = serde_json::to_vec_pretty(value).context("encode JSON asset")?;
                bytes.push(b'\n');
                let (preview, truncated) = bounded_asset_preview(value, 1_024);
                (bytes, preview, truncated, "json")
            }
            WorkflowAssetContent::TextUtf8(text) => {
                let (sanitized, report) = policy.redact_text(text);
                *text = sanitized;
                redaction.merge(report);
                let bytes = text.as_bytes().to_vec();
                let (preview, truncated) = bounded_text_preview(text, 1_024);
                (bytes, Value::String(preview), truncated, "txt")
            }
        };
        policy.assert_clean(&bytes)?;
        if bytes.len() > MAX_WORKFLOW_ASSET_BYTES {
            bail!(
                "step '{}' asset '{}' is {} bytes; limit is {}",
                context.node.id,
                asset.id,
                bytes.len(),
                MAX_WORKFLOW_ASSET_BYTES
            );
        }
        let relative = PathBuf::from("deliverables")
            .join(&context.run_id)
            .join(&context.attempt_id)
            .join(&context.node.id)
            .join(format!("{}.{}", asset.id, extension));
        let namespaced_id = format!("{}.{}", context.node.id, asset.id);
        let reference = artifact::write_bytes(
            &context.output_dir,
            &relative,
            namespaced_id.clone(),
            asset.kind.clone(),
            asset.media_type.clone(),
            &bytes,
        )?;
        reports.push(WorkflowAssetReport {
            id: asset.id,
            namespaced_id,
            kind: asset.kind,
            media_type: asset.media_type,
            content_sha256: reference.sha256.clone(),
            size_bytes: reference.size_bytes,
            preview,
            preview_truncated,
            artifact: reference,
        });
    }
    Ok((reports, redaction))
}

fn validate_asset_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        bail!("asset id '{id}' is not path-safe");
    }
    Ok(())
}

fn bounded_asset_preview(value: &Value, limit: usize) -> (Value, bool) {
    let encoded = serde_json::to_vec(value).unwrap_or_default();
    if encoded.len() <= limit {
        return (value.clone(), false);
    }
    (
        serde_json::json!({
            "truncated": true,
            "size_bytes": encoded.len(),
            "sha256": artifact::sha256_value(value).ok(),
        }),
        true,
    )
}

fn bounded_text_preview(value: &str, limit: usize) -> (String, bool) {
    if value.len() <= limit {
        return (value.to_string(), false);
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}…", &value[..end]), true)
}

fn dependencies_terminal(
    node: &WorkflowNodeV1,
    reports: &HashMap<String, WorkflowStepReport>,
) -> bool {
    node.depends_on
        .iter()
        .all(|dependency| reports[dependency].status.terminal())
}

fn dependencies_ready(
    node: &WorkflowNodeV1,
    reports: &HashMap<String, WorkflowStepReport>,
) -> bool {
    if !dependencies_terminal(node, reports) {
        return false;
    }
    match node.dependency_policy {
        DependencyPolicy::Succeeded => node
            .depends_on
            .iter()
            .all(|dependency| reports[dependency].status.dependency_succeeded()),
        DependencyPolicy::Terminal => true,
    }
}

fn activation_satisfied(
    node: &WorkflowNodeV1,
    outputs: &HashMap<String, BTreeMap<String, TypedPortValue>>,
) -> Result<bool> {
    let evaluate = |condition: &super::BooleanCondition| -> Result<bool> {
        let output = outputs
            .get(&condition.node_id)
            .and_then(|ports| ports.get(&condition.port))
            .with_context(|| {
                format!(
                    "activation output '{}.{}' is unavailable",
                    condition.node_id, condition.port
                )
            })?;
        let value = output
            .value
            .as_bool()
            .context("validated boolean activation output changed type")?;
        Ok(value == condition.equals)
    };
    match &node.activation {
        ActivationPolicy::Always => Ok(true),
        ActivationPolicy::All(conditions) => {
            for condition in conditions {
                if !evaluate(condition)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        ActivationPolicy::Any(conditions) => {
            for condition in conditions {
                if evaluate(condition)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

fn resolve_inputs(
    node: &WorkflowNodeV1,
    outputs: &HashMap<String, BTreeMap<String, TypedPortValue>>,
) -> Result<BTreeMap<String, TypedPortValue>> {
    node.inputs
        .iter()
        .map(|(name, binding)| {
            let mut value = match binding {
                WorkflowInputBinding::Literal { kind, value } => TypedPortValue {
                    kind: *kind,
                    value: value.clone(),
                },
                WorkflowInputBinding::Output { node_id, port } => outputs
                    .get(node_id)
                    .and_then(|outputs| outputs.get(port))
                    .cloned()
                    .with_context(|| format!("resolve input '{name}' from '{node_id}.{port}'"))?,
            };
            value.validate()?;
            RedactionPolicy::from_environment().redact_value(&mut value.value);
            let encoded = serde_json::to_vec(&value).context("encode sanitized step input")?;
            if encoded.len() > MAX_PORT_VALUE_BYTES {
                bail!(
                    "step input '{name}' exceeds the sanitized {} byte limit",
                    MAX_PORT_VALUE_BYTES
                );
            }
            Ok((name.clone(), value))
        })
        .collect()
}

async fn cancel_active(catalog: &StepCatalog, contexts: &HashMap<String, StepExecutorContext>) {
    for context in contexts.values() {
        if let Some(registered) = catalog.get(&context.node.step_type, context.node.step_version) {
            if let Err(error) = registered.executor.cancel(context).await {
                tracing::warn!(node = context.node.id, error = %format!("{error:#}"), "cancel workflow step failed");
            }
        }
    }
}

fn step_failure(phase: WorkflowFailurePhase, error: anyhow::Error) -> WorkflowStepFailure {
    let (message, _) = RedactionPolicy::from_environment().redact_text(&format!("{error:#}"));
    WorkflowStepFailure {
        phase,
        message,
        technical: true,
    }
}

fn budget_violation<'a>(
    limits: &super::WorkflowLimits,
    reports: impl Iterator<Item = &'a WorkflowStepReport>,
) -> Option<String> {
    let reports = reports.collect::<Vec<_>>();
    if let Some(limit) = limits.max_cost_usd {
        let observed = reports
            .iter()
            .filter_map(|report| report.cost_usd)
            .sum::<f64>();
        if observed > limit {
            return Some(format!(
                "workflow cost budget exceeded: observed ${observed:.6}, limit ${limit:.6}"
            ));
        }
    }
    if let Some(limit) = limits.max_total_tokens {
        let observed = reports
            .iter()
            .filter_map(|report| report.metrics.as_ref())
            .map(|metrics| {
                metrics
                    .pointer("/totals/input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .saturating_add(
                        metrics
                            .pointer("/totals/output_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                    )
            })
            .fold(0_u64, u64::saturating_add);
        if observed > limit {
            return Some(format!(
                "workflow token budget exceeded: observed {observed}, limit {limit}"
            ));
        }
    }
    None
}

fn persist_checkpoint<'a>(
    store: &CheckpointStore,
    materialized: &MaterializedWorkflow,
    run_id: &str,
    attempt_id: &str,
    reports: &HashMap<String, WorkflowStepReport>,
    active: impl Iterator<Item = &'a String>,
) -> Result<ArtifactReference> {
    let steps = ordered_reports(materialized, reports.clone());
    store.persist(&checkpoint_value(
        materialized,
        run_id,
        attempt_id,
        &steps,
        active,
    ))
}

fn checkpoint_value<'a>(
    materialized: &MaterializedWorkflow,
    run_id: &str,
    attempt_id: &str,
    steps: &[WorkflowStepReport],
    active: impl Iterator<Item = &'a String>,
) -> WorkflowCheckpointV1 {
    let mut terminal_nodes = steps
        .iter()
        .filter_map(|step| step.status.terminal().then_some(step.node_id.clone()))
        .collect::<Vec<_>>();
    terminal_nodes.sort();
    let mut active_nodes = active.cloned().collect::<Vec<_>>();
    active_nodes.sort();
    WorkflowCheckpointV1 {
        schema_version: 1,
        workflow_id: materialized.definition.id.clone(),
        workflow_sha256: materialized.sha256.clone(),
        run_id: run_id.to_string(),
        attempt_id: attempt_id.to_string(),
        flow_snapshot: evidence_snapshot(materialized),
        updated_at: timestamp(),
        terminal_nodes,
        active_nodes,
        steps: steps.to_vec(),
    }
}

fn evidence_snapshot(materialized: &MaterializedWorkflow) -> Value {
    let tests = materialized
        .topological_order
        .iter()
        .filter_map(|id| {
            materialized
                .definition
                .nodes
                .iter()
                .find(|node| &node.id == id)
        })
        .map(|node| {
            serde_json::json!({
                "id": node.id,
                "semantic_test": node.step_type,
                "depends_on": node.depends_on,
                "required": node.required,
                "activation": node.activation,
                "dependency_policy": node.dependency_policy,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "kind": "rust_flow_evidence",
        "executable": false,
        "scenario_id": materialized.definition.id,
        "scenario_version": materialized.definition.scenario_version,
        "sha256": materialized.sha256,
        "tests": tests,
    })
}

fn ordered_reports(
    materialized: &MaterializedWorkflow,
    mut reports: HashMap<String, WorkflowStepReport>,
) -> Vec<WorkflowStepReport> {
    materialized
        .topological_order
        .iter()
        .map(|id| reports.remove(id).expect("validated report"))
        .collect()
}

fn aggregate_cost(steps: &[WorkflowStepReport]) -> Option<f64> {
    let costs = steps
        .iter()
        .filter_map(|step| step.cost_usd)
        .collect::<Vec<_>>();
    (!costs.is_empty()).then(|| costs.iter().sum())
}

fn materialize_criteria(
    declarations: &[super::WorkflowCriterionDeclaration],
    steps: &[WorkflowStepReport],
) -> Vec<WorkflowCriterionResult> {
    declarations
        .iter()
        .map(|declaration| {
            let step = steps
                .iter()
                .find(|step| step.node_id == declaration.producer_node_id)
                .expect("validated criterion producer");
            let observed = step
                .outputs
                .get(&declaration.output_port)
                .and_then(|output| {
                    serde_json::from_value::<WorkflowEvaluationResult>(output.value.clone()).ok()
                });
            let (outcome, summary, score, evidence_ids) = match observed {
                Some(observed) => (
                    observed.outcome,
                    observed.summary,
                    observed.score,
                    observed.evidence_ids,
                ),
                None => (
                    super::WorkflowEvaluationOutcome::NotEvaluated,
                    if step.status == WorkflowStepStatus::Skipped {
                        step.skip_reason.clone().unwrap_or_else(|| {
                            "criterion producer was skipped by workflow activation".into()
                        })
                    } else {
                        format!(
                            "criterion output '{}.{}' was not materialized",
                            declaration.producer_node_id, declaration.output_port
                        )
                    },
                    None,
                    Vec::new(),
                ),
            };
            WorkflowCriterionResult {
                id: declaration.id.clone(),
                weight: declaration.weight,
                producer_node_id: declaration.producer_node_id.clone(),
                output_port: declaration.output_port.clone(),
                advisory: declaration.advisory,
                outcome,
                summary,
                score,
                evidence_ids,
            }
        })
        .collect()
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::workflow::{
        ControlSource, PortValueKind, ReplayPolicy, StepOperationalKind, StepPortDescriptor,
        StepTypeDescriptor, WorkflowLimits,
    };

    struct Delayed {
        active: Arc<AtomicUsize>,
        maximum: Arc<AtomicUsize>,
    }

    struct Cancellable {
        cancel_called: Arc<AtomicUsize>,
        cleanup_called: Arc<AtomicUsize>,
    }

    struct CountingWorkflowCleanup(Arc<AtomicUsize>);

    #[async_trait]
    impl super::super::WorkflowCleanupHook for CountingWorkflowCleanup {
        async fn cleanup(&self, _context: &WorkflowCleanupContext) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[async_trait]
    impl super::super::StepExecutor for Cancellable {
        async fn execute(&self, mut context: StepExecutorContext) -> Result<StepExecutorOutput> {
            loop {
                if *context.cancellation.borrow() {
                    bail!("cancelled by workflow");
                }
                context.cancellation.changed().await?;
            }
        }

        async fn cancel(&self, _context: &StepExecutorContext) -> Result<()> {
            self.cancel_called.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn cleanup(&self, _context: &StepExecutorContext) -> Result<()> {
            self.cleanup_called.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[async_trait]
    impl super::super::StepExecutor for Delayed {
        async fn execute(&self, context: StepExecutorContext) -> Result<StepExecutorOutput> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(active, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(30)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(StepExecutorOutput {
                outputs: BTreeMap::from([(
                    "ok".into(),
                    TypedPortValue {
                        kind: PortValueKind::Boolean,
                        value: json!(context.node.config["ok"].as_bool().unwrap_or(true)),
                    },
                )]),
                metrics: context
                    .node
                    .config
                    .get("input_tokens")
                    .and_then(Value::as_u64)
                    .map(|input_tokens| {
                        json!({
                            "totals": {
                                "input_tokens": input_tokens,
                                "output_tokens": context.node.config["output_tokens"]
                                    .as_u64()
                                    .unwrap_or(0)
                            }
                        })
                    }),
                cost_usd: context.node.config.get("cost_usd").and_then(Value::as_f64),
                ..StepExecutorOutput::default()
            })
        }
    }

    fn catalog(active: Arc<AtomicUsize>, maximum: Arc<AtomicUsize>) -> StepCatalog {
        let mut catalog = StepCatalog::new();
        catalog
            .register(
                StepTypeDescriptor {
                    id: "test.delay".into(),
                    version: 1,
                    description: "delayed deterministic step".into(),
                    config_schema: json!({
                        "type": "object",
                        "properties": {
                            "ok": {"type": "boolean"},
                            "cost_usd": {"type": "number", "minimum": 0},
                            "input_tokens": {"type": "integer", "minimum": 0},
                            "output_tokens": {"type": "integer", "minimum": 0}
                        },
                        "additionalProperties": false
                    }),
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::from([(
                        "ok".into(),
                        StepPortDescriptor {
                            kind: PortValueKind::Boolean,
                            optional: false,
                            control_source: Some(ControlSource::Deterministic),
                        },
                    )]),
                    capabilities: Vec::new(),
                    required_functions: Vec::new(),
                    replay_policy: ReplayPolicy::Idempotent,
                    operational_kind: StepOperationalKind::Transformation,
                },
                Arc::new(Delayed { active, maximum }),
            )
            .unwrap();
        catalog
    }

    fn node(id: &str, dependencies: Vec<&str>, required: bool) -> WorkflowNodeV1 {
        WorkflowNodeV1 {
            id: id.into(),
            step_type: "test.delay".into(),
            step_version: 1,
            config: json!({}),
            depends_on: dependencies.into_iter().map(str::to_string).collect(),
            inputs: BTreeMap::new(),
            activation: ActivationPolicy::Always,
            dependency_policy: DependencyPolicy::Succeeded,
            required,
        }
    }

    fn cancellable_catalog(
        cancel_called: Arc<AtomicUsize>,
        cleanup_called: Arc<AtomicUsize>,
    ) -> StepCatalog {
        let mut catalog = StepCatalog::new();
        catalog
            .register(
                StepTypeDescriptor {
                    id: "test.cancellable".into(),
                    version: 1,
                    description: "cancellable test step".into(),
                    config_schema: json!({
                        "type": "object",
                        "additionalProperties": false
                    }),
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::new(),
                    capabilities: Vec::new(),
                    required_functions: Vec::new(),
                    replay_policy: ReplayPolicy::Idempotent,
                    operational_kind: StepOperationalKind::Transformation,
                },
                Arc::new(Cancellable {
                    cancel_called,
                    cleanup_called,
                }),
            )
            .unwrap();
        catalog
    }

    #[tokio::test]
    async fn executes_ready_nodes_in_parallel_and_joins_deterministically() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let catalog = Arc::new(catalog(active, maximum.clone()));
        let definition = WorkflowDefinitionV1 {
            schema_version: 1,
            id: "parallel.test".into(),
            scenario_version: 1,
            description: "parallel scheduler".into(),
            limits: WorkflowLimits {
                max_parallel: 2,
                ..WorkflowLimits::default()
            },
            nodes: vec![
                node("root_a", vec![], true),
                node("root_b", vec![], true),
                node("join", vec!["root_a", "root_b"], true),
            ],
            criteria: Vec::new(),
        };
        let output = tempfile::tempdir().unwrap();
        let report = execute_workflow(
            &definition,
            catalog,
            WorkflowExecutionRequest::local(output.path().to_path_buf()),
        )
        .await
        .unwrap();
        assert!(report.passed);
        assert!(maximum.load(Ordering::SeqCst) >= 2);
        assert_eq!(
            report
                .steps
                .iter()
                .map(|step| step.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["root_a", "root_b", "join"]
        );
        report.checkpoint.verify(output.path()).unwrap();
    }

    #[tokio::test]
    async fn terminal_join_runs_after_a_branch_is_skipped() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let catalog_instance = Arc::new(catalog(active, maximum));
        let root = node("root", vec![], true);
        let mut branch = node("branch", vec!["root"], false);
        branch.activation = ActivationPolicy::All(vec![super::super::BooleanCondition {
            node_id: "root".into(),
            port: "ok".into(),
            equals: false,
        }]);
        let mut join = node("join", vec!["branch"], true);
        join.dependency_policy = DependencyPolicy::Terminal;
        let definition = WorkflowDefinitionV1 {
            schema_version: 1,
            id: "branch.test".into(),
            scenario_version: 1,
            description: "branch scheduler".into(),
            limits: WorkflowLimits::default(),
            nodes: vec![root, branch, join],
            criteria: Vec::new(),
        };
        let output = tempfile::tempdir().unwrap();
        let report = execute_workflow(
            &definition,
            catalog_instance,
            WorkflowExecutionRequest::local(output.path().to_path_buf()),
        )
        .await
        .unwrap();
        assert_eq!(report.steps[1].status, WorkflowStepStatus::Skipped);
        assert_eq!(report.steps[2].status, WorkflowStepStatus::Succeeded);
    }

    #[tokio::test]
    async fn cancellation_stops_active_steps_and_still_runs_cleanup() {
        let cancel_called = Arc::new(AtomicUsize::new(0));
        let cleanup_called = Arc::new(AtomicUsize::new(0));
        let catalog = Arc::new(cancellable_catalog(
            cancel_called.clone(),
            cleanup_called.clone(),
        ));
        let mut active = node("active", vec![], true);
        active.step_type = "test.cancellable".into();
        let definition = WorkflowDefinitionV1 {
            schema_version: 1,
            id: "cancel.test".into(),
            scenario_version: 1,
            description: "cancel active workflow".into(),
            limits: WorkflowLimits::default(),
            nodes: vec![active],
            criteria: Vec::new(),
        };
        let output = tempfile::tempdir().unwrap();
        let (sender, cancellation) = watch::channel(false);
        let workflow_cleanup_called = Arc::new(AtomicUsize::new(0));
        let execution = execute_workflow(
            &definition,
            catalog,
            WorkflowExecutionRequest {
                output_dir: output.path().to_path_buf(),
                run_id: "cancel-run".into(),
                attempt_number: 1,
                cancellation,
                cleanup_hook: Arc::new(CountingWorkflowCleanup(workflow_cleanup_called.clone())),
            },
        );
        let cancel = async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            sender.send(true).unwrap();
        };
        let (report, _) = tokio::join!(execution, cancel);
        let report = report.unwrap();

        assert!(report.technical_failure);
        assert!(!report.passed);
        assert_eq!(report.steps[0].status, WorkflowStepStatus::Cancelled);
        assert!(cancel_called.load(Ordering::SeqCst) >= 1);
        assert!(cleanup_called.load(Ordering::SeqCst) >= 1);
        assert_eq!(workflow_cleanup_called.load(Ordering::SeqCst), 1);
        assert_eq!(report.cleanup.status, WorkflowCleanupStatus::Succeeded);
        report.checkpoint.verify(output.path()).unwrap();
    }

    #[tokio::test]
    async fn aggregate_cost_and_token_budgets_stop_the_workflow() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let catalog_instance = Arc::new(catalog(active, maximum));
        let mut expensive = node("expensive", vec![], true);
        expensive.config = json!({
            "cost_usd": 2.0,
            "input_tokens": 80,
            "output_tokens": 40
        });
        let definition = WorkflowDefinitionV1 {
            schema_version: 1,
            id: "budget.test".into(),
            scenario_version: 1,
            description: "aggregate budget enforcement".into(),
            limits: WorkflowLimits {
                max_cost_usd: Some(1.0),
                max_total_tokens: Some(100),
                ..WorkflowLimits::default()
            },
            nodes: vec![expensive],
            criteria: Vec::new(),
        };
        let output = tempfile::tempdir().unwrap();
        let report = execute_workflow(
            &definition,
            catalog_instance,
            WorkflowExecutionRequest::local(output.path().to_path_buf()),
        )
        .await
        .unwrap();

        assert!(!report.passed);
        assert!(report.technical_failure);
        assert_eq!(report.steps[0].status, WorkflowStepStatus::Failed);
        assert!(report.steps[0]
            .failures
            .iter()
            .any(|failure| failure.message.contains("cost budget exceeded")));
        report.checkpoint.verify(output.path()).unwrap();

        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let catalog = Arc::new(catalog(active, maximum));
        let mut token_heavy = node("token_heavy", vec![], true);
        token_heavy.config = json!({"input_tokens": 80, "output_tokens": 40});
        let token_definition = WorkflowDefinitionV1 {
            schema_version: 1,
            id: "token_budget.test".into(),
            scenario_version: 1,
            description: "aggregate token budget enforcement".into(),
            limits: WorkflowLimits {
                max_total_tokens: Some(100),
                ..WorkflowLimits::default()
            },
            nodes: vec![token_heavy],
            criteria: Vec::new(),
        };
        let report = execute_workflow(
            &token_definition,
            catalog,
            WorkflowExecutionRequest::local(output.path().to_path_buf()),
        )
        .await
        .unwrap();
        assert!(report.steps[0]
            .failures
            .iter()
            .any(|failure| failure.message.contains("token budget exceeded")));
    }

    #[test]
    fn asset_inventory_is_bounded_before_any_file_is_written() {
        let output = tempfile::tempdir().unwrap();
        let (_sender, cancellation) = watch::channel(false);
        let context = StepExecutorContext {
            workflow_id: "asset.test".into(),
            workflow_sha256: "sha256:test".into(),
            run_id: "run".into(),
            attempt_id: "attempt".into(),
            node: node("producer", vec![], true),
            inputs: BTreeMap::new(),
            output_dir: output.path().to_path_buf(),
            cancellation,
        };
        let assets = (0..=MAX_WORKFLOW_ASSETS_PER_STEP)
            .map(|index| CapturedWorkflowAsset {
                id: format!("asset_{index}"),
                kind: "test".into(),
                media_type: "application/json".into(),
                content: WorkflowAssetContent::Json(json!({"index": index})),
                provenance: Vec::new(),
            })
            .collect();

        let error = persist_assets(&context, assets).unwrap_err().to_string();
        assert!(error.contains("limit is 64"), "{error}");
        assert!(!output.path().join("deliverables").exists());
    }

    #[test]
    fn literal_inputs_are_redacted_and_bounded_before_executor_delivery() {
        let mut consumer = node("consumer", vec![], true);
        consumer.inputs.insert(
            "data".into(),
            WorkflowInputBinding::Literal {
                kind: PortValueKind::Json,
                value: json!({"password": "do-not-deliver", "value": 7}),
            },
        );
        let inputs = resolve_inputs(&consumer, &HashMap::new()).unwrap();
        assert_eq!(inputs["data"].value["password"], "[REDACTED]");
        assert_eq!(inputs["data"].value["value"], 7);
    }

    #[test]
    fn skipped_optional_criterion_is_not_evaluated_without_an_invented_score() {
        let optional = node("optional", vec![], false);
        let step = WorkflowStepReport::skipped(&optional, "branch condition was false");
        let criteria = materialize_criteria(
            &[super::super::WorkflowCriterionDeclaration {
                id: "optional_quality".into(),
                weight: 100,
                producer_node_id: "optional".into(),
                output_port: "assessment".into(),
                advisory: true,
            }],
            &[step],
        );

        assert_eq!(
            criteria[0].outcome,
            super::super::WorkflowEvaluationOutcome::NotEvaluated
        );
        assert!(criteria[0].score.is_none());
        assert!(criteria[0].summary.contains("branch condition"));
    }
}
