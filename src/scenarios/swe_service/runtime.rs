use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use iii_sdk::{runtime::FunctionRef, RegisterFunction};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
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
    exec_id: String,
    github_id: String,
    prepared: Value,
}

#[derive(Default)]
struct SharedState {
    attempt: Option<Attempt>,
    registrations: Vec<FunctionRef>,
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

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkspaceExecRequest {
    #[serde(rename = "_caller_worker_id", default)]
    #[schemars(skip)]
    _caller_worker_id: Option<String>,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default = "default_exec_timeout_ms")]
    timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum GithubOperation {
    Issue,
    Push,
    Pr,
    Ci,
    Review,
    Merge,
    Release,
    Inspect,
    CloseIssue,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GithubRequest {
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    _caller_worker_id: Option<String>,
    operation: GithubOperation,
    #[serde(default)]
    head: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

fn default_exec_timeout_ms() -> u64 {
    30_000
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
            (!case.lifecycle()).then_some(Duration::from_secs(600)),
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
        if self.case.lifecycle() {
            let info = self
                .harness
                .trigger_value(
                    "engine::functions::info",
                    json!({"function_ids":["harness::status"]}),
                )
                .await?;
            let schema = info
                .get("functions")
                .and_then(Value::as_array)
                .and_then(|functions| {
                    functions
                        .iter()
                        .find(|function| function["function_id"] == "harness::status")
                })
                .and_then(|function| function.get("response_schema"))
                .context("Lifecycle requires the registered harness::status contract")?;
            if crate::wire::schema_at_path(schema, "stop_reason").is_none() {
                bail!("software_company_lifecycle requires harness::status.stop_reason to continue native max_turns boundaries without a run limit; update the Harness runtime");
            }
        }
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
            exec_id: format!("e2etest::swe_exec_{}", execution.attempt_id),
            github_id: format!("e2etest::swe_github_{}", execution.attempt_id),
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
        let exec_registration = self.harness.client().register_function(
            attempt.exec_id.clone(),
            RegisterFunction::new_async(move |request: WorkspaceExecRequest| {
                let shared = callback.clone();
                async move {
                    shared
                        .workspace_exec(request)
                        .await
                        .map_err(|error| iii_sdk::errors::Error::from(format!("{error:#}")))
                }
            })
            .description(
                "Run one command inside the isolated SWE workspace. Pass the program in command, \
                 each argument in args, and an optional timeout_ms up to 120000. The working \
                 directory is the repository root; host paths and network are unavailable.",
            ),
        );
        let callback = self.clone();
        let checkpoint_registration = self.harness.client().register_function(
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
        let mut registrations = vec![exec_registration, checkpoint_registration];
        if self.case.lifecycle() {
            let callback = self.clone();
            let github_registration = self.harness.client().register_function(
                attempt.github_id.clone(),
                RegisterFunction::new_async(move |request: GithubRequest| {
                    let shared = callback.clone();
                    async move {
                        shared
                            .github(request)
                            .await
                            .map_err(|error| iii_sdk::errors::Error::from(format!("{error:#}")))
                    }
                })
                .description(
                    "Perform one trusted lifecycle GitHub operation. Use issue, push, pr, ci, review, \
                     merge, release, inspect, or close_issue; provide head, title, body, and version only \
                     when relevant. The operation is recorded against this lifecycle attempt and returns an operation_id.",
                ),
            );
            registrations.push(github_registration);
        }
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .registrations = registrations;
        Ok(completed(true))
    }

    async fn verify_boundary(&self, attempt: &Attempt) -> Result<()> {
        let canary = attempt.private_root.join("isolation-canary");
        let secret = uuid::Uuid::new_v4().to_string();
        std::fs::write(&canary, &secret)?;
        // This is a trusted preflight, never placed in the subject transcript.
        let script = "import json,pathlib,sys; p=pathlib.Path(sys.argv[1]); ok=p.is_file(); q=pathlib.Path(sys.argv[2]);\ntry:\n q.read_bytes(); isolated=False\nexcept (PermissionError,FileNotFoundError):\n isolated=True\nprint(json.dumps({'workspace_readable':ok,'trusted_unreadable':isolated}))";
        let output = self
            .isolated_exec(
                attempt,
                WorkspaceExecRequest {
                    _caller_worker_id: None,
                    command: "python3".into(),
                    args: vec![
                        "-I".into(),
                        "-c".into(),
                        script.into(),
                        attempt
                            .workspace
                            .join("README.md")
                            .to_string_lossy()
                            .into_owned(),
                        canary.to_string_lossy().into_owned(),
                    ],
                    timeout_ms: 15_000,
                },
            )
            .await
            .context("SWE requires bubblewrap workspace isolation")?;
        if output.get("exit_code").and_then(Value::as_i64) != Some(0) {
            bail!("SWE isolated shell cannot execute the workspace boundary preflight");
        }
        let value: Value =
            serde_json::from_str(output.get("stdout").and_then(Value::as_str).unwrap_or(""))
                .context("SWE shell returned invalid isolation evidence")?;
        if value.get("workspace_readable") != Some(&Value::Bool(true))
            || value.get("trusted_unreadable") != Some(&Value::Bool(true))
        {
            // The probe is trusted output, never the subject's, so it is safe
            // to report. Without it the two invariants are indistinguishable
            // in the failure, and telling them apart needs a whole
            // investigation: one means the fixture never reached the
            // workspace, the other that nothing confines the shell's child.
            bail!(
                "SWE shell must see the exported workspace and must not read controller state or \
                 future snapshots (probe: {value})"
            );
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

    async fn workspace_exec(&self, request: WorkspaceExecRequest) -> Result<Value> {
        let attempt = self.attempt()?;
        self.isolated_exec(&attempt, request).await
    }

    async fn isolated_exec(
        &self,
        attempt: &Attempt,
        request: WorkspaceExecRequest,
    ) -> Result<Value> {
        if request.command.is_empty() || request.timeout_ms == 0 || request.timeout_ms > 120_000 {
            bail!("command must be non-empty and timeout_ms must be between 1 and 120000");
        }
        let args = isolated_argv(&attempt.workspace, request.command, request.args);
        self.harness
            .trigger_value(
                "shell::exec",
                json!({
                    "command":"/usr/bin/bwrap",
                    "args":args,
                    "cwd":attempt.workspace,
                    "timeout_ms":request.timeout_ms,
                    "fs_scope":{"root":attempt.workspace,"grants":[],"boundary":"workspace"},
                }),
            )
            .await
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
        if self.case.lifecycle() {
            assets::github_controller(&attempt.private_root, &args).await
        } else {
            assets::controller(&attempt.private_root, &args).await
        }
    }

    async fn github(&self, request: GithubRequest) -> Result<Value> {
        if !self.case.lifecycle() {
            bail!("GitHub lifecycle operations are unavailable for isolated SWE tickets");
        }
        let attempt = self.attempt()?;
        let request = serde_json::to_string(&request)?;
        let response = assets::github_controller(
            &attempt.private_root,
            &[
                "github".into(),
                "--state-file".into(),
                attempt.state_file.to_string_lossy().into_owned(),
                "--request".into(),
                request,
            ],
        )
        .await?;
        if response
            .get("operation_id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            bail!("trusted GitHub callback returned no operation_id");
        }
        Ok(response)
    }

    async fn subject(&self, execution: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let attempt = self.attempt()?;
        let github = if self.case.lifecycle() {
            format!(
            "For every observable company lifecycle operation, use {}: issue, push, pr, ci, review, merge, release, inspect, and close_issue. It is the only GitHub surface, has no token exposure, and returns an operation_id that you must retain in your delivery evidence.\n\n",
            attempt.github_id
        )
        } else {
            String::new()
        };
        let prompt = format!(
            "Work as the software engineer responsible for the service in {}. Read its public contracts, investigate the request, implement it, add your own regression tests under tests/agent, and maintain useful documentation. You may delegate when useful; you remain responsible for integration. Only this workspace is authorized. Preserve tests/reference and benchmark controls. Use {} for every command and file read or write; it always starts at the repository root.\n\n{}{}\n\nDeliver a clean committed change by calling {} with the current ticket number and full HEAD SHA. Preserve accepted commits. If requirements are revealed, acknowledge their revision_id on the next submission; a compatible implementation may reuse the same SHA. Continue in this same session when a next ticket is returned. On rejected, address the evidence; on completed or capability_failure, stop and summarize the last accepted work. Do not invent future tickets.",
            attempt.workspace.display(), attempt.exec_id, github, attempt.prepared.get("prompt").and_then(Value::as_str).context("missing first ticket")?, attempt.checkpoint_id,
        );
        // Harness may accept the unique ID even when its response is lost or malformed.
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .send_attempted = true;
        let mut request = SendRequest {
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
                max_turns: self.case.generations(),
                max_cost_usd: None,
                max_output_tokens: (!self.case.lifecycle()).then_some(32_768),
                max_total_tokens: self.case.tokens(),
                max_validation_retries: None,
                functions: Some(FunctionPolicy {
                    allow: [
                        "engine::functions::list",
                        "engine::functions::info",
                        "engine::triggers::list",
                        "engine::triggers::info",
                        "harness::spawn",
                        "harness::status",
                        "harness::session-tree",
                        "harness::trigger::*",
                        "state::*",
                    ]
                    .into_iter()
                    .map(str::to_string)
                    .chain([attempt.exec_id.clone(), attempt.checkpoint_id.clone()])
                    .chain(self.case.lifecycle().then(|| attempt.github_id.clone()))
                    .collect(),
                    deny: [
                        "e2e::*",
                        "coder::*",
                        "shell::*",
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
        };
        let result = loop {
            let response = {
                let _lock = self.stopping.lock().await;
                if *execution.cancellation.borrow()
                    || self
                        .state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .stop_reason
                        .is_some()
                {
                    break Err(anyhow::anyhow!("Lifecycle cancelled before continuation"));
                }
                self.state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .tree_stopped = false;
                self.harness.send(request.clone()).await?
            };
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
            if result.is_ok() && self.case.lifecycle() {
                let status = self
                    .harness
                    .trigger_value(
                        "harness::status",
                        json!({
                            "session_id":attempt.session_id,"verbose":true,
                        }),
                    )
                    .await?;
                if status.get("stop_reason").and_then(Value::as_str) == Some("max_turns") {
                    if status.get("session_id").and_then(Value::as_str) != Some(&attempt.session_id)
                        || status.get("turn_id").and_then(Value::as_str) != Some(&response.turn_id)
                    {
                        bail!("Lifecycle continuation received status for a different session or turn");
                    }
                    let checkpoint: Value =
                        serde_json::from_slice(&std::fs::read(&attempt.state_file)?)?;
                    if checkpoint
                        .get("terminal_status")
                        .is_some_and(Value::is_null)
                    {
                        request.message = MessageInput::Text(format!(
                        "Continue the software company lifecycle from checkpoint {} using {} and {}. The platform's native turn ended; the benchmark has no generation budget. Preserve all accepted work and finish the remaining stages.",
                        checkpoint["current_ticket"], attempt.exec_id, attempt.checkpoint_id,
                    ));
                        request.session = None;
                        request.idempotency_key = Some(format!(
                            "swe:{}:{}:continue:{}",
                            execution.run_id, execution.attempt_id, response.turn_id
                        ));
                        continue;
                    }
                }
            }
            break result;
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
                let evaluations = evaluations(self.shared.case, &report)?;
                Ok(StepExecutorOutput {
                    outputs: evaluations
                        .iter()
                        .map(|evaluation| {
                            Ok((
                                evaluation.id.clone(),
                                TypedPortValue {
                                    kind: PortValueKind::Assessment,
                                    value: serde_json::to_value(evaluation)?,
                                },
                            ))
                        })
                        .collect::<Result<_>>()?,
                    captured_assets: vec![CapturedWorkflowAsset {
                        id: REPORT_ID.into(),
                        kind: "swe-service-report".into(),
                        media_type: "application/json".into(),
                        content: WorkflowAssetContent::Json(report.clone()),
                        provenance: Vec::new(),
                    }],
                    evaluation: StepEvaluation {
                        hard_gates: vec![WorkflowGateResult {
                            id: "delivery_complete".into(),
                            passed,
                            reason: format!(
                                "Lifecycle or ticket terminal state: {}",
                                report["terminal_status"]
                            ),
                            evidence_ids: vec![REPORT_ID.into()],
                        }],
                        evaluations,
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
            for registration in std::mem::take(
                &mut self
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .registrations,
            ) {
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

fn isolated_argv(workspace: &Path, command: String, args: Vec<String>) -> Vec<String> {
    let mut isolated = vec![
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--cap-drop",
        "ALL",
        "--clearenv",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    for root in ["/usr", "/lib", "/lib64", "/bin"] {
        if Path::new(root).exists() {
            isolated.extend(["--ro-bind".into(), root.into(), root.into()]);
        }
    }
    isolated.extend([
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
        "--tmpfs".into(),
        "/tmp".into(),
        "--bind".into(),
        workspace.to_string_lossy().into_owned(),
        workspace.to_string_lossy().into_owned(),
        "--chdir".into(),
        workspace.to_string_lossy().into_owned(),
        "--setenv".into(),
        "PATH".into(),
        "/usr/local/bin:/usr/bin:/bin".into(),
        "--setenv".into(),
        "HOME".into(),
        "/tmp".into(),
        "--setenv".into(),
        "TMPDIR".into(),
        "/tmp".into(),
        "--setenv".into(),
        "LANG".into(),
        "C.UTF-8".into(),
        "--".into(),
        command,
    ]);
    isolated.extend(args);
    isolated
}

fn aggregate_limit(metrics: &SessionMetricsResponse, case: Case) -> Option<&'static str> {
    if case
        .generations()
        .is_some_and(|limit| metrics.totals.turns > u64::from(limit))
    {
        return Some("generations");
    }
    if metrics
        .totals
        .input_tokens
        .zip(metrics.totals.output_tokens)
        .zip(case.tokens())
        .is_some_and(|((input, output), limit)| input.saturating_add(output) > limit)
    {
        return Some("tokens");
    }
    None
}

fn evaluations(case: Case, report: &Value) -> Result<Vec<WorkflowEvaluationResult>> {
    let completed = report["terminal_status"] == "completed";
    if !case.lifecycle() {
        let accepted = report["accepted_tickets"].as_array().map_or(0, Vec::len);
        return Ok(vec![WorkflowEvaluationResult {
            id: "swe_delivery".into(),
            outcome: if completed {
                WorkflowEvaluationOutcome::Passed
            } else {
                WorkflowEvaluationOutcome::Failed
            },
            summary: format!(
                "{accepted}/1 committed SWE ticket accepted; terminal={}",
                report["terminal_status"]
            ),
            score: Some(accepted.min(1) as f64),
            evidence_ids: vec![REPORT_ID.into()],
        }]);
    }
    let stages = report["lifecycle"]["stages"]
        .as_array()
        .context("missing lifecycle stage evidence")?;
    super::LIFECYCLE_CRITERIA
        .iter()
        .map(|(id, _, description)| {
            let score = match *id {
                "convergence" => {
                    let accepted = report["accepted_tickets"]
                        .as_array()
                        .context("missing accepted lifecycle checkpoints")?
                        .len() as f64;
                    let rejected = report["lifecycle"]["rejected_checkpoints"]
                        .as_u64()
                        .context("missing rejected checkpoints")?
                        as f64;
                    Some(if completed {
                        accepted / (accepted + rejected).max(1.0)
                    } else {
                        0.0
                    })
                }
                "resource_efficiency" if !completed => Some(0.0),
                "resource_efficiency" => {
                    let measured = report["elapsed_ms"]
                        .as_u64()
                        .zip(
                            report
                                .pointer("/metrics/totals/turns")
                                .and_then(Value::as_u64),
                        )
                        .zip(
                            report
                                .pointer("/metrics/totals/input_tokens")
                                .and_then(Value::as_u64),
                        )
                        .zip(
                            report
                                .pointer("/metrics/totals/output_tokens")
                                .and_then(Value::as_u64),
                        );
                    measured
                        .filter(|_| report.pointer("/metrics/complete") == Some(&Value::Bool(true)))
                        .map(|(((elapsed, turns), input), output)| {
                            [
                                (5_400_000.0, elapsed),
                                (320.0, turns),
                                (1_500_000.0, input.saturating_add(output)),
                            ]
                            .iter()
                            .map(|(reference, value)| (reference / (*value).max(1) as f64).min(1.0))
                            .sum::<f64>()
                                / 3.0
                        })
                }
                _ => Some(
                    stages
                        .iter()
                        .find(|stage| stage["id"] == *id)
                        .and_then(|stage| stage["score"].as_f64())
                        .filter(|score| (0.0..=1.0).contains(score))
                        .with_context(|| format!("missing or invalid lifecycle {id} assessment"))?,
                ),
            };
            Ok(WorkflowEvaluationResult {
                id: (*id).into(),
                outcome: match score {
                    None => WorkflowEvaluationOutcome::NotEvaluated,
                    Some(1.0) => WorkflowEvaluationOutcome::Passed,
                    Some(score) if score > 0.0 => WorkflowEvaluationOutcome::Advisory,
                    _ => WorkflowEvaluationOutcome::Failed,
                },
                summary: (*description).into(),
                score,
                evidence_ids: vec![REPORT_ID.into()],
            })
        })
        .collect()
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
        sent: Mutex<Vec<Value>>,
        slow_stops: bool,
        native_turns: usize,
        send_gate: Option<Arc<tokio::sync::Semaphore>>,
    }

    impl FakeHarness {
        fn new(turns: u64, input: u64, output: u64) -> Self {
            Self {
                lose_send: false,
                wait_for_cancel: false,
                final_metrics: metrics(turns, input, output),
                calls: Mutex::new(Vec::new()),
                sent: Mutex::new(Vec::new()),
                slow_stops: false,
                native_turns: 0,
                send_gate: None,
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
        async fn trigger_value(&self, function: &str, payload: Value) -> Result<Value> {
            assert_eq!(function, "harness::status");
            let sends = self.sent.lock().unwrap().len();
            Ok(
                json!({"session_id":payload["session_id"],"turn_id":format!("turn-{sends}"),
                "status":"completed","stop_reason":(sends <= self.native_turns).then_some("max_turns")}),
            )
        }
        async fn send(&self, request: SendRequest) -> Result<SendResponse> {
            self.sent
                .lock()
                .unwrap()
                .push(serde_json::to_value(&request).unwrap());
            let id = request.session_id.unwrap();
            self.record(format!("send:{id}"));
            if let Some(gate) = &self.send_gate {
                gate.acquire().await.unwrap().forget();
            }
            if self.lose_send {
                bail!("response lost after session accepted");
            }
            Ok(SendResponse::from_normalized(
                crate::wire::SendResponsePayload {
                    session_id: id,
                    turn_id: format!("turn-{}", self.sent.lock().unwrap().len()),
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
                    exec_id: "test::exec".into(),
                    github_id: "test::github".into(),
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
    async fn lifecycle_continues_native_turns_without_aggregate_limits_in_the_same_session() {
        let mut api = FakeHarness::new(50_000, 100_000_000, 10_000_000);
        api.native_turns = 2;
        let api = Arc::new(api);
        let (_temp, mut shared, context) = fixture(api.clone()).await;
        Arc::get_mut(&mut shared).unwrap().case = Case {
            ticket: 0,
            id: "software_company_lifecycle",
        };
        shared.subject(&context).await.unwrap();
        {
            let sent = api.sent.lock().unwrap();
            assert_eq!(sent.len(), 3);
            for request in sent.iter() {
                assert_eq!(request["session_id"], "swe_attempt-test");
                for key in [
                    "max_turns",
                    "max_total_tokens",
                    "max_output_tokens",
                    "max_cost_usd",
                ] {
                    assert!(request["options"].get(key).is_none());
                }
            }
            assert!(sent[1]["idempotency_key"]
                .as_str()
                .unwrap()
                .ends_with(":continue:turn-1"));
            assert!(sent[2]["idempotency_key"]
                .as_str()
                .unwrap()
                .ends_with(":continue:turn-2"));
            assert!(shared.state.lock().unwrap().stop_reason.is_none());
        }
        shared.cleanup(&cleanup_context(&context)).await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_waits_for_in_flight_send_then_stops_the_created_session() {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let mut api = FakeHarness::new(1, 1, 1);
        api.send_gate = Some(gate.clone());
        api.wait_for_cancel = true;
        let api = Arc::new(api);
        let (_temp, shared, mut context) = fixture(api.clone()).await;
        let (cancel, receiver) = tokio::sync::watch::channel(false);
        context.cancellation = receiver;
        let running = {
            let shared = shared.clone();
            let context = context.clone();
            tokio::spawn(async move { shared.subject(&context).await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            while api.sent.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        cancel.send(true).unwrap();
        context
            .termination
            .set(WorkflowTerminationReason::Cancelled);
        let stopping = {
            let shared = shared.clone();
            let context = context.clone();
            tokio::spawn(async move {
                Executor {
                    shared,
                    step: workflow::SUBJECT,
                }
                .cancel(&context)
                .await
            })
        };
        tokio::task::yield_now().await;
        assert!(!api
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("stop:")));
        gate.add_permits(1);
        running.await.unwrap().unwrap();
        stopping.await.unwrap().unwrap();
        assert!(api
            .calls
            .lock()
            .unwrap()
            .contains(&"stop:swe_attempt-test".to_owned()));
        shared.cleanup(&cleanup_context(&context)).await.unwrap();
        assert!(!shared.attempt().unwrap().workspace.exists());
    }

    #[test]
    fn lifecycle_efficiency_is_scored_after_completion_and_missing_metrics_are_not_zero() {
        let case = Case {
            ticket: 0,
            id: "software_company_lifecycle",
        };
        let mut report = json!({"terminal_status":"completed","accepted_tickets":[1,2,3,4,5,6,7,8],
            "elapsed_ms":10_800_000,"metrics":{"complete":true,"totals":{"turns":640,"input_tokens":2_000_000,"output_tokens":1_000_000}},
            "lifecycle":{"rejected_checkpoints":8,"stages":super::super::LIFECYCLE_CRITERIA[..8].iter().map(|(id,_,_)|json!({"id":id,"score":1.0})).collect::<Vec<_>>()}});
        let result = evaluations(case, &report).unwrap();
        for id in ["convergence", "resource_efficiency"] {
            let criterion = result.iter().find(|item| item.id == id).unwrap();
            assert_eq!(criterion.score, Some(0.5));
            assert_eq!(criterion.outcome, WorkflowEvaluationOutcome::Advisory);
        }
        report["metrics"]["totals"]["output_tokens"] = Value::Null;
        let result = evaluations(case, &report).unwrap();
        let missing = result
            .iter()
            .find(|item| item.id == "resource_efficiency")
            .unwrap();
        assert_eq!(missing.score, None);
        assert_eq!(missing.outcome, WorkflowEvaluationOutcome::NotEvaluated);
        report["terminal_status"] = "capability_failure".into();
        assert_eq!(
            evaluations(case, &report)
                .unwrap()
                .iter()
                .find(|item| item.id == "resource_efficiency")
                .unwrap()
                .score,
            Some(0.0)
        );
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
    async fn subject_can_only_execute_commands_through_the_isolated_function() {
        let api = Arc::new(FakeHarness::new(1, 1, 1));
        let (_temp, shared, context) = fixture(api.clone()).await;
        shared.subject(&context).await.unwrap();
        let sent = api.sent.lock().unwrap();
        let policy = &sent[0]["options"]["functions"];
        assert!(policy["allow"]
            .as_array()
            .unwrap()
            .contains(&json!("test::exec")));
        assert!(policy["deny"]
            .as_array()
            .unwrap()
            .contains(&json!("shell::*")));
        assert!(policy["deny"]
            .as_array()
            .unwrap()
            .contains(&json!("coder::*")));
        assert!(!policy["allow"]
            .as_array()
            .unwrap()
            .iter()
            .any(|function| function.as_str() == Some("coder::*")));
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
            definition.limits.workflow_timeout_seconds = Some(if cancelled { 10 } else { 1 });
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
            let scenario = crate::scenarios::ScenarioId::SweConfigIsolation;
            let case = scenario
                .materialize("swe-runtime-test", scenario.canonical_seed())
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
        let lifecycle = Case {
            ticket: 0,
            id: "software_company_lifecycle",
        };
        assert_eq!(
            aggregate_limit(&metrics(320, 1_400_000, 100_000), lifecycle),
            None
        );
        assert_eq!(aggregate_limit(&metrics(321, 1, 1), lifecycle), None);
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

    #[test]
    fn github_callback_accepts_only_the_lifecycle_operation_contract() {
        let request: GithubRequest = serde_json::from_value(json!({
            "operation":"close_issue", "title":"close", "_caller_worker_id":"worker",
        }))
        .unwrap();
        assert!(matches!(request.operation, GithubOperation::CloseIssue));
        assert!(serde_json::to_value(&request)
            .unwrap()
            .get("_caller_worker_id")
            .is_none());
        assert!(serde_json::from_value::<GithubRequest>(json!({
            "operation":"delete_repository",
        }))
        .is_err());
        assert!(serde_json::from_value::<GithubRequest>(json!({
            "operation":"issue", "ticket":1,
        }))
        .is_err());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn workspace_exec_is_writable_but_cannot_see_private_files_or_host_processes() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let private = temp.path().join("private");
        std::fs::write(&private, "hidden").unwrap();
        let mut host_process = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let script = "import json,os,pathlib,subprocess,sys; pathlib.Path('change').write_text('ok'); subprocess.run(['git','init','-q'],check=True); subprocess.run(['git','config','user.name','SWE'],check=True); subprocess.run(['git','config','user.email','swe@example.invalid'],check=True); subprocess.run(['git','add','change'],check=True); subprocess.run(['git','commit','-qm','change'],check=True); print(json.dumps({'cwd':os.getcwd(),'private':pathlib.Path(sys.argv[1]).exists(),'host_process':pathlib.Path('/proc',sys.argv[2]).exists()}))";
        let args = isolated_argv(
            &workspace,
            "python3".into(),
            vec![
                "-I".into(),
                "-c".into(),
                script.into(),
                private.to_string_lossy().into_owned(),
                host_process.id().to_string(),
            ],
        );
        let output = std::process::Command::new("/usr/bin/bwrap")
            .args(args)
            .output()
            .expect("bubblewrap is required for SWE scenarios");
        let _ = host_process.kill();
        let _ = host_process.wait();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let evidence: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(evidence["cwd"], workspace.to_string_lossy().as_ref());
        assert_eq!(evidence["private"], false);
        assert_eq!(evidence["host_process"], false);
        assert_eq!(
            std::fs::read_to_string(workspace.join("change")).unwrap(),
            "ok"
        );
        assert!(workspace.join(".git").is_dir());
    }
}
