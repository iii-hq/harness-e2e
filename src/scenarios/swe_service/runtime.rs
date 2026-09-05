use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use iii_sdk::{runtime::FunctionRef, RegisterFunction};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::wire::{
    FunctionPolicy, MessageInput, SendOptions, SendRequest, SendResponse, SessionInit,
    SessionMetricsResponse, SessionTreeResponse,
};
use crate::workflow::{
    CapturedWorkflowAsset, PortValueKind, StepCatalog, StepEvaluation, StepExecutor,
    StepExecutorContext, StepExecutorOutput, TypedPortValue, WorkflowAssetContent,
    WorkflowCleanupContext, WorkflowCleanupHook, WorkflowEvaluationOutcome,
    WorkflowEvaluationResult, WorkflowGateResult, WorkflowTerminationReason,
};

use super::{assets, workflow, Case, FIXTURE_REVISION, REPORT_ID, WORKSPACE_ROOT_ENV};

const CLEANUP_SECONDS: u64 = 300;

#[derive(Clone)]
struct Attempt {
    private_root: PathBuf,
    workspace: PathBuf,
    state_file: PathBuf,
    output_dir: PathBuf,
    attempt_id: String,
    session_id: String,
    checkpoint_id: String,
    prepared: Value,
}

#[derive(Default)]
struct SharedState {
    attempt: Option<Attempt>,
    registration: Option<FunctionRef>,
    stop_reason: Option<String>,
    metrics: Option<Value>,
    transcript: Option<Value>,
    final_report: Option<Value>,
    send_attempted: bool,
    shutdown_deadline: Option<tokio::time::Instant>,
    tree_stopped: bool,
}

struct Shared {
    harness: Arc<dyn Harness>,
    stopping: tokio::sync::Mutex<()>,
    model: String,
    provider: String,
    case: Case,
    state: Mutex<SharedState>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CheckpointRequest {
    #[serde(rename = "_caller_worker_id", default)]
    #[schemars(skip)]
    _caller_worker_id: Option<String>,
    ticket: u8,
    head: String,
    #[serde(default)]
    revision_id: Option<String>,
}

struct Executor {
    shared: Arc<Shared>,
    step: &'static str,
}

pub fn register(
    catalog: &mut StepCatalog,
    case: Case,
    context: Arc<E2eContext>,
    model: &str,
    provider: &str,
) -> Result<Arc<dyn WorkflowCleanupHook>> {
    let shared = Arc::new(Shared {
        harness: context,
        stopping: tokio::sync::Mutex::new(()),
        model: model.into(),
        provider: provider.into(),
        case,
        state: Mutex::new(SharedState::default()),
    });
    for descriptor in workflow::descriptors() {
        let step = match descriptor.id.as_str() {
            workflow::PREPARE => workflow::PREPARE,
            workflow::SUBJECT => workflow::SUBJECT,
            _ => workflow::CAPTURE,
        };
        catalog.register(
            descriptor,
            Arc::new(Executor {
                shared: shared.clone(),
                step,
            }),
        )?;
    }
    Ok(shared)
}

#[async_trait]
trait Harness: Send + Sync {
    fn client(&self) -> &iii_sdk::IIIClient;
    async fn trigger_value(&self, function: &str, payload: Value) -> Result<Value>;
    async fn send(&self, request: SendRequest) -> Result<SendResponse>;
    async fn wait(
        &self,
        case: Case,
        session: &str,
        turn: &str,
        cancellation: &tokio::sync::watch::Receiver<bool>,
    ) -> Result<SessionMetricsResponse>;
    async fn metrics(&self, session: &str) -> Result<SessionMetricsResponse>;
    async fn transcript(&self, session: &str) -> Result<Value>;
    async fn tree(&self, session: &str) -> Result<Vec<String>>;
    async fn stop(&self, session: &str) -> Result<()>;
    async fn teardown(&self, session: &str) -> Result<()>;
}

#[async_trait]
impl Harness for E2eContext {
    fn client(&self) -> &iii_sdk::IIIClient {
        E2eContext::client(self)
    }
    async fn trigger_value(&self, function: &str, payload: Value) -> Result<Value> {
        E2eContext::trigger_value(self, function, payload).await
    }
    async fn send(&self, request: SendRequest) -> Result<SendResponse> {
        self.trigger("harness::send", request).await
    }
    async fn wait(
        &self,
        case: Case,
        session: &str,
        turn: &str,
        cancellation: &tokio::sync::watch::Receiver<bool>,
    ) -> Result<SessionMetricsResponse> {
        self.wait_for_turn(
            case.id,
            session,
            turn,
            Duration::from_secs(600),
            true,
            Some(cancellation),
        )
        .await
    }
    async fn metrics(&self, session: &str) -> Result<SessionMetricsResponse> {
        E2eContext::metrics(self, session).await
    }
    async fn transcript(&self, session: &str) -> Result<Value> {
        E2eContext::transcript(self, session).await
    }
    async fn tree(&self, session: &str) -> Result<Vec<String>> {
        Ok(self
            .trigger::<_, SessionTreeResponse>(
                "harness::session-tree",
                json!({"root_session_id":session}),
            )
            .await?
            .sessions
            .iter()
            .map(|session| session.session_id.clone())
            .collect())
    }
    async fn stop(&self, session: &str) -> Result<()> {
        self.stop_session(session, None).await
    }
    async fn teardown(&self, session: &str) -> Result<()> {
        E2eContext::teardown(self, session).await.map(|_| ())
    }
}

impl Shared {
    fn attempt(&self) -> Result<Attempt> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .attempt
            .clone()
            .context("SWE attempt has not been prepared")
    }

    fn stop_reason(&self, reason: &str) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stop_reason
            .get_or_insert_with(|| reason.into());
    }

    async fn prepare(
        self: &Arc<Self>,
        execution: &StepExecutorContext,
    ) -> Result<StepExecutorOutput> {
        if self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .attempt
            .is_some()
        {
            bail!("SWE prepare cannot replace an active workspace");
        }
        safe_component(&execution.attempt_id)?;
        let private_root = execution
            .output_dir
            .join(".swe-runtime")
            .join(&execution.attempt_id);
        if private_root.exists() {
            bail!("SWE attempt directory already exists");
        }
        let workspace_parent = std::env::var_os(WORKSPACE_ROOT_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("harness-e2e-swe-workspaces"));
        if !workspace_parent.is_absolute() {
            bail!("{WORKSPACE_ROOT_ENV} must be an absolute path");
        }
        std::fs::create_dir_all(&workspace_parent)?;
        let workspace = workspace_parent.canonicalize()?.join(&execution.attempt_id);
        std::fs::create_dir(&workspace).context("reserve unique SWE workspace")?;
        std::fs::create_dir_all(&private_root)?;
        let private_root = private_root.canonicalize()?;
        let state_file = private_root.join("state.json");
        let mut attempt = Attempt {
            private_root: private_root.clone(),
            workspace: workspace.clone(),
            state_file: state_file.clone(),
            output_dir: execution.output_dir.clone(),
            attempt_id: execution.attempt_id.clone(),
            session_id: format!("swe_{}", execution.attempt_id),
            checkpoint_id: format!("e2etest::swe_checkpoint_{}", execution.attempt_id),
            prepared: Value::Null,
        };
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .attempt = Some(attempt.clone());
        let source = assets::unpack(&private_root).await?;
        let prepared = assets::controller(
            &private_root,
            &[
                "prepare".into(),
                "--fixture-root".into(),
                source.to_string_lossy().into_owned(),
                "--workspace".into(),
                workspace.to_string_lossy().into_owned(),
                "--state-file".into(),
                state_file.to_string_lossy().into_owned(),
                "--probes".into(),
                private_root
                    .join("probes.py")
                    .to_string_lossy()
                    .into_owned(),
                "--isolation".into(),
                private_root
                    .join("isolation.py")
                    .to_string_lossy()
                    .into_owned(),
                "--mode".into(),
                self.case.mode().into(),
                "--ticket".into(),
                self.case.first_ticket().to_string(),
                "--fixture-revision".into(),
                FIXTURE_REVISION.into(),
                "--run-id".into(),
                execution.run_id.clone(),
            ],
        )
        .await?;
        attempt.prepared = prepared;
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .attempt = Some(attempt.clone());
        self.verify_boundary(&attempt).await?;
        let callback = self.clone();
        let registration = self.harness.client().register_function(
            attempt.checkpoint_id.clone(),
            RegisterFunction::new_async(move |request: CheckpointRequest| {
                let shared = callback.clone();
                async move {
                    let result = shared.checkpoint(request).await;
                    match result {
                        Ok(value) => Ok::<Value, iii_sdk::errors::Error>(value),
                        Err(error) => {
                            shared.stop_reason("infrastructure_error");
                            Ok(json!({"status":"infrastructure_error","feedback":format!("{error:#}")}))
                        }
                    }
                }
            }).description("Submit a committed SWE ticket. Returns factual acceptance, a revision to acknowledge, or the next ticket. Supply ticket, full head SHA, and revision_id only when acknowledging revealed requirements."),
        );
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .registration = Some(registration);
        Ok(completed(true))
    }

    async fn verify_boundary(&self, attempt: &Attempt) -> Result<()> {
        let canary = attempt.private_root.join("isolation-canary");
        let secret = uuid::Uuid::new_v4().to_string();
        std::fs::write(&canary, &secret)?;
        // This is a trusted preflight, never placed in the subject transcript.
        let script = "import json,pathlib,sys; p=pathlib.Path(sys.argv[1]); ok=p.is_file(); q=pathlib.Path(sys.argv[2]);\ntry:\n q.read_bytes(); isolated=False\nexcept (PermissionError,FileNotFoundError):\n isolated=True\nprint(json.dumps({'workspace_readable':ok,'trusted_unreadable':isolated}))";
        let output = self.harness.trigger_value("shell::exec", json!({
            "command":"python3", "args":["-I","-c",script,attempt.workspace.join("README.md").to_string_lossy(),canary.to_string_lossy()],
            "cwd":attempt.workspace, "timeout_ms":15000,
            "fs_scope":{"root":attempt.workspace,"grants":[],"boundary":"workspace"},
        })).await.context("SWE requires a workspace-only isolated shell worker")?;
        if output.get("exit_code").and_then(Value::as_i64) != Some(0) {
            bail!("SWE isolated shell cannot execute the workspace boundary preflight");
        }
        let value: Value =
            serde_json::from_str(output.get("stdout").and_then(Value::as_str).unwrap_or(""))
                .context("SWE shell returned invalid isolation evidence")?;
        if value.get("workspace_readable") != Some(&Value::Bool(true))
            || value.get("trusted_unreadable") != Some(&Value::Bool(true))
        {
            bail!("SWE shell must see the exported workspace and must not read controller state or future snapshots");
        }
        // Fail before model calls when the trusted code verifier has no OS boundary.
        let baseline = assets::command(
            "python3",
            &[
                "-I".into(),
                attempt
                    .private_root
                    .join("isolation.py")
                    .to_string_lossy()
                    .into_owned(),
                "--workspace".into(),
                attempt.workspace.to_string_lossy().into_owned(),
                "--probes".into(),
                attempt
                    .private_root
                    .join("probes.py")
                    .to_string_lossy()
                    .into_owned(),
                "--through".into(),
                (self.case.first_ticket() - 1).to_string(),
            ],
            Duration::from_secs(245),
        )
        .await?;
        let baseline: Value =
            serde_json::from_slice(&baseline).context("invalid SWE baseline evidence")?;
        if baseline.get("passed") != Some(&Value::Bool(true)) {
            bail!("SWE selected entry snapshot did not satisfy its accepted reference prefix");
        }
        Ok(())
    }

    async fn checkpoint(&self, request: CheckpointRequest) -> Result<Value> {
        if !(1..=8).contains(&request.ticket)
            || request.head.len() != 40
            || !request.head.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Ok(
                json!({"status":"rejected","feedback":"Supply a valid ticket number and the full committed HEAD SHA."}),
            );
        }
        let attempt = self.attempt()?;
        let mut args = vec![
            "checkpoint".into(),
            "--state-file".into(),
            attempt.state_file.to_string_lossy().into_owned(),
            "--ticket".into(),
            request.ticket.to_string(),
            "--head".into(),
            request.head,
        ];
        if let Some(revision) = request.revision_id {
            args.extend(["--revision-id".into(), revision]);
        }
        assets::controller(&attempt.private_root, &args).await
    }

    async fn subject(&self, execution: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let attempt = self.attempt()?;
        let prompt = format!(
            "Work as the software engineer responsible for the service in {}. Read its public contracts, investigate the request, implement it, add your own regression tests under tests/agent, and maintain useful documentation. You may delegate when useful; you remain responsible for integration. Only this workspace is authorized. Preserve tests/reference and benchmark controls.\n\n{}\n\nDeliver a clean committed change by calling {} with the current ticket number and full HEAD SHA. Preserve accepted commits. If requirements are revealed, acknowledge their revision_id on the next submission; a compatible implementation may reuse the same SHA. Continue in this same session when a next ticket is returned. On rejected, address the evidence; on completed or capability_failure, stop and summarize the last accepted work. Do not invent future tickets.",
            attempt.workspace.display(), attempt.prepared.get("prompt").and_then(Value::as_str).context("missing first ticket")?, attempt.checkpoint_id,
        );
        // Harness may accept the unique ID even when its response is lost or malformed.
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .send_attempted = true;
        let response = self
            .harness
            .send(SendRequest {
                session_id: Some(attempt.session_id.clone()),
                message: MessageInput::Text(prompt),
                model: Some(self.model.clone()),
                provider: Some(self.provider.clone()),
                idempotency_key: Some(format!("swe:{}:{}", execution.run_id, execution.attempt_id)),
                session: Some(SessionInit {
                    title: Some(self.case.description().into()),
                    metadata: Some(
                        json!({"e2e_scenario":self.case.id,"e2e_attempt_id":execution.attempt_id}),
                    ),
                }),
                options: Some(SendOptions {
                    max_turns: Some(self.case.generations()),
                    max_output_tokens: Some(32_768),
                    max_total_tokens: Some(self.case.tokens()),
                    max_validation_retries: None,
                    functions: Some(FunctionPolicy {
                        allow: [
                            "engine::functions::list",
                            "engine::functions::info",
                            "engine::triggers::list",
                            "engine::triggers::info",
                            "coder::*",
                            "shell::*",
                            "harness::spawn",
                            "harness::status",
                            "harness::session-tree",
                            "harness::trigger::*",
                            "state::*",
                        ]
                        .into_iter()
                        .map(str::to_string)
                        .chain([attempt.checkpoint_id.clone()])
                        .collect(),
                        deny: [
                            "e2e::*",
                            "github::*",
                            "configuration::*",
                            "compose::*",
                            "router::*",
                            "harness::send",
                            "harness::run",
                        ]
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                        ..FunctionPolicy::default()
                    }),
                    metadata: Some(json!({"fs_scope":{"root":attempt.workspace}})),
                }),
            })
            .await?;
        if !response.accepted
            || response.session_id != attempt.session_id
            || response.merged == Some(true)
            || response.queued == Some(true)
        {
            bail!("SWE Harness session was not accepted independently");
        }
        let waiting = self.harness.wait(
            self.case,
            &attempt.session_id,
            &response.turn_id,
            &execution.cancellation,
        );
        tokio::pin!(waiting);
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        let result = loop {
            tokio::select! {
                result = &mut waiting => break result,
                _ = interval.tick() => {
                    let stored_stop = self.state.lock().unwrap_or_else(|error| error.into_inner()).stop_reason.clone();
                    if stored_stop.is_some() {
                        self.stop_tree(&attempt.session_id).await;
                        break Err(anyhow::anyhow!("SWE trusted checkpoint failed"));
                    }
                    match self.harness.metrics(&attempt.session_id).await {
                        Ok(metrics) => {
                            self.state.lock().unwrap_or_else(|error| error.into_inner()).metrics = Some(serde_json::to_value(&metrics)?);
                            if aggregate_limit(&metrics, self.case).is_some() {
                                self.stop_reason("resource_limit");
                                self.stop_tree(&attempt.session_id).await;
                                break Err(anyhow::anyhow!("SWE aggregate generation or token limit reached"));
                            }
                        },
                        Err(error) => tracing::warn!(error = %error, "SWE resource watchdog could not sample metrics"),
                    }
                },
            }
        };
        let metrics = match result {
            Ok(metrics) => Some(metrics),
            Err(error) => {
                self.stop_reason(termination_status(execution, "infrastructure_error"));
                self.stop_tree(&attempt.session_id).await;
                tracing::warn!(error = %error, "SWE subject ended before completing its workflow");
                tokio::time::timeout_at(
                    self.phase_deadline(75),
                    self.harness.metrics(&attempt.session_id),
                )
                .await
                .ok()
                .and_then(Result::ok)
            }
        };
        // The completion sample may beat the independent five-second watchdog.
        if let Some(ref metrics) = metrics {
            if aggregate_limit(metrics, self.case).is_some() {
                self.stop_reason("resource_limit");
                self.stop_tree(&attempt.session_id).await;
            }
        }
        let transcript = tokio::time::timeout(
            Duration::from_secs(10),
            self.harness.transcript(&attempt.session_id),
        )
        .await
        .ok()
        .and_then(Result::ok);
        {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(ref metrics) = metrics {
                state.metrics = Some(serde_json::to_value(metrics)?);
            }
            state.transcript = transcript.clone();
        }
        Ok(StepExecutorOutput {
            transcript,
            metrics: metrics.as_ref().map(serde_json::to_value).transpose()?,
            cost_usd: metrics.as_ref().and_then(|value| value.totals.cost_usd),
            harness_session_id: Some(attempt.session_id.clone()),
            ..completed(true)
        })
    }

    fn shutdown_deadline(&self) -> tokio::time::Instant {
        *self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .shutdown_deadline
            .get_or_insert_with(|| {
                tokio::time::Instant::now() + Duration::from_secs(CLEANUP_SECONDS)
            })
    }

    fn phase_deadline(&self, seconds: u64) -> tokio::time::Instant {
        self.shutdown_deadline() - Duration::from_secs(CLEANUP_SECONDS - seconds)
    }

    async fn stop_tree(&self, session_id: &str) {
        // All callers share the original shutdown deadline; repeated cancellation cannot renew it.
        let deadline = self.phase_deadline(60);
        let stop = async {
            let _lock = self.stopping.lock().await;
            if self
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .tree_stopped
            {
                return;
            }
            let tree =
                tokio::time::timeout(Duration::from_secs(10), self.harness.tree(session_id)).await;
            let mut sessions = tree.ok().and_then(Result::ok).unwrap_or_default();
            if !sessions.iter().any(|id| id == session_id) {
                sessions.push(session_id.into());
            }
            // A slow child cannot consume a fresh ten seconds per descendant.
            futures_util::future::join_all(sessions.iter().rev().map(|id| async {
                let _ = tokio::time::timeout(Duration::from_secs(10), self.harness.stop(id)).await;
            }))
            .await;
            self.state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .tree_stopped = true;
        };
        let _ = tokio::time::timeout_at(deadline, stop).await;
    }

    async fn capture_report(&self) -> Result<Value> {
        let attempt = self.attempt()?;
        if self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .send_attempted
        {
            self.stop_tree(&attempt.session_id).await;
        }
        tokio::time::timeout_at(self.phase_deadline(180), self.capture_report_inner())
            .await
            .context("SWE final capture exhausted shutdown budget")?
    }

    async fn capture_report_inner(&self) -> Result<Value> {
        let attempt = self.attempt()?;
        let reason = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stop_reason
            .clone();
        let mut args = vec![
            "capture".into(),
            "--state-file".into(),
            attempt.state_file.to_string_lossy().into_owned(),
        ];
        args.extend([
            "--terminal-status".into(),
            reason
                .clone()
                .unwrap_or_else(|| "capability_failure".into()),
        ]);
        // Harness turn cancellation alone does not stop background workspace processes.
        assets::controller(
            &attempt.private_root,
            &[
                "quiesce".into(),
                "--state-file".into(),
                attempt.state_file.to_string_lossy().into_owned(),
            ],
        )
        .await?;
        let report = assets::controller(&attempt.private_root, &args).await?;
        self.persist_report(report)
    }

    fn persist_report(&self, mut report: Value) -> Result<Value> {
        let attempt = self.attempt()?;
        let reason = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stop_reason
            .clone();
        report["scenario_id"] = self.case.id.into();
        report["attempt_id"] = attempt.attempt_id.clone().into();
        report["session_id"] = attempt.session_id.clone().into();
        if let Some(reason) = reason {
            report["terminal_status"] = reason.into();
        }
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(ref metrics) = state.metrics {
            report["metrics"] = metrics.clone();
        }
        if let Some(ref transcript) = state.transcript {
            report["transcript"] = transcript.clone();
        }
        drop(state);
        let path = final_report_path(&attempt.output_dir, &attempt.attempt_id);
        std::fs::create_dir_all(path.parent().context("SWE report parent")?)?;
        crate::artifact::write_json(
            &attempt.output_dir,
            path.strip_prefix(&attempt.output_dir)?,
            format!("{}-swe-report", attempt.attempt_id),
            "swe-service-report",
            &report,
        )?;
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .final_report = Some(report.clone());
        Ok(report)
    }
}

#[async_trait]
impl StepExecutor for Executor {
    async fn execute(&self, context: StepExecutorContext) -> Result<StepExecutorOutput> {
        let result = match self.step {
            workflow::PREPARE => self.shared.prepare(&context).await,
            workflow::SUBJECT => self.shared.subject(&context).await,
            _ => {
                let report = self.shared.capture_report().await?;
                let passed =
                    report.get("terminal_status").and_then(Value::as_str) == Some("completed");
                let accepted = report
                    .get("accepted_tickets")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                let total = if self.shared.case.journey() { 8 } else { 1 };
                let evaluation = WorkflowEvaluationResult {
                    id: "swe_delivery".into(),
                    outcome: if passed {
                        WorkflowEvaluationOutcome::Passed
                    } else {
                        WorkflowEvaluationOutcome::Failed
                    },
                    summary: format!(
                        "{accepted}/{total} committed SWE tickets accepted; terminal={}",
                        report["terminal_status"]
                    ),
                    score: Some(accepted.min(total) as f64 / total as f64),
                    evidence_ids: vec![REPORT_ID.into()],
                };
                Ok(StepExecutorOutput {
                    outputs: BTreeMap::from([(
                        "delivery".into(),
                        TypedPortValue {
                            kind: PortValueKind::Assessment,
                            value: serde_json::to_value(&evaluation)?,
                        },
                    )]),
                    captured_assets: vec![CapturedWorkflowAsset {
                        id: REPORT_ID.into(),
                        kind: "swe-service-report".into(),
                        media_type: "application/json".into(),
                        content: WorkflowAssetContent::Json(report),
                        provenance: Vec::new(),
                    }],
                    evaluation: StepEvaluation {
                        hard_gates: vec![WorkflowGateResult {
                            id: "delivery_complete".into(),
                            passed,
                            reason: evaluation.summary.clone(),
                            evidence_ids: vec![REPORT_ID.into()],
                        }],
                        evaluations: vec![evaluation],
                    },
                    ..StepExecutorOutput::default()
                })
            }
        };
        if result.is_err() {
            self.shared.stop_reason("infrastructure_error");
        }
        result
    }

    async fn evaluate(
        &self,
        _context: &StepExecutorContext,
        execution: &StepExecutorOutput,
        _assets: &[CapturedWorkflowAsset],
    ) -> Result<StepEvaluation> {
        Ok(execution.evaluation.clone())
    }

    async fn cancel(&self, context: &StepExecutorContext) -> Result<()> {
        self.shared
            .stop_reason(termination_status(context, "resource_limit"));
        if let Ok(attempt) = self.shared.attempt() {
            self.shared.stop_tree(&attempt.session_id).await;
        }
        Ok(())
    }
}

#[async_trait]
impl WorkflowCleanupHook for Shared {
    async fn cleanup(&self, _context: &WorkflowCleanupContext) -> Result<()> {
        let Ok(attempt) = self.attempt() else {
            return Ok(());
        };
        let cleanup = async {
            if !attempt.state_file.is_file() {
                // No model was started: these directories were reserved exclusively by prepare.
                if attempt.workspace.exists() {
                    std::fs::remove_dir_all(&attempt.workspace)?;
                }
                if attempt.private_root.exists() {
                    std::fs::remove_dir_all(&attempt.private_root)?;
                }
                return Ok(());
            }
            let send_attempted = self
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .send_attempted;
            if send_attempted {
                self.stop_tree(&attempt.session_id).await;
            }
            let capture = tokio::time::timeout_at(self.phase_deadline(180), self.capture_report())
                .await
                .context("SWE final capture exhausted shutdown budget")
                .and_then(|result| result);
            if let Some(registration) = self
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .registration
                .take()
            {
                registration.unregister();
            }
            let teardown = if send_attempted {
                tokio::time::timeout_at(
                    self.phase_deadline(210),
                    self.harness.teardown(&attempt.session_id),
                )
                .await
                .context("SWE teardown exhausted shutdown budget")
                .and_then(|result| result)
            } else {
                Ok(())
            };
            // Always try OS cleanup even if capture or Harness teardown failed.
            let cleanup = tokio::time::timeout_at(
                self.phase_deadline(295),
                assets::controller(
                    &attempt.private_root,
                    &[
                        "cleanup".into(),
                        "--state-file".into(),
                        attempt.state_file.to_string_lossy().into_owned(),
                    ],
                ),
            )
            .await
            .context("SWE OS cleanup exhausted shutdown budget")
            .and_then(|result| result);
            if cleanup.is_ok() {
                // Controller cleanup captures again after stopping processes. Publish THAT evidence.
                let refreshed =
                    std::fs::read(format!("{}.report.json", attempt.state_file.display()))?;
                self.persist_report(serde_json::from_slice(&refreshed)?)?;
            }
            cleanup?;
            capture?;
            teardown?;
            std::fs::remove_dir_all(&attempt.private_root)?;
            Ok(())
        };
        tokio::time::timeout_at(self.shutdown_deadline(), cleanup)
            .await
            .context("SWE cleanup exceeded five minutes")?
    }
}

fn termination_status(context: &StepExecutorContext, fallback: &'static str) -> &'static str {
    match context.termination.reason() {
        Some(WorkflowTerminationReason::Deadline) => "resource_limit",
        Some(WorkflowTerminationReason::Cancelled) => "cancelled",
        None if *context.cancellation.borrow() => "cancelled",
        None => fallback,
    }
}

fn completed(value: bool) -> StepExecutorOutput {
    StepExecutorOutput {
        outputs: BTreeMap::from([(
            "completed".into(),
            TypedPortValue {
                kind: PortValueKind::Boolean,
                value: Value::Bool(value),
            },
        )]),
        ..Default::default()
    }
}

pub(crate) fn final_report_path(output: &Path, attempt: &str) -> PathBuf {
    output
        .join("deliverables")
        .join(attempt)
        .join("swe_service_report.json")
}

fn safe_component(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("invalid SWE attempt identifier");
    }
    Ok(())
}

fn aggregate_limit(metrics: &SessionMetricsResponse, case: Case) -> Option<&'static str> {
    if metrics.totals.turns > u64::from(case.generations()) {
        return Some("generations");
    }
    if metrics
        .totals
        .input_tokens
        .zip(metrics.totals.output_tokens)
        .is_some_and(|(input, output)| input.saturating_add(output) > case.tokens())
    {
        return Some("tokens");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{SessionMetricsPayload, SessionUsageTotals};

    fn metrics(turns: u64, input: u64, output: u64) -> SessionMetricsResponse {
        SessionMetricsResponse::from_normalized(SessionMetricsPayload {
            root_session_id: "parent".into(),
            complete: false,
            totals: SessionUsageTotals {
                sessions: 3,
                turns,
                input_tokens: Some(input),
                output_tokens: Some(output),
                ..Default::default()
            },
            by_session: Vec::new(),
            traces: None,
        })
    }

    struct FakeHarness {
        lose_send: bool,
        wait_for_cancel: bool,
        final_metrics: SessionMetricsResponse,
        calls: Mutex<Vec<String>>,
        slow_stops: bool,
    }

    impl FakeHarness {
        fn new(turns: u64, input: u64, output: u64) -> Self {
            Self {
                lose_send: false,
                wait_for_cancel: false,
                final_metrics: metrics(turns, input, output),
                calls: Mutex::new(Vec::new()),
                slow_stops: false,
            }
        }
        fn record(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }
    }

    #[async_trait]
    impl Harness for FakeHarness {
        fn client(&self) -> &iii_sdk::IIIClient {
            panic!("fixture does not register RPC handlers")
        }
        async fn trigger_value(&self, _: &str, _: Value) -> Result<Value> {
            bail!("unexpected boundary call")
        }
        async fn send(&self, request: SendRequest) -> Result<SendResponse> {
            let id = request.session_id.unwrap();
            self.record(format!("send:{id}"));
            if self.lose_send {
                bail!("response lost after session accepted");
            }
            Ok(SendResponse::from_normalized(
                crate::wire::SendResponsePayload {
                    session_id: id,
                    turn_id: "turn-1".into(),
                    accepted: true,
                    merged: None,
                    queued: None,
                    deduplicated: None,
                },
            ))
        }
        async fn wait(
            &self,
            _: Case,
            _: &str,
            _: &str,
            cancellation: &tokio::sync::watch::Receiver<bool>,
        ) -> Result<SessionMetricsResponse> {
            if self.wait_for_cancel {
                let mut cancellation = cancellation.clone();
                while !*cancellation.borrow() {
                    cancellation.changed().await?;
                }
                bail!("opaque transport abort; no timeout keywords");
            }
            Ok(self.final_metrics.clone())
        }
        async fn metrics(&self, _: &str) -> Result<SessionMetricsResponse> {
            Ok(metrics(1, 1, 1))
        }
        async fn transcript(&self, _: &str) -> Result<Value> {
            Ok(json!({"messages":[]}))
        }
        async fn tree(&self, session: &str) -> Result<Vec<String>> {
            self.record(format!("tree:{session}"));
            Ok((0..30)
                .map(|id| format!("child-{id}"))
                .chain([session.into()])
                .collect())
        }
        async fn stop(&self, session: &str) -> Result<()> {
            self.record(format!("stop:{session}"));
            if self.slow_stops {
                tokio::time::sleep(Duration::from_secs(600)).await;
            }
            Ok(())
        }
        async fn teardown(&self, session: &str) -> Result<()> {
            self.record(format!("teardown:{session}"));
            Ok(())
        }
    }

    async fn fixture(
        api: Arc<FakeHarness>,
    ) -> (tempfile::TempDir, Arc<Shared>, StepExecutorContext) {
        let temp = tempfile::tempdir().unwrap();
        let private_root = temp.path().join("trusted");
        let source = assets::unpack(&private_root).await.unwrap();
        let workspace = temp.path().join("workspace");
        let state_file = private_root.join("state.json");
        // Prepare/capture/cleanup are production controller calls over the pinned fixture.
        let prepared = assets::controller(
            &private_root,
            &[
                "prepare".into(),
                "--fixture-root".into(),
                source.to_string_lossy().into_owned(),
                "--workspace".into(),
                workspace.to_string_lossy().into_owned(),
                "--state-file".into(),
                state_file.to_string_lossy().into_owned(),
                "--probes".into(),
                private_root
                    .join("probes.py")
                    .to_string_lossy()
                    .into_owned(),
                "--isolation".into(),
                private_root
                    .join("isolation.py")
                    .to_string_lossy()
                    .into_owned(),
                "--mode".into(),
                "isolated".into(),
                "--ticket".into(),
                "1".into(),
                "--fixture-revision".into(),
                FIXTURE_REVISION.into(),
                "--run-id".into(),
                "test-run".into(),
            ],
        )
        .await
        .unwrap();
        let definition = workflow::definition(crate::scenarios::ScenarioId::SweConfigIsolation);
        let context = StepExecutorContext {
            workflow_id: definition.id.clone(),
            workflow_sha256: definition.canonical_sha256().unwrap(),
            run_id: "test-run".into(),
            attempt_id: "attempt-test".into(),
            node: definition.nodes[1].clone(),
            replay_policy: crate::workflow::ReplayPolicy::NonRepeatable,
            inputs: BTreeMap::new(),
            output_dir: temp.path().join("output"),
            cancellation: tokio::sync::watch::channel(false).1,
            termination: Default::default(),
        };
        let shared = Arc::new(Shared {
            harness: api,
            stopping: tokio::sync::Mutex::new(()),
            model: "test".into(),
            provider: "test".into(),
            case: Case {
                ticket: 1,
                id: "swe_config_isolation",
            },
            state: Mutex::new(SharedState {
                attempt: Some(Attempt {
                    private_root,
                    workspace,
                    state_file,
                    output_dir: context.output_dir.clone(),
                    attempt_id: context.attempt_id.clone(),
                    session_id: "swe_attempt-test".into(),
                    checkpoint_id: "test::checkpoint".into(),
                    prepared,
                }),
                ..Default::default()
            }),
        });
        (temp, shared, context)
    }

    fn cleanup_context(context: &StepExecutorContext) -> WorkflowCleanupContext {
        WorkflowCleanupContext {
            workflow_id: context.workflow_id.clone(),
            workflow_sha256: context.workflow_sha256.clone(),
            run_id: context.run_id.clone(),
            attempt_id: context.attempt_id.clone(),
            output_dir: context.output_dir.clone(),
        }
    }

    #[tokio::test]
    async fn completion_sample_cannot_escape_generation_or_token_limit() {
        for (turns, input, output) in [(65, 1, 1), (1, 250_000, 1)] {
            let api = Arc::new(FakeHarness::new(turns, input, output));
            let (_temp, shared, context) = fixture(api).await;
            // wait() completes immediately while every independent watchdog sample is under budget.
            shared.subject(&context).await.unwrap();
            let report = shared.capture_report().await.unwrap();
            assert_eq!(report["terminal_status"], "resource_limit");
            assert_eq!(report["metrics"]["totals"]["turns"], turns);
            shared.cleanup(&cleanup_context(&context)).await.unwrap();
        }
    }

    #[tokio::test]
    async fn lost_send_response_still_stops_and_tears_down_the_unique_session() {
        let mut api = FakeHarness::new(1, 1, 1);
        api.lose_send = true;
        let api = Arc::new(api);
        let (_temp, shared, context) = fixture(api.clone()).await;
        let executor = Executor {
            shared: shared.clone(),
            step: workflow::SUBJECT,
        };
        assert!(executor.execute(context.clone()).await.is_err());
        shared.cleanup(&cleanup_context(&context)).await.unwrap();
        let calls = api.calls.lock().unwrap();
        assert!(calls.contains(&"stop:swe_attempt-test".into()));
        assert!(calls.contains(&"stop:child-29".into()));
        assert!(calls.contains(&"teardown:swe_attempt-test".into()));
    }

    #[tokio::test]
    async fn repeated_stop_requests_share_one_deadline_and_reserve_cleanup_time() {
        let mut api = FakeHarness::new(1, 1, 1);
        api.slow_stops = true;
        let api = Arc::new(api);
        let (_temp, shared, context) = fixture(api.clone()).await;
        // The first 59.9 seconds of the shutdown budget have already elapsed.
        let deadline = tokio::time::Instant::now() + Duration::from_millis(240_100);
        shared.state.lock().unwrap().shutdown_deadline = Some(deadline);
        shared.state.lock().unwrap().send_attempted = true;
        tokio::time::timeout(Duration::from_secs(1), async {
            shared.stop_tree("swe_attempt-test").await;
            shared.stop_tree("swe_attempt-test").await;
        })
        .await
        .expect("thirty stalled descendants must not get independent shutdown budgets");
        assert_eq!(shared.shutdown_deadline(), deadline);
        shared.cleanup(&cleanup_context(&context)).await.unwrap();
        assert!(!shared.attempt().unwrap().workspace.exists());
        assert_eq!(shared.shutdown_deadline(), deadline);
    }

    #[tokio::test]
    async fn capture_quiesces_background_writes_and_cleanup_publishes_refreshed_evidence() {
        let (_temp, shared, context) = fixture(Arc::new(FakeHarness::new(1, 1, 1))).await;
        let attempt = shared.attempt().unwrap();
        let mut process = tokio::process::Command::new("python3").args(["-I", "-c", "import signal,time,pathlib,sys; signal.signal(signal.SIGTERM,lambda *_:(pathlib.Path('late-write.txt').write_text('shutdown evidence'),sys.exit(0))); pathlib.Path('ready').touch(); time.sleep(60)"]).current_dir(&attempt.workspace).kill_on_drop(true).spawn().unwrap();
        for _ in 0..100 {
            if attempt.workspace.join("ready").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(attempt.workspace.join("ready").exists());
        let report = shared.capture_report().await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), process.wait())
            .await
            .expect("capture must stop owned workspace processes")
            .unwrap();
        assert!(report["unaccepted_patch"]
            .as_str()
            .unwrap()
            .contains("shutdown evidence"));
        // Simulate an external writer after an earlier capture; cleanup's final capture must win.
        std::fs::write(
            attempt.workspace.join("after-capture.txt"),
            "refreshed final evidence",
        )
        .unwrap();
        shared.cleanup(&cleanup_context(&context)).await.unwrap();
        let report: Value = serde_json::from_slice(
            &std::fs::read(final_report_path(&context.output_dir, &context.attempt_id)).unwrap(),
        )
        .unwrap();
        assert!(report["unaccepted_patch"]
            .as_str()
            .unwrap()
            .contains("refreshed final evidence"));
        assert!(!attempt.private_root.exists());
    }

    #[tokio::test]
    async fn real_scheduler_deadline_and_user_cancel_have_distinct_final_outcomes() {
        for cancelled in [false, true] {
            let mut api = FakeHarness::new(1, 1, 1);
            api.wait_for_cancel = true;
            let (_temp, shared, context) = fixture(Arc::new(api)).await;
            let mut definition =
                workflow::definition(crate::scenarios::ScenarioId::SweConfigIsolation);
            definition.nodes.remove(0);
            definition.nodes[0].depends_on.clear();
            definition.limits.workflow_timeout_seconds = if cancelled { 10 } else { 1 };
            definition.limits.step_timeout_seconds = definition.limits.workflow_timeout_seconds;
            let mut catalog = StepCatalog::default();
            for descriptor in workflow::descriptors()
                .into_iter()
                .filter(|d| d.id != workflow::PREPARE)
            {
                let step = if descriptor.id == workflow::SUBJECT {
                    workflow::SUBJECT
                } else {
                    workflow::CAPTURE
                };
                catalog
                    .register(
                        descriptor,
                        Arc::new(Executor {
                            shared: shared.clone(),
                            step,
                        }),
                    )
                    .unwrap();
            }
            let (sender, cancellation) = tokio::sync::watch::channel(false);
            let cancel = tokio::spawn(async move {
                if cancelled {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    sender.send(true).unwrap();
                } else {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            });
            let workflow_report = crate::workflow::execute_workflow(
                &definition,
                Arc::new(catalog),
                crate::workflow::WorkflowExecutionRequest {
                    output_dir: context.output_dir.clone(),
                    run_id: context.run_id.clone(),
                    attempt_id: Some(context.attempt_id.clone()),
                    attempt_number: 1,
                    cancellation,
                    cleanup_hook: shared.clone(),
                },
            )
            .await
            .unwrap();
            cancel.abort();
            assert!(workflow_report.technical_failure);
            assert_eq!(
                workflow_report.cleanup.status,
                crate::workflow::WorkflowCleanupStatus::Succeeded
            );
            let report = shared.state.lock().unwrap().final_report.clone().unwrap();
            assert_eq!(
                report["terminal_status"],
                if cancelled {
                    "cancelled"
                } else {
                    "resource_limit"
                }
            );
            let case = super::super::materialize(crate::scenarios::ScenarioId::SweConfigIsolation)
                .unwrap()
                .case;
            let mut top = crate::report::E2eRunReport::new(
                context.run_id.clone(),
                context.attempt_id.clone(),
                1,
                "".into(),
                "".into(),
            );
            let status = super::super::execution_outcome(&context.output_dir, &context.attempt_id);
            crate::suite::populate_composite_report_with_terminal(
                &mut top,
                workflow_report,
                status,
            );
            super::super::attach_report(&context.output_dir, &context.attempt_id, &case, &mut top)
                .unwrap();
            assert_eq!(
                top.status,
                if cancelled {
                    crate::report::RunStatus::SubjectError
                } else {
                    crate::report::RunStatus::ResourceLimit
                }
            );
            assert!(top
                .failures
                .iter()
                .all(|failure| failure.domain != crate::report::FailureDomain::E2eInfrastructure));
        }
    }

    #[test]
    fn aggregate_budget_accepts_exact_limit_and_rejects_extra_descendant_work() {
        let case = Case {
            ticket: 1,
            id: "swe_config_isolation",
        };
        assert_eq!(aggregate_limit(&metrics(64, 200_000, 50_000), case), None);
        assert_eq!(
            aggregate_limit(&metrics(65, 100, 100), case),
            Some("generations")
        );
        assert_eq!(
            aggregate_limit(&metrics(3, 200_001, 50_000), case),
            Some("tokens")
        );
        assert_eq!(
            aggregate_limit(&metrics(3, u64::MAX, 5), case),
            Some("tokens")
        );
        let journey = Case {
            ticket: 0,
            id: "swe_service_journey",
        };
        assert_eq!(
            aggregate_limit(&metrics(320, 1_400_000, 100_000), journey),
            None
        );
        assert_eq!(
            aggregate_limit(&metrics(321, 1, 1), journey),
            Some("generations")
        );
    }

    #[test]
    fn checkpoint_accepts_engine_metadata_and_a_revision_ack_but_no_paths() {
        let request: CheckpointRequest = serde_json::from_value(json!({
            "ticket":5,"head":"a".repeat(40),"revision_id":"revision-1","_caller_worker_id":"worker",
        })).unwrap();
        assert_eq!(request.revision_id.as_deref(), Some("revision-1"));
        assert!(serde_json::from_value::<CheckpointRequest>(json!({
            "ticket":1,"head":"a".repeat(40),"state_file":"/foreign/state.json",
        }))
        .is_err());
    }
}
