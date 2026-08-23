//! Daily engineering ticket benchmark.
//!
//! A protected launcher prepares one reviewed fixture worktree and exports it
//! through `HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH`. The Harness subject
//! owns the ordinary inspect/reproduce/edit/test/report loop. A runner-owned
//! post-turn function independently audits every completion attempt and emits
//! bounded factual feedback without exposing hidden probe source.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use iii_sdk::RegisterFunction;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::Command;

use crate::artifact;
use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::common;
use super::validation_hook::{HookEnvelope, HookVerdict};
use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedDeliverableContent, CapturedInvariant,
    CleanupFuture, ComplexityProfile, DeliverableCaptureFuture, DeliverableContract,
    EvaluationFuture, ExecutionPolicy, InvariantSpec, MaterializedScenario, ProvenanceEvidence,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "engineering_ticket";
pub const VERSION: u32 = 2;
pub const CANONICAL_SEED: u64 = 1005;

const FIXTURE_PATH_ENV: &str = "HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH";
const HOOK_TYPE: &str = "harness::hook::post-turn";
const NETWORK_PROFILE: &str = "offline-v1";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_ASSET_BYTES: u64 = 262_144;
const OWNED_EVIDENCE_DIR: &str = "harness-e2e-engineering-ticket";

const ENGINEERING_DISCIPLINE: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "engineering_discipline",
    25,
    "Relevant source and tests were inspected and a real red baseline was reproduced before the first production edit.",
    EvaluationDimension::StructuralIntegrity,
);
const TICKET_ACCEPTANCE: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "ticket_acceptance",
    40,
    "The focused, hidden semantic, and complete public probes independently accept the final production patch.",
    EvaluationDimension::Deliverable,
);
const VALIDATION_CONVERGENCE: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "validation_convergence",
    20,
    "Completion attempts were durably audited and the latest patch converged within the factual-feedback budget.",
    EvaluationDimension::StructuralIntegrity,
);
const SCOPE_AND_LIFECYCLE: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "scope_and_lifecycle",
    15,
    "Only allowed production paths changed, protected content remained exact, and the terminal session left cleanup-owned resources bounded.",
    EvaluationDimension::StructuralIntegrity,
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    ENGINEERING_DISCIPLINE,
    TICKET_ACCEPTANCE,
    VALIDATION_CONVERGENCE,
    SCOPE_AND_LIFECYCLE,
];

const GRANULAR_GATES: &[(&str, &str)] = &[
    (
        "fixture_identity_exact",
        "The fixture HEAD and reviewed manifest identities are exact.",
    ),
    (
        "fixture_clean_before_run",
        "The fixture was clean before the subject started.",
    ),
    (
        "red_baseline_verified_by_runner",
        "The runner independently observed the declared red baseline.",
    ),
    (
        "relevant_source_read_before_first_edit",
        "A relevant production path was read before the first edit.",
    ),
    (
        "relevant_test_read_before_first_edit",
        "A relevant public test was read before the first edit.",
    ),
    (
        "subject_reproduced_failure_before_first_edit",
        "The subject ran the focused failing probe before editing.",
    ),
    (
        "production_patch_present",
        "A non-empty production patch exists.",
    ),
    (
        "allowed_paths_only",
        "Every changed path is in the case allowlist.",
    ),
    ("tests_unchanged", "Public tests remain byte-identical."),
    (
        "task_manifest_unchanged",
        "The public task manifest remains byte-identical.",
    ),
    (
        "git_metadata_unchanged",
        "HEAD and refs retain their initial identities.",
    ),
    (
        "no_network_or_external_write",
        "No prohibited network or outside-root operation was observed.",
    ),
    (
        "focused_test_passed",
        "The latest independent focused probe passed.",
    ),
    (
        "hidden_semantic_cases_passed",
        "Every latest hidden semantic probe passed.",
    ),
    (
        "full_suite_passed",
        "The latest independent full public suite passed.",
    ),
    (
        "original_failure_eliminated",
        "The original focused failure is eliminated.",
    ),
    (
        "unrelated_behavior_preserved",
        "The full public regression suite remains green.",
    ),
    (
        "patch_file_budget_passed",
        "The changed-file budget is respected.",
    ),
    (
        "patch_line_budget_passed",
        "The changed-line budget is respected.",
    ),
    (
        "completion_attempt_observed",
        "At least one post-turn completion attempt was audited.",
    ),
    (
        "validator_attempts_persisted",
        "Every auditor verdict was persisted before it was returned.",
    ),
    (
        "validator_feedback_factual",
        "Validator feedback contains bounded outcomes and no hidden implementation source.",
    ),
    (
        "validation_round_budget_respected",
        "The factual-feedback retry budget is respected.",
    ),
    (
        "accepted_after_latest_patch",
        "The accepted attempt matches the final patch hash.",
    ),
    (
        "root_session_terminal",
        "The root Harness session reached terminal state.",
    ),
    (
        "child_sessions_terminal",
        "The complete terminal tree includes all child sessions.",
    ),
];

#[derive(Debug, Clone, Copy)]
struct CommandSpec {
    program: &'static str,
    args: &'static [&'static str],
    display: &'static str,
}

const FOCUSED: CommandSpec = CommandSpec {
    program: "python3",
    args: &["tests/focused_probe.py"],
    display: "python3 tests/focused_probe.py",
};
const FULL: CommandSpec = CommandSpec {
    program: "python3",
    args: &[
        "-m",
        "unittest",
        "discover",
        "-s",
        "tests",
        "-p",
        "test_*.py",
    ],
    display: "python3 -m unittest discover -s tests -p test_*.py",
};

#[derive(Debug, Clone, Copy)]
pub struct TaskCase {
    pub id: &'static str,
    pub case_version: u32,
    pub canonical_seed: u64,
    pub fixture_repository: &'static str,
    pub fixture_revision: &'static str,
    pub fixture_manifest_sha256: &'static str,
    pub ticket: &'static str,
    focused_test: CommandSpec,
    full_test: CommandSpec,
    pub allowed_production_paths: &'static [&'static str],
    pub protected_paths: &'static [&'static str],
    pub relevant_read_paths: &'static [&'static str],
    pub public_probe_ids: &'static [&'static str],
    pub hidden_probe_manifest_sha256: &'static str,
    hidden_probes: &'static [HiddenProbe],
    pub maximum_validation_rounds: u8,
    pub maximum_changed_files: u16,
    pub maximum_patch_lines: u32,
    pub complexity_profile: ComplexityProfile,
}

#[derive(Debug, Clone, Copy)]
struct HiddenProbe {
    id: &'static str,
    command: CommandSpec,
}

const CANCELLATION_HIDDEN: &[HiddenProbe] = &[
    HiddenProbe {
        id: "cancelled_terminal_state",
        command: CommandSpec {
            program: "python3",
            args: &["-c", "import asyncio; from src.cancellation import Operation; async def check():\n o=Operation(); e=asyncio.Event(); t=asyncio.create_task(o.run(e)); await e.wait(); t.cancel();\n try: await t\n except asyncio.CancelledError: pass\n assert not o.resource_open and o.state == 'cancelled'\nasyncio.run(check())"],
            display: "hidden:cancelled_terminal_state",
        },
    },
];

const L4_PROFILE: ComplexityProfile = ComplexityProfile {
    planning_depth: 4,
    dependency_depth: 3,
    parallel_branches: 2,
    external_systems: 3,
    state_transitions: 10,
    wake_cycles: 1,
    validation_loops: 2,
    artifact_count: 8,
    coordination_edges: 5,
    ambiguity_level: 6,
};

const CASES: &[TaskCase] = &[
    TaskCase {
        id: "async_cancellation",
        case_version: 1,
        canonical_seed: 1005,
        fixture_repository: "tests/fixtures/engineering-ticket/async_cancellation",
        fixture_revision: "7a6b25b3cd12d66af74a358ae86e0d2b846bd384",
        fixture_manifest_sha256: "sha256:169b2d8ac5f377d6b77b167ced67762f28c9bee1912d8e1ae88849d6b6e30cdf",
        ticket: "Guarantee resource cleanup and a terminal cancelled state when an asynchronous operation is cancelled after it starts.",
        focused_test: FOCUSED,
        full_test: FULL,
        allowed_production_paths: &["src/cancellation.py"],
        protected_paths: &["tests", ".harness-e2e/task-case.json"],
        relevant_read_paths: &["src/cancellation.py", "tests"],
        public_probe_ids: &["cancellation_cleanup"],
        hidden_probe_manifest_sha256: "sha256:cc1cccdab98db4c9b89d52f23d7d9bf83a9069e9af0c952bb316f15434360326",
        hidden_probes: CANCELLATION_HIDDEN,
        maximum_validation_rounds: 2,
        maximum_changed_files: 1,
        maximum_patch_lines: 48,
        complexity_profile: L4_PROFILE,
    },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProbeRecord {
    id: String,
    command: String,
    passed: bool,
    exit_code: Option<i32>,
    timed_out: bool,
    duration_ms: u64,
    stdout_sha256: String,
    stderr_sha256: String,
    observation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BaselineRecord {
    fixture_head: String,
    fixture_manifest_sha256: String,
    initial_ref_sha256: String,
    initial_symbolic_ref: Option<String>,
    protected_hashes: BTreeMap<String, String>,
    focused: ProbeRecord,
    full_suite: ProbeRecord,
    expected_failure_observed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AttemptRecord {
    attempt: u32,
    patch_sha256: String,
    patch_present: bool,
    changed_paths: Vec<String>,
    changed_files: u64,
    changed_lines: u64,
    allowed_paths_only: bool,
    protected_paths_exact: bool,
    focused: ProbeRecord,
    hidden: Vec<ProbeRecord>,
    full_suite: ProbeRecord,
    accepted: bool,
    feedback: Option<String>,
    persisted_before_verdict: bool,
}

#[derive(Debug)]
struct RuntimeEvidence {
    root: PathBuf,
    case: &'static TaskCase,
    baseline: BaselineRecord,
    evidence_dir: PathBuf,
    attempts: Vec<AttemptRecord>,
    infrastructure_error: Option<String>,
}

type SharedRuntime = Arc<Mutex<RuntimeEvidence>>;

fn runtime_registry() -> &'static Mutex<HashMap<String, SharedRuntime>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, SharedRuntime>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id, &CASES[0])
}

pub fn materialize(namespace: &str, _seed: u64) -> Result<MaterializedScenario> {
    let task = task_case();
    task.validate()?;
    let inputs = json!({
        "task_case_id": task.id,
        "case_version": task.case_version,
        "canonical_seed": task.canonical_seed,
        "fixture_repository": task.fixture_repository,
        "fixture_revision": task.fixture_revision,
        "fixture_manifest_sha256": task.fixture_manifest_sha256,
        "ticket": task.ticket,
        "focused_test_command": task.focused_test.display,
        "full_test_command": task.full_test.display,
        "allowed_production_paths": task.allowed_production_paths,
        "protected_paths": task.protected_paths,
        "relevant_read_paths": task.relevant_read_paths,
        "public_probe_ids": task.public_probe_ids,
        "hidden_probe_manifest_sha256": task.hidden_probe_manifest_sha256,
        "maximum_validation_rounds": task.maximum_validation_rounds,
        "maximum_changed_files": task.maximum_changed_files,
        "maximum_patch_lines": task.maximum_patch_lines,
        "network_profile": NETWORK_PROFILE,
    });
    let case = ScenarioCase::new(
        ID,
        VERSION,
        CANONICAL_SEED,
        inputs,
        task.complexity_profile,
        vec![
            "e2e::control-plane-v1".into(),
            "iii::functions".into(),
            "iii::coder".into(),
            "iii::shell".into(),
            "iii::triggers".into(),
            "harness::post-turn-validation".into(),
        ],
        deliverable_contract(),
    )?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(namespace, task),
        case,
        capture: Some(capture),
    })
}

fn task_case() -> &'static TaskCase {
    &CASES[0]
}

impl TaskCase {
    fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty()
            || self.case_version == 0
            || self.maximum_validation_rounds == 0
            || self.maximum_changed_files == 0
            || self.maximum_patch_lines == 0
        {
            bail!(
                "engineering ticket case '{}' has an invalid identity or budget",
                self.id
            );
        }
        for digest in [
            self.fixture_manifest_sha256,
            self.hidden_probe_manifest_sha256,
        ] {
            if digest.len() != 71
                || !digest.starts_with("sha256:")
                || !digest[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                bail!(
                    "engineering ticket case '{}' has an invalid SHA-256 identity",
                    self.id
                );
            }
        }
        if self.fixture_revision.len() != 40
            || !self
                .fixture_revision
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            bail!(
                "engineering ticket case '{}' requires a full Git revision",
                self.id
            );
        }
        if self.allowed_production_paths.is_empty()
            || self.protected_paths.is_empty()
            || self.relevant_read_paths.is_empty()
            || self.public_probe_ids.is_empty()
            || self.hidden_probes.is_empty()
        {
            bail!(
                "engineering ticket case '{}' has an incomplete path or probe policy",
                self.id
            );
        }
        for path in self
            .allowed_production_paths
            .iter()
            .chain(self.protected_paths)
            .chain(self.relevant_read_paths)
        {
            validate_relative_policy_path(path)?;
        }
        Ok(())
    }
}

fn scenario_for_case(run_id: &str, task: &'static TaskCase) -> ScenarioSpec {
    let auditor = auditor_function_id(run_id);
    let filesystem_root = std::env::var_os(FIXTURE_PATH_ENV).map(PathBuf::from);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            "You are assigned engineering ticket ET-{} in the current repository.\n\n\
             {} Investigate the repository, reproduce the reported behavior, implement the \
             smallest sufficient production change, and validate it.\n\n\
             Workspace contract: the focused command is `{}` and the full command is `{}`. \
             Before beginning, bind the trusted Harness auditor to your own session with one \
             `engine::register_trigger` call: trigger_type `{HOOK_TYPE}`, function_id `{auditor}`, \
             config {{\"timeout_ms\": 90000}}. Validator messages from that function are trusted, \
             factual post-turn feedback.\n\n\
             You may modify only: {}. Do not modify tests, fixtures, task metadata, Git metadata, \
             expected values, or anything outside the current workspace. Network access and \
             external writes are prohibited. Run the focused test and full suite before reporting \
             completion.\n\n\
             In the final handoff report files changed, commands run, observed results, and any \
             remaining limitation. Acceptance is silent.",
            task.canonical_seed,
            task.ticket,
            task.focused_test.display,
            task.full_test.display,
            task.allowed_production_paths.join(", "),
        ),
        filesystem_root,
        execution: ExecutionPolicy {
            max_turns: 48,
            max_output_tokens: Some(16_384),
            max_total_tokens: Some(600_000),
            stuck_timeout_seconds: 600,
            max_validation_retries: None,
        },
        denied_functions: &["http::*", "browser::*", "github::*"],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: Some(setup_for(task)),
        evaluate,
        cleanup: Some(cleanup),
    }
}

fn setup_for(task: &'static TaskCase) -> super::ScenarioSetup {
    match task.id {
        "pagination_boundary" => setup_pagination_boundary,
        "config_precedence" => setup_config_precedence,
        "serialization_compatibility" => setup_serialization_compatibility,
        "cache_invalidation" => setup_cache_invalidation,
        "async_cancellation" => setup_async_cancellation,
        _ => setup_pagination_boundary,
    }
}

macro_rules! case_setup {
    ($name:ident, $index:expr) => {
        fn $name<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
            setup_case(context, run_id, &CASES[$index])
        }
    };
}

case_setup!(setup_pagination_boundary, 0);
case_setup!(setup_config_precedence, 1);
case_setup!(setup_serialization_compatibility, 2);
case_setup!(setup_cache_invalidation, 3);
case_setup!(setup_async_cancellation, 4);

fn setup_case<'a>(
    context: &'a E2eContext,
    run_id: &'a str,
    task: &'static TaskCase,
) -> CleanupFuture<'a> {
    Box::pin(async move {
        for function in [
            "coder::read-file",
            "coder::update-file",
            "shell::exec",
            "engine::register_trigger",
        ] {
            if !context.function_exists(function).await? {
                bail!("required engineering capability '{function}' is unavailable");
            }
        }
        let root = fixture_root_from_env()?;
        let baseline = preflight_fixture(task, &root).await?;
        let evidence_dir = owned_evidence_dir(run_id)?;
        std::fs::create_dir_all(&evidence_dir).with_context(|| {
            format!(
                "create auditor evidence directory {}",
                evidence_dir.display()
            )
        })?;
        let runtime = Arc::new(Mutex::new(RuntimeEvidence {
            root: root.clone(),
            case: task,
            baseline,
            evidence_dir,
            attempts: Vec::new(),
            infrastructure_error: None,
        }));
        runtime_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(run_id.to_string(), runtime.clone());

        context.client().register_function(
            auditor_function_id(run_id),
            RegisterFunction::new_async(move |_envelope: HookEnvelope| {
                let runtime = runtime.clone();
                async move {
                    let (root, task, attempt, evidence_dir) = {
                        let evidence = runtime
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        (
                            evidence.root.clone(),
                            evidence.case,
                            evidence.attempts.len() as u32 + 1,
                            evidence.evidence_dir.clone(),
                        )
                    };
                    let mut record = match audit_attempt(task, &root, attempt).await {
                        Ok(record) => record,
                        Err(error) => {
                            runtime
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .infrastructure_error = Some(format!("auditor failed: {error:#}"));
                            return Ok::<HookVerdict, iii_sdk::errors::Error>(HookVerdict {
                                decision: "continue".into(),
                                reason: None,
                            });
                        }
                    };
                    if let Err(error) = persist_attempt(&evidence_dir, &record) {
                        runtime
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .infrastructure_error = Some(format!("persist auditor attempt: {error:#}"));
                        return Ok::<HookVerdict, iii_sdk::errors::Error>(HookVerdict {
                            decision: "continue".into(),
                            reason: None,
                        });
                    }
                    record.persisted_before_verdict = true;
                    let verdict = if record.accepted {
                        HookVerdict {
                            decision: "continue".into(),
                            reason: None,
                        }
                    } else {
                        HookVerdict {
                            decision: "deny".into(),
                            reason: record.feedback.clone(),
                        }
                    };
                    runtime
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .attempts
                        .push(record);
                    Ok::<HookVerdict, iii_sdk::errors::Error>(verdict)
                }
            })
            .description(
                "Attempt-owned engineering acceptance auditor. Reports bounded factual failures; hidden probe source is never returned.",
            ),
        );
        Ok(())
    })
}

async fn preflight_fixture(task: &'static TaskCase, root: &Path) -> Result<BaselineRecord> {
    validate_fixture_root(root)?;
    if !root.join(".git").exists() {
        bail!(
            "engineering fixture {} is not a Git worktree",
            root.display()
        );
    }
    let initial_status = git(root, &["status", "--porcelain=v1", "--untracked-files=all"]).await?;
    if !initial_status.trim().is_empty() {
        bail!("engineering fixture is dirty before the run: {initial_status}");
    }
    let head = git(root, &["rev-parse", "HEAD"]).await?;
    if head != task.fixture_revision {
        bail!(
            "fixture HEAD {head} differs from expected {}",
            task.fixture_revision
        );
    }
    let manifest_path = root.join(".harness-e2e/task-case.json");
    let manifest_bytes = std::fs::read(&manifest_path)
        .with_context(|| format!("read task manifest {}", manifest_path.display()))?;
    let manifest_hash = artifact::sha256_bytes(&manifest_bytes);
    if manifest_hash != task.fixture_manifest_sha256 {
        bail!(
            "task manifest hash {manifest_hash} differs from expected {}",
            task.fixture_manifest_sha256
        );
    }
    let manifest: Value = serde_json::from_slice(&manifest_bytes).context("parse task manifest")?;
    if manifest.get("task_case_id").and_then(Value::as_str) != Some(task.id)
        || manifest.get("canonical_seed").and_then(Value::as_u64) != Some(task.canonical_seed)
        || manifest.get("network_profile").and_then(Value::as_str) != Some(NETWORK_PROFILE)
        || manifest.get("focused_test_command").and_then(Value::as_str)
            != Some(task.focused_test.display)
        || manifest.get("full_test_command").and_then(Value::as_str) != Some(task.full_test.display)
    {
        bail!("task manifest does not match selected case '{}'", task.id);
    }
    reject_escaping_symlinks(root)?;
    let protected_hashes = hash_policy_paths(root, task.protected_paths)?;
    let initial_refs = refs_snapshot(root).await?;
    let initial_ref_sha256 = artifact::sha256_bytes(initial_refs.as_bytes());
    let initial_symbolic_ref = git_optional(root, &["symbolic-ref", "-q", "HEAD"]).await?;
    let focused = run_probe(root, task.public_probe_ids[0], task.focused_test).await?;
    let full_suite = run_probe(root, "full_public_baseline", task.full_test).await?;
    let expected_failure_observed = !focused.passed
        && !full_suite.passed
        && task
            .public_probe_ids
            .iter()
            .any(|probe| focused.observation.contains(probe));
    if !expected_failure_observed {
        bail!(
            "fixture did not reproduce the reviewed red baseline for '{}'",
            task.id
        );
    }
    let after_status = git(root, &["status", "--porcelain=v1", "--untracked-files=all"]).await?;
    if !after_status.trim().is_empty() {
        bail!("baseline probes dirtied the fixture: {after_status}");
    }
    Ok(BaselineRecord {
        fixture_head: head,
        fixture_manifest_sha256: manifest_hash,
        initial_ref_sha256,
        initial_symbolic_ref,
        protected_hashes,
        focused,
        full_suite,
        expected_failure_observed,
    })
}

async fn audit_attempt(
    task: &'static TaskCase,
    root: &Path,
    attempt: u32,
) -> Result<AttemptRecord> {
    let diff = git(
        root,
        &["diff", "--no-ext-diff", task.fixture_revision, "--"],
    )
    .await?;
    let patch_sha256 = artifact::sha256_bytes(diff.as_bytes());
    let stats = diff_stats(root, task.fixture_revision).await?;
    let protected_paths_exact = hash_policy_paths(root, task.protected_paths)?
        == preflight_protected_hashes(task, root).await?;
    let allowed_paths_only = stats
        .paths
        .iter()
        .all(|path| path_allowed(path, task.allowed_production_paths));
    let focused = run_probe(root, task.public_probe_ids[0], task.focused_test).await?;
    let mut hidden = Vec::with_capacity(task.hidden_probes.len());
    for probe in task.hidden_probes {
        hidden.push(run_probe(root, probe.id, probe.command).await?);
    }
    let full_suite = run_probe(root, "full_public_suite", task.full_test).await?;
    let patch_present = !diff.trim().is_empty();
    let changed_files_ok = stats.paths.len() <= usize::from(task.maximum_changed_files);
    let changed_lines_ok = stats.lines <= u64::from(task.maximum_patch_lines);
    let within_round_budget = attempt <= u32::from(task.maximum_validation_rounds) + 1;
    let accepted = patch_present
        && allowed_paths_only
        && protected_paths_exact
        && changed_files_ok
        && changed_lines_ok
        && focused.passed
        && hidden.iter().all(|probe| probe.passed)
        && full_suite.passed
        && within_round_budget;
    let feedback = (!accepted).then(|| {
        factual_feedback(FeedbackFacts {
            task,
            patch_present,
            allowed_paths_only,
            protected_paths_exact,
            changed_files_ok,
            changed_lines_ok,
            focused: &focused,
            hidden: &hidden,
            full_suite: &full_suite,
            within_round_budget,
        })
    });
    Ok(AttemptRecord {
        attempt,
        patch_sha256,
        patch_present,
        changed_paths: stats.paths,
        changed_files: stats.files,
        changed_lines: stats.lines,
        allowed_paths_only,
        protected_paths_exact,
        focused,
        hidden,
        full_suite,
        accepted,
        feedback,
        persisted_before_verdict: false,
    })
}

async fn preflight_protected_hashes(
    task: &'static TaskCase,
    root: &Path,
) -> Result<BTreeMap<String, String>> {
    let runtime = runtime_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .values()
        .find(|runtime| {
            let evidence = runtime
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            evidence.root == root && evidence.case.id == task.id
        })
        .cloned();
    runtime
        .map(|runtime| {
            runtime
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .baseline
                .protected_hashes
                .clone()
        })
        .context("attempt auditor has no matching preflight record")
}

#[derive(Debug, Default)]
struct DiffStats {
    paths: Vec<String>,
    files: u64,
    lines: u64,
}

async fn diff_stats(root: &Path, revision: &str) -> Result<DiffStats> {
    let numstat = git(root, &["diff", "--numstat", revision, "--"]).await?;
    let mut stats = DiffStats::default();
    for line in numstat.lines().filter(|line| !line.trim().is_empty()) {
        let mut fields = line.splitn(3, '\t');
        let added = fields.next().unwrap_or("0").parse::<u64>().unwrap_or(0);
        let deleted = fields.next().unwrap_or("0").parse::<u64>().unwrap_or(0);
        let Some(path) = fields.next() else { continue };
        stats.paths.push(path.replace('\\', "/"));
        stats.lines = stats.lines.saturating_add(added.saturating_add(deleted));
    }
    stats.paths.sort();
    stats.paths.dedup();
    stats.files = stats.paths.len().try_into().unwrap_or(u64::MAX);
    Ok(stats)
}

struct FeedbackFacts<'a> {
    task: &'a TaskCase,
    patch_present: bool,
    allowed_paths_only: bool,
    protected_paths_exact: bool,
    changed_files_ok: bool,
    changed_lines_ok: bool,
    focused: &'a ProbeRecord,
    hidden: &'a [ProbeRecord],
    full_suite: &'a ProbeRecord,
    within_round_budget: bool,
}

fn factual_feedback(facts: FeedbackFacts<'_>) -> String {
    let FeedbackFacts {
        task,
        patch_present,
        allowed_paths_only,
        protected_paths_exact,
        changed_files_ok,
        changed_lines_ok,
        focused,
        hidden,
        full_suite,
        within_round_budget,
    } = facts;
    let mut facts = Vec::new();
    if !patch_present {
        facts.push("production patch: none observed".to_string());
    }
    if !allowed_paths_only {
        facts.push("changed paths: at least one path is outside the production allowlist".into());
    }
    if !protected_paths_exact {
        facts.push("protected paths: content differs from the fixture baseline".into());
    }
    if !focused.passed {
        facts.push(format!("focused public probe {}: failed", focused.id));
    }
    for probe in hidden.iter().filter(|probe| !probe.passed) {
        facts.push(format!("hidden probe {}: failed", probe.id));
    }
    if !full_suite.passed {
        facts.push("full public suite: failed".into());
    }
    if !changed_files_ok {
        facts.push(format!(
            "changed-file budget: exceeded maximum {}",
            task.maximum_changed_files
        ));
    }
    if !changed_lines_ok {
        facts.push(format!(
            "patch-line budget: exceeded maximum {}",
            task.maximum_patch_lines
        ));
    }
    if !within_round_budget {
        facts.push(format!(
            "validation-round budget: exceeded maximum {} repair rounds",
            task.maximum_validation_rounds
        ));
    }
    format!(
        "VALIDATOR: acceptance is incomplete.\n- {}\nRepair the production implementation, rerun relevant tests, and report the observed result. The validator will not prescribe the patch.",
        facts.join("\n- ")
    )
}

fn persist_attempt(directory: &Path, record: &AttemptRecord) -> Result<()> {
    let path = directory.join(format!("attempt-{:02}.json", record.attempt));
    let mut persisted = record.clone();
    persisted.persisted_before_verdict = true;
    let mut bytes = serde_json::to_vec_pretty(&persisted)?;
    bytes.push(b'\n');
    artifact::write_atomic(&path, &bytes)
}

async fn run_probe(root: &Path, id: &str, spec: CommandSpec) -> Result<ProbeRecord> {
    let started = Instant::now();
    let mut command = Command::new(spec.program);
    command
        .args(spec.args)
        .current_dir(root)
        .stdin(Stdio::null())
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONHASHSEED", "0")
        .env("NO_PROXY", "*")
        .env("no_proxy", "*");
    let output = match tokio::time::timeout(COMMAND_TIMEOUT, command.output()).await {
        Ok(output) => output.with_context(|| format!("run probe '{}'", spec.display))?,
        Err(_) => {
            return Ok(ProbeRecord {
                id: id.to_string(),
                command: spec.display.to_string(),
                passed: false,
                exit_code: None,
                timed_out: true,
                duration_ms: elapsed_ms(started),
                stdout_sha256: artifact::sha256_bytes(&[]),
                stderr_sha256: artifact::sha256_bytes(&[]),
                observation: format!("{id}:TIMEOUT"),
            })
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let observation = bounded_observation(&format!("{}\n{}", stdout.trim(), stderr.trim()));
    Ok(ProbeRecord {
        id: id.to_string(),
        command: spec.display.to_string(),
        passed: output.status.success(),
        exit_code: output.status.code(),
        timed_out: false,
        duration_ms: elapsed_ms(started),
        stdout_sha256: artifact::sha256_bytes(&output.stdout),
        stderr_sha256: artifact::sha256_bytes(&output.stderr),
        observation,
    })
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn bounded_observation(value: &str) -> String {
    value.chars().take(512).collect()
}

fn capture<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let evidence = collect_evidence(observation, run_id).await?;
        let evaluation = evaluate_evidence(&evidence, observation)?;
        let invariants = super::captured_gate_invariants(evaluation);
        let session_provenance = ProvenanceEvidence {
            kind: "session".into(),
            source_id: observation.metrics.root_session_id.clone(),
            relation: "observed_engineering_turn".into(),
        };
        Ok(vec![
            json_deliverable(
                "ticket_contract",
                "engineering_ticket_contract",
                evidence.ticket_contract,
                vec![],
                vec![ProvenanceEvidence {
                    kind: "scenario_case".into(),
                    source_id: observation.case.case_id.clone(),
                    relation: "materialized_ticket_contract".into(),
                }],
            ),
            json_deliverable(
                "baseline_record",
                "engineering_baseline",
                serde_json::to_value(&evidence.baseline)?,
                vec![],
                vec![ProvenanceEvidence {
                    kind: "git_revision".into(),
                    source_id: evidence.baseline.fixture_head.clone(),
                    relation: "runner_verified_red_baseline".into(),
                }],
            ),
            json_deliverable(
                "inspection_record",
                "engineering_inspection",
                serde_json::to_value(&evidence.inspection)?,
                vec![],
                vec![session_provenance.clone()],
            ),
            CapturedDeliverable {
                id: "candidate_patch".into(),
                kind: "code_patch".into(),
                content: CapturedDeliverableContent::TextUtf8(evidence.patch.clone()),
                invariants: vec![],
                provenance: vec![ProvenanceEvidence {
                    kind: "git_diff".into(),
                    source_id: evidence.baseline.fixture_head.clone(),
                    relation: "candidate_against_fixture_revision".into(),
                }],
            },
            json_deliverable(
                "change_manifest",
                "change_manifest",
                serde_json::to_value(&evidence.change_manifest)?,
                vec![],
                vec![ProvenanceEvidence {
                    kind: "git_worktree".into(),
                    source_id: evidence.root.display().to_string(),
                    relation: "captured_before_cleanup".into(),
                }],
            ),
            json_deliverable(
                "validation_matrix",
                "validation_matrix",
                json!({ "attempts": evidence.attempts }),
                vec![],
                vec![ProvenanceEvidence {
                    kind: "auditor_function".into(),
                    source_id: auditor_function_id(run_id),
                    relation: "persisted_attempt_verdicts".into(),
                }],
            ),
            json_deliverable(
                "repair_timeline",
                "repair_timeline",
                serde_json::to_value(&evidence.repair_timeline)?,
                vec![],
                vec![session_provenance.clone()],
            ),
            json_deliverable(
                "engineering_report",
                "engineering_report",
                json!({
                    "response": observation.response,
                    "task_case_id": evidence.task.id,
                    "focused_test_command": evidence.task.focused_test.display,
                    "full_test_command": evidence.task.full_test.display,
                    "latest_accepted": evidence.attempts.last().is_some_and(|attempt| attempt.accepted),
                }),
                invariants,
                vec![session_provenance],
            ),
        ])
    })
}

fn json_deliverable(
    id: &str,
    kind: &str,
    content: Value,
    invariants: Vec<CapturedInvariant>,
    provenance: Vec<ProvenanceEvidence>,
) -> CapturedDeliverable {
    CapturedDeliverable {
        id: id.into(),
        kind: kind.into(),
        content: content.into(),
        invariants,
        provenance,
    }
}

#[derive(Debug, Clone, Serialize)]
struct InspectionRecord {
    relevant_source_read_call: Option<u64>,
    relevant_test_read_call: Option<u64>,
    baseline_reproduction_call: Option<u64>,
    first_edit_call: Option<u64>,
    first_focused_green_call: Option<u64>,
    focused_command_count: u64,
    full_suite_command_count: u64,
    identical_command_repetitions: u64,
}

#[derive(Debug, Clone, Serialize)]
struct ChangeManifest {
    initial_revision: String,
    final_head: String,
    initial_ref_sha256: String,
    final_ref_sha256: String,
    patch_sha256: String,
    changed_paths: Vec<String>,
    changed_files: u64,
    changed_lines: u64,
    allowed_paths_only: bool,
    tests_unchanged: bool,
    task_manifest_unchanged: bool,
    protected_paths_exact: bool,
    git_metadata_unchanged: bool,
    status: String,
}

#[derive(Debug, Clone, Serialize)]
struct RepairTimeline {
    completion_attempts: u64,
    validator_nudges: u64,
    first_pass_acceptance: bool,
    repair_converged: bool,
    patch_hashes: Vec<String>,
    patch_lines: Vec<u64>,
}

struct EngineeringEvidence {
    root: PathBuf,
    task: &'static TaskCase,
    ticket_contract: Value,
    baseline: BaselineRecord,
    inspection: InspectionRecord,
    patch: String,
    change_manifest: ChangeManifest,
    attempts: Vec<AttemptRecord>,
    repair_timeline: RepairTimeline,
    nudges: Vec<String>,
    auditor_registrations: usize,
    prohibited_effect_observed: bool,
}

async fn collect_evidence(
    observation: &ScenarioObservation,
    run_id: &str,
) -> Result<EngineeringEvidence> {
    let runtime = runtime_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(run_id)
        .cloned()
        .context("engineering ticket setup evidence is unavailable")?;
    let (root, task, baseline, attempts, infrastructure_error) = {
        let evidence = runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            evidence.root.clone(),
            evidence.case,
            evidence.baseline.clone(),
            evidence.attempts.clone(),
            evidence.infrastructure_error.clone(),
        )
    };
    if let Some(error) = infrastructure_error {
        bail!("engineering ticket auditor infrastructure failure: {error}");
    }
    let patch = git(
        &root,
        &["diff", "--no-ext-diff", task.fixture_revision, "--"],
    )
    .await?;
    let stats = diff_stats(&root, task.fixture_revision).await?;
    let final_head = git(&root, &["rev-parse", "HEAD"]).await?;
    let final_refs = refs_snapshot(&root).await?;
    let final_ref_sha256 = artifact::sha256_bytes(final_refs.as_bytes());
    let status = git(
        &root,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )
    .await?;
    let protected = hash_policy_paths(&root, task.protected_paths)?;
    let baseline_tests = baseline
        .protected_hashes
        .iter()
        .filter(|(path, _)| path.as_str() == "tests" || path.starts_with("tests/"))
        .collect::<BTreeMap<_, _>>();
    let current_tests = protected
        .iter()
        .filter(|(path, _)| path.as_str() == "tests" || path.starts_with("tests/"))
        .collect::<BTreeMap<_, _>>();
    let tests_unchanged = current_tests == baseline_tests;
    let manifest_path = ".harness-e2e/task-case.json";
    let task_manifest_unchanged =
        protected.get(manifest_path) == baseline.protected_hashes.get(manifest_path);
    let protected_paths_exact = protected == baseline.protected_hashes;
    let git_metadata_unchanged =
        final_head == baseline.fixture_head && final_ref_sha256 == baseline.initial_ref_sha256;
    let allowed_paths_only = stats
        .paths
        .iter()
        .all(|path| path_allowed(path, task.allowed_production_paths));
    let inspection = inspection_record(task, &root, &observation.transcript);
    let nudges = nudge_texts(&observation.transcript);
    let calls = common::function_calls(&observation.transcript);
    let auditor = auditor_function_id(run_id);
    let auditor_registrations = calls
        .iter()
        .filter(|call| {
            call.function_id == "engine::register_trigger"
                && call.arguments.get("trigger_type").and_then(Value::as_str) == Some(HOOK_TYPE)
                && call.arguments.get("function_id").and_then(Value::as_str)
                    == Some(auditor.as_str())
                && call.arguments.pointer("/config/sessions").is_none()
        })
        .count();
    let prohibited_effect_observed = calls.iter().any(prohibited_effect);
    let repair_timeline = RepairTimeline {
        completion_attempts: attempts.len().try_into().unwrap_or(u64::MAX),
        validator_nudges: nudges.len().try_into().unwrap_or(u64::MAX),
        first_pass_acceptance: attempts.first().is_some_and(|attempt| attempt.accepted),
        repair_converged: attempts.last().is_some_and(|attempt| attempt.accepted),
        patch_hashes: attempts
            .iter()
            .map(|attempt| attempt.patch_sha256.clone())
            .collect(),
        patch_lines: attempts
            .iter()
            .map(|attempt| attempt.changed_lines)
            .collect(),
    };
    let initial_ref_sha256 = baseline.initial_ref_sha256.clone();
    let patch_sha256 = artifact::sha256_bytes(patch.as_bytes());
    let change_manifest = ChangeManifest {
        initial_revision: task.fixture_revision.into(),
        final_head,
        initial_ref_sha256,
        final_ref_sha256,
        patch_sha256,
        changed_paths: stats.paths,
        changed_files: stats.files,
        changed_lines: stats.lines,
        allowed_paths_only,
        tests_unchanged,
        task_manifest_unchanged,
        protected_paths_exact,
        git_metadata_unchanged,
        status,
    };
    Ok(EngineeringEvidence {
        root,
        task,
        ticket_contract: observation.case.inputs.clone(),
        baseline,
        inspection,
        patch,
        change_manifest,
        attempts,
        repair_timeline,
        nudges,
        auditor_registrations,
        prohibited_effect_observed,
    })
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let evidence = collect_evidence(observation, run_id).await?;
        evaluate_evidence(&evidence, observation)
    })
}

fn evaluate_evidence(
    evidence: &EngineeringEvidence,
    observation: &ScenarioObservation,
) -> Result<super::ObjectiveEvaluation> {
    let inspection = &evidence.inspection;
    let first_edit = inspection.first_edit_call;
    let relevant_source_before_edit = before(inspection.relevant_source_read_call, first_edit);
    let relevant_test_before_edit = before(inspection.relevant_test_read_call, first_edit);
    let baseline_before_edit = before(inspection.baseline_reproduction_call, first_edit);
    let discipline =
        relevant_source_before_edit && relevant_test_before_edit && baseline_before_edit;
    let latest = evidence.attempts.last();
    let focused_passed = latest.is_some_and(|attempt| attempt.focused.passed);
    let hidden_passed =
        latest.is_some_and(|attempt| attempt.hidden.iter().all(|probe| probe.passed));
    let full_passed = latest.is_some_and(|attempt| attempt.full_suite.passed);
    let acceptance = latest.is_some_and(|attempt| attempt.accepted)
        && focused_passed
        && hidden_passed
        && full_passed;
    let all_persisted = !evidence.attempts.is_empty()
        && evidence
            .attempts
            .iter()
            .all(|attempt| attempt.persisted_before_verdict);
    let feedback_factual = evidence.nudges.iter().all(|nudge| {
        nudge.contains("VALIDATOR: acceptance is incomplete")
            && nudge.contains("will not prescribe the patch")
            && !nudge.contains("from src.")
    });
    let round_budget =
        evidence.nudges.len() <= usize::from(evidence.task.maximum_validation_rounds);
    let final_patch_hash = artifact::sha256_bytes(evidence.patch.as_bytes());
    let accepted_after_latest_patch =
        latest.is_some_and(|attempt| attempt.accepted && attempt.patch_sha256 == final_patch_hash);
    let convergence = evidence.auditor_registrations == 1
        && all_persisted
        && feedback_factual
        && round_budget
        && accepted_after_latest_patch;
    let change = &evidence.change_manifest;
    let patch_present = !evidence.patch.trim().is_empty();
    let file_budget = change.changed_files <= u64::from(evidence.task.maximum_changed_files);
    let line_budget = change.changed_lines <= u64::from(evidence.task.maximum_patch_lines);
    let scope = patch_present
        && change.allowed_paths_only
        && change.tests_unchanged
        && change.task_manifest_unchanged
        && change.protected_paths_exact
        && change.git_metadata_unchanged
        && !evidence.prohibited_effect_observed
        && observation.metrics.complete;

    let mut evaluation = assessment::build_evaluation([
        ENGINEERING_DISCIPLINE.full_or_zero(
            discipline,
            format!(
                "source_read={:?}, test_read={:?}, baseline={:?}, first_edit={:?}",
                inspection.relevant_source_read_call,
                inspection.relevant_test_read_call,
                inspection.baseline_reproduction_call,
                inspection.first_edit_call
            ),
        ),
        TICKET_ACCEPTANCE.full_or_zero(
            acceptance,
            format!(
                "focused={focused_passed}, hidden={hidden_passed}, full={full_passed}, latest accepted={}",
                latest.is_some_and(|attempt| attempt.accepted)
            ),
        ),
        VALIDATION_CONVERGENCE.full_or_zero(
            convergence,
            format!(
                "registrations={}, attempts={}, nudges={}, persisted={all_persisted}, latest_patch={accepted_after_latest_patch}",
                evidence.auditor_registrations,
                evidence.attempts.len(),
                evidence.nudges.len()
            ),
        ),
        SCOPE_AND_LIFECYCLE.full_or_zero(
            scope,
            format!(
                "allowed={}, protected={}, refs={}, prohibited_effect={}, terminal={}",
                change.allowed_paths_only,
                change.protected_paths_exact,
                change.git_metadata_unchanged,
                evidence.prohibited_effect_observed,
                observation.metrics.complete
            ),
        ),
    ]);

    let granular = [
        (
            "fixture_identity_exact",
            evidence.baseline.fixture_head == evidence.task.fixture_revision
                && evidence.baseline.fixture_manifest_sha256
                    == evidence.task.fixture_manifest_sha256,
        ),
        ("fixture_clean_before_run", true),
        (
            "red_baseline_verified_by_runner",
            evidence.baseline.expected_failure_observed,
        ),
        (
            "relevant_source_read_before_first_edit",
            relevant_source_before_edit,
        ),
        (
            "relevant_test_read_before_first_edit",
            relevant_test_before_edit,
        ),
        (
            "subject_reproduced_failure_before_first_edit",
            baseline_before_edit,
        ),
        ("production_patch_present", patch_present),
        ("allowed_paths_only", change.allowed_paths_only),
        ("tests_unchanged", change.tests_unchanged),
        ("task_manifest_unchanged", change.task_manifest_unchanged),
        ("git_metadata_unchanged", change.git_metadata_unchanged),
        (
            "no_network_or_external_write",
            !evidence.prohibited_effect_observed,
        ),
        ("focused_test_passed", focused_passed),
        ("hidden_semantic_cases_passed", hidden_passed),
        ("full_suite_passed", full_passed),
        ("original_failure_eliminated", focused_passed),
        ("unrelated_behavior_preserved", full_passed),
        ("patch_file_budget_passed", file_budget),
        ("patch_line_budget_passed", line_budget),
        ("completion_attempt_observed", !evidence.attempts.is_empty()),
        ("validator_attempts_persisted", all_persisted),
        ("validator_feedback_factual", feedback_factual),
        ("validation_round_budget_respected", round_budget),
        ("accepted_after_latest_patch", accepted_after_latest_patch),
        ("root_session_terminal", observation.metrics.complete),
        ("child_sessions_terminal", observation.metrics.complete),
    ];
    for (id, passed) in granular {
        evaluation.hard_gates.push(common::gate(
            id,
            passed,
            format!("deterministic engineering evidence: {id}={passed}"),
        ));
    }
    Ok(evaluation)
}

fn before(left: Option<u64>, right: Option<u64>) -> bool {
    matches!((left, right), (Some(left), Some(right)) if left < right)
}

fn inspection_record(task: &TaskCase, root: &Path, transcript: &Value) -> InspectionRecord {
    let invocations = common::function_invocations(transcript);
    let calls = invocations
        .iter()
        .map(|invocation| invocation.call.clone())
        .collect::<Vec<_>>();
    let first_edit_call = indexed_call(&calls, is_edit_call);
    let relevant_source_read_call = indexed_call(&calls, |call| {
        read_paths(root, call).iter().any(|path| {
            task.allowed_production_paths
                .iter()
                .any(|allowed| policy_path_matches(path, allowed))
        })
    });
    let relevant_test_read_call = indexed_call(&calls, |call| {
        read_paths(root, call)
            .iter()
            .any(|path| policy_path_matches(path, "tests"))
    });
    let mut baseline_reproduction_call = None;
    let mut first_focused_green_call = None;
    let mut focused_indexes = Vec::new();
    let mut full_indexes = Vec::new();
    for (index, invocation) in invocations.iter().enumerate() {
        if shell_command_matches(&invocation.call, task.focused_test) {
            let call_index = index as u64 + 1;
            focused_indexes.push(call_index);
            if let Some(result) = common::function_result(transcript, invocation) {
                let exit = result.pointer("/details/exit_code").and_then(Value::as_i64);
                if exit.is_some_and(|exit| exit != 0) && baseline_reproduction_call.is_none() {
                    baseline_reproduction_call = Some(call_index);
                }
                if exit == Some(0) && first_focused_green_call.is_none() {
                    first_focused_green_call = Some(call_index);
                }
            }
        }
        if shell_command_matches(&invocation.call, task.full_test) {
            full_indexes.push(index as u64 + 1);
        }
    }
    let identical_command_repetitions =
        focused_indexes.len().saturating_sub(1) + full_indexes.len().saturating_sub(1);
    InspectionRecord {
        relevant_source_read_call,
        relevant_test_read_call,
        baseline_reproduction_call,
        first_edit_call,
        first_focused_green_call,
        focused_command_count: focused_indexes.len().try_into().unwrap_or(u64::MAX),
        full_suite_command_count: full_indexes.len().try_into().unwrap_or(u64::MAX),
        identical_command_repetitions: identical_command_repetitions.try_into().unwrap_or(u64::MAX),
    }
}

fn indexed_call(
    calls: &[common::ObservedFunctionCall],
    predicate: impl Fn(&common::ObservedFunctionCall) -> bool,
) -> Option<u64> {
    calls
        .iter()
        .position(predicate)
        .map(|index| index as u64 + 1)
}

fn is_edit_call(call: &common::ObservedFunctionCall) -> bool {
    matches!(
        call.function_id.as_str(),
        "coder::create-file" | "coder::update-file" | "coder::move" | "coder::delete-file"
    ) || (call.function_id == "shell::exec"
        && call
            .arguments
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| {
                matches!(
                    Path::new(command).file_name().and_then(OsStr::to_str),
                    Some("apply_patch" | "patch" | "sed" | "perl")
                )
            }))
}

fn read_paths(root: &Path, call: &common::ObservedFunctionCall) -> Vec<String> {
    let mut values = Vec::new();
    let reads_paths = call.function_id == "coder::read-file"
        || (call.function_id == "shell::exec"
            && call
                .arguments
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| {
                    matches!(
                        Path::new(command).file_name().and_then(OsStr::to_str),
                        Some("cat" | "sed" | "rg" | "grep" | "head" | "tail")
                    )
                }));
    if reads_paths {
        collect_path_strings(&call.arguments, &mut values);
    }
    values
        .into_iter()
        .filter_map(|value| normalize_observed_path_in(root, &value))
        .collect()
}

fn collect_path_strings(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::String(value) => found.push(value.clone()),
        Value::Array(values) => values
            .iter()
            .for_each(|value| collect_path_strings(value, found)),
        Value::Object(values) => values
            .values()
            .for_each(|value| collect_path_strings(value, found)),
        _ => {}
    }
}

fn shell_command_matches(call: &common::ObservedFunctionCall, spec: CommandSpec) -> bool {
    if call.function_id != "shell::exec" {
        return false;
    }
    let Some(command) = call.arguments.get("command").and_then(Value::as_str) else {
        return false;
    };
    if command == spec.display {
        return true;
    }
    let program_matches = Path::new(command).file_name().and_then(OsStr::to_str)
        == Path::new(spec.program).file_name().and_then(OsStr::to_str);
    let args = call.arguments.get("args").and_then(Value::as_array);
    program_matches
        && args.is_some_and(|args| {
            args.iter()
                .filter_map(Value::as_str)
                .eq(spec.args.iter().copied())
        })
}

fn prohibited_effect(call: &common::ObservedFunctionCall) -> bool {
    if call.function_id.starts_with("http::")
        || call.function_id.starts_with("browser::")
        || call.function_id.starts_with("github::")
    {
        return true;
    }
    call.function_id == "shell::exec"
        && call
            .arguments
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| {
                matches!(
                    Path::new(command).file_name().and_then(OsStr::to_str),
                    Some("curl" | "wget" | "scp" | "ssh")
                ) || (Path::new(command).file_name().and_then(OsStr::to_str) == Some("git")
                    && call
                        .arguments
                        .get("args")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .any(|arg| matches!(arg, "push" | "pull" | "fetch" | "clone")))
            })
}

fn nudge_texts(transcript: &Value) -> Vec<String> {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry
                .get("entry_id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.contains("_nudge_"))
                || entry
                    .pointer("/origin/validation")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        })
        .filter_map(|entry| {
            entry
                .pointer("/message/content")
                .and_then(Value::as_array)
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|block| block.get("text").and_then(Value::as_str))
                        .collect::<String>()
                })
        })
        .collect()
}

fn deliverable_contract() -> DeliverableContract {
    let object_schema = json!({ "type": "object", "additionalProperties": true });
    DeliverableContract {
        artifacts: vec![
            artifact_expectation(
                "ticket_contract",
                "engineering_ticket_contract",
                object_schema.clone(),
            ),
            artifact_expectation(
                "baseline_record",
                "engineering_baseline",
                object_schema.clone(),
            ),
            artifact_expectation(
                "inspection_record",
                "engineering_inspection",
                object_schema.clone(),
            ),
            ArtifactExpectation {
                id: "candidate_patch".into(),
                kind: "code_patch".into(),
                media_type: "text/x-diff; charset=utf-8".into(),
                // Text assets are validated by MIME, but every declared schema
                // must still be a syntactically valid JSON Schema.
                schema: json!({}),
                max_size_bytes: MAX_ASSET_BYTES,
            },
            artifact_expectation("change_manifest", "change_manifest", object_schema.clone()),
            artifact_expectation(
                "validation_matrix",
                "validation_matrix",
                object_schema.clone(),
            ),
            artifact_expectation("repair_timeline", "repair_timeline", object_schema.clone()),
            artifact_expectation("engineering_report", "engineering_report", object_schema),
        ],
        invariants: ASSESSMENTS
            .iter()
            .map(|assessment| InvariantSpec {
                id: assessment.id().into(),
                description: assessment.description().into(),
            })
            .chain(
                GRANULAR_GATES
                    .iter()
                    .map(|(id, description)| InvariantSpec {
                        id: (*id).into(),
                        description: (*description).into(),
                    }),
            )
            .collect(),
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn artifact_expectation(id: &str, kind: &str, schema: Value) -> ArtifactExpectation {
    ArtifactExpectation {
        id: id.into(),
        kind: kind.into(),
        media_type: "application/json".into(),
        schema,
        max_size_bytes: MAX_ASSET_BYTES,
    }
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let runtime = runtime_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(run_id);
        let Some(runtime) = runtime else {
            return Ok(());
        };
        let (root, baseline, evidence_dir) = {
            let evidence = runtime
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                evidence.root.clone(),
                evidence.baseline.clone(),
                evidence.evidence_dir.clone(),
            )
        };
        validate_fixture_root(&root)?;
        if let Some(reference) = baseline.initial_symbolic_ref.as_deref() {
            git(&root, &["checkout", "-f", reference]).await?;
        } else {
            git(
                &root,
                &["checkout", "--detach", "-f", &baseline.fixture_head],
            )
            .await?;
        }
        git(&root, &["reset", "--hard", &baseline.fixture_head]).await?;
        restore_refs(&root, &baseline.initial_ref_sha256).await?;
        git(&root, &["clean", "-fd"]).await?;
        let status = git(
            &root,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )
        .await?;
        let head = git(&root, &["rev-parse", "HEAD"]).await?;
        if !status.is_empty() || head != baseline.fixture_head {
            bail!("engineering fixture cleanup did not restore exact clean HEAD");
        }
        validate_owned_evidence_dir(&evidence_dir)?;
        if evidence_dir.exists() {
            std::fs::remove_dir_all(&evidence_dir)
                .with_context(|| format!("remove owned evidence {}", evidence_dir.display()))?;
        }
        Ok(())
    })
}

async fn restore_refs(root: &Path, initial_ref_sha256: &str) -> Result<()> {
    let snapshot = refs_snapshot(root).await?;
    if artifact::sha256_bytes(snapshot.as_bytes()) == initial_ref_sha256 {
        return Ok(());
    }
    // Resetting the original branch repairs its normal ref. Extra refs remain
    // a cleanup error rather than being broadly deleted without their initial
    // names in the retained baseline.
    let refreshed = refs_snapshot(root).await?;
    if artifact::sha256_bytes(refreshed.as_bytes()) != initial_ref_sha256 {
        bail!("Git refs differ from the preflight identity after reset");
    }
    Ok(())
}

fn fixture_root_from_env() -> Result<PathBuf> {
    let raw = std::env::var_os(FIXTURE_PATH_ENV)
        .with_context(|| format!("{FIXTURE_PATH_ENV} must point to the prepared fixture"))?;
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        bail!("{FIXTURE_PATH_ENV} must be absolute: {}", path.display());
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("canonicalize fixture {}", path.display()))?;
    if canonical != path {
        bail!("fixture path must already be canonical: {}", path.display());
    }
    validate_fixture_root(&canonical)?;
    Ok(canonical)
}

fn validate_fixture_root(root: &Path) -> Result<()> {
    if !root.is_absolute() || root.parent().is_none() {
        bail!("fixture root must be a non-root absolute path");
    }
    if root == Path::new("/") {
        bail!("fixture root cannot be filesystem root");
    }
    if std::env::var_os("HOME").is_some_and(|home| home == root.as_os_str()) {
        bail!("fixture root cannot be the home directory");
    }
    if root.to_str().is_none() {
        bail!("fixture root must be valid UTF-8");
    }
    Ok(())
}

fn validate_relative_policy_path(path: &str) -> Result<()> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!(
            "invalid engineering ticket policy path '{}'",
            path.display()
        );
    }
    Ok(())
}

fn reject_escaping_symlinks(root: &Path) -> Result<()> {
    fn visit(root: &Path, directory: &Path) -> Result<()> {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.file_name() == Some(OsStr::new(".git")) {
                continue;
            }
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                let target = path.canonicalize()?;
                if !target.starts_with(root) {
                    bail!("fixture symlink escapes root: {}", path.display());
                }
            } else if metadata.is_dir() {
                visit(root, &path)?;
            }
        }
        Ok(())
    }
    visit(root, root)
}

fn hash_policy_paths(root: &Path, policies: &[&str]) -> Result<BTreeMap<String, String>> {
    let mut files = Vec::new();
    for policy in policies {
        let path = root.join(policy);
        if path.is_file() {
            files.push(path);
        } else if path.is_dir() {
            collect_files(&path, &mut files)?;
        } else {
            // A missing protected path is subject evidence, not an evaluator
            // infrastructure error. The sentinel cannot equal a preflight
            // content digest and therefore deterministically fails integrity.
            files.push(path);
        }
    }
    files.sort();
    files.dedup();
    files
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            let digest = if path.is_file() {
                artifact::sha256_bytes(&std::fs::read(&path)?)
            } else {
                "missing".to_string()
            };
            Ok((relative, digest))
        })
        .collect()
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            collect_files(&path, files)?;
        } else if metadata.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn path_allowed(path: &str, policies: &[&str]) -> bool {
    policies
        .iter()
        .any(|policy| policy_path_matches(path, policy))
}

fn policy_path_matches(path: &str, policy: &str) -> bool {
    let Some(path) = normalize_observed_path(path) else {
        return false;
    };
    let Some(policy) = normalize_observed_path(policy) else {
        return false;
    };
    path == policy || path.starts_with(&format!("{policy}/"))
}

fn normalize_observed_path(path: &str) -> Option<String> {
    let mut normalized = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Prefix(_) | Component::RootDir => return None,
            Component::ParentDir => return None,
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
        }
    }
    (!normalized.as_os_str().is_empty()).then(|| normalized.to_string_lossy().replace('\\', "/"))
}

fn normalize_observed_path_in(root: &Path, path: &str) -> Option<String> {
    let path = Path::new(path);
    if path.is_absolute() {
        let relative = path.strip_prefix(root).ok()?;
        return normalize_observed_path(relative.to_str()?);
    }
    normalize_observed_path(path.to_str()?)
}

fn auditor_function_id(run_id: &str) -> String {
    format!("e2etest::engineering_audit_{}", suffix(run_id))
}

fn suffix(run_id: &str) -> String {
    let mut value: String = run_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(12)
        .collect();
    while value.len() < 4 {
        value.push('0');
    }
    value
}

fn owned_evidence_dir(run_id: &str) -> Result<PathBuf> {
    let directory = std::env::temp_dir()
        .join(OWNED_EVIDENCE_DIR)
        .join(suffix(run_id));
    validate_owned_evidence_dir(&directory)?;
    if directory.exists() {
        bail!(
            "attempt-owned evidence directory already exists: {}",
            directory.display()
        );
    }
    Ok(directory)
}

fn validate_owned_evidence_dir(path: &Path) -> Result<()> {
    let temp = std::env::temp_dir()
        .canonicalize()
        .unwrap_or_else(|_| std::env::temp_dir());
    let parent = path.parent().context("owned evidence path has no parent")?;
    let expected_parent = temp.join(OWNED_EVIDENCE_DIR);
    if parent != expected_parent || path.file_name().is_none() {
        bail!("refusing unscoped evidence cleanup at {}", path.display());
    }
    Ok(())
}

async fn refs_snapshot(root: &Path) -> Result<String> {
    git(
        root,
        &[
            "for-each-ref",
            "--sort=refname",
            "--format=%(refname) %(objectname)",
        ],
    )
    .await
}

async fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .await
        .with_context(|| format!("run git {} in {}", args.join(" "), root.display()))?;
    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

async fn git_optional(root: &Path, args: &[&str]) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .await?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8(output.stdout)?.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn only_the_async_cancellation_case_is_materialized() {
        assert_eq!(task_case().id, "async_cancellation");
        let materialized = materialize("catalog", 1004).unwrap();
        assert_eq!(materialized.case.seed, CANONICAL_SEED);
        assert_eq!(
            materialized.case.inputs["task_case_id"],
            "async_cancellation"
        );
    }

    #[test]
    fn all_cases_validate_and_publish_eight_assets() {
        for case in CASES {
            case.validate().unwrap();
            let materialized = materialize("catalog", case.canonical_seed).unwrap();
            assert_eq!(materialized.case.inputs["task_case_id"], case.id);
            assert_eq!(materialized.case.complexity.profile.artifact_count, 8);
            assert_eq!(materialized.case.deliverable_contract.artifacts.len(), 8);
            assert!(
                materialized
                    .case
                    .deliverable_contract
                    .capture_before_cleanup
            );
            assert!(materialized.case.deliverable_contract.provenance_required);
            assert!(materialized.capture.is_some());
        }
    }

    #[tokio::test]
    async fn fixture_templates_reproduce_exact_revision_manifest_and_red_baseline() {
        for case in CASES {
            let temporary = tempfile::tempdir().unwrap();
            let repository = temporary.path().join(case.id);
            copy_tree(
                &Path::new(env!("CARGO_MANIFEST_DIR")).join(case.fixture_repository),
                &repository,
            )
            .unwrap();
            assert!(std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&repository)
                .status()
                .unwrap()
                .success());
            assert!(std::process::Command::new("git")
                .args(["add", "."])
                .current_dir(&repository)
                .status()
                .unwrap()
                .success());
            let committed = std::process::Command::new("git")
                .args(["commit", "-qm", "engineering ticket fixture"])
                .current_dir(&repository)
                .env("GIT_AUTHOR_NAME", "Harness E2E")
                .env("GIT_AUTHOR_EMAIL", "harness-e2e@example.invalid")
                .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
                .env("GIT_COMMITTER_NAME", "Harness E2E")
                .env("GIT_COMMITTER_EMAIL", "harness-e2e@example.invalid")
                .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
                .status()
                .unwrap();
            assert!(committed.success(), "failed to commit fixture {}", case.id);
            let baseline = preflight_fixture(case, &repository).await.unwrap();
            assert_eq!(baseline.fixture_head, case.fixture_revision, "{}", case.id);
            assert_eq!(
                baseline.fixture_manifest_sha256, case.fixture_manifest_sha256,
                "{}",
                case.id
            );
            assert!(baseline.expected_failure_observed, "{}", case.id);
        }
    }

    #[test]
    fn complexity_tiers_match_the_reviewed_catalog() {
        use super::super::ComplexityTier;
        assert_eq!(
            materialize("retained", CANONICAL_SEED)
                .unwrap()
                .case
                .complexity
                .tier,
            ComplexityTier::L4Coordinated
        );
    }

    #[test]
    fn path_normalization_accepts_equivalent_in_root_forms_and_rejects_escape() {
        assert!(policy_path_matches(
            "./src/pagination.py",
            "src/pagination.py"
        ));
        assert!(policy_path_matches("tests/test_engineering.py", "tests"));
        assert!(!policy_path_matches(
            "../tests/test_engineering.py",
            "tests"
        ));
        assert!(!policy_path_matches(
            "/tmp/tests/test_engineering.py",
            "tests"
        ));
        assert_eq!(
            normalize_observed_path_in(Path::new("/tmp/fixture"), "/tmp/fixture/src/pagination.py"),
            Some("src/pagination.py".into())
        );
        assert_eq!(
            normalize_observed_path_in(Path::new("/tmp/fixture"), "/tmp/outside.py"),
            None
        );
    }

    #[test]
    fn factual_feedback_names_outcomes_without_revealing_probe_source() {
        let failed = ProbeRecord {
            id: "hidden_case".into(),
            command: "hidden:hidden_case".into(),
            passed: false,
            exit_code: Some(1),
            timed_out: false,
            duration_ms: 1,
            stdout_sha256: artifact::sha256_bytes(b""),
            stderr_sha256: artifact::sha256_bytes(b""),
            observation: "failed".into(),
        };
        let feedback = factual_feedback(FeedbackFacts {
            task: &CASES[0],
            patch_present: true,
            allowed_paths_only: true,
            protected_paths_exact: true,
            changed_files_ok: true,
            changed_lines_ok: true,
            focused: &failed,
            hidden: std::slice::from_ref(&failed),
            full_suite: &failed,
            within_round_budget: true,
        });
        assert!(feedback.contains("hidden probe hidden_case: failed"));
        assert!(feedback.contains("will not prescribe the patch"));
        assert!(!feedback.contains("from src."));
        assert!(!feedback.contains("assert "));
    }

    #[test]
    fn command_and_ordering_evidence_is_correlated_by_call_id() {
        let transcript = json!({
            "messages": [
                { "message": { "role": "assistant", "content": [{
                    "type": "function_call", "id": "read-source", "function_id": "coder::read-file",
                    "arguments": { "files": [{ "path": "src/cancellation.py" }] }
                }] } },
                { "message": { "role": "assistant", "content": [{
                    "type": "function_call", "id": "read-test", "function_id": "coder::read-file",
                    "arguments": { "files": [{ "path": "tests/test_engineering.py" }] }
                }] } },
                { "message": { "role": "assistant", "content": [{
                    "type": "function_call", "id": "baseline", "function_id": "shell::exec",
                    "arguments": { "command": "python3", "args": ["tests/focused_probe.py"] }
                }] } },
                { "message": { "role": "function_result", "function_call_id": "baseline", "function_id": "shell::exec", "is_error": false,
                    "content": [], "details": { "exit_code": 1 } } },
                { "message": { "role": "assistant", "content": [{
                    "type": "function_call", "id": "edit", "function_id": "coder::update-file",
                    "arguments": { "files": [{ "path": "src/cancellation.py" }] }
                }] } }
            ]
        });
        let record = inspection_record(&CASES[0], Path::new("/tmp/fixture"), &transcript);
        assert_eq!(record.relevant_source_read_call, Some(1));
        assert_eq!(record.relevant_test_read_call, Some(2));
        assert_eq!(record.baseline_reproduction_call, Some(3));
        assert_eq!(record.first_edit_call, Some(4));
        assert!(before(
            record.baseline_reproduction_call,
            record.first_edit_call
        ));
    }

    #[test]
    fn network_and_external_git_operations_are_observed() {
        let call = common::ObservedFunctionCall {
            function_id: "shell::exec".into(),
            arguments: json!({ "command": "git", "args": ["push", "origin", "main"] }),
        };
        assert!(prohibited_effect(&call));
        let safe = common::ObservedFunctionCall {
            function_id: "shell::exec".into(),
            arguments: json!({ "command": "git", "args": ["diff", "--stat"] }),
        };
        assert!(!prohibited_effect(&safe));
    }

    #[test]
    fn deliverable_contract_declares_every_emitted_invariant_once() {
        let contract = deliverable_contract();
        let ids = contract
            .invariants
            .iter()
            .map(|invariant| invariant.id.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), contract.invariants.len());
        assert_eq!(ids.len(), ASSESSMENTS.len() + GRANULAR_GATES.len());
    }

    fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
        std::fs::create_dir_all(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            let source_path = entry.path();
            let destination_path = destination.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_tree(&source_path, &destination_path)?;
            } else {
                std::fs::copy(source_path, destination_path)?;
            }
        }
        Ok(())
    }
}
