//! Port a working Python library to Rust: identical output, no dependencies,
//! one binary, and faster.
//!
//! The reference implementation ships with the scenario and is executable, so
//! the session can run it as often as it likes. What it cannot do is see the
//! scripts it will be judged on: those are written after the session ends and
//! rendered by the reference to produce the expected bytes. Parity and the
//! dependency and packaging rules are hard gates. Speed is advisory, because
//! a ratio measured on one machine is a comparison between runs rather than a
//! line to pass or fail.

use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::build::repo;
use crate::scenarios::deliverable::workspace;
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::probe;
use crate::scenarios::{
    CapturedInvariant, CleanupFuture, DeliverableCaptureFuture, EvaluationFuture,
    MaterializedScenario, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "port.python_to_rust";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "ported_renderer";
const REFERENCE: &str = "reference";
const PORT: &str = "port";
const BINARY: &str = "port/target/release/tte";
const MANIFEST: &str = "port/Cargo.toml";
const SAMPLE_SCRIPTS: &str = "corpus";
const HELD_SCRIPTS: &str = "verification";
const BUILD_TIMEOUT: Duration = Duration::from_secs(900);
const RENDER_TIMEOUT: Duration = Duration::from_secs(300);
/// Full marks for the advisory speed criteria at these values.
const TARGET_SPEEDUP: f64 = 5.0;
const TARGET_STARTUP_MS: f64 = 10.0;
const HOOK_TYPE: &str = "harness::hook::post-turn";
const HOOK_POINT: &str = "post_turn";
const READY: &str = "PORT_READY";
/// How many times the validator may send the session back to the job.
const CONTINUATIONS: u32 = 80;

const REFERENCE_FILES: [(&str, &str); 7] = [
    (
        "__init__.py",
        include_str!("../../../tests/fixtures/port-reference/__init__.py"),
    ),
    (
        "__main__.py",
        include_str!("../../../tests/fixtures/port-reference/__main__.py"),
    ),
    (
        "easing.py",
        include_str!("../../../tests/fixtures/port-reference/easing.py"),
    ),
    (
        "effects.py",
        include_str!("../../../tests/fixtures/port-reference/effects.py"),
    ),
    (
        "palette.py",
        include_str!("../../../tests/fixtures/port-reference/palette.py"),
    ),
    (
        "random.py",
        include_str!("../../../tests/fixtures/port-reference/random.py"),
    ),
    (
        "render.py",
        include_str!("../../../tests/fixtures/port-reference/render.py"),
    ),
];

const BUILDS_RELEASE: AssessmentSpec = AssessmentSpec::hard_gated(
    "builds_release",
    15,
    "The port builds in release mode and answers the version flag.",
);
const PARITY_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "parity_exact",
    40,
    "On scripts it never saw, the port's output is byte-identical to the reference implementation's.",
);
const NO_RUNTIME_DEPENDENCIES: AssessmentSpec = AssessmentSpec::hard_gated(
    "no_runtime_dependencies",
    15,
    "The manifest declares no runtime dependencies: the port stands on the standard library.",
);
const STANDALONE_BINARY: AssessmentSpec = AssessmentSpec::hard_gated(
    "standalone_binary",
    10,
    "The built binary runs from outside the source tree, carrying nothing with it.",
);
const FASTER_THAN_REFERENCE: AssessmentSpec = AssessmentSpec::score_only(
    "faster_than_reference",
    15,
    "How much faster the port renders the same heavy script. Advisory: a ratio on one machine compares runs, it does not pass or fail one.",
);
const QUICK_STARTUP: AssessmentSpec = AssessmentSpec::score_only(
    "quick_startup",
    5,
    "How long the binary takes to answer at all. Advisory, for the same reason.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    BUILDS_RELEASE,
    PARITY_EXACT,
    NO_RUNTIME_DEPENDENCIES,
    STANDALONE_BINARY,
    FASTER_THAN_REFERENCE,
    QUICK_STARTUP,
];

/// Scripts the session can see and develop against.
fn sample_scripts() -> Vec<(String, Value)> {
    vec![
        (
            "greeting.json".to_string(),
            json!({
                "width": 24,
                "frames": 6,
                "lines": [
                    { "text": "hello harness", "effect": "typewriter",
                      "options": { "easing": "out_quad", "colours": ["#ff0000", "#0000ff"] } },
                    { "text": "scatter me", "effect": "scatter",
                      "options": { "seed": 7, "colours": ["#00ff00"] } }
                ]
            }),
        ),
        (
            "pulse.json".to_string(),
            json!({
                "width": 12,
                "frames": 4,
                "lines": [
                    { "text": "pulse", "effect": "pulse",
                      "options": { "easing": "in_sine", "colours": ["#111111", "#eeeeee"] } }
                ]
            }),
        ),
    ]
}

/// Scripts written after the session ends. Every effect, every easing curve,
/// and the edges: one frame, text wider than the declared width, a seed that
/// changes the landing order, a gradient with three stops.
fn held_scripts() -> Vec<(String, Value)> {
    vec![
        (
            "every-effect.json".to_string(),
            json!({
                "width": 32,
                "frames": 9,
                "lines": [
                    { "text": "typewriter", "effect": "typewriter",
                      "options": { "easing": "in_out_cubic", "colours": ["#123456", "#abcdef"] } },
                    { "text": "wipe across", "effect": "wipe",
                      "options": { "easing": "out_bounce", "colours": ["#ff0000", "#00ff00", "#0000ff"], "leader": ">" } },
                    { "text": "scattered text", "effect": "scatter",
                      "options": { "seed": 991, "easing": "in_quad", "colours": ["#010203", "#fdfeff"] } },
                    { "text": "falling rain", "effect": "rain",
                      "options": { "seed": 42, "colours": ["#00ffff"] } },
                    { "text": "pulsing", "effect": "pulse",
                      "options": { "easing": "in_sine", "colours": ["#000000", "#ffffff"] } }
                ]
            }),
        ),
        (
            "single-frame.json".to_string(),
            json!({
                "width": 8,
                "frames": 1,
                "lines": [
                    { "text": "edge", "effect": "wipe", "options": { "colours": ["#ffffff"] } }
                ]
            }),
        ),
        (
            "narrow-width.json".to_string(),
            json!({
                "width": 3,
                "frames": 5,
                "lines": [
                    { "text": "wider than the width", "effect": "typewriter",
                      "options": { "easing": "linear", "colours": ["#ff00ff", "#00ff00"] } }
                ]
            }),
        ),
        (
            "reseeded.json".to_string(),
            json!({
                "width": 20,
                "frames": 7,
                "lines": [
                    { "text": "seed matters", "effect": "scatter",
                      "options": { "seed": 12345678901234567890_u64, "colours": ["#abcabc"] } },
                    { "text": "so does rain", "effect": "rain",
                      "options": { "seed": 3, "colours": ["#001122", "#334455", "#667788"] } }
                ]
            }),
        ),
    ]
}

/// A script big enough that the difference between the two implementations is
/// the thing being measured, not the process start.
fn heavy_script() -> Value {
    const EFFECTS: [&str; 5] = ["typewriter", "wipe", "scatter", "rain", "pulse"];
    const EASINGS: [&str; 4] = ["linear", "out_quad", "in_out_cubic", "out_bounce"];
    let lines: Vec<Value> = (0..12)
        .map(|index| {
            let effect = EFFECTS[index % EFFECTS.len()];
            let easing = EASINGS[index % EASINGS.len()];
            json!({
                "text": format!("line {index} of a heavy render workload"),
                "effect": effect,
                "options": {
                    "seed": 1000 + index,
                    "easing": easing,
                    "colours": ["#102030", "#405060", "#708090"],
                }
            })
        })
        .collect();
    json!({ "width": 120, "frames": 300, "lines": lines })
}

fn write_scripts(root: &Path, directory: &str, scripts: &[(String, Value)]) -> anyhow::Result<()> {
    for (name, script) in scripts {
        workspace::write(
            root,
            &format!("{directory}/{name}"),
            &serde_json::to_string_pretty(script)?,
        )?;
    }
    Ok(())
}

/// The hook contract's answer shape.
#[derive(Debug, Serialize)]
struct HookVerdict {
    decision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

impl HookVerdict {
    fn carry_on() -> Self {
        Self {
            decision: "continue".into(),
            reason: None,
        }
    }

    fn value(self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({ "decision": "continue" }))
    }
}

fn keep_working(reason: impl Into<String>) -> HookVerdict {
    HookVerdict {
        decision: "deny".into(),
        reason: Some(reason.into()),
    }
}

/// What exists so far, in one line, so a session that has been through
/// compaction can re-orient without re-reading the tree.
fn progress_note(root: &Path) -> String {
    let present = |relative: &str| {
        if root.join(relative).exists() {
            "yes"
        } else {
            "no"
        }
    };
    format!(
        "so far: {MANIFEST} {}, port/src {}, {BINARY} {}",
        present(MANIFEST),
        present("port/src"),
        present(BINARY)
    )
}

/// A turn ending is not the job ending. Until the port builds and matches the
/// reference on the sample scripts, every turn is sent back to work with what
/// is missing; only then does the validator let the session finish.
async fn continuation_verdict(root: &Path, claimed_ready: bool) -> HookVerdict {
    if !root.join(MANIFEST).exists() {
        return keep_working(format!(
            "The port does not exist yet: there is no {MANIFEST}. Keep working; {}",
            progress_note(root)
        ));
    }
    if !claimed_ready {
        return keep_working(format!(
            "Keep going until the port builds and renders the corpus scripts exactly like the \
             reference, then reply with exactly {READY}. {}",
            progress_note(root)
        ));
    }

    let build = repo::run(
        root,
        "cargo",
        &["build", "--release", "--manifest-path", MANIFEST],
        BUILD_TIMEOUT,
    )
    .await;
    match build {
        Some(run) if run.status == Some(0) => {}
        Some(run) => {
            let tail: String = run.stderr.chars().rev().take(600).collect();
            let tail: String = tail.chars().rev().collect();
            return keep_working(format!(
                "The release build failed. Fix it and try again:\n{tail}"
            ));
        }
        None => {
            return keep_working(
                "The release build did not finish inside its time budget. Simplify the build and \
                 try again.",
            )
        }
    }

    for (name, _) in sample_scripts() {
        let script = format!("{SAMPLE_SCRIPTS}/{name}");
        let expected = repo::run(
            root,
            "python3",
            &["-m", REFERENCE, "render", &script],
            RENDER_TIMEOUT,
        )
        .await;
        let observed = repo::run(root, BINARY, &["render", &script], RENDER_TIMEOUT).await;
        match (expected, observed) {
            (Some(expected), Some(observed)) if expected.stdout == observed.stdout => {}
            (Some(expected), Some(observed)) => {
                let first = expected
                    .stdout
                    .char_indices()
                    .zip(observed.stdout.chars())
                    .find(|((_, left), right)| left != right)
                    .map(|((index, _), _)| index);
                return keep_working(format!(
                    "`{script}` does not match the reference yet: first difference at byte \
                     {first:?} of {} expected bytes. Compare the two outputs and fix the port.",
                    expected.stdout.len()
                ));
            }
            _ => {
                return keep_working(format!(
                    "`{script}` could not be rendered by both implementations. Make \
                     `{BINARY} render <script>` work first."
                ))
            }
        }
    }

    HookVerdict::carry_on()
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let root = workspace::root(ID, run_id);
        for (name, source) in REFERENCE_FILES {
            workspace::write(&root, &format!("{REFERENCE}/{name}"), source)?;
        }
        write_scripts(&root, SAMPLE_SCRIPTS, &sample_scripts())?;

        let validator = probe::id("port_gate", run_id);
        let workspace_root = root.clone();
        probe::register(
            context,
            validator.clone(),
            "E2E port validator: sends the session back to work until the port builds and matches \
             the reference.",
            move |envelope: Value| {
                let workspace_root = workspace_root.clone();
                async move {
                    // A wrong hook point means this validator is gating
                    // something other than turn completion: say so by letting
                    // it through rather than blocking the wrong thing.
                    if envelope.get("point").and_then(Value::as_str) != Some(HOOK_POINT) {
                        return Ok(HookVerdict::carry_on().value());
                    }
                    let claimed_ready = match envelope.get("result") {
                        Some(Value::String(text)) => text.contains(READY),
                        Some(other) => other.to_string().contains(READY),
                        None => false,
                    };
                    Ok(continuation_verdict(&workspace_root, claimed_ready)
                        .await
                        .value())
                }
            },
        );

        // The runner installs the binding itself, scoped to this run's
        // session: a scenario that depends on the session registering its own
        // continuation is one prompt-following slip away from ending early.
        context
            .trigger_value(
                "engine::register_trigger",
                json!({
                    "trigger_type": HOOK_TYPE,
                    "function_id": validator,
                    "config": {
                        "sessions": [format!("e2e_{run_id}")],
                        "timeout_ms": 900_000_u64,
                    },
                }),
            )
            .await?;
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Port a working library from Python to Rust. This is a long job; take the turns it \
             needs.\n\n\
             The reference implementation is `{REFERENCE}/`, a text-effects renderer you can run \
             right now:\n\
             - `python3 -m {REFERENCE} render <script.json>` writes rendered frames to stdout.\n\
             - `python3 -m {REFERENCE} --version` writes its version.\n\
             Read it. It is the specification: the easing curves, the colour rounding, the \
             bundled random generator, the order effects resolve in, the frame separator, and the \
             padding rules are all decided there, and some of them are decided in ways a rewrite \
             gets wrong by default.\n\n\
             Build the port:\n\
             1. A Rust crate in `{PORT}/` that builds with `cargo build --release --manifest-path \
             {MANIFEST}` and produces the binary `{BINARY}`.\n\
             2. The same command line: `{BINARY} render <script.json>` and `{BINARY} --version`.\n\
             3. Byte-identical stdout to the reference for the same script. Not equivalent, not \
             visually the same: the same bytes, escape sequences and all.\n\
             4. No runtime dependencies. `[dependencies]` in the manifest stays empty; the \
             standard library is the whole toolbox. Development dependencies are fine.\n\
             5. The binary must run on its own, from any directory, with no source tree beside \
             it.\n\
             6. It should be substantially faster than the reference. Measure it rather than \
             assuming it.\n\n\
             Sample scripts are in `{SAMPLE_SCRIPTS}/`. They are samples: the port will be \
             checked against scripts you have not seen, covering every effect, every easing \
             curve, multi-stop gradients, a single-frame script, text wider than the declared \
             width, and other seeds. Match the reference's behaviour, not these files.\n\n\
             A validator checks your work at the end of every turn and will send you back to \
             it with what is still wrong, so do not stop early: keep going until it accepts. \
             When the port builds and renders the sample scripts exactly like the reference, \
             reply with exactly one line: `{READY}`."
        ),
        filesystem_root: Some(workspace::root(ID, run_id)),
        execution: kit::long_policy(400, 32_768, 12_000_000, 3_600, CONTINUATIONS),
        assessments: ASSESSMENTS,
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
    .spec()
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = super::case(
        ID,
        VERSION,
        seed,
        json!({
            "reference": REFERENCE,
            "binary": BINARY,
            "manifest": MANIFEST,
            "sample_scripts": sample_scripts().iter().map(|(name, _)| name).collect::<Vec<_>>(),
            "verification": {
                "held_scripts": held_scripts().iter().map(|(name, _)| name).collect::<Vec<_>>(),
                "parity": "byte-identical stdout against the reference",
                "advisory": ["faster_than_reference", "quick_startup"],
            },
        }),
        super::port_profile(),
        &[],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["parity", "speedup", "response"],
                "additionalProperties": true
            }),
            ASSESSMENTS,
        ),
    )?;
    Ok(MaterializedScenario {
        spec: scenario(namespace),
        case,
        capture: Some(capture),
    })
}

struct Verification {
    built: bool,
    matched: Vec<String>,
    diverged: Vec<String>,
    no_dependencies: bool,
    standalone: bool,
    speedup: Option<f64>,
    startup_ms: Option<f64>,
    stderr: String,
}

/// `[dependencies]` must be empty. Dev-dependencies and profile sections are
/// none of this gate's business, so only that one table is read.
fn declares_no_dependencies(manifest: &str) -> bool {
    let mut inside = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[dependencies]";
            continue;
        }
        if inside && !line.is_empty() && !line.starts_with('#') {
            return false;
        }
    }
    true
}

async fn timed(
    root: &Path,
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Option<(repo::Execution, f64)> {
    let started = Instant::now();
    let run = repo::run(root, program, args, timeout).await?;
    Some((run, started.elapsed().as_secs_f64() * 1000.0))
}

/// One verification per attempt, shared by the evaluation and the captured
/// evidence. Re-running it would repeat the work and, where anything is
/// timed, answer differently the second time.
static VERIFIED: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<Verification>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

fn cached(run_id: &str) -> Option<std::sync::Arc<Verification>> {
    VERIFIED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(run_id)
        .cloned()
}

async fn verify(run_id: &str) -> std::sync::Arc<Verification> {
    if let Some(verification) = cached(run_id) {
        return verification;
    }
    let verification = std::sync::Arc::new(run_verification(run_id).await);
    VERIFIED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(run_id.to_string(), std::sync::Arc::clone(&verification));
    verification
}

fn forget_verification(run_id: &str) {
    VERIFIED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(run_id);
}

async fn run_verification(run_id: &str) -> Verification {
    let root = workspace::root(ID, run_id);
    let held = held_scripts();
    let mut verification = Verification {
        built: false,
        matched: Vec::new(),
        diverged: held.iter().map(|(name, _)| name.clone()).collect(),
        no_dependencies: false,
        standalone: false,
        speedup: None,
        startup_ms: None,
        stderr: String::new(),
    };

    let build = repo::run(
        &root,
        "cargo",
        &["build", "--release", "--manifest-path", MANIFEST],
        BUILD_TIMEOUT,
    )
    .await;
    verification.stderr = build
        .as_ref()
        .map(|run| run.stderr.chars().rev().take(512).collect::<String>())
        .unwrap_or_default()
        .chars()
        .rev()
        .collect();
    if build.as_ref().is_none_or(|run| run.status != Some(0)) {
        return verification;
    }
    let version = repo::run(&root, BINARY, &["--version"], RENDER_TIMEOUT).await;
    verification.built = version.as_ref().is_some_and(|run| run.status == Some(0));
    if !verification.built {
        return verification;
    }

    if write_scripts(&root, HELD_SCRIPTS, &held).is_ok() {
        verification.matched.clear();
        verification.diverged.clear();
        for (name, _) in &held {
            let script = format!("{HELD_SCRIPTS}/{name}");
            let expected = repo::run(
                &root,
                "python3",
                &["-m", REFERENCE, "render", &script],
                RENDER_TIMEOUT,
            )
            .await;
            let observed = repo::run(&root, BINARY, &["render", &script], RENDER_TIMEOUT).await;
            let agrees = match (expected.as_ref(), observed.as_ref()) {
                (Some(expected), Some(observed)) => {
                    expected.status == Some(0)
                        && observed.status == Some(0)
                        && expected.stdout == observed.stdout
                }
                _ => false,
            };
            if agrees {
                verification.matched.push(name.clone());
            } else {
                verification.diverged.push(name.clone());
            }
        }
    }

    verification.no_dependencies = workspace::read(&root, MANIFEST)
        .as_deref()
        .is_some_and(declares_no_dependencies);

    // A binary that needs its source tree beside it is not a single executable.
    let elsewhere = std::env::temp_dir().join(format!("harness-e2e-port-{run_id}"));
    if std::fs::create_dir_all(&elsewhere).is_ok() {
        let carried = elsewhere.join("tte");
        if std::fs::copy(root.join(BINARY), &carried).is_ok() {
            verification.standalone = repo::run(
                &elsewhere,
                carried.to_string_lossy().as_ref(),
                &["--version"],
                RENDER_TIMEOUT,
            )
            .await
            .is_some_and(|run| run.status == Some(0));
        }
        let _ = std::fs::remove_dir_all(&elsewhere);
    }

    if workspace::write(
        &root,
        "verification/heavy.json",
        &serde_json::to_string(&heavy_script()).unwrap_or_default(),
    )
    .is_ok()
    {
        let reference = timed(
            &root,
            "python3",
            &["-m", REFERENCE, "render", "verification/heavy.json"],
            RENDER_TIMEOUT,
        )
        .await;
        let ported = timed(
            &root,
            BINARY,
            &["render", "verification/heavy.json"],
            RENDER_TIMEOUT,
        )
        .await;
        if let (Some((reference_run, reference_ms)), Some((ported_run, ported_ms))) =
            (reference, ported)
        {
            if reference_run.status == Some(0) && ported_run.status == Some(0) {
                verification.speedup = Some(reference_ms / ported_ms.max(0.001));
            }
        }
    }
    verification.startup_ms = timed(&root, BINARY, &["--version"], RENDER_TIMEOUT)
        .await
        .map(|(_, elapsed)| elapsed);

    verification
}

fn speed_points(speedup: Option<f64>) -> u8 {
    match speedup {
        Some(ratio) if ratio > 1.0 => {
            let share = ((ratio - 1.0) / (TARGET_SPEEDUP - 1.0)).clamp(0.0, 1.0);
            (f64::from(FASTER_THAN_REFERENCE.weight()) * share).round() as u8
        }
        _ => 0,
    }
}

fn startup_points(startup_ms: Option<f64>) -> u8 {
    match startup_ms {
        Some(elapsed) => {
            let share = (TARGET_STARTUP_MS / elapsed.max(0.001)).clamp(0.0, 1.0);
            (f64::from(QUICK_STARTUP.weight()) * share).round() as u8
        }
        None => 0,
    }
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let verification = verify(run_id).await;
        let expected = held_scripts().len();

        Ok(assessment::build_evaluation([
            BUILDS_RELEASE.full_or_zero(
                verification.built && observation.response.contains("PORT_READY"),
                format!(
                    "release build and version flag succeeded: {}; build tail: {}",
                    verification.built, verification.stderr
                ),
            ),
            PARITY_EXACT.full_or_zero(
                verification.matched.len() == expected && verification.diverged.is_empty(),
                format!(
                    "{} of {expected} held-out script(s) matched byte for byte; diverged: {:?}",
                    verification.matched.len(),
                    verification.diverged
                ),
            ),
            NO_RUNTIME_DEPENDENCIES.full_or_zero(
                verification.no_dependencies,
                format!(
                    "`{MANIFEST}` declares no runtime dependencies: {}",
                    verification.no_dependencies
                ),
            ),
            STANDALONE_BINARY.full_or_zero(
                verification.standalone,
                format!(
                    "the copied binary ran outside the tree: {}",
                    verification.standalone
                ),
            ),
            FASTER_THAN_REFERENCE.award(
                speed_points(verification.speedup),
                format!(
                    "{:?}x the reference on the heavy script, full marks at {TARGET_SPEEDUP}x",
                    verification.speedup
                ),
            )?,
            QUICK_STARTUP.award(
                startup_points(verification.startup_ms),
                format!(
                    "{:?}ms to answer the version flag, full marks at {TARGET_STARTUP_MS}ms",
                    verification.startup_ms
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
        let verification = verify(run_id).await;
        let invariants = vec![
            CapturedInvariant {
                id: "parity_exact".to_string(),
                passed: verification.diverged.is_empty() && !verification.matched.is_empty(),
                reason: format!("diverged on {:?}", verification.diverged),
            },
            CapturedInvariant {
                id: "no_runtime_dependencies".to_string(),
                passed: verification.no_dependencies,
                reason: format!("manifest read from `{MANIFEST}`"),
            },
        ];
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "parity": {
                    "matched": verification.matched,
                    "diverged": verification.diverged,
                },
                "speedup": verification.speedup,
                "startup_ms": verification.startup_ms,
                "standalone": verification.standalone,
                "turns": observation.metrics.totals.turns,
                "output_tokens": observation.metrics.totals.output_tokens,
                "cost_usd": observation.metrics.totals.cost_usd,
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_port_verification_before_cleanup",
            )],
        )])
    })
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        forget_verification(run_id);
        probe::release(run_id);
        workspace::remove(&workspace::root(ID, run_id));
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_ships_every_module_it_imports() {
        let names: Vec<&str> = REFERENCE_FILES.iter().map(|(name, _)| *name).collect();
        for module in [
            "easing.py",
            "effects.py",
            "palette.py",
            "random.py",
            "render.py",
        ] {
            assert!(names.contains(&module), "{module} is missing");
        }
        for (name, source) in REFERENCE_FILES {
            assert!(!source.trim().is_empty(), "{name} is empty");
        }
    }

    #[test]
    fn held_scripts_cover_every_effect_and_are_not_the_samples() {
        let held = serde_json::to_string(&held_scripts()).unwrap();
        for effect in ["typewriter", "wipe", "scatter", "rain", "pulse"] {
            assert!(held.contains(effect), "{effect} is never exercised");
        }
        let sample_names: Vec<String> =
            sample_scripts().into_iter().map(|(name, _)| name).collect();
        for (name, _) in held_scripts() {
            assert!(!sample_names.contains(&name), "{name} is in both sets");
        }
    }

    #[test]
    fn an_empty_dependency_table_is_the_only_one_accepted() {
        assert!(declares_no_dependencies(
            "[package]\nname = \"tte\"\n\n[dependencies]\n\n[dev-dependencies]\ntempfile = \"3\"\n"
        ));
        assert!(declares_no_dependencies("[package]\nname = \"tte\"\n"));
        assert!(!declares_no_dependencies(
            "[package]\nname = \"tte\"\n\n[dependencies]\nserde = \"1\"\n"
        ));
    }

    #[test]
    fn advisory_speed_points_scale_with_the_measurement() {
        assert_eq!(speed_points(None), 0);
        assert_eq!(speed_points(Some(1.0)), 0);
        assert_eq!(
            speed_points(Some(TARGET_SPEEDUP)),
            FASTER_THAN_REFERENCE.weight()
        );
        assert_eq!(speed_points(Some(50.0)), FASTER_THAN_REFERENCE.weight());
        assert!(speed_points(Some(3.0)) > 0);
        assert_eq!(
            startup_points(Some(TARGET_STARTUP_MS / 2.0)),
            QUICK_STARTUP.weight()
        );
        assert!(startup_points(Some(100.0)) < QUICK_STARTUP.weight());
    }
}
