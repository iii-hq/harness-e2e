//! Correct-chess-engine build benchmark.
//!
//! The subject is handed a pinned, frozen fixture repository (`iii-hq/e2e-fixture`,
//! `chess/` subtree) copied into its workspace. `engine/engine.py` ships with the
//! CLI plumbing done and two functions — `legal_moves(fen)` and `perft(fen, depth)`
//! — raising `NotImplementedError`. The subject implements fully correct standard
//! chess rules (castling, en passant, promotions, pins, check evasion) with the
//! standard library only, keeping a fixed CLI contract:
//!
//! - `python3 engine/engine.py perft <FEN> <DEPTH>` prints one integer.
//! - `python3 engine/engine.py legalmoves <FEN>` prints space-separated ascending UCI.
//!
//! Verification is entirely runner-side (no subject at evaluate time): the runner
//! executes the subject-authored engine over a fixed battery of positions and
//! compares its exact stdout against the shared chess kernel oracle
//! (`super::chess_engine`, which wraps `shakmaty`). The kernel is the single source
//! of truth for perft node counts and legal-move sets, so this scenario never
//! reimplements chess.
//!
//! Determinism and reproducibility: `materialize` is a pure function of its
//! `(namespace, seed)` and the pinned constants below — it never reads the
//! filesystem or environment, so `cargo test --lib` works without the fixture
//! present. Only `setup`, `evaluate`, `capture`, and `cleanup` touch the fixture
//! checkout or the copied workspace.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::json;
use tokio::process::Command;

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::chess_engine;
use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedDeliverableContent, CapturedInvariant,
    CleanupFuture, ComplexityProfile, DeliverableCaptureFuture, DeliverableContract,
    EvaluationFuture, ExecutionPolicy, InvariantSpec, MaterializedScenario, ProvenanceEvidence,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "chess_engine_build";
const VERSION: u32 = 2;

// --- Pinned, frozen fixture identity (do not edit) --------------------------
//
// The fixture is consumed via a local checkout of the repository root exported
// through `HARNESS_E2E_FIXTURE_PATH`; the relevant content lives under `chess/`.
const FIXTURE_PATH_ENV: &str = "HARNESS_E2E_FIXTURE_PATH";
const FIXTURE_REPOSITORY: &str = "iii-hq/e2e-fixture";
const CHESS_SUBTREE: &str = "chess";
const FIXTURE_REVISION: &str = "c4b68b8dc9588e730cd903ef679007a1a9974e80";
const CHESS_MANIFEST_SHA256: &str =
    "sha256:b2166cc0001a75a2afa0fdc1275d9252ac0e45bec6d4e59e6d04b2d53bd5f9f7";
const NETWORK_PROFILE: &str = "offline-v1";

/// Path of the engine relative to the copied workspace root (POSIX form).
const ENGINE_RELPATH: &str = "engine/engine.py";

const ENGINE_SOURCE_ID: &str = "engine_source";
const ENGINE_SOURCE_KIND: &str = "engine_source";
/// Text assets are validated by MIME (`text/*` + `utf-8`), never by their JSON
/// Schema, but a declared schema must still be a syntactically valid JSON Schema
/// — an empty schema accepts any text. This mirrors `engineering_ticket`'s
/// `candidate_patch` TextUtf8 artifact shape.
const ENGINE_SOURCE_MEDIA_TYPE: &str = "text/x-python; charset=utf-8";
const MAX_ENGINE_SOURCE_BYTES: u64 = 65_536;

/// Per-invocation wall-clock budget for one engine subprocess.
const ENGINE_TIMEOUT: Duration = Duration::from_secs(30);

/// L2Stateful profile: the subject consumes one external fixture system and
/// produces exactly one captured artifact. `external_systems > 0` alone derives
/// `L2Stateful`; `artifact_count == 1` matches the single-artifact contract.
const PROFILE: ComplexityProfile = ComplexityProfile {
    planning_depth: 1,
    dependency_depth: 0,
    parallel_branches: 0,
    external_systems: 1,
    state_transitions: 0,
    wake_cycles: 0,
    validation_loops: 0,
    artifact_count: 1,
    coordination_edges: 0,
    ambiguity_level: 0,
    agent_owned_decomposition: false,
    material_invalidation_events: 0,
    replan_loops: 0,
    compensable_mutations: 0,
    durable_resume_cycles: 0,
    coherent_long_horizon: false,
};

// --- Fixed verification battery (positions + expected via kernel oracle) -----

const KIWIPETE_FEN: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
const EN_PASSANT_FEN: &str = "rnbqkbnr/ppp1pppp/8/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3";
const PROMOTION_FEN: &str = "8/P7/8/8/8/8/8/k1K5 w - - 0 1";

/// The `(fen, depth)` perft invocations: STARTPOS depths 1..=4, Kiwipete depths
/// 1..=2, one en-passant position depth 1, one promotion position depth 1. The
/// expected node counts come exclusively from `chess_engine::perft`.
fn perft_plan() -> Vec<(&'static str, u32)> {
    let mut plan = Vec::new();
    for depth in 1..=4 {
        plan.push((chess_engine::STARTPOS, depth));
    }
    for depth in 1..=2 {
        plan.push((KIWIPETE_FEN, depth));
    }
    plan.push((EN_PASSANT_FEN, 1));
    plan.push((PROMOTION_FEN, 1));
    plan
}

/// The FENs at which `legalmoves` is compared against `chess_engine::legal_moves`.
fn legalmoves_fens() -> [&'static str; 4] {
    [
        chess_engine::STARTPOS,
        KIWIPETE_FEN,
        EN_PASSANT_FEN,
        PROMOTION_FEN,
    ]
}

// --- Assessments (weights total exactly 100) --------------------------------

const PERFT_EXACT: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "perft_exact",
    40,
    "Every perft position in the battery — STARTPOS depths 1-4, Kiwipete depths 1-2, an en-passant position, and a promotion position — reports a node count exactly equal to the shared chess kernel oracle. Any wrong count fails the gate.",
    EvaluationDimension::Deliverable,
);
const LEGAL_MOVES_CORRECT: AssessmentSpec = AssessmentSpec::hard_gated(
    "legal_moves_correct",
    30,
    "For every battery FEN the engine's ascending UCI legal-move line equals the kernel oracle's sorted legal-move set exactly.",
);
const INTERFACE_CONTRACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "interface_contract",
    20,
    "For every invocation the engine exits zero, writes empty stderr, and prints stdout that parses to the expected shape (a single integer for perft, space-separated UCI tokens for legalmoves): the engine actually runs and obeys the protocol.",
);
const BUILD_DISCIPLINE: AssessmentSpec = AssessmentSpec::score_only(
    "build_discipline",
    10,
    "Every engine invocation finished within the per-invocation time budget with no crash or timeout.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    PERFT_EXACT,
    LEGAL_MOVES_CORRECT,
    INTERFACE_CONTRACT,
    BUILD_DISCIPLINE,
];

// --- Scenario construction ---------------------------------------------------

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn materialize(namespace: &str, seed: u64) -> Result<MaterializedScenario> {
    // Pure: inputs are derived only from pinned constants — no filesystem or
    // environment access, so materialization works without the fixture present.
    let inputs = json!({
        "fixture_repository": FIXTURE_REPOSITORY,
        "fixture_subtree": CHESS_SUBTREE,
        "fixture_path_env": FIXTURE_PATH_ENV,
        "fixture_revision": FIXTURE_REVISION,
        "chess_manifest_sha256": CHESS_MANIFEST_SHA256,
        "engine_relpath": ENGINE_RELPATH,
        "perft_cli": "python3 engine/engine.py perft <FEN> <DEPTH>",
        "legalmoves_cli": "python3 engine/engine.py legalmoves <FEN>",
        "network_profile": NETWORK_PROFILE,
    });
    let case = ScenarioCase::new(
        ID,
        VERSION,
        seed,
        inputs,
        PROFILE,
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::coder".to_string(),
            "iii::shell".to_string(),
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
    let engine = engine_path(&root);
    let readme = root.join("README.md");
    let protocol = root.join("PROTOCOL.md");
    let public_perft = root.join("tests/public_perft.py");
    let public_legalmoves = root.join("tests/public_legalmoves.py");
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            r#"A pinned chess fixture repository has been copied into your workspace at `{root}`.

Read `{readme}` and `{protocol}` first — they describe the repository layout and the exact CLI
protocol you must preserve.

Implement the two functions in `{engine}`:
  - `legal_moves(fen)` — return the legal moves from `fen`.
  - `perft(fen, depth)` — return the exact perft node count from `fen` at `depth`.

Implement fully correct standard chess rules: castling (including rights and path/check
constraints), en passant, all four promotion pieces, absolute pins, and check evasion. Use the
Python standard library only — no third-party packages and no network access.

Keep the CLI contract exactly as shipped:
  - `python3 engine/engine.py perft <FEN> <DEPTH>` prints one integer.
  - `python3 engine/engine.py legalmoves <FEN>` prints the legal moves as space-separated UCI in
    ascending order.
In both cases write only that line to stdout, leave stderr empty, and exit 0.

You may self-check with `python3 {public_perft}` and `python3 {public_legalmoves}`. Do not edit the
tests, the protocol, or the fixture metadata. When you are done, finish with a one-line note that
the engine is implemented."#,
            root = root.display(),
            readme = readme.display(),
            protocol = protocol.display(),
            engine = engine.display(),
            public_perft = public_perft.display(),
            public_legalmoves = public_legalmoves.display(),
        ),
        filesystem_root: Some(root),
        execution: ExecutionPolicy {
            max_turns: 48,
            max_output_tokens: Some(16_384),
            max_total_tokens: Some(1_000_000),
            stuck_timeout_seconds: 600,
            max_validation_retries: None,
        },
        denied_functions: &["http::*", "browser::*", "github::*"],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: ENGINE_SOURCE_ID.to_string(),
            kind: ENGINE_SOURCE_KIND.to_string(),
            media_type: ENGINE_SOURCE_MEDIA_TYPE.to_string(),
            schema: json!({}),
            max_size_bytes: MAX_ENGINE_SOURCE_BYTES,
        }],
        invariants: vec![
            InvariantSpec {
                id: "perft_exact".to_string(),
                description:
                    "The captured engine reproduces every battery perft count exactly against the kernel oracle."
                        .to_string(),
            },
            InvariantSpec {
                id: "legal_moves_correct".to_string(),
                description:
                    "The captured engine reproduces every battery legal-move set exactly against the kernel oracle."
                        .to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

// --- setup: verify capabilities, validate + copy the frozen fixture ----------

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        for function in ["coder::read-file", "coder::update-file", "shell::exec"] {
            if !context.function_exists(function).await? {
                bail!("required chess-engine build capability '{function}' is unavailable");
            }
        }

        let fixture = fixture_root_from_env()?;
        let chess_dir = fixture.join(CHESS_SUBTREE);
        validate_fixture_chess_dir(&chess_dir)?;

        // The manifest is authoritative: recompute it identically over `chess/`.
        let manifest = compute_chess_manifest_sha256(&chess_dir)
            .context("recompute frozen chess/ fixture manifest")?;
        if manifest != CHESS_MANIFEST_SHA256 {
            bail!("chess fixture manifest {manifest} differs from pinned {CHESS_MANIFEST_SHA256}");
        }

        // The revision is advisory when this is a git checkout, best-effort.
        if fixture.join(".git").exists() {
            match git_head(&fixture).await {
                Some(head) if head != FIXTURE_REVISION => bail!(
                    "chess fixture HEAD {head} differs from pinned revision {FIXTURE_REVISION}"
                ),
                _ => {}
            }
        }

        // Copy the subtree into the workspace; never edit the checkout in place.
        let workspace = workspace_root(run_id);
        remove_workspace(&workspace)?;
        fs::create_dir_all(&workspace)
            .with_context(|| format!("create workspace {}", workspace.display()))?;
        copy_subtree(&chess_dir, &workspace)
            .with_context(|| format!("copy chess/ subtree into {}", workspace.display()))?;
        if !engine_path(&workspace).is_file() {
            bail!("chess workspace is missing engine/engine.py after copy");
        }
        Ok(())
    })
}

fn fixture_root_from_env() -> Result<PathBuf> {
    let raw = std::env::var_os(FIXTURE_PATH_ENV)
        .filter(|value| !value.is_empty())
        .with_context(|| {
            format!("{FIXTURE_PATH_ENV} must point to a local checkout of the chess fixture")
        })?;
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        bail!("{FIXTURE_PATH_ENV} must be absolute: {}", path.display());
    }
    // `canonicalize` also fails cleanly when the checkout does not exist.
    path.canonicalize()
        .with_context(|| format!("canonicalize chess fixture {}", path.display()))
}

fn validate_fixture_chess_dir(chess_dir: &Path) -> Result<()> {
    if !chess_dir.is_dir() {
        bail!(
            "chess fixture subtree is missing or not a directory: {}",
            chess_dir.display()
        );
    }
    let engine = chess_dir.join(ENGINE_RELPATH);
    if !engine.is_file() {
        bail!("chess fixture is missing {}", engine.display());
    }
    Ok(())
}

/// Recompute the `chess/` manifest SHA-256 identically to the frozen algorithm:
/// walk the subtree; collect files sorted by POSIX relpath (relative to
/// `chess/`); exclude any `__pycache__` directory and any `*.pyc`; for each file
/// emit `"<relpath>\n<lowercase-hex sha256 of file bytes>\n"`; concatenate in
/// sorted order; sha256 the concatenation; prefix with `sha256:`.
fn compute_chess_manifest_sha256(chess_dir: &Path) -> Result<String> {
    let mut relpaths = Vec::new();
    collect_manifest_files(chess_dir, chess_dir, &mut relpaths)?;
    relpaths.sort();
    let mut concatenation = Vec::new();
    for relpath in relpaths {
        let bytes = fs::read(chess_dir.join(&relpath))
            .with_context(|| format!("read fixture file {relpath}"))?;
        concatenation.extend_from_slice(relpath.as_bytes());
        concatenation.push(b'\n');
        concatenation.extend_from_slice(sha256_hex(&bytes).as_bytes());
        concatenation.push(b'\n');
    }
    Ok(crate::artifact::sha256_bytes(&concatenation))
}

fn collect_manifest_files(root: &Path, directory: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(directory)
        .with_context(|| format!("read directory {}", directory.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        let path = entry.path();
        if file_type.is_dir() {
            if name == OsStr::new("__pycache__") {
                continue;
            }
            collect_manifest_files(root, &path, out)?;
        } else if file_type.is_file() {
            if path.extension().and_then(OsStr::to_str) == Some("pyc") {
                continue;
            }
            let relpath = path
                .strip_prefix(root)
                .with_context(|| format!("relativize {}", path.display()))?
                .to_string_lossy()
                .replace('\\', "/");
            out.push(relpath);
        }
    }
    Ok(())
}

/// Lowercase-hex SHA-256 of `bytes` without the `sha256:` prefix.
fn sha256_hex(bytes: &[u8]) -> String {
    crate::artifact::sha256_bytes(bytes)
        .strip_prefix("sha256:")
        .expect("sha256_bytes always emits a sha256: prefix")
        .to_string()
}

/// Copy the contents of `source` into `destination`, excluding `__pycache__`
/// directories and `*.pyc` files (mirroring the manifest exclusion rule).
fn copy_subtree(source: &Path, destination: &Path) -> Result<()> {
    for entry in
        fs::read_dir(source).with_context(|| format!("read directory {}", source.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        let from = entry.path();
        if file_type.is_dir() {
            if name == OsStr::new("__pycache__") {
                continue;
            }
            let into = destination.join(&name);
            fs::create_dir_all(&into).with_context(|| format!("create {}", into.display()))?;
            copy_subtree(&from, &into)?;
        } else if file_type.is_file() {
            if from.extension().and_then(OsStr::to_str) == Some("pyc") {
                continue;
            }
            let into = destination.join(&name);
            fs::copy(&from, &into)
                .with_context(|| format!("copy {} -> {}", from.display(), into.display()))?;
        }
    }
    Ok(())
}

async fn git_head(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// --- Engine execution + pure comparison layer --------------------------------

/// One engine subprocess result, reduced to what the comparison layer needs.
#[derive(Debug, Clone)]
struct EngineRun {
    stdout_trimmed: String,
    stderr: String,
    exit_ok: bool,
    timed_out: bool,
}

/// Per-position verdict, split into the three orthogonal aspects the gates read.
#[derive(Debug, Clone)]
struct PositionReport {
    label: String,
    /// exit 0 AND empty stderr AND stdout parses to the expected shape.
    interface_ok: bool,
    /// `interface_ok` AND the parsed value equals the kernel oracle.
    value_ok: bool,
    /// Ran to completion within budget without crash or timeout.
    finished: bool,
}

#[derive(Debug, Clone)]
struct BatteryReport {
    perft: Vec<PositionReport>,
    legal: Vec<PositionReport>,
}

impl BatteryReport {
    fn all(&self) -> impl Iterator<Item = &PositionReport> {
        self.perft.iter().chain(self.legal.iter())
    }

    fn perft_exact(&self) -> bool {
        !self.perft.is_empty() && self.perft.iter().all(|report| report.value_ok)
    }

    fn legal_moves_correct(&self) -> bool {
        !self.legal.is_empty() && self.legal.iter().all(|report| report.value_ok)
    }

    fn interface_contract(&self) -> bool {
        self.all().all(|report| report.interface_ok)
    }

    fn build_discipline(&self) -> bool {
        self.all().all(|report| report.finished)
    }
}

/// Execute one engine invocation with a network-free, deterministic environment
/// and a per-invocation timeout. `python3 <engine_path> <args...>` runs with the
/// workspace root as its working directory.
async fn run_engine(engine_path: &Path, args: &[String]) -> Result<EngineRun> {
    let workspace = engine_path.parent().and_then(Path::parent);
    let mut command = Command::new("python3");
    command.arg(engine_path).args(args);
    if let Some(directory) = workspace {
        command.current_dir(directory);
    }
    command
        .stdin(Stdio::null())
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONHASHSEED", "0")
        .env("NO_PROXY", "*")
        .env("no_proxy", "*");
    match tokio::time::timeout(ENGINE_TIMEOUT, command.output()).await {
        Ok(output) => {
            let output = output.with_context(|| format!("run engine {}", engine_path.display()))?;
            Ok(EngineRun {
                stdout_trimmed: String::from_utf8_lossy(&output.stdout).trim().to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                exit_ok: output.status.success(),
                timed_out: false,
            })
        }
        Err(_) => Ok(EngineRun {
            stdout_trimmed: String::new(),
            stderr: String::new(),
            exit_ok: false,
            timed_out: true,
        }),
    }
}

/// Pure comparison for a perft invocation: the shape is a single unsigned integer.
fn evaluate_perft_run(label: String, run: &EngineRun, expected: u64) -> PositionReport {
    let parsed = parse_single_u64(&run.stdout_trimmed);
    let interface_ok = run.exit_ok && run.stderr.is_empty() && parsed.is_some();
    let value_ok = interface_ok && parsed == Some(expected);
    PositionReport {
        label,
        interface_ok,
        value_ok,
        finished: !run.timed_out && run.exit_ok,
    }
}

/// Pure comparison for a legalmoves invocation: the shape is a single line of
/// space-separated UCI tokens; the value must equal the sorted oracle set.
fn evaluate_legalmoves_run(label: String, run: &EngineRun, expected: &[String]) -> PositionReport {
    let parsed = parse_uci_line(&run.stdout_trimmed);
    let interface_ok = run.exit_ok && run.stderr.is_empty() && parsed.is_some();
    let value_ok = interface_ok && parsed.as_deref() == Some(expected);
    PositionReport {
        label,
        interface_ok,
        value_ok,
        finished: !run.timed_out && run.exit_ok,
    }
}

fn parse_single_u64(stdout: &str) -> Option<u64> {
    stdout.parse::<u64>().ok()
}

fn parse_uci_line(stdout: &str) -> Option<Vec<String>> {
    if stdout.contains('\n') {
        return None;
    }
    let tokens: Vec<String> = stdout.split_whitespace().map(str::to_string).collect();
    tokens
        .iter()
        .all(|token| is_uci_move(token))
        .then_some(tokens)
}

fn is_uci_move(token: &str) -> bool {
    let bytes = token.as_bytes();
    if bytes.len() != 4 && bytes.len() != 5 {
        return false;
    }
    let is_file = |byte: u8| (b'a'..=b'h').contains(&byte);
    let is_rank = |byte: u8| (b'1'..=b'8').contains(&byte);
    if !(is_file(bytes[0]) && is_rank(bytes[1]) && is_file(bytes[2]) && is_rank(bytes[3])) {
        return false;
    }
    if bytes.len() == 5 {
        matches!(bytes[4], b'q' | b'r' | b'b' | b'n')
    } else {
        true
    }
}

/// Drive the full battery: expected values come exclusively from the kernel
/// oracle, compared against the subject engine's exact stdout.
async fn run_full_battery(engine_path: &Path) -> Result<BatteryReport> {
    let mut perft = Vec::new();
    for (fen, depth) in perft_plan() {
        let expected = chess_engine::perft(fen, depth)
            .with_context(|| format!("kernel perft oracle for `{fen}` depth {depth}"))?;
        let args = vec!["perft".to_string(), fen.to_string(), depth.to_string()];
        let run = run_engine(engine_path, &args).await?;
        perft.push(evaluate_perft_run(
            format!("perft d{depth} `{fen}`"),
            &run,
            expected,
        ));
    }
    let mut legal = Vec::new();
    for fen in legalmoves_fens() {
        let expected = chess_engine::legal_moves(fen)
            .with_context(|| format!("kernel legalmoves oracle for `{fen}`"))?;
        let args = vec!["legalmoves".to_string(), fen.to_string()];
        let run = run_engine(engine_path, &args).await?;
        legal.push(evaluate_legalmoves_run(
            format!("legalmoves `{fen}`"),
            &run,
            &expected,
        ));
    }
    Ok(BatteryReport { perft, legal })
}

fn reason_over<'a>(
    aspect: &str,
    reports: impl Iterator<Item = &'a PositionReport>,
    pass: impl Fn(&PositionReport) -> bool,
) -> String {
    let mut total = 0usize;
    let mut failing = Vec::new();
    for report in reports {
        total += 1;
        if !pass(report) {
            failing.push(report.label.clone());
        }
    }
    if failing.is_empty() {
        format!("{aspect}: all {total} invocation(s) satisfied")
    } else {
        format!(
            "{aspect}: {}/{total} failing -> {}",
            failing.len(),
            failing.join("; ")
        )
    }
}

// --- evaluate: runner-side battery vs kernel oracle --------------------------

fn evaluate<'a>(
    _context: &'a E2eContext,
    _observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let engine = engine_path(&workspace_root(run_id));
        let battery = run_full_battery(&engine).await?;
        Ok(assessment::build_evaluation([
            PERFT_EXACT.full_or_zero(
                battery.perft_exact(),
                reason_over("perft_exact", battery.perft.iter(), |report| {
                    report.value_ok
                }),
            ),
            LEGAL_MOVES_CORRECT.full_or_zero(
                battery.legal_moves_correct(),
                reason_over("legal_moves_correct", battery.legal.iter(), |report| {
                    report.value_ok
                }),
            ),
            INTERFACE_CONTRACT.full_or_zero(
                battery.interface_contract(),
                reason_over("interface_contract", battery.all(), |report| {
                    report.interface_ok
                }),
            ),
            BUILD_DISCIPLINE.full_or_zero(
                battery.build_discipline(),
                reason_over("build_discipline", battery.all(), |report| report.finished),
            ),
        ]))
    })
}

// --- capture: read the engine source before cleanup --------------------------

fn capture<'a>(
    _context: &'a E2eContext,
    _observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let workspace = workspace_root(run_id);
        let engine = engine_path(&workspace);
        let source = read_engine_source(&engine);
        let battery = run_full_battery(&engine).await.ok();
        let perft_exact = battery.as_ref().is_some_and(BatteryReport::perft_exact);
        let legal_correct = battery
            .as_ref()
            .is_some_and(BatteryReport::legal_moves_correct);

        // Provenance is asserted only once both correctness gates pass.
        let provenance = if perft_exact && legal_correct {
            vec![ProvenanceEvidence {
                kind: "file".to_string(),
                source_id: ENGINE_RELPATH.to_string(),
                relation: "authored_engine".to_string(),
            }]
        } else {
            Vec::new()
        };

        Ok(vec![CapturedDeliverable {
            id: ENGINE_SOURCE_ID.to_string(),
            kind: ENGINE_SOURCE_KIND.to_string(),
            content: CapturedDeliverableContent::TextUtf8(source),
            invariants: vec![
                CapturedInvariant {
                    id: "perft_exact".to_string(),
                    passed: perft_exact,
                    reason: match battery.as_ref() {
                        Some(battery) => {
                            reason_over("perft_exact", battery.perft.iter(), |report| {
                                report.value_ok
                            })
                        }
                        None => "engine battery did not run".to_string(),
                    },
                },
                CapturedInvariant {
                    id: "legal_moves_correct".to_string(),
                    passed: legal_correct,
                    reason: match battery.as_ref() {
                        Some(battery) => {
                            reason_over("legal_moves_correct", battery.legal.iter(), |report| {
                                report.value_ok
                            })
                        }
                        None => "engine battery did not run".to_string(),
                    },
                },
            ],
            provenance,
        }])
    })
}

fn read_engine_source(engine_path: &Path) -> String {
    fs::read_to_string(engine_path).unwrap_or_default()
}

// --- cleanup: guarded removal of the copied workspace ------------------------

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move { remove_workspace(&workspace_root(run_id)) })
}

fn workspace_root(run_id: &str) -> PathBuf {
    let base = std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let base = fs::canonicalize(&base).unwrap_or(base);
    base.join("scenario-workspaces")
        .join(format!("{ID}-{run_id}"))
}

fn engine_path(workspace: &Path) -> PathBuf {
    workspace.join(ENGINE_RELPATH)
}

fn remove_workspace(root: &Path) -> Result<()> {
    guard_workspace_path(root)?;
    match fs::remove_dir_all(root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove workspace {}", root.display())),
    }
}

/// Refuse to remove anything that is not a scenario-owned workspace directory.
fn guard_workspace_path(root: &Path) -> Result<()> {
    let parent = root.parent().context("workspace path has no parent")?;
    if parent.file_name() != Some(OsStr::new("scenario-workspaces")) {
        bail!(
            "refuse to remove workspace outside the scenario-workspaces base: {}",
            root.display()
        );
    }
    let name = root
        .file_name()
        .and_then(OsStr::to_str)
        .context("workspace path has no name")?;
    if !name.starts_with(&format!("{ID}-")) {
        bail!(
            "refuse to remove workspace with an unexpected name: {}",
            root.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_ok(stdout: &str) -> EngineRun {
        EngineRun {
            stdout_trimmed: stdout.to_string(),
            stderr: String::new(),
            exit_ok: true,
            timed_out: false,
        }
    }

    #[test]
    fn pinned_manifest_and_revision_constants_are_well_formed() {
        let hex = CHESS_MANIFEST_SHA256
            .strip_prefix("sha256:")
            .expect("manifest constant must carry a sha256: prefix");
        assert_eq!(hex.len(), 64);
        assert!(hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
        assert_eq!(FIXTURE_REVISION.len(), 40);
        assert!(FIXTURE_REVISION
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit()));
    }

    /// Independently recompute the documented manifest over a controlled tiny
    /// tree (the real fixture cannot be embedded), and prove `__pycache__`
    /// directories and `*.pyc` files are excluded.
    #[test]
    fn manifest_algorithm_matches_an_independent_reconstruction() {
        let temporary = tempfile::tempdir().unwrap();
        let chess = temporary.path().join("chess");
        let files: [(&str, &[u8]); 3] = [
            ("README.md", b"readme\n"),
            ("engine/engine.py", b"engine-body\n"),
            ("engine/rules/moves.py", b"moves\n"),
        ];
        for (relpath, bytes) in files {
            let path = chess.join(relpath);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
        }
        // Excluded content must not affect the digest.
        fs::create_dir_all(chess.join("engine/__pycache__")).unwrap();
        fs::write(
            chess.join("engine/__pycache__/moves.cpython-311.pyc"),
            b"junk",
        )
        .unwrap();
        fs::write(chess.join("engine/rules/moves.pyc"), b"more junk").unwrap();

        let computed = compute_chess_manifest_sha256(&chess).unwrap();

        // Reconstruct the expected digest from the rule, sorted by relpath.
        let mut sorted = files.to_vec();
        sorted.sort_by(|left, right| left.0.cmp(right.0));
        let mut concatenation = Vec::new();
        for (relpath, bytes) in sorted {
            concatenation.extend_from_slice(relpath.as_bytes());
            concatenation.push(b'\n');
            concatenation.extend_from_slice(sha256_hex(bytes).as_bytes());
            concatenation.push(b'\n');
        }
        let expected = crate::artifact::sha256_bytes(&concatenation);

        assert_eq!(computed, expected);
        assert!(computed.starts_with("sha256:"));
        assert_eq!(computed.len(), "sha256:".len() + 64);
    }

    #[test]
    fn battery_expected_values_come_from_the_kernel_and_are_non_empty() {
        let plan = perft_plan();
        assert_eq!(plan.len(), 8);
        for (fen, depth) in plan {
            let expected = chess_engine::perft(fen, depth).unwrap();
            assert!(expected > 0, "perft `{fen}` d{depth} should be positive");
        }
        // Anchor a couple of canonical node counts explicitly.
        assert_eq!(chess_engine::perft(chess_engine::STARTPOS, 1).unwrap(), 20);
        assert_eq!(chess_engine::perft(KIWIPETE_FEN, 1).unwrap(), 48);

        for fen in legalmoves_fens() {
            let moves = chess_engine::legal_moves(fen).unwrap();
            assert!(
                !moves.is_empty(),
                "legal moves for `{fen}` should be non-empty"
            );
            let mut sorted = moves.clone();
            sorted.sort();
            assert_eq!(moves, sorted, "kernel oracle returns ascending UCI");
        }
    }

    #[test]
    fn pure_comparison_accepts_correct_and_rejects_wrong_perft() {
        let expected = chess_engine::perft(chess_engine::STARTPOS, 1).unwrap();
        let good = evaluate_perft_run("startpos d1".to_string(), &run_ok("20"), expected);
        assert!(good.interface_ok && good.value_ok && good.finished);

        // Wrong count: shape is valid, value is not.
        let wrong = evaluate_perft_run("startpos d1".to_string(), &run_ok("19"), expected);
        assert!(wrong.interface_ok && !wrong.value_ok);

        // Non-integer stdout is a shape (interface) failure.
        let malformed = evaluate_perft_run("startpos d1".to_string(), &run_ok("twenty"), expected);
        assert!(!malformed.interface_ok && !malformed.value_ok);

        // Non-empty stderr fails the interface even with correct stdout.
        let noisy = EngineRun {
            stderr: "warning".to_string(),
            ..run_ok("20")
        };
        let noisy = evaluate_perft_run("startpos d1".to_string(), &noisy, expected);
        assert!(!noisy.interface_ok && !noisy.value_ok);

        // Non-zero exit fails both interface and completion.
        let crashed = EngineRun {
            exit_ok: false,
            ..run_ok("20")
        };
        let crashed = evaluate_perft_run("startpos d1".to_string(), &crashed, expected);
        assert!(!crashed.interface_ok && !crashed.value_ok && !crashed.finished);
    }

    #[test]
    fn pure_comparison_accepts_correct_and_rejects_wrong_legalmoves() {
        let expected = chess_engine::legal_moves(chess_engine::STARTPOS).unwrap();
        let line = expected.join(" ");

        let good = evaluate_legalmoves_run("startpos".to_string(), &run_ok(&line), &expected);
        assert!(good.interface_ok && good.value_ok);

        // Wrong move set: valid shape, wrong value.
        let mut dropped = expected.clone();
        dropped.pop();
        let wrong = evaluate_legalmoves_run(
            "startpos".to_string(),
            &run_ok(&dropped.join(" ")),
            &expected,
        );
        assert!(wrong.interface_ok && !wrong.value_ok);

        // A token that is not a UCI move is a shape failure.
        let malformed = evaluate_legalmoves_run(
            "startpos".to_string(),
            &run_ok("e2e4 not-a-move"),
            &expected,
        );
        assert!(!malformed.interface_ok);

        // Non-empty stderr fails the interface even with the correct set.
        let noisy = EngineRun {
            stderr: "trace".to_string(),
            ..run_ok(&line)
        };
        let noisy = evaluate_legalmoves_run("startpos".to_string(), &noisy, &expected);
        assert!(!noisy.interface_ok && !noisy.value_ok);
    }

    #[tokio::test]
    async fn run_engine_executes_a_tiny_correct_script() {
        let temporary = tempfile::tempdir().unwrap();
        let engine = temporary.path().join("engine").join("engine.py");
        fs::create_dir_all(engine.parent().unwrap()).unwrap();
        // A minimal engine that satisfies exactly the STARTPOS-depth-1 perft.
        fs::write(
            &engine,
            "import sys\n\
             args = sys.argv[1:]\n\
             if args and args[0] == 'perft':\n\
             \x20   print(20)\n\
             else:\n\
             \x20   sys.exit(2)\n",
        )
        .unwrap();

        let run = run_engine(
            &engine,
            &[
                "perft".to_string(),
                chess_engine::STARTPOS.to_string(),
                "1".to_string(),
            ],
        )
        .await
        .unwrap();
        assert!(run.exit_ok && !run.timed_out);
        assert_eq!(run.stdout_trimmed, "20");
        assert!(run.stderr.is_empty());

        let expected = chess_engine::perft(chess_engine::STARTPOS, 1).unwrap();
        let report = evaluate_perft_run("startpos d1".to_string(), &run, expected);
        assert!(report.interface_ok && report.value_ok && report.finished);
    }

    #[test]
    fn materialize_is_reproducible_across_namespaces_and_is_l2_stateful() {
        use super::super::ComplexityTier;

        let seed = super::super::stable_seed(ID);
        let first = materialize("attempt-a", seed).unwrap();
        let retry = materialize("attempt-b", seed).unwrap();
        first.validate().unwrap();
        retry.validate().unwrap();

        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs, retry.case.inputs);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert_eq!(first.case.scenario_id, ID);
        assert_eq!(first.case.scenario_version, VERSION);
        assert_eq!(first.case.complexity.tier, ComplexityTier::L2Stateful);

        // Contract/capture/profile coherence.
        assert_eq!(
            usize::from(first.case.complexity.profile.artifact_count),
            first.case.deliverable_contract.artifacts.len()
        );
        assert_eq!(first.case.deliverable_contract.artifacts.len(), 1);
        assert!(first.capture.is_some());
        assert!(first.case.deliverable_contract.capture_before_cleanup);
        assert!(first.case.deliverable_contract.provenance_required);
        for invariant in &first.case.deliverable_contract.invariants {
            assert!(!invariant.description.trim().is_empty());
        }
        for artifact in &first.case.deliverable_contract.artifacts {
            jsonschema::JSONSchema::compile(&artifact.schema).unwrap();
            assert!(artifact.media_type.starts_with("text/"));
            assert!(artifact.media_type.to_ascii_lowercase().contains("utf-8"));
        }

        // Criterion weights total exactly 100 and the spec validates.
        first.spec.validate().unwrap();
        let total: u16 = first
            .spec
            .criteria
            .iter()
            .map(|criterion| u16::from(criterion.weight))
            .sum();
        assert_eq!(total, 100);
    }

    #[test]
    fn materialize_does_not_touch_the_fixture_or_its_environment() {
        // No fixture and no fixture env var are required to materialize: the
        // filesystem_root points at the copy destination under the workspace
        // base, never at the fixture checkout.
        let materialized = materialize("no-fixture", 7).unwrap();
        let root = materialized.spec.filesystem_root.expect("filesystem root");
        let root = root.to_string_lossy();
        assert!(root.contains("scenario-workspaces"));
        assert!(root.ends_with(&format!("{ID}-no-fixture")));
        assert!(materialized.capture.is_some());
    }

    #[test]
    fn guard_refuses_paths_outside_the_workspace_base() {
        let good = workspace_root("run-1");
        assert!(guard_workspace_path(&good).is_ok());

        assert!(guard_workspace_path(Path::new("/tmp/not-a-workspace")).is_err());
        assert!(
            guard_workspace_path(Path::new("/tmp/scenario-workspaces/other_scenario-run")).is_err()
        );
    }
}
