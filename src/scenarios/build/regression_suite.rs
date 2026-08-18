//! Build a regression suite for a library, then judge the suite by breaking
//! the library underneath it. A suite that passes everything and a suite that
//! fails everything are both worthless, and both fail here.

use std::path::Path;
use std::time::Duration;

use serde_json::json;

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::deliverable::workspace;
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CapturedInvariant, CleanupFuture, DeliverableCaptureFuture, EvaluationFuture,
    MaterializedScenario, ScenarioObservation, ScenarioSpec,
};

use super::repo;

pub const ID: &str = "build.regression_suite";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "regression_suite_system";
const LIBRARY: &str = "library/pricing.py";
const TEST_DIRECTORY: &str = "tests";
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

const SUITE_RUNS_GREEN: AssessmentSpec = AssessmentSpec::hard_gated(
    "suite_runs_green",
    20,
    "The suite passes against the library as written.",
);
const DEFECTS_CAUGHT: AssessmentSpec = AssessmentSpec::hard_gated(
    "defects_caught",
    45,
    "Every behavioural defect the runner introduces afterwards makes the suite fail.",
);
const NO_FALSE_ALARM: AssessmentSpec = AssessmentSpec::hard_gated(
    "no_false_alarm",
    20,
    "A change that alters no behaviour leaves the suite green, so it is not simply failing always.",
);
const SUITE_RESTORED: AssessmentSpec = AssessmentSpec::hard_gated(
    "suite_restored",
    15,
    "With the library restored, the suite is green again: the defects were what failed it.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    SUITE_RUNS_GREEN,
    DEFECTS_CAUGHT,
    NO_FALSE_ALARM,
    SUITE_RESTORED,
];

const PRISTINE: &[&str] = &[
    "TAX_RATE = 0.2",
    "",
    "",
    "def subtotal(items):",
    "    return round(sum(price * quantity for price, quantity in items), 2)",
    "",
    "",
    "def tax(amount):",
    "    return round(amount * TAX_RATE, 2)",
    "",
    "",
    "def total(items):",
    "    net = subtotal(items)",
    "    return round(net + tax(net), 2)",
    "",
    "",
    "def discount(amount, code):",
    "    if code == \"HALF\":",
    "        return round(amount / 2, 2)",
    "    if code == \"TENOFF\":",
    "        return round(max(amount - 10, 0), 2)",
    "    return amount",
];

/// Each defect replaces exactly one line with something that changes what the
/// library does. A suite worth having notices every one of them.
const DEFECTS: [(&str, &str, &str); 4] = [
    ("tax_rate_changed", "TAX_RATE = 0.2", "TAX_RATE = 0.25"),
    (
        "discount_floor_removed",
        "        return round(max(amount - 10, 0), 2)",
        "        return round(amount - 10, 2)",
    ),
    (
        "discount_code_case_changed",
        "    if code == \"HALF\":",
        "    if code == \"half\":",
    ),
    (
        "quantity_ignored",
        "    return round(sum(price * quantity for price, quantity in items), 2)",
        "    return round(sum(price for price, _quantity in items), 2)",
    ),
];

/// A change with no behavioural effect: a suite that fails this one is failing
/// for the wrong reason.
const INERT_CHANGE: &str = "\n# reviewed by the pricing team\n";

fn library_source() -> String {
    let mut source = PRISTINE.join("\n");
    source.push('\n');
    source
}

fn defective_source(target: &str, replacement: &str) -> String {
    let mut source = library_source();
    assert!(source.contains(target));
    source = source.replace(target, replacement);
    source
}

fn write_library(root: &Path, source: &str) -> bool {
    workspace::write(root, LIBRARY, source).is_ok()
}

async fn suite_passes(root: &Path) -> Option<bool> {
    let run = repo::run(
        root,
        "python3",
        &[
            "-m",
            "unittest",
            "discover",
            "-s",
            TEST_DIRECTORY,
            "-t",
            ".",
        ],
        RUN_TIMEOUT,
    )
    .await?;
    Some(run.status == Some(0))
}

fn setup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let root = workspace::root(ID, run_id);
        workspace::write(&root, LIBRARY, &library_source())?;
        workspace::write(
            &root,
            "library/__init__.py",
            "from .pricing import discount, subtotal, tax, total\n",
        )?;
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Build a regression suite for the pricing library in this workspace. Take as many \
             turns as you need.\n\n\
             The library is `{LIBRARY}`. Read it: it computes a subtotal from `(price, \
             quantity)` pairs, a tax at the module's rate, a total, and a discount for the codes \
             `HALF` and `TENOFF`, where `TENOFF` never takes a total below zero.\n\n\
             The suite:\n\
             1. Lives under `{TEST_DIRECTORY}/` and runs with \
             `python3 -m unittest discover -s {TEST_DIRECTORY} -t .` from the workspace root, \
             exiting 0 when everything passes.\n\
             2. Uses only the Python 3 standard library and imports the library rather than \
             copying its logic.\n\
             3. Pins the behaviour that matters: the tax rate, that quantity is part of the \
             subtotal, that the total is subtotal plus tax, that each discount code applies, that \
             an unknown code changes nothing, and that `TENOFF` floors at zero.\n\
             4. Asserts values, not implementation details. Do not assert on source text, file \
             contents, or line numbers.\n\n\
             The library will be modified after you finish, in ways that change what it \
             computes. Your suite has to fail when that happens, and only when that happens.\n\n\
             When the suite is green, reply with exactly one line: `SUITE_READY`."
        ),
        filesystem_root: Some(workspace::root(ID, run_id)),
        execution: kit::policy(40, 600_000, 900),
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
            "library": LIBRARY,
            "test_directory": TEST_DIRECTORY,
            "verification": {
                "defects": DEFECTS.iter().map(|(name, _, _)| name).collect::<Vec<_>>(),
                "inert_change_expected_green": true,
            },
        }),
        super::system_profile(3, 6),
        &["iii::shell"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["green_before", "caught", "response"],
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
    green_before: bool,
    caught: Vec<String>,
    missed: Vec<String>,
    inert_stayed_green: bool,
    green_after: bool,
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
    let green_before = suite_passes(&root).await.unwrap_or(false);

    let mut caught = Vec::new();
    let mut missed = Vec::new();
    for (name, target, replacement) in DEFECTS {
        if !write_library(&root, &defective_source(target, replacement)) {
            missed.push(name.to_string());
            continue;
        }
        match suite_passes(&root).await {
            Some(false) => caught.push(name.to_string()),
            _ => missed.push(name.to_string()),
        }
    }

    let mut inert = library_source();
    inert.push_str(INERT_CHANGE);
    let inert_stayed_green =
        write_library(&root, &inert) && suite_passes(&root).await.unwrap_or(false);

    let green_after =
        write_library(&root, &library_source()) && suite_passes(&root).await.unwrap_or(false);

    Verification {
        green_before,
        caught,
        missed,
        inert_stayed_green,
        green_after,
    }
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let verification = verify(run_id).await;

        Ok(assessment::build_evaluation([
            SUITE_RUNS_GREEN.full_or_zero(
                verification.green_before && observation.response.contains("SUITE_READY"),
                format!(
                    "suite green before any change: {}",
                    verification.green_before
                ),
            ),
            DEFECTS_CAUGHT.full_or_zero(
                verification.caught.len() == DEFECTS.len(),
                format!(
                    "caught {:?}; missed {:?}",
                    verification.caught, verification.missed
                ),
            ),
            NO_FALSE_ALARM.full_or_zero(
                verification.inert_stayed_green,
                format!(
                    "a comment-only change left the suite green: {}",
                    verification.inert_stayed_green
                ),
            ),
            SUITE_RESTORED.full_or_zero(
                verification.green_after,
                format!(
                    "suite green again after restoring the library: {}",
                    verification.green_after
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
        let verification = verify(run_id).await;
        let invariants = vec![
            CapturedInvariant {
                id: "defects_caught".to_string(),
                passed: verification.caught.len() == DEFECTS.len(),
                reason: format!("missed {:?}", verification.missed),
            },
            CapturedInvariant {
                id: "no_false_alarm".to_string(),
                passed: verification.inert_stayed_green,
                reason: "a behaviour-preserving change must stay green".to_string(),
            },
        ];
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "green_before": verification.green_before,
                "caught": verification.caught,
                "missed": verification.missed,
                "inert_stayed_green": verification.inert_stayed_green,
                "green_after": verification.green_after,
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_suite_verification_before_cleanup",
            )],
        )])
    })
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        forget_verification(run_id);
        workspace::remove(&workspace::root(ID, run_id));
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_defect_targets_a_line_the_library_actually_has() {
        let source = library_source();
        for (_, target, replacement) in DEFECTS {
            assert!(source.contains(target), "{target}");
            assert_ne!(target, replacement);
            assert!(defective_source(target, replacement) != source);
        }
    }

    #[test]
    fn the_inert_change_leaves_every_statement_intact() {
        let mut inert = library_source();
        inert.push_str(INERT_CHANGE);
        for line in PRISTINE {
            assert!(inert.contains(line));
        }
    }
}
