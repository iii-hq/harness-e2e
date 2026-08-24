//! Deterministic performance-regression benchmark.
//!
//! The subject receives a run-scoped Python fixture whose implementation is
//! correct but quadratic. Public and hidden correctness probes protect the
//! behavior, while instrumented values count equality/hash work independently
//! of host speed. Wall-clock timing is captured only as an advisory signal.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
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

pub const ID: &str = "performance_regression";
pub const VERSION: u32 = 2;
pub const CANONICAL_SEED: u64 = 1041;

const DELIVERABLE_ID: &str = "performance_audit";
const WORKLOAD_SMALL: u64 = 128;
const WORKLOAD_LARGE: u64 = 256;
const WORK_LIMIT_LARGE: u64 = 2_048;
const MINIMUM_REDUCTION_FACTOR: u64 = 8;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(90);

const PRODUCTION_PATH: &str = "src/deduplicate.py";
const TEST_PATH: &str = "tests/test_deduplicate.py";
const TASK_PATH: &str = "task.json";
const BASELINE_SOURCE: &str =
    include_str!("../../tests/fixtures/performance-regression/src/deduplicate.py");
const PUBLIC_TESTS: &str =
    include_str!("../../tests/fixtures/performance-regression/tests/test_deduplicate.py");
const TASK_MANIFEST: &str = include_str!("../../tests/fixtures/performance-regression/task.json");

const FUNCTIONAL_CORRECTNESS: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "functional_correctness",
    40,
    "The complete public suite and runner-owned hidden semantic probes accept the optimized implementation.",
    EvaluationDimension::Deliverable,
);
const DETERMINISTIC_IMPROVEMENT: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "deterministic_improvement",
    35,
    "Instrumented equality/hash work is bounded, scales near-linearly, and improves by at least the declared factor.",
    EvaluationDimension::StructuralIntegrity,
);
const PATCH_SCOPE: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "patch_scope",
    15,
    "Only the allowed production file changed; public tests, task manifest, and fixture topology remain exact.",
    EvaluationDimension::StructuralIntegrity,
);
const WALL_CLOCK_SIGNAL: AssessmentSpec = AssessmentSpec::score_only(
    "wall_clock_signal",
    10,
    "The candidate median wall-clock measurement improves over the run-local baseline; this host-dependent signal is advisory.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    FUNCTIONAL_CORRECTNESS,
    DETERMINISTIC_IMPROVEMENT,
    PATCH_SCOPE,
    WALL_CLOCK_SIGNAL,
];

const HIDDEN_PROBE: &str = r#"
import importlib.util
import json
from pathlib import Path

module_path = Path('src/deduplicate.py')
spec = importlib.util.spec_from_file_location('candidate', module_path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
stable_unique = module.stable_unique

correctness = (
    stable_unique([3, 1, 3, 2, 1]) == [3, 1, 2]
    and stable_unique([]) == []
    and stable_unique(iter(['a', 'b', 'a', 'c', 'b'])) == ['a', 'b', 'c']
    and stable_unique([None, None, False, False, True]) == [None, False, True]
)

class Tracked:
    equality_calls = 0
    hash_calls = 0

    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        type(self).equality_calls += 1
        return isinstance(other, Tracked) and self.value == other.value

    def __hash__(self):
        type(self).hash_calls += 1
        return hash(self.value)

def measured_work(size):
    Tracked.equality_calls = 0
    Tracked.hash_calls = 0
    values = [Tracked(index) for index in range(size)]
    output = stable_unique(iter(values))
    valid = len(output) == size and all(item is values[index] for index, item in enumerate(output))
    return valid, Tracked.equality_calls + Tracked.hash_calls

small_valid, work_128 = measured_work(128)
large_valid, work_256 = measured_work(256)
print(json.dumps({
    'correctness': correctness and small_valid and large_valid,
    'work_128': work_128,
    'work_256': work_256,
}, sort_keys=True))
"#;

const WALL_CLOCK_PROBE: &str = r#"
import importlib.util
import json
import statistics
import time
from pathlib import Path

spec = importlib.util.spec_from_file_location('candidate', Path('src/deduplicate.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
values = list(range(3000))
samples = []
for _ in range(5):
    started = time.perf_counter_ns()
    output = module.stable_unique(values)
    samples.append(time.perf_counter_ns() - started)
    if output != values:
        raise AssertionError('candidate changed functional output')
print(json.dumps({'median_ns': int(statistics.median(samples))}, sort_keys=True))
"#;

#[derive(Debug, Clone)]
struct Baseline {
    work_256: u64,
    median_ns: u64,
}

fn baselines() -> &'static Mutex<HashMap<String, Baseline>> {
    static BASELINES: OnceLock<Mutex<HashMap<String, Baseline>>> = OnceLock::new();
    BASELINES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ProbeOutput {
    correctness: bool,
    work_128: u64,
    work_256: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct WallClockOutput {
    median_ns: u64,
}

#[derive(Debug)]
struct CommandOutcome {
    success: bool,
    stdout: String,
    stderr: String,
}

#[derive(Debug, Clone)]
struct PerformanceAudit {
    public_tests_passed: bool,
    hidden: ProbeOutput,
    baseline: Option<Baseline>,
    candidate_median_ns: Option<u64>,
    protected_files_exact: bool,
    production_patch_present: bool,
    unexpected_paths: Vec<String>,
    public_output: String,
    hidden_output: String,
}

impl PerformanceAudit {
    fn functional_correctness(&self) -> bool {
        self.public_tests_passed && self.hidden.correctness
    }

    fn deterministic_improvement(&self) -> bool {
        let Some(baseline) = &self.baseline else {
            return false;
        };
        self.hidden.work_256 > 0
            && self.hidden.work_256 <= WORK_LIMIT_LARGE
            && baseline.work_256
                >= self
                    .hidden
                    .work_256
                    .saturating_mul(MINIMUM_REDUCTION_FACTOR)
            && self.hidden.work_128 > 0
            && self.hidden.work_256 <= self.hidden.work_128.saturating_mul(3)
    }

    fn scope_valid(&self) -> bool {
        self.protected_files_exact
            && self.production_patch_present
            && self.unexpected_paths.is_empty()
    }

    fn wall_clock_improved(&self) -> bool {
        self.baseline
            .as_ref()
            .zip(self.candidate_median_ns)
            .is_some_and(|(baseline, candidate)| candidate < baseline.median_ns)
    }
}

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
            "task": "stable-unique-quadratic-regression",
            "language": "python",
            "canonical_seed": CANONICAL_SEED,
            "allowed_production_paths": [PRODUCTION_PATH],
            "protected_paths": [TEST_PATH, TASK_PATH],
            "workloads": [WORKLOAD_SMALL, WORKLOAD_LARGE],
            "deterministic_work_limit": WORK_LIMIT_LARGE,
            "minimum_reduction_factor": MINIMUM_REDUCTION_FACTOR,
            "wall_clock_policy": "advisory",
        }),
        ComplexityProfile {
            planning_depth: 4,
            dependency_depth: 2,
            validation_loops: 2,
            artifact_count: 1,
            ambiguity_level: 4,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "e2e::filesystem".to_string(),
            "e2e::shell".to_string(),
            "python3".to_string(),
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
    let root = fixture_root(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            r#"Fix the performance regression in this isolated fixture: `{}`.

`stable_unique` is functionally correct but performs quadratic work on distinct inputs. Preserve
its contract: return the first occurrence of each hashable value in encounter order and accept any
iterable. Optimize the implementation so work scales near-linearly.

Workflow:
1. Read `task.json`, `src/deduplicate.py`, and the public test before editing.
2. Reproduce the public suite with:
   `python3 -m unittest discover -s tests -p 'test_*.py'`
3. Edit only `src/deduplicate.py`.
4. Re-run the complete public suite.

Do not modify tests or `task.json`, add files, access the network, or write outside this fixture.
The runner owns hidden semantic and instrumented-work probes. Report the changed file, public test
result, and why the new algorithm is near-linear. Do not claim a wall-clock speedup you did not
measure yourself."#,
            root.display()
        ),
        filesystem_root: Some(root),
        execution: ExecutionPolicy {
            max_turns: 32,
            max_output_tokens: Some(16_384),
            max_total_tokens: Some(400_000),
            stuck_timeout_seconds: 600,
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

fn fixture_root(run_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join("harness-e2e-performance-regression")
        .join(suffix(run_id))
}

fn expected_files() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        (PRODUCTION_PATH, BASELINE_SOURCE),
        (TEST_PATH, PUBLIC_TESTS),
        (TASK_PATH, TASK_MANIFEST),
    ])
}

fn ensure_safe_fixture_root(root: &Path) -> Result<()> {
    let expected_parent = std::env::temp_dir().join("harness-e2e-performance-regression");
    if root.parent() != Some(expected_parent.as_path()) {
        bail!(
            "refusing fixture operation outside {}: {}",
            expected_parent.display(),
            root.display()
        );
    }
    let Some(leaf) = root.file_name().and_then(|leaf| leaf.to_str()) else {
        bail!("fixture root has no UTF-8 leaf: {}", root.display());
    };
    if leaf.is_empty()
        || !leaf
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("fixture root leaf is unsafe: {leaf:?}");
    }
    Ok(())
}

fn reset_fixture(root: &Path) -> Result<()> {
    ensure_safe_fixture_root(root)?;
    if fs::symlink_metadata(root).is_ok() {
        remove_fixture_root(root)?;
    }
    for (relative, content) in expected_files() {
        let path = root.join(relative);
        let parent = path
            .parent()
            .with_context(|| format!("fixture file has no parent: {}", path.display()))?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
        fs::write(&path, content)
            .with_context(|| format!("failed writing fixture file {}", path.display()))?;
    }
    Ok(())
}

fn remove_fixture_root(root: &Path) -> Result<()> {
    ensure_safe_fixture_root(root)?;
    let Ok(metadata) = fs::symlink_metadata(root) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(root)
            .with_context(|| format!("failed removing fixture link {}", root.display()))?;
    } else {
        fs::remove_dir_all(root)
            .with_context(|| format!("failed removing fixture root {}", root.display()))?;
    }
    Ok(())
}

fn setup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let root = fixture_root(run_id);
        reset_fixture(&root)?;
        let public = run_python(
            &root,
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
            bail!(
                "canonical performance fixture public suite is not green: {}",
                public.stderr
            );
        }
        let hidden = run_json_probe::<ProbeOutput>(&root, HIDDEN_PROBE).await?;
        if !hidden.correctness || hidden.work_256 <= WORK_LIMIT_LARGE {
            bail!(
                "canonical fixture no longer has the intended correct quadratic baseline: correctness={}, work_256={}",
                hidden.correctness,
                hidden.work_256
            );
        }
        let wall = run_json_probe::<WallClockOutput>(&root, WALL_CLOCK_PROBE).await?;
        baselines()
            .lock()
            .expect("performance baseline lock poisoned")
            .insert(
                run_id.to_string(),
                Baseline {
                    work_256: hidden.work_256,
                    median_ns: wall.median_ns,
                },
            );
        Ok(())
    })
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
        .context("python probe timed out")?
        .context("failed launching python probe")?;
    Ok(CommandOutcome {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

async fn run_json_probe<T>(root: &Path, script: &str) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let outcome = run_python(root, &["-c", script]).await?;
    if !outcome.success {
        bail!("python probe failed: {}", outcome.stderr);
    }
    let line = outcome
        .stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .context("python probe returned no JSON line")?;
    serde_json::from_str(line).context("python probe returned invalid JSON")
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

async fn audit_fixture(run_id: &str) -> Result<PerformanceAudit> {
    let root = fixture_root(run_id);
    ensure_safe_fixture_root(&root)?;
    let observed_paths = collect_files(&root)?;
    let expected_paths = expected_files().keys().copied().collect::<BTreeSet<_>>();
    let unexpected_paths = observed_paths
        .iter()
        .filter(|path| !expected_paths.contains(path.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let protected_files_exact = [TEST_PATH, TASK_PATH].into_iter().all(|relative| {
        fs::read_to_string(root.join(relative)).ok().as_deref()
            == expected_files().get(relative).copied()
    });
    let production_patch_present = fs::read_to_string(root.join(PRODUCTION_PATH))
        .ok()
        .is_some_and(|source| source != BASELINE_SOURCE);

    let public = run_python(
        &root,
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
    let hidden_command = run_python(&root, &["-c", HIDDEN_PROBE]).await?;
    let hidden = if hidden_command.success {
        hidden_command
            .stdout
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .and_then(|line| serde_json::from_str(line).ok())
            .unwrap_or_default()
    } else {
        ProbeOutput::default()
    };
    let wall_command = run_python(&root, &["-c", WALL_CLOCK_PROBE]).await?;
    let candidate_median_ns = wall_command
        .success
        .then(|| {
            wall_command
                .stdout
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .and_then(|line| serde_json::from_str::<WallClockOutput>(line).ok())
                .map(|output| output.median_ns)
        })
        .flatten();
    let baseline = baselines()
        .lock()
        .expect("performance baseline lock poisoned")
        .get(run_id)
        .cloned();
    Ok(PerformanceAudit {
        public_tests_passed: public.success,
        hidden,
        baseline,
        candidate_median_ns,
        protected_files_exact,
        production_patch_present,
        unexpected_paths,
        public_output: format!("{}{}", public.stdout, public.stderr),
        hidden_output: format!("{}{}", hidden_command.stdout, hidden_command.stderr),
    })
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    _observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let audit = audit_fixture(run_id).await?;
        let baseline_work = audit.baseline.as_ref().map(|baseline| baseline.work_256);
        let baseline_median = audit.baseline.as_ref().map(|baseline| baseline.median_ns);
        Ok(assessment::build_evaluation([
            FUNCTIONAL_CORRECTNESS.full_or_zero(
                audit.functional_correctness(),
                format!(
                    "public_tests_passed={}, hidden_correctness={}; public_output={:?}; hidden_output={:?}",
                    audit.public_tests_passed,
                    audit.hidden.correctness,
                    audit.public_output,
                    audit.hidden_output
                ),
            ),
            DETERMINISTIC_IMPROVEMENT.full_or_zero(
                audit.deterministic_improvement(),
                format!(
                    "baseline_work_256={baseline_work:?}, candidate_work_128={}, candidate_work_256={}, limit_256={WORK_LIMIT_LARGE}, minimum_reduction={MINIMUM_REDUCTION_FACTOR}x",
                    audit.hidden.work_128, audit.hidden.work_256
                ),
            ),
            PATCH_SCOPE.full_or_zero(
                audit.scope_valid(),
                format!(
                    "protected_files_exact={}, production_patch_present={}, unexpected_paths={:?}",
                    audit.protected_files_exact,
                    audit.production_patch_present,
                    audit.unexpected_paths
                ),
            ),
            WALL_CLOCK_SIGNAL.full_or_zero(
                audit.wall_clock_improved(),
                format!(
                    "run-local baseline_median_ns={baseline_median:?}, candidate_median_ns={:?}; advisory only",
                    audit.candidate_median_ns
                ),
            ),
        ]))
    })
}

fn capture<'a>(
    _context: &'a E2eContext,
    _observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let audit = audit_fixture(run_id).await?;
        let functional = audit.functional_correctness();
        let deterministic = audit.deterministic_improvement();
        let scope = audit.scope_valid();
        let wall = audit.wall_clock_improved();
        let mut measurements = vec![json!({
            "id": "candidate_operations",
            "value": audit.hidden.work_256,
            "unit": "operations",
        })];
        if let Some(baseline) = &audit.baseline {
            measurements.extend([
                json!({
                    "id": "baseline_operations",
                    "value": baseline.work_256,
                    "unit": "operations",
                }),
                json!({
                    "id": "operation_reduction_ratio",
                    "value": baseline.work_256 as f64 / audit.hidden.work_256.max(1) as f64,
                    "unit": "ratio",
                }),
                json!({
                    "id": "baseline_median_ns",
                    "value": baseline.median_ns,
                    "unit": "ns",
                }),
            ]);
        }
        if let Some(candidate_median_ns) = audit.candidate_median_ns {
            measurements.push(json!({
                "id": "candidate_median_ns",
                "value": candidate_median_ns,
                "unit": "ns",
            }));
        }
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "performance_audit".to_string(),
            content: json!({
                "public_tests_passed": audit.public_tests_passed,
                "hidden_correctness": audit.hidden.correctness,
                "work": {
                    "workload_128": audit.hidden.work_128,
                    "workload_256": audit.hidden.work_256,
                    "baseline_256": audit.baseline.as_ref().map(|baseline| baseline.work_256),
                    "limit_256": WORK_LIMIT_LARGE,
                },
                "wall_clock": {
                    "policy": "advisory",
                    "baseline_median_ns": audit.baseline.as_ref().map(|baseline| baseline.median_ns),
                    "candidate_median_ns": audit.candidate_median_ns,
                    "improved": wall,
                },
                "scope": {
                    "protected_files_exact": audit.protected_files_exact,
                    "production_patch_present": audit.production_patch_present,
                    "unexpected_paths": audit.unexpected_paths,
                },
                "measurements": measurements,
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: "functional_behavior_preserved".to_string(),
                    passed: functional,
                    reason: "public and hidden semantic probes independently exercised the final source"
                        .to_string(),
                },
                CapturedInvariant {
                    id: "deterministic_work_reduced".to_string(),
                    passed: deterministic,
                    reason: format!(
                        "candidate used {} instrumented operation(s) at workload {WORKLOAD_LARGE}",
                        audit.hidden.work_256
                    ),
                },
                CapturedInvariant {
                    id: "protected_fixture_exact".to_string(),
                    passed: scope,
                    reason: "only the allowed production path may differ from the frozen fixture"
                        .to_string(),
                },
            ],
            provenance: vec![ProvenanceEvidence {
                kind: "filesystem".to_string(),
                source_id: fixture_root(run_id).join(PRODUCTION_PATH).display().to_string(),
                relation: "independently_probed".to_string(),
            }],
        }])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "performance_audit".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["public_tests_passed", "hidden_correctness", "work", "wall_clock", "scope", "measurements"],
                "properties": {
                    "public_tests_passed": { "type": "boolean" },
                    "hidden_correctness": { "type": "boolean" },
                    "work": { "type": "object" },
                    "wall_clock": { "type": "object" },
                    "scope": { "type": "object" },
                    "measurements": { "type": "array" }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 32_768,
        }],
        invariants: vec![
            InvariantSpec {
                id: "functional_behavior_preserved".to_string(),
                description: "Public and hidden behavior remains correct after optimization."
                    .to_string(),
            },
            InvariantSpec {
                id: "deterministic_work_reduced".to_string(),
                description:
                    "Instrumented work satisfies the absolute, scaling, and reduction gates."
                        .to_string(),
            },
            InvariantSpec {
                id: "protected_fixture_exact".to_string(),
                description: "Protected fixture files and topology remain byte-exact.".to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        baselines()
            .lock()
            .expect("performance baseline lock poisoned")
            .remove(run_id);
        remove_fixture_root(&fixture_root(run_id))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_paths_are_narrow_and_canonical() {
        let files = expected_files();
        assert_eq!(
            files.keys().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([PRODUCTION_PATH, TEST_PATH, TASK_PATH])
        );
        assert!(BASELINE_SOURCE.contains("value not in result"));
        assert!(TASK_MANIFEST.contains("deterministic_work_limit_at_256"));
    }

    #[test]
    fn deterministic_gate_rejects_quadratic_and_accepts_linear_work() {
        let base = Baseline {
            work_256: 32_640,
            median_ns: 10_000,
        };
        let quadratic = PerformanceAudit {
            public_tests_passed: true,
            hidden: ProbeOutput {
                correctness: true,
                work_128: 8_128,
                work_256: 32_640,
            },
            baseline: Some(base.clone()),
            candidate_median_ns: Some(9_000),
            protected_files_exact: true,
            production_patch_present: true,
            unexpected_paths: vec![],
            public_output: String::new(),
            hidden_output: String::new(),
        };
        assert!(!quadratic.deterministic_improvement());
        let mut linear = quadratic;
        linear.hidden.work_128 = 256;
        linear.hidden.work_256 = 512;
        assert!(linear.deterministic_improvement());
    }

    #[test]
    fn scenario_and_materialization_validate() {
        scenario("performance-test").validate().unwrap();
        materialize("performance-test", CANONICAL_SEED)
            .unwrap()
            .validate()
            .unwrap();
    }

    #[tokio::test]
    async fn frozen_baseline_is_green_and_quadratic() {
        let run_id = format!("performance-fixture-test-{}", std::process::id());
        let root = fixture_root(&run_id);
        reset_fixture(&root).unwrap();
        let public = run_python(
            &root,
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
        assert!(public.success, "{}", public.stderr);
        let hidden = run_json_probe::<ProbeOutput>(&root, HIDDEN_PROBE)
            .await
            .unwrap();
        assert!(hidden.correctness);
        assert!(hidden.work_256 > WORK_LIMIT_LARGE);
        remove_fixture_root(&root).unwrap();
    }
}
