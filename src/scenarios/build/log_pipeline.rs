//! Build a log aggregation pipeline, then judge it by feeding it logs it has
//! never seen and comparing its aggregates against the runner's own count.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::deliverable::workspace;
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CapturedInvariant, CleanupFuture, DeliverableCaptureFuture, EvaluationFuture,
    MaterializedScenario, ScenarioObservation, ScenarioSpec,
};

use super::repo;

pub const ID: &str = "build.log_pipeline";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "log_pipeline_system";
const ENTRYPOINT: &str = "pipeline/aggregate.py";
const SAMPLE: &str = "sample-logs";
const HOLDOUT: &str = "holdout-logs";
const BULK_LINES: usize = 20_000;
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

const SYSTEM_RUNS: AssessmentSpec = AssessmentSpec::hard_gated(
    "system_runs",
    15,
    "The pipeline runs from its documented entrypoint and prints a JSON summary.",
);
const AGGREGATES_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "aggregates_exact",
    40,
    "Level counts, per-service counts, and the busiest service match the runner's own count of logs the pipeline never saw.",
);
const MALFORMED_QUARANTINED: AssessmentSpec = AssessmentSpec::hard_gated(
    "malformed_quarantined",
    20,
    "Unparseable lines are counted and skipped rather than crashing the run or inflating a level.",
);
const HANDLES_VOLUME: AssessmentSpec = AssessmentSpec::hard_gated(
    "handles_volume",
    25,
    "A twenty-thousand-line file is aggregated correctly inside the time budget.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    SYSTEM_RUNS,
    AGGREGATES_EXACT,
    MALFORMED_QUARANTINED,
    HANDLES_VOLUME,
];

const LEVELS: [&str; 4] = ["DEBUG", "INFO", "WARN", "ERROR"];
const SERVICES: [&str; 3] = ["checkout", "billing", "search"];

/// One log line in the format the prompt declares.
fn line(index: usize, level: &str, service: &str) -> String {
    format!(
        "2026-08-18T09:{:02}:{:02}Z {level} {service} request {index} completed",
        index % 60,
        (index * 7) % 60
    )
}

struct Corpus {
    files: Vec<(String, String)>,
    levels: BTreeMap<String, usize>,
    services: BTreeMap<String, usize>,
    malformed: usize,
}

/// Deterministic logs plus the aggregates they imply, counted here so the
/// expectation never depends on the system under test.
fn corpus(seed: usize, lines: usize, malformed_every: usize) -> Corpus {
    let mut body = String::new();
    let mut levels: BTreeMap<String, usize> = BTreeMap::new();
    let mut services: BTreeMap<String, usize> = BTreeMap::new();
    let mut malformed = 0;
    for index in 0..lines {
        let position = index + seed;
        if malformed_every > 0 && position.is_multiple_of(malformed_every) {
            body.push_str("-- truncated record --\n");
            malformed += 1;
            continue;
        }
        let level = LEVELS[position % LEVELS.len()];
        let service = SERVICES[position % SERVICES.len()];
        body.push_str(&line(index, level, service));
        body.push('\n');
        *levels.entry(level.to_string()).or_default() += 1;
        *services.entry(service.to_string()).or_default() += 1;
    }
    Corpus {
        files: vec![("app.log".to_string(), body)],
        levels,
        services,
        malformed,
    }
}

fn write_corpus(root: &std::path::Path, directory: &str, corpus: &Corpus) -> anyhow::Result<()> {
    for (name, body) in &corpus.files {
        workspace::write(root, &format!("{directory}/{name}"), body)?;
    }
    Ok(())
}

fn busiest(services: &BTreeMap<String, usize>) -> Option<String> {
    services
        .iter()
        .max_by_key(|(name, count)| (**count, std::cmp::Reverse((*name).clone())))
        .map(|(name, _)| name.clone())
}

fn setup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let root = workspace::root(ID, run_id);
        write_corpus(&root, SAMPLE, &corpus(0, 40, 11))
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Build a log aggregation pipeline in this workspace, then leave it ready to run. \
             Take as many turns as you need.\n\n\
             The system:\n\
             1. `{ENTRYPOINT}` is the entrypoint. `python3 {ENTRYPOINT} <directory>` reads every \
             `.log` file under that directory and prints one JSON object to stdout: \
             {{\"levels\": {{\"<LEVEL>\": <count>}}, \"services\": {{\"<service>\": <count>}}, \
             \"busiest_service\": \"<service>\", \"malformed_lines\": <count>, \"lines_read\": \
             <count>}}. Print nothing else on stdout, and exit 0.\n\
             2. A well-formed line is `<timestamp> <LEVEL> <service> <message>`, where LEVEL is \
             one of DEBUG, INFO, WARN, ERROR and service is a single word. Count each \
             well-formed line under its level and its service.\n\
             3. A line that does not match that shape is malformed: count it under \
             `malformed_lines`, do not count it under any level or service, and never let it end \
             the run. `lines_read` counts every line, well-formed or not.\n\
             4. `busiest_service` is the service with the most lines.\n\
             5. Use only the Python 3 standard library. The pipeline must handle a file of tens \
             of thousands of lines without loading it all into memory at once.\n\n\
             A sample directory is in `{SAMPLE}/`. It is a sample, not the corpus: your pipeline \
             will be run against logs you have not seen, with different volumes and different \
             malformed lines.\n\n\
             When the pipeline works, reply with exactly one line: `PIPELINE_READY`."
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
            "entrypoint": ENTRYPOINT,
            "sample_directory": SAMPLE,
            "verification": {
                "holdout_directory": HOLDOUT,
                "bulk_lines": BULK_LINES,
                "levels": LEVELS,
                "services": SERVICES,
            },
        }),
        super::system_profile(2, 5),
        &["iii::shell"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["reported", "expected", "response"],
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
    reported: Value,
    expected: Value,
    ran: bool,
    aggregates_exact: bool,
    malformed_exact: bool,
    volume_exact: bool,
    stderr: String,
}

fn summary(corpus: &Corpus) -> Value {
    json!({
        "levels": corpus.levels,
        "services": corpus.services,
        "busiest_service": busiest(&corpus.services),
        "malformed_lines": corpus.malformed,
    })
}

fn observed_summary(report: &Value) -> Value {
    json!({
        "levels": report.get("levels").cloned().unwrap_or(Value::Null),
        "services": report.get("services").cloned().unwrap_or(Value::Null),
        "busiest_service": report.get("busiest_service").cloned().unwrap_or(Value::Null),
        "malformed_lines": report.get("malformed_lines").cloned().unwrap_or(Value::Null),
    })
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
    let held = corpus(3, 500, 17);
    let bulk = corpus(5, BULK_LINES, 0);
    let expected = summary(&held);

    if write_corpus(&root, HOLDOUT, &held).is_err() {
        return Verification {
            reported: Value::Null,
            expected,
            ran: false,
            aggregates_exact: false,
            malformed_exact: false,
            volume_exact: false,
            stderr: "the runner could not plant the held-out logs".to_string(),
        };
    }

    let run = repo::run(&root, "python3", &[ENTRYPOINT, HOLDOUT], RUN_TIMEOUT).await;
    let report = run.as_ref().and_then(repo::Execution::json);
    let reported = report.as_ref().map(observed_summary).unwrap_or(Value::Null);
    let malformed_exact = report
        .as_ref()
        .and_then(|report| report.get("malformed_lines").and_then(Value::as_u64))
        == Some(held.malformed as u64)
        && report
            .as_ref()
            .and_then(|report| report.get("lines_read").and_then(Value::as_u64))
            == Some((held.levels.values().sum::<usize>() + held.malformed) as u64);

    let volume_exact = match write_corpus(&root, "bulk-logs", &bulk) {
        Ok(()) => {
            let bulk_run =
                repo::run(&root, "python3", &[ENTRYPOINT, "bulk-logs"], RUN_TIMEOUT).await;
            bulk_run
                .as_ref()
                .and_then(repo::Execution::json)
                .map(|report| observed_summary(&report) == summary(&bulk))
                .unwrap_or(false)
        }
        Err(_) => false,
    };

    Verification {
        aggregates_exact: reported == expected,
        malformed_exact,
        volume_exact,
        ran: report.is_some(),
        reported,
        expected,
        stderr: run
            .as_ref()
            .map(|run| run.stderr.chars().take(512).collect())
            .unwrap_or_default(),
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
            SYSTEM_RUNS.full_or_zero(
                verification.ran && observation.response.contains("PIPELINE_READY"),
                format!("entrypoint produced a JSON summary: {}", verification.ran),
            ),
            AGGREGATES_EXACT.full_or_zero(
                verification.aggregates_exact,
                format!(
                    "expected {}, observed {}",
                    verification.expected, verification.reported
                ),
            ),
            MALFORMED_QUARANTINED.full_or_zero(
                verification.malformed_exact,
                format!(
                    "malformed and total line counts matched: {}",
                    verification.malformed_exact
                ),
            ),
            HANDLES_VOLUME.full_or_zero(
                verification.volume_exact,
                format!(
                    "{BULK_LINES} lines aggregated correctly in time: {}",
                    verification.volume_exact
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
                id: "system_runs".to_string(),
                passed: verification.ran,
                reason: verification.stderr.clone(),
            },
            CapturedInvariant {
                id: "aggregates_exact".to_string(),
                passed: verification.aggregates_exact,
                reason: "held-out aggregates compared against the runner's own count".to_string(),
            },
        ];
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "reported": verification.reported,
                "expected": verification.expected,
                "handled_volume": verification.volume_exact,
                "stderr_excerpt": verification.stderr,
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_pipeline_verification_before_cleanup",
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
    fn the_runner_counts_the_corpus_it_writes() {
        let corpus = corpus(3, 500, 17);
        let counted: usize = corpus.levels.values().sum();
        assert_eq!(counted + corpus.malformed, 500);
        assert_eq!(corpus.services.values().sum::<usize>(), counted);
        assert!(corpus.malformed > 0);
    }

    #[test]
    fn the_sample_and_the_holdout_differ() {
        assert_ne!(corpus(0, 40, 11).files[0].1, corpus(3, 500, 17).files[0].1);
    }

    #[test]
    fn the_busiest_service_is_the_one_with_the_most_lines() {
        let mut services = BTreeMap::new();
        services.insert("billing".to_string(), 4);
        services.insert("checkout".to_string(), 9);
        assert_eq!(busiest(&services), Some("checkout".to_string()));
    }
}
