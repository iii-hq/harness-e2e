//! `typescript_chat_service` — build a streaming TypeScript chat service from a
//! frozen skeleton and prove the running application against runner-owned probes.
//!
//! The subject receives an isolated workspace holding `PROTOCOL.md` (the frozen
//! wire contract), `TASK.md` (fourteen numbered goals), a public Node test suite,
//! and `src/server.ts` with every exported function raising `not implemented`.
//! It must implement the whole service: SSE token streaming, exact multi-turn
//! payloads, bounded history, local tools, structured-output titles, per
//! conversation token budgets, provider-failure handling, and preflight.
//!
//! Verification is entirely runner-side and behavioral: after the session ends,
//! the runner starts the subject's application against a scripted
//! OpenAI-compatible provider it owns and drives it over HTTP. The probe lives
//! outside the workspace, so the subject can neither read nor satisfy it by
//! construction. Only Node built-ins are involved — TypeScript is executed
//! directly by Node's type stripping, so the fixture stays dependency-free and
//! offline.
//!
//! `materialize` is a pure function of the pinned constants; only `setup`,
//! `evaluate`, `capture`, and `cleanup` touch the filesystem.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::json;
use tokio::process::Command;

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::validation_loop::suffix;
use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture, ComplexityProfile,
    DeliverableCaptureFuture, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase, ScenarioObservation,
    ScenarioSpec,
};

pub const ID: &str = "typescript_chat_service";
pub const VERSION: u32 = 2;
pub const CANONICAL_SEED: u64 = 7_311;

/// One-paragraph editorial description shown above the prompt on the dashboard.
pub const SUMMARY: &str = "Build a complete streaming chat service in TypeScript from a frozen \
skeleton, then have the running application judged. The subject implements token streaming over \
Server-Sent Events, exact multi-turn provider payloads, bounded history, two locally executed \
tools, structured-output titles, per-conversation token budgets, provider-failure handling, and \
boot preflight \u{2014} Node built-ins only, no dependencies, no build step. After the session the \
runner starts the application against a scripted provider it owns and drives it over HTTP, so \
every goal is graded on observed behavior rather than on the subject's own report.";

const DELIVERABLE_ID: &str = "chat_service_audit";

const SERVER_PATH: &str = "src/server.ts";
const PUBLIC_TEST_PATH: &str = "tests/server.test.ts";
const PROTOCOL_PATH: &str = "PROTOCOL.md";
const TASK_PATH: &str = "TASK.md";
const MANIFEST_PATH: &str = "package.json";
const TSCONFIG_PATH: &str = "tsconfig.json";
const README_PATH: &str = "README.md";
const FINAL_TOKEN: &str = "TS-CHAT-READY";

const SKELETON_SERVER: &str =
    include_str!("../../tests/fixtures/typescript-chat-service/src/server.ts");
const PUBLIC_TESTS: &str =
    include_str!("../../tests/fixtures/typescript-chat-service/tests/server.test.ts");
const PROTOCOL: &str = include_str!("../../tests/fixtures/typescript-chat-service/PROTOCOL.md");
const TASK: &str = include_str!("../../tests/fixtures/typescript-chat-service/TASK.md");
const MANIFEST: &str = include_str!("../../tests/fixtures/typescript-chat-service/package.json");
const TSCONFIG: &str = include_str!("../../tests/fixtures/typescript-chat-service/tsconfig.json");
/// Runner-owned verification harness. Written outside the subject workspace at
/// evaluation time and never visible to the session.
const HIDDEN_PROBE: &str =
    include_str!("../../tests/fixtures/typescript-chat-service/verify/probe.mjs");
/// Reference solution, used only by `cargo test` to prove the fixture is
/// solvable and that the hidden probe accepts a correct implementation.
#[cfg(test)]
const REFERENCE_SERVER: &str =
    include_str!("../../tests/fixtures/typescript-chat-service/reference/server.ts");

const PUBLIC_TEST_TIMEOUT: Duration = Duration::from_secs(180);
const PROBE_TIMEOUT: Duration = Duration::from_secs(600);
const BOOT_TIMEOUT: Duration = Duration::from_secs(60);

/// Hidden checks grouped by the assessment they gate. Every name must exist in
/// the probe output; a missing name counts as a failure.
const SERVICE_CONTRACT_CHECKS: &[&str] = &[
    "service_boots",
    "conversation_created",
    "conversation_persisted",
    "authorized_provider_call",
    "preflight_rejects_missing_key",
];
const STREAMING_CHECKS: &[&str] = &["stream_incremental", "stream_is_live", "done_reports_usage"];
const STATE_CHECKS: &[&str] = &[
    "system_prompt_applied",
    "multi_turn_payload",
    "history_bounded",
    "concurrent_conversations_isolated",
];
const TOOL_CHECKS: &[&str] = &[
    "tools_advertised",
    "tool_calculator",
    "tool_result_returned_to_provider",
    "tool_server_time",
    "structured_title",
    "title_uses_structured_output",
];
const RESILIENCE_CHECKS: &[&str] = &[
    "token_budget_enforced",
    "provider_error_surfaced",
    "failed_turn_not_persisted",
    "failed_turn_not_replayed",
];
const SHAPE_CHECKS: &[&str] = &[
    "zero_dependencies",
    "typescript_without_any",
    "readme_documents_service",
];

const SERVICE_CONTRACT: AssessmentSpec = AssessmentSpec::scored_in(
    "service_contract",
    15,
    "The application boots, serves the documented endpoints with the documented status codes, authenticates against the provider, and refuses to start without its credentials.",
    EvaluationDimension::Deliverable,
);
const STREAMING_FIDELITY: AssessmentSpec = AssessmentSpec::scored_in(
    "streaming_fidelity",
    20,
    "Provider fragments reach the client as individual delta events while the provider is still sending, and the turn closes with accurate usage.",
    EvaluationDimension::Deliverable,
);
const CONVERSATION_STATE: AssessmentSpec = AssessmentSpec::scored_in(
    "conversation_state",
    20,
    "Provider payloads are byte-exact across turns, history is bounded to the configured window, and concurrent conversations stay isolated.",
    EvaluationDimension::StructuralIntegrity,
);
const TOOLS_AND_STRUCTURED_OUTPUT: AssessmentSpec = AssessmentSpec::scored_in(
    "tools_and_structured_output",
    20,
    "Both tools are advertised, executed locally with correct results, returned to the provider, and the conversation title is produced through a structured-output request.",
    EvaluationDimension::Deliverable,
);
const BUDGET_AND_RESILIENCE: AssessmentSpec = AssessmentSpec::scored_in(
    "budget_and_resilience",
    15,
    "The token budget refuses a turn without calling the provider, and a provider failure ends the turn without storing or replaying it.",
    EvaluationDimension::Robustness,
);
const SUITE_AND_SCOPE: AssessmentSpec = AssessmentSpec::scored_in(
    "public_suite_and_scope",
    10,
    "The public suite is green under the runner, the protected fixture is byte-exact, the workspace stays dependency-free, and the service is documented.",
    EvaluationDimension::StructuralIntegrity,
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    SERVICE_CONTRACT,
    STREAMING_FIDELITY,
    CONVERSATION_STATE,
    TOOLS_AND_STRUCTURED_OUTPUT,
    BUDGET_AND_RESILIENCE,
    SUITE_AND_SCOPE,
];

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn allowed_functions(_run_id: &str) -> Vec<String> {
    vec![
        "engine::functions::list".into(),
        "engine::functions::info".into(),
        "coder::*".into(),
        "shell::*".into(),
    ]
}

pub fn materialize(namespace: &str, _seed: u64) -> Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        ID,
        VERSION,
        CANONICAL_SEED,
        json!({
            "task": "streaming-typescript-chat-service",
            "language": "typescript",
            "runtime": "node-type-stripping",
            "dependency_policy": "zero-runtime-and-dev-dependencies",
            "entrypoint": SERVER_PATH,
            "protected_paths": [PUBLIC_TEST_PATH, PROTOCOL_PATH, TASK_PATH, TSCONFIG_PATH],
            "documented_goals": 14,
            "hidden_check_families": 6,
            "final_token": FINAL_TOKEN,
        }),
        ComplexityProfile {
            planning_depth: 5,
            dependency_depth: 3,
            external_systems: 1,
            state_transitions: 6,
            validation_loops: 2,
            artifact_count: 1,
            coordination_edges: 2,
            ambiguity_level: 3,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "e2e::filesystem".to_string(),
            "e2e::shell".to_string(),
            "node".to_string(),
        ],
        deliverable_contract(),
    )?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(namespace),
        case,
        capture: Some(capture),
    })
}

fn scenario_for_case(run_id: &str) -> ScenarioSpec {
    let root = workspace_root(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            r#"Build a streaming chat service in TypeScript inside the isolated workspace `{root}`.

`{TASK_PATH}` lists fourteen numbered goals (G1-G14) and `{PROTOCOL_PATH}` is the frozen wire
contract for the service API, the OpenAI-compatible provider API, and the two tools. Both files
are authoritative; where this prompt is shorter, they decide.

Phase 1 - implement. `{SERVER_PATH}` declares the exported contract with every function raising
`not implemented`. Implement all of it with Node built-ins only: no dependency, no build step, no
network access other than the configured provider. You may add further modules under `src/`, but
`{SERVER_PATH}` stays the entrypoint and keeps its exported names and signatures. Write
`{README_PATH}`. Do not modify `{PUBLIC_TEST_PATH}`, `{PROTOCOL_PATH}`, `{TASK_PATH}`, or
`{TSCONFIG_PATH}`, and do not add a dependency to `{MANIFEST_PATH}`.

Phase 2 - verify the running application yourself. Reproduce the red baseline with `node --test`
before your first edit and rerun it until the whole public suite is green. Then start the service
and exercise it end to end against a provider stub you write yourself outside `src/` and delete
before you finish: stream a turn, run a tool call, exhaust a token budget, and fail a provider
request. The runner owns a hidden verification suite that starts your application against its own
scripted provider and drives it over HTTP, so a goal you did not exercise is a goal you did not
deliver.

Phase 3 - report. Finish with one line per goal in the form `G<number>: <met|not met> - <evidence>`,
then the exact token `{FINAL_TOKEN}` on its own last line if and only if every goal is met and you
observed it. If any goal is unmet, report `INCOMPLETE` instead and name the goals."#,
            root = root.display(),
        ),
        filesystem_root: Some(root),
        execution: ExecutionPolicy {
            max_turns: 80,
            max_output_tokens: Some(32_768),
            max_total_tokens: Some(1_500_000),
            stuck_timeout_seconds: 1_200,
            max_validation_retries: None,
        },
        denied_functions: &["web::*", "scrapling::*", "http::*"],
        criteria: assessment::criteria(ASSESSMENTS),
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
}

// --- workspace ---------------------------------------------------------------

fn workspace_parent() -> PathBuf {
    std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("harness-e2e-typescript-chat-service")
}

fn workspace_root(run_id: &str) -> PathBuf {
    workspace_parent().join(suffix(run_id))
}

/// The hidden probe is written to a sibling of the workspace so the subject
/// never sees it while the session runs.
fn verify_root(run_id: &str) -> PathBuf {
    workspace_parent().join(format!("{}-verify", suffix(run_id)))
}

fn expected_files() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        (SERVER_PATH, SKELETON_SERVER),
        (PUBLIC_TEST_PATH, PUBLIC_TESTS),
        (PROTOCOL_PATH, PROTOCOL),
        (TASK_PATH, TASK),
        (MANIFEST_PATH, MANIFEST),
        (TSCONFIG_PATH, TSCONFIG),
    ])
}

fn protected_files() -> [&'static str; 4] {
    [PUBLIC_TEST_PATH, PROTOCOL_PATH, TASK_PATH, TSCONFIG_PATH]
}

fn ensure_safe_root(root: &Path) -> Result<()> {
    let parent = workspace_parent();
    if root.parent() != Some(parent.as_path()) {
        bail!(
            "refusing workspace operation outside {}: {}",
            parent.display(),
            root.display()
        );
    }
    let Some(leaf) = root.file_name().and_then(|leaf| leaf.to_str()) else {
        bail!("workspace root has no UTF-8 leaf: {}", root.display());
    };
    if leaf.is_empty()
        || !leaf
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("workspace root leaf is unsafe: {leaf:?}");
    }
    Ok(())
}

fn remove_root(root: &Path) -> Result<()> {
    ensure_safe_root(root)?;
    let Ok(metadata) = fs::symlink_metadata(root) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(root)
            .with_context(|| format!("failed removing workspace link {}", root.display()))?;
    } else {
        fs::remove_dir_all(root)
            .with_context(|| format!("failed removing workspace {}", root.display()))?;
    }
    Ok(())
}

fn reset_workspace(root: &Path) -> Result<()> {
    remove_root(root)?;
    for (relative, content) in expected_files() {
        let path = root.join(relative);
        let parent = path
            .parent()
            .with_context(|| format!("workspace file has no parent: {}", path.display()))?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
        fs::write(&path, content)
            .with_context(|| format!("failed writing workspace file {}", path.display()))?;
    }
    Ok(())
}

fn write_probe(root: &Path) -> Result<PathBuf> {
    remove_root(root)?;
    fs::create_dir_all(root)
        .with_context(|| format!("failed creating probe directory {}", root.display()))?;
    let path = root.join("probe.mjs");
    fs::write(&path, HIDDEN_PROBE)
        .with_context(|| format!("failed writing hidden probe {}", path.display()))?;
    Ok(path)
}

fn collect_files(root: &Path) -> Result<Vec<String>> {
    fn visit(root: &Path, directory: &Path, paths: &mut Vec<String>) -> Result<()> {
        for entry in fs::read_dir(directory)
            .with_context(|| format!("failed reading {}", directory.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                let relative = path.strip_prefix(root)?.to_string_lossy().into_owned();
                paths.push(format!("{relative}#symlink"));
            } else if metadata.is_dir() {
                visit(root, &path, paths)?;
            } else if metadata.is_file() {
                paths.push(path.strip_prefix(root)?.to_string_lossy().into_owned());
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    visit(root, root, &mut paths)?;
    paths.sort();
    Ok(paths)
}

/// Additional source modules under `src/` and the required `README.md` are part
/// of the deliverable; anything else is a scope violation.
fn is_allowed_addition(path: &str) -> bool {
    path == README_PATH || (path.starts_with("src/") && path.ends_with(".ts"))
}

// --- process helpers ---------------------------------------------------------

#[derive(Debug, Clone)]
struct CommandOutcome {
    success: bool,
    stdout: String,
    stderr: String,
}

async fn run_node(
    directory: &Path,
    args: &[&str],
    timeout: Duration,
    label: &str,
) -> Result<CommandOutcome> {
    let mut command = Command::new("node");
    command
        .args(args)
        .current_dir(directory)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .with_context(|| format!("{label} timed out"))?
        .with_context(|| format!("failed launching {label}"))?;
    Ok(CommandOutcome {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

async fn run_public_suite(root: &Path) -> Result<CommandOutcome> {
    run_node(root, &["--test"], PUBLIC_TEST_TIMEOUT, "public Node suite").await
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ProbeOutput {
    passed: bool,
    #[serde(default)]
    checks: BTreeMap<String, bool>,
}

impl ProbeOutput {
    fn group(&self, names: &[&str]) -> bool {
        names
            .iter()
            .all(|name| self.checks.get(*name).copied().unwrap_or(false))
    }

    fn failed(&self) -> Vec<String> {
        self.checks
            .iter()
            .filter(|(_, passed)| !**passed)
            .map(|(name, _)| name.clone())
            .collect()
    }
}

async fn run_hidden_probe(run_id: &str) -> Result<(ProbeOutput, String)> {
    let root = workspace_root(run_id);
    let probe_root = verify_root(run_id);
    let probe = write_probe(&probe_root)?;
    let outcome = run_node(
        &probe_root,
        &[
            probe.to_string_lossy().as_ref(),
            root.to_string_lossy().as_ref(),
        ],
        PROBE_TIMEOUT,
        "hidden verification probe",
    )
    .await?;
    let parsed = outcome
        .stdout
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .and_then(|line| serde_json::from_str::<ProbeOutput>(line).ok())
        .unwrap_or_default();
    Ok((
        parsed,
        format!("{}{}", tail(&outcome.stdout), tail(&outcome.stderr)),
    ))
}

fn tail(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() <= 2_000 {
        return trimmed.to_string();
    }
    trimmed[trimmed.len() - 2_000..].to_string()
}

// --- setup -------------------------------------------------------------------

fn setup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let root = workspace_root(run_id);
        reset_workspace(&root)?;
        // The skeleton must execute as TypeScript under this Node build, and it
        // must fail loudly: the runtime check doubles as a red baseline.
        let boot = run_node(&root, &[SERVER_PATH], BOOT_TIMEOUT, "skeleton boot").await?;
        if boot.success {
            bail!("the typescript chat skeleton unexpectedly starts successfully");
        }
        if !boot.stderr.contains("not implemented") {
            bail!(
                "node cannot execute the TypeScript skeleton directly (type stripping requires Node >= 22.6): {}",
                tail(&boot.stderr)
            );
        }
        let public = run_public_suite(&root).await?;
        if public.success {
            bail!("the typescript chat skeleton unexpectedly passes its public suite");
        }
        let (probe, output) = run_hidden_probe(run_id).await?;
        if probe.passed {
            bail!("the typescript chat skeleton unexpectedly passes the hidden probe: {output}");
        }
        if !probe
            .checks
            .get("harness_completed")
            .copied()
            .unwrap_or(false)
        {
            bail!("the hidden verification probe did not complete on this host: {output}");
        }
        Ok(())
    })
}

// --- audit -------------------------------------------------------------------

#[derive(Debug)]
struct ServiceAudit {
    public_tests_passed: bool,
    public_output: String,
    probe: ProbeOutput,
    probe_output: String,
    server_source: Option<String>,
    implementation_present: bool,
    protected_files_exact: bool,
    readme_present: bool,
    unexpected_paths: Vec<String>,
}

impl ServiceAudit {
    fn scope_valid(&self) -> bool {
        self.public_tests_passed
            && self.protected_files_exact
            && self.implementation_present
            && self.readme_present
            && self.unexpected_paths.is_empty()
            && self.probe.group(SHAPE_CHECKS)
    }
}

async fn audit(run_id: &str) -> Result<ServiceAudit> {
    let root = workspace_root(run_id);
    ensure_safe_root(&root)?;
    let server_source = fs::read_to_string(root.join(SERVER_PATH)).ok();
    let implementation_present = server_source
        .as_deref()
        .is_some_and(|source| source != SKELETON_SERVER && !source.trim().is_empty());
    let protected_files_exact = protected_files().into_iter().all(|relative| {
        fs::read_to_string(root.join(relative)).ok().as_deref()
            == expected_files().get(relative).copied()
    });
    let readme_present = fs::read_to_string(root.join(README_PATH))
        .ok()
        .is_some_and(|readme| !readme.trim().is_empty());
    let known = expected_files().keys().copied().collect::<BTreeSet<_>>();
    let unexpected_paths = collect_files(&root)?
        .into_iter()
        .filter(|path| !known.contains(path.as_str()) && !is_allowed_addition(path))
        .collect::<Vec<_>>();
    let public = run_public_suite(&root).await?;
    let (probe, probe_output) = run_hidden_probe(run_id).await?;
    Ok(ServiceAudit {
        public_tests_passed: public.success,
        public_output: format!("{}{}", tail(&public.stdout), tail(&public.stderr)),
        probe,
        probe_output,
        server_source,
        implementation_present,
        protected_files_exact,
        readme_present,
        unexpected_paths,
    })
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    _observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let audit = audit(run_id).await?;
        let failed = audit.probe.failed();
        Ok(assessment::build_evaluation(
            if audit.implementation_present {
                crate::report::CompletionState::Completed
            } else {
                crate::report::CompletionState::TaskIncomplete
            },
            [
                SERVICE_CONTRACT.full_or_zero(
                    audit.probe.group(SERVICE_CONTRACT_CHECKS),
                    format!("checks={SERVICE_CONTRACT_CHECKS:?}, failed={failed:?}"),
                ),
                STREAMING_FIDELITY.full_or_zero(
                    audit.probe.group(STREAMING_CHECKS),
                    format!("checks={STREAMING_CHECKS:?}, failed={failed:?}"),
                ),
                CONVERSATION_STATE.full_or_zero(
                    audit.probe.group(STATE_CHECKS),
                    format!("checks={STATE_CHECKS:?}, failed={failed:?}"),
                ),
                TOOLS_AND_STRUCTURED_OUTPUT.full_or_zero(
                    audit.probe.group(TOOL_CHECKS),
                    format!("checks={TOOL_CHECKS:?}, failed={failed:?}"),
                ),
                BUDGET_AND_RESILIENCE.full_or_zero(
                    audit.probe.group(RESILIENCE_CHECKS),
                    format!("checks={RESILIENCE_CHECKS:?}, failed={failed:?}"),
                ),
                SUITE_AND_SCOPE.full_or_zero(
                    audit.scope_valid(),
                    format!(
                        "public_tests_passed={}, protected_files_exact={}, readme_present={}, unexpected_paths={:?}, shape_failed={:?}; public_output={:?}; probe_output={:?}",
                        audit.public_tests_passed,
                        audit.protected_files_exact,
                        audit.readme_present,
                        audit.unexpected_paths,
                        SHAPE_CHECKS
                            .iter()
                            .filter(|name| !audit.probe.checks.get(**name).copied().unwrap_or(false))
                            .collect::<Vec<_>>(),
                        audit.public_output,
                        audit.probe_output
                    ),
                ),
            ],
        ))
    })
}

fn capture<'a>(
    _context: &'a E2eContext,
    _observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let audit = audit(run_id).await?;
        let behavior = audit.probe.passed;
        let scope = audit.scope_valid();
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "application_audit".to_string(),
            content: json!({
                "entrypoint": SERVER_PATH,
                "source": audit.server_source,
                "public_tests_passed": audit.public_tests_passed,
                "hidden": {
                    "passed": audit.probe.passed,
                    "checks": audit.probe.checks,
                },
                "scope": {
                    "implementation_present": audit.implementation_present,
                    "protected_files_exact": audit.protected_files_exact,
                    "readme_present": audit.readme_present,
                    "unexpected_paths": audit.unexpected_paths,
                },
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: "public_suite_green".to_string(),
                    passed: audit.public_tests_passed,
                    reason: "the runner independently executed `node --test` in the workspace"
                        .to_string(),
                },
                CapturedInvariant {
                    id: "application_behavior_verified".to_string(),
                    passed: behavior,
                    reason: format!(
                        "the runner started the application against its own scripted provider; failed checks: {:?}",
                        audit.probe.failed()
                    ),
                },
                CapturedInvariant {
                    id: "workspace_scope_exact".to_string(),
                    passed: scope,
                    reason: "protected fixture files, dependency policy, and workspace topology were audited"
                        .to_string(),
                },
            ],
            provenance: vec![ProvenanceEvidence {
                kind: "filesystem_path".to_string(),
                source_id: workspace_root(run_id).join(SERVER_PATH).display().to_string(),
                relation: "independently_executed_before_cleanup".to_string(),
            }],
        }])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "application_audit".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["entrypoint", "source", "public_tests_passed", "hidden", "scope"],
                "properties": {
                    "entrypoint": {"const": SERVER_PATH},
                    "source": {"type": ["string", "null"]},
                    "public_tests_passed": {"type": "boolean"},
                    "hidden": {"type": "object"},
                    "scope": {"type": "object"}
                },
                "additionalProperties": false
            }),
            max_size_bytes: 160_000,
        }],
        invariants: vec![
            InvariantSpec {
                id: "public_suite_green".to_string(),
                description: "The runner independently accepts the public Node suite.".to_string(),
            },
            InvariantSpec {
                id: "application_behavior_verified".to_string(),
                description:
                    "The running application satisfies the runner-owned behavioral probes."
                        .to_string(),
            },
            InvariantSpec {
                id: "workspace_scope_exact".to_string(),
                description:
                    "Protected fixture files, the dependency policy, and the topology hold."
                        .to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        remove_root(&verify_root(run_id))?;
        remove_root(&workspace_root(run_id))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_and_materialization_validate() {
        scenario("typescript-chat-test").validate().unwrap();
        materialize("typescript-chat-test", CANONICAL_SEED)
            .unwrap()
            .validate()
            .unwrap();
    }

    #[test]
    fn fixture_topology_is_narrow_and_canonical() {
        assert_eq!(
            expected_files().keys().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([
                SERVER_PATH,
                PUBLIC_TEST_PATH,
                PROTOCOL_PATH,
                TASK_PATH,
                MANIFEST_PATH,
                TSCONFIG_PATH,
            ])
        );
        assert!(SKELETON_SERVER.contains("createServer is not implemented"));
        assert!(TASK.contains("G14"));
        assert!(PROTOCOL.contains("token_budget_exceeded"));
        assert!(MANIFEST.contains("\"dependencies\": {}"));
    }

    #[test]
    fn allowed_additions_are_sources_and_the_readme() {
        for path in [README_PATH, "src/server.ts", "src/tools/calculator.ts"] {
            assert!(is_allowed_addition(path), "{path}");
        }
        for path in [
            "node_modules/left-pad/index.js",
            "tests/extra.test.ts",
            "src/notes.md",
            "provider-stub.mjs",
        ] {
            assert!(!is_allowed_addition(path), "{path}");
        }
    }

    #[test]
    fn probe_groups_cover_the_documented_goals() {
        let groups = [
            SERVICE_CONTRACT_CHECKS,
            STREAMING_CHECKS,
            STATE_CHECKS,
            TOOL_CHECKS,
            RESILIENCE_CHECKS,
            SHAPE_CHECKS,
        ];
        let mut names = BTreeSet::new();
        for group in groups {
            for name in group {
                assert!(names.insert(*name), "duplicate check {name}");
                assert!(HIDDEN_PROBE.contains(&format!("\"{name}\"")), "{name}");
            }
        }
        assert_eq!(names.len(), 25);
    }

    /// The frozen skeleton must be red and the reference solution must satisfy
    /// both the public suite and every hidden check, on the same host that will
    /// grade a subject.
    #[tokio::test]
    async fn skeleton_is_red_and_the_reference_solution_is_green() {
        let run_id = format!("typescript-chat-fixture-{}", std::process::id());
        let root = workspace_root(&run_id);
        reset_workspace(&root).unwrap();
        let public = run_public_suite(&root).await.unwrap();
        assert!(!public.success, "skeleton public suite unexpectedly green");
        let (baseline, output) = run_hidden_probe(&run_id).await.unwrap();
        assert!(!baseline.passed, "skeleton hidden probe unexpectedly green");
        assert!(
            baseline
                .checks
                .get("harness_completed")
                .copied()
                .unwrap_or(false),
            "hidden probe did not complete: {output}"
        );

        fs::write(root.join(SERVER_PATH), REFERENCE_SERVER).unwrap();
        fs::write(
            root.join(README_PATH),
            "# Chat service\n\nRun `npm start`. Configuration: `PORT`, `LLM_BASE_URL`, \
`LLM_API_KEY`, `LLM_MODEL`, `SYSTEM_PROMPT`, `MAX_HISTORY_TURNS`, `TOKEN_BUDGET`. \
`LLM_API_KEY` and `LLM_BASE_URL` are checked at boot and the process exits non-zero when \
either is missing. Endpoints: `GET /healthz`, `POST /api/conversations`, \
`POST /api/conversations/:id/messages` (Server-Sent Events), `GET /api/conversations/:id`. \
Tools: `server_time` and `calculator` are advertised to the provider and executed locally. \
History is bounded by MAX_HISTORY_TURNS and turns are refused past TOKEN_BUDGET. \
Tests: `npm test` runs the public suite with the Node test runner.\n",
        )
        .unwrap();
        let reference_public = run_public_suite(&root).await.unwrap();
        assert!(
            reference_public.success,
            "reference public suite failed: {}{}",
            reference_public.stdout, reference_public.stderr
        );
        let (reference, reference_output) = run_hidden_probe(&run_id).await.unwrap();
        assert!(
            reference.passed,
            "reference solution failed hidden checks {:?}: {reference_output}",
            reference.failed()
        );

        let audit = audit(&run_id).await.unwrap();
        assert!(audit.scope_valid(), "{audit:?}");

        remove_root(&verify_root(&run_id)).unwrap();
        remove_root(&root).unwrap();
    }
}
