//! `shell_coder_sandbox` — repair a multi-case Python reconciliation module,
//! prove the repair with public and runner-owned hidden probes, and then run
//! the exact repaired source in both the host workspace and a networkless VM.
//!
//! The public scenario version intentionally remains stable. The harder cohort
//! is identified by its canonical seed, materialized inputs, contract hash,
//! and assessment profile.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::process::Command;

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::validation_loop::suffix;
use super::{
    common, ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture,
    ComplexityProfile, DeliverableCaptureFuture, DeliverableContract, EvaluationFuture,
    ExecutionPolicy, InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "shell_coder_sandbox";
const VERSION: u32 = 5;
pub const CANONICAL_SEED: u64 = 2_051;
const DIFFICULTY_PROFILE: &str = "code-hard-2026-08";

const CODE_DELIVERABLE_ID: &str = "verified_code_file";
const EXECUTION_DELIVERABLE_ID: &str = "execution_evidence";
const SOURCE_PATH: &str = "src/reconcile.py";
const PUBLIC_TEST_PATH: &str = "tests/test_reconcile.py";
const TASK_PATH: &str = "TASK.md";
const DIAGNOSIS_DRAFT_PATH: &str = "diagnosis.tmp.md";
const DIAGNOSIS_PATH: &str = "evidence/diagnosis.md";
const SANDBOX_SOURCE_PATH: &str = "/workspace/src/reconcile.py";
const SANDBOX_TEST_PATH: &str = "/workspace/tests/test_reconcile.py";
const SANDBOX_WORKDIR: &str = "/workspace";
const HOST_DEMO_STDOUT: &str = r#"{"accounts":[{"account":"alpha","balance_cents":825},{"account":"beta","balance_cents":0}]}"#;
const MAX_SOURCE_BYTES: u64 = 64 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(90);
const FIXTURE_PATH_ENV: &str = "HARNESS_E2E_FIXTURE_PATH";
const FIXTURE_REPOSITORY: &str = "iii-hq/e2e-fixture";
const FIXTURE_SUBTREE: &str = "shell-coder-sandbox";
const FIXTURE_REVISION: &str = "16f6b9e05e34e09c824191eed0631d77f85be6a9";
const FIXTURE_MANIFEST_SHA256: &str =
    "sha256:cf8c9afcdf9a52feaee0cf5264c6b4268efe8a7c54ae013ebbd4bf43c44d3b84";

const HIDDEN_PROBE: &str = r#"import copy
import importlib.util
import json
from pathlib import Path

path = Path("src/reconcile.py")
spec = importlib.util.spec_from_file_location("candidate", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
reconcile = module.reconcile
checks = {}

events = [
    {"event_id": "move", "account": "old", "revision": 1, "amount_cents": 900, "kind": "charge"},
    {"event_id": "move", "account": "new", "revision": 3, "amount_cents": 400, "kind": "refund"},
    {"event_id": "move", "account": "stale", "revision": 2, "amount_cents": 700, "kind": "charge"},
    {"event_id": "keep", "account": "new", "revision": 1, "amount_cents": 1000, "kind": "charge"},
]
snapshot = copy.deepcopy(events)
checks["generator_and_out_of_order"] = reconcile((event for event in events)) == [
    {"account": "new", "balance_cents": 600},
]
checks["input_not_mutated"] = events == snapshot

voided = [
    {"event_id": "x", "account": "zero", "revision": 4, "amount_cents": 10, "kind": "void"},
    {"event_id": "x", "account": "ignored", "revision": 1, "amount_cents": 10, "kind": "charge"},
]
checks["zero_account_retained"] = reconcile(voided) == [
    {"account": "zero", "balance_cents": 0},
]

same = {"event_id": "same", "account": "a", "revision": 2, "amount_cents": 3, "kind": "charge"}
checks["identical_duplicate"] = reconcile([same, copy.deepcopy(same)]) == [
    {"account": "a", "balance_cents": 3},
]
try:
    reconcile([same, {**same, "kind": "refund"}])
    checks["conflict_rejected"] = False
except ValueError:
    checks["conflict_rejected"] = True

invalid_cases = [
    [{"event_id": "", "account": "a", "revision": 1, "amount_cents": 1, "kind": "charge"}],
    [{"event_id": "x", "account": "", "revision": 1, "amount_cents": 1, "kind": "charge"}],
    [{"event_id": "x", "account": "a", "revision": True, "amount_cents": 1, "kind": "charge"}],
    [{"event_id": "x", "account": "a", "revision": 1, "amount_cents": True, "kind": "charge"}],
    [{"event_id": "x", "account": "a", "revision": 1, "amount_cents": -1, "kind": "charge"}],
    [{"event_id": "x", "account": "a", "revision": 1, "amount_cents": 1, "kind": "unknown"}],
]
validation = []
for case in invalid_cases:
    try:
        reconcile(case)
        validation.append(False)
    except (TypeError, ValueError, KeyError):
        validation.append(True)
checks["validation"] = all(validation)
checks["demo"] = reconcile(module.demo_events()) == [
    {"account": "alpha", "balance_cents": 825},
    {"account": "beta", "balance_cents": 0},
]
print(json.dumps({"passed": all(checks.values()), "checks": checks}, sort_keys=True))
"#;

#[cfg(test)]
const REFERENCE_SOURCE: &str = r#"import json
import sys


def _normalized(event):
    if not isinstance(event, dict):
        raise TypeError("events must be dictionaries")
    event_id = event["event_id"]
    account = event["account"]
    revision = event["revision"]
    amount = event["amount_cents"]
    kind = event["kind"]
    if not isinstance(event_id, str) or not event_id:
        raise ValueError("event_id must be a non-empty string")
    if not isinstance(account, str) or not account:
        raise ValueError("account must be a non-empty string")
    if isinstance(revision, bool) or not isinstance(revision, int) or revision <= 0:
        raise ValueError("revision must be a positive integer")
    if isinstance(amount, bool) or not isinstance(amount, int) or amount < 0:
        raise ValueError("amount_cents must be a non-negative integer")
    if kind not in {"charge", "refund", "void"}:
        raise ValueError("unknown event kind")
    return (event_id, account, revision, amount, kind)


def reconcile(events):
    winners = {}
    for event in events:
        normalized = _normalized(event)
        event_id, _, revision, _, _ = normalized
        previous = winners.get(event_id)
        if previous is None or revision > previous[2]:
            winners[event_id] = normalized
        elif revision == previous[2] and normalized != previous:
            raise ValueError("conflicting event revision")
    totals = {}
    for _, account, _, amount, kind in winners.values():
        totals.setdefault(account, 0)
        if kind == "charge":
            totals[account] += amount
        elif kind == "refund":
            totals[account] -= amount
    return [
        {"account": account, "balance_cents": totals[account]}
        for account in sorted(totals)
    ]


def demo_events():
    return [
        {"event_id": "e1", "account": "alpha", "revision": 1, "amount_cents": 1000, "kind": "charge"},
        {"event_id": "e2", "account": "alpha", "revision": 1, "amount_cents": 175, "kind": "refund"},
        {"event_id": "e3", "account": "beta", "revision": 1, "amount_cents": 400, "kind": "charge"},
        {"event_id": "e3", "account": "beta", "revision": 2, "amount_cents": 400, "kind": "void"},
        {"event_id": "e1", "account": "alpha", "revision": 1, "amount_cents": 1000, "kind": "charge"},
    ]


if __name__ == "__main__":
    if sys.argv[1:] != ["--demo"]:
        raise SystemExit("usage: reconcile.py --demo")
    print(json.dumps({"accounts": reconcile(demo_events())}, sort_keys=True, separators=(",", ":")))
"#;

const WORKER_SETUP: AssessmentSpec = AssessmentSpec::hard_gated(
    "worker_setup",
    5,
    "The shell and sandbox workers are installed and expose every required code-execution surface.",
);
const INVESTIGATION: AssessmentSpec = AssessmentSpec::hard_gated(
    "investigation_and_red_baseline",
    15,
    "Source and tests are inspected and the public failure is reproduced before the first production edit.",
);
const DIAGNOSIS: AssessmentSpec = AssessmentSpec::score_only(
    "evidence_grounded_diagnosis",
    5,
    "A durable diagnosis is created and retained as the only additional workspace artifact.",
);
const PUBLIC_CORRECTNESS: AssessmentSpec = AssessmentSpec::hard_gated(
    "public_correctness",
    20,
    "The subject reruns the public suite after editing and the runner independently observes it green.",
);
const HIDDEN_CORRECTNESS: AssessmentSpec = AssessmentSpec::hard_gated(
    "hidden_correctness",
    25,
    "Runner-owned probes accept generators, out-of-order revisions, account migration, conflicts, validation, idempotency, and input immutability.",
);
const HOST_EXECUTION: AssessmentSpec = AssessmentSpec::hard_gated(
    "host_execution",
    10,
    "The repaired CLI runs in the host workspace and emits the exact compact JSON contract.",
);
const SANDBOX_PARITY: AssessmentSpec = AssessmentSpec::hard_gated(
    "sandbox_parity",
    15,
    "The exact repaired source and protected public tests pass inside a networkless sandbox, whose demo matches the host output.",
);
const SCOPE_AND_LIFECYCLE: AssessmentSpec = AssessmentSpec::hard_gated(
    "scope_and_lifecycle",
    5,
    "Protected files remain exact, only the source and diagnosis differ, and the sandbox lifecycle terminates cleanly.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    WORKER_SETUP,
    INVESTIGATION,
    DIAGNOSIS,
    PUBLIC_CORRECTNESS,
    HIDDEN_CORRECTNESS,
    HOST_EXECUTION,
    SANDBOX_PARITY,
    SCOPE_AND_LIFECYCLE,
];

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn materialize(namespace: &str, _seed: u64) -> Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        ID,
        VERSION,
        CANONICAL_SEED,
        json!({
            "difficulty_profile": DIFFICULTY_PROFILE,
            "fixture_repository": FIXTURE_REPOSITORY,
            "fixture_revision": FIXTURE_REVISION,
            "fixture_subtree": FIXTURE_SUBTREE,
            "fixture_manifest_sha256": FIXTURE_MANIFEST_SHA256,
            "source_path": SOURCE_PATH,
            "public_test_path": PUBLIC_TEST_PATH,
            "task_path": TASK_PATH,
            "diagnosis_path": DIAGNOSIS_PATH,
            "host_demo_stdout": HOST_DEMO_STDOUT,
            "sandbox_resources": {"cpus": 1, "memory_mb": 256, "network": false},
            "hidden_probe_families": 7,
        }),
        ComplexityProfile {
            planning_depth: 5,
            dependency_depth: 4,
            external_systems: 3,
            state_transitions: 12,
            validation_loops: 2,
            artifact_count: 2,
            coordination_edges: 3,
            ambiguity_level: 5,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::registry".to_string(),
            "iii::coder".to_string(),
            "iii::shell".to_string(),
            "iii::sandbox".to_string(),
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
    let sandbox_name = sandbox_name(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            r#"Repair the event reconciliation implementation in the isolated workspace `{root}`.

Follow this evidence order:
1. Add the `shell` worker from the public registry and wait. Then add `iii-sandbox` and wait.
2. Call `coder::info`, then read `{source}`, `{tests}`, and `{task}` with `coder::read-file`.
3. Before any edit, reproduce the failing baseline with exactly:
   `python3 -m unittest discover -s tests -p test_*.py`
4. Create `{diagnosis_draft}` with a concise diagnosis grounded in the failure.
5. Repair only `{source}` with `coder::update-file`. Use the standard library only.
6. Rerun the complete public suite until green, read the final source, and run
   `python3 {source} --demo`. Its stdout must be exactly `{host_stdout}`.
7. Move `{diagnosis_draft}` to `{diagnosis}` using `coder::move`. Add no other files.
8. Create one sandbox named `{sandbox_name}` with image `python`, exactly 1 CPU, 256 MB,
   and networking disabled. With `sandbox::fs::write` and `parents: true`, copy the exact
   final source to `{sandbox_source}` and unchanged public tests to `{sandbox_test}`.
9. In sandbox workdir `{sandbox_workdir}`, run the same unittest discovery command with
   `sandbox::exec`, then run `python3 {sandbox_source} --demo`. Require green tests and the
   same compact JSON. List the active sandbox, then stop it with `wait: true`.

Do not modify `{tests}` or `{task}`, use the network, write outside the workspace, or leave
the sandbox running. Hidden runner-side probes cover additional cases. Finish with a short
report containing the red baseline, green public suite, host demo, sandbox suite, and sandbox
demo results."#,
            root = root.display(),
            source = SOURCE_PATH,
            tests = PUBLIC_TEST_PATH,
            task = TASK_PATH,
            diagnosis_draft = DIAGNOSIS_DRAFT_PATH,
            diagnosis = DIAGNOSIS_PATH,
            host_stdout = HOST_DEMO_STDOUT,
            sandbox_source = SANDBOX_SOURCE_PATH,
            sandbox_test = SANDBOX_TEST_PATH,
            sandbox_workdir = SANDBOX_WORKDIR,
        ),
        filesystem_root: Some(root),
        execution: ExecutionPolicy {
            max_turns: 56,
            max_output_tokens: Some(16_384),
            max_total_tokens: Some(1_000_000),
            stuck_timeout_seconds: 900,
            max_validation_retries: None,
        },
        denied_functions: &["web::*", "scrapling::*", "http::*"],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
}

#[derive(Debug, Clone)]
struct FixtureAssets {
    source: String,
    public_tests: String,
    task: String,
}

fn expected_fixture_files(assets: &FixtureAssets) -> BTreeMap<&'static str, &str> {
    BTreeMap::from([
        (SOURCE_PATH, assets.source.as_str()),
        (PUBLIC_TEST_PATH, assets.public_tests.as_str()),
        (TASK_PATH, assets.task.as_str()),
    ])
}

fn load_fixture_assets() -> Result<FixtureAssets> {
    let checkout = fixture_root_from_env()?;
    validate_fixture_revision(&checkout)?;
    let subtree = checkout.join(FIXTURE_SUBTREE);
    if !subtree.is_dir() {
        bail!(
            "fixture subtree is missing or not a directory: {}",
            subtree.display()
        );
    }
    let observed_paths = collect_files(&subtree)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let expected_paths = [SOURCE_PATH, PUBLIC_TEST_PATH, TASK_PATH]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    if observed_paths != expected_paths {
        bail!(
            "fixture subtree paths differ: expected {expected_paths:?}, observed {observed_paths:?}"
        );
    }
    let manifest = compute_fixture_manifest_sha256(&subtree)?;
    if manifest != FIXTURE_MANIFEST_SHA256 {
        bail!("fixture manifest {manifest} differs from pinned {FIXTURE_MANIFEST_SHA256}");
    }
    Ok(FixtureAssets {
        source: read_fixture_asset(&subtree, SOURCE_PATH)?,
        public_tests: read_fixture_asset(&subtree, PUBLIC_TEST_PATH)?,
        task: read_fixture_asset(&subtree, TASK_PATH)?,
    })
}

fn fixture_root_from_env() -> Result<PathBuf> {
    let raw = std::env::var_os(FIXTURE_PATH_ENV)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{FIXTURE_PATH_ENV} must point to the fixture checkout"))?;
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        bail!("{FIXTURE_PATH_ENV} must be absolute: {}", path.display());
    }
    path.canonicalize()
        .with_context(|| format!("canonicalize fixture checkout {}", path.display()))
}

fn validate_fixture_revision(checkout: &Path) -> Result<()> {
    if !checkout.join(".git").exists() {
        bail!(
            "fixture checkout is not a Git repository: {}",
            checkout.display()
        );
    }
    let output = std::process::Command::new("git")
        .args(["-C"])
        .arg(checkout)
        .args(["rev-parse", "HEAD"])
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("read fixture revision from {}", checkout.display()))?;
    if !output.status.success() {
        bail!(
            "read fixture revision from {}: {}",
            checkout.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let revision = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if revision != FIXTURE_REVISION {
        bail!("fixture revision {revision} differs from pinned {FIXTURE_REVISION}");
    }
    Ok(())
}

fn compute_fixture_manifest_sha256(subtree: &Path) -> Result<String> {
    let mut concatenation = Vec::new();
    for relative in collect_files(subtree)? {
        let bytes = fs::read(subtree.join(&relative))
            .with_context(|| format!("read fixture asset {relative}"))?;
        let file_hash = crate::artifact::sha256_bytes(&bytes);
        let hex = file_hash
            .strip_prefix("sha256:")
            .context("fixture asset hash is not sha256-prefixed")?;
        concatenation.extend_from_slice(relative.as_bytes());
        concatenation.push(b'\n');
        concatenation.extend_from_slice(hex.as_bytes());
        concatenation.push(b'\n');
    }
    Ok(crate::artifact::sha256_bytes(&concatenation))
}

fn read_fixture_asset(subtree: &Path, relative: &str) -> Result<String> {
    fs::read_to_string(subtree.join(relative))
        .with_context(|| format!("read fixture asset {relative}"))
}

fn write_fixture(root: &Path, assets: &FixtureAssets) -> Result<()> {
    for (relative, content) in expected_fixture_files(assets) {
        let path = root.join(relative);
        let parent = path.parent().context("fixture file has no parent")?;
        fs::create_dir_all(parent)?;
        fs::write(&path, content)
            .with_context(|| format!("write fixture file {}", path.display()))?;
    }
    Ok(())
}

fn reset_fixture(root: &Path, assets: &FixtureAssets) -> Result<()> {
    ensure_safe_workspace(root)?;
    remove_workspace(root)?;
    write_fixture(root, assets)
}

fn setup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let assets = load_fixture_assets()?;
        let root = workspace_root(run_id);
        reset_fixture(&root, &assets)?;
        let public = run_public_tests(&root).await?;
        if public.success {
            bail!("shell/coder fixture baseline unexpectedly passes its public suite");
        }
        if run_hidden_probe(&root).await?.passed {
            bail!("shell/coder fixture baseline unexpectedly passes hidden probes");
        }
        Ok(())
    })
}

#[derive(Debug, Clone, Default, Deserialize)]
struct HiddenProbeOutput {
    passed: bool,
    #[serde(default)]
    checks: BTreeMap<String, bool>,
}

#[derive(Debug, Clone)]
struct CommandOutcome {
    success: bool,
    stdout: String,
    stderr: String,
}

#[derive(Debug)]
struct FixtureAudit {
    source: Option<String>,
    public: CommandOutcome,
    hidden: HiddenProbeOutput,
    hidden_output: String,
    demo: CommandOutcome,
    protected_files_exact: bool,
    production_patch_present: bool,
    diagnosis_present: bool,
    unexpected_paths: Vec<String>,
}

impl FixtureAudit {
    fn scope_valid(&self) -> bool {
        self.protected_files_exact
            && self.production_patch_present
            && self.diagnosis_present
            && self.unexpected_paths.is_empty()
            && self
                .source
                .as_ref()
                .is_some_and(|source| !source.is_empty() && source.len() as u64 <= MAX_SOURCE_BYTES)
    }
}

async fn audit_fixture(run_id: &str, assets: &FixtureAssets) -> Result<FixtureAudit> {
    let root = workspace_root(run_id);
    ensure_safe_workspace(&root)?;
    let source = fs::read_to_string(root.join(SOURCE_PATH)).ok();
    let public = run_public_tests(&root).await?;
    let hidden_command = run_python(&root, &["-c", HIDDEN_PROBE]).await?;
    let hidden = parse_hidden_probe(&hidden_command);
    let demo = run_python(&root, &[SOURCE_PATH, "--demo"]).await?;
    let protected_files_exact = [PUBLIC_TEST_PATH, TASK_PATH].into_iter().all(|relative| {
        fs::read_to_string(root.join(relative)).ok().as_deref()
            == expected_fixture_files(assets).get(relative).copied()
    });
    let production_patch_present = source
        .as_deref()
        .is_some_and(|candidate| candidate != assets.source);
    let diagnosis_present = fs::read_to_string(root.join(DIAGNOSIS_PATH))
        .ok()
        .is_some_and(|diagnosis| !diagnosis.trim().is_empty())
        && !root.join(DIAGNOSIS_DRAFT_PATH).exists();
    let expected_paths = [SOURCE_PATH, PUBLIC_TEST_PATH, TASK_PATH, DIAGNOSIS_PATH]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let unexpected_paths = collect_files(&root)?
        .into_iter()
        .filter(|path| !is_ignored_python_artifact(path))
        .filter(|path| !expected_paths.contains(path.as_str()))
        .collect();
    Ok(FixtureAudit {
        source,
        public,
        hidden,
        hidden_output: format!("{}{}", hidden_command.stdout, hidden_command.stderr),
        demo,
        protected_files_exact,
        production_patch_present,
        diagnosis_present,
        unexpected_paths,
    })
}

#[derive(Debug)]
struct WorkflowAudit {
    installed_shell: Option<usize>,
    installed_sandbox: Option<usize>,
    shell_install_succeeded: bool,
    sandbox_install_succeeded: bool,
    coder_info: Option<usize>,
    source_read: Option<usize>,
    tests_read: Option<usize>,
    task_read: Option<usize>,
    first_source_edit: Option<usize>,
    red_baseline: Option<usize>,
    green_public: Option<usize>,
    diagnosis_create: Option<usize>,
    diagnosis_move: Option<usize>,
    host_demo: Option<usize>,
    reads_before_edit: bool,
    red_before_edit: bool,
    evidence_ordered: bool,
}

impl WorkflowAudit {
    fn investigation_points(&self) -> u8 {
        let mut points = 0;
        if self.coder_info.is_some() {
            points += 3;
        }
        if self.source_read.is_some() && self.tests_read.is_some() && self.task_read.is_some() {
            points += 6;
        }
        if self.red_before_edit {
            points += 6;
        }
        points
    }

    fn investigation_complete(&self) -> bool {
        self.reads_before_edit && self.red_before_edit
    }

    fn diagnosis_complete(&self) -> bool {
        self.diagnosis_create.is_some() && self.diagnosis_move.is_some()
    }
}

fn workflow_audit(observation: &ScenarioObservation, root: &Path) -> WorkflowAudit {
    let invocations = common::function_invocations(&observation.transcript);
    let installed_shell = invocation_index(&invocations, |call| is_registry_install(call, "shell"));
    let installed_sandbox = invocation_index(&invocations, |call| {
        is_registry_install(call, "iii-sandbox")
    });
    let shell_install_succeeded = invocations.iter().any(|invocation| {
        is_registry_install(&invocation.call, "shell")
            && common::function_result(&observation.transcript, invocation).is_some()
    });
    let sandbox_install_succeeded = invocations.iter().any(|invocation| {
        is_registry_install(&invocation.call, "iii-sandbox")
            && common::function_result(&observation.transcript, invocation).is_some()
    });
    let coder_info = invocation_index(&invocations, |call| call.function_id == "coder::info");
    let source_read = invocation_index(&invocations, |call| is_coder_read(call, root, SOURCE_PATH));
    let tests_read = invocation_index(&invocations, |call| {
        is_coder_read(call, root, PUBLIC_TEST_PATH)
    });
    let task_read = invocation_index(&invocations, |call| is_coder_read(call, root, TASK_PATH));
    let first_source_edit = invocation_index(&invocations, |call| {
        is_coder_update(call, root, SOURCE_PATH)
    });
    let diagnosis_create = invocation_index(&invocations, |call| {
        is_coder_create(call, root, DIAGNOSIS_DRAFT_PATH)
    });
    let final_source_read =
        invocation_indexes(&invocations, |call| is_coder_read(call, root, SOURCE_PATH))
            .into_iter()
            .find(|index| first_source_edit.is_some_and(|edit| *index > edit));
    let diagnosis_move = invocation_index(&invocations, |call| {
        is_coder_move(call, root, DIAGNOSIS_DRAFT_PATH, DIAGNOSIS_PATH)
    });
    let mut red_baseline = None;
    let mut green_public = None;
    let mut host_demo = None;
    for (index, invocation) in invocations.iter().enumerate() {
        let Some(result) = common::function_result(&observation.transcript, invocation) else {
            continue;
        };
        if is_public_test_call(&invocation.call, root) {
            match result_exit_code(result) {
                Some(code) if code != 0 && red_baseline.is_none() => red_baseline = Some(index),
                Some(0) if first_source_edit.is_some_and(|edit| index > edit) => {
                    green_public.get_or_insert(index);
                }
                _ => {}
            }
        }
        if is_host_demo_call(&invocation.call, root)
            && successful_output_result(result, HOST_DEMO_STDOUT)
        {
            host_demo.get_or_insert(index);
        }
    }
    let reads_before_edit = matches!(
        (coder_info, source_read, tests_read, task_read, first_source_edit),
        (Some(info), Some(source), Some(tests), Some(task), Some(edit))
            if info < source && source < edit && tests < edit && task < edit
    );
    let red_before_edit =
        matches!((red_baseline, first_source_edit), (Some(red), Some(edit)) if red < edit);
    let evidence_ordered = matches!(
        (red_baseline, diagnosis_create, first_source_edit, green_public, final_source_read, host_demo, diagnosis_move),
        (Some(red), Some(diagnosis), Some(edit), Some(green), Some(read), Some(demo), Some(moved))
            if red < diagnosis && diagnosis < edit && edit < green && green < read && read < demo && demo < moved
    );
    WorkflowAudit {
        installed_shell,
        installed_sandbox,
        shell_install_succeeded,
        sandbox_install_succeeded,
        coder_info,
        source_read,
        tests_read,
        task_read,
        first_source_edit,
        red_baseline,
        green_public,
        diagnosis_create,
        diagnosis_move,
        host_demo,
        reads_before_edit,
        red_before_edit,
        evidence_ordered,
    }
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let root = workspace_root(run_id);
        let assets = load_fixture_assets()?;
        let fixture = audit_fixture(run_id, &assets).await?;
        let workflow = workflow_audit(observation, &root);
        let shell_ready = context.function_exists("shell::exec").await?;
        let coder_ready = context.function_exists("coder::update-file").await?;
        let mut sandbox_ready = true;
        for function in [
            "sandbox::create",
            "sandbox::fs::write",
            "sandbox::exec",
            "sandbox::list",
            "sandbox::stop",
        ] {
            sandbox_ready &= context.function_exists(function).await?;
        }
        let worker_setup = workflow.installed_shell.is_some()
            && workflow.installed_sandbox.is_some()
            && workflow.shell_install_succeeded
            && workflow.sandbox_install_succeeded
            && workflow
                .installed_shell
                .zip(workflow.installed_sandbox)
                .is_some_and(|(shell, sandbox)| shell < sandbox)
            && shell_ready
            && coder_ready
            && sandbox_ready;
        let sandbox = sandbox_evidence(
            &observation.transcript,
            &sandbox_name(run_id),
            fixture.source.as_deref(),
            &assets.public_tests,
        );
        let public_correctness =
            workflow.red_before_edit && workflow.green_public.is_some() && fixture.public.success;
        let host_execution = workflow.host_demo.is_some()
            && fixture.demo.success
            && fixture.demo.stdout.trim() == HOST_DEMO_STDOUT
            && fixture.demo.stderr.trim().is_empty();
        let scope = fixture.scope_valid() && workflow.evidence_ordered;
        Ok(assessment::build_evaluation([
            WORKER_SETUP.full_or_zero(
                worker_setup,
                format!(
                    "shell add={:?}/ok={}, sandbox add={:?}/ok={}, shell={shell_ready}, coder={coder_ready}, sandbox={sandbox_ready}",
                    workflow.installed_shell,
                    workflow.shell_install_succeeded,
                    workflow.installed_sandbox,
                    workflow.sandbox_install_succeeded,
                ),
            ),
            INVESTIGATION.gate_and_points(
                workflow.investigation_complete(),
                workflow.investigation_points(),
                format!(
                    "info={:?}, source={:?}, tests={:?}, task={:?}, red={:?}, edit={:?}",
                    workflow.coder_info,
                    workflow.source_read,
                    workflow.tests_read,
                    workflow.task_read,
                    workflow.red_baseline,
                    workflow.first_source_edit
                ),
            )?,
            DIAGNOSIS.award(
                if workflow.diagnosis_complete() && fixture.diagnosis_present {
                    DIAGNOSIS.weight()
                } else {
                    0
                },
                format!(
                    "create={:?}, move={:?}, retained={}",
                    workflow.diagnosis_create, workflow.diagnosis_move, fixture.diagnosis_present
                ),
            )?,
            PUBLIC_CORRECTNESS.full_or_zero(
                public_correctness,
                format!(
                    "subject red={:?}, green={:?}, runner green={}, stdout={:?}, stderr={:?}",
                    workflow.red_baseline,
                    workflow.green_public,
                    fixture.public.success,
                    fixture.public.stdout,
                    fixture.public.stderr
                ),
            ),
            HIDDEN_CORRECTNESS.full_or_zero(
                fixture.hidden.passed,
                format!("checks={:?}; output={:?}", fixture.hidden.checks, fixture.hidden_output),
            ),
            HOST_EXECUTION.full_or_zero(
                host_execution,
                format!(
                    "subject demo={:?}, runner success={}, stdout={:?}, stderr={:?}",
                    workflow.host_demo, fixture.demo.success, fixture.demo.stdout, fixture.demo.stderr
                ),
            ),
            SANDBOX_PARITY.full_or_zero(sandbox.complete(), sandbox.summary()),
            SCOPE_AND_LIFECYCLE.full_or_zero(
                scope && sandbox.stopped,
                format!(
                    "protected={}, patch={}, diagnosis={}, unexpected={:?}, ordered={}, stopped={}",
                    fixture.protected_files_exact,
                    fixture.production_patch_present,
                    fixture.diagnosis_present,
                    fixture.unexpected_paths,
                    workflow.evidence_ordered,
                    sandbox.stopped
                ),
            ),
        ]))
    })
}

fn capture<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let root = workspace_root(run_id);
        let assets = load_fixture_assets()?;
        let fixture = audit_fixture(run_id, &assets).await?;
        let workflow = workflow_audit(observation, &root);
        let sandbox = sandbox_evidence(
            &observation.transcript,
            &sandbox_name(run_id),
            fixture.source.as_deref(),
            &assets.public_tests,
        );
        let scope = fixture.scope_valid() && workflow.evidence_ordered;
        Ok(vec![
            CapturedDeliverable {
                id: CODE_DELIVERABLE_ID.to_string(),
                kind: "code_repair".to_string(),
                content: json!({
                    "path": SOURCE_PATH,
                    "content": fixture.source,
                    "public_tests_passed": fixture.public.success,
                    "hidden": {"passed": fixture.hidden.passed, "checks": fixture.hidden.checks},
                    "scope": {
                        "protected_files_exact": fixture.protected_files_exact,
                        "production_patch_present": fixture.production_patch_present,
                        "diagnosis_present": fixture.diagnosis_present,
                        "unexpected_paths": fixture.unexpected_paths,
                    },
                })
                .into(),
                invariants: vec![
                    CapturedInvariant {
                        id: "public_tests_green".to_string(),
                        passed: fixture.public.success,
                        reason: "runner independently executed the public suite".to_string(),
                    },
                    CapturedInvariant {
                        id: "hidden_tests_green".to_string(),
                        passed: fixture.hidden.passed,
                        reason: format!("runner-owned checks: {:?}", fixture.hidden.checks),
                    },
                    CapturedInvariant {
                        id: "repair_scope_exact".to_string(),
                        passed: scope,
                        reason: "protected fixture and topology were audited".to_string(),
                    },
                ],
                provenance: vec![
                    ProvenanceEvidence {
                        kind: "filesystem_path".to_string(),
                        source_id: root.join(SOURCE_PATH).display().to_string(),
                        relation: "independently_probed_before_cleanup".to_string(),
                    },
                    ProvenanceEvidence {
                        kind: "git_revision".to_string(),
                        source_id: FIXTURE_REVISION.to_string(),
                        relation: "derived_from_pinned_fixture".to_string(),
                    },
                ],
            },
            CapturedDeliverable {
                id: EXECUTION_DELIVERABLE_ID.to_string(),
                kind: "execution_result".to_string(),
                content: json!({
                    "host": {
                        "success": fixture.demo.success,
                        "stdout": fixture.demo.stdout.trim(),
                        "stderr": fixture.demo.stderr.trim(),
                    },
                    "sandbox": sandbox.as_json(),
                    "workflow": {
                        "red_baseline": workflow.red_baseline,
                        "green_public": workflow.green_public,
                        "host_demo": workflow.host_demo,
                        "evidence_ordered": workflow.evidence_ordered,
                    },
                })
                .into(),
                invariants: vec![
                    CapturedInvariant {
                        id: "host_demo_exact".to_string(),
                        passed: fixture.demo.success
                            && fixture.demo.stdout.trim() == HOST_DEMO_STDOUT,
                        reason: format!("expected exact host stdout `{HOST_DEMO_STDOUT}`"),
                    },
                    CapturedInvariant {
                        id: "sandbox_parity_complete".to_string(),
                        passed: sandbox.complete(),
                        reason: sandbox.summary(),
                    },
                ],
                provenance: vec![ProvenanceEvidence {
                    kind: "sandbox".to_string(),
                    source_id: sandbox_name(run_id),
                    relation: "exact_source_reexecuted_and_stopped".to_string(),
                }],
            },
        ])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![
            ArtifactExpectation {
                id: CODE_DELIVERABLE_ID.to_string(),
                kind: "code_repair".to_string(),
                media_type: "application/json".to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["path", "content", "public_tests_passed", "hidden", "scope"],
                    "properties": {
                        "path": {"const": SOURCE_PATH},
                        "content": {"type": ["string", "null"]},
                        "public_tests_passed": {"type": "boolean"},
                        "hidden": {"type": "object"},
                        "scope": {"type": "object"}
                    },
                    "additionalProperties": false
                }),
                max_size_bytes: 96_000,
            },
            ArtifactExpectation {
                id: EXECUTION_DELIVERABLE_ID.to_string(),
                kind: "execution_result".to_string(),
                media_type: "application/json".to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["host", "sandbox", "workflow"],
                    "properties": {
                        "host": {"type": "object"},
                        "sandbox": {"type": "object"},
                        "workflow": {"type": "object"}
                    },
                    "additionalProperties": false
                }),
                max_size_bytes: 32_768,
            },
        ],
        invariants: vec![
            InvariantSpec {
                id: "public_tests_green".to_string(),
                description: "The runner independently accepts the public suite.".to_string(),
            },
            InvariantSpec {
                id: "hidden_tests_green".to_string(),
                description: "Runner-owned adversarial probes accept the repair.".to_string(),
            },
            InvariantSpec {
                id: "repair_scope_exact".to_string(),
                description: "Only the production source and diagnosis differ.".to_string(),
            },
            InvariantSpec {
                id: "host_demo_exact".to_string(),
                description: "The host CLI preserves the compact JSON contract.".to_string(),
            },
            InvariantSpec {
                id: "sandbox_parity_complete".to_string(),
                description: "The exact repair passes inside a stopped sandbox.".to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

async fn run_public_tests(root: &Path) -> Result<CommandOutcome> {
    run_python(
        root,
        &[
            "-m",
            "unittest",
            "discover",
            "-s",
            "tests",
            "-p",
            "test_*.py",
        ],
    )
    .await
}

async fn run_hidden_probe(root: &Path) -> Result<HiddenProbeOutput> {
    let outcome = run_python(root, &["-c", HIDDEN_PROBE]).await?;
    Ok(parse_hidden_probe(&outcome))
}

fn parse_hidden_probe(outcome: &CommandOutcome) -> HiddenProbeOutput {
    outcome
        .success
        .then(|| {
            outcome
                .stdout
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .and_then(|line| serde_json::from_str(line).ok())
        })
        .flatten()
        .unwrap_or_default()
}

async fn run_python(root: &Path, args: &[&str]) -> Result<CommandOutcome> {
    let mut command = Command::new("python3");
    command
        .args(args)
        .current_dir(root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(COMMAND_TIMEOUT, command.output())
        .await
        .context("shell/coder Python probe timed out")?
        .context("launch shell/coder Python probe")?;
    Ok(CommandOutcome {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn invocation_index(
    invocations: &[common::ObservedFunctionInvocation],
    predicate: impl Fn(&common::ObservedFunctionCall) -> bool,
) -> Option<usize> {
    invocations
        .iter()
        .position(|invocation| predicate(&invocation.call))
}

fn invocation_indexes(
    invocations: &[common::ObservedFunctionInvocation],
    predicate: impl Fn(&common::ObservedFunctionCall) -> bool,
) -> Vec<usize> {
    invocations
        .iter()
        .enumerate()
        .filter_map(|(index, invocation)| predicate(&invocation.call).then_some(index))
        .collect()
}

fn is_registry_install(call: &common::ObservedFunctionCall, worker: &str) -> bool {
    call.function_id == "worker::add"
        && call
            .arguments
            .pointer("/source/kind")
            .and_then(Value::as_str)
            == Some("registry")
        && call
            .arguments
            .pointer("/source/name")
            .and_then(Value::as_str)
            == Some(worker)
}

fn is_coder_read(call: &common::ObservedFunctionCall, root: &Path, relative: &str) -> bool {
    call.function_id == "coder::read-file"
        && workspace_path_matches(
            call.arguments.get("path").and_then(Value::as_str),
            root,
            relative,
        )
}

fn is_coder_update(call: &common::ObservedFunctionCall, root: &Path, relative: &str) -> bool {
    call.function_id == "coder::update-file"
        && workspace_path_matches(
            call.arguments
                .pointer("/files/0/path")
                .and_then(Value::as_str),
            root,
            relative,
        )
        && call
            .arguments
            .pointer("/files/0/ops")
            .and_then(Value::as_array)
            .is_some_and(|ops| !ops.is_empty())
        && call
            .arguments
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(|files| files.len() == 1)
}

fn is_coder_create(call: &common::ObservedFunctionCall, root: &Path, relative: &str) -> bool {
    call.function_id == "coder::create-file"
        && workspace_path_matches(
            call.arguments
                .pointer("/files/0/path")
                .and_then(Value::as_str),
            root,
            relative,
        )
        && call
            .arguments
            .pointer("/files/0/content")
            .and_then(Value::as_str)
            .is_some_and(|content| !content.trim().is_empty())
        && call
            .arguments
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(|files| files.len() == 1)
}

fn is_coder_move(call: &common::ObservedFunctionCall, root: &Path, from: &str, to: &str) -> bool {
    call.function_id == "coder::move"
        && workspace_path_matches(
            call.arguments
                .pointer("/files/0/from")
                .and_then(Value::as_str),
            root,
            from,
        )
        && workspace_path_matches(
            call.arguments
                .pointer("/files/0/to")
                .and_then(Value::as_str),
            root,
            to,
        )
        && call
            .arguments
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(|files| files.len() == 1)
}

fn is_public_test_call(call: &common::ObservedFunctionCall, root: &Path) -> bool {
    is_shell_python_call(
        call,
        root,
        &[
            "-m",
            "unittest",
            "discover",
            "-s",
            "tests",
            "-p",
            "test_*.py",
        ],
    )
}

fn is_host_demo_call(call: &common::ObservedFunctionCall, root: &Path) -> bool {
    is_shell_python_call(call, root, &[SOURCE_PATH, "--demo"])
}

fn is_shell_python_call(
    call: &common::ObservedFunctionCall,
    root: &Path,
    expected_args: &[&str],
) -> bool {
    if call.function_id != "shell::exec" || !host_target(&call.arguments) {
        return false;
    }
    let python = call
        .arguments
        .get("command")
        .and_then(Value::as_str)
        .and_then(|value| Path::new(value).file_name().and_then(OsStr::to_str))
        .is_some_and(|value| matches!(value, "python" | "python3"));
    let args_match = call
        .arguments
        .get("args")
        .and_then(Value::as_array)
        .is_some_and(|args| {
            args.len() == expected_args.len()
                && args.iter().zip(expected_args).all(|(observed, expected)| {
                    observed.as_str().is_some_and(|observed| {
                        if *expected == SOURCE_PATH {
                            workspace_path_matches(Some(observed), root, SOURCE_PATH)
                        } else {
                            observed == *expected
                        }
                    })
                })
        });
    python && args_match
}

fn host_target(arguments: &Value) -> bool {
    arguments.get("target").is_none()
        || arguments.pointer("/target/kind").and_then(Value::as_str) == Some("host")
}

fn result_exit_code(result: &Value) -> Option<i64> {
    result.pointer("/details/exit_code").and_then(Value::as_i64)
}

fn workspace_path_matches(value: Option<&str>, root: &Path, expected: &str) -> bool {
    let Some(value) = value else {
        return false;
    };
    let path = Path::new(value);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    normalize_workspace_path(&resolved) == normalize_workspace_path(&root.join(expected))
}

fn normalize_workspace_path(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => return None,
            Component::Normal(part) => normalized.push(part),
        }
    }
    Some(normalized)
}

fn function_results<'a>(transcript: &'a Value, function_id: &'a str) -> Vec<&'a Value> {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("message"))
        .filter(|message| {
            message.get("role").and_then(Value::as_str) == Some("function_result")
                && message.get("function_id").and_then(Value::as_str) == Some(function_id)
                && message.get("is_error").and_then(Value::as_bool) == Some(false)
        })
        .collect()
}

fn successful_output_result(message: &Value, expected: &str) -> bool {
    result_exit_code(message) == Some(0)
        && message
            .pointer("/details/stdout")
            .and_then(Value::as_str)
            .is_some_and(|stdout| stdout.trim() == expected)
        && message
            .pointer("/details/stderr")
            .and_then(Value::as_str)
            .is_some_and(|stderr| stderr.trim().is_empty())
        && message
            .pointer("/details/timed_out")
            .and_then(Value::as_bool)
            != Some(true)
}

#[derive(Debug, Default)]
struct SandboxEvidence {
    sandbox_id: Option<String>,
    exact_create: bool,
    source_written: bool,
    tests_written: bool,
    public_tests_passed: bool,
    demo_passed: bool,
    listed_active: bool,
    stopped: bool,
    ordered: bool,
}

impl SandboxEvidence {
    fn complete(&self) -> bool {
        self.sandbox_id.is_some()
            && self.exact_create
            && self.source_written
            && self.tests_written
            && self.public_tests_passed
            && self.demo_passed
            && self.listed_active
            && self.stopped
            && self.ordered
    }

    fn summary(&self) -> String {
        format!(
            "sandbox id={}, create={}, source={}, tests={}, public={}, demo={}, listed={}, stopped={}, ordered={}",
            self.sandbox_id.as_deref().unwrap_or("missing"),
            self.exact_create,
            self.source_written,
            self.tests_written,
            self.public_tests_passed,
            self.demo_passed,
            self.listed_active,
            self.stopped,
            self.ordered
        )
    }

    fn as_json(&self) -> Value {
        json!({
            "sandbox_id": self.sandbox_id,
            "exact_create": self.exact_create,
            "source_written": self.source_written,
            "tests_written": self.tests_written,
            "public_tests_passed": self.public_tests_passed,
            "demo_passed": self.demo_passed,
            "listed_active": self.listed_active,
            "stopped": self.stopped,
            "ordered": self.ordered,
        })
    }
}

fn sandbox_evidence(
    transcript: &Value,
    expected_name: &str,
    final_source: Option<&str>,
    public_tests: &str,
) -> SandboxEvidence {
    let invocations = common::function_invocations(transcript);
    let sandbox_id = invocations.iter().find_map(|invocation| {
        (invocation.call.function_id == "sandbox::create"
            && exact_sandbox_create(&invocation.call, expected_name))
        .then(|| common::function_result(transcript, invocation))
        .flatten()
        .and_then(|result| {
            result
                .pointer("/details/sandbox_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
    });
    let Some(id) = sandbox_id.as_deref() else {
        return SandboxEvidence {
            sandbox_id,
            ..SandboxEvidence::default()
        };
    };
    let exact_create = invocations
        .iter()
        .any(|invocation| exact_sandbox_create(&invocation.call, expected_name));
    let source_written = final_source.is_some_and(|source| {
        correlated_call_succeeded(transcript, &invocations, |call| {
            is_sandbox_write(call, id, SANDBOX_SOURCE_PATH, source)
        })
    });
    let tests_written = correlated_call_succeeded(transcript, &invocations, |call| {
        is_sandbox_write(call, id, SANDBOX_TEST_PATH, public_tests)
    });
    let public_tests_passed = correlated_result_matches(
        transcript,
        &invocations,
        |call| is_sandbox_public_test(call, id),
        |result| result_exit_code(result) == Some(0),
    );
    let demo_passed = correlated_result_matches(
        transcript,
        &invocations,
        |call| is_sandbox_demo(call, id),
        |result| successful_output_result(result, HOST_DEMO_STDOUT),
    );
    let listed_active = function_results(transcript, "sandbox::list")
        .into_iter()
        .any(|message| {
            message
                .pointer("/details/sandboxes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|sandbox| {
                    sandbox.get("sandbox_id").and_then(Value::as_str) == Some(id)
                        && sandbox.get("name").and_then(Value::as_str) == Some(expected_name)
                        && sandbox.get("stopped").and_then(Value::as_bool) == Some(false)
                })
        });
    let stopped = correlated_result_matches(
        transcript,
        &invocations,
        |call| {
            call.function_id == "sandbox::stop"
                && call.arguments.get("sandbox_id").and_then(Value::as_str) == Some(id)
                && call.arguments.get("wait").and_then(Value::as_bool) == Some(true)
        },
        |result| {
            result.pointer("/details/stopped").and_then(Value::as_bool) == Some(true)
                && result
                    .pointer("/details/sandbox_id")
                    .and_then(Value::as_str)
                    == Some(id)
        },
    );
    let ordered =
        sandbox_calls_are_ordered(&invocations, expected_name, id, final_source, public_tests);
    SandboxEvidence {
        sandbox_id,
        exact_create,
        source_written,
        tests_written,
        public_tests_passed,
        demo_passed,
        listed_active,
        stopped,
        ordered,
    }
}

fn exact_sandbox_create(call: &common::ObservedFunctionCall, expected_name: &str) -> bool {
    call.function_id == "sandbox::create"
        && call.arguments.get("image").and_then(Value::as_str) == Some("python")
        && call.arguments.get("name").and_then(Value::as_str) == Some(expected_name)
        && call.arguments.get("cpus").and_then(Value::as_u64) == Some(1)
        && call.arguments.get("memory_mb").and_then(Value::as_u64) == Some(256)
        && call.arguments.get("network").and_then(Value::as_bool) == Some(false)
}

fn is_sandbox_write(
    call: &common::ObservedFunctionCall,
    sandbox_id: &str,
    path: &str,
    content: &str,
) -> bool {
    call.function_id == "sandbox::fs::write"
        && call.arguments.get("sandbox_id").and_then(Value::as_str) == Some(sandbox_id)
        && call.arguments.get("path").and_then(Value::as_str) == Some(path)
        && call.arguments.get("content").and_then(Value::as_str) == Some(content)
        && call.arguments.get("parents").and_then(Value::as_bool) == Some(true)
}

fn is_sandbox_public_test(call: &common::ObservedFunctionCall, sandbox_id: &str) -> bool {
    is_sandbox_python_call(
        call,
        sandbox_id,
        &[
            "-m",
            "unittest",
            "discover",
            "-s",
            "tests",
            "-p",
            "test_*.py",
        ],
    )
}

fn is_sandbox_demo(call: &common::ObservedFunctionCall, sandbox_id: &str) -> bool {
    is_sandbox_python_call(call, sandbox_id, &[SANDBOX_SOURCE_PATH, "--demo"])
}

fn is_sandbox_python_call(
    call: &common::ObservedFunctionCall,
    sandbox_id: &str,
    expected_args: &[&str],
) -> bool {
    call.function_id == "sandbox::exec"
        && call.arguments.get("sandbox_id").and_then(Value::as_str) == Some(sandbox_id)
        && call.arguments.get("cmd").and_then(Value::as_str) == Some("python3")
        && call.arguments.get("workdir").and_then(Value::as_str) == Some(SANDBOX_WORKDIR)
        && call
            .arguments
            .get("args")
            .and_then(Value::as_array)
            .is_some_and(|args| {
                args.len() == expected_args.len()
                    && args
                        .iter()
                        .filter_map(Value::as_str)
                        .eq(expected_args.iter().copied())
            })
}

fn sandbox_calls_are_ordered(
    invocations: &[common::ObservedFunctionInvocation],
    expected_name: &str,
    sandbox_id: &str,
    final_source: Option<&str>,
    public_tests: &str,
) -> bool {
    let create = invocation_index(invocations, |call| {
        exact_sandbox_create(call, expected_name)
    });
    let source = final_source.and_then(|source| {
        invocation_index(invocations, |call| {
            is_sandbox_write(call, sandbox_id, SANDBOX_SOURCE_PATH, source)
        })
    });
    let tests = invocation_index(invocations, |call| {
        is_sandbox_write(call, sandbox_id, SANDBOX_TEST_PATH, public_tests)
    });
    let public = invocation_index(invocations, |call| is_sandbox_public_test(call, sandbox_id));
    let demo = invocation_index(invocations, |call| is_sandbox_demo(call, sandbox_id));
    let list = demo.and_then(|demo| {
        invocation_indexes(invocations, |call| call.function_id == "sandbox::list")
            .into_iter()
            .find(|list| *list > demo)
    });
    let stop = list.and_then(|list| {
        invocation_indexes(invocations, |call| {
            call.function_id == "sandbox::stop"
                && call.arguments.get("sandbox_id").and_then(Value::as_str) == Some(sandbox_id)
                && call.arguments.get("wait").and_then(Value::as_bool) == Some(true)
        })
        .into_iter()
        .find(|stop| *stop > list)
    });
    matches!(
        (create, source, tests, public, demo, list, stop),
        (Some(create), Some(source), Some(tests), Some(public), Some(demo), Some(list), Some(stop))
            if create < source && create < tests && source < public && tests < public
                && public < demo && demo < list && list < stop
    )
}

fn correlated_call_succeeded(
    transcript: &Value,
    invocations: &[common::ObservedFunctionInvocation],
    call_matches: impl Fn(&common::ObservedFunctionCall) -> bool,
) -> bool {
    correlated_result_matches(transcript, invocations, call_matches, |_| true)
}

fn correlated_result_matches(
    transcript: &Value,
    invocations: &[common::ObservedFunctionInvocation],
    call_matches: impl Fn(&common::ObservedFunctionCall) -> bool,
    result_matches: impl Fn(&Value) -> bool,
) -> bool {
    invocations
        .iter()
        .filter(|invocation| call_matches(&invocation.call))
        .any(|invocation| {
            common::function_result(transcript, invocation).is_some_and(&result_matches)
        })
}

fn collect_files(root: &Path) -> Result<Vec<String>> {
    fn visit(root: &Path, directory: &Path, paths: &mut Vec<String>) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                paths.push(format!(
                    "{}#symlink",
                    path.strip_prefix(root)?.to_string_lossy()
                ));
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

fn is_ignored_python_artifact(path: &str) -> bool {
    path.split('/').any(|part| part == "__pycache__") || path.ends_with(".pyc")
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let mut failures = Vec::new();
        if let Err(error) = cleanup_owned_sandbox(context, &sandbox_name(run_id)).await {
            failures.push(format!("sandbox cleanup: {error:#}"));
        }
        if let Err(error) = remove_workspace(&workspace_root(run_id)) {
            failures.push(format!("workspace cleanup: {error:#}"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            bail!(failures.join("; "))
        }
    })
}

async fn cleanup_owned_sandbox(context: &E2eContext, expected_name: &str) -> Result<()> {
    if !context.function_exists("sandbox::list").await? {
        return Ok(());
    }
    let listed = context.trigger_value("sandbox::list", json!({})).await?;
    for sandbox_id in owned_running_sandbox_ids(&listed, expected_name) {
        let _: Value = context
            .trigger(
                "sandbox::stop",
                json!({"sandbox_id": sandbox_id, "wait": true}),
            )
            .await?;
    }
    Ok(())
}

fn owned_running_sandbox_ids(listed: &Value, expected_name: &str) -> Vec<String> {
    listed
        .get("sandboxes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|sandbox| sandbox.get("name").and_then(Value::as_str) == Some(expected_name))
        .filter(|sandbox| sandbox.get("stopped").and_then(Value::as_bool) != Some(true))
        .filter_map(|sandbox| sandbox.get("sandbox_id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

fn remove_workspace(root: &Path) -> Result<()> {
    ensure_safe_workspace(root)?;
    let Ok(metadata) = fs::symlink_metadata(root) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(root)?;
    } else {
        fs::remove_dir_all(root)?;
    }
    Ok(())
}

fn ensure_safe_workspace(root: &Path) -> Result<()> {
    let parent = workspace_parent();
    if root.parent() != Some(parent.as_path()) {
        bail!(
            "refusing shell/coder workspace operation outside {}: {}",
            parent.display(),
            root.display()
        );
    }
    let Some(leaf) = root.file_name().and_then(OsStr::to_str) else {
        bail!("shell/coder workspace has no UTF-8 leaf");
    };
    if !leaf.starts_with("shell-coder-")
        || !leaf
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("unsafe shell/coder workspace leaf {leaf:?}");
    }
    Ok(())
}

fn workspace_parent() -> PathBuf {
    let base = std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let base = fs::canonicalize(&base).unwrap_or(base);
    base.join("scenario-workspaces")
}

fn workspace_root(run_id: &str) -> PathBuf {
    workspace_parent().join(format!("shell-coder-{}", suffix(run_id)))
}

fn sandbox_name(run_id: &str) -> String {
    format!("e2e-shell-coder-{}", suffix(run_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harder_case_keeps_version_and_has_a_distinct_cohort() {
        let materialized = materialize("catalog", CANONICAL_SEED).unwrap();
        let rotated = materialize("catalog", 7).unwrap();
        assert_eq!(materialized.case.scenario_version, 5);
        assert_eq!(materialized.case.seed, CANONICAL_SEED);
        assert_eq!(rotated.case.case_id, materialized.case.case_id);
        assert_eq!(
            crate::scenarios::ScenarioId::ShellCoderSandbox.canonical_seed(),
            CANONICAL_SEED
        );
        assert_eq!(
            materialized.case.inputs["difficulty_profile"],
            DIFFICULTY_PROFILE
        );
        assert_eq!(
            materialized.case.inputs["fixture_repository"],
            FIXTURE_REPOSITORY
        );
        assert_eq!(
            materialized.case.inputs["fixture_revision"],
            FIXTURE_REVISION
        );
        assert_eq!(materialized.case.inputs["fixture_subtree"], FIXTURE_SUBTREE);
        assert_eq!(
            materialized.case.inputs["fixture_manifest_sha256"],
            FIXTURE_MANIFEST_SHA256
        );
        assert_eq!(
            materialized.case.complexity.tier,
            super::super::ComplexityTier::L4Coordinated
        );
        assert_eq!(
            ASSESSMENTS
                .iter()
                .map(|assessment| u16::from(assessment.weight()))
                .sum::<u16>(),
            100
        );
    }

    #[tokio::test]
    async fn reference_passes_hidden_runner_probe_and_demo() {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir_all(temporary.path().join("src")).unwrap();
        fs::write(temporary.path().join(SOURCE_PATH), REFERENCE_SOURCE).unwrap();
        let hidden = run_hidden_probe(temporary.path()).await.unwrap();
        let demo = run_python(temporary.path(), &[SOURCE_PATH, "--demo"])
            .await
            .unwrap();
        assert!(hidden.passed, "{:?}", hidden.checks);
        assert!(demo.success, "{}", demo.stderr);
        assert_eq!(demo.stdout.trim(), HOST_DEMO_STDOUT);
    }

    #[tokio::test]
    #[ignore = "requires the pinned HARNESS_E2E_FIXTURE_PATH checkout"]
    async fn pinned_external_fixture_is_valid_and_starts_red() {
        let assets = load_fixture_assets().unwrap();
        let temporary = tempfile::tempdir().unwrap();
        write_fixture(temporary.path(), &assets).unwrap();

        assert!(!run_public_tests(temporary.path()).await.unwrap().success);
        assert!(!run_hidden_probe(temporary.path()).await.unwrap().passed);
    }

    #[test]
    fn fixture_manifest_is_path_and_content_bound() {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir_all(temporary.path().join("src")).unwrap();
        fs::create_dir_all(temporary.path().join("tests")).unwrap();
        fs::write(temporary.path().join(TASK_PATH), "task\n").unwrap();
        fs::write(temporary.path().join(SOURCE_PATH), "source\n").unwrap();
        fs::write(temporary.path().join(PUBLIC_TEST_PATH), "tests\n").unwrap();

        assert_eq!(
            compute_fixture_manifest_sha256(temporary.path()).unwrap(),
            "sha256:406b314a5efd07a62e4ab3b51bdc54a88c4d9bef0c592b7d63e56ebe85ea3682"
        );
    }

    #[test]
    fn workspace_paths_reject_parent_traversal() {
        let root = Path::new("/tmp/workspace");
        assert!(workspace_path_matches(Some(SOURCE_PATH), root, SOURCE_PATH));
        assert!(!workspace_path_matches(
            Some("../reconcile.py"),
            root,
            SOURCE_PATH
        ));
    }

    #[test]
    fn shell_evidence_requires_every_argument_to_be_a_string() {
        let root = PathBuf::from("/tmp/scenario-workspaces/shell-coder-test");
        let exact = common::ObservedFunctionCall {
            function_id: "shell::exec".to_string(),
            arguments: json!({
                "command": "python3",
                "args": ["-m", "unittest", "discover", "-s", "tests", "-p", "test_*.py"]
            }),
        };
        assert!(is_public_test_call(&exact, &root));

        let malformed = common::ObservedFunctionCall {
            function_id: "shell::exec".to_string(),
            arguments: json!({
                "command": "python3",
                "args": ["-m", "unittest", "discover", "-s", "tests", "-p", 42]
            }),
        };
        assert!(!is_public_test_call(&malformed, &root));
    }
}
