//! Reproducible regression forensics over a real Git repository.
//!
//! The subject receives a self-contained Git bundle for the first 500 commits
//! of `coderefinery/git-bisect-exercise`, a public case manifest, and a probe.
//! The bundle makes the exercise independent of the upstream network while
//! preserving the original commits. The first-bad revision stays private in
//! this evaluator: neither the public manifest nor the prepared workspace
//! contains it as an oracle.

use std::collections::{BTreeSet, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::Command;

use crate::artifact::sha256_bytes;
use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::{
    common, ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture,
    ComplexityProfile, DeliverableCaptureFuture, DeliverableContract, EvaluationFuture,
    ExecutionPolicy, InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "git_regression_forensics";
const VERSION: u32 = 2;

const ACQUISITION_ID: &str = "repository_acquisition";
const TRACE_ID: &str = "investigation_trace";
const REPORT_ID: &str = "forensics_report";

const GOOD_SHA: &str = "f0ea95083275f5bd06f7c0b2498652929f3a2f01";
const BAD_SHA: &str = "15c23f04e5a168ba347b495c166b63775cc63599";
const GOOD_TREE: &str = "7cf8b4b73d8e75a097bb15b5874891a5ce842d20";
const BAD_TREE: &str = "8179bbc077b53e6fd8defb1386b8b42c43722d53";
const CULPRIT_SHA: &str = "326f68a558501a6f44d7685c2c1795794bac09b5";
const CULPRIT_PARENT_SHA: &str = "41af86cd42a966e4375b68bbad4af1384a0558cf";
const RANGE_COMMIT_COUNT: usize = 499;
const REPOSITORY_COMMIT_COUNT: usize = 500;
const CHANGED_PATH: &str = "get_pi.py";

const BUNDLE_RELATIVE_PATH: &str = "input/repository.bundle";
const MANIFEST_RELATIVE_PATH: &str = "input/case.json";
const PROBE_RELATIVE_PATH: &str = "input/probe.py";
const CHECKOUT_RELATIVE_PATH: &str = "checkout";
const TRACE_RELATIVE_PATH: &str = "probe-trace.jsonl";
const REPORT_RELATIVE_PATH: &str = "forensics-report.json";

const PUBLIC_MANIFEST: &str =
    include_str!("../../tests/fixtures/git-regression-forensics/manifest.json");
const PROBE_BYTES: &[u8] = include_bytes!("../../tests/fixtures/git-regression-forensics/probe.py");
const BUNDLE_BYTES: &[u8] =
    include_bytes!("../../tests/fixtures/git-regression-forensics/repository.bundle");

const REPOSITORY_ACQUIRED: AssessmentSpec = AssessmentSpec::hard_gated(
    "repository_acquired",
    20,
    "The immutable bundle was cloned through the shell worker and the resulting repository has the exact expected history and origin.",
);
const ENDPOINTS_REPRODUCED: AssessmentSpec = AssessmentSpec::hard_gated(
    "endpoints_reproduced",
    20,
    "The supplied probe classified the known-good endpoint as passing and the known-bad endpoint as failing before the search.",
);
const FIRST_BAD_IDENTIFIED: AssessmentSpec = AssessmentSpec::hard_gated(
    "first_bad_identified",
    40,
    "The report names the exact first bad commit and the runner independently verifies its pass/fail boundary.",
);
const EVIDENCE_GROUNDED: AssessmentSpec = AssessmentSpec::hard_gated(
    "evidence_grounded",
    10,
    "The structured report cites resolvable commits and changed paths in the immutable repository.",
);
const SEARCH_EFFICIENCY: AssessmentSpec = AssessmentSpec::score_only(
    "search_efficiency",
    10,
    "The investigation approaches binary-search efficiency without redundant probe executions or tool errors.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    REPOSITORY_ACQUIRED,
    ENDPOINTS_REPRODUCED,
    FIRST_BAD_IDENTIFIED,
    EVIDENCE_GROUNDED,
    SEARCH_EFFICIENCY,
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProbeRecord {
    revision: String,
    passed: bool,
    program_exit_code: i32,
    observed_stdout: String,
    duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ReportEvidence {
    commit: String,
    path: String,
    observation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ForensicsReport {
    culprit_sha: String,
    culprit_parent_sha: String,
    summary: String,
    changed_paths: Vec<String>,
    tested_commits: Vec<String>,
    evidence: Vec<ReportEvidence>,
}

#[derive(Debug, Clone, Default)]
struct TraceCapture {
    records: Vec<ProbeRecord>,
    invalid_lines: Vec<usize>,
}

#[derive(Debug, Clone, Serialize)]
struct TraceMetrics {
    total_probe_runs: usize,
    unique_revisions: usize,
    duplicate_probe_runs: usize,
    unique_search_revisions: usize,
    ideal_search_revisions: usize,
    search_amplification_milli: u64,
    first_culprit_probe_index: Option<usize>,
}

#[derive(Debug, Clone)]
struct Snapshot {
    public_manifest: Value,
    bundle_sha256: String,
    bundle_size_bytes: u64,
    probe_sha256: String,
    manifest_valid: bool,
    bundle_valid: bool,
    probe_valid: bool,
    shell_install_observed: bool,
    clone_observed: bool,
    direct_probe_calls: usize,
    checkout_is_repository: bool,
    checkout_head: Option<String>,
    checkout_origin: Option<String>,
    expected_checkout_origin: String,
    checkout_status: Option<String>,
    checkout_fsck: bool,
    good_tree: Option<String>,
    bad_tree: Option<String>,
    observed_range_commit_count: Option<usize>,
    trace: TraceCapture,
    trace_metrics: TraceMetrics,
    report: Option<ForensicsReport>,
    report_parse_error: Option<String>,
    runner_good_passed: bool,
    runner_bad_failed: bool,
    runner_parent_passed: bool,
    runner_culprit_failed: bool,
    report_evidence_resolves: bool,
}

impl Snapshot {
    fn acquisition_passed(&self) -> bool {
        self.manifest_valid
            && self.bundle_valid
            && self.probe_valid
            && (self.shell_install_observed || self.clone_observed)
            && self.clone_observed
            && self.checkout_is_repository
            && self.checkout_head.as_deref() == Some(BAD_SHA)
            && self.checkout_status.as_deref() == Some("")
            && self.checkout_fsck
            && self.good_tree.as_deref() == Some(GOOD_TREE)
            && self.bad_tree.as_deref() == Some(BAD_TREE)
            && self.observed_range_commit_count == Some(RANGE_COMMIT_COUNT)
            && self.checkout_origin.as_deref().is_some_and(|origin| {
                normalize_path(Path::new(origin))
                    == normalize_path(Path::new(&self.expected_checkout_origin))
            })
    }

    fn endpoints_passed(&self) -> bool {
        self.trace.invalid_lines.is_empty()
            && self.trace.records.first().is_some_and(|record| {
                record.revision == GOOD_SHA && record.passed && record.observed_stdout == "3.14"
            })
            && self.trace.records.get(1).is_some_and(|record| {
                record.revision == BAD_SHA && !record.passed && record.observed_stdout != "3.14"
            })
            && self.direct_probe_calls >= 2
            && self.runner_good_passed
            && self.runner_bad_failed
    }

    fn culprit_passed(&self) -> bool {
        self.report.as_ref().is_some_and(|report| {
            report.culprit_sha == CULPRIT_SHA
                && report.culprit_parent_sha == CULPRIT_PARENT_SHA
                && report
                    .tested_commits
                    .iter()
                    .any(|revision| revision == CULPRIT_SHA)
                && tested_commits_match_trace(report, &self.trace.records)
                && self
                    .trace
                    .records
                    .iter()
                    .any(|record| record.revision == CULPRIT_SHA && !record.passed)
        }) && self.runner_parent_passed
            && self.runner_culprit_failed
    }

    fn evidence_passed(&self) -> bool {
        self.report_parse_error.is_none()
            && self.report_evidence_resolves
            && self.report.as_ref().is_some_and(|report| {
                !report.summary.trim().is_empty()
                    && report.changed_paths == [CHANGED_PATH]
                    && report.evidence.iter().any(|evidence| {
                        evidence.commit == CULPRIT_SHA
                            && evidence.path == CHANGED_PATH
                            && !evidence.observation.trim().is_empty()
                    })
            })
    }
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn materialize(namespace: &str, seed: u64) -> Result<MaterializedScenario> {
    let inputs: Value = serde_json::from_str(PUBLIC_MANIFEST)
        .context("decode embedded Git regression case manifest")?;
    validate_public_manifest(&inputs)?;
    let case = ScenarioCase::new(
        ID,
        VERSION,
        seed,
        inputs,
        ComplexityProfile {
            planning_depth: 4,
            dependency_depth: 3,
            parallel_branches: 2,
            external_systems: 2,
            state_transitions: 9,
            validation_loops: 1,
            artifact_count: 3,
            coordination_edges: 3,
            ambiguity_level: 5,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::registry".to_string(),
            "iii::shell".to_string(),
            "git::offline-bundle".to_string(),
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
    let bundle = root.join(BUNDLE_RELATIVE_PATH);
    let probe = root.join(PROBE_RELATIVE_PATH);
    let checkout = root.join(CHECKOUT_RELATIVE_PATH);
    let trace = root.join(TRACE_RELATIVE_PATH);
    let report = root.join(REPORT_RELATIVE_PATH);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            r#"Investigate a regression in the supplied immutable snapshot of the real repository
`coderefinery/git-bisect-exercise`.

The exercise is network-independent: use only the local Git bundle `{bundle}`. The original
upstream URL in the public case manifest is provenance, not an instruction to contact it.

1. Use the `shell` worker already provided by the Harness stack. Do not reinstall, update, or
   replace it during the benchmark.
2. Clone the bundle with a direct `git clone --no-hardlinks` invocation into the exact path
   `{checkout}`. Do not use another remote, fetch, pull, or edit the bundle.
3. Read `{manifest}`. Establish the baseline first: detach at the full known-good revision
   `{good}`, run `python3 {probe} {checkout} --trace {trace}`, then detach at the full known-bad
   revision `{bad}` and run that exact probe again. A non-zero probe result means the checked-out
   revision is bad and is expected for the second endpoint.
4. Locate the first bad commit between those endpoints. Use `git bisect start {bad} {good}` and
   then one bounded `git bisect run python3 {probe} {checkout} --trace {trace}` invocation; the
   probe's zero/non-zero exit status already matches Git's good/bad convention. Do not implement
   an open-ended shell loop. Every classified revision must be recorded by the supplied probe.
5. Reset the checkout to detached `{bad}` and leave it clean. Do not modify `get_pi.py`, the Git
   history, the probe, or the public manifest.
6. Write `{report}` as strict JSON with exactly these fields:
   - `culprit_sha`: full 40-character first-bad commit SHA
   - `culprit_parent_sha`: its full first-parent SHA
   - `summary`: concise causal explanation
   - `changed_paths`: array of paths changed by the culprit
   - `tested_commits`: unique full SHAs actually present in the probe trace
   - `evidence`: non-empty array of objects with exactly `commit`, `path`, and `observation`

Do not claim that host networking was technically blocked; report only that the supplied fixture
did not require upstream access. Finish only after the JSON report exists and the checkout is
clean at the known-bad revision."#,
            bundle = bundle.display(),
            checkout = checkout.display(),
            manifest = root.join(MANIFEST_RELATIVE_PATH).display(),
            good = GOOD_SHA,
            bad = BAD_SHA,
            probe = probe.display(),
            trace = trace.display(),
            report = report.display(),
        ),
        filesystem_root: Some(root),
        execution: ExecutionPolicy {
            max_turns: 30,
            max_output_tokens: Some(12_288),
            max_total_tokens: Some(600_000),
            stuck_timeout_seconds: 600,
            max_validation_retries: None,
        },
        denied_functions: &[],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
}

fn setup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move { prepare_workspace(&workspace_root(run_id)).await })
}

async fn prepare_workspace(root: &Path) -> Result<()> {
    let manifest: Value =
        serde_json::from_str(PUBLIC_MANIFEST).context("decode embedded public case manifest")?;
    validate_public_manifest(&manifest)?;

    let input = root.join("input");
    fs::create_dir_all(&input).with_context(|| format!("create {}", input.display()))?;
    write_exact(&root.join(BUNDLE_RELATIVE_PATH), BUNDLE_BYTES)?;
    write_exact(
        &root.join(MANIFEST_RELATIVE_PATH),
        PUBLIC_MANIFEST.as_bytes(),
    )?;
    write_exact(&root.join(PROBE_RELATIVE_PATH), PROBE_BYTES)?;
    make_probe_executable(&root.join(PROBE_RELATIVE_PATH))?;

    let preflight = root.join(".fixture-preflight");
    remove_directory(&preflight)?;
    let validation = validate_fixture(root, &preflight).await;
    let cleanup = remove_directory(&preflight);
    match (validation, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error).context("remove fixture preflight workspace"),
        (Err(error), Err(cleanup_error)) => {
            Err(error.context(format!("also failed to remove preflight: {cleanup_error}")))
        }
    }
}

fn validate_public_manifest(manifest: &Value) -> Result<()> {
    if PUBLIC_MANIFEST.contains(CULPRIT_SHA) || PUBLIC_MANIFEST.contains(CULPRIT_PARENT_SHA) {
        bail!("public case manifest leaks the private first-bad oracle");
    }
    let expected = [
        ("/schema_version", Value::from(1)),
        (
            "/case_id",
            Value::String("coderefinery-git-bisect-first-500-v1".into()),
        ),
        ("/history/good_sha", Value::String(GOOD_SHA.into())),
        ("/history/bad_sha", Value::String(BAD_SHA.into())),
        ("/history/good_tree", Value::String(GOOD_TREE.into())),
        ("/history/bad_tree", Value::String(BAD_TREE.into())),
        (
            "/history/range_commit_count",
            Value::from(RANGE_COMMIT_COUNT),
        ),
        (
            "/history/repository_commit_count",
            Value::from(REPOSITORY_COMMIT_COUNT),
        ),
        ("/bundle/path", Value::String(BUNDLE_RELATIVE_PATH.into())),
        ("/probe/path", Value::String(PROBE_RELATIVE_PATH.into())),
        (
            "/outputs/checkout",
            Value::String(CHECKOUT_RELATIVE_PATH.into()),
        ),
        ("/outputs/trace", Value::String(TRACE_RELATIVE_PATH.into())),
        (
            "/outputs/report",
            Value::String(REPORT_RELATIVE_PATH.into()),
        ),
    ];
    for (pointer, expected) in expected {
        if manifest.pointer(pointer) != Some(&expected) {
            bail!("public case manifest has unexpected value at {pointer}");
        }
    }
    let expected_bundle_sha = manifest_string(manifest, "/bundle/sha256")?;
    if expected_bundle_sha != sha256_bytes(BUNDLE_BYTES) {
        bail!("embedded Git bundle SHA-256 differs from public manifest");
    }
    if manifest
        .pointer("/bundle/size_bytes")
        .and_then(Value::as_u64)
        != Some(BUNDLE_BYTES.len() as u64)
    {
        bail!("embedded Git bundle size differs from public manifest");
    }
    if manifest_string(manifest, "/probe/sha256")? != sha256_bytes(PROBE_BYTES) {
        bail!("embedded probe SHA-256 differs from public manifest");
    }
    Ok(())
}

async fn validate_fixture(root: &Path, checkout: &Path) -> Result<()> {
    let manifest: Value = serde_json::from_str(PUBLIC_MANIFEST)?;
    let bundle = root.join(BUNDLE_RELATIVE_PATH);
    let probe = root.join(PROBE_RELATIVE_PATH);
    let bundle_bytes = fs::read(&bundle).with_context(|| format!("read {}", bundle.display()))?;
    let probe_bytes = fs::read(&probe).with_context(|| format!("read {}", probe.display()))?;
    if sha256_bytes(&bundle_bytes) != manifest_string(&manifest, "/bundle/sha256")? {
        bail!("materialized Git bundle failed SHA-256 verification");
    }
    if sha256_bytes(&probe_bytes) != manifest_string(&manifest, "/probe/sha256")? {
        bail!("materialized regression probe failed SHA-256 verification");
    }

    let bundle_arg = bundle.display().to_string();
    let checkout_arg = checkout.display().to_string();
    git(
        root,
        &["clone", "--no-hardlinks", &bundle_arg, &checkout_arg],
    )
    .await?;
    expect_git(checkout, &["rev-parse", "HEAD"], BAD_SHA).await?;
    expect_git(
        checkout,
        &["rev-parse", &format!("{GOOD_SHA}^{{tree}}")],
        GOOD_TREE,
    )
    .await?;
    expect_git(
        checkout,
        &["rev-parse", &format!("{BAD_SHA}^{{tree}}")],
        BAD_TREE,
    )
    .await?;
    expect_git(
        checkout,
        &["rev-list", "--count", "HEAD"],
        &REPOSITORY_COMMIT_COUNT.to_string(),
    )
    .await?;
    expect_git(
        checkout,
        &["rev-list", "--count", &format!("{GOOD_SHA}..{BAD_SHA}")],
        &RANGE_COMMIT_COUNT.to_string(),
    )
    .await?;
    expect_git(
        checkout,
        &["ls-tree", "-r", "--name-only", BAD_SHA],
        CHANGED_PATH,
    )
    .await?;
    git(
        checkout,
        &["merge-base", "--is-ancestor", GOOD_SHA, BAD_SHA],
    )
    .await?;
    expect_git(
        checkout,
        &["rev-parse", &format!("{CULPRIT_SHA}^")],
        CULPRIT_PARENT_SHA,
    )
    .await?;

    let trace = checkout.join("preflight-trace.jsonl");
    for (revision, expected_pass) in [
        (GOOD_SHA, true),
        (BAD_SHA, false),
        (CULPRIT_PARENT_SHA, true),
        (CULPRIT_SHA, false),
    ] {
        git(checkout, &["checkout", "--detach", revision]).await?;
        let passed = execute_probe(&probe, checkout, &trace).await?;
        if passed != expected_pass {
            bail!(
                "fixture probe classified {revision} as passed={passed}; expected {expected_pass}"
            );
        }
    }
    let trace = read_trace(&trace);
    if !trace.invalid_lines.is_empty() || trace.records.len() != 4 {
        bail!("fixture probe did not emit four valid preflight records");
    }
    Ok(())
}

fn capture<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let snapshot = collect_snapshot(&workspace_root(run_id), observation).await;
        Ok(vec![
            CapturedDeliverable {
                id: ACQUISITION_ID.to_string(),
                kind: "git_repository_acquisition".to_string(),
                content: acquisition_content(&snapshot).into(),
                invariants: vec![CapturedInvariant {
                    id: "repository_bundle_and_clone_verified".to_string(),
                    passed: snapshot.acquisition_passed(),
                    reason: acquisition_reason(&snapshot),
                }],
                provenance: vec![ProvenanceEvidence {
                    kind: "git_repository".to_string(),
                    source_id:
                        "coderefinery/git-bisect-exercise@15c23f04e5a168ba347b495c166b63775cc63599"
                            .to_string(),
                    relation: "materialized_from_verified_bundle".to_string(),
                }],
            },
            CapturedDeliverable {
                id: TRACE_ID.to_string(),
                kind: "git_probe_trace".to_string(),
                content: trace_content(&snapshot).into(),
                invariants: vec![CapturedInvariant {
                    id: "good_and_bad_endpoints_reproduced".to_string(),
                    passed: snapshot.endpoints_passed(),
                    reason: endpoint_reason(&snapshot),
                }],
                provenance: vec![ProvenanceEvidence {
                    kind: "session".to_string(),
                    source_id: observation.metrics.root_session_id.clone(),
                    relation: "executed_revision_probes".to_string(),
                }],
            },
            CapturedDeliverable {
                id: REPORT_ID.to_string(),
                kind: "git_forensics_report".to_string(),
                content: report_content(&snapshot).into(),
                invariants: vec![
                    CapturedInvariant {
                        id: "first_bad_boundary_verified".to_string(),
                        passed: snapshot.culprit_passed(),
                        reason: culprit_reason(&snapshot),
                    },
                    CapturedInvariant {
                        id: "report_evidence_resolves".to_string(),
                        passed: snapshot.evidence_passed(),
                        reason: evidence_reason(&snapshot),
                    },
                ],
                provenance: vec![ProvenanceEvidence {
                    kind: "git_commit".to_string(),
                    source_id: snapshot
                        .report
                        .as_ref()
                        .map(|report| report.culprit_sha.clone())
                        .unwrap_or_else(|| "unresolved".to_string()),
                    relation: "reported_as_first_bad".to_string(),
                }],
            },
        ])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![
            ArtifactExpectation {
                id: ACQUISITION_ID.to_string(),
                kind: "git_repository_acquisition".to_string(),
                media_type: "application/json".to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["manifest", "bundle", "probe", "clone", "runner_validation"],
                    "properties": {
                        "manifest": {"type": "object"},
                        "bundle": {"type": "object"},
                        "probe": {"type": "object"},
                        "clone": {"type": "object"},
                        "runner_validation": {"type": "object"}
                    },
                    "additionalProperties": false
                }),
                max_size_bytes: 64 * 1024,
            },
            ArtifactExpectation {
                id: TRACE_ID.to_string(),
                kind: "git_probe_trace".to_string(),
                media_type: "application/json".to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["records", "invalid_lines", "metrics"],
                    "properties": {
                        "records": {"type": "array"},
                        "invalid_lines": {"type": "array"},
                        "metrics": {"type": "object"}
                    },
                    "additionalProperties": false
                }),
                max_size_bytes: 256 * 1024,
            },
            ArtifactExpectation {
                id: REPORT_ID.to_string(),
                kind: "git_forensics_report".to_string(),
                media_type: "application/json".to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["report", "parse_error", "evidence_resolves"],
                    "properties": {
                        "report": {"type": ["object", "null"]},
                        "parse_error": {"type": ["string", "null"]},
                        "evidence_resolves": {"type": "boolean"}
                    },
                    "additionalProperties": false
                }),
                max_size_bytes: 64 * 1024,
            },
        ],
        invariants: vec![
            InvariantSpec {
                id: "repository_bundle_and_clone_verified".to_string(),
                description: "Bundle, probe, manifest, cloned history, origin, and checkout integrity are exact."
                    .to_string(),
            },
            InvariantSpec {
                id: "good_and_bad_endpoints_reproduced".to_string(),
                description: "The first two probe records reproduce good then bad, and the runner independently agrees."
                    .to_string(),
            },
            InvariantSpec {
                id: "first_bad_boundary_verified".to_string(),
                description: "The reported first bad commit matches the private oracle and its parent passes."
                    .to_string(),
            },
            InvariantSpec {
                id: "report_evidence_resolves".to_string(),
                description: "Every evidence commit and path resolves within the immutable history."
                    .to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let snapshot = collect_snapshot(&workspace_root(run_id), observation).await;
        let acquisition = snapshot.acquisition_passed();
        let endpoints = snapshot.endpoints_passed();
        let culprit = snapshot.culprit_passed();
        let evidence = snapshot.evidence_passed();
        let efficiency_points = efficiency_points(&snapshot, observation);
        Ok(assessment::build_evaluation(
            if evidence {
                crate::report::CompletionState::Completed
            } else {
                crate::report::CompletionState::TaskIncomplete
            },
            [
                REPOSITORY_ACQUIRED.full_or_zero(acquisition, acquisition_reason(&snapshot)),
                ENDPOINTS_REPRODUCED.full_or_zero(endpoints, endpoint_reason(&snapshot)),
                FIRST_BAD_IDENTIFIED.full_or_zero(culprit, culprit_reason(&snapshot)),
                EVIDENCE_GROUNDED.full_or_zero(evidence, evidence_reason(&snapshot)),
                SEARCH_EFFICIENCY.award(
                    efficiency_points,
                    format!(
                    "unique search revisions={}, ideal={}, duplicates={}, function-call errors={}",
                    snapshot.trace_metrics.unique_search_revisions,
                    snapshot.trace_metrics.ideal_search_revisions,
                    snapshot.trace_metrics.duplicate_probe_runs,
                    observation.metrics.totals.function_call_errors,
                ),
                )?,
            ],
        ))
    })
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move { remove_directory(&workspace_root(run_id)) })
}

async fn collect_snapshot(root: &Path, observation: &ScenarioObservation) -> Snapshot {
    let manifest_path = root.join(MANIFEST_RELATIVE_PATH);
    let bundle_path = root.join(BUNDLE_RELATIVE_PATH);
    let probe_path = root.join(PROBE_RELATIVE_PATH);
    let checkout = root.join(CHECKOUT_RELATIVE_PATH);
    let trace = read_trace(&root.join(TRACE_RELATIVE_PATH));
    let trace_metrics = trace_metrics(&trace.records);
    let public_manifest = fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or(Value::Null);
    let bundle_bytes = fs::read(&bundle_path).unwrap_or_default();
    let probe_bytes = fs::read(&probe_path).unwrap_or_default();
    let calls = common::function_calls(&observation.transcript);
    let shell_install_observed = calls.iter().any(|call| {
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
                == Some("shell")
    });
    let clone_observed = calls
        .iter()
        .any(|call| exact_bundle_clone(call, &bundle_path, &checkout));
    let direct_probe_calls = calls
        .iter()
        .filter(|call| {
            exact_probe_call(
                call,
                &probe_path,
                &checkout,
                &root.join(TRACE_RELATIVE_PATH),
            )
        })
        .count();
    let checkout_head = git_optional(&checkout, &["rev-parse", "HEAD"]).await;
    let checkout_origin = git_optional(&checkout, &["config", "--get", "remote.origin.url"]).await;
    let checkout_status = git_optional(
        &checkout,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )
    .await;
    let checkout_is_repository = git_optional(&checkout, &["rev-parse", "--is-inside-work-tree"])
        .await
        .as_deref()
        == Some("true");
    let checkout_fsck = git_succeeds(&checkout, &["fsck", "--no-dangling"]).await;
    let good_tree = git_optional(&checkout, &["rev-parse", &format!("{GOOD_SHA}^{{tree}}")]).await;
    let bad_tree = git_optional(&checkout, &["rev-parse", &format!("{BAD_SHA}^{{tree}}")]).await;
    let observed_range_commit_count = git_optional(
        &checkout,
        &["rev-list", "--count", &format!("{GOOD_SHA}..{BAD_SHA}")],
    )
    .await
    .and_then(|count| count.parse().ok());
    let (report, report_parse_error) = read_report(&root.join(REPORT_RELATIVE_PATH));
    let runner_good_passed = runner_classify(root, &checkout, GOOD_SHA).await == Some(true);
    let runner_bad_failed = runner_classify(root, &checkout, BAD_SHA).await == Some(false);
    let runner_parent_passed =
        runner_classify(root, &checkout, CULPRIT_PARENT_SHA).await == Some(true);
    let runner_culprit_failed = runner_classify(root, &checkout, CULPRIT_SHA).await == Some(false);
    let report_evidence_resolves = match report.as_ref() {
        Some(report) => validate_report_evidence(&checkout, report).await,
        None => false,
    };
    let manifest_valid = validate_public_manifest(&public_manifest).is_ok();
    let bundle_sha256 = sha256_bytes(&bundle_bytes);
    let probe_sha256 = sha256_bytes(&probe_bytes);
    let bundle_valid = public_manifest
        .pointer("/bundle/sha256")
        .and_then(Value::as_str)
        == Some(bundle_sha256.as_str())
        && public_manifest
            .pointer("/bundle/size_bytes")
            .and_then(Value::as_u64)
            == Some(bundle_bytes.len() as u64);
    let probe_valid = public_manifest
        .pointer("/probe/sha256")
        .and_then(Value::as_str)
        == Some(probe_sha256.as_str());

    Snapshot {
        public_manifest,
        bundle_sha256,
        bundle_size_bytes: bundle_bytes.len() as u64,
        probe_sha256,
        manifest_valid,
        bundle_valid,
        probe_valid,
        shell_install_observed,
        clone_observed,
        direct_probe_calls,
        checkout_is_repository,
        checkout_head,
        checkout_origin,
        expected_checkout_origin: bundle_path.display().to_string(),
        checkout_status,
        checkout_fsck,
        good_tree,
        bad_tree,
        observed_range_commit_count,
        trace,
        trace_metrics,
        report,
        report_parse_error,
        runner_good_passed,
        runner_bad_failed,
        runner_parent_passed,
        runner_culprit_failed,
        report_evidence_resolves,
    }
}

fn acquisition_content(snapshot: &Snapshot) -> Value {
    json!({
        "manifest": snapshot.public_manifest,
        "bundle": {
            "sha256": snapshot.bundle_sha256,
            "size_bytes": snapshot.bundle_size_bytes,
            "valid": snapshot.bundle_valid,
        },
        "probe": {
            "sha256": snapshot.probe_sha256,
            "valid": snapshot.probe_valid,
        },
        "clone": {
            "shell_install_observed": snapshot.shell_install_observed,
            "clone_observed": snapshot.clone_observed,
            "direct_probe_calls": snapshot.direct_probe_calls,
            "is_repository": snapshot.checkout_is_repository,
            "head": snapshot.checkout_head,
            "origin": snapshot.checkout_origin,
            "expected_origin": snapshot.expected_checkout_origin,
            "status": snapshot.checkout_status,
            "fsck": snapshot.checkout_fsck,
            "good_tree": snapshot.good_tree,
            "bad_tree": snapshot.bad_tree,
            "range_commit_count": snapshot.observed_range_commit_count,
        },
        "runner_validation": {
            "good_passed": snapshot.runner_good_passed,
            "bad_failed": snapshot.runner_bad_failed,
            "culprit_parent_passed": snapshot.runner_parent_passed,
            "culprit_failed": snapshot.runner_culprit_failed,
        }
    })
}

fn trace_content(snapshot: &Snapshot) -> Value {
    json!({
        "records": snapshot.trace.records,
        "invalid_lines": snapshot.trace.invalid_lines,
        "metrics": snapshot.trace_metrics,
    })
}

fn report_content(snapshot: &Snapshot) -> Value {
    json!({
        "report": snapshot.report,
        "parse_error": snapshot.report_parse_error,
        "evidence_resolves": snapshot.report_evidence_resolves,
    })
}

fn acquisition_reason(snapshot: &Snapshot) -> String {
    format!(
        "manifest/bundle/probe={}/{}/{}, shell add={}, direct clone={}, repo={}, head={:?}, origin={:?}, clean={:?}, fsck={}, trees={:?}/{:?}, range={:?}",
        snapshot.manifest_valid,
        snapshot.bundle_valid,
        snapshot.probe_valid,
        snapshot.shell_install_observed,
        snapshot.clone_observed,
        snapshot.checkout_is_repository,
        snapshot.checkout_head,
        snapshot.checkout_origin,
        snapshot.checkout_status,
        snapshot.checkout_fsck,
        snapshot.good_tree,
        snapshot.bad_tree,
        snapshot.observed_range_commit_count,
    )
}

fn endpoint_reason(snapshot: &Snapshot) -> String {
    format!(
        "trace records={}, invalid lines={:?}, direct probe calls={}, first={:?}, second={:?}, runner good/bad={}/{}",
        snapshot.trace.records.len(),
        snapshot.trace.invalid_lines,
        snapshot.direct_probe_calls,
        snapshot.trace.records.first(),
        snapshot.trace.records.get(1),
        snapshot.runner_good_passed,
        snapshot.runner_bad_failed,
    )
}

fn culprit_reason(snapshot: &Snapshot) -> String {
    format!(
        "reported culprit/parent={:?}/{:?}, culprit probed={}, runner parent/culprit={}/{}",
        snapshot.report.as_ref().map(|report| &report.culprit_sha),
        snapshot
            .report
            .as_ref()
            .map(|report| &report.culprit_parent_sha),
        snapshot
            .trace
            .records
            .iter()
            .any(|record| record.revision == CULPRIT_SHA && !record.passed),
        snapshot.runner_parent_passed,
        snapshot.runner_culprit_failed,
    )
}

fn evidence_reason(snapshot: &Snapshot) -> String {
    format!(
        "report parsed={}, parse error={:?}, references resolve={}, summary/paths/evidence valid={}",
        snapshot.report.is_some(),
        snapshot.report_parse_error,
        snapshot.report_evidence_resolves,
        snapshot.evidence_passed(),
    )
}

fn efficiency_points(snapshot: &Snapshot, observation: &ScenarioObservation) -> u8 {
    if observation.metrics.totals.function_call_errors > 0
        || !snapshot.trace.invalid_lines.is_empty()
        || snapshot.trace_metrics.unique_search_revisions == 0
    {
        return 0;
    }
    let observed = snapshot.trace_metrics.unique_search_revisions;
    let ideal = snapshot.trace_metrics.ideal_search_revisions;
    if observed <= ideal.saturating_add(2) && snapshot.trace_metrics.duplicate_probe_runs == 0 {
        SEARCH_EFFICIENCY.weight()
    } else if observed <= ideal.saturating_mul(2) {
        SEARCH_EFFICIENCY.weight() / 2
    } else {
        0
    }
}

fn trace_metrics(records: &[ProbeRecord]) -> TraceMetrics {
    let unique = records
        .iter()
        .map(|record| record.revision.as_str())
        .collect::<HashSet<_>>();
    let unique_search_revisions = unique
        .iter()
        .filter(|revision| **revision != GOOD_SHA && **revision != BAD_SHA)
        .count();
    let ideal_search_revisions = ceil_log2(RANGE_COMMIT_COUNT);
    let search_amplification_milli = if ideal_search_revisions == 0 {
        0
    } else {
        (unique_search_revisions as u64)
            .saturating_mul(1000)
            .checked_div(ideal_search_revisions as u64)
            .unwrap_or(0)
    };
    TraceMetrics {
        total_probe_runs: records.len(),
        unique_revisions: unique.len(),
        duplicate_probe_runs: records.len().saturating_sub(unique.len()),
        unique_search_revisions,
        ideal_search_revisions,
        search_amplification_milli,
        first_culprit_probe_index: records
            .iter()
            .position(|record| record.revision == CULPRIT_SHA)
            .map(|index| index + 1),
    }
}

fn ceil_log2(value: usize) -> usize {
    if value <= 1 {
        0
    } else {
        usize::BITS as usize - (value - 1).leading_zeros() as usize
    }
}

fn read_trace(path: &Path) -> TraceCapture {
    let Ok(content) = fs::read_to_string(path) else {
        return TraceCapture::default();
    };
    let mut capture = TraceCapture::default();
    for (index, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<ProbeRecord>(line) {
            Ok(record) if valid_probe_record(&record) => capture.records.push(record),
            _ => capture.invalid_lines.push(index + 1),
        }
    }
    capture
}

fn read_report(path: &Path) -> (Option<ForensicsReport>, Option<String>) {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => return (None, Some(format!("read report: {error}"))),
    };
    match serde_json::from_str::<ForensicsReport>(&content) {
        Ok(report) => (Some(report), None),
        Err(error) => (None, Some(format!("decode report: {error}"))),
    }
}

async fn validate_report_evidence(checkout: &Path, report: &ForensicsReport) -> bool {
    if !full_sha(&report.culprit_sha)
        || !full_sha(&report.culprit_parent_sha)
        || report.evidence.is_empty()
        || report.tested_commits.iter().any(|sha| !full_sha(sha))
    {
        return false;
    }
    let changed = match git_optional(
        checkout,
        &["diff", "--name-only", CULPRIT_PARENT_SHA, CULPRIT_SHA, "--"],
    )
    .await
    {
        Some(output) => output.lines().map(str::to_string).collect::<BTreeSet<_>>(),
        None => return false,
    };
    if report
        .changed_paths
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        != changed
    {
        return false;
    }
    for evidence in &report.evidence {
        if !full_sha(&evidence.commit)
            || !safe_relative_path(&evidence.path)
            || evidence.observation.trim().is_empty()
        {
            return false;
        }
        if !git_succeeds(
            checkout,
            &["merge-base", "--is-ancestor", GOOD_SHA, &evidence.commit],
        )
        .await
            || !git_succeeds(
                checkout,
                &["merge-base", "--is-ancestor", &evidence.commit, BAD_SHA],
            )
            .await
            || !git_succeeds(
                checkout,
                &[
                    "cat-file",
                    "-e",
                    &format!("{}:{}", evidence.commit, evidence.path),
                ],
            )
            .await
        {
            return false;
        }
    }
    true
}

fn exact_bundle_clone(call: &common::ObservedFunctionCall, bundle: &Path, checkout: &Path) -> bool {
    if call.function_id != "shell::exec"
        || call.arguments.get("command").and_then(Value::as_str) != Some("git")
    {
        return false;
    }
    let Some(args) = call.arguments.get("args").and_then(Value::as_array) else {
        return false;
    };
    let args = args.iter().filter_map(Value::as_str).collect::<Vec<_>>();
    args.len() == 4
        && args[0] == "clone"
        && args[1] == "--no-hardlinks"
        && normalize_path(Path::new(args[2])) == normalize_path(bundle)
        && normalize_path(Path::new(args[3])) == normalize_path(checkout)
}

fn exact_probe_call(
    call: &common::ObservedFunctionCall,
    probe: &Path,
    checkout: &Path,
    trace: &Path,
) -> bool {
    if call.function_id != "shell::exec"
        || call.arguments.get("command").and_then(Value::as_str) != Some("python3")
    {
        return false;
    }
    let Some(args) = call.arguments.get("args").and_then(Value::as_array) else {
        return false;
    };
    let args = args.iter().filter_map(Value::as_str).collect::<Vec<_>>();
    args.len() == 4
        && normalize_path(Path::new(args[0])) == normalize_path(probe)
        && normalize_path(Path::new(args[1])) == normalize_path(checkout)
        && args[2] == "--trace"
        && normalize_path(Path::new(args[3])) == normalize_path(trace)
}

fn tested_commits_match_trace(report: &ForensicsReport, records: &[ProbeRecord]) -> bool {
    if report.tested_commits.iter().any(|sha| !full_sha(sha)) {
        return false;
    }
    let reported = report
        .tested_commits
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let observed = records
        .iter()
        .map(|record| record.revision.as_str())
        .collect::<BTreeSet<_>>();
    reported.len() == report.tested_commits.len() && reported == observed
}

fn valid_probe_record(record: &ProbeRecord) -> bool {
    full_sha(&record.revision)
        && record.program_exit_code == 0
        && record.passed == (record.observed_stdout == "3.14")
}

async fn runner_classify(root: &Path, checkout: &Path, revision: &str) -> Option<bool> {
    let source = git_optional(checkout, &["show", &format!("{revision}:{CHANGED_PATH}")]).await?;
    let path = root.join(format!(".runner-validation-{revision}.py"));
    if fs::write(&path, source.as_bytes()).is_err() {
        return None;
    }
    let output = Command::new("python3")
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .await
        .ok();
    let _ = fs::remove_file(&path);
    output.map(|output| {
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim() == "3.14"
            && String::from_utf8_lossy(&output.stderr).trim().is_empty()
    })
}

async fn execute_probe(probe: &Path, repository: &Path, trace: &Path) -> Result<bool> {
    let output = Command::new("python3")
        .arg(probe)
        .arg(repository)
        .arg("--trace")
        .arg(trace)
        .stdin(Stdio::null())
        .output()
        .await
        .with_context(|| format!("execute regression probe in {}", repository.display()))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        code => bail!(
            "regression probe exited {:?}: {}",
            code,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

async fn expect_git(cwd: &Path, args: &[&str], expected: &str) -> Result<()> {
    let observed = git(cwd, args).await?;
    if observed != expected {
        bail!(
            "git {} in {} returned {:?}; expected {:?}",
            args.join(" "),
            cwd.display(),
            observed,
            expected
        );
    }
    Ok(())
}

async fn git_optional(cwd: &Path, args: &[&str]) -> Option<String> {
    git(cwd, args).await.ok()
}

async fn git_succeeds(cwd: &Path, args: &[&str]) -> bool {
    git(cwd, args).await.is_ok()
}

async fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", OsStr::new("/dev/null"))
        .env("GIT_CONFIG_SYSTEM", OsStr::new("/dev/null"))
        .env("GIT_TERMINAL_PROMPT", OsStr::new("0"))
        .stdin(Stdio::null())
        .output()
        .await
        .with_context(|| format!("run git {} in {}", args.join(" "), cwd.display()))?;
    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            cwd.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn manifest_string<'a>(manifest: &'a Value, pointer: &str) -> Result<&'a str> {
    manifest
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("public manifest is missing {pointer}"))
}

fn write_exact(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        let existing = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        if existing != bytes {
            bail!(
                "refuse to replace unexpected fixture file {}",
                path.display()
            );
        }
        return Ok(());
    }
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

#[cfg(unix)]
fn make_probe_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o555);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("make {} executable", path.display()))
}

#[cfg(not(unix))]
fn make_probe_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn remove_directory(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

fn workspace_root(run_id: &str) -> PathBuf {
    let base = std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let base = fs::canonicalize(&base).unwrap_or(base);
    base.join("scenario-workspaces")
        .join(format!("{ID}-{run_id}"))
}

fn normalize_path(path: &Path) -> Option<PathBuf> {
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

fn safe_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn full_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_report() -> ForensicsReport {
        ForensicsReport {
            culprit_sha: CULPRIT_SHA.to_string(),
            culprit_parent_sha: CULPRIT_PARENT_SHA.to_string(),
            summary: "The changed term shifts the approximation away from 3.14.".to_string(),
            changed_paths: vec![CHANGED_PATH.to_string()],
            tested_commits: vec![
                GOOD_SHA.to_string(),
                BAD_SHA.to_string(),
                CULPRIT_SHA.to_string(),
            ],
            evidence: vec![ReportEvidence {
                commit: CULPRIT_SHA.to_string(),
                path: CHANGED_PATH.to_string(),
                observation: "The t7 term changes sign and magnitude.".to_string(),
            }],
        }
    }

    #[test]
    fn public_manifest_is_exact_and_does_not_leak_oracle() {
        let manifest: Value = serde_json::from_str(PUBLIC_MANIFEST).unwrap();
        validate_public_manifest(&manifest).unwrap();
        assert!(!PUBLIC_MANIFEST.contains(CULPRIT_SHA));
        assert!(!PUBLIC_MANIFEST.contains(CULPRIT_PARENT_SHA));
        let bundle_sha256 = sha256_bytes(BUNDLE_BYTES);
        let probe_sha256 = sha256_bytes(PROBE_BYTES);
        assert_eq!(
            Some(bundle_sha256.as_str()),
            manifest["bundle"]["sha256"].as_str()
        );
        assert_eq!(
            Some(probe_sha256.as_str()),
            manifest["probe"]["sha256"].as_str()
        );
    }

    #[tokio::test]
    async fn embedded_bundle_stops_at_commit_500_and_reproduces_boundary() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::create_dir_all(root.join("input")).unwrap();
        write_exact(&root.join(BUNDLE_RELATIVE_PATH), BUNDLE_BYTES).unwrap();
        write_exact(&root.join(PROBE_RELATIVE_PATH), PROBE_BYTES).unwrap();
        validate_fixture(root, &root.join("preflight"))
            .await
            .unwrap();
        let clone = root.join("preflight");
        assert_eq!(
            git(&clone, &["rev-list", "--count", BAD_SHA])
                .await
                .unwrap(),
            "500"
        );
        assert_eq!(
            git(&clone, &["ls-tree", "-r", "--name-only", BAD_SHA])
                .await
                .unwrap(),
            CHANGED_PATH
        );
    }

    #[tokio::test]
    async fn evidence_validation_rejects_wrong_changed_path() {
        let temporary = tempfile::tempdir().unwrap();
        let bundle = temporary.path().join("repository.bundle");
        fs::write(&bundle, BUNDLE_BYTES).unwrap();
        let clone = temporary.path().join("checkout");
        let bundle_arg = bundle.display().to_string();
        let clone_arg = clone.display().to_string();
        git(
            temporary.path(),
            &["clone", "--no-hardlinks", &bundle_arg, &clone_arg],
        )
        .await
        .unwrap();
        assert!(validate_report_evidence(&clone, &valid_report()).await);
        let mut invalid = valid_report();
        invalid.changed_paths = vec!["not-changed.py".to_string()];
        assert!(!validate_report_evidence(&clone, &invalid).await);
    }

    #[test]
    fn trace_metrics_distinguish_search_work_and_duplicates() {
        let records = [
            (GOOD_SHA, true),
            (BAD_SHA, false),
            ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", true),
            ("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", false),
            ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", true),
            (CULPRIT_SHA, false),
        ]
        .into_iter()
        .map(|(revision, passed)| ProbeRecord {
            revision: revision.to_string(),
            passed,
            program_exit_code: 0,
            observed_stdout: if passed { "3.14" } else { "3.57" }.to_string(),
            duration_ms: 1,
        })
        .collect::<Vec<_>>();
        let metrics = trace_metrics(&records);
        assert_eq!(metrics.total_probe_runs, 6);
        assert_eq!(metrics.unique_revisions, 5);
        assert_eq!(metrics.duplicate_probe_runs, 1);
        assert_eq!(metrics.unique_search_revisions, 3);
        assert_eq!(metrics.ideal_search_revisions, 9);
        assert_eq!(metrics.first_culprit_probe_index, Some(6));
    }

    #[test]
    fn report_contract_is_strict_and_requires_full_shas() {
        let encoded = serde_json::to_string(&valid_report()).unwrap();
        let decoded: ForensicsReport = serde_json::from_str(&encoded).unwrap();
        assert!(full_sha(&decoded.culprit_sha));
        let with_extra = encoded.replace(
            &format!("\"culprit_sha\":\"{CULPRIT_SHA}\""),
            &format!("\"culprit_sha\":\"{CULPRIT_SHA}\",\"unexpected\":true"),
        );
        assert!(serde_json::from_str::<ForensicsReport>(&with_extra).is_err());
        assert!(!full_sha("326f68a"));
        assert!(!full_sha("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"));
    }

    #[test]
    fn materialization_is_stable_and_publishes_three_assets() {
        let first = materialize("attempt-a", 91).unwrap();
        let retry = materialize("attempt-b", 91).unwrap();
        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert_eq!(first.case.deliverable_contract.artifacts.len(), 3);
        assert_eq!(first.case.complexity.profile.artifact_count, 3);
        assert!(!first.spec.prompt.contains(CULPRIT_SHA));
        first.validate().unwrap();
    }

    #[test]
    fn runner_owned_asset_shapes_satisfy_their_strict_schemas() {
        let contract = deliverable_contract();
        let samples = [
            json!({
                "manifest": {},
                "bundle": {},
                "probe": {},
                "clone": {},
                "runner_validation": {}
            }),
            json!({"records": [], "invalid_lines": [], "metrics": {}}),
        ];
        for (expectation, sample) in contract.artifacts.iter().zip(samples) {
            let schema = jsonschema::JSONSchema::compile(&expectation.schema).unwrap();
            assert!(
                schema.is_valid(&sample),
                "{} rejected its own shape",
                expectation.id
            );
        }
    }

    #[test]
    fn cleanup_is_idempotent() {
        let temporary = tempfile::tempdir().unwrap();
        let owned = temporary.path().join("owned");
        fs::create_dir_all(&owned).unwrap();
        fs::write(owned.join("file"), b"data").unwrap();
        remove_directory(&owned).unwrap();
        remove_directory(&owned).unwrap();
        assert!(!owned.exists());
    }
}
