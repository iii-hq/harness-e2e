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
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{
    ArtifactExpectation, CapturedDeliverable, CleanupFuture, ComplexityProfile, CriterionAward,
    CriterionSpec, DeliverableCaptureFuture, DeliverableContract, EvaluationFuture,
    ExecutionPolicy, MaterializedScenario, ObjectiveEvaluation, ProvenanceEvidence, ScenarioCase,
    ScenarioObservation, ScenarioSetup, ScenarioSpec,
};
use crate::context::E2eContext;
use crate::report::{CompletionState, EvaluationDimension};

const CATALOG: &str = include_str!("catalog.json");
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

pub fn spec(index: usize, run_id: &str) -> ScenarioSpec {
    let case = case(index);
    let setups: [ScenarioSetup; 7] = [
        setup_c1, setup_c2, setup_c3, setup_c4, setup_c5, setup_c6, setup_c7,
    ];
    ScenarioSpec {
        id: case.id.as_str(), version: 1,
        prompt: format!("{}\n\n{}\n\nAcceptance criteria:\n{}\n\nThe repository is /workspace inside an isolated container. Dependencies are installed; external networking is disabled. Use agent_trigger with {{\"function\":\"{}\",\"description\":\"Inspect repository\",\"payload\":{{\"command\":\"pwd\"}}}} to inspect, edit and test. This is a shell executor, not delegation. Commands have a 120-second and 256-KiB output limit. Reaching either limit returns nonzero feedback after candidate processes are stopped; use a narrower command and continue. Do not inspect the host working directory.",
            catalog().shared_prompt, case.prompt, case.criteria.iter().map(|c| format!("- {c}")).collect::<Vec<_>>().join("\n"), function_id(run_id)),
        filesystem_root: None,
        execution: ExecutionPolicy { max_turns: 100, max_output_tokens: Some(65_536),
            max_total_tokens: Some(1_000_000), stuck_timeout_seconds: 1_800, max_validation_retries: Some(0) },
        denied_functions: &["harness::spawn", "shell::*", "coder::*", "compose::*", "router::*", "harness::send", "harness::run"],
        criteria: criteria(index), setup: Some(setups[index]), evaluate, cleanup: Some(cleanup),
    }
}

fn criteria(index: usize) -> Vec<CriterionSpec> {
    const NAMES: [&str; 5] = [
        "criterion_1",
        "criterion_2",
        "criterion_3",
        "criterion_4",
        "criterion_5",
    ];
    let case = case(index);
    case.criteria
        .iter()
        .enumerate()
        .map(|(i, text)| {
            CriterionSpec::scored(
                NAMES[i],
                100 / case.criteria.len() as u8,
                text,
                EvaluationDimension::Deliverable,
            )
        })
        .collect()
}

pub fn materialize(index: usize, run_id: &str) -> Result<MaterializedScenario> {
    let item = case(index);
    let contract = DeliverableContract {
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
    };
    let case = ScenarioCase::new(
        IDS[index],
        1,
        super::stable_seed(IDS[index]),
        json!({"base_commit":item.base_commit,"reference_commit":item.reference_commit,
            "prompt":item.prompt,"criteria":item.criteria,"catalog_sha256":format!("{:x}", Sha256::digest(CATALOG.as_bytes())),
            "isolation":"docker-none-nonroot-readonly", "max_cost_usd":5, "subject_deadline_seconds":1800}),
        ComplexityProfile {
            planning_depth: 4,
            dependency_depth: 3,
            external_systems: 2,
            state_transitions: 5,
            validation_loops: 3,
            artifact_count: 1,
            ..Default::default()
        },
        vec![
            "e2e::control-plane-v1".into(),
            "iii::functions".into(),
            "docker".into(),
            "node".into(),
            "playwright".into(),
        ],
        contract,
    )?;
    Ok(MaterializedScenario {
        spec: spec(index, run_id),
        case,
        capture: Some(capture),
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExecRequest {
    command: String,
    #[serde(default, rename = "_caller_worker_id")]
    #[schemars(skip)]
    _caller: Option<String>,
}

macro_rules! setup_hook {
    ($name:ident, $index:expr) => {
        fn $name<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
            Box::pin(setup(context, run_id, $index))
        }
    };
}
setup_hook!(setup_c1, 0);
setup_hook!(setup_c2, 1);
setup_hook!(setup_c3, 2);
setup_hook!(setup_c4, 3);
setup_hook!(setup_c5, 4);
setup_hook!(setup_c6, 5);
setup_hook!(setup_c7, 6);

async fn setup(context: &E2eContext, run_id: &str, index: usize) -> Result<()> {
    let path = root(run_id);
    fs::create_dir_all(path.parent().context("Kanban parent")?)?;
    fs::create_dir(&path).context("Kanban attempt directory must be new")?;
    let scripts = path.join("controller");
    fs::create_dir(&scripts)?;
    for (name, source) in [
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
    ] {
        fs::write(scripts.join(name), source)?;
    }
    let config: Value = serde_json::from_slice(&fs::read(
        std::env::var("HARNESS_E2E_KANBAN_RUNTIME").context(
            "HARNESS_E2E_KANBAN_RUNTIME must name the administrator-provisioned runtime JSON",
        )?,
    )?)?;
    let mut command = Command::new("python3");
    command.env("III_TELEMETRY_ENABLED", "false");
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
                        .env("III_TELEMETRY_ENABLED", "false")
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

fn capture<'a>(
    _context: &'a E2eContext,
    _observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
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
    })
}

pub(crate) fn diagnostics(run_id: &str) -> Result<Value> {
    let path = root(run_id);
    let evidence = path.join("run/evidence");
    let mut content = serde_json::Map::new();
    for (name, key) in [
        ("result.json", "result"),
        ("provenance.json", "provenance"),
        ("coverage.json", "coverage"),
    ] {
        let file = evidence.join(name);
        if file.is_file() {
            content.insert(key.into(), serde_json::from_slice(&fs::read(file)?)?);
        }
    }
    let diff = evidence.join("subject.diff");
    if diff.is_file() && fs::metadata(&diff)?.len() > 8 * 1024 * 1024 {
        bail!("candidate diff exceeds evidence bound");
    }
    content.insert(
        "diff".into(),
        if diff.is_file() {
            fs::read_to_string(diff)?.into()
        } else {
            Value::Null
        },
    );
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

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    _run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
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
        let awards = criteria(index)
            .into_iter()
            .map(|criterion| {
                let check = result["checks"]
                    .as_array()
                    .and_then(|checks| checks.iter().find(|check| check["id"] == criterion.id));
                CriterionAward {
                    id: criterion.id.into(),
                    awarded: Some(if check.is_some_and(|c| c["status"] == "passed") {
                        criterion.weight
                    } else {
                        0
                    }),
                    reason: check
                        .map(|c| c["detail"].to_string())
                        .unwrap_or_else(|| "Required check missing".into()),
                }
            })
            .collect();
        Ok(ObjectiveEvaluation {
            completion: if result["status"] == "passed" {
                CompletionState::Completed
            } else {
                CompletionState::TaskIncomplete
            },
            awards,
            infrastructure_error: None,
        })
    })
}

fn validate_evaluation(value: &Value) -> Result<()> {
    let result = &value["result"];
    let prerequisite_failed = result["status"] == "failed"
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
        });
    if !prerequisite_failed
        && (result["functional_status"].is_null() || value["coverage"]["complete"] != true)
    {
        bail!(
            "Kanban evaluator is unavailable or incomplete: {}",
            result["status"]
        );
    }
    Ok(())
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
            let materialized = materialize(index, "contract-test").unwrap();
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
        }
    }
}
