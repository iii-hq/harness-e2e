use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Args;
use iii_sdk::RegisterFunction;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::Command;
use uuid::Uuid;

use crate::context::E2eContext;
use crate::wire::{
    FunctionPolicy, MessageInput, SendOptions, SendRequest, SendResponse, SessionInit,
    StatusReport, TurnStatus,
};

const LIFECYCLE: &str =
    include_str!("../repository-tasks/registry-version-comparison/lifecycle.py");
const CAPTURE: &str = include_str!("../repository-tasks/registry-version-comparison/capture.cjs");
const REQUIREMENTS: &str =
    include_str!("../repository-tasks/registry-version-comparison/requirements.md");
const REFERENCE: &str =
    include_str!("../repository-tasks/registry-version-comparison/reference-plan.md");
const PROMPTS: [&str; 4] = [
    include_str!("../repository-tasks/registry-version-comparison/test-1-planning.md"),
    include_str!("../repository-tasks/registry-version-comparison/test-2-implementation.md"),
    include_str!("../repository-tasks/registry-version-comparison/test-3-environment.md"),
    include_str!("../repository-tasks/registry-version-comparison/test-4-verification.md"),
];

#[derive(Debug, Clone, Args)]
pub struct RegistryTestsArgs {
    #[arg(long, default_value = "ws://127.0.0.1:49134")]
    pub url: String,
    #[arg(long)]
    pub model: String,
    #[arg(long)]
    pub provider: String,
    #[arg(long)]
    pub output: PathBuf,
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=4))]
    pub test: Option<u8>,
    #[arg(long)]
    pub implementation: Option<PathBuf>,
    #[arg(long, default_value_t = 45000)]
    pub base_port: u16,
    #[arg(long, default_value_t = 3600)]
    pub timeout_seconds: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ExecInput {
    command: String,
    timeout_ms: u64,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct ExecOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

#[derive(Debug, Serialize)]
struct TestReport {
    test: u8,
    status: String,
    error: Option<String>,
    session_id: Option<String>,
    transcript: Option<Value>,
    metrics: Option<Value>,
    delivery_status: Option<Value>,
}

#[derive(Debug, Serialize)]
struct RegistryReport {
    evidence_only: bool,
    model: String,
    provider: String,
    tests: Vec<TestReport>,
}

pub async fn run_registry_tests(mut args: RegistryTestsArgs) -> Result<()> {
    validate(&args)?;
    if let Some(parent) = args
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(&args.output).context("--output must be a fresh path")?;
    args.output = args.output.canonicalize()?;
    let assets = args.output.join("controller-assets");
    materialize_assets(&assets)?;
    let context = Arc::new(E2eContext::connect(&args.url).await?);
    context.bind_turn_completed().await?;
    let mut reports = if let Some(test) = args.test {
        vec![run_test(context.clone(), &args, test, args.implementation.as_deref()).await]
    } else {
        let (one, (two, four), three) = tokio::join!(
            run_test(context.clone(), &args, 1, None),
            async {
                let two = run_test(context.clone(), &args, 2, None).await;
                let delivery = args.output.join("test-2/delivery");
                let four = if delivery.exists() {
                    run_test(context.clone(), &args, 4, Some(&delivery)).await
                } else {
                    TestReport {
                        test: 4,
                        status: "blocked".into(),
                        error: Some("Test 2 produced no delivery bundle".into()),
                        session_id: None,
                        transcript: None,
                        metrics: None,
                        delivery_status: None,
                    }
                };
                (two, four)
            },
            run_test(context.clone(), &args, 3, None)
        );
        vec![one, two, three, four]
    };
    reports.sort_by_key(|report| report.test);
    let execution_failed = reports.iter().any(|report| report.status != "finished");
    std::fs::write(
        args.output.join("report.json"),
        serde_json::to_vec_pretty(&RegistryReport {
            evidence_only: true,
            model: args.model.clone(),
            provider: args.provider.clone(),
            tests: reports,
        })?,
    )?;
    context.unbind_turn_completed().await.ok();
    context.shutdown().await;
    if execution_failed {
        bail!(
            "Registry task execution did not finish normally; inspect {}/report.json",
            args.output.display()
        );
    }
    Ok(())
}

fn materialize_assets(assets: &Path) -> Result<()> {
    std::fs::create_dir(assets)?;
    std::fs::write(assets.join("lifecycle.py"), LIFECYCLE)?;
    std::fs::write(assets.join("capture.cjs"), CAPTURE)?;
    std::fs::write(assets.join("requirements.md"), REQUIREMENTS)?;
    std::fs::write(assets.join("reference-plan.md"), REFERENCE)?;
    for (index, (name, prompt)) in ["planning", "implementation", "environment", "verification"]
        .iter()
        .zip(PROMPTS)
        .enumerate()
    {
        std::fs::write(assets.join(format!("test-{}-{name}.md", index + 1)), prompt)?;
    }
    Ok(())
}

fn validate(args: &RegistryTestsArgs) -> Result<()> {
    if args.output.exists() {
        bail!("--output must be a fresh path");
    }
    if args.timeout_seconds == 0 {
        bail!("--timeout-seconds must be positive");
    }
    if args.base_port > u16::MAX - 9 {
        bail!("--base-port must leave room for per-test ports");
    }
    if args.test == Some(4) && args.implementation.is_none() {
        bail!("--implementation is required when running only test 4");
    }
    if let Some(path) = &args.implementation {
        if !path.exists() {
            bail!("--implementation does not exist: {}", path.display());
        }
    }
    Ok(())
}

async fn run_test(
    context: Arc<E2eContext>,
    args: &RegistryTestsArgs,
    test: u8,
    implementation: Option<&Path>,
) -> TestReport {
    let mut result = match run_test_inner(context.clone(), args, test, implementation).await {
        Ok(report) => report,
        Err(error) => TestReport {
            test,
            status: "failed".into(),
            error: Some(format!("{error:#}")),
            session_id: None,
            transcript: None,
            metrics: None,
            delivery_status: None,
        },
    };
    let root = args.output.join(format!("test-{test}"));
    let lifecycle = args.output.join("controller-assets/lifecycle.py");
    let mut finish = Command::new("python3");
    finish
        .arg(&lifecycle)
        .args(["finish", "--root"])
        .arg(&root)
        .args(["--subject-status", &result.status]);
    match checked_json(&mut finish, "finish").await {
        Ok(status) => result.delivery_status = Some(status),
        Err(error) => {
            result.status = "failed".into();
            result.error = Some(merge_error(result.error.take(), error));
        }
    }
    let mut cleanup = Command::new("python3");
    cleanup
        .arg(&lifecycle)
        .args(["cleanup", "--root"])
        .arg(&root);
    match checked_json(&mut cleanup, "cleanup").await {
        Ok(value)
            if value
                .get("cleanup_exit_code")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                == 0 => {}
        Ok(value) => {
            result.status = "failed".into();
            result.error = Some(merge_error(
                result.error.take(),
                anyhow::anyhow!("cleanup failed: {value}"),
            ));
        }
        Err(error) => {
            result.status = "failed".into();
            result.error = Some(merge_error(result.error.take(), error));
        }
    }
    if root.exists() {
        let _ = std::fs::write(
            root.join("controller-report.json"),
            serde_json::to_vec_pretty(&result).unwrap_or_default(),
        );
    }
    result
}

fn merge_error(existing: Option<String>, error: anyhow::Error) -> String {
    existing.map_or_else(
        || format!("{error:#}"),
        |value| format!("{value}; {error:#}"),
    )
}

async fn run_test_inner(
    context: Arc<E2eContext>,
    args: &RegistryTestsArgs,
    test: u8,
    implementation: Option<&Path>,
) -> Result<TestReport> {
    let root = args.output.join(format!("test-{test}"));
    let lifecycle = args.output.join("controller-assets/lifecycle.py");
    let mut prepare = Command::new("python3");
    prepare
        .arg(&lifecycle)
        .args(["prepare", "--root"])
        .arg(&root)
        .args([
            "--test",
            &test.to_string(),
            "--web-port",
            &(args.base_port + (test as u16 * 2)).to_string(),
            "--api-port",
            &(args.base_port + (test as u16 * 2) + 1).to_string(),
            "--assets",
        ])
        .arg(args.output.join("controller-assets"));
    if let Some(path) = implementation {
        prepare.args(["--implementation"]).arg(path);
    }
    checked_json(&mut prepare, "prepare").await?;
    let function_id = format!("registry_task_{}::exec", Uuid::new_v4().simple());
    let exec_root = root.clone();
    let lifecycle_exec = lifecycle.clone();
    let _registration = context.client().register_function(
        function_id.clone(),
        RegisterFunction::new_async(move |input: ExecInput| {
            let root = exec_root.clone();
            let lifecycle = lifecycle_exec.clone();
            async move {
                if input.timeout_ms == 0 || input.timeout_ms > 120_000 {
                    return Err(iii_sdk::errors::Error::Handler(
                        "timeout_ms must be between 1 and 120000".into(),
                    ));
                }
                let timeout = input.timeout_ms;
                let output = Command::new("python3")
                    .arg(lifecycle)
                    .args(["exec", "--root"])
                    .arg(root)
                    .args([
                        "--command",
                        &input.command,
                        "--timeout-ms",
                        &timeout.to_string(),
                    ])
                    .output()
                    .await
                    .map_err(|e| iii_sdk::errors::Error::Handler(e.to_string()))?;
                if !output.status.success() {
                    return Err(iii_sdk::errors::Error::Handler(
                        String::from_utf8_lossy(&output.stderr).into_owned(),
                    ));
                }
                serde_json::from_slice::<ExecOutput>(&output.stdout)
                    .map_err(|e| iii_sdk::errors::Error::Handler(e.to_string()))
            }
        })
        .description("Execute one bounded command inside this Registry task workspace."),
    );
    let session_id = format!("registry-test-{test}-{}", Uuid::new_v4().simple());
    let message = format!("{}\n\nYour only execution tool is `{function_id}`. Use it for every workspace read, write, command, and test. Its `command` runs inside the isolated task container at `/workspace`; `timeout_ms` must be 1..=120000. You may use engine function discovery only to find this exact tool.", PROMPTS[(test - 1) as usize]);
    let response: SendResponse = match context
        .trigger(
            "harness::send",
            SendRequest {
                session_id: Some(session_id.clone()),
                message: MessageInput::Text(message),
                model: Some(args.model.clone()),
                provider: Some(args.provider.clone()),
                idempotency_key: Some(Uuid::new_v4().to_string()),
                session: Some(SessionInit {
                    title: Some(format!("Registry test {test}")),
                    metadata: Some(json!({"registry_test": test, "evidence_only": true})),
                }),
                options: Some(SendOptions {
                    functions: Some(FunctionPolicy {
                        allow: vec![
                            function_id.clone(),
                            "engine::functions::list".into(),
                            "engine::functions::info".into(),
                        ],
                        deny: vec![
                            "shell::*".into(),
                            "coder::*".into(),
                            "state::*".into(),
                            "harness::*".into(),
                            "session::*".into(),
                            "compose::*".into(),
                            "worker::*".into(),
                            "browser::*".into(),
                            "http::*".into(),
                            "git::*".into(),
                            "filesystem::*".into(),
                        ],
                        ..Default::default()
                    }),
                    max_turns: Some(128),
                    max_output_tokens: Some(32_768),
                    ..Default::default()
                }),
            },
        )
        .await
    {
        Ok(response) => response,
        Err(error) => {
            context.stop_session(&session_id, None).await.ok();
            context.teardown(&session_id).await.ok();
            return Err(error);
        }
    };
    if !response.accepted
        || response.merged == Some(true)
        || response.queued == Some(true)
        || response.session_id != session_id
    {
        context.stop_session(&session_id, None).await.ok();
        context.teardown(&session_id).await.ok();
        bail!("harness::send returned an unexpected response for test {test}: {response:?}");
    }
    let wait = tokio::time::timeout(
        Duration::from_secs(args.timeout_seconds),
        context.wait_for_tree(
            &format!("registry-test-{test}"),
            &session_id,
            Duration::from_secs(args.timeout_seconds),
            false,
            None,
        ),
    )
    .await;
    let mut error = None;
    let status = match wait {
        Ok(Ok(_)) => match context
            .trigger::<_, Option<StatusReport>>(
                "harness::status",
                json!({"session_id": &session_id}),
            )
            .await
            .ok()
            .flatten()
            .map(|v| v.status)
        {
            Some(TurnStatus::Completed) => "finished",
            Some(TurnStatus::Cancelled) => "cancelled",
            Some(TurnStatus::Failed) => "failed",
            _ => "unknown",
        },
        Ok(Err(wait_error)) => {
            error = Some(format!("{wait_error:#}"));
            context.stop_session(&session_id, None).await.ok();
            "failed"
        }
        Err(_) => {
            error = Some(format!("test exceeded {} seconds", args.timeout_seconds));
            context.stop_session(&session_id, None).await.ok();
            "timed_out"
        }
    }
    .to_string();
    let transcript = context.transcript(&session_id).await.ok();
    let metrics = context
        .metrics(&session_id)
        .await
        .ok()
        .and_then(|v| serde_json::to_value(v).ok());
    context.teardown(&session_id).await.ok();
    Ok(TestReport {
        test,
        status,
        error,
        session_id: Some(session_id),
        transcript,
        metrics,
        delivery_status: None,
    })
}

async fn checked_json(command: &mut Command, action: &str) -> Result<Value> {
    let output = command
        .output()
        .await
        .with_context(|| format!("run lifecycle {action}"))?;
    if !output.status.success() {
        bail!(
            "lifecycle {action} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("decode lifecycle {action} output"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> RegistryTestsArgs {
        RegistryTestsArgs {
            url: "ws://localhost".into(),
            model: "model".into(),
            provider: "provider".into(),
            output: PathBuf::from("definitely-fresh-registry-output"),
            test: None,
            implementation: None,
            base_port: 45000,
            timeout_seconds: 3600,
        }
    }

    #[test]
    fn materializes_every_lifecycle_input() {
        let root = tempfile::tempdir().unwrap();
        let assets = root.path().join("assets");
        materialize_assets(&assets).unwrap();
        for name in [
            "lifecycle.py",
            "capture.cjs",
            "requirements.md",
            "reference-plan.md",
            "test-1-planning.md",
            "test-2-implementation.md",
            "test-3-environment.md",
            "test-4-verification.md",
        ] {
            assert!(
                !std::fs::read(assets.join(name)).unwrap().is_empty(),
                "{name}"
            );
        }
    }

    #[test]
    fn validates_bounds_and_test_four_handoff() {
        let mut value = args();
        assert!(validate(&value).is_ok());
        value.timeout_seconds = 0;
        assert!(validate(&value).is_err());
        value.timeout_seconds = 1;
        value.base_port = u16::MAX;
        assert!(validate(&value).is_err());
        value.base_port = 45000;
        value.test = Some(4);
        assert!(validate(&value).is_err());
    }
}
