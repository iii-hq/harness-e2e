use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use iii_sdk::{runtime::FunctionRef, RegisterFunction};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{
    async_trait, ArtifactExpectation, Capability, CapturedDeliverable, CriterionAward,
    CriterionSpec, DeliverableContract, ExecutionPolicy, ObjectiveEvaluation, ProvenanceEvidence,
    Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};
use crate::context::E2eContext;
use crate::report::{CompletionState, EvaluationDimension};

const RUBRIC: &str = include_str!("../../../scripts/kanban_eval/rubric.json");
const RUBRIC_VERSION: u8 = 2;
const CATALOG: &str = include_str!("catalog.json");
const INSTRUCTIONS: &str = include_str!("../../../scripts/kanban_eval/instructions.md");
const REPORT: &str = "kanban_evaluation";
pub const IDS: [&str; 7] = [
    "kanban_c1_foundation",
    "kanban_c2_persistence",
    "kanban_c3_board",
    "kanban_c4_ticket_flow",
    "kanban_c5_edit_move",
    "kanban_c6_discussion",
    "kanban_c7_live",
];

#[derive(Deserialize)]
struct Catalog {
    shared_prompt: String,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    id: String,
    base_commit: String,
    reference_commit: String,
    prompt: String,
    criteria: Vec<String>,
}

fn catalog() -> &'static Catalog {
    static VALUE: OnceLock<Catalog> = OnceLock::new();
    VALUE.get_or_init(|| serde_json::from_str(CATALOG).expect("embedded Kanban catalog"))
}

fn case(index: usize) -> &'static Case {
    &catalog().cases[index]
}

struct Runtime {
    process: Child,
    function: Option<FunctionRef>,
    monitor: Option<tokio::task::JoinHandle<()>>,
}
fn registry() -> &'static Mutex<HashMap<String, Runtime>> {
    static VALUE: OnceLock<Mutex<HashMap<String, Runtime>>> = OnceLock::new();
    VALUE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn root(run_id: &str) -> PathBuf {
    std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("kanban-evaluation")
        .join(format!("{:x}", Sha256::digest(run_id.as_bytes())))
}

pub fn function_id(run_id: &str) -> String {
    format!("kanban_eval_{:x}::exec", Sha256::digest(run_id.as_bytes()))
}

/// One of the seven Kanban cases, indexed into [`IDS`] in catalog order.
pub struct Kanban(pub usize);

#[async_trait]
impl Scenario for Kanban {
    fn id(&self) -> &'static str {
        IDS[self.0]
    }

    fn canonical_seed_only(&self) -> bool {
        true
    }

    fn case(&self, _seed: u64) -> Result<ScenarioCase> {
        let item = case(self.0);
        ScenarioCase::new(
            IDS[self.0],
            super::stable_seed(IDS[self.0]),
            json!({"base_commit":item.base_commit,"reference_commit":item.reference_commit,
                "prompt":item.prompt,"criteria":item.criteria,"rubric_version":RUBRIC_VERSION,
                "rubric":rubric(self.0),"rubric_sha256":format!("{:x}", Sha256::digest(RUBRIC.as_bytes())),"catalog_sha256":format!("{:x}", Sha256::digest(CATALOG.as_bytes())),
                "runtime_instructions_sha256":format!("{:x}", Sha256::digest(INSTRUCTIONS.as_bytes())),
                "isolation":"docker-none-nonroot-readonly", "max_cost_usd":5, "subject_deadline_seconds":1800}),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiFunctions,
                Capability::Docker,
                Capability::Node,
                Capability::Playwright,
            ],
            DeliverableContract {
                artifacts: vec![ArtifactExpectation {
                    id: REPORT.into(),
                    kind: "application_audit".into(),
                    media_type: "application/json".into(),
                    schema: json!({"type":"object", "required":["result","provenance","diff"]}),
                    max_size_bytes: 16 * 1024 * 1024,
                }],
                provenance_required: true,
                capture_before_cleanup: true,
                ..Default::default()
            },
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let case = case(self.0);
        ScenarioSpec {
            id: case.id.as_str(),
            prompt: format!("{}\n\n{}\n\nAcceptance criteria:\n{}\n\n{}\n\nThe repository is /workspace inside an isolated container. Dependencies are installed; external networking is disabled. Use agent_trigger with {{\"function\":\"{}\",\"description\":\"Inspect repository\",\"payload\":{{\"command\":\"pwd\"}}}} to inspect, edit and test. This is a shell executor, not delegation. Commands have a 120-second and 256-KiB output limit. Reaching either limit returns nonzero feedback after candidate processes are stopped; use a narrower command and continue. Do not inspect the host working directory.",
                catalog().shared_prompt, case.prompt, rubric(self.0).iter().map(|c| format!("- {} ({} points)", c.description, c.weight)).collect::<Vec<_>>().join("\n"), INSTRUCTIONS.trim(), function_id(run_id)),
            filesystem_root: None,
            execution: ExecutionPolicy { max_turns: Some(100), max_output_tokens: Some(65_536),
                max_total_tokens: Some(1_000_000), stuck_timeout_seconds: 1_800, max_validation_retries: Some(0) },
            denied_functions: &["harness::spawn", "shell::*", "coder::*", "compose::*", "router::*", "harness::send", "harness::run"],
            criteria: criteria(self.0),
        }
    }

    fn required_functions(&self, run_id: &str) -> Vec<String> {
        vec![function_id(run_id)]
    }

    fn allowed_functions(&self, run_id: &str) -> Option<Vec<String>> {
        Some(vec![function_id(run_id)])
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> Result<()> {
        setup(context, run_id, self.0).await
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> Result<Vec<CapturedDeliverable>> {
        capture(context, observation, run_id).await
    }

    async fn evaluate(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> Result<ObjectiveEvaluation> {
        evaluate(context, observation, run_id).await
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> Result<()> {
        cleanup(context, run_id).await
    }
}

#[derive(Deserialize, Serialize)]
struct RubricCriterion {
    id: String,
    weight: u8,
    description: String,
    checks: Vec<String>,
    /// Marks the case's primary flow: the task counts as completed only when
    /// every gate passed. Everything else only moves the score.
    #[serde(default)]
    gate: bool,
}

fn rubric(index: usize) -> &'static [RubricCriterion] {
    static VALUE: OnceLock<HashMap<String, Vec<RubricCriterion>>> = OnceLock::new();
    &VALUE.get_or_init(|| serde_json::from_str(RUBRIC).expect("embedded Kanban rubric"))[IDS[index]]
}

fn criteria(index: usize) -> Vec<CriterionSpec> {
    rubric(index)
        .iter()
        .map(|criterion| {
            CriterionSpec::scored(
                criterion.id.as_str(),
                criterion.weight,
                criterion.description.as_str(),
                EvaluationDimension::Deliverable,
            )
            .with_gate(criterion.gate)
        })
        .collect()
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExecRequest {
    command: String,
    #[serde(default, rename = "_caller_worker_id")]
    #[schemars(skip)]
    _caller: Option<String>,
}

async fn setup(context: &E2eContext, run_id: &str, index: usize) -> Result<()> {
    let path = root(run_id);
    fs::create_dir_all(path.parent().context("Kanban parent")?)?;
    fs::create_dir(&path).context("Kanban attempt directory must be new")?;
    let scripts = path.join("controller");
    fs::create_dir(&scripts)?;
    for (name, source) in [
        ("rubric.json", RUBRIC),
        (
            "run.py",
            include_str!("../../../scripts/kanban_eval/run.py"),
        ),
        (
            "snapshot.py",
            include_str!("../../../scripts/kanban_eval/snapshot.py"),
        ),
        (
            "exec.py",
            include_str!("../../../scripts/kanban_eval/exec.py"),
        ),
        (
            "probe.mjs",
            include_str!("../../../scripts/kanban_eval/probe.mjs"),
        ),
        ("catalog.json", CATALOG),
        ("instructions.md", INSTRUCTIONS),
    ] {
        fs::write(scripts.join(name), source)?;
    }
    let config: Value = serde_json::from_slice(&fs::read(
        std::env::var("HARNESS_E2E_KANBAN_RUNTIME").context(
            "HARNESS_E2E_KANBAN_RUNTIME must name the administrator-provisioned runtime JSON",
        )?,
    )?)?;
    let mut command = Command::new("python3");
    command.arg(scripts.join("run.py"));
    for key in [
        "fixture",
        "image",
        "node",
        "iii",
        "pnpm",
        "dependencies",
        "browser-dependencies",
        "browsers",
        "playwright-module",
    ] {
        command.arg(format!("--{key}")).arg(
            config[key]
                .as_str()
                .with_context(|| format!("missing runtime {key}"))?,
        );
    }
    let log = fs::File::create(path.join("controller.log"))?;
    command
        .arg("--catalog")
        .arg(scripts.join("catalog.json"))
        .args([
            "--case",
            IDS[index],
            "--revision",
            "base",
            "--external-subject",
            "--output",
        ])
        .arg(path.join("run"))
        .stdin(Stdio::null())
        .stderr(log.try_clone()?)
        .stdout(log);
    let process = command
        .spawn()
        .context("start isolated Kanban controller")?;
    registry().lock().unwrap().insert(
        run_id.into(),
        Runtime {
            process,
            function: None,
            monitor: None,
        },
    );
    let deadline = Instant::now() + Duration::from_secs(180);
    let ready_path = path.join("run/ready.json");
    loop {
        if ready_path.is_file() {
            break;
        }
        if registry()
            .lock()
            .unwrap()
            .get_mut(run_id)
            .context("missing Kanban controller")?
            .process
            .try_wait()?
            .is_some()
        {
            bail!(
                "Kanban preparation failed; see {}",
                path.join("controller.log").display()
            );
        }
        if Instant::now() >= deadline {
            bail!("Kanban preparation timed out");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let ready: Value = serde_json::from_slice(&fs::read(ready_path)?)?;
    let candidate = ready["candidate"]
        .as_str()
        .context("missing candidate")?
        .to_owned();
    if candidate.len() != 64 || !candidate.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("invalid candidate identity");
    }
    let keeper: u32 = ready["keeper"]
        .as_str()
        .context("missing candidate keeper")?
        .parse()
        .context("invalid candidate keeper")?;
    if keeper <= 1 {
        bail!("invalid candidate keeper");
    }
    let command_script = scripts.join("exec.py");
    let cancel_path = path.join("run/cancel");
    let command_lock = Arc::new(tokio::sync::Mutex::new(()));
    let function = context.client().register_function(
        function_id(run_id),
        RegisterFunction::new_async(move |request: ExecRequest| {
            let candidate = candidate.clone();
            let command_script = command_script.clone();
            let cancel_path = cancel_path.clone();
            let command_lock = command_lock.clone();
            async move {
                let _guard = command_lock.lock().await;
                let result = tokio::task::spawn_blocking(move || -> Result<Value> {
                    if request.command.len() > 65_536 {
                        bail!("command exceeds 64 KiB");
                    }
                    let mut child = Command::new("python3")
                        .arg(command_script)
                        .arg(candidate)
                        .arg(keeper.to_string())
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()?;
                    child
                        .stdin
                        .take()
                        .context("command stdin")?
                        .write_all(request.command.as_bytes())?;
                    let result = child.wait_with_output()?;
                    if !result.status.success() {
                        fs::write(cancel_path, b"command boundary failed")?;
                        bail!(
                            "isolated command boundary failed: {}",
                            String::from_utf8_lossy(&result.stderr)
                        );
                    }
                    Ok(serde_json::from_slice(&result.stdout)?)
                })
                .await;
                match result {
                    Ok(Ok(value)) => Ok(value),
                    other => Err(iii_sdk::errors::Error::Runtime(format!("{other:?}"))),
                }
            }
        })
        .description("Execute one shell command in the isolated Kanban repository at /workspace."),
    );
    registry()
        .lock()
        .unwrap()
        .get_mut(run_id)
        .context("missing Kanban runtime")?
        .function = Some(function);
    let client = context.client().clone();
    let attempt = run_id.to_owned();
    let completion = path.join("run/subject-complete");
    let monitor = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if completion.exists() {
                break;
            }
            let exited = {
                let mut runtimes = registry().lock().unwrap();
                match runtimes.get_mut(&attempt) {
                    None => break,
                    Some(runtime) => !matches!(runtime.process.try_wait(), Ok(None)),
                }
            };
            if exited {
                let _ = client
                    .trigger(iii_sdk::protocol::TriggerRequest {
                        function_id: "harness::stop".into(),
                        payload: json!({"session_id":format!("e2e_{attempt}")}),
                        action: None,
                        timeout_ms: Some(10_000),
                    })
                    .await;
                break;
            }
        }
    });
    registry()
        .lock()
        .unwrap()
        .get_mut(run_id)
        .context("missing runtime")?
        .monitor = Some(monitor);
    Ok(())
}

async fn capture(
    _context: &E2eContext,
    _observation: &ScenarioObservation,
    run_id: &str,
) -> Result<Vec<CapturedDeliverable>> {
    let path = root(run_id);
    fs::write(
        path.join("run/subject-complete"),
        br#"{"model_invoked":true}"#,
    )?;
    let deadline = Instant::now() + Duration::from_secs(1200);
    let exit = loop {
        if let Some(exit) = registry()
            .lock()
            .unwrap()
            .get_mut(run_id)
            .context("missing controller")?
            .process
            .try_wait()?
        {
            break exit;
        }
        if Instant::now() >= deadline {
            bail!("Kanban private evaluation timed out");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    let content = diagnostics(run_id)?;
    let result = content.get("result").context("missing evaluation result")?;
    let expected_exit = match (
        result["status"].as_str(),
        result["functional_status"].as_str(),
    ) {
        (Some("infrastructure_failed" | "evaluation_failed"), _) => 2,
        (_, Some("passed")) => 0,
        _ => 1,
    };
    if exit.code() != Some(expected_exit) {
        bail!("controller exit disagrees with evidence: {exit}");
    }
    Ok(vec![CapturedDeliverable {
        id: REPORT.into(),
        kind: "application_audit".into(),
        content: content.into(),
        invariants: vec![],
        provenance: vec![ProvenanceEvidence {
            kind: "isolated_runtime".into(),
            source_id: format!("kanban-evaluation/{run_id}/evidence"),
            relation: "private_probes_before_cleanup".into(),
        }],
    }])
}

pub(crate) fn diagnostics(run_id: &str) -> Result<Value> {
    let path = root(run_id);
    let evidence = path.join("run/evidence");
    let mut content = serde_json::Map::new();
    for (name, key) in [
        ("result.json", "result"),
        ("provenance.json", "provenance"),
        ("coverage.json", "coverage"),
        ("functional-result.json", "functional_result"),
        ("source-integrity.json", "source_integrity"),
    ] {
        let file = evidence.join(name);
        if file.is_file() {
            content.insert(key.into(), serde_json::from_slice(&fs::read(file)?)?);
        }
    }
    for (name, key) in [
        ("subject.diff", "diff"),
        ("evaluated.diff", "evaluated_diff"),
    ] {
        let diff = evidence.join(name);
        if diff.is_file() && fs::metadata(&diff)?.len() > 8 * 1024 * 1024 {
            bail!("candidate diff exceeds evidence bound: {name}");
        }
        content.insert(
            key.into(),
            if diff.is_file() {
                fs::read_to_string(diff)?.into()
            } else {
                Value::Null
            },
        );
    }
    let mut attachments = serde_json::Map::new();
    let mut files = fs::read_dir(&evidence)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.push(path.join("controller.log"));
    for file in files {
        let name = file
            .file_name()
            .context("evidence filename")?
            .to_string_lossy()
            .into_owned();
        match file.extension().and_then(|extension| extension.to_str()) {
            Some("log") => {
                let bytes = fs::read(&file)?;
                let tail = &bytes[bytes.len().saturating_sub(256 * 1024)..];
                attachments.insert(
                    name,
                    json!({
                        "media_type":"text/plain", "content":String::from_utf8_lossy(tail),
                        "size_bytes":bytes.len(), "truncated":tail.len() != bytes.len()
                    }),
                );
            }
            Some("png") => {
                if fs::metadata(&file)?.len() > 4 * 1024 * 1024 {
                    bail!("screenshot exceeds evidence bound");
                }
                attachments.insert(name, json!({
                    "media_type":"image/png", "encoding":"base64", "content":STANDARD.encode(fs::read(file)?)
                }));
            }
            _ => {}
        }
    }
    content.insert("attachments".into(), attachments.into());
    if serde_json::to_vec(&content)?.len() > 16 * 1024 * 1024 {
        bail!("Kanban evidence exceeds artifact bound");
    }
    Ok(Value::Object(content))
}

async fn evaluate(
    _context: &E2eContext,
    observation: &ScenarioObservation,
    _run_id: &str,
) -> Result<ObjectiveEvaluation> {
    let value = observation
        .deliverables
        .iter()
        .find(|d| d.id == REPORT)
        .context("missing Kanban evidence")?
        .content
        .as_json()
        .context("invalid Kanban evidence")?;
    validate_evaluation(value)?;
    let result = &value["result"];
    let index = IDS
        .iter()
        .position(|id| *id == observation.case.scenario_id)
        .context("unknown Kanban case")?;
    validate_rubric(index, result)?;
    let awards = criterion_awards(index, result);
    Ok(ObjectiveEvaluation {
        completion: completion(index, result),
        awards,
        infrastructure_error: None,
    })
}

/// Completed means the subject delivered the case's primary flow: the
/// application builds and starts, and every gate criterion passed. Other
/// criteria only move the score. A gate the probe never reached is incomplete
/// when an earlier check failed, and undetermined when nothing failed at all.
fn completion(index: usize, result: &Value) -> CompletionState {
    let functional_failed = match result["functional_status"].as_str() {
        Some("failed") => true,
        Some("passed") => false,
        _ => return CompletionState::Undetermined,
    };
    if prerequisite_failed(result) {
        return CompletionState::TaskIncomplete;
    }
    let gates = rubric(index).iter().filter(|criterion| criterion.gate);
    let mut unreached = false;
    for gate in gates {
        let status = result["checks"]
            .as_array()
            .and_then(|checks| checks.iter().find(|check| check["id"] == gate.id))
            .and_then(|check| check["status"].as_str());
        match status {
            Some("passed") => {}
            Some("failed") => return CompletionState::TaskIncomplete,
            _ => unreached = true,
        }
    }
    if !unreached {
        CompletionState::Completed
    } else if functional_failed {
        CompletionState::TaskIncomplete
    } else {
        CompletionState::Undetermined
    }
}

fn prerequisite_failed(result: &Value) -> bool {
    result["status"] == "failed"
        && result["functional_status"] == "failed"
        && result["checks"].as_array().is_some_and(|checks| {
            checks.iter().any(|check| {
                check["status"] == "failed"
                    && matches!(
                        check["id"].as_str(),
                        Some(
                            "application_present"
                                | "typecheck"
                                | "test"
                                | "build"
                                | "application_startup"
                        )
                    )
            })
        })
}

fn criterion_awards(index: usize, result: &Value) -> Vec<CriterionAward> {
    criteria(index)
        .into_iter()
        .map(|criterion| {
            let check = result["checks"]
                .as_array()
                .and_then(|checks| checks.iter().find(|check| check["id"] == criterion.id));
            CriterionAward {
                id: criterion.id.into(),
                awarded: match check.and_then(|check| check["status"].as_str()) {
                    Some("passed") => Some(criterion.weight),
                    Some("failed") => Some(0),
                    _ => None,
                },
                reason: check
                    .and_then(|check| check["detail"].as_str())
                    .unwrap_or(if prerequisite_failed(result) {
                        "Not verified: application prerequisite failed; see build/startup evidence"
                    } else {
                        "Not verified: prerequisite evidence unavailable"
                    })
                    .into(),
            }
        })
        .collect()
}

fn validate_rubric(index: usize, result: &Value) -> Result<()> {
    if prerequisite_failed(result) {
        return Ok(());
    }
    if result["rubric_version"] != RUBRIC_VERSION || result["case_id"] != IDS[index] {
        bail!("Kanban evidence does not match the scenario rubric revision");
    }
    let checks = result["checks"]
        .as_array()
        .context("missing Kanban checks")?;
    let mut statuses = HashMap::new();
    for check in checks {
        let id = check["id"].as_str().context("missing check id")?;
        let status = check["status"].as_str().context("missing check status")?;
        if !matches!(status, "passed" | "failed" | "unverified")
            || statuses.insert(id, status).is_some()
        {
            bail!("Invalid or duplicated Kanban check: {id}");
        }
    }
    let rubric = rubric(index);
    if statuses
        .keys()
        .filter(|id| id.starts_with("criterion_"))
        .count()
        != rubric.len()
    {
        bail!("Kanban evidence is missing required criteria or contains unknown criteria");
    }
    for criterion in rubric {
        let expected = if criterion
            .checks
            .iter()
            .any(|id| statuses.get(id.as_str()) == Some(&"failed"))
        {
            "failed"
        } else if criterion
            .checks
            .iter()
            .all(|id| statuses.get(id.as_str()) == Some(&"passed"))
        {
            "passed"
        } else {
            "unverified"
        };
        if statuses.get(criterion.id.as_str()) != Some(&expected) {
            bail!(
                "Kanban criterion disagrees with its evidence: {}",
                criterion.id
            );
        }
    }
    Ok(())
}

fn validate_evaluation(value: &Value) -> Result<()> {
    let result = &value["result"];
    let prerequisite_failed = prerequisite_failed(result);
    let partial_observations = matches!(result["status"].as_str(), Some("failed" | "incomplete"))
        && matches!(
            result["functional_status"].as_str(),
            Some("failed" | "passed")
        )
        && result["checks"].as_array().is_some_and(|checks| {
            checks.iter().any(|check| {
                check["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("criterion_"))
                    && check["status"] == "unverified"
            })
        });
    if !prerequisite_failed
        && (result["functional_status"].is_null()
            || (value["coverage"]["complete"] != true && !partial_observations))
    {
        bail!(
            "Kanban evaluator is unavailable or incomplete: {}; {}",
            result["status"],
            result["error"]
                .as_str()
                .unwrap_or("required functional evidence is unavailable")
        );
    }
    Ok(())
}

async fn cleanup(_context: &E2eContext, run_id: &str) -> Result<()> {
    let runtime = registry().lock().unwrap().remove(run_id);
    let path = root(run_id).join("run");
    let mut failures = Vec::new();
    if path.is_dir() {
        if let Err(error) = fs::write(path.join("cancel"), b"cleanup") {
            failures.push(error.to_string());
        }
    }
    if let Some(mut runtime) = runtime {
        if let Some(monitor) = runtime.monitor {
            monitor.abort();
        }
        if let Some(function) = runtime.function {
            function.unregister();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while matches!(runtime.process.try_wait(), Ok(None)) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if !matches!(runtime.process.try_wait(), Ok(Some(_))) {
            if let Err(error) = runtime.process.kill() {
                failures.push(error.to_string());
            }
            if let Err(error) = runtime.process.wait() {
                failures.push(error.to_string());
            }
        }
    }
    let state = path.join("containers.json");
    if state.is_file() {
        let value: Value = serde_json::from_slice(&fs::read(state)?)?;
        for container in value["containers"]
            .as_array()
            .context("invalid container state")?
        {
            let id = container.as_str().context("invalid container id")?;
            if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("invalid cleanup identity");
            }
            let mut command = tokio::process::Command::new("docker");
            command
                .args(["rm", "-f", id])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            match tokio::time::timeout(Duration::from_secs(15), command.status()).await {
                Ok(Ok(status)) if status.success() => {}
                other => failures.push(format!("container cleanup {id}: {other:?}")),
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        bail!("Kanban cleanup: {}", failures.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rubric_changes_case_identity_and_exposes_independent_weighted_criteria() {
        for (index, id) in IDS.iter().enumerate() {
            let scenario: crate::scenarios::ScenarioId = id.parse().unwrap();
            let materialized = scenario
                .materialize("rubric-test", scenario.canonical_seed())
                .unwrap();
            materialized.validate().unwrap();
            assert!(materialized.spec.criteria.len() > case(index).criteria.len());
            assert_eq!(
                materialized
                    .spec
                    .criteria
                    .iter()
                    .map(|c| u16::from(c.weight))
                    .sum::<u16>(),
                100
            );
            let case = Kanban(index).case(0).unwrap();
            let serialized = serde_json::to_value(case).unwrap();
            assert!(serialized.to_string().contains("rubric_sha256"));
            assert!(materialized.spec.prompt.contains("points)"));
        }
    }

    #[test]
    fn partial_evaluation_preserves_pass_fail_and_unverified_awards() {
        validate_evaluation(&json!({
            "result": {"status":"failed","functional_status":"failed", "checks":[
                {"id":"criterion_creation","status":"passed"},
                {"id":"criterion_create_error","status":"failed"},
                {"id":"criterion_details","status":"unverified"}
            ]}, "coverage":{"complete":false}
        }))
        .unwrap();
        let awards = criterion_awards(
            3,
            &json!({"checks":[
                {"id":"criterion_cards","status":"passed","detail":"Keyboard and pointer opened details"},
                {"id":"criterion_modal","status":"failed","detail":"Title did not receive focus"},
                {"id":"criterion_create_error","status":"unverified","detail":"Fixture unavailable"}
            ]}),
        );
        assert_eq!(awards[0].awarded, Some(10));
        assert_eq!(awards[1].awarded, Some(0));
        assert_eq!(awards[2].awarded, None);
        assert_eq!(awards[3].awarded, None);
        assert_eq!(awards[1].reason, "Title did not receive focus");
    }

    #[test]
    fn startup_failure_does_not_fabricate_functional_criterion_failures() {
        let result = json!({"status":"failed", "functional_status":"failed", "checks":[
            {"id":"application_startup", "status":"failed", "detail":"Compose file is missing"}
        ]});
        validate_evaluation(&json!({"result":result})).unwrap();
        let awards = criterion_awards(0, &result);
        assert!(awards.iter().all(|award| award.awarded.is_none()));
        assert!(awards
            .iter()
            .all(|award| award.reason.contains("prerequisite failed")));
    }

    #[test]
    fn completion_follows_the_gate_criteria_not_the_whole_rubric() {
        let checks = |entries: &[(&str, &str)]| {
            json!({"status":"failed","functional_status":"failed","checks":entries
                .iter().map(|(id, status)| json!({"id":id,"status":status})).collect::<Vec<_>>()})
        };
        // C4: gates are cards + creation; a failed details check only costs points.
        assert_eq!(
            completion(
                3,
                &checks(&[
                    ("criterion_cards", "passed"),
                    ("criterion_creation", "passed"),
                    ("criterion_details", "failed")
                ])
            ),
            CompletionState::Completed
        );
        assert_eq!(
            completion(
                3,
                &checks(&[
                    ("criterion_cards", "passed"),
                    ("criterion_creation", "failed")
                ])
            ),
            CompletionState::TaskIncomplete
        );
        // A gate blocked by an earlier failure is incomplete; one the probe simply never reached is not measured.
        assert_eq!(
            completion(
                4,
                &checks(&[
                    ("criterion_cancel", "failed"),
                    ("criterion_save", "unverified"),
                    ("criterion_drag", "passed")
                ])
            ),
            CompletionState::TaskIncomplete
        );
        assert_eq!(
            completion(
                4,
                &json!({"status":"incomplete","functional_status":"passed","checks":[
                    {"id":"criterion_save","status":"unverified"},{"id":"criterion_drag","status":"passed"}]})
            ),
            CompletionState::Undetermined
        );
        // C6: posting is the gate; the stricter comment assertions only move the score.
        assert_eq!(
            completion(
                5,
                &checks(&[
                    ("criterion_post", "passed"),
                    ("criterion_comments", "failed")
                ])
            ),
            CompletionState::Completed
        );
        // Drag is not a gate: a failed move still leaves the edit flow delivered.
        assert_eq!(
            completion(
                4,
                &checks(&[("criterion_save", "passed"), ("criterion_drag", "failed")])
            ),
            CompletionState::Completed
        );
        assert_eq!(
            completion(0, &checks(&[("application_startup", "failed")])),
            CompletionState::TaskIncomplete
        );
        assert_eq!(
            completion(
                0,
                &json!({"status":"evaluation_failed","functional_status":null,"checks":[]})
            ),
            CompletionState::Undetermined
        );
        assert!(IDS
            .iter()
            .enumerate()
            .all(|(index, _)| rubric(index).iter().any(|criterion| criterion.gate)));
    }

    #[test]
    fn the_reported_gates_are_the_ones_completion_reads() {
        for (index, id) in IDS.iter().enumerate() {
            let reported = criteria(index)
                .into_iter()
                .filter(|criterion| criterion.gate)
                .map(|criterion| criterion.id)
                .collect::<Vec<_>>();
            let decided = rubric(index)
                .iter()
                .filter(|criterion| criterion.gate)
                .map(|criterion| criterion.id.as_str())
                .collect::<Vec<_>>();
            assert_eq!(reported, decided, "{id}");
        }
        // C5: saving is the primary flow.
        assert_eq!(
            criteria(4)
                .into_iter()
                .filter(|criterion| criterion.gate)
                .map(|criterion| criterion.id)
                .collect::<Vec<_>>(),
            ["criterion_save"]
        );
    }

    #[test]
    fn rubric_rejects_missing_duplicate_stale_and_contradictory_evidence() {
        let mut checks: Vec<Value> = rubric(3)
            .iter()
            .map(|criterion| json!({"id":criterion.id,"status":"unverified"}))
            .collect();
        checks[0]["status"] = json!("passed");
        checks.push(json!({"id":"ticket_cards","status":"passed"}));
        let valid = json!({"rubric_version":2,"case_id":IDS[3],"checks":checks});
        validate_rubric(3, &valid).unwrap();
        let mut missing = valid.clone();
        missing["checks"].as_array_mut().unwrap().remove(1);
        assert!(validate_rubric(3, &missing).is_err());
        let mut duplicate = valid.clone();
        duplicate["checks"]
            .as_array_mut()
            .unwrap()
            .push(checks[0].clone());
        assert!(validate_rubric(3, &duplicate).is_err());
        let mut contradictory = valid.clone();
        contradictory["checks"][0]["status"] = json!("failed");
        assert!(validate_rubric(3, &contradictory).is_err());
        let mut stale = valid.clone();
        stale["rubric_version"] = json!(1);
        assert!(validate_rubric(3, &stale).is_err());
        assert!(validate_rubric(2, &valid).is_err());
    }

    #[test]
    fn integrity_diagnostics_keep_both_diffs_and_the_provisional_verdict() {
        let run_id = format!("kanban-integrity-test-{}", uuid::Uuid::new_v4());
        let path = root(&run_id).join("run/evidence");
        fs::create_dir_all(&path).unwrap();
        fs::write(root(&run_id).join("controller.log"), "evaluation finished").unwrap();
        fs::write(path.join("subject.diff"), "delivered source").unwrap();
        fs::write(path.join("evaluated.diff"), "evaluated source").unwrap();
        fs::write(
            path.join("source-integrity.json"),
            br#"{"changed":true,"changed_paths":["app.ts"]}"#,
        )
        .unwrap();
        fs::write(
            path.join("functional-result.json"),
            br#"{"status":"failed","functional_status":"failed"}"#,
        )
        .unwrap();
        fs::write(
            path.join("result.json"),
            br#"{"status":"evaluation_failed","error":"candidate source changed: app.ts"}"#,
        )
        .unwrap();
        let value = diagnostics(&run_id).unwrap();
        assert_eq!(value["diff"], "delivered source");
        assert_eq!(value["evaluated_diff"], "evaluated source");
        assert_eq!(value["source_integrity"]["changed_paths"][0], "app.ts");
        assert_eq!(value["functional_result"]["status"], "failed");
        let error = validate_evaluation(&value).unwrap_err().to_string();
        assert!(error.contains("app.ts"), "{error}");
        fs::remove_dir_all(root(&run_id)).unwrap();
    }

    #[test]
    fn empty_delivery_with_failed_live_probes_remains_a_functional_failure() {
        validate_evaluation(&json!({
            "diff": "", "source_integrity": {"changed": false},
            "result": {"status": "failed", "functional_status": "failed",
                       "checks": [{"id": "live_sse_protocol_contract", "status": "failed"}]},
            "coverage": {"complete": true}
        }))
        .unwrap();
    }
    #[test]
    fn terminal_controller_diagnostics_survive_without_subject_metrics_or_a_diff() {
        let run_id = format!("kanban-diagnostic-test-{}", uuid::Uuid::new_v4());
        let path = root(&run_id);
        fs::create_dir_all(path.join("run/evidence")).unwrap();
        fs::write(
            path.join("controller.log"),
            "controller stopped at command bound",
        )
        .unwrap();
        fs::write(path.join("run/evidence/runtime.log"), "runtime evidence").unwrap();
        fs::write(
            path.join("run/evidence/result.json"),
            br#"{"status":"infrastructure_failed","functional_status":null}"#,
        )
        .unwrap();
        let evidence = diagnostics(&run_id).unwrap();
        assert_eq!(evidence["result"]["status"], "infrastructure_failed");
        assert!(evidence["diff"].is_null());
        assert_eq!(
            evidence["attachments"]["controller.log"]["content"],
            "controller stopped at command bound"
        );
        assert_eq!(
            evidence["attachments"]["runtime.log"]["content"],
            "runtime evidence"
        );
        assert!(evidence.get("metrics").is_none());
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn candidate_prerequisite_failure_is_not_evaluator_unavailability() {
        for id in [
            "application_present",
            "typecheck",
            "test",
            "build",
            "application_startup",
        ] {
            let value = json!({"result":{"status":"failed", "functional_status":"failed", "checks":[{"id":id,"status":"failed"}]}});
            validate_evaluation(&value).unwrap();
        }
        for status in ["infrastructure_failed", "evaluation_failed", "incomplete"] {
            assert!(validate_evaluation(&json!({"result":{"status":status}})).is_err());
        }
        assert!(validate_evaluation(&json!({"result":{"status":"passed","functional_status":"passed"},"coverage":{"complete":false}})).is_err());
        validate_evaluation(&json!({"result":{"status":"passed","functional_status":"passed"},"coverage":{"complete":true}})).unwrap();
    }
    #[test]
    fn attempt_identities_do_not_collide_on_a_short_prefix() {
        assert_ne!(root("abcd1111"), root("abcd2222"));
        assert_ne!(function_id("abcd1111"), function_id("abcd2222"));
    }
    #[test]
    fn pinned_cases_are_valid_and_do_not_expose_future_revisions_in_prompts() {
        for (index, id) in IDS.iter().enumerate() {
            let scenario: crate::scenarios::ScenarioId = id.parse().unwrap();
            let materialized = scenario
                .materialize("contract-test", scenario.canonical_seed())
                .unwrap();
            materialized.validate().unwrap();
            assert_eq!(case(index).id, *id);
            assert!(!materialized
                .spec
                .prompt
                .contains(&case(index).reference_commit));
            assert!(materialized
                .spec
                .prompt
                .contains(&function_id("contract-test")));
            assert!(materialized.spec.prompt.contains("_caller_worker_id"));
            assert!(materialized.spec.prompt.contains("final response"));
        }
    }
}
