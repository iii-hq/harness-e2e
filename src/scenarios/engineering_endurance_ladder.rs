//! Cumulative, Git-backed engineering benchmark that advances until the first
//! ticket the subject cannot complete within a bounded number of repair rounds.
//!
//! A runner-owned checkpoint function is the only authority that reveals the
//! next ticket. Each accepted checkpoint is a clean, immutable Git boundary and
//! has independently passed the public suite plus cumulative hidden probes.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use iii_sdk::runtime::FunctionRef;
use iii_sdk::RegisterFunction;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::process::Command;

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture, ComplexityProfile,
    DeliverableCaptureFuture, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase, ScenarioObservation,
    ScenarioSpec,
};

pub const ID: &str = "engineering_endurance_ladder";
const VERSION: u32 = 1;
pub const CANONICAL_SEED: u64 = 0x656e_6475_7261_0001;
const DELIVERABLE_ID: &str = "engineering_endurance_report";
const BRANCH: &str = "endurance-run";
const MAX_ATTEMPTS_PER_RUNG: u32 = 3;
const MAX_CHANGED_FILES_PER_RUNG: usize = 4;
const MAX_CHANGED_LINES_PER_RUNG: u64 = 700;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(90);
const PRODUCTION_PATH: &str = "src/durable_queue.py";
const PUBLIC_TEST_PATH: &str = "tests/test_durable_queue.py";
const MANIFEST_PATH: &str = ".harness-e2e/endurance-case.json";
const GITIGNORE_PATH: &str = ".gitignore";

const BASELINE_SOURCE: &str =
    include_str!("../../tests/fixtures/engineering-endurance-ladder/src/durable_queue.py");
const PUBLIC_TESTS: &str =
    include_str!("../../tests/fixtures/engineering-endurance-ladder/tests/test_durable_queue.py");
const CASE_MANIFEST: &str = include_str!(
    "../../tests/fixtures/engineering-endurance-ladder/.harness-e2e/endurance-case.json"
);
const GITIGNORE: &str =
    include_str!("../../tests/fixtures/engineering-endurance-ladder/.gitignore");

const CAPABILITY_DEPTH: AssessmentSpec = AssessmentSpec::score_only_in(
    "capability_depth",
    65,
    "Points scale with the longest cumulative prefix of independently accepted tickets.",
    EvaluationDimension::Deliverable,
);
const TERMINAL_PROTOCOL: AssessmentSpec = AssessmentSpec::hard_gated(
    "terminal_protocol",
    5,
    "The session reaches either all-rungs-complete or an evidence-backed first capability failure.",
);
const GIT_INTEGRITY: AssessmentSpec = AssessmentSpec::hard_gated(
    "git_checkpoint_integrity",
    10,
    "Every accepted checkpoint is an immutable, clean, non-merge descendant touching production only.",
);
const REGRESSION_INTEGRITY: AssessmentSpec = AssessmentSpec::hard_gated(
    "regression_integrity",
    10,
    "Every accepted checkpoint passed the public suite and all hidden probes through its rung.",
);
const CONVERGENCE: AssessmentSpec = AssessmentSpec::score_only(
    "repair_convergence",
    5,
    "Accepted tickets converge with few rejected checkpoint rounds.",
);
const EFFICIENCY: AssessmentSpec = AssessmentSpec::score_only(
    "change_efficiency",
    5,
    "The accepted implementation remains within a compact cumulative change budget.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    CAPABILITY_DEPTH,
    TERMINAL_PROTOCOL,
    GIT_INTEGRITY,
    REGRESSION_INTEGRITY,
    CONVERGENCE,
    EFFICIENCY,
];

#[derive(Debug, Clone, Copy)]
struct Ticket {
    id: &'static str,
    title: &'static str,
    body: &'static str,
}

const TICKETS: &[Ticket] = &[
    Ticket {
        id: "idempotent-submit",
        title: "Make submission idempotent",
        body: r#"Add durable idempotency to `submit(payload, idempotency_key=None)`. A non-empty key must return the original job id after reload without appending another submission. Reusing a key with a different payload must raise `ValueError`. Calls without a key keep creating distinct jobs. Preserve the public API and append-only JSONL durability."#,
    },
    Ticket {
        id: "retry-backoff",
        title: "Add claims and exponential retry backoff",
        body: r#"Add `claim(now, worker_id, lease_seconds=30)` and `fail(job_id, error, now, worker_id, base_delay=5, max_attempts=3)`. Claim the oldest eligible pending job and persist its owner. Failure increments `attempts`; before `max_attempts` it returns to pending with `available_at = now + base_delay * 2 ** (attempts - 1)`, otherwise it becomes terminal `failed`. Reload must preserve every field and ineligible jobs must not be claimed."#,
    },
    Ticket {
        id: "lease-recovery",
        title: "Recover expired worker leases safely",
        body: r#"Persist `lease_expires_at` on claim. Add `reap_expired(now)` returning the ids actually returned to pending; a lease expires when `now >= lease_expires_at`. Cancelled or terminal jobs are never reaped. Completing or failing a claimed job must verify `worker_id`; introduce `LeaseConflict` for a wrong or absent owner. Keep the original `complete(job_id)` behavior for jobs that were never claimed."#,
    },
    Ticket {
        id: "truncated-journal",
        title: "Recover a crash-truncated journal tail",
        body: r#"Introduce `JournalCorruptError`. Startup may ignore exactly one malformed, non-empty final line caused by a partial append, while malformed JSON anywhere else must raise `JournalCorruptError`. Add `repair_truncated_tail()` which durably removes only that ignored tail and returns whether a repair occurred. Valid events must never be discarded and the repaired file must end with a newline."#,
    },
    Ticket {
        id: "cancellation",
        title: "Add durable idempotent cancellation",
        body: r#"Add `cancel(job_id, reason=None) -> bool`. Pending or running work becomes terminal `cancelled`, clears lease metadata, persists the optional reason, and returns true. Repeated cancellation returns false without appending. Completed or failed work also returns false. Cancelled jobs must survive reload and never be claimed or reaped."#,
    },
    Ticket {
        id: "optimistic-revisions",
        title: "Protect mutations with optimistic revisions",
        body: r#"Every job must expose a positive integer `revision`, starting at 1 and incrementing exactly once for each persisted state transition. Introduce `RevisionConflict`. Mutating methods (`claim`, `fail`, `complete`, `cancel`) must accept optional `expected_revision`; when supplied and stale, raise `RevisionConflict` before appending or changing memory. Reload must reconstruct the exact revision."#,
    },
    Ticket {
        id: "legacy-migration",
        title: "Migrate legacy v1 journal events",
        body: r#"Support legacy submitted events that use `data` instead of `payload` and omit revision/attempt scheduling fields. Add `migrate() -> int` that atomically rewrites a mixed valid journal into the current schema and returns the number of legacy events converted. A second migration is a byte-for-byte no-op returning zero. Preserve job ids, payloads, ordering, idempotency keys, and observable state."#,
    },
    Ticket {
        id: "atomic-compaction",
        title: "Compact the journal atomically",
        body: r#"Add `compact()`. Rewrite the journal through a same-directory temporary file plus flush/fsync and atomic replace. After reload, jobs, states, retries, revisions, leases, cancellation reasons, and idempotency behavior must be identical. The compacted journal should contain at most one snapshot event per job plus one metadata event, and no temporary file may remain after success."#,
    },
    Ticket {
        id: "batch-claim",
        title: "Add deterministic batch claims",
        body: r#"Add `claim_batch(now, worker_id, limit, lease_seconds=30)`. Reject negative limits; zero returns an empty list without mutation. Return at most `limit` eligible jobs, ordered by `available_at`, then original submission order, then id. Each selected job must undergo the same durable transition as `claim`, with no job selected twice. `claim` should remain behaviorally equivalent to a batch of one."#,
    },
    Ticket {
        id: "operational-stats",
        title: "Expose deterministic operational statistics",
        body: r#"Add non-mutating `stats(now)`. Return counts for every known status, `eligible_pending`, `leased_running`, `expired_running`, `oldest_eligible_age`, and `total`. Age is `max(0, now - submitted_at)` and is null when nothing is eligible. Persist `submitted_at` (defaulting submit's optional `now` to 0 for compatibility). Calling stats must not append, reorder, repair, or otherwise change journal bytes."#,
    },
];

// This is deliberately kept runner-side. Tickets describe behavior; this probe
// is independent evidence and is never materialized into the subject workspace.
const HIDDEN_PROBE: &str = r#"
import json, sys, tempfile
from pathlib import Path

from src.durable_queue import DurableQueue

rung = int(sys.argv[1])

def queue():
    temporary = tempfile.TemporaryDirectory()
    path = Path(temporary.name) / 'queue.jsonl'
    return temporary, path, DurableQueue(path)

def expect_raises(name, operation):
    try:
        operation()
    except Exception as error:
        assert error.__class__.__name__ == name, (name, type(error).__name__, str(error))
    else:
        raise AssertionError(f'expected {name}')

checks = []

if rung >= 1:
    t, path, q = queue()
    first = q.submit({'x': 1}, 'key-a')
    assert q.submit({'x': 1}, 'key-a') == first
    assert DurableQueue(path).submit({'x': 1}, 'key-a') == first
    expect_raises('ValueError', lambda: q.submit({'x': 2}, 'key-a'))
    assert q.submit({'x': 1}) != q.submit({'x': 1})
    assert sum(json.loads(line)['type'] == 'submitted' for line in path.read_text().splitlines()) == 3
    checks.append('idempotent-submit')

if rung >= 2:
    t, path, q = queue()
    job = q.submit({'x': 1})
    claimed = q.claim(now=100, worker_id='a')
    assert claimed['id'] == job and claimed['status'] == 'running'
    q.fail(job, 'transient', now=101, worker_id='a', base_delay=5, max_attempts=3)
    state = DurableQueue(path).get(job)
    assert state['status'] == 'pending' and state['attempts'] == 1 and state['available_at'] == 106
    assert q.claim(now=105, worker_id='b') is None
    assert q.claim(now=106, worker_id='b')['id'] == job
    q.fail(job, 'again', now=107, worker_id='b', base_delay=5, max_attempts=2)
    assert q.get(job)['status'] == 'failed'
    checks.append('retry-backoff')

if rung >= 3:
    t, path, q = queue()
    job = q.submit({'x': 1})
    q.claim(now=10, worker_id='a', lease_seconds=5)
    assert q.reap_expired(14) == []
    assert q.reap_expired(15) == [job]
    assert q.reap_expired(16) == []
    q.claim(now=16, worker_id='b')
    expect_raises('LeaseConflict', lambda: q.complete(job, worker_id='a'))
    q.complete(job, worker_id='b')
    assert DurableQueue(path).get(job)['status'] == 'completed'
    checks.append('lease-recovery')

if rung >= 4:
    t, path, q = queue()
    job = q.submit({'x': 1})
    with path.open('ab') as stream: stream.write(b'{"type":"sub')
    recovered = DurableQueue(path)
    assert recovered.get(job)['id'] == job
    assert recovered.repair_truncated_tail() is True
    assert path.read_bytes().endswith(b'\n') and b'{"type":"sub' not in path.read_bytes()
    assert recovered.repair_truncated_tail() is False
    path.write_text('{bad}\n' + json.dumps({'type':'submitted','job_id':'ok','payload':{}}) + '\n')
    expect_raises('JournalCorruptError', lambda: DurableQueue(path))
    checks.append('truncated-journal')

if rung >= 5:
    t, path, q = queue()
    pending = q.submit({'x': 1}); running = q.submit({'x': 2}); done = q.submit({'x': 3})
    q.claim(now=1, worker_id='a')
    assert q.cancel(pending, 'operator') is True and q.cancel(pending, 'again') is False
    q.claim(now=2, worker_id='b'); assert q.cancel(running) is True
    q.complete(done); assert q.cancel(done) is False
    reloaded = DurableQueue(path)
    assert reloaded.get(pending)['status'] == 'cancelled'
    assert reloaded.get(running)['status'] == 'cancelled'
    assert reloaded.claim(now=100, worker_id='z') is None
    assert reloaded.reap_expired(100) == []
    checks.append('cancellation')

if rung >= 6:
    t, path, q = queue()
    job = q.submit({'x': 1}); assert q.get(job)['revision'] == 1
    q.claim(now=1, worker_id='a', expected_revision=1)
    assert q.get(job)['revision'] == 2
    before = path.read_bytes()
    expect_raises('RevisionConflict', lambda: q.cancel(job, expected_revision=1))
    assert path.read_bytes() == before and q.get(job)['revision'] == 2
    q.complete(job, worker_id='a', expected_revision=2)
    assert DurableQueue(path).get(job)['revision'] == 3
    checks.append('optimistic-revisions')

if rung >= 7:
    t, path, q = queue()
    legacy = json.dumps({'type':'submitted','job_id':'legacy','data':{'v':1},'idempotency_key':'old'})
    current = json.dumps({'type':'submitted','job_id':'current','payload':{'v':2},'idempotency_key':'new','revision':1,'attempts':0,'available_at':0,'submitted_at':0})
    path.write_text(legacy + '\n' + current + '\n')
    q = DurableQueue(path)
    assert q.get('legacy')['payload'] == {'v':1}
    assert q.migrate() == 1
    first = path.read_bytes(); assert q.migrate() == 0 and path.read_bytes() == first
    reloaded = DurableQueue(path)
    assert reloaded.submit({'v':1}, 'old') == 'legacy'
    assert [j['id'] for j in reloaded.list_jobs()] == ['legacy','current']
    checks.append('legacy-migration')

if rung >= 8:
    t, path, q = queue()
    one = q.submit({'x':1}, 'one', now=1); two = q.submit({'x':2}, 'two', now=2)
    q.claim(now=3, worker_id='a'); q.fail(one, 'x', now=4, worker_id='a')
    q.cancel(two, 'stop')
    expected = q.list_jobs(); q.compact()
    assert DurableQueue(path).list_jobs() == expected
    assert len(path.read_text().splitlines()) <= len(expected) + 1
    assert not list(path.parent.glob(path.name + '*.tmp'))
    assert DurableQueue(path).submit({'x':1}, 'one') == one
    checks.append('atomic-compaction')

if rung >= 9:
    t, path, q = queue()
    ids = [q.submit({'n': n}, now=n) for n in [3,1,2,1]]
    before = path.read_bytes(); assert q.claim_batch(10, 'a', 0) == [] and path.read_bytes() == before
    expect_raises('ValueError', lambda: q.claim_batch(10, 'a', -1))
    claimed = q.claim_batch(10, 'a', 3)
    assert [item['id'] for item in claimed] == [ids[1], ids[3], ids[2]]
    assert q.claim(10, 'b')['id'] == ids[0] and q.claim(10, 'b') is None
    assert len({item['id'] for item in claimed}) == 3
    checks.append('batch-claim')

if rung >= 10:
    t, path, q = queue()
    eligible = q.submit({'x':1}, now=5); leased = q.submit({'x':2}, now=7); future = q.submit({'x':3}, now=9)
    q.claim(now=10, worker_id='a', lease_seconds=5)
    q.claim(now=10, worker_id='b', lease_seconds=100)
    # Return one claimed job to delayed pending without making it terminal.
    q.fail(leased, 'later', now=10, worker_id='b', base_delay=50)
    before = path.read_bytes(); stats = q.stats(now=20); assert path.read_bytes() == before
    assert stats['total'] == 3 and stats['running'] == 1 and stats['pending'] == 2
    assert stats['expired_running'] == 1 and stats['leased_running'] == 0
    assert stats['eligible_pending'] == 1 and stats['oldest_eligible_age'] == 11
    checks.append('operational-stats')

print(json.dumps({'passed': True, 'rung': rung, 'checks': checks}, sort_keys=True))
"#;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CheckpointRequest {
    pub rung: u32,
    pub head_sha: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CheckpointResponse {
    pub status: String,
    pub accepted_rungs: u32,
    pub accepted_head: String,
    pub feedback: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_ticket: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ValidationEvidence {
    public_tests_passed: bool,
    hidden_probes_passed: bool,
    worktree_clean: bool,
    branch_valid: bool,
    refs_valid: bool,
    git_config_valid: bool,
    remotes_valid: bool,
    ancestry_valid: bool,
    non_merge_commits: u64,
    changed_paths: Vec<String>,
    changed_lines: u64,
    scope_valid: bool,
    public_output: String,
    hidden_output: String,
}

impl ValidationEvidence {
    fn accepted(&self) -> bool {
        self.public_tests_passed
            && self.hidden_probes_passed
            && self.worktree_clean
            && self.branch_valid
            && self.refs_valid
            && self.git_config_valid
            && self.remotes_valid
            && self.ancestry_valid
            && self.non_merge_commits > 0
            && self.scope_valid
    }

    fn feedback(&self) -> String {
        let mut failures = Vec::new();
        if !self.public_tests_passed {
            failures.push(format!(
                "public tests failed: {}",
                bounded(&self.public_output, 1_500)
            ));
        }
        if !self.hidden_probes_passed {
            failures.push(format!(
                "hidden acceptance failed: {}",
                bounded(&self.hidden_output, 1_500)
            ));
        }
        if !self.worktree_clean {
            failures.push("worktree is not clean; commit the intended production change".into());
        }
        if !self.branch_valid {
            failures.push(format!("branch must remain `{BRANCH}`"));
        }
        if !self.refs_valid {
            failures.push("refs changed; do not create branches or tags".into());
        }
        if !self.git_config_valid {
            failures.push("local Git configuration changed after fixture setup".into());
        }
        if !self.remotes_valid {
            failures.push("the disposable repository must not have remotes".into());
        }
        if !self.ancestry_valid {
            failures.push("accepted checkpoint is no longer an ancestor of HEAD".into());
        }
        if self.non_merge_commits == 0 {
            failures.push("the rung requires one or more new non-merge commits".into());
        }
        if !self.scope_valid {
            failures.push(format!(
                "scope or budget invalid: paths={:?}, changed_lines={} (max files={}, max lines={})",
                self.changed_paths,
                self.changed_lines,
                MAX_CHANGED_FILES_PER_RUNG,
                MAX_CHANGED_LINES_PER_RUNG
            ));
        }
        failures.join("; ")
    }
}

#[derive(Debug, Clone, Serialize)]
struct CheckpointRecord {
    rung: u32,
    ticket_id: String,
    attempt: u32,
    requested_head: String,
    previous_accepted_head: String,
    observed_at_ms: u64,
    duration_ms: u64,
    accepted: bool,
    feedback: String,
    evidence: ValidationEvidence,
}

#[derive(Debug, Clone, Serialize)]
struct EnduranceSnapshot {
    initial_head: String,
    accepted_head: String,
    accepted_rungs: u32,
    terminal_status: Option<String>,
    terminal_rung: Option<u32>,
    started_at_ms: u64,
    elapsed_ms: u64,
    records: Vec<CheckpointRecord>,
}

struct EnduranceState {
    initial_head: String,
    accepted_head: String,
    accepted_rungs: u32,
    expected_git_config: String,
    attempts: HashMap<u32, u32>,
    terminal_status: Option<String>,
    terminal_rung: Option<u32>,
    started_at_ms: u64,
    records: Vec<CheckpointRecord>,
}

struct FixtureRuntime {
    function: FunctionRef,
    state: Arc<Mutex<EnduranceState>>,
}

static FIXTURES: OnceLock<Mutex<HashMap<String, FixtureRuntime>>> = OnceLock::new();

fn fixture_registry() -> &'static Mutex<HashMap<String, FixtureRuntime>> {
    FIXTURES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn bounded(value: &str, maximum: usize) -> String {
    let mut output = value.chars().take(maximum).collect::<String>();
    if value.chars().count() > maximum {
        output.push_str("...");
    }
    output
}

fn checkpoint_function_id(run_id: &str) -> String {
    format!(
        "e2etest::engineering_endurance_checkpoint_{}",
        run_suffix(run_id)
    )
}

fn run_suffix(run_id: &str) -> String {
    format!("{:016x}", super::stable_seed(&format!("{ID}:{run_id}")))
}

pub fn required_functions(run_id: &str) -> Vec<String> {
    vec![checkpoint_function_id(run_id)]
}

pub fn allowed_functions(run_id: &str) -> Vec<String> {
    vec![
        checkpoint_function_id(run_id),
        "engine::functions::list".into(),
        "engine::functions::info".into(),
        "coder::*".into(),
        "shell::*".into(),
    ]
}

fn fixture_root(run_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join("harness-e2e-engineering-endurance")
        .join(run_suffix(run_id))
}

fn ensure_safe_root(root: &Path) -> Result<()> {
    let parent = std::env::temp_dir().join("harness-e2e-engineering-endurance");
    if root.parent() != Some(parent.as_path()) {
        bail!(
            "refusing endurance fixture operation outside {}",
            parent.display()
        );
    }
    let leaf = root
        .file_name()
        .and_then(|value| value.to_str())
        .context("endurance fixture has no UTF-8 leaf")?;
    if leaf.is_empty()
        || !leaf
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("unsafe endurance fixture leaf {leaf:?}");
    }
    Ok(())
}

fn remove_fixture(root: &Path) -> Result<()> {
    ensure_safe_root(root)?;
    let Ok(metadata) = fs::symlink_metadata(root) else {
        return Ok(());
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(root)
            .with_context(|| format!("remove endurance fixture {}", root.display()))?;
    } else {
        fs::remove_file(root)
            .with_context(|| format!("remove endurance fixture link {}", root.display()))?;
    }
    Ok(())
}

fn write_fixture(root: &Path) -> Result<()> {
    ensure_safe_root(root)?;
    remove_fixture(root)?;
    for (relative, content) in [
        (PRODUCTION_PATH, BASELINE_SOURCE),
        (PUBLIC_TEST_PATH, PUBLIC_TESTS),
        (MANIFEST_PATH, CASE_MANIFEST),
        (GITIGNORE_PATH, GITIGNORE),
    ] {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().context("fixture path has no parent")?)?;
        fs::write(&path, content)
            .with_context(|| format!("write endurance fixture {}", path.display()))?;
    }
    Ok(())
}

#[derive(Debug)]
struct CommandOutcome {
    success: bool,
    stdout: String,
    stderr: String,
}

async fn run_command(root: &Path, program: &str, args: &[&str]) -> Result<CommandOutcome> {
    let output = tokio::time::timeout(
        COMMAND_TIMEOUT,
        Command::new(program)
            .args(args)
            .current_dir(root)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .with_context(|| format!("{program} command timed out"))?
    .with_context(|| format!("launch {program}"))?;
    Ok(CommandOutcome {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

async fn git(root: &Path, args: &[&str]) -> Result<CommandOutcome> {
    run_command(root, "git", args).await
}

async fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let output = git(root, args).await?;
    if !output.success {
        bail!("git {} failed: {}", args.join(" "), output.stderr.trim());
    }
    Ok(output.stdout.trim().to_string())
}

async fn initialize_git(root: &Path) -> Result<String> {
    let init = git(root, &["init", "-q", "-b", BRANCH]).await?;
    if !init.success {
        bail!("git init failed: {}", init.stderr);
    }
    for args in [
        &["config", "user.name", "Harness E2E Fixture"][..],
        &["config", "user.email", "harness-e2e@invalid.local"][..],
        &["config", "commit.gpgsign", "false"][..],
        &[
            "add",
            "--",
            GITIGNORE_PATH,
            MANIFEST_PATH,
            PRODUCTION_PATH,
            PUBLIC_TEST_PATH,
        ][..],
        &["commit", "-q", "-m", "Initialize durable queue fixture"][..],
    ] {
        let output = git(root, args).await?;
        if !output.success {
            bail!("git {} failed: {}", args.join(" "), output.stderr.trim());
        }
    }
    git_text(root, &["rev-parse", "HEAD"]).await
}

async fn audit_checkpoint(
    root: &Path,
    previous: &str,
    rung: u32,
    expected_git_config: &str,
) -> Result<ValidationEvidence> {
    let status = git_text(root, &["status", "--porcelain=v1", "--untracked-files=all"]).await?;
    let branch = git_text(root, &["symbolic-ref", "--short", "HEAD"]).await?;
    let refs = git_text(root, &["for-each-ref", "--format=%(refname)"]).await?;
    let git_config = git_text(root, &["config", "--local", "--list"]).await?;
    let remotes = git_text(root, &["remote"]).await?;
    let ancestry = git(root, &["merge-base", "--is-ancestor", previous, "HEAD"]).await?;
    let commits = git_text(root, &["rev-list", "--count", &format!("{previous}..HEAD")])
        .await?
        .parse::<u64>()
        .unwrap_or_default();
    let merges = git_text(
        root,
        &["rev-list", "--merges", &format!("{previous}..HEAD")],
    )
    .await?;
    let changed = git_text(root, &["diff", "--name-only", previous, "HEAD"]).await?;
    let changed_paths = changed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let numstat = git_text(root, &["diff", "--numstat", previous, "HEAD"]).await?;
    let changed_lines = numstat
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let added = fields.next()?.parse::<u64>().ok()?;
            let removed = fields.next()?.parse::<u64>().ok()?;
            Some(added.saturating_add(removed))
        })
        .sum();
    let expected_regular_paths = [
        PRODUCTION_PATH,
        PUBLIC_TEST_PATH,
        MANIFEST_PATH,
        GITIGNORE_PATH,
    ]
    .into_iter()
    .all(|relative| {
        fs::symlink_metadata(root.join(relative))
            .ok()
            .is_some_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
    });
    let protected_exact = expected_regular_paths
        && fs::read_to_string(root.join(PUBLIC_TEST_PATH))
            .ok()
            .as_deref()
            == Some(PUBLIC_TESTS)
        && fs::read_to_string(root.join(MANIFEST_PATH)).ok().as_deref() == Some(CASE_MANIFEST)
        && fs::read_to_string(root.join(GITIGNORE_PATH))
            .ok()
            .as_deref()
            == Some(GITIGNORE);
    let scope_valid = protected_exact
        && !changed_paths.is_empty()
        && changed_paths.len() <= MAX_CHANGED_FILES_PER_RUNG
        && changed_paths.iter().all(|path| path == PRODUCTION_PATH)
        && changed_lines <= MAX_CHANGED_LINES_PER_RUNG;

    let public = run_command(
        root,
        "python3",
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
    .await?;
    let hidden = run_command(root, "python3", &["-c", HIDDEN_PROBE, &rung.to_string()]).await?;

    Ok(ValidationEvidence {
        public_tests_passed: public.success,
        hidden_probes_passed: hidden.success,
        worktree_clean: status.is_empty(),
        branch_valid: branch == BRANCH,
        refs_valid: refs == format!("refs/heads/{BRANCH}"),
        git_config_valid: git_config == expected_git_config,
        remotes_valid: remotes.is_empty(),
        ancestry_valid: ancestry.success && merges.is_empty(),
        non_merge_commits: if merges.is_empty() { commits } else { 0 },
        changed_paths,
        changed_lines,
        scope_valid,
        public_output: format!("{}{}", public.stdout, public.stderr),
        hidden_output: format!("{}{}", hidden.stdout, hidden.stderr),
    })
}

fn ticket_text(rung: usize) -> String {
    let ticket = TICKETS[rung - 1];
    format!(
        "Rung {rung}/{} — {} (`{}`)\n\n{}\n\nImplement only in `{PRODUCTION_PATH}`. Run the complete public suite, commit the change, then call the checkpoint function with rung={rung} and the exact committed HEAD SHA.",
        TICKETS.len(), ticket.title, ticket.id, ticket.body
    )
}

async fn handle_checkpoint(
    root: PathBuf,
    state: Arc<Mutex<EnduranceState>>,
    request: CheckpointRequest,
) -> Result<CheckpointResponse, iii_sdk::errors::Error> {
    let (expected_rung, previous, expected_git_config, terminal) = {
        let state = lock_unpoisoned(&state);
        (
            state.accepted_rungs + 1,
            state.accepted_head.clone(),
            state.expected_git_config.clone(),
            state.terminal_status.clone(),
        )
    };
    if let Some(status) = terminal {
        let state = lock_unpoisoned(&state);
        return Ok(CheckpointResponse {
            status,
            accepted_rungs: state.accepted_rungs,
            accepted_head: state.accepted_head.clone(),
            feedback: "the endurance ladder is already terminal".into(),
            next_ticket: None,
        });
    }
    if request.rung != expected_rung || request.rung == 0 || request.rung as usize > TICKETS.len() {
        let state = lock_unpoisoned(&state);
        return Ok(CheckpointResponse {
            status: "rejected".into(),
            accepted_rungs: state.accepted_rungs,
            accepted_head: state.accepted_head.clone(),
            feedback: format!("expected rung {expected_rung}; received {}", request.rung),
            next_ticket: None,
        });
    }

    let observed_head = git_text(&root, &["rev-parse", "HEAD"])
        .await
        .unwrap_or_default();
    let started = now_ms();
    let mut evidence =
        match audit_checkpoint(&root, &previous, request.rung, &expected_git_config).await {
            Ok(evidence) => evidence,
            Err(error) => ValidationEvidence {
                public_tests_passed: false,
                hidden_probes_passed: false,
                worktree_clean: false,
                branch_valid: false,
                refs_valid: false,
                git_config_valid: false,
                remotes_valid: false,
                ancestry_valid: false,
                non_merge_commits: 0,
                changed_paths: Vec::new(),
                changed_lines: 0,
                scope_valid: false,
                public_output: String::new(),
                hidden_output: format!("checkpoint audit failed: {error:#}"),
            },
        };
    if request.head_sha != observed_head
        || request.head_sha.len() != 40
        || !request
            .head_sha
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        evidence.ancestry_valid = false;
        evidence.hidden_output.push_str(&format!(
            "\nrequest head {:?} does not match observed HEAD {:?}",
            request.head_sha, observed_head
        ));
    }
    let accepted = evidence.accepted();
    let feedback = if accepted {
        "public suite, cumulative hidden probes, Git ancestry, scope, and cleanliness accepted"
            .to_string()
    } else {
        evidence.feedback()
    };

    let mut state = lock_unpoisoned(&state);
    let attempt = {
        let attempts = state.attempts.entry(request.rung).or_default();
        *attempts += 1;
        *attempts
    };
    state.records.push(CheckpointRecord {
        rung: request.rung,
        ticket_id: TICKETS[request.rung as usize - 1].id.to_string(),
        attempt,
        requested_head: request.head_sha.clone(),
        previous_accepted_head: previous,
        observed_at_ms: started,
        duration_ms: now_ms().saturating_sub(started),
        accepted,
        feedback: feedback.clone(),
        evidence,
    });

    if accepted {
        state.accepted_rungs = request.rung;
        state.accepted_head = observed_head;
        if state.accepted_rungs as usize == TICKETS.len() {
            state.terminal_status = Some("completed".into());
            state.terminal_rung = Some(request.rung);
            return Ok(CheckpointResponse {
                status: "completed".into(),
                accepted_rungs: state.accepted_rungs,
                accepted_head: state.accepted_head.clone(),
                feedback,
                next_ticket: None,
            });
        }
        let next_rung = state.accepted_rungs as usize + 1;
        return Ok(CheckpointResponse {
            status: "accepted".into(),
            accepted_rungs: state.accepted_rungs,
            accepted_head: state.accepted_head.clone(),
            feedback,
            next_ticket: Some(ticket_text(next_rung)),
        });
    }

    if attempt >= MAX_ATTEMPTS_PER_RUNG {
        state.terminal_status = Some("capability_failure".into());
        state.terminal_rung = Some(request.rung);
        return Ok(CheckpointResponse {
            status: "capability_failure".into(),
            accepted_rungs: state.accepted_rungs,
            accepted_head: state.accepted_head.clone(),
            feedback,
            next_ticket: None,
        });
    }
    Ok(CheckpointResponse {
        status: "rejected".into(),
        accepted_rungs: state.accepted_rungs,
        accepted_head: state.accepted_head.clone(),
        feedback,
        next_ticket: None,
    })
}

fn release_fixture(run_id: &str) {
    if let Some(runtime) = lock_unpoisoned(fixture_registry()).remove(run_id) {
        runtime.function.unregister();
    }
}

fn snapshot(run_id: &str) -> Option<EnduranceSnapshot> {
    let registry = lock_unpoisoned(fixture_registry());
    let runtime = registry.get(run_id)?;
    let state = lock_unpoisoned(&runtime.state);
    Some(EnduranceSnapshot {
        initial_head: state.initial_head.clone(),
        accepted_head: state.accepted_head.clone(),
        accepted_rungs: state.accepted_rungs,
        terminal_status: state.terminal_status.clone(),
        terminal_rung: state.terminal_rung,
        started_at_ms: state.started_at_ms,
        elapsed_ms: now_ms().saturating_sub(state.started_at_ms),
        records: state.records.clone(),
    })
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        release_fixture(run_id);
        let root = fixture_root(run_id);
        write_fixture(&root)?;
        let initial_head = initialize_git(&root).await?;
        let expected_git_config = git_text(&root, &["config", "--local", "--list"]).await?;
        let public = run_command(
            &root,
            "python3",
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
        .await?;
        if !public.success {
            bail!("endurance baseline public suite is red: {}", public.stderr);
        }

        let state = Arc::new(Mutex::new(EnduranceState {
            initial_head: initial_head.clone(),
            accepted_head: initial_head,
            accepted_rungs: 0,
            expected_git_config,
            attempts: HashMap::new(),
            terminal_status: None,
            terminal_rung: None,
            started_at_ms: now_ms(),
            records: Vec::new(),
        }));
        let function_id = checkpoint_function_id(run_id);
        let handler_root = root.clone();
        let handler_state = Arc::clone(&state);
        let function = context.client().register_function(
            function_id,
            RegisterFunction::new_async(move |request: CheckpointRequest| {
                let root = handler_root.clone();
                let state = Arc::clone(&handler_state);
                async move { handle_checkpoint(root, state, request).await }
            })
            .description(
                "Trusted engineering endurance checkpoint. Audits a committed Git rung, runs public and cumulative hidden probes, and reveals the next ticket only after acceptance.",
            ),
        );
        lock_unpoisoned(fixture_registry())
            .insert(run_id.to_string(), FixtureRuntime { function, state });
        Ok(())
    })
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        release_fixture(run_id);
        remove_fixture(&fixture_root(run_id))?;
        Ok(())
    })
}

fn scenario_for_case(run_id: &str) -> ScenarioSpec {
    let root = fixture_root(run_id);
    let checkpoint = checkpoint_function_id(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            r#"You are the sole implementer in a cumulative engineering endurance run. Work in the
isolated Git repository `{}`. Read `{MANIFEST_PATH}`, the source, and the public tests before
editing. Network and GitHub access are forbidden.

{} 

For every rung:
1. inspect and reproduce the current behavior;
2. edit production code only (never tests, manifest, refs, branch, or Git configuration);
3. run `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p 'test_*.py'`;
4. create one or more non-merge commits and leave the worktree clean;
5. call `{checkpoint}` with the rung number and exact `git rev-parse HEAD` SHA.

On `rejected`, repair in this same session and checkpoint the same rung again; you may replace
unaccepted commits, but never rewrite an accepted checkpoint. On `accepted`, implement exactly
the `next_ticket` returned by the trusted checkpoint. On `completed` or `capability_failure`, stop
and report accepted rung count, terminal rung, and last accepted SHA. Do not invent future tickets
or stop merely because one checkpoint was rejected."#,
            root.display(),
            ticket_text(1),
        ),
        filesystem_root: Some(root),
        execution: ExecutionPolicy {
            max_turns: 320,
            max_output_tokens: Some(32_768),
            max_total_tokens: Some(3_000_000),
            stuck_timeout_seconds: 1_200,
            max_validation_retries: None,
        },
        denied_functions: &[
            "web::*",
            "scrapling::*",
            "http::*",
            "github::*",
            "harness::spawn",
            "harness::trigger::*",
            "state::*",
            "database::*",
        ],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn materialize(namespace: &str, _seed: u64) -> Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        ID,
        VERSION,
        CANONICAL_SEED,
        json!({
            "task": "cumulative-durable-queue-engineering",
            "language": "python",
            "rungs": TICKETS.iter().enumerate().map(|(index, ticket)| json!({
                "rung": index + 1,
                "id": ticket.id,
                "title": ticket.title,
            })).collect::<Vec<_>>(),
            "max_attempts_per_rung": MAX_ATTEMPTS_PER_RUNG,
            "production_paths": [PRODUCTION_PATH],
            "protected_paths": [PUBLIC_TEST_PATH, MANIFEST_PATH, GITIGNORE_PATH],
            "termination": "first_rung_with_three_rejected_checkpoints_or_all_complete",
            "github_handoff": {
                "repository": "iii-hq/e2e-fixture",
                "publisher": "trusted_runner_only",
                "branch_prefix": "benchmark-runs/endurance/",
                "subject_credentials": false,
            },
        }),
        ComplexityProfile {
            planning_depth: 10,
            dependency_depth: 8,
            state_transitions: 10,
            validation_loops: 10,
            artifact_count: 1,
            ambiguity_level: 6,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".into(),
            "iii::functions".into(),
            "e2e::filesystem".into(),
            "e2e::shell".into(),
            "e2e::git".into(),
            "python3".into(),
            "github::trusted-handoff".into(),
        ],
        deliverable_contract(),
    )?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(namespace),
        case,
        capture: Some(capture),
    })
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    _observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let snapshot = snapshot(run_id).context("endurance fixture state is unavailable")?;
        let terminal = matches!(
            snapshot.terminal_status.as_deref(),
            Some("completed" | "capability_failure")
        );
        let accepted = snapshot
            .records
            .iter()
            .filter(|record| record.accepted)
            .collect::<Vec<_>>();
        let git_integrity = accepted.iter().all(|record| {
            record.evidence.worktree_clean
                && record.evidence.branch_valid
                && record.evidence.refs_valid
                && record.evidence.git_config_valid
                && record.evidence.remotes_valid
                && record.evidence.ancestry_valid
                && record.evidence.non_merge_commits > 0
                && record.evidence.scope_valid
        });
        let regression_integrity = accepted.iter().all(|record| {
            record.evidence.public_tests_passed && record.evidence.hidden_probes_passed
        });
        let depth_points = ((u64::from(snapshot.accepted_rungs)
            * u64::from(CAPABILITY_DEPTH.weight()))
            / TICKETS.len() as u64) as u8;
        let rejection_count = snapshot
            .records
            .iter()
            .filter(|record| !record.accepted)
            .count();
        let convergence_points = if snapshot.accepted_rungs == 0 {
            0
        } else {
            CONVERGENCE
                .weight()
                .saturating_sub(rejection_count.min(CONVERGENCE.weight() as usize) as u8)
        };
        let changed_lines: u64 = accepted
            .iter()
            .map(|record| record.evidence.changed_lines)
            .sum();
        let efficiency_points = if snapshot.accepted_rungs == 0 {
            0
        } else if changed_lines <= u64::from(snapshot.accepted_rungs) * 250 {
            EFFICIENCY.weight()
        } else if changed_lines <= u64::from(snapshot.accepted_rungs) * 500 {
            3
        } else {
            1
        };
        Ok(assessment::build_evaluation([
            CAPABILITY_DEPTH.award(
                depth_points,
                format!(
                    "accepted {}/{} cumulative rungs",
                    snapshot.accepted_rungs,
                    TICKETS.len()
                ),
            )?,
            TERMINAL_PROTOCOL.full_or_zero(
                terminal,
                format!(
                    "terminal_status={:?}, terminal_rung={:?}",
                    snapshot.terminal_status, snapshot.terminal_rung
                ),
            ),
            GIT_INTEGRITY.full_or_zero(
                git_integrity,
                format!(
                    "{} accepted checkpoint(s) retained clean Git scope and ancestry",
                    accepted.len()
                ),
            ),
            REGRESSION_INTEGRITY.full_or_zero(
                regression_integrity,
                format!(
                    "{} accepted checkpoint(s) passed public and cumulative hidden probes",
                    accepted.len()
                ),
            ),
            CONVERGENCE.award(
                convergence_points,
                format!(
                    "{} rejected round(s) across {} accepted rung(s)",
                    rejection_count, snapshot.accepted_rungs
                ),
            )?,
            EFFICIENCY.award(
                efficiency_points,
                format!(
                    "{} changed line(s) across accepted rung ranges",
                    changed_lines
                ),
            )?,
        ]))
    })
}

fn capture<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let snapshot = snapshot(run_id).context("endurance fixture state is unavailable")?;
        let accepted_records = snapshot
            .records
            .iter()
            .filter(|record| record.accepted)
            .count();
        let rejected_records = snapshot.records.len().saturating_sub(accepted_records);
        let total_changed_lines: u64 = snapshot
            .records
            .iter()
            .filter(|record| record.accepted)
            .map(|record| record.evidence.changed_lines)
            .sum();
        let terminal = matches!(
            snapshot.terminal_status.as_deref(),
            Some("completed" | "capability_failure")
        );
        let integrity = snapshot
            .records
            .iter()
            .filter(|record| record.accepted)
            .all(|record| record.evidence.accepted());
        let accepted_patch = git_text(
            &fixture_root(run_id),
            &[
                "diff",
                "--binary",
                &snapshot.initial_head,
                &snapshot.accepted_head,
            ],
        )
        .await
        .unwrap_or_default();
        let github_handoff = json!({
            "kind": "engineering-endurance-github-handoff",
            "version": 1,
            "repository": "iii-hq/e2e-fixture",
            "base_sha": snapshot.initial_head,
            "head_sha": snapshot.accepted_head,
            "suggested_branch": format!("benchmark-runs/endurance/{}", run_suffix(run_id)),
            "draft_pr": true,
            "check_name": "Harness E2E / engineering endurance",
            "publisher": "trusted_runner",
            "subject_had_github_credentials": false,
        });
        let totals = &observation.metrics.totals;
        let mut measurements = vec![
            json!({"id": "max_accepted_rung", "value": snapshot.accepted_rungs, "unit": "rungs"}),
            json!({"id": "accepted_tickets", "value": accepted_records, "unit": "tickets"}),
            json!({"id": "checkpoint_rejections", "value": rejected_records, "unit": "checkpoints"}),
            json!({"id": "time_to_boundary_ms", "value": snapshot.elapsed_ms, "unit": "ms"}),
            json!({"id": "accepted_changed_lines", "value": total_changed_lines, "unit": "lines"}),
            json!({"id": "subject_turns", "value": totals.turns, "unit": "turns"}),
            json!({"id": "subject_function_calls", "value": totals.function_calls, "unit": "calls"}),
            json!({"id": "subject_function_errors", "value": totals.function_call_errors, "unit": "errors"}),
        ];
        if !snapshot.records.is_empty() {
            measurements.push(json!({
                "id": "checkpoint_acceptance_ratio",
                "value": accepted_records as f64 / snapshot.records.len() as f64,
                "unit": "ratio"
            }));
        }
        if snapshot.accepted_rungs > 0 {
            measurements.push(json!({
                "id": "turns_per_accepted_rung",
                "value": totals.turns as f64 / f64::from(snapshot.accepted_rungs),
                "unit": "turns_per_rung"
            }));
        }
        if totals.input_tokens.is_some() || totals.output_tokens.is_some() {
            let total_tokens = totals
                .input_tokens
                .unwrap_or_default()
                .saturating_add(totals.output_tokens.unwrap_or_default());
            measurements.push(json!({
                "id": "subject_total_tokens",
                "value": total_tokens,
                "unit": "tokens"
            }));
        }
        if let Some(cost_usd) = totals.cost_usd.filter(|value| value.is_finite()) {
            measurements.push(json!({
                "id": "subject_cost_usd",
                "value": cost_usd,
                "unit": "usd"
            }));
        }
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.into(),
            kind: "engineering_endurance_report".into(),
            content: json!({
                "scenario_version": VERSION,
                "initial_head": snapshot.initial_head,
                "accepted_head": snapshot.accepted_head,
                "accepted_rungs": snapshot.accepted_rungs,
                "total_rungs": TICKETS.len(),
                "terminal_status": snapshot.terminal_status,
                "terminal_rung": snapshot.terminal_rung,
                "started_at_ms": snapshot.started_at_ms,
                "elapsed_ms": snapshot.elapsed_ms,
                "accepted_checkpoints": accepted_records,
                "rejected_checkpoints": rejected_records,
                "total_changed_lines": total_changed_lines,
                "accepted_patch": accepted_patch,
                "checkpoints": snapshot.records,
                "github_handoff": github_handoff,
                "measurements": measurements
            }).into(),
            invariants: vec![
                CapturedInvariant {
                    id: "terminal_evidence_persisted".into(),
                    passed: terminal,
                    reason: "a completed ladder or first exhausted rung is the terminal benchmark observation".into(),
                },
                CapturedInvariant {
                    id: "accepted_prefix_integrity".into(),
                    passed: integrity,
                    reason: "every accepted prefix boundary contains its independent Git and test evidence".into(),
                },
                CapturedInvariant {
                    id: "github_credentials_isolated".into(),
                    passed: true,
                    reason: "the scenario emits a trusted handoff; GitHub is denied to the subject session".into(),
                },
            ],
            provenance: vec![ProvenanceEvidence {
                kind: "git".into(),
                source_id: fixture_root(run_id).display().to_string(),
                relation: "cumulative_checkpoint_ranges".into(),
            }],
        }])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.into(),
            kind: "engineering_endurance_report".into(),
            media_type: "application/json".into(),
            schema: json!({
                "type": "object",
                "required": [
                    "scenario_version", "initial_head", "accepted_head", "accepted_rungs",
                    "total_rungs", "terminal_status", "elapsed_ms", "checkpoints",
                    "accepted_patch", "github_handoff", "measurements"
                ],
                "properties": {
                    "scenario_version": {"const": 1},
                    "initial_head": {"type": "string", "pattern": "^[0-9a-f]{40}$"},
                    "accepted_head": {"type": "string", "pattern": "^[0-9a-f]{40}$"},
                    "accepted_rungs": {"type": "integer", "minimum": 0, "maximum": 10},
                    "total_rungs": {"const": 10},
                    "terminal_status": {"type": ["string", "null"]},
                    "terminal_rung": {"type": ["integer", "null"]},
                    "started_at_ms": {"type": "integer"},
                    "elapsed_ms": {"type": "integer"},
                    "accepted_checkpoints": {"type": "integer"},
                    "rejected_checkpoints": {"type": "integer"},
                    "total_changed_lines": {"type": "integer"},
                    "accepted_patch": {"type": "string"},
                    "checkpoints": {"type": "array"},
                    "github_handoff": {"type": "object"},
                    "measurements": {"type": "array", "minItems": 8, "maxItems": 12}
                },
                "additionalProperties": false
            }),
            max_size_bytes: 1_048_576,
        }],
        invariants: vec![
            InvariantSpec {
                id: "terminal_evidence_persisted".into(),
                description: "The longitudinal report records the terminal capability boundary."
                    .into(),
            },
            InvariantSpec {
                id: "accepted_prefix_integrity".into(),
                description:
                    "Each accepted rung preserves Git ancestry, scope, and cumulative tests.".into(),
            },
            InvariantSpec {
                id: "github_credentials_isolated".into(),
                description: "Only a trusted post-run publisher receives GitHub authority.".into(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNG_ONE_SOURCE: &str = r#"from __future__ import annotations

import json
import uuid
from pathlib import Path
from typing import Any

class JobNotFound(KeyError):
    pass

class DurableQueue:
    def __init__(self, journal_path: str | Path):
        self.journal_path = Path(journal_path)
        self.journal_path.parent.mkdir(parents=True, exist_ok=True)
        self.journal_path.touch(exist_ok=True)
        self._jobs: dict[str, dict[str, Any]] = {}
        self._idempotency: dict[str, tuple[str, dict[str, Any]]] = {}
        self._load()

    def _load(self):
        for line in self.journal_path.read_text(encoding='utf-8').splitlines():
            if not line.strip(): continue
            event = json.loads(line)
            if event['type'] == 'submitted':
                job = {'id': event['job_id'], 'payload': event['payload'], 'status': 'pending'}
                self._jobs[event['job_id']] = job
                key = event.get('idempotency_key')
                if key: self._idempotency[key] = (event['job_id'], event['payload'])
            elif event['type'] == 'completed':
                self._jobs[event['job_id']]['status'] = 'completed'

    def _append(self, event):
        with self.journal_path.open('a', encoding='utf-8') as journal:
            journal.write(json.dumps(event, sort_keys=True, separators=(',', ':')) + '\n')
            journal.flush()

    def submit(self, payload, idempotency_key=None):
        if idempotency_key:
            existing = self._idempotency.get(idempotency_key)
            if existing:
                if existing[1] != payload: raise ValueError('idempotency key payload conflict')
                return existing[0]
        job_id = uuid.uuid4().hex
        event = {'type': 'submitted', 'job_id': job_id, 'payload': payload}
        if idempotency_key: event['idempotency_key'] = idempotency_key
        self._append(event)
        self._jobs[job_id] = {'id': job_id, 'payload': payload, 'status': 'pending'}
        if idempotency_key: self._idempotency[idempotency_key] = (job_id, payload)
        return job_id

    def get(self, job_id):
        try: return dict(self._jobs[job_id])
        except KeyError as error: raise JobNotFound(job_id) from error

    def list_jobs(self):
        return [dict(job) for job in self._jobs.values()]

    def complete(self, job_id):
        if job_id not in self._jobs: raise JobNotFound(job_id)
        self._append({'type': 'completed', 'job_id': job_id})
        self._jobs[job_id]['status'] = 'completed'
"#;

    async fn initialize_test_repository(run_id: &str) -> (PathBuf, String) {
        let root = fixture_root(run_id);
        write_fixture(&root).unwrap();
        let head = initialize_git(&root).await.unwrap();
        (root, head)
    }

    async fn commit_all(root: &Path, message: &str) {
        for args in [&["add", "--all"][..], &["commit", "-q", "-m", message][..]] {
            let result = git(root, args).await.unwrap();
            assert!(result.success, "{}", result.stderr);
        }
    }

    #[test]
    fn contract_exposes_ten_ordered_cumulative_tickets() {
        assert_eq!(TICKETS.len(), 10);
        assert_eq!(TICKETS[0].id, "idempotent-submit");
        assert_eq!(TICKETS[9].id, "operational-stats");
        assert_eq!(deliverable_contract().artifacts.len(), 1);
    }

    #[test]
    fn materialization_is_canonical_across_namespaces() {
        let first = materialize("alpha-attempt", 7).unwrap();
        let retry = materialize("omega-attempt", 99).unwrap();
        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs, retry.case.inputs);
        assert_ne!(first.spec.prompt, retry.spec.prompt);
        assert_eq!(first.case.seed, CANONICAL_SEED);
        assert_eq!(first.case.complexity.profile.validation_loops, 10);
    }

    #[test]
    fn subject_capabilities_exclude_github_and_network() {
        let spec = scenario("attempt");
        assert!(spec.denied_functions.contains(&"github::*"));
        assert!(spec.denied_functions.contains(&"web::*"));
        assert_eq!(required_functions("attempt").len(), 1);
        assert!(allowed_functions("attempt").contains(&"coder::*".to_string()));
    }

    #[tokio::test]
    async fn baseline_is_green_and_each_hidden_rung_is_initially_red() {
        let root = fixture_root("module-test-baseline");
        write_fixture(&root).unwrap();
        initialize_git(&root).await.unwrap();
        let public = run_command(
            &root,
            "python3",
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
        .unwrap();
        assert!(public.success, "{}{}", public.stdout, public.stderr);
        for rung in 1..=TICKETS.len() {
            let hidden = run_command(&root, "python3", &["-c", HIDDEN_PROBE, &rung.to_string()])
                .await
                .unwrap();
            assert!(!hidden.success, "rung {rung} unexpectedly passed baseline");
        }
        remove_fixture(&root).unwrap();
    }

    #[tokio::test]
    async fn audit_accepts_a_clean_production_commit_that_passes_the_current_rung() {
        let (root, initial) = initialize_test_repository("rung-one-audit").await;
        fs::write(root.join(PRODUCTION_PATH), RUNG_ONE_SOURCE).unwrap();
        commit_all(&root, "Implement idempotent submission").await;
        let expected_git_config = git_text(&root, &["config", "--local", "--list"])
            .await
            .unwrap();
        let audit = audit_checkpoint(&root, &initial, 1, &expected_git_config)
            .await
            .unwrap();
        assert!(audit.accepted(), "{}", audit.feedback());
        assert_eq!(audit.changed_paths, vec![PRODUCTION_PATH]);
        assert_eq!(audit.non_merge_commits, 1);
        remove_fixture(&root).unwrap();
    }

    #[tokio::test]
    async fn checkpoint_reveals_only_the_next_ticket_after_acceptance() {
        let (root, initial) = initialize_test_repository("next-ticket-checkpoint").await;
        fs::write(root.join(PRODUCTION_PATH), RUNG_ONE_SOURCE).unwrap();
        commit_all(&root, "Implement idempotent submission").await;
        let head = git_text(&root, &["rev-parse", "HEAD"]).await.unwrap();
        let state = Arc::new(Mutex::new(EnduranceState {
            initial_head: initial.clone(),
            accepted_head: initial,
            accepted_rungs: 0,
            expected_git_config: git_text(&root, &["config", "--local", "--list"])
                .await
                .unwrap(),
            attempts: HashMap::new(),
            terminal_status: None,
            terminal_rung: None,
            started_at_ms: now_ms(),
            records: Vec::new(),
        }));
        let response = handle_checkpoint(
            root.clone(),
            Arc::clone(&state),
            CheckpointRequest {
                rung: 1,
                head_sha: head.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(response.status, "accepted");
        assert_eq!(response.accepted_head, head);
        assert!(response.next_ticket.unwrap().contains("Rung 2/10"));
        assert_eq!(lock_unpoisoned(&state).accepted_rungs, 1);
        remove_fixture(&root).unwrap();
    }

    #[tokio::test]
    async fn three_rejections_establish_a_terminal_capability_boundary() {
        let (root, initial) = initialize_test_repository("fail-boundary").await;
        let state = Arc::new(Mutex::new(EnduranceState {
            initial_head: initial.clone(),
            accepted_head: initial.clone(),
            accepted_rungs: 0,
            expected_git_config: git_text(&root, &["config", "--local", "--list"])
                .await
                .unwrap(),
            attempts: HashMap::new(),
            terminal_status: None,
            terminal_rung: None,
            started_at_ms: now_ms(),
            records: Vec::new(),
        }));
        for expected_status in ["rejected", "rejected", "capability_failure"] {
            let response = handle_checkpoint(
                root.clone(),
                Arc::clone(&state),
                CheckpointRequest {
                    rung: 1,
                    head_sha: initial.clone(),
                },
            )
            .await
            .unwrap();
            assert_eq!(response.status, expected_status);
            assert!(response.next_ticket.is_none());
        }
        let state = lock_unpoisoned(&state);
        assert_eq!(state.accepted_rungs, 0);
        assert_eq!(state.terminal_status.as_deref(), Some("capability_failure"));
        assert_eq!(state.terminal_rung, Some(1));
        assert_eq!(state.records.len(), 3);
        drop(state);
        remove_fixture(&root).unwrap();
    }

    #[tokio::test]
    async fn audit_rejects_protected_changes_and_dirty_worktrees() {
        let (root, initial) = initialize_test_repository("scope-protected").await;
        fs::write(root.join(PUBLIC_TEST_PATH), "# tampered\n").unwrap();
        commit_all(&root, "Change protected test").await;
        let expected_git_config = git_text(&root, &["config", "--local", "--list"])
            .await
            .unwrap();
        let protected = audit_checkpoint(&root, &initial, 1, &expected_git_config)
            .await
            .unwrap();
        assert!(!protected.scope_valid);

        fs::write(root.join(PRODUCTION_PATH), "# dirty\n").unwrap();
        let dirty = audit_checkpoint(&root, &initial, 1, &expected_git_config)
            .await
            .unwrap();
        assert!(!dirty.worktree_clean);

        let changed_config = git(&root, &["config", "core.hooksPath", "/tmp/not-allowed"])
            .await
            .unwrap();
        assert!(changed_config.success, "{}", changed_config.stderr);
        let config_tamper = audit_checkpoint(&root, &initial, 1, &expected_git_config)
            .await
            .unwrap();
        assert!(!config_tamper.git_config_valid);
        remove_fixture(&root).unwrap();
    }
}
