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
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::{IIIClient, RegisterFunction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::Command;

use crate::artifact;
use crate::assessment::{AssessmentOutcome, AssessmentScore};
use crate::context::E2eContext;
use crate::report::{
    E2eRunReport, E2eScenarioReport, EfficiencyReport, EvaluationDimension, RunStatus,
};

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
pub const GIT_HANDOFF_ID: &str = "engineering_ticket_git_handoff";
pub const GIT_HANDOFF_VERSION: u32 = 2;

const FIXTURE_PATH_ENV: &str = "HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH";
const HOOK_TYPE: &str = "harness::hook::post-turn";
const NETWORK_PROFILE: &str = "offline-v1";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_ASSET_BYTES: u64 = 262_144;
const OWNED_EVIDENCE_DIR: &str = "harness-e2e-engineering-ticket";
const GIT_HANDOFF_EVIDENCE_DIR: &str = "harness-e2e-engineering-ticket-git-handoff";
const IMPLEMENTATION_PLAN_PATH: &str = "IMPLEMENTATION_PLAN.md";
const MAX_IMPLEMENTATION_PLAN_BYTES: usize = 32 * 1024;
const GIT_HANDOFF_WAKE_TIMEOUT_MS: u64 = 10 * 60 * 1_000;

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

const HANDOFF_ORCHESTRATION: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "orchestration_discipline",
    15,
    "The Harness root registered phase-scoped validators and wakes before spawning exactly one planner and one implementer in order.",
    EvaluationDimension::StructuralIntegrity,
);
const HANDOFF_GIT_INTEGRITY: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "git_handoff_integrity",
    20,
    "The accepted plan and implementation are clean, linear Git checkpoints on the original branch, and the plan is the only cross-session work product.",
    EvaluationDimension::StructuralIntegrity,
);
const HANDOFF_TICKET_ACCEPTANCE: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "ticket_acceptance",
    35,
    "The focused, hidden semantic, and complete public probes independently accept the committed implementation checkpoint.",
    EvaluationDimension::Deliverable,
);
const HANDOFF_SCOPE_AND_LIFECYCLE: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "scope_and_lifecycle",
    10,
    "The root did not edit, child effects remained in scope, protected content stayed exact, and the complete three-session tree terminated cleanly.",
    EvaluationDimension::StructuralIntegrity,
);
const HANDOFF_PAIRED_EFFICIENCY: AssessmentSpec = AssessmentSpec::score_only_in(
    "paired_efficiency",
    15,
    "Efficiency relative to the matching engineering_ticket baseline from the same suite, weighted toward tokens and observed work rather than wall-clock noise.",
    EvaluationDimension::Efficiency,
);
const HANDOFF_CONVERGENCE: AssessmentSpec = AssessmentSpec::score_only_in(
    "handoff_convergence",
    5,
    "The planner and implementer checkpoints converged on their first attempts without auditor nudges.",
    EvaluationDimension::StructuralIntegrity,
);
const GIT_HANDOFF_REQUIRED_ASSESSMENTS: &[AssessmentSpec] = &[
    HANDOFF_ORCHESTRATION,
    HANDOFF_GIT_INTEGRITY,
    HANDOFF_TICKET_ACCEPTANCE,
    HANDOFF_SCOPE_AND_LIFECYCLE,
];
const GIT_HANDOFF_ASSESSMENTS: &[AssessmentSpec] = &[
    HANDOFF_ORCHESTRATION,
    HANDOFF_GIT_INTEGRITY,
    HANDOFF_TICKET_ACCEPTANCE,
    HANDOFF_SCOPE_AND_LIFECYCLE,
    HANDOFF_PAIRED_EFFICIENCY,
    HANDOFF_CONVERGENCE,
];

const GIT_HANDOFF_GRANULAR_GATES: &[(&str, &str)] = &[
    (
        "fixture_identity_exact",
        "The reviewed fixture revision and manifest are exact.",
    ),
    (
        "red_baseline_verified_by_runner",
        "The runner independently reproduced the red baseline.",
    ),
    (
        "planner_checkpoint_accepted",
        "The planner produced an accepted committed Markdown checkpoint.",
    ),
    (
        "implementation_checkpoint_accepted",
        "The implementer produced an accepted committed production checkpoint.",
    ),
    (
        "plan_checkpoint_ancestor",
        "The accepted plan checkpoint remains an ancestor of the final checkpoint.",
    ),
    (
        "original_branch_preserved",
        "Both checkpoints advance only the original symbolic branch.",
    ),
    (
        "no_merge_commits",
        "Neither accepted phase contains a merge commit.",
    ),
    (
        "no_additional_refs",
        "No tag, branch, or other ref was added or changed outside the original branch.",
    ),
    (
        "worktree_clean_at_checkpoints",
        "The worktree was clean at both accepted checkpoints.",
    ),
    (
        "implementation_plan_preserved",
        "The implementation did not rewrite the accepted plan.",
    ),
    (
        "git_only_handoff",
        "The implementer spawn carried paths only and no planner output or plan body.",
    ),
    (
        "root_did_not_edit",
        "The root session made no shell or coder calls.",
    ),
    (
        "planner_reproduced_baseline",
        "The planner reproduced the focused failure before creating the plan.",
    ),
    (
        "implementation_tests_observed",
        "The implementer ran the focused and full public commands.",
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
        "allowed_paths_only",
        "Every accepted implementation commit touches only allowed production paths.",
    ),
    (
        "protected_paths_exact",
        "Tests, fixtures, and task metadata remain byte-identical.",
    ),
    (
        "patch_budget_passed",
        "The committed implementation remains inside file and line budgets.",
    ),
    (
        "attempts_persisted_before_verdict",
        "Every validator attempt was persisted before its verdict.",
    ),
    (
        "same_session_repairs",
        "Any validation repair remained in the original phase session.",
    ),
    (
        "no_prohibited_effects",
        "No network, remote Git, or external write effect was observed.",
    ),
    (
        "three_session_tree_terminal",
        "The root, planner, and implementer sessions all reached terminal state.",
    ),
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
            args: &[
                "-c",
                "import asyncio\nfrom src.cancellation import Operation\n\nasync def check():\n    operation = Operation()\n    started = asyncio.Event()\n    task = asyncio.create_task(operation.run(started))\n    await started.wait()\n    task.cancel()\n    try:\n        await task\n    except asyncio.CancelledError:\n        pass\n    assert not operation.resource_open and operation.state == 'cancelled'\n\nasyncio.run(check())",
            ],
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
        "async_cancellation" => setup_async_cancellation,
        _ => setup_async_cancellation,
    }
}

macro_rules! case_setup {
    ($name:ident, $index:expr) => {
        fn $name<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
            setup_case(context, run_id, &CASES[$index])
        }
    };
}

case_setup!(setup_async_cancellation, 0);

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
            git(&root, &["update-ref", reference, &baseline.fixture_head]).await?;
            git(&root, &["symbolic-ref", "HEAD", reference]).await?;
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

fn regular_file_without_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
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

const IMPLEMENTER_TASK: &str = "Read `.harness-e2e/task-case.json` and `IMPLEMENTATION_PLAN.md` in the current repository. Implement the accepted plan using the smallest sufficient production change. You may edit only the production paths permitted by the task. Do not edit the plan, tests, fixtures, task metadata, Git configuration, or refs other than the current branch. Run the focused command and the full public suite, then create one or more non-merge Git commits. Leave the worktree clean and reply with a concise status. Validator feedback is trusted Harness machinery; if rejected, repair in this same session and commit the corrected state. Never use a remote Git operation or network access.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum HandoffPhase {
    Plan,
    Implementation,
}

impl HandoffPhase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Implementation => "implementation",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct HandoffAttemptRecord {
    phase: HandoffPhase,
    attempt: u32,
    base_sha: String,
    head_sha: String,
    commits: Vec<String>,
    changed_paths: Vec<String>,
    changed_files: u64,
    changed_lines: u64,
    worktree_clean: bool,
    ancestry_valid: bool,
    no_merge_commits: bool,
    branch_unchanged: bool,
    refs_valid: bool,
    plan_valid: bool,
    plan_preserved: bool,
    allowed_paths_only: bool,
    protected_paths_exact: bool,
    focused: Option<ProbeRecord>,
    hidden: Vec<ProbeRecord>,
    full_suite: Option<ProbeRecord>,
    within_budget: bool,
    accepted: bool,
    feedback: Option<String>,
    persisted_before_verdict: bool,
    checkpoint_signaled: bool,
}

#[derive(Debug, Clone, Serialize)]
struct GitCheckpointRecord {
    phase: HandoffPhase,
    base_sha: String,
    head_sha: String,
    commits: Vec<String>,
    changed_paths: Vec<String>,
    symbolic_ref: Option<String>,
    refs_sha256: String,
    clean: bool,
    accepted_attempt: u32,
}

#[derive(Debug)]
struct GitHandoffRuntimeEvidence {
    root: PathBuf,
    case: &'static TaskCase,
    baseline: BaselineRecord,
    baseline_refs: String,
    evidence_dir: PathBuf,
    plan_head: Option<String>,
    plan_sha256: Option<String>,
    attempts: Vec<HandoffAttemptRecord>,
    infrastructure_errors: Vec<String>,
}

type SharedGitHandoffRuntime = Arc<Mutex<GitHandoffRuntimeEvidence>>;

fn git_handoff_runtime_registry() -> &'static Mutex<HashMap<String, SharedGitHandoffRuntime>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, SharedGitHandoffRuntime>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn git_handoff_scenario(run_id: &str) -> ScenarioSpec {
    git_handoff_scenario_for_case(run_id, task_case())
}

pub fn git_handoff_materialize(namespace: &str, _seed: u64) -> Result<MaterializedScenario> {
    let task = task_case();
    task.validate()?;
    let inputs = json!({
        "task_case_id": task.id,
        "case_version": task.case_version,
        "canonical_seed": task.canonical_seed,
        "reference_scenario_id": ID,
        "workflow_mode": "git_handoff",
        "fixture_repository": task.fixture_repository,
        "fixture_revision": task.fixture_revision,
        "fixture_manifest_sha256": task.fixture_manifest_sha256,
        "ticket": task.ticket,
        "plan_path": IMPLEMENTATION_PLAN_PATH,
        "handoff_payload": "git_only",
        "commit_policy": "one_or_more_linear_commits_per_phase",
        "focused_test_command": task.focused_test.display,
        "full_test_command": task.full_test.display,
        "allowed_production_paths": task.allowed_production_paths,
        "protected_paths": task.protected_paths,
        "public_probe_ids": task.public_probe_ids,
        "hidden_probe_manifest_sha256": task.hidden_probe_manifest_sha256,
        "maximum_validation_rounds_per_phase": task.maximum_validation_rounds,
        "maximum_changed_files": task.maximum_changed_files,
        "maximum_patch_lines": task.maximum_patch_lines,
        "network_profile": NETWORK_PROFILE,
    });
    let case = ScenarioCase::new(
        GIT_HANDOFF_ID,
        GIT_HANDOFF_VERSION,
        CANONICAL_SEED,
        inputs,
        ComplexityProfile {
            planning_depth: 5,
            dependency_depth: 4,
            parallel_branches: 1,
            external_systems: 4,
            state_transitions: 16,
            wake_cycles: 2,
            validation_loops: 4,
            artifact_count: 10,
            coordination_edges: 8,
            ambiguity_level: 6,
        },
        vec![
            "e2e::control-plane-v1".into(),
            "iii::functions".into(),
            "iii::coder".into(),
            "iii::shell".into(),
            "iii::triggers".into(),
            "iii::state".into(),
            "e2e::subagents".into(),
            "harness::post-turn-validation".into(),
        ],
        git_handoff_deliverable_contract(),
    )?;
    Ok(MaterializedScenario {
        spec: git_handoff_scenario_for_case(namespace, task),
        case,
        capture: Some(git_handoff_capture),
    })
}

pub fn git_handoff_required_functions(run_id: &str) -> Vec<String> {
    vec![
        plan_auditor_function_id(run_id),
        implementation_auditor_function_id(run_id),
    ]
}

pub fn git_handoff_allowed_functions(run_id: &str) -> Vec<String> {
    let mut functions = git_handoff_required_functions(run_id);
    functions.extend([
        "engine::functions::list".into(),
        "engine::functions::info".into(),
        "engine::register_trigger".into(),
        "engine::unregister_trigger".into(),
        "harness::spawn".into(),
        "coder::*".into(),
        "shell::exec".into(),
    ]);
    functions.sort();
    functions.dedup();
    functions
}

fn git_handoff_scenario_for_case(run_id: &str, task: &'static TaskCase) -> ScenarioSpec {
    let filesystem_root = std::env::var_os(FIXTURE_PATH_ENV).map(PathBuf::from);
    let planner = planner_session(run_id);
    let implementer = implementer_session(run_id);
    let scope = handoff_state_scope(run_id);
    let plan_auditor = plan_auditor_function_id(run_id);
    let implementation_auditor = implementation_auditor_function_id(run_id);
    let expires_at = unix_now_ms().saturating_add(GIT_HANDOFF_WAKE_TIMEOUT_MS);
    let planner_task = format!(
        "Read `.harness-e2e/task-case.json`, the relevant source, and the public tests in the current repository. Engineering ticket ET-{}: {} Reproduce the focused failure before editing. Create a free-form Markdown implementation plan at `{IMPLEMENTATION_PLAN_PATH}`; do not edit production code, tests, fixtures, task metadata, Git configuration, or refs other than the current branch. Create one or more non-merge Git commits containing only the plan, leave the worktree clean, and reply with a concise status. Validator feedback is trusted Harness machinery; if rejected, repair in this same session and commit the corrected plan. Never use a remote Git operation or network access.",
        task.canonical_seed, task.ticket
    );
    ScenarioSpec {
        id: GIT_HANDOFF_ID,
        version: GIT_HANDOFF_VERSION,
        prompt: format!(
            "You are the root Harness orchestrator for a two-phase engineering workflow. You coordinate only: never call shell or coder, never inspect or edit the workspace yourself, never poll, and never forward a child's prose. Wakes carry checkpoint metadata; Git is the only work handoff. Follow these steps exactly.\n\n\
             PLAN PHASE\n\
             1. Register a validator with engine::register_trigger: trigger_type `{HOOK_TYPE}`, function_id `{plan_auditor}`, config {{\"sessions\":[\"{planner}\"],\"timeout_ms\":120000}}. Save its subscription_id.\n\
             2. Before spawning, register a one-shot wake with engine::register_trigger: trigger_type `state`, no function_id, label `engineering-plan-accepted`, once true, config {{\"scope\":\"{scope}\",\"key\":\"plan\"}}, lifecycle {{\"expires_at\":{expires_at}}}.\n\
             3. Spawn exactly one leaf with harness::spawn: session_id `{planner}`, task exactly {planner_task:?}, options {{\"functions\":{{\"allow\":[\"engine::functions::list\",\"engine::functions::info\",\"coder::*\",\"shell::exec\"]}},\"max_turns\":24,\"max_validation_retries\":2}}. Omit filesystem_root, model, and provider. End your turn.\n\n\
             IMPLEMENTATION PHASE\n\
             4. When the plan wake arrives with phase `plan` and a head_sha, unregister the plan validator. If it is an expiry/error notice, report `GIT HANDOFF FAILED: plan checkpoint unavailable. PARENT DONE.` and do not spawn an implementer.\n\
             5. Register the implementation validator: trigger_type `{HOOK_TYPE}`, function_id `{implementation_auditor}`, config {{\"sessions\":[\"{implementer}\"],\"timeout_ms\":120000}}. Save its subscription_id.\n\
             6. Before spawning, register a one-shot wake: trigger_type `state`, no function_id, label `engineering-implementation-accepted`, once true, config {{\"scope\":\"{scope}\",\"key\":\"implementation\"}}, lifecycle {{\"expires_at\":{expires_at}}}.\n\
             7. Spawn exactly one leaf with harness::spawn: session_id `{implementer}`, task exactly {IMPLEMENTER_TASK:?}, options {{\"functions\":{{\"allow\":[\"engine::functions::list\",\"engine::functions::info\",\"coder::*\",\"shell::exec\"]}},\"max_turns\":40,\"max_validation_retries\":2}}. Omit filesystem_root, model, and provider. End your turn.\n\n\
             FINALIZATION\n\
             8. When the implementation wake arrives with phase `implementation` and a head_sha, unregister the implementation validator and reply `GIT HANDOFF COMPLETE at <head_sha>. PARENT DONE.` If it is an expiry/error notice, unregister the implementation validator and reply `GIT HANDOFF FAILED: implementation checkpoint unavailable. PARENT DONE.`",
        ),
        filesystem_root,
        execution: ExecutionPolicy {
            max_turns: 64,
            max_output_tokens: Some(16_384),
            max_total_tokens: Some(600_000),
            stuck_timeout_seconds: 900,
            max_validation_retries: None,
        },
        denied_functions: &["http::*", "browser::*", "github::*"],
        criteria: assessment::criteria(GIT_HANDOFF_ASSESSMENTS),
        judge_reference: None,
        setup: Some(git_handoff_setup),
        evaluate: git_handoff_evaluate,
        cleanup: Some(git_handoff_cleanup),
    }
}

fn planner_session(run_id: &str) -> String {
    format!("e2e_{run_id}-planner")
}

fn implementer_session(run_id: &str) -> String {
    format!("e2e_{run_id}-implementer")
}

fn handoff_state_scope(run_id: &str) -> String {
    format!("engineering-git-handoff-{}", suffix(run_id))
}

fn plan_auditor_function_id(run_id: &str) -> String {
    format!("e2etest::engineering_plan_audit_{}", suffix(run_id))
}

fn implementation_auditor_function_id(run_id: &str) -> String {
    format!(
        "e2etest::engineering_implementation_audit_{}",
        suffix(run_id)
    )
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn git_handoff_setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        for function in [
            "coder::read-file",
            "coder::update-file",
            "shell::exec",
            "engine::register_trigger",
            "engine::unregister_trigger",
            "harness::spawn",
            "state::set",
            "state::delete",
        ] {
            if !context.function_exists(function).await? {
                bail!("required Git handoff capability '{function}' is unavailable");
            }
        }
        let task = task_case();
        let root = fixture_root_from_env()?;
        let baseline = preflight_fixture(task, &root).await?;
        let remotes = git(&root, &["remote"]).await?;
        if !remotes.trim().is_empty() {
            bail!("engineering Git handoff fixture must not expose Git remotes");
        }
        for key in ["user.name", "user.email"] {
            if git_optional(&root, &["config", "--local", "--get", key])
                .await?
                .is_none_or(|value| value.trim().is_empty())
            {
                bail!("engineering Git handoff fixture requires local Git {key}");
            }
        }
        let baseline_refs = refs_snapshot(&root).await?;
        let evidence_dir = git_handoff_owned_evidence_dir(run_id)?;
        std::fs::create_dir_all(&evidence_dir).with_context(|| {
            format!(
                "create Git handoff auditor evidence directory {}",
                evidence_dir.display()
            )
        })?;
        let runtime = Arc::new(Mutex::new(GitHandoffRuntimeEvidence {
            root,
            case: task,
            baseline,
            baseline_refs,
            evidence_dir,
            plan_head: None,
            plan_sha256: None,
            attempts: Vec::new(),
            infrastructure_errors: Vec::new(),
        }));
        git_handoff_runtime_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(run_id.to_string(), runtime.clone());

        let plan_runtime = runtime.clone();
        let plan_client = context.client().clone();
        let plan_scope = handoff_state_scope(run_id);
        context.client().register_function(
            plan_auditor_function_id(run_id),
            RegisterFunction::new_async(move |_envelope: HookEnvelope| {
                let runtime = plan_runtime.clone();
                let client = plan_client.clone();
                let scope = plan_scope.clone();
                async move { run_handoff_auditor(runtime, client, scope, HandoffPhase::Plan).await }
            })
            .description(
                "Runner-owned Git plan checkpoint auditor. It validates committed state and emits only factual repair feedback.",
            ),
        );

        let implementation_runtime = runtime;
        let implementation_client = context.client().clone();
        let implementation_scope = handoff_state_scope(run_id);
        context.client().register_function(
            implementation_auditor_function_id(run_id),
            RegisterFunction::new_async(move |_envelope: HookEnvelope| {
                let runtime = implementation_runtime.clone();
                let client = implementation_client.clone();
                let scope = implementation_scope.clone();
                async move {
                    run_handoff_auditor(
                        runtime,
                        client,
                        scope,
                        HandoffPhase::Implementation,
                    )
                    .await
                }
            })
            .description(
                "Runner-owned committed implementation auditor. It runs independent probes and emits only factual repair feedback.",
            ),
        );
        Ok(())
    })
}

async fn run_handoff_auditor(
    runtime: SharedGitHandoffRuntime,
    client: IIIClient,
    scope: String,
    phase: HandoffPhase,
) -> std::result::Result<HookVerdict, iii_sdk::errors::Error> {
    let (root, task, baseline, baseline_refs, plan_head, plan_sha256, attempt, evidence_dir) = {
        let evidence = runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            evidence.root.clone(),
            evidence.case,
            evidence.baseline.clone(),
            evidence.baseline_refs.clone(),
            evidence.plan_head.clone(),
            evidence.plan_sha256.clone(),
            evidence
                .attempts
                .iter()
                .filter(|record| record.phase == phase)
                .count() as u32
                + 1,
            evidence.evidence_dir.clone(),
        )
    };
    let audited = match phase {
        HandoffPhase::Plan => {
            audit_plan_checkpoint(task, &root, &baseline, &baseline_refs, attempt).await
        }
        HandoffPhase::Implementation => match (plan_head, plan_sha256) {
            (Some(plan_head), Some(plan_sha256)) => {
                audit_implementation_checkpoint(
                    task,
                    &root,
                    &baseline,
                    &baseline_refs,
                    &plan_head,
                    &plan_sha256,
                    attempt,
                )
                .await
            }
            (None, _) => Err(anyhow::anyhow!(
                "implementation auditor has no accepted plan checkpoint"
            )),
            (_, None) => Err(anyhow::anyhow!(
                "implementation auditor has no accepted plan digest"
            )),
        },
    };
    let mut record = match audited {
        Ok(record) => record,
        Err(error) => {
            runtime
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .infrastructure_errors
                .push(format!("{} auditor failed: {error:#}", phase.as_str()));
            return Ok(HookVerdict {
                decision: "deny".into(),
                reason: Some(format!(
                    "VALIDATOR: trusted {} checkpoint audit was unavailable; leave the worktree unchanged and retry.",
                    phase.as_str()
                )),
            });
        }
    };
    record.persisted_before_verdict = true;
    if let Err(error) = persist_handoff_attempt(&evidence_dir, &record) {
        runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .infrastructure_errors
            .push(format!(
                "persist {} auditor attempt: {error:#}",
                phase.as_str()
            ));
        return Ok(HookVerdict {
            decision: "deny".into(),
            reason: Some(format!(
                "VALIDATOR: trusted {} checkpoint evidence could not be persisted; leave the worktree unchanged and retry.",
                phase.as_str()
            )),
        });
    }

    let accepted = record.accepted;
    let head_sha = record.head_sha.clone();
    let feedback = record.feedback.clone();
    {
        let mut evidence = runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if accepted && phase == HandoffPhase::Plan {
            evidence.plan_head = Some(head_sha.clone());
            evidence.plan_sha256 = std::fs::read(root.join(IMPLEMENTATION_PLAN_PATH))
                .ok()
                .map(|bytes| artifact::sha256_bytes(&bytes));
        }
        evidence.attempts.push(record);
    }

    if !accepted {
        return Ok(HookVerdict {
            decision: "deny".into(),
            reason: feedback,
        });
    }

    let signal = client
        .trigger(TriggerRequest {
            function_id: "state::set".into(),
            payload: json!({
                "scope": scope,
                "key": phase.as_str(),
                "value": checkpoint_wake_value(phase, &head_sha),
            }),
            action: None,
            timeout_ms: Some(15_000),
        })
        .await;
    match signal {
        Ok(_) => {
            let mut evidence = runtime
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(last) = evidence.attempts.last_mut() {
                last.checkpoint_signaled = true;
            }
            Ok(HookVerdict {
                decision: "continue".into(),
                reason: None,
            })
        }
        Err(error) => {
            runtime
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .infrastructure_errors
                .push(format!(
                    "signal accepted {} checkpoint: {error}",
                    phase.as_str()
                ));
            Ok(HookVerdict {
                decision: "deny".into(),
                reason: Some(format!(
                    "VALIDATOR: the {} checkpoint passed, but trusted wake signaling failed; leave the worktree unchanged and retry.",
                    phase.as_str()
                )),
            })
        }
    }
}

fn checkpoint_wake_value(phase: HandoffPhase, head_sha: &str) -> Value {
    json!({ "phase": phase.as_str(), "head_sha": head_sha })
}

async fn audit_plan_checkpoint(
    task: &'static TaskCase,
    root: &Path,
    baseline: &BaselineRecord,
    baseline_refs: &str,
    attempt: u32,
) -> Result<HandoffAttemptRecord> {
    let base_sha = baseline.fixture_head.clone();
    let head_sha = git(root, &["rev-parse", "HEAD"]).await?;
    let commits = commits_between(root, &base_sha, &head_sha).await?;
    let changed_paths = changed_paths_in_history(root, &base_sha, &head_sha).await?;
    let stats = diff_stats(root, &base_sha).await?;
    let status = git(root, &["status", "--porcelain=v1", "--untracked-files=all"]).await?;
    let worktree_clean = status.is_empty();
    let ancestry_valid =
        head_sha != base_sha && git_is_ancestor(root, &base_sha, &head_sha).await?;
    let no_merge_commits = git(
        root,
        &["rev-list", "--merges", &format!("{base_sha}..{head_sha}")],
    )
    .await?
    .is_empty();
    let symbolic_ref = git_optional(root, &["symbolic-ref", "-q", "HEAD"]).await?;
    let branch_unchanged = symbolic_ref == baseline.initial_symbolic_ref;
    let current_refs = refs_snapshot(root).await?;
    let refs_valid = refs_advance_only(
        baseline_refs,
        &current_refs,
        baseline.initial_symbolic_ref.as_deref(),
        &head_sha,
    );
    let plan_path = root.join(IMPLEMENTATION_PLAN_PATH);
    let plan_is_regular = regular_file_without_symlink(&plan_path);
    let plan = plan_is_regular
        .then(|| std::fs::read(&plan_path).ok())
        .flatten();
    let plan_valid = plan.as_ref().is_some_and(|bytes| {
        bytes.len() <= MAX_IMPLEMENTATION_PLAN_BYTES
            && String::from_utf8(bytes.clone()).is_ok_and(|text| !text.trim().is_empty())
    });
    let allowed_paths_only = !changed_paths.is_empty()
        && changed_paths
            .iter()
            .all(|path| path == IMPLEMENTATION_PLAN_PATH)
        && plan_is_regular;
    let protected_paths_exact =
        hash_policy_paths(root, task.protected_paths)? == baseline.protected_hashes;
    let within_budget = attempt <= u32::from(task.maximum_validation_rounds) + 1;
    let accepted = !commits.is_empty()
        && ancestry_valid
        && no_merge_commits
        && branch_unchanged
        && refs_valid
        && worktree_clean
        && plan_valid
        && allowed_paths_only
        && protected_paths_exact
        && within_budget;
    let feedback = (!accepted).then(|| {
        handoff_feedback(
            HandoffPhase::Plan,
            &[
                ("one or more committed plan changes", !commits.is_empty()),
                ("baseline ancestry", ancestry_valid),
                ("no merge commits", no_merge_commits),
                ("original branch", branch_unchanged),
                ("no additional refs", refs_valid),
                ("clean worktree", worktree_clean),
                ("non-empty UTF-8 plan within 32 KiB", plan_valid),
                ("only IMPLEMENTATION_PLAN.md changed", allowed_paths_only),
                ("protected paths exact", protected_paths_exact),
                ("validation round budget", within_budget),
            ],
        )
    });
    Ok(HandoffAttemptRecord {
        phase: HandoffPhase::Plan,
        attempt,
        base_sha,
        head_sha,
        commits,
        changed_paths,
        changed_files: stats.files,
        changed_lines: stats.lines,
        worktree_clean,
        ancestry_valid,
        no_merge_commits,
        branch_unchanged,
        refs_valid,
        plan_valid,
        plan_preserved: true,
        allowed_paths_only,
        protected_paths_exact,
        focused: None,
        hidden: Vec::new(),
        full_suite: None,
        within_budget,
        accepted,
        feedback,
        persisted_before_verdict: false,
        checkpoint_signaled: false,
    })
}

async fn audit_implementation_checkpoint(
    task: &'static TaskCase,
    root: &Path,
    baseline: &BaselineRecord,
    baseline_refs: &str,
    plan_head: &str,
    plan_sha256: &str,
    attempt: u32,
) -> Result<HandoffAttemptRecord> {
    let base_sha = plan_head.to_string();
    let head_sha = git(root, &["rev-parse", "HEAD"]).await?;
    let commits = commits_between(root, &base_sha, &head_sha).await?;
    let changed_paths = changed_paths_in_history(root, &base_sha, &head_sha).await?;
    let stats = diff_stats(root, &base_sha).await?;
    let patch = git(root, &["diff", "--no-ext-diff", &base_sha, &head_sha, "--"]).await?;
    let ancestry_valid = head_sha != base_sha
        && git_is_ancestor(root, &baseline.fixture_head, &base_sha).await?
        && git_is_ancestor(root, &base_sha, &head_sha).await?;
    let no_merge_commits = git(
        root,
        &["rev-list", "--merges", &format!("{base_sha}..{head_sha}")],
    )
    .await?
    .is_empty();
    let symbolic_ref = git_optional(root, &["symbolic-ref", "-q", "HEAD"]).await?;
    let branch_unchanged = symbolic_ref == baseline.initial_symbolic_ref;
    let current_refs = refs_snapshot(root).await?;
    let refs_valid = refs_advance_only(
        baseline_refs,
        &current_refs,
        baseline.initial_symbolic_ref.as_deref(),
        &head_sha,
    );
    let plan_preserved = std::fs::read(root.join(IMPLEMENTATION_PLAN_PATH))
        .ok()
        .is_some_and(|bytes| artifact::sha256_bytes(&bytes) == plan_sha256);
    let allowed_paths_only = !changed_paths.is_empty()
        && changed_paths.iter().all(|path| {
            path_allowed(path, task.allowed_production_paths)
                && regular_file_without_symlink(&root.join(path))
        });
    let protected_paths_exact =
        hash_policy_paths(root, task.protected_paths)? == baseline.protected_hashes;
    let focused = run_probe(root, task.public_probe_ids[0], task.focused_test).await?;
    let mut hidden = Vec::with_capacity(task.hidden_probes.len());
    for probe in task.hidden_probes {
        hidden.push(run_probe(root, probe.id, probe.command).await?);
    }
    let full_suite = run_probe(root, "full_public_suite", task.full_test).await?;
    let status = git(root, &["status", "--porcelain=v1", "--untracked-files=all"]).await?;
    let worktree_clean = status.is_empty();
    let changed_budget = stats.files <= u64::from(task.maximum_changed_files)
        && stats.lines <= u64::from(task.maximum_patch_lines);
    let within_budget = changed_budget && attempt <= u32::from(task.maximum_validation_rounds) + 1;
    let accepted = !commits.is_empty()
        && !patch.trim().is_empty()
        && ancestry_valid
        && no_merge_commits
        && branch_unchanged
        && refs_valid
        && worktree_clean
        && plan_preserved
        && allowed_paths_only
        && protected_paths_exact
        && focused.passed
        && hidden.iter().all(|probe| probe.passed)
        && full_suite.passed
        && within_budget;
    let feedback = (!accepted).then(|| {
        handoff_feedback(
            HandoffPhase::Implementation,
            &[
                (
                    "one or more committed production changes",
                    !commits.is_empty(),
                ),
                (
                    "non-empty committed production patch",
                    !patch.trim().is_empty(),
                ),
                ("accepted plan ancestry", ancestry_valid),
                ("no merge commits", no_merge_commits),
                ("original branch", branch_unchanged),
                ("no additional refs", refs_valid),
                ("clean worktree", worktree_clean),
                ("accepted plan unchanged", plan_preserved),
                ("only allowed production paths changed", allowed_paths_only),
                ("protected paths exact", protected_paths_exact),
                ("focused probe passed", focused.passed),
                (
                    "hidden probes passed",
                    hidden.iter().all(|probe| probe.passed),
                ),
                ("full public suite passed", full_suite.passed),
                ("file, line, and validation budgets", within_budget),
            ],
        )
    });
    Ok(HandoffAttemptRecord {
        phase: HandoffPhase::Implementation,
        attempt,
        base_sha,
        head_sha,
        commits,
        changed_paths,
        changed_files: stats.files,
        changed_lines: stats.lines,
        worktree_clean,
        ancestry_valid,
        no_merge_commits,
        branch_unchanged,
        refs_valid,
        plan_valid: true,
        plan_preserved,
        allowed_paths_only,
        protected_paths_exact,
        focused: Some(focused),
        hidden,
        full_suite: Some(full_suite),
        within_budget,
        accepted,
        feedback,
        persisted_before_verdict: false,
        checkpoint_signaled: false,
    })
}

fn handoff_feedback(phase: HandoffPhase, facts: &[(&str, bool)]) -> String {
    let failures = facts
        .iter()
        .filter_map(|(name, passed)| (!passed).then_some(*name))
        .collect::<Vec<_>>();
    format!(
        "VALIDATOR: {} checkpoint is incomplete: {}. Repair the observed state in this same session; the auditor will not prescribe code or plan content.",
        phase.as_str(),
        failures.join("; ")
    )
}

async fn commits_between(root: &Path, base: &str, head: &str) -> Result<Vec<String>> {
    if base == head {
        return Ok(Vec::new());
    }
    Ok(
        git(root, &["rev-list", "--reverse", &format!("{base}..{head}")])
            .await?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_string)
            .collect(),
    )
}

async fn changed_paths_in_history(root: &Path, base: &str, head: &str) -> Result<Vec<String>> {
    if base == head {
        return Ok(Vec::new());
    }
    let output = git(
        root,
        &[
            "log",
            "--format=",
            "--name-only",
            &format!("{base}..{head}"),
            "--",
        ],
    )
    .await?;
    let mut paths = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(normalize_observed_path)
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

async fn git_is_ancestor(root: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let status = Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(root)
        .stdin(Stdio::null())
        .status()
        .await
        .with_context(|| {
            format!(
                "run git merge-base --is-ancestor {ancestor} {descendant} in {}",
                root.display()
            )
        })?;
    Ok(status.success())
}

fn refs_advance_only(
    baseline: &str,
    current: &str,
    symbolic_ref: Option<&str>,
    expected_head: &str,
) -> bool {
    let Some(symbolic_ref) = symbolic_ref else {
        return false;
    };
    let parse = |snapshot: &str| {
        snapshot
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(name, sha)| (name.to_string(), sha.to_string()))
            .collect::<BTreeMap<_, _>>()
    };
    let baseline = parse(baseline);
    let current = parse(current);
    baseline.len() == current.len()
        && baseline.keys().eq(current.keys())
        && current
            .get(symbolic_ref)
            .is_some_and(|sha| sha == expected_head)
        && baseline.iter().all(|(name, sha)| {
            name == symbolic_ref || current.get(name).is_some_and(|current| current == sha)
        })
}

fn parse_refs_snapshot(snapshot: &str) -> Result<BTreeMap<String, String>> {
    snapshot
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (name, sha) = line
                .split_once(' ')
                .with_context(|| format!("invalid Git ref snapshot row: {line}"))?;
            if !name.starts_with("refs/")
                || sha.len() != 40
                || !sha.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                bail!("invalid Git ref snapshot row: {line}");
            }
            Ok((name.to_string(), sha.to_string()))
        })
        .collect()
}

async fn restore_exact_refs(root: &Path, baseline_snapshot: &str) -> Result<()> {
    let baseline = parse_refs_snapshot(baseline_snapshot)?;
    let current = parse_refs_snapshot(&refs_snapshot(root).await?)?;
    for name in current.keys().filter(|name| !baseline.contains_key(*name)) {
        git(root, &["update-ref", "-d", name]).await?;
    }
    for (name, sha) in &baseline {
        git(root, &["update-ref", name, sha]).await?;
    }
    let restored = refs_snapshot(root).await?;
    if restored != baseline_snapshot {
        bail!("Git refs differ from their exact preflight snapshot after restoration");
    }
    Ok(())
}

fn persist_handoff_attempt(path: &Path, record: &HandoffAttemptRecord) -> Result<()> {
    validate_git_handoff_evidence_dir(path)?;
    let target = path.join(format!(
        "{}-attempt-{:02}.json",
        record.phase.as_str(),
        record.attempt
    ));
    let temporary = target.with_extension("json.tmp");
    let mut rendered = serde_json::to_vec_pretty(record)?;
    rendered.push(b'\n');
    std::fs::write(&temporary, rendered)
        .with_context(|| format!("write temporary auditor evidence {}", temporary.display()))?;
    std::fs::rename(&temporary, &target)
        .with_context(|| format!("publish auditor evidence {}", target.display()))?;
    Ok(())
}

fn git_handoff_owned_evidence_dir(run_id: &str) -> Result<PathBuf> {
    let directory = std::env::temp_dir()
        .join(GIT_HANDOFF_EVIDENCE_DIR)
        .join(suffix(run_id));
    validate_git_handoff_evidence_dir(&directory)?;
    if directory.exists() {
        bail!(
            "attempt-owned Git handoff evidence directory already exists: {}",
            directory.display()
        );
    }
    Ok(directory)
}

fn validate_git_handoff_evidence_dir(path: &Path) -> Result<()> {
    let temp = std::env::temp_dir()
        .canonicalize()
        .unwrap_or_else(|_| std::env::temp_dir());
    let parent = path
        .parent()
        .context("Git handoff evidence path has no parent")?;
    let expected_parent = temp.join(GIT_HANDOFF_EVIDENCE_DIR);
    if parent != expected_parent || path.file_name().is_none() {
        bail!(
            "refusing unscoped Git handoff evidence operation at {}",
            path.display()
        );
    }
    Ok(())
}

struct GitHandoffEvidence {
    root: PathBuf,
    task: &'static TaskCase,
    ticket_contract: Value,
    baseline: BaselineRecord,
    baseline_refs: String,
    plan_head: String,
    final_head: String,
    final_refs: String,
    final_status: String,
    plan: String,
    plan_sha256: String,
    patch: String,
    attempts: Vec<HandoffAttemptRecord>,
    plan_checkpoint: GitCheckpointRecord,
    implementation_checkpoint: GitCheckpointRecord,
    planner_inspection: InspectionRecord,
    implementer_inspection: InspectionRecord,
    planner_transcript: Value,
    implementer_transcript: Value,
    planner_nudges: usize,
    implementer_nudges: usize,
    orchestration_ordered: bool,
    wakes_valid: bool,
    plan_wake_before_implementer: bool,
    git_only_handoff: bool,
    root_did_not_edit: bool,
    prohibited_effect_observed: bool,
    session_tree_exact: bool,
}

async fn collect_git_handoff_evidence(
    context: &E2eContext,
    observation: &ScenarioObservation,
    run_id: &str,
) -> Result<GitHandoffEvidence> {
    let runtime = git_handoff_runtime_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(run_id)
        .cloned()
        .context("engineering Git handoff setup evidence is unavailable")?;
    let (
        root,
        task,
        baseline,
        baseline_refs,
        plan_head,
        plan_sha256,
        attempts,
        infrastructure_errors,
    ) = {
        let evidence = runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            evidence.root.clone(),
            evidence.case,
            evidence.baseline.clone(),
            evidence.baseline_refs.clone(),
            evidence.plan_head.clone(),
            evidence.plan_sha256.clone(),
            evidence.attempts.clone(),
            evidence.infrastructure_errors.clone(),
        )
    };
    if !infrastructure_errors.is_empty() {
        bail!(
            "engineering Git handoff auditor infrastructure failure: {}",
            infrastructure_errors.join("; ")
        );
    }
    let plan_head = plan_head.context("accepted plan checkpoint is unavailable")?;
    let expected_plan_sha256 = plan_sha256.context("accepted plan digest is unavailable")?;
    let final_head = git(&root, &["rev-parse", "HEAD"]).await?;
    let final_refs = refs_snapshot(&root).await?;
    let final_status = git(
        &root,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )
    .await?;
    let plan_bytes = std::fs::read(root.join(IMPLEMENTATION_PLAN_PATH))
        .context("read accepted implementation plan")?;
    let plan_sha256 = artifact::sha256_bytes(&plan_bytes);
    if plan_sha256 != expected_plan_sha256 {
        bail!("accepted implementation plan changed before evidence capture");
    }
    let plan =
        String::from_utf8(plan_bytes).context("accepted implementation plan is not UTF-8")?;
    let patch = git(
        &root,
        &["diff", "--no-ext-diff", &plan_head, &final_head, "--"],
    )
    .await?;
    let accepted_plan = attempts
        .iter()
        .rev()
        .find(|record| record.phase == HandoffPhase::Plan && record.accepted)
        .cloned()
        .context("accepted plan attempt is unavailable")?;
    let accepted_implementation = attempts
        .iter()
        .rev()
        .find(|record| record.phase == HandoffPhase::Implementation && record.accepted)
        .cloned()
        .context("accepted implementation attempt is unavailable")?;
    let plan_checkpoint = checkpoint_from_attempt(
        &accepted_plan,
        baseline.initial_symbolic_ref.clone(),
        &final_refs_for_head(
            &baseline_refs,
            baseline.initial_symbolic_ref.as_deref(),
            &plan_head,
        ),
    );
    let implementation_checkpoint = checkpoint_from_attempt(
        &accepted_implementation,
        baseline.initial_symbolic_ref.clone(),
        &final_refs,
    );

    let planner = planner_session(run_id);
    let implementer = implementer_session(run_id);
    let planner_transcript = context.transcript(&planner).await.unwrap_or(Value::Null);
    let implementer_transcript = context
        .transcript(&implementer)
        .await
        .unwrap_or(Value::Null);
    let planner_inspection = inspection_record(task, &root, &planner_transcript);
    let implementer_inspection = inspection_record(task, &root, &implementer_transcript);
    let planner_nudges = nudge_texts(&planner_transcript).len();
    let implementer_nudges = nudge_texts(&implementer_transcript).len();

    let root_calls = common::function_calls(&observation.transcript);
    let orchestration_ordered = handoff_orchestration_ordered(&root_calls, run_id);
    let wakes_valid = handoff_wakes_valid(&observation.transcript);
    let plan_wake_before_implementer =
        plan_wake_precedes_implementer_spawn(&observation.transcript, &implementer);
    let implementation_spawn_call = root_calls.iter().find(|call| {
        call.function_id == "harness::spawn"
            && call.arguments.get("session_id").and_then(Value::as_str)
                == Some(implementer.as_str())
    });
    let git_only_handoff =
        implementation_spawn_call.is_some_and(|call| implementation_spawn_is_git_only(call, &plan));
    let root_did_not_edit = root_calls.iter().all(root_call_is_coordination_only);
    let prohibited_effect_observed = root_calls
        .iter()
        .chain(common::function_calls(&planner_transcript).iter())
        .chain(common::function_calls(&implementer_transcript).iter())
        .any(prohibited_effect);
    let expected_sessions = [
        observation.metrics.root_session_id.as_str(),
        planner.as_str(),
        implementer.as_str(),
    ]
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>();
    let observed_sessions = observation
        .metrics
        .by_session
        .iter()
        .map(|session| session.session_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let session_tree_exact = observation.metrics.complete && observed_sessions == expected_sessions;

    Ok(GitHandoffEvidence {
        root,
        task,
        ticket_contract: observation.case.inputs.clone(),
        baseline,
        baseline_refs,
        plan_head,
        final_head,
        final_refs,
        final_status,
        plan,
        plan_sha256,
        patch,
        attempts,
        plan_checkpoint,
        implementation_checkpoint,
        planner_inspection,
        implementer_inspection,
        planner_transcript,
        implementer_transcript,
        planner_nudges,
        implementer_nudges,
        orchestration_ordered,
        wakes_valid,
        plan_wake_before_implementer,
        git_only_handoff,
        root_did_not_edit,
        prohibited_effect_observed,
        session_tree_exact,
    })
}

fn checkpoint_from_attempt(
    attempt: &HandoffAttemptRecord,
    symbolic_ref: Option<String>,
    refs: &str,
) -> GitCheckpointRecord {
    GitCheckpointRecord {
        phase: attempt.phase,
        base_sha: attempt.base_sha.clone(),
        head_sha: attempt.head_sha.clone(),
        commits: attempt.commits.clone(),
        changed_paths: attempt.changed_paths.clone(),
        symbolic_ref,
        refs_sha256: artifact::sha256_bytes(refs.as_bytes()),
        clean: attempt.worktree_clean,
        accepted_attempt: attempt.attempt,
    }
}

fn final_refs_for_head(baseline_refs: &str, symbolic_ref: Option<&str>, head: &str) -> String {
    let Some(symbolic_ref) = symbolic_ref else {
        return baseline_refs.to_string();
    };
    baseline_refs
        .lines()
        .map(|line| {
            line.split_once(' ').map_or_else(
                || line.to_string(),
                |(name, sha)| {
                    if name == symbolic_ref {
                        format!("{name} {head}")
                    } else {
                        format!("{name} {sha}")
                    }
                },
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn handoff_orchestration_ordered(calls: &[common::ObservedFunctionCall], run_id: &str) -> bool {
    let planner = planner_session(run_id);
    let implementer = implementer_session(run_id);
    let scope = handoff_state_scope(run_id);
    let plan_validator = calls.iter().position(|call| {
        scoped_validator_registration(call, &plan_auditor_function_id(run_id), &planner)
    });
    let plan_wake = calls
        .iter()
        .position(|call| state_wake_registration(call, &scope, HandoffPhase::Plan.as_str()));
    let planner_spawn = calls.iter().position(|call| {
        call.function_id == "harness::spawn"
            && call.arguments.get("session_id").and_then(Value::as_str) == Some(planner.as_str())
    });
    let implementation_validator = calls.iter().position(|call| {
        scoped_validator_registration(
            call,
            &implementation_auditor_function_id(run_id),
            &implementer,
        )
    });
    let implementation_wake = calls.iter().position(|call| {
        state_wake_registration(call, &scope, HandoffPhase::Implementation.as_str())
    });
    let implementer_spawn = calls.iter().position(|call| {
        call.function_id == "harness::spawn"
            && call.arguments.get("session_id").and_then(Value::as_str)
                == Some(implementer.as_str())
    });
    let unregisters = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| trigger_unregistration(call))
        .map(|(position, _)| position)
        .collect::<Vec<_>>();
    matches!(
        (
            plan_validator,
            plan_wake,
            planner_spawn,
            unregisters.first().copied(),
            implementation_validator,
            implementation_wake,
            implementer_spawn,
            unregisters.get(1).copied(),
        ),
        (Some(pv), Some(pw), Some(ps), Some(pu), Some(iv), Some(iw), Some(is), Some(iu))
            if pv < ps && pw < ps && ps < pu && pu < iv && iv < is && iw < is && is < iu
    ) && calls
        .iter()
        .filter(|call| call.function_id == "harness::spawn")
        .count()
        == 2
        && unregisters.len() == 2
        && calls
            .iter()
            .filter(|call| call.function_id == "engine::unregister_trigger")
            .count()
            == 2
}

fn implementation_spawn_is_git_only(call: &common::ObservedFunctionCall, plan: &str) -> bool {
    call.function_id == "harness::spawn"
        && spawn_task_text(&call.arguments) == Some(IMPLEMENTER_TASK)
        && call.arguments.pointer("/options/filesystem_root").is_none()
        && call.arguments.get("filesystem_root").is_none()
        && call.arguments.get("model").is_none()
        && call.arguments.get("provider").is_none()
        && !call.arguments.to_string().contains(plan)
}

fn root_call_is_coordination_only(call: &common::ObservedFunctionCall) -> bool {
    call.function_id != "shell::exec" && !call.function_id.starts_with("coder::")
}

fn trigger_unregistration(call: &common::ObservedFunctionCall) -> bool {
    call.function_id == "engine::unregister_trigger"
        && call
            .arguments
            .get("subscription_id")
            .or_else(|| call.arguments.get("id"))
            .and_then(Value::as_str)
            .is_some_and(|subscription_id| !subscription_id.trim().is_empty())
}

fn handoff_wakes_valid(transcript: &Value) -> bool {
    [
        "engineering-plan-accepted",
        "engineering-implementation-accepted",
    ]
    .into_iter()
    .all(|label| {
        let records = common::trigger_fired_records(transcript)
            .into_iter()
            .filter(|record| record.get("label").and_then(Value::as_str) == Some(label))
            .collect::<Vec<_>>();
        records.len() == 1
            && records[0].get("retired").and_then(Value::as_bool) == Some(true)
            && records[0].get("once").and_then(Value::as_bool) == Some(true)
            && records[0].get("target").and_then(Value::as_str) == Some("harness::send")
    })
}

fn plan_wake_precedes_implementer_spawn(transcript: &Value, implementer: &str) -> bool {
    let mut plan_wake = None;
    let mut implementer_spawn = None;
    for (position, entry) in transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        if entry.pointer("/custom/custom_type").and_then(Value::as_str) == Some("trigger_fired")
            && entry.pointer("/custom/data/label").and_then(Value::as_str)
                == Some("engineering-plan-accepted")
        {
            plan_wake.get_or_insert(position);
        }
        let spawned_here = entry
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|block| {
                if block.get("type").and_then(Value::as_str) != Some("function_call") {
                    return false;
                }
                let function_id = block.get("function_id").and_then(Value::as_str);
                let Some(arguments) = block.get("arguments") else {
                    return false;
                };
                if function_id == Some("harness::spawn") {
                    return arguments.get("session_id").and_then(Value::as_str)
                        == Some(implementer);
                }
                function_id == Some("agent_trigger")
                    && arguments.get("function").and_then(Value::as_str) == Some("harness::spawn")
                    && arguments
                        .pointer("/payload/session_id")
                        .and_then(Value::as_str)
                        == Some(implementer)
            });
        if spawned_here {
            implementer_spawn.get_or_insert(position);
        }
    }
    matches!((plan_wake, implementer_spawn), (Some(wake), Some(spawn)) if wake < spawn)
}

fn scoped_validator_registration(
    call: &common::ObservedFunctionCall,
    function_id: &str,
    session_id: &str,
) -> bool {
    call.function_id == "engine::register_trigger"
        && call.arguments.get("trigger_type").and_then(Value::as_str) == Some(HOOK_TYPE)
        && call.arguments.get("function_id").and_then(Value::as_str) == Some(function_id)
        && call
            .arguments
            .pointer("/config/sessions/0")
            .and_then(Value::as_str)
            == Some(session_id)
}

fn state_wake_registration(call: &common::ObservedFunctionCall, scope: &str, key: &str) -> bool {
    call.function_id == "engine::register_trigger"
        && call.arguments.get("trigger_type").and_then(Value::as_str) == Some("state")
        && common::is_wake_registration(&call.arguments)
        && common::requested_once(&call.arguments)
        && call
            .arguments
            .pointer("/config/scope")
            .and_then(Value::as_str)
            == Some(scope)
        && call
            .arguments
            .pointer("/config/key")
            .and_then(Value::as_str)
            == Some(key)
        && call
            .arguments
            .pointer("/lifecycle/expires_at")
            .and_then(Value::as_u64)
            .is_some()
}

fn spawn_task_text(arguments: &Value) -> Option<&str> {
    arguments
        .get("task")
        .and_then(Value::as_str)
        .or_else(|| arguments.pointer("/task/text").and_then(Value::as_str))
}

fn git_handoff_evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let evidence = collect_git_handoff_evidence(context, observation, run_id).await?;
        evaluate_git_handoff_evidence(&evidence, observation).await
    })
}

async fn evaluate_git_handoff_evidence(
    evidence: &GitHandoffEvidence,
    _observation: &ScenarioObservation,
) -> Result<super::ObjectiveEvaluation> {
    let accepted_plan = evidence
        .attempts
        .iter()
        .rev()
        .find(|record| record.phase == HandoffPhase::Plan && record.accepted);
    let accepted_implementation = evidence
        .attempts
        .iter()
        .rev()
        .find(|record| record.phase == HandoffPhase::Implementation && record.accepted);
    let plan_accepted = accepted_plan.is_some_and(|record| record.checkpoint_signaled);
    let implementation_accepted =
        accepted_implementation.is_some_and(|record| record.checkpoint_signaled);
    let plan_ancestor =
        git_is_ancestor(&evidence.root, &evidence.plan_head, &evidence.final_head).await?;
    let planner_reproduced = evidence
        .planner_inspection
        .baseline_reproduction_call
        .is_some();
    let implementation_tests_observed = evidence.implementer_inspection.focused_command_count > 0
        && evidence.implementer_inspection.full_suite_command_count > 0;
    let latest = accepted_implementation;
    let focused_passed = latest
        .and_then(|record| record.focused.as_ref())
        .is_some_and(|p| p.passed);
    let hidden_passed = latest.is_some_and(|record| record.hidden.iter().all(|p| p.passed));
    let full_passed = latest
        .and_then(|record| record.full_suite.as_ref())
        .is_some_and(|p| p.passed);
    let attempts_persisted = !evidence.attempts.is_empty()
        && evidence
            .attempts
            .iter()
            .all(|attempt| attempt.persisted_before_verdict);
    let repair_budget = evidence.planner_nudges
        <= usize::from(evidence.task.maximum_validation_rounds)
        && evidence.implementer_nudges <= usize::from(evidence.task.maximum_validation_rounds);
    let same_session_repairs = repair_budget;
    let planner_first_pass = accepted_plan.is_some_and(|record| record.attempt == 1);
    let implementer_first_pass = accepted_implementation.is_some_and(|record| record.attempt == 1);
    let no_auditor_nudges = evidence.planner_nudges == 0 && evidence.implementer_nudges == 0;
    let convergence_points = u8::from(planner_first_pass) * 2
        + u8::from(implementer_first_pass) * 2
        + u8::from(no_auditor_nudges);
    let git_integrity = plan_accepted
        && implementation_accepted
        && plan_ancestor
        && accepted_plan.is_some_and(|record| {
            record.worktree_clean
                && record.branch_unchanged
                && record.refs_valid
                && record.no_merge_commits
                && record.plan_valid
                && record.allowed_paths_only
        })
        && accepted_implementation.is_some_and(|record| {
            record.worktree_clean
                && record.branch_unchanged
                && record.refs_valid
                && record.no_merge_commits
                && record.plan_preserved
                && record.allowed_paths_only
        });
    let orchestration = evidence.orchestration_ordered
        && evidence.wakes_valid
        && evidence.plan_wake_before_implementer
        && evidence.git_only_handoff
        && evidence.root_did_not_edit
        && planner_reproduced
        && implementation_tests_observed;
    let ticket_acceptance =
        implementation_accepted && focused_passed && hidden_passed && full_passed;
    let scope = accepted_implementation.is_some_and(|record| {
        record.protected_paths_exact && record.within_budget && record.allowed_paths_only
    }) && evidence.final_status.is_empty()
        && !evidence.prohibited_effect_observed
        && evidence.session_tree_exact
        && attempts_persisted
        && same_session_repairs;

    let mut evaluation = assessment::build_evaluation([
        HANDOFF_ORCHESTRATION.full_or_zero(
            orchestration,
            format!(
                "ordered={}, wakes_valid={}, plan_wake_before_implementer={}, git_only={}, root_no_edit={}, planner_red={}, implementer_tests={}",
                evidence.orchestration_ordered,
                evidence.wakes_valid,
                evidence.plan_wake_before_implementer,
                evidence.git_only_handoff,
                evidence.root_did_not_edit,
                planner_reproduced,
                implementation_tests_observed
            ),
        ),
        HANDOFF_GIT_INTEGRITY.full_or_zero(
            git_integrity,
            format!(
                "plan_accepted={plan_accepted}, implementation_accepted={implementation_accepted}, plan_ancestor={plan_ancestor}"
            ),
        ),
        HANDOFF_TICKET_ACCEPTANCE.full_or_zero(
            ticket_acceptance,
            format!(
                "focused={focused_passed}, hidden={hidden_passed}, full={full_passed}"
            ),
        ),
        HANDOFF_SCOPE_AND_LIFECYCLE.full_or_zero(
            scope,
            format!(
                "protected={}, clean={}, prohibited={}, sessions={}, persisted={}, repair_budget={}",
                latest.is_some_and(|record| record.protected_paths_exact),
                evidence.final_status.is_empty(),
                evidence.prohibited_effect_observed,
                evidence.session_tree_exact,
                attempts_persisted,
                repair_budget
            ),
        ),
        HANDOFF_PAIRED_EFFICIENCY.award(
            HANDOFF_PAIRED_EFFICIENCY.weight(),
            "pending suite-level comparison with the matching engineering_ticket baseline",
        )?,
        HANDOFF_CONVERGENCE.award(
            convergence_points,
            format!(
                "planner_first_pass={planner_first_pass}, implementer_first_pass={implementer_first_pass}, no_auditor_nudges={no_auditor_nudges}"
            ),
        )?,
    ]);

    let original_branch_preserved = accepted_plan.is_some_and(|record| record.branch_unchanged)
        && latest.is_some_and(|record| record.branch_unchanged);
    let no_merges = accepted_plan.is_some_and(|record| record.no_merge_commits)
        && latest.is_some_and(|record| record.no_merge_commits);
    let refs_valid = accepted_plan.is_some_and(|record| record.refs_valid)
        && latest.is_some_and(|record| record.refs_valid);
    let clean_checkpoints = accepted_plan.is_some_and(|record| record.worktree_clean)
        && latest.is_some_and(|record| record.worktree_clean);
    let plan_preserved = latest.is_some_and(|record| record.plan_preserved);
    let allowed_paths = latest.is_some_and(|record| record.allowed_paths_only);
    let protected = latest.is_some_and(|record| record.protected_paths_exact);
    let patch_budget = latest.is_some_and(|record| record.within_budget);
    for (id, passed) in [
        (
            "fixture_identity_exact",
            evidence.baseline.fixture_head == evidence.task.fixture_revision
                && evidence.baseline.fixture_manifest_sha256
                    == evidence.task.fixture_manifest_sha256,
        ),
        (
            "red_baseline_verified_by_runner",
            evidence.baseline.expected_failure_observed,
        ),
        ("planner_checkpoint_accepted", plan_accepted),
        (
            "implementation_checkpoint_accepted",
            implementation_accepted,
        ),
        ("plan_checkpoint_ancestor", plan_ancestor),
        ("original_branch_preserved", original_branch_preserved),
        ("no_merge_commits", no_merges),
        ("no_additional_refs", refs_valid),
        ("worktree_clean_at_checkpoints", clean_checkpoints),
        ("implementation_plan_preserved", plan_preserved),
        ("git_only_handoff", evidence.git_only_handoff),
        ("root_did_not_edit", evidence.root_did_not_edit),
        ("planner_reproduced_baseline", planner_reproduced),
        (
            "implementation_tests_observed",
            implementation_tests_observed,
        ),
        ("focused_test_passed", focused_passed),
        ("hidden_semantic_cases_passed", hidden_passed),
        ("full_suite_passed", full_passed),
        ("allowed_paths_only", allowed_paths),
        ("protected_paths_exact", protected),
        ("patch_budget_passed", patch_budget),
        ("attempts_persisted_before_verdict", attempts_persisted),
        ("same_session_repairs", same_session_repairs),
        (
            "no_prohibited_effects",
            !evidence.prohibited_effect_observed,
        ),
        ("three_session_tree_terminal", evidence.session_tree_exact),
    ] {
        evaluation.hard_gates.push(common::gate(
            id,
            passed,
            format!("deterministic Git handoff evidence: {id}={passed}"),
        ));
    }
    Ok(evaluation)
}

#[derive(Debug, Clone, Copy)]
struct EfficiencySnapshot {
    wall_time_ms: u64,
    turns: u64,
    function_calls: u64,
    total_tokens: u64,
    work_amplification: f64,
}

#[derive(Debug)]
struct PairedEfficiencyScore {
    awarded: u8,
    reason: String,
}

/// Re-scores the advisory efficiency criterion after all scenarios have run.
/// The comparison is suite-local, so model, stack and execution environment
/// are identical while seed, task case and repetition align the samples.
/// Hard gates and run status are deliberately left untouched.
pub(crate) fn apply_paired_efficiency(scenarios: &mut [E2eScenarioReport]) {
    let mut baselines = HashMap::<(u64, String), Vec<Option<EfficiencySnapshot>>>::new();
    for scenario in scenarios
        .iter()
        .filter(|scenario| scenario.scenario_id == ID && scenario.scenario_version == VERSION)
    {
        let Some(case) = scenario.case.as_ref() else {
            continue;
        };
        let Some(task_case_id) = case.inputs.get("task_case_id").and_then(Value::as_str) else {
            continue;
        };
        baselines.insert(
            (case.seed, task_case_id.to_string()),
            scenario
                .runs
                .iter()
                .map(|run| {
                    (run.status == RunStatus::Passed)
                        .then(|| efficiency_snapshot(run))
                        .flatten()
                })
                .collect(),
        );
    }

    for scenario in scenarios.iter_mut().filter(|scenario| {
        scenario.scenario_id == GIT_HANDOFF_ID && scenario.scenario_version == GIT_HANDOFF_VERSION
    }) {
        let comparison_key = scenario.case.as_ref().and_then(|case| {
            case.inputs
                .get("task_case_id")
                .and_then(Value::as_str)
                .map(|task_case_id| (case.seed, task_case_id.to_string()))
        });
        let paired_baselines = comparison_key.as_ref().and_then(|key| baselines.get(key));

        for (index, run) in scenario.runs.iter_mut().enumerate() {
            let outcome = paired_baselines
                .and_then(|runs| runs.get(index))
                .and_then(|baseline| *baseline)
                .and_then(|baseline| {
                    efficiency_snapshot(run)
                        .and_then(|handoff| paired_efficiency_score(baseline, handoff))
                });
            let unavailable_reason = match (comparison_key.as_ref(), paired_baselines) {
                (None, _) => {
                    "paired efficiency unavailable: handoff task-case identity is missing"
                }
                (Some(_), None) => {
                    "paired efficiency unavailable: matching engineering_ticket baseline was not executed in this suite"
                }
                (Some(_), Some(runs)) if runs.get(index).is_none() => {
                    "paired efficiency unavailable: matching baseline repetition is missing"
                }
                (Some(_), Some(runs)) if runs[index].is_none() => {
                    "paired efficiency unavailable: matching baseline did not pass with complete efficiency metrics"
                }
                _ => {
                    "paired efficiency unavailable: handoff efficiency metrics are incomplete or invalid"
                }
            };
            apply_paired_efficiency_to_run(run, outcome, unavailable_reason);
        }

        let Some(case) = scenario.case.clone() else {
            continue;
        };
        let execution_policy = scenario.execution_policy;
        let runs = std::mem::take(&mut scenario.runs);
        *scenario = E2eScenarioReport::aggregate_case(case, execution_policy, runs);
    }
}

fn efficiency_snapshot(run: &E2eRunReport) -> Option<EfficiencySnapshot> {
    efficiency_snapshot_from_report(run.efficiency.as_ref()?)
}

fn efficiency_snapshot_from_report(efficiency: &EfficiencyReport) -> Option<EfficiencySnapshot> {
    let turns = efficiency
        .root_turns?
        .checked_add(efficiency.child_turns?)?;
    let snapshot = EfficiencySnapshot {
        wall_time_ms: efficiency.wall_time_ms,
        turns,
        function_calls: efficiency.function_calls?,
        total_tokens: efficiency.total_tokens?,
        work_amplification: efficiency.work_amplification?,
    };
    (snapshot.wall_time_ms > 0
        && snapshot.turns > 0
        && snapshot.function_calls > 0
        && snapshot.total_tokens > 0
        && snapshot.work_amplification.is_finite()
        && snapshot.work_amplification > 0.0)
        .then_some(snapshot)
}

fn paired_efficiency_score(
    baseline: EfficiencySnapshot,
    handoff: EfficiencySnapshot,
) -> Option<PairedEfficiencyScore> {
    let ratios = [
        (
            "tokens",
            handoff.total_tokens as f64 / baseline.total_tokens as f64,
            6,
        ),
        ("turns", handoff.turns as f64 / baseline.turns as f64, 3),
        (
            "calls",
            handoff.function_calls as f64 / baseline.function_calls as f64,
            2,
        ),
        (
            "wall_time",
            handoff.wall_time_ms as f64 / baseline.wall_time_ms as f64,
            2,
        ),
        (
            "work_amplification",
            handoff.work_amplification / baseline.work_amplification,
            2,
        ),
    ];
    if ratios.iter().any(|(_, ratio, _)| !ratio.is_finite()) {
        return None;
    }
    let components = ratios
        .iter()
        .map(|(name, ratio, possible)| (*name, *ratio, ratio_points(*ratio, *possible), *possible))
        .collect::<Vec<_>>();
    let awarded = components.iter().map(|(_, _, awarded, _)| *awarded).sum();
    let reason = format!(
        "paired efficiency v1 against engineering_ticket: {}; bands are <=1.25x full, <=1.50x 75%, <=1.75x 50%, <=2.00x 25%, >2.00x zero",
        components
            .iter()
            .map(|(name, ratio, awarded, possible)| {
                format!("{name}={ratio:.3}x ({awarded}/{possible})")
            })
            .collect::<Vec<_>>()
            .join(", ")
    );
    Some(PairedEfficiencyScore { awarded, reason })
}

fn ratio_points(ratio: f64, possible: u8) -> u8 {
    let quartiles = if ratio <= 1.25 {
        4
    } else if ratio <= 1.50 {
        3
    } else if ratio <= 1.75 {
        2
    } else if ratio <= 2.00 {
        1
    } else {
        0
    };
    (u16::from(possible) * quartiles + 2)
        .checked_div(4)
        .and_then(|points| u8::try_from(points).ok())
        .unwrap_or(0)
}

fn apply_paired_efficiency_to_run(
    run: &mut E2eRunReport,
    outcome: Option<PairedEfficiencyScore>,
    unavailable_reason: &str,
) {
    let Some(criterion) = run
        .criteria
        .iter_mut()
        .find(|criterion| criterion.id == HANDOFF_PAIRED_EFFICIENCY.id())
    else {
        return;
    };
    let possible = criterion.possible;
    let assessment = run
        .assessment_results
        .iter_mut()
        .find(|assessment| assessment.criterion_id == HANDOFF_PAIRED_EFFICIENCY.id());
    match outcome {
        Some(outcome) => {
            criterion.awarded = Some(outcome.awarded);
            criterion.reason.clone_from(&outcome.reason);
            if let Some(assessment) = assessment {
                assessment.outcome = if outcome.awarded == possible {
                    AssessmentOutcome::Passed
                } else if outcome.awarded == 0 {
                    AssessmentOutcome::Failed
                } else {
                    AssessmentOutcome::Partial
                };
                assessment.score = Some(AssessmentScore {
                    awarded: outcome.awarded,
                    possible,
                });
                assessment.summary = outcome.reason;
            }
        }
        None => {
            criterion.awarded = None;
            criterion.reason = unavailable_reason.to_string();
            if let Some(assessment) = assessment {
                assessment.outcome = AssessmentOutcome::Unavailable;
                assessment.score = None;
                assessment.summary = unavailable_reason.to_string();
            }
        }
    }
    run.score = run.criteria.iter().try_fold(0_u8, |score, criterion| {
        criterion
            .awarded
            .and_then(|awarded| score.checked_add(awarded))
    });
    run.refresh_dimensions(!run.deliverables.is_empty());
}

fn git_handoff_capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let evidence = collect_git_handoff_evidence(context, observation, run_id).await?;
        let evaluation = evaluate_git_handoff_evidence(&evidence, observation).await?;
        let invariants = super::captured_gate_invariants(evaluation);
        let root_provenance = ProvenanceEvidence {
            kind: "session".into(),
            source_id: observation.metrics.root_session_id.clone(),
            relation: "orchestrated_git_handoff".into(),
        };
        let planner_provenance = ProvenanceEvidence {
            kind: "session".into(),
            source_id: planner_session(run_id),
            relation: "planned_and_committed".into(),
        };
        let implementer_provenance = ProvenanceEvidence {
            kind: "session".into(),
            source_id: implementer_session(run_id),
            relation: "implemented_and_committed".into(),
        };
        let initial_refs_sha256 = artifact::sha256_bytes(evidence.baseline_refs.as_bytes());
        let final_refs_sha256 = artifact::sha256_bytes(evidence.final_refs.as_bytes());
        let plan_refs = final_refs_for_head(
            &evidence.baseline_refs,
            evidence.baseline.initial_symbolic_ref.as_deref(),
            &evidence.plan_head,
        );
        let plan_refs_sha256 = artifact::sha256_bytes(plan_refs.as_bytes());
        let implementation_attempts = evidence
            .attempts
            .iter()
            .filter(|attempt| attempt.phase == HandoffPhase::Implementation)
            .count();
        let plan_attempts = evidence
            .attempts
            .len()
            .saturating_sub(implementation_attempts);
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
                json!({
                    "planner": evidence.planner_inspection,
                    "implementer": evidence.implementer_inspection,
                }),
                vec![],
                vec![planner_provenance.clone(), implementer_provenance.clone()],
            ),
            CapturedDeliverable {
                id: "implementation_plan".into(),
                kind: "implementation_plan".into(),
                content: CapturedDeliverableContent::TextUtf8(evidence.plan.clone()),
                invariants: vec![],
                provenance: vec![ProvenanceEvidence {
                    kind: "git_blob".into(),
                    source_id: evidence.plan_sha256.clone(),
                    relation: "committed_at_plan_checkpoint".into(),
                }],
            },
            json_deliverable(
                "git_checkpoints",
                "git_checkpoints",
                json!({
                    "c0": evidence.baseline.fixture_head,
                    "cplan": evidence.plan_checkpoint,
                    "cfinal": evidence.implementation_checkpoint,
                    "branch": evidence.baseline.initial_symbolic_ref,
                    "ranges": {
                        "plan": format!("{}..{}", evidence.baseline.fixture_head, evidence.plan_head),
                        "implementation": format!("{}..{}", evidence.plan_head, evidence.final_head),
                    },
                    "refs": {
                        "c0": evidence.baseline_refs,
                        "cplan": plan_refs,
                        "cfinal": evidence.final_refs,
                        "c0_sha256": initial_refs_sha256,
                        "cplan_sha256": plan_refs_sha256,
                        "cfinal_sha256": final_refs_sha256,
                    },
                }),
                vec![],
                vec![planner_provenance.clone(), implementer_provenance.clone()],
            ),
            CapturedDeliverable {
                id: "candidate_patch".into(),
                kind: "code_patch".into(),
                content: CapturedDeliverableContent::TextUtf8(evidence.patch.clone()),
                invariants: vec![],
                provenance: vec![ProvenanceEvidence {
                    kind: "git_diff".into(),
                    source_id: format!("{}..{}", evidence.plan_head, evidence.final_head),
                    relation: "committed_implementation_patch".into(),
                }],
            },
            json_deliverable(
                "change_manifest",
                "change_manifest",
                json!({
                    "initial_revision": evidence.baseline.fixture_head,
                    "plan_revision": evidence.plan_head,
                    "final_revision": evidence.final_head,
                    "plan_sha256": evidence.plan_sha256,
                    "patch_sha256": artifact::sha256_bytes(evidence.patch.as_bytes()),
                    "final_status": evidence.final_status,
                    "original_symbolic_ref": evidence.baseline.initial_symbolic_ref,
                    "initial_refs_sha256": initial_refs_sha256,
                    "final_refs_sha256": final_refs_sha256,
                }),
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
                    source_id: format!(
                        "{},{}",
                        plan_auditor_function_id(run_id),
                        implementation_auditor_function_id(run_id)
                    ),
                    relation: "persisted_phase_verdicts".into(),
                }],
            ),
            json_deliverable(
                "repair_timeline",
                "repair_timeline",
                json!({
                    "plan": { "attempts": plan_attempts, "nudges": evidence.planner_nudges, "session_id": planner_session(run_id) },
                    "implementation": { "attempts": implementation_attempts, "nudges": evidence.implementer_nudges, "session_id": implementer_session(run_id) },
                }),
                vec![],
                vec![planner_provenance.clone(), implementer_provenance.clone()],
            ),
            json_deliverable(
                "engineering_report",
                "engineering_report",
                json!({
                    "response": observation.response,
                    "task_case_id": evidence.task.id,
                    "root_session_id": observation.metrics.root_session_id,
                    "planner_session_id": planner_session(run_id),
                    "implementer_session_id": implementer_session(run_id),
                    "topology": {
                        "nodes": [
                            { "session_id": observation.metrics.root_session_id, "role": "root_orchestrator" },
                            { "session_id": planner_session(run_id), "role": "planner_leaf" },
                            { "session_id": implementer_session(run_id), "role": "implementer_leaf" },
                        ],
                        "edges": [
                            { "from": observation.metrics.root_session_id, "to": planner_session(run_id), "handoff": "ticket_and_repository" },
                            { "from": planner_session(run_id), "to": implementer_session(run_id), "handoff": "git_checkpoint_only" },
                        ],
                        "phase_order": ["plan", "implementation"],
                    },
                    "session_tree_exact": evidence.session_tree_exact,
                    "orchestration_ordered": evidence.orchestration_ordered,
                    "wakes_valid": evidence.wakes_valid,
                    "plan_wake_before_implementer": evidence.plan_wake_before_implementer,
                    "git_only_handoff": evidence.git_only_handoff,
                    "planner_transcript_observed": !evidence.planner_transcript.is_null(),
                    "implementer_transcript_observed": !evidence.implementer_transcript.is_null(),
                }),
                invariants,
                vec![root_provenance, planner_provenance, implementer_provenance],
            ),
        ])
    })
}

fn git_handoff_deliverable_contract() -> DeliverableContract {
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
                id: "implementation_plan".into(),
                kind: "implementation_plan".into(),
                media_type: "text/markdown; charset=utf-8".into(),
                schema: json!({}),
                max_size_bytes: MAX_IMPLEMENTATION_PLAN_BYTES as u64,
            },
            artifact_expectation("git_checkpoints", "git_checkpoints", object_schema.clone()),
            ArtifactExpectation {
                id: "candidate_patch".into(),
                kind: "code_patch".into(),
                media_type: "text/x-diff; charset=utf-8".into(),
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
        invariants: GIT_HANDOFF_REQUIRED_ASSESSMENTS
            .iter()
            .map(|assessment| InvariantSpec {
                id: assessment.id().into(),
                description: assessment.description().into(),
            })
            .chain(
                GIT_HANDOFF_GRANULAR_GATES
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

fn git_handoff_cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let mut state_cleanup_errors = Vec::new();
        for key in [
            HandoffPhase::Plan.as_str(),
            HandoffPhase::Implementation.as_str(),
        ] {
            let deletion: Result<Value> = context
                .trigger(
                    "state::delete",
                    json!({ "scope": handoff_state_scope(run_id), "key": key }),
                )
                .await;
            if let Err(error) = deletion {
                state_cleanup_errors.push(format!("delete handoff state key {key}: {error:#}"));
            }
        }
        let runtime = git_handoff_runtime_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(run_id);
        let Some(runtime) = runtime else {
            if !state_cleanup_errors.is_empty() {
                bail!("{}", state_cleanup_errors.join("; "));
            }
            return Ok(());
        };
        let (root, baseline, baseline_refs, evidence_dir) = {
            let evidence = runtime
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                evidence.root.clone(),
                evidence.baseline.clone(),
                evidence.baseline_refs.clone(),
                evidence.evidence_dir.clone(),
            )
        };
        validate_fixture_root(&root)?;
        if let Some(reference) = baseline.initial_symbolic_ref.as_deref() {
            git(&root, &["update-ref", reference, &baseline.fixture_head]).await?;
            git(&root, &["symbolic-ref", "HEAD", reference]).await?;
        } else {
            git(
                &root,
                &["checkout", "--detach", "-f", &baseline.fixture_head],
            )
            .await?;
        }
        git(&root, &["reset", "--hard", &baseline.fixture_head]).await?;
        git(&root, &["clean", "-fd"]).await?;
        restore_exact_refs(&root, &baseline_refs).await?;
        let status = git(
            &root,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )
        .await?;
        let head = git(&root, &["rev-parse", "HEAD"]).await?;
        let refs = refs_snapshot(&root).await?;
        let symbolic_ref = git_optional(&root, &["symbolic-ref", "-q", "HEAD"]).await?;
        if !status.is_empty()
            || head != baseline.fixture_head
            || refs != baseline_refs
            || symbolic_ref != baseline.initial_symbolic_ref
        {
            bail!(
                "engineering Git handoff cleanup did not restore exact HEAD, branch, refs, and status"
            );
        }
        validate_git_handoff_evidence_dir(&evidence_dir)?;
        if evidence_dir.exists() {
            std::fs::remove_dir_all(&evidence_dir).with_context(|| {
                format!("remove Git handoff evidence {}", evidence_dir.display())
            })?;
        }
        if !state_cleanup_errors.is_empty() {
            bail!("{}", state_cleanup_errors.join("; "));
        }
        Ok(())
    })
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

    #[test]
    fn engineering_ticket_v2_remains_the_single_session_baseline() {
        let baseline = scenario("regression");
        let materialized = materialize("regression", CANONICAL_SEED).unwrap();
        assert_eq!(baseline.id, ID);
        assert_eq!(baseline.version, VERSION);
        assert_eq!(VERSION, 2);
        assert!(!baseline.prompt.contains("harness::spawn"));
        assert!(!materialized
            .case
            .required_capabilities
            .contains(&"e2e::subagents".to_string()));
        assert_eq!(materialized.case.deliverable_contract.artifacts.len(), 8);
    }

    #[test]
    fn git_handoff_materializes_a_distinct_ten_asset_contract() {
        let materialized = git_handoff_materialize("catalog", 42).unwrap();
        assert_eq!(materialized.spec.id, GIT_HANDOFF_ID);
        assert_eq!(materialized.spec.version, GIT_HANDOFF_VERSION);
        assert_eq!(GIT_HANDOFF_VERSION, 2);
        assert_eq!(materialized.case.seed, CANONICAL_SEED);
        assert_eq!(materialized.case.inputs["reference_scenario_id"], ID);
        assert_eq!(materialized.case.inputs["handoff_payload"], "git_only");
        assert_eq!(materialized.case.complexity.profile.artifact_count, 10);
        assert_eq!(materialized.case.deliverable_contract.artifacts.len(), 10);
        assert!(materialized
            .case
            .required_capabilities
            .contains(&"e2e::subagents".to_string()));
        let artifact_ids = materialized
            .case
            .deliverable_contract
            .artifacts
            .iter()
            .map(|artifact| artifact.id.as_str())
            .collect::<HashSet<_>>();
        assert!(artifact_ids.contains("implementation_plan"));
        assert!(artifact_ids.contains("git_checkpoints"));
    }

    #[test]
    fn git_handoff_v2_separates_hard_gates_from_graded_signals() {
        let spec = git_handoff_scenario("rubric");
        let rubric = spec
            .criteria
            .iter()
            .map(|criterion| {
                (
                    criterion.id,
                    criterion.weight,
                    criterion.policy,
                    criterion.dimension,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rubric,
            vec![
                (
                    "orchestration_discipline",
                    15,
                    crate::assessment::AssessmentPolicy::HardGate,
                    EvaluationDimension::StructuralIntegrity,
                ),
                (
                    "git_handoff_integrity",
                    20,
                    crate::assessment::AssessmentPolicy::HardGate,
                    EvaluationDimension::StructuralIntegrity,
                ),
                (
                    "ticket_acceptance",
                    35,
                    crate::assessment::AssessmentPolicy::HardGate,
                    EvaluationDimension::Deliverable,
                ),
                (
                    "scope_and_lifecycle",
                    10,
                    crate::assessment::AssessmentPolicy::HardGate,
                    EvaluationDimension::StructuralIntegrity,
                ),
                (
                    "paired_efficiency",
                    15,
                    crate::assessment::AssessmentPolicy::Advisory,
                    EvaluationDimension::Efficiency,
                ),
                (
                    "handoff_convergence",
                    5,
                    crate::assessment::AssessmentPolicy::Advisory,
                    EvaluationDimension::StructuralIntegrity,
                ),
            ]
        );
        assert_eq!(
            spec.criteria
                .iter()
                .map(|criterion| u16::from(criterion.weight))
                .sum::<u16>(),
            100
        );
        let invariant_ids = git_handoff_deliverable_contract()
            .invariants
            .into_iter()
            .map(|invariant| invariant.id)
            .collect::<HashSet<_>>();
        assert!(invariant_ids.contains("orchestration_discipline"));
        assert!(invariant_ids.contains("git_handoff_integrity"));
        assert!(!invariant_ids.contains("paired_efficiency"));
        assert!(!invariant_ids.contains("handoff_convergence"));
    }

    #[test]
    fn paired_efficiency_uses_quartile_bands() {
        assert_eq!(ratio_points(1.25, 6), 6);
        assert_eq!(ratio_points(1.50, 6), 5);
        assert_eq!(ratio_points(1.75, 6), 3);
        assert_eq!(ratio_points(2.00, 6), 2);
        assert_eq!(ratio_points(2.01, 6), 0);
        assert_eq!(ratio_points(1.50, 3), 2);
        assert_eq!(ratio_points(2.00, 2), 1);
    }

    #[test]
    fn suite_local_pairing_reduces_score_without_changing_pass_status() {
        let baseline = comparison_scenario(
            materialize("comparison", CANONICAL_SEED).unwrap(),
            comparison_run(
                ID,
                test_efficiency(168_631, 12, 17, 60_830, 1.380_952_380_952_381),
            ),
        );
        let handoff = comparison_scenario(
            git_handoff_materialize("comparison", CANONICAL_SEED).unwrap(),
            comparison_run(
                GIT_HANDOFF_ID,
                test_efficiency(260_537, 36, 37, 121_439, 2.433_333_333_333_333),
            ),
        );
        let mut scenarios = vec![baseline, handoff];

        apply_paired_efficiency(&mut scenarios);

        let run = &scenarios[1].runs[0];
        assert_eq!(run.status, RunStatus::Passed);
        assert_eq!(run.score, Some(89));
        let efficiency = run
            .criteria
            .iter()
            .find(|criterion| criterion.id == "paired_efficiency")
            .unwrap();
        assert_eq!(efficiency.awarded, Some(4));
        assert!(efficiency.reason.contains("tokens=1.996x (2/6)"));
        assert!(efficiency.reason.contains("turns=3.000x (0/3)"));
        let assessment = run
            .assessment_results
            .iter()
            .find(|assessment| assessment.criterion_id == "paired_efficiency")
            .unwrap();
        assert_eq!(assessment.outcome, AssessmentOutcome::Partial);
        assert_eq!(assessment.score.as_ref().unwrap().awarded, 4);
        assert_eq!(scenarios[1].aggregate.median_score, Some(89.0));
        assert!(scenarios[1].passed);
    }

    #[test]
    fn unpaired_handoff_passes_with_an_unavailable_score() {
        let handoff = comparison_scenario(
            git_handoff_materialize("comparison", CANONICAL_SEED).unwrap(),
            comparison_run(GIT_HANDOFF_ID, test_efficiency(100, 10, 10, 10_000, 1.0)),
        );
        let mut scenarios = vec![handoff];

        apply_paired_efficiency(&mut scenarios);

        let run = &scenarios[0].runs[0];
        assert_eq!(run.status, RunStatus::Passed);
        assert_eq!(run.score, None);
        let efficiency = run
            .criteria
            .iter()
            .find(|criterion| criterion.id == "paired_efficiency")
            .unwrap();
        assert_eq!(efficiency.awarded, None);
        assert!(efficiency.reason.contains("baseline was not executed"));
        let assessment = run
            .assessment_results
            .iter()
            .find(|assessment| assessment.criterion_id == "paired_efficiency")
            .unwrap();
        assert_eq!(assessment.outcome, AssessmentOutcome::Unavailable);
        assert_eq!(assessment.score, None);
        assert_eq!(scenarios[0].aggregate.scored_runs, 0);
        assert!(scenarios[0].passed);
    }

    fn comparison_scenario(
        materialized: MaterializedScenario,
        run: E2eRunReport,
    ) -> E2eScenarioReport {
        E2eScenarioReport::aggregate_case(materialized.case, materialized.spec.execution, vec![run])
    }

    fn comparison_run(scenario_id: &str, efficiency: EfficiencyReport) -> E2eRunReport {
        let spec = if scenario_id == GIT_HANDOFF_ID {
            git_handoff_scenario("comparison")
        } else {
            scenario("comparison")
        };
        let mut run = E2eRunReport::new(
            format!("{scenario_id}-run"),
            format!("{scenario_id}-attempt"),
            1,
            format!("{scenario_id}-session"),
            "test prompt".into(),
        );
        run.status = RunStatus::Passed;
        run.efficiency = Some(efficiency);
        run.criteria = spec
            .criteria
            .iter()
            .map(|criterion| crate::report::CriterionReport {
                id: criterion.id.into(),
                possible: criterion.weight,
                awarded: Some(criterion.weight),
                reason: "test evidence passed".into(),
            })
            .collect();
        run.assessment_results = spec
            .criteria
            .iter()
            .map(|criterion| crate::assessment::AssessmentResult {
                criterion_id: criterion.id.into(),
                target: crate::assessment::AssessmentTarget {
                    kind: crate::assessment::AssessmentTargetKind::Criterion,
                    id: criterion.id.into(),
                },
                kind: criterion.kind,
                policy: criterion.policy,
                dimension: criterion.dimension,
                source: criterion.source,
                outcome: AssessmentOutcome::Passed,
                score: Some(AssessmentScore {
                    awarded: criterion.weight,
                    possible: criterion.weight,
                }),
                confidence: None,
                summary: "test evidence passed".into(),
                evidence: Vec::new(),
                analyzer: None,
                analyzer_usage: None,
            })
            .collect();
        run.score = Some(100);
        run
    }

    fn test_efficiency(
        wall_time_ms: u64,
        turns: u64,
        function_calls: u64,
        total_tokens: u64,
        work_amplification: f64,
    ) -> EfficiencyReport {
        EfficiencyReport {
            wall_time_ms,
            root_turns: Some(turns),
            child_turns: Some(0),
            child_sessions: Some(0),
            function_calls: Some(function_calls),
            function_call_errors: Some(0),
            validation_retries: Some(0),
            transient_resumes: Some(0),
            wake_resumes: Some(0),
            effective_fan_out: Some(0),
            critical_path_ms: Some(wall_time_ms),
            input_tokens: Some(total_tokens),
            output_tokens: Some(0),
            total_tokens: Some(total_tokens),
            cost_usd: None,
            minimum_expected_work: 1,
            observed_work: turns.checked_add(function_calls),
            work_amplification: Some(work_amplification),
            technical_attempts: 1,
            observed_complexity: crate::report::ObservedComplexityReport::default(),
            unavailable: BTreeMap::new(),
        }
    }

    #[test]
    fn git_handoff_prompt_keeps_the_implementation_payload_git_only() {
        let prompt = git_handoff_scenario("attempt-1").prompt;
        assert!(prompt.contains("e2e_attempt-1-planner"));
        assert!(prompt.contains("e2e_attempt-1-implementer"));
        assert!(prompt.contains(IMPLEMENTER_TASK));
        assert!(IMPLEMENTER_TASK.contains(".harness-e2e/task-case.json"));
        assert!(IMPLEMENTER_TASK.contains(IMPLEMENTATION_PLAN_PATH));
        assert!(!IMPLEMENTER_TASK.contains(task_case().ticket));
        assert!(!prompt.contains("\"filesystem_root\""));
    }

    #[test]
    fn git_handoff_topology_requires_auditor_and_wake_before_each_spawn() {
        let run_id = "attempt-7";
        let planner = planner_session(run_id);
        let implementer = implementer_session(run_id);
        let scope = handoff_state_scope(run_id);
        let call = |function_id: &str, arguments: Value| common::ObservedFunctionCall {
            function_id: function_id.into(),
            arguments,
        };
        let mut calls = vec![
            call(
                "engine::register_trigger",
                json!({
                    "trigger_type": HOOK_TYPE,
                    "function_id": plan_auditor_function_id(run_id),
                    "config": { "sessions": [planner] },
                }),
            ),
            call(
                "engine::register_trigger",
                json!({
                    "trigger_type": "state",
                    "once": true,
                    "config": { "scope": scope, "key": "plan" },
                    "lifecycle": { "expires_at": 1 },
                }),
            ),
            call("harness::spawn", json!({ "session_id": planner })),
            call(
                "engine::unregister_trigger",
                json!({ "subscription_id": "plan-validator" }),
            ),
            call(
                "engine::register_trigger",
                json!({
                    "trigger_type": HOOK_TYPE,
                    "function_id": implementation_auditor_function_id(run_id),
                    "config": { "sessions": [implementer] },
                }),
            ),
            call(
                "engine::register_trigger",
                json!({
                    "trigger_type": "state",
                    "once": true,
                    "config": { "scope": scope, "key": "implementation" },
                    "lifecycle": { "expires_at": 2 },
                }),
            ),
            call(
                "harness::spawn",
                json!({
                    "session_id": implementer,
                    "task": IMPLEMENTER_TASK,
                    "options": { "functions": { "allow": ["coder::*", "shell::exec"] } },
                }),
            ),
            call(
                "engine::unregister_trigger",
                json!({ "id": "implementation-validator" }),
            ),
        ];
        assert!(handoff_orchestration_ordered(&calls, run_id));
        assert!(implementation_spawn_is_git_only(
            &calls[6],
            "# private plan body"
        ));
        assert!(calls.iter().all(root_call_is_coordination_only));

        calls.swap(5, 6);
        assert!(!handoff_orchestration_ordered(&calls, run_id));
    }

    #[test]
    fn accepted_checkpoint_wake_contains_only_phase_and_sha() {
        let value = checkpoint_wake_value(HandoffPhase::Plan, "abc123");
        assert_eq!(value, json!({ "phase": "plan", "head_sha": "abc123" }));
        assert_eq!(value.as_object().unwrap().len(), 2);
    }

    #[test]
    fn root_transcript_places_the_accepted_plan_wake_before_implementer_spawn() {
        let implementer = implementer_session("attempt-9");
        let transcript = json!({
            "messages": [
                { "custom": { "custom_type": "trigger_fired", "data": {
                    "label": "engineering-plan-accepted", "retired": true,
                    "once": true, "target": "harness::send"
                }}},
                { "message": { "role": "assistant", "content": [{
                    "type": "function_call", "function_id": "harness::spawn",
                    "arguments": { "session_id": implementer }
                }]}},
                { "custom": { "custom_type": "trigger_fired", "data": {
                    "label": "engineering-implementation-accepted", "retired": true,
                    "once": true, "target": "harness::send"
                }}}
            ]
        });
        assert!(handoff_wakes_valid(&transcript));
        assert!(plan_wake_precedes_implementer_spawn(
            &transcript,
            &implementer
        ));

        let reversed = json!({
            "messages": [
                { "message": { "role": "assistant", "content": [{
                    "type": "function_call", "function_id": "harness::spawn",
                    "arguments": { "session_id": implementer }
                }]}},
                { "custom": { "custom_type": "trigger_fired", "data": {
                    "label": "engineering-plan-accepted", "retired": true,
                    "once": true, "target": "harness::send"
                }}}
            ]
        });
        assert!(!plan_wake_precedes_implementer_spawn(
            &reversed,
            &implementer
        ));
    }

    #[test]
    fn refs_policy_allows_only_the_original_branch_to_advance() {
        let baseline = "refs/heads/e2e/run aaaa\nrefs/tags/reviewed bbbb";
        let advanced = "refs/heads/e2e/run cccc\nrefs/tags/reviewed bbbb";
        assert!(refs_advance_only(
            baseline,
            advanced,
            Some("refs/heads/e2e/run"),
            "cccc"
        ));
        assert!(!refs_advance_only(
            baseline,
            "refs/heads/e2e/run cccc\nrefs/tags/reviewed dddd",
            Some("refs/heads/e2e/run"),
            "cccc"
        ));
        assert!(!refs_advance_only(
            baseline,
            "refs/heads/e2e/run cccc\nrefs/heads/extra cccc\nrefs/tags/reviewed bbbb",
            Some("refs/heads/e2e/run"),
            "cccc"
        ));
    }

    #[tokio::test]
    async fn disposable_fixture_refs_can_be_restored_to_the_exact_snapshot() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = prepare_git_handoff_fixture(temporary.path());
        let baseline = preflight_fixture(task_case(), &repository).await.unwrap();
        let snapshot = refs_snapshot(&repository).await.unwrap();
        std::fs::write(repository.join(IMPLEMENTATION_PLAN_PATH), "# plan\n").unwrap();
        test_git(&repository, &["add", IMPLEMENTATION_PLAN_PATH]);
        test_git(&repository, &["commit", "-qm", "advance fixture"]);
        test_git(&repository, &["tag", "unexpected"]);

        let original_ref = baseline.initial_symbolic_ref.as_deref().unwrap();
        test_git(
            &repository,
            &["update-ref", original_ref, &baseline.fixture_head],
        );
        test_git(&repository, &["symbolic-ref", "HEAD", original_ref]);
        test_git(&repository, &["reset", "--hard", &baseline.fixture_head]);
        test_git(&repository, &["clean", "-fd"]);
        restore_exact_refs(&repository, &snapshot).await.unwrap();

        assert_eq!(refs_snapshot(&repository).await.unwrap(), snapshot);
        assert_eq!(
            git(&repository, &["rev-parse", "HEAD"]).await.unwrap(),
            baseline.fixture_head
        );
        assert_eq!(
            git_optional(&repository, &["symbolic-ref", "-q", "HEAD"])
                .await
                .unwrap(),
            baseline.initial_symbolic_ref
        );
        assert!(git(
            &repository,
            &["status", "--porcelain=v1", "--untracked-files=all"]
        )
        .await
        .unwrap()
        .is_empty());
    }

    #[tokio::test]
    async fn git_checkpoint_auditors_accept_multiple_commits_per_phase() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = prepare_git_handoff_fixture(temporary.path());
        let task = task_case();
        let baseline = preflight_fixture(task, &repository).await.unwrap();
        let baseline_refs = refs_snapshot(&repository).await.unwrap();

        std::fs::write(
            repository.join(IMPLEMENTATION_PLAN_PATH),
            "# Plan\n\nInspect cancellation and preserve CancelledError.\n",
        )
        .unwrap();
        test_git(&repository, &["add", IMPLEMENTATION_PLAN_PATH]);
        test_git(&repository, &["commit", "-qm", "plan cancellation repair"]);
        std::fs::write(
            repository.join(IMPLEMENTATION_PLAN_PATH),
            "# Plan\n\nInspect cancellation and preserve CancelledError.\n\nRun focused and full tests.\n",
        )
        .unwrap();
        test_git(&repository, &["add", IMPLEMENTATION_PLAN_PATH]);
        test_git(&repository, &["commit", "-qm", "clarify validation"]);

        let plan = audit_plan_checkpoint(task, &repository, &baseline, &baseline_refs, 1)
            .await
            .unwrap();
        assert!(plan.accepted, "{:?}", plan.feedback);
        assert_eq!(plan.commits.len(), 2);
        let plan_sha256 = artifact::sha256_bytes(
            &std::fs::read(repository.join(IMPLEMENTATION_PLAN_PATH)).unwrap(),
        );

        std::fs::write(
            repository.join("src/cancellation.py"),
            "# Cancellation behavior is explicit below.\nimport asyncio\n\n\nclass Operation:\n    def __init__(self):\n        self.resource_open = False\n        self.state = \"pending\"\n\n    async def run(self, started):\n        self.resource_open = True\n        self.state = \"running\"\n        started.set()\n        await asyncio.sleep(3600)\n        self.resource_open = False\n        self.state = \"completed\"\n",
        )
        .unwrap();
        test_git(&repository, &["add", "src/cancellation.py"]);
        test_git(
            &repository,
            &["commit", "-qm", "document cancellation behavior"],
        );
        std::fs::write(
            repository.join("src/cancellation.py"),
            valid_cancellation_source(),
        )
        .unwrap();
        test_git(&repository, &["add", "src/cancellation.py"]);
        test_git(
            &repository,
            &["commit", "-qm", "handle cancellation cleanup"],
        );
        let implementation = audit_implementation_checkpoint(
            task,
            &repository,
            &baseline,
            &baseline_refs,
            &plan.head_sha,
            &plan_sha256,
            1,
        )
        .await
        .unwrap();
        assert!(implementation.accepted, "{:?}", implementation.feedback);
        assert_eq!(implementation.commits.len(), 2);
        assert_eq!(implementation.changed_paths, vec!["src/cancellation.py"]);
        assert!(implementation.focused.as_ref().unwrap().passed);
        assert!(implementation.hidden.iter().all(|probe| probe.passed));
        assert!(implementation.full_suite.as_ref().unwrap().passed);
    }

    #[tokio::test]
    async fn plan_checkpoint_rejects_dirty_oversized_merge_and_extra_ref_states() {
        let task = task_case();

        let dirty_temp = tempfile::tempdir().unwrap();
        let dirty = prepare_git_handoff_fixture(dirty_temp.path());
        let dirty_baseline = preflight_fixture(task, &dirty).await.unwrap();
        let dirty_refs = refs_snapshot(&dirty).await.unwrap();
        std::fs::write(dirty.join(IMPLEMENTATION_PLAN_PATH), "# uncommitted\n").unwrap();
        let record = audit_plan_checkpoint(task, &dirty, &dirty_baseline, &dirty_refs, 1)
            .await
            .unwrap();
        assert!(!record.accepted);
        assert!(!record.worktree_clean);
        assert!(record.commits.is_empty());

        let large_temp = tempfile::tempdir().unwrap();
        let large = prepare_git_handoff_fixture(large_temp.path());
        let large_baseline = preflight_fixture(task, &large).await.unwrap();
        let large_refs = refs_snapshot(&large).await.unwrap();
        std::fs::write(
            large.join(IMPLEMENTATION_PLAN_PATH),
            vec![b'x'; MAX_IMPLEMENTATION_PLAN_BYTES + 1],
        )
        .unwrap();
        test_git(&large, &["add", IMPLEMENTATION_PLAN_PATH]);
        test_git(&large, &["commit", "-qm", "oversized plan"]);
        let record = audit_plan_checkpoint(task, &large, &large_baseline, &large_refs, 1)
            .await
            .unwrap();
        assert!(!record.accepted);
        assert!(!record.plan_valid);

        let merge_temp = tempfile::tempdir().unwrap();
        let merged = prepare_git_handoff_fixture(merge_temp.path());
        let merge_baseline = preflight_fixture(task, &merged).await.unwrap();
        let merge_refs = refs_snapshot(&merged).await.unwrap();
        test_git(&merged, &["checkout", "-qb", "side"]);
        std::fs::write(merged.join(IMPLEMENTATION_PLAN_PATH), "# side plan\n").unwrap();
        test_git(&merged, &["add", IMPLEMENTATION_PLAN_PATH]);
        test_git(&merged, &["commit", "-qm", "side plan"]);
        let original = merge_baseline
            .initial_symbolic_ref
            .as_deref()
            .unwrap()
            .strip_prefix("refs/heads/")
            .unwrap();
        test_git(&merged, &["checkout", "-q", original]);
        test_git(&merged, &["merge", "--no-ff", "side", "-qm", "merge plan"]);
        test_git(&merged, &["tag", "extra-ref"]);
        let record = audit_plan_checkpoint(task, &merged, &merge_baseline, &merge_refs, 1)
            .await
            .unwrap();
        assert!(!record.accepted);
        assert!(!record.no_merge_commits);
        assert!(!record.refs_valid);
    }

    #[tokio::test]
    async fn plan_checkpoint_rejects_missing_empty_symlink_branch_and_ancestry_states() {
        let task = task_case();

        let missing_temp = tempfile::tempdir().unwrap();
        let missing = prepare_git_handoff_fixture(missing_temp.path());
        let missing_baseline = preflight_fixture(task, &missing).await.unwrap();
        let missing_refs = refs_snapshot(&missing).await.unwrap();
        std::fs::write(missing.join(IMPLEMENTATION_PLAN_PATH), "# temporary\n").unwrap();
        test_git(&missing, &["add", IMPLEMENTATION_PLAN_PATH]);
        test_git(&missing, &["commit", "-qm", "add temporary plan"]);
        test_git(&missing, &["rm", "-q", IMPLEMENTATION_PLAN_PATH]);
        test_git(&missing, &["commit", "-qm", "remove plan"]);
        let record = audit_plan_checkpoint(task, &missing, &missing_baseline, &missing_refs, 1)
            .await
            .unwrap();
        assert!(!record.plan_valid);
        assert!(!record.accepted);

        let empty_temp = tempfile::tempdir().unwrap();
        let empty = prepare_git_handoff_fixture(empty_temp.path());
        let empty_baseline = preflight_fixture(task, &empty).await.unwrap();
        let empty_refs = refs_snapshot(&empty).await.unwrap();
        std::fs::write(empty.join(IMPLEMENTATION_PLAN_PATH), "").unwrap();
        test_git(&empty, &["add", IMPLEMENTATION_PLAN_PATH]);
        test_git(&empty, &["commit", "-qm", "add empty plan"]);
        let record = audit_plan_checkpoint(task, &empty, &empty_baseline, &empty_refs, 1)
            .await
            .unwrap();
        assert!(!record.plan_valid);
        assert!(!record.accepted);

        let symlink_temp = tempfile::tempdir().unwrap();
        let symlink = prepare_git_handoff_fixture(symlink_temp.path());
        let symlink_baseline = preflight_fixture(task, &symlink).await.unwrap();
        let symlink_refs = refs_snapshot(&symlink).await.unwrap();
        std::os::unix::fs::symlink(
            "src/cancellation.py",
            symlink.join(IMPLEMENTATION_PLAN_PATH),
        )
        .unwrap();
        test_git(&symlink, &["add", IMPLEMENTATION_PLAN_PATH]);
        test_git(&symlink, &["commit", "-qm", "add symlink plan"]);
        let record = audit_plan_checkpoint(task, &symlink, &symlink_baseline, &symlink_refs, 1)
            .await
            .unwrap();
        assert!(!record.plan_valid);
        assert!(!record.allowed_paths_only);
        assert!(!record.accepted);

        let branch_temp = tempfile::tempdir().unwrap();
        let branch = prepare_git_handoff_fixture(branch_temp.path());
        let branch_baseline = preflight_fixture(task, &branch).await.unwrap();
        let branch_refs = refs_snapshot(&branch).await.unwrap();
        std::fs::write(branch.join(IMPLEMENTATION_PLAN_PATH), "# plan\n").unwrap();
        test_git(&branch, &["add", IMPLEMENTATION_PLAN_PATH]);
        test_git(&branch, &["commit", "-qm", "commit plan"]);
        test_git(&branch, &["checkout", "-qb", "switched"]);
        let record = audit_plan_checkpoint(task, &branch, &branch_baseline, &branch_refs, 1)
            .await
            .unwrap();
        assert!(!record.branch_unchanged);
        assert!(!record.refs_valid);
        assert!(!record.accepted);

        let ancestry_temp = tempfile::tempdir().unwrap();
        let ancestry = prepare_git_handoff_fixture(ancestry_temp.path());
        let ancestry_baseline = preflight_fixture(task, &ancestry).await.unwrap();
        let ancestry_refs = refs_snapshot(&ancestry).await.unwrap();
        test_git(&ancestry, &["checkout", "--orphan", "unrelated"]);
        test_git(&ancestry, &["rm", "-q", "-r", "-f", "."]);
        std::fs::write(
            ancestry.join(IMPLEMENTATION_PLAN_PATH),
            "# unrelated plan\n",
        )
        .unwrap();
        test_git(&ancestry, &["add", IMPLEMENTATION_PLAN_PATH]);
        test_git(&ancestry, &["commit", "-qm", "unrelated plan"]);
        let record = audit_plan_checkpoint(task, &ancestry, &ancestry_baseline, &ancestry_refs, 1)
            .await
            .unwrap();
        assert!(!record.ancestry_valid);
        assert!(!record.accepted);
    }

    #[tokio::test]
    async fn implementation_checkpoint_enforces_immutable_and_bounded_production_scope() {
        let task = task_case();

        let plan_temp = tempfile::tempdir().unwrap();
        let plan_repo = prepare_git_handoff_fixture(plan_temp.path());
        let (baseline, refs, plan, plan_sha256) = accepted_test_plan(&plan_repo).await;
        std::fs::write(
            plan_repo.join("src/cancellation.py"),
            valid_cancellation_source(),
        )
        .unwrap();
        std::fs::write(
            plan_repo.join(IMPLEMENTATION_PLAN_PATH),
            "# rewritten plan\n",
        )
        .unwrap();
        test_git(&plan_repo, &["add", "."]);
        test_git(&plan_repo, &["commit", "-qm", "rewrite accepted plan"]);
        let record = audit_implementation_checkpoint(
            task,
            &plan_repo,
            &baseline,
            &refs,
            &plan.head_sha,
            &plan_sha256,
            1,
        )
        .await
        .unwrap();
        assert!(!record.plan_preserved);
        assert!(!record.allowed_paths_only);
        assert!(!record.accepted);

        let protected_temp = tempfile::tempdir().unwrap();
        let protected_repo = prepare_git_handoff_fixture(protected_temp.path());
        let (baseline, refs, plan, plan_sha256) = accepted_test_plan(&protected_repo).await;
        std::fs::write(
            protected_repo.join("src/cancellation.py"),
            valid_cancellation_source(),
        )
        .unwrap();
        let test_path = protected_repo.join("tests/test_engineering.py");
        let mut public_test = std::fs::read_to_string(&test_path).unwrap();
        public_test.push_str("\n# forbidden test edit\n");
        std::fs::write(&test_path, public_test).unwrap();
        let manifest_path = protected_repo.join(".harness-e2e/task-case.json");
        let mut manifest = std::fs::read_to_string(&manifest_path).unwrap();
        manifest.push(' ');
        std::fs::write(&manifest_path, manifest).unwrap();
        test_git(&protected_repo, &["add", "."]);
        test_git(&protected_repo, &["commit", "-qm", "edit protected inputs"]);
        let record = audit_implementation_checkpoint(
            task,
            &protected_repo,
            &baseline,
            &refs,
            &plan.head_sha,
            &plan_sha256,
            1,
        )
        .await
        .unwrap();
        assert!(!record.protected_paths_exact);
        assert!(!record.allowed_paths_only);
        assert!(!record.accepted);

        let outside_temp = tempfile::tempdir().unwrap();
        let outside_repo = prepare_git_handoff_fixture(outside_temp.path());
        let (baseline, refs, plan, plan_sha256) = accepted_test_plan(&outside_repo).await;
        std::fs::write(
            outside_repo.join("src/cancellation.py"),
            valid_cancellation_source(),
        )
        .unwrap();
        std::fs::write(outside_repo.join("NOT_ALLOWED.md"), "outside scope\n").unwrap();
        test_git(&outside_repo, &["add", "."]);
        test_git(
            &outside_repo,
            &["commit", "-qm", "edit outside production allowlist"],
        );
        let record = audit_implementation_checkpoint(
            task,
            &outside_repo,
            &baseline,
            &refs,
            &plan.head_sha,
            &plan_sha256,
            1,
        )
        .await
        .unwrap();
        assert!(!record.allowed_paths_only);
        assert!(record.protected_paths_exact);
        assert!(!record.accepted);

        let budget_temp = tempfile::tempdir().unwrap();
        let budget_repo = prepare_git_handoff_fixture(budget_temp.path());
        let (baseline, refs, plan, plan_sha256) = accepted_test_plan(&budget_repo).await;
        let oversized_source = format!(
            "{}\n{}",
            valid_cancellation_source(),
            (0..60)
                .map(|index| format!("# padding {index}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        std::fs::write(budget_repo.join("src/cancellation.py"), oversized_source).unwrap();
        test_git(&budget_repo, &["add", "src/cancellation.py"]);
        test_git(&budget_repo, &["commit", "-qm", "exceed line budget"]);
        let record = audit_implementation_checkpoint(
            task,
            &budget_repo,
            &baseline,
            &refs,
            &plan.head_sha,
            &plan_sha256,
            1,
        )
        .await
        .unwrap();
        assert!(record.focused.as_ref().unwrap().passed);
        assert!(!record.within_budget);
        assert!(!record.accepted);

        let symlink_temp = tempfile::tempdir().unwrap();
        let symlink_repo = prepare_git_handoff_fixture(symlink_temp.path());
        let (baseline, refs, plan, plan_sha256) = accepted_test_plan(&symlink_repo).await;
        std::fs::remove_file(symlink_repo.join("src/cancellation.py")).unwrap();
        std::os::unix::fs::symlink(
            "../tests/focused_probe.py",
            symlink_repo.join("src/cancellation.py"),
        )
        .unwrap();
        test_git(&symlink_repo, &["add", "src/cancellation.py"]);
        test_git(
            &symlink_repo,
            &["commit", "-qm", "replace production with symlink"],
        );
        let record = audit_implementation_checkpoint(
            task,
            &symlink_repo,
            &baseline,
            &refs,
            &plan.head_sha,
            &plan_sha256,
            1,
        )
        .await
        .unwrap();
        assert!(!record.allowed_paths_only);
        assert!(!record.accepted);
    }

    #[tokio::test]
    async fn rejected_implementation_can_replace_unaccepted_commits_in_the_same_phase() {
        let repository_temp = tempfile::tempdir().unwrap();
        let repository = prepare_git_handoff_fixture(repository_temp.path());
        let task = task_case();
        let (baseline, refs, plan, plan_sha256) = accepted_test_plan(&repository).await;
        let branch = git_optional(&repository, &["symbolic-ref", "-q", "HEAD"])
            .await
            .unwrap();

        let mut still_failing =
            std::fs::read_to_string(repository.join("src/cancellation.py")).unwrap();
        still_failing.push_str("\n# first rejected implementation\n");
        std::fs::write(repository.join("src/cancellation.py"), still_failing).unwrap();
        test_git(&repository, &["add", "src/cancellation.py"]);
        test_git(
            &repository,
            &["commit", "-qm", "first implementation attempt"],
        );
        let rejected = audit_implementation_checkpoint(
            task,
            &repository,
            &baseline,
            &refs,
            &plan.head_sha,
            &plan_sha256,
            1,
        )
        .await
        .unwrap();
        assert!(!rejected.accepted);
        assert!(!rejected.focused.as_ref().unwrap().passed);

        std::fs::write(
            repository.join("src/cancellation.py"),
            valid_cancellation_source(),
        )
        .unwrap();
        test_git(&repository, &["add", "src/cancellation.py"]);
        test_git(
            &repository,
            &["commit", "--amend", "-qm", "repair cancellation"],
        );
        let repaired = audit_implementation_checkpoint(
            task,
            &repository,
            &baseline,
            &refs,
            &plan.head_sha,
            &plan_sha256,
            2,
        )
        .await
        .unwrap();
        assert!(repaired.accepted, "{:?}", repaired.feedback);
        assert_ne!(rejected.head_sha, repaired.head_sha);
        assert_eq!(
            git_optional(&repository, &["symbolic-ref", "-q", "HEAD"])
                .await
                .unwrap(),
            branch
        );
        assert!(
            git_is_ancestor(&repository, &plan.head_sha, &repaired.head_sha)
                .await
                .unwrap()
        );
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

    fn prepare_git_handoff_fixture(parent: &Path) -> PathBuf {
        let repository = parent.join("fixture");
        copy_tree(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join(task_case().fixture_repository),
            &repository,
        )
        .unwrap();
        test_git(&repository, &["init", "-q"]);
        test_git(&repository, &["add", "."]);
        let status = std::process::Command::new("git")
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
        assert!(status.success());
        test_git(
            &repository,
            &["config", "--local", "user.name", "Harness E2E"],
        );
        test_git(
            &repository,
            &[
                "config",
                "--local",
                "user.email",
                "harness-e2e@example.invalid",
            ],
        );
        repository
    }

    fn valid_cancellation_source() -> &'static str {
        "import asyncio\n\n\nclass Operation:\n    def __init__(self):\n        self.resource_open = False\n        self.state = \"pending\"\n\n    async def run(self, started):\n        self.resource_open = True\n        self.state = \"running\"\n        started.set()\n        try:\n            await asyncio.sleep(3600)\n        except asyncio.CancelledError:\n            self.state = \"cancelled\"\n            raise\n        else:\n            self.state = \"completed\"\n        finally:\n            self.resource_open = False\n"
    }

    async fn accepted_test_plan(
        repository: &Path,
    ) -> (BaselineRecord, String, HandoffAttemptRecord, String) {
        let task = task_case();
        let baseline = preflight_fixture(task, repository).await.unwrap();
        let refs = refs_snapshot(repository).await.unwrap();
        std::fs::write(
            repository.join(IMPLEMENTATION_PLAN_PATH),
            "# Implementation plan\n\nRepair cancellation cleanup and validate it.\n",
        )
        .unwrap();
        test_git(repository, &["add", IMPLEMENTATION_PLAN_PATH]);
        test_git(repository, &["commit", "-qm", "commit implementation plan"]);
        let plan = audit_plan_checkpoint(task, repository, &baseline, &refs, 1)
            .await
            .unwrap();
        assert!(plan.accepted, "{:?}", plan.feedback);
        let digest = artifact::sha256_bytes(
            &std::fs::read(repository.join(IMPLEMENTATION_PLAN_PATH)).unwrap(),
        );
        (baseline, refs, plan, digest)
    }

    fn test_git(repository: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repository)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }
}
