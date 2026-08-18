//! Build a working security scanner from a prompt, then judge it by running
//! it: against a tree it has never seen, against a clean tree, and twice over
//! for determinism. The deliverable is a system, and using that system is the
//! verification.

use std::time::Duration;

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::deliverable::workspace;
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

use super::repo::{self, PlantedFile};

pub const ID: &str = "build.security_scanner";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "security_scanner_system";
const ENTRYPOINT: &str = "scanner/scan.py";
const RULES_FILE: &str = "scanner/rules.json";
const SAMPLE: &str = "sample";
const HOLDOUT: &str = "holdout";
const CLEAN: &str = "clean";
const RULES: [&str; 5] = [
    "hardcoded_secret",
    "shell_injection",
    "sql_string_concat",
    "permissive_cors",
    "insecure_random",
];
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

const SYSTEM_RUNS: AssessmentSpec = AssessmentSpec::hard_gated(
    "system_runs",
    15,
    "The built scanner runs from its documented entrypoint and prints a JSON report.",
);
const FINDINGS_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "findings_exact",
    40,
    "On a tree it never saw, the scanner reports exactly the planted issues: none missed, none invented.",
);
const CLEAN_TREE_SILENT: AssessmentSpec = AssessmentSpec::hard_gated(
    "clean_tree_silent",
    15,
    "A tree with only safe equivalents produces no findings and a success exit code.",
);
const DETERMINISTIC: AssessmentSpec = AssessmentSpec::hard_gated(
    "deterministic",
    15,
    "Two runs over the same tree produce the same report.",
);
const RULES_DOCUMENTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "rules_documented",
    15,
    "Every rule is declared with a severity in the rules file.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    SYSTEM_RUNS,
    FINDINGS_EXACT,
    CLEAN_TREE_SILENT,
    DETERMINISTIC,
    RULES_DOCUMENTED,
];

/// What the agent may look at while building.
const SAMPLE_FILES: &[PlantedFile] = &[
    PlantedFile {
        path: "service/settings.py",
        lines: &[
            "import os",
            "",
            "SESSION_TOKEN = \"tok_live_44ab99f2c1d7\"",
            "DATABASE_URL = os.environ[\"DATABASE_URL\"]",
        ],
        findings: &[("hardcoded_secret", 3)],
    },
    PlantedFile {
        path: "service/reports.py",
        lines: &[
            "def rows(conn, owner):",
            "    query = \"SELECT id FROM reports WHERE owner = '\" + owner + \"'\"",
            "    return conn.execute(query)",
        ],
        findings: &[("sql_string_concat", 2)],
    },
];

/// What the runner plants after the session has ended.
const HOLDOUT_FILES: &[PlantedFile] = &[
    PlantedFile {
        path: "app/config.py",
        lines: &[
            "import os",
            "",
            "API_KEY = \"sk_live_9f8a7b6c5d4e\"",
            "DATABASE_URL = os.environ[\"DATABASE_URL\"]",
        ],
        findings: &[("hardcoded_secret", 3)],
    },
    PlantedFile {
        path: "app/store.py",
        lines: &[
            "def find_user(conn, name):",
            "    query = \"SELECT * FROM users WHERE name = '\" + name + \"'\"",
            "    return conn.execute(query)",
            "",
            "def find_user_safely(conn, name):",
            "    return conn.execute(\"SELECT * FROM users WHERE name = ?\", (name,))",
        ],
        findings: &[("sql_string_concat", 2)],
    },
    PlantedFile {
        path: "app/jobs.py",
        lines: &[
            "import subprocess",
            "",
            "def tail(path):",
            "    subprocess.run(\"tail -n 20 \" + path, shell=True)",
            "",
            "def tail_safely(path):",
            "    subprocess.run([\"tail\", \"-n\", \"20\", path])",
        ],
        findings: &[("shell_injection", 4)],
    },
    PlantedFile {
        path: "app/web.py",
        lines: &[
            "def cors_headers():",
            "    return {\"Access-Control-Allow-Origin\": \"*\"}",
            "",
            "def scoped_headers():",
            "    return {\"Access-Control-Allow-Origin\": \"https://console.internal\"}",
        ],
        findings: &[("permissive_cors", 2)],
    },
    PlantedFile {
        path: "app/tokens.py",
        lines: &[
            "import random",
            "import secrets",
            "",
            "def new_password():",
            "    return \"\".join(random.choice(\"abcdef0123456789\") for _ in range(16))",
            "",
            "def new_session_id():",
            "    return secrets.token_hex(16)",
        ],
        findings: &[("insecure_random", 5)],
    },
    PlantedFile {
        path: "docs/onboarding.md",
        lines: &[
            "# Onboarding",
            "",
            "Never commit a password, API key, or session token to this repository.",
            "Use parameterised SQL such as `SELECT * FROM users WHERE name = ?`.",
        ],
        findings: &[],
    },
];

/// The same shapes with only the safe variants left.
const CLEAN_FILES: &[PlantedFile] = &[
    PlantedFile {
        path: "app/config.py",
        lines: &[
            "import os",
            "",
            "API_KEY = os.environ[\"API_KEY\"]",
            "DATABASE_URL = os.environ[\"DATABASE_URL\"]",
        ],
        findings: &[],
    },
    PlantedFile {
        path: "app/store.py",
        lines: &[
            "def find_user(conn, name):",
            "    return conn.execute(\"SELECT * FROM users WHERE name = ?\", (name,))",
        ],
        findings: &[],
    },
    PlantedFile {
        path: "app/jobs.py",
        lines: &[
            "import subprocess",
            "",
            "def tail(path):",
            "    subprocess.run([\"tail\", \"-n\", \"20\", path])",
        ],
        findings: &[],
    },
    PlantedFile {
        path: "app/web.py",
        lines: &[
            "def cors_headers():",
            "    return {\"Access-Control-Allow-Origin\": \"https://console.internal\"}",
        ],
        findings: &[],
    },
    PlantedFile {
        path: "app/tokens.py",
        lines: &[
            "import secrets",
            "",
            "def new_session_id():",
            "    return secrets.token_hex(16)",
        ],
        findings: &[],
    },
];

fn setup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let root = workspace::root(ID, run_id);
        repo::plant(&root, SAMPLE, SAMPLE_FILES)?;
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let rules = RULES.join("`, `");
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Build a working static security scanner in this workspace, then leave it ready to \
             run. Take as many turns as you need.\n\n\
             The system:\n\
             1. `{ENTRYPOINT}` is the entrypoint. `python3 {ENTRYPOINT} <directory>` scans every \
             file under that directory and prints one JSON object to stdout: \
             {{\"findings\": [{{\"rule\": \"<id>\", \"file\": \"<path>\", \"line\": <1-based \
             line>, \"severity\": \"<high|medium|low>\"}}], \"files_scanned\": <count>}}. Exit 1 \
             when there is at least one finding and 0 when there are none. Print nothing else on \
             stdout.\n\
             2. Use only the Python 3 standard library. No network access, no installed \
             packages.\n\
             3. Implement exactly these rule ids: `{rules}`.\n\
             - `hardcoded_secret`: a literal credential assigned to a name that reads like a \
             secret, key, token, or password.\n\
             - `shell_injection`: a shell command built by concatenating or interpolating a \
             value.\n\
             - `sql_string_concat`: a SQL statement built by concatenating or interpolating a \
             value.\n\
             - `permissive_cors`: an `Access-Control-Allow-Origin` value of `*`.\n\
             - `insecure_random`: the `random` module used to produce a secret, token, password, \
             or key.\n\
             4. Safe equivalents must not be reported: parameterised SQL, a command passed as an \
             argument list, an origin naming a host, `secrets` for tokens, a value read from the \
             environment, and prose in documentation that merely mentions these words.\n\
             5. Write `{RULES_FILE}`: a JSON object mapping each rule id to \
             {{\"severity\": \"<high|medium|low>\", \"description\": \"<one line>\"}}.\n\n\
             A sample tree is already in `{SAMPLE}/` for you to test against. It is a sample, not \
             the corpus: your scanner will be run against directories you have not seen, so \
             detect the patterns rather than matching these paths, names, or line numbers.\n\n\
             When the scanner works, reply with exactly one line: `SCANNER_READY rules=5`."
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
            "rules_file": RULES_FILE,
            "rules": RULES,
            "sample_tree": SAMPLE,
            "verification": {
                "holdout_findings": repo::expected_findings(HOLDOUT, HOLDOUT_FILES).len(),
                "clean_tree": CLEAN,
                "runs_compared_for_determinism": 2,
            },
        }),
        super::system_profile(3, 6),
        &["iii::shell"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["reported", "expected", "clean_findings", "response"],
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
    reported: Vec<(String, String, usize)>,
    /// Kept bounded and surfaced as evidence: when a built system crashes,
    /// this is the only thing that explains the empty report.
    stderr: String,
    expected: Vec<(String, String, usize)>,
    clean_findings: usize,
    clean_status: Option<i32>,
    holdout_status: Option<i32>,
    ran: bool,
    deterministic: bool,
    rules_documented: bool,
}

/// Plant the unseen trees, then run what the agent built against them.
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
    let expected = repo::expected_findings(HOLDOUT, HOLDOUT_FILES);
    let planted = repo::plant(&root, HOLDOUT, HOLDOUT_FILES)
        .and_then(|()| repo::plant(&root, CLEAN, CLEAN_FILES));
    if planted.is_err() {
        return Verification {
            reported: Vec::new(),
            stderr: "the runner could not plant the verification trees".to_string(),
            expected,
            clean_findings: usize::MAX,
            clean_status: None,
            holdout_status: None,
            ran: false,
            deterministic: false,
            rules_documented: false,
        };
    }

    let first = repo::run(&root, "python3", &[ENTRYPOINT, HOLDOUT], RUN_TIMEOUT).await;
    let second = repo::run(&root, "python3", &[ENTRYPOINT, HOLDOUT], RUN_TIMEOUT).await;
    let clean = repo::run(&root, "python3", &[ENTRYPOINT, CLEAN], RUN_TIMEOUT).await;

    let report = first.as_ref().and_then(repo::Execution::json);
    let ran = report.is_some();
    let reported = report
        .as_ref()
        .map(|report| repo::reported_findings(report, HOLDOUT))
        .unwrap_or_default();
    let deterministic = match (first.as_ref(), second.as_ref()) {
        (Some(first), Some(second)) => {
            first.stdout.trim() == second.stdout.trim() && first.status == second.status
        }
        _ => false,
    };
    let clean_report = clean.as_ref().and_then(repo::Execution::json);
    let clean_findings = clean_report
        .as_ref()
        .map(|report| repo::reported_findings(report, CLEAN).len())
        .unwrap_or(usize::MAX);

    let declared = workspace::read_json(&root, RULES_FILE).unwrap_or(Value::Null);
    let rules_documented = RULES.iter().all(|rule| {
        declared
            .get(rule)
            .and_then(|entry| entry.get("severity"))
            .and_then(Value::as_str)
            .is_some_and(|severity| matches!(severity, "high" | "medium" | "low"))
    });

    Verification {
        reported,
        stderr: first
            .as_ref()
            .map(|run| run.stderr.chars().take(512).collect())
            .unwrap_or_default(),
        expected,
        clean_findings,
        clean_status: clean.as_ref().and_then(|run| run.status),
        holdout_status: first.as_ref().and_then(|run| run.status),
        ran,
        deterministic,
        rules_documented,
    }
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let verification = verify(run_id).await;
        let missed: Vec<_> = verification
            .expected
            .iter()
            .filter(|finding| !verification.reported.contains(finding))
            .collect();
        let invented: Vec<_> = verification
            .reported
            .iter()
            .filter(|finding| !verification.expected.contains(finding))
            .collect();

        Ok(assessment::build_evaluation([
            SYSTEM_RUNS.full_or_zero(
                verification.ran && observation.response.contains("SCANNER_READY"),
                format!(
                    "entrypoint produced a JSON report: {}; exit status {:?}",
                    verification.ran, verification.holdout_status
                ),
            ),
            FINDINGS_EXACT.full_or_zero(
                verification.ran
                    && verification.reported == verification.expected
                    && verification.holdout_status == Some(1),
                format!(
                    "missed {missed:?}; invented {invented:?}; exit status {:?}",
                    verification.holdout_status
                ),
            ),
            CLEAN_TREE_SILENT.full_or_zero(
                verification.clean_findings == 0 && verification.clean_status == Some(0),
                format!(
                    "clean tree produced {} finding(s) with exit status {:?}",
                    verification.clean_findings, verification.clean_status
                ),
            ),
            DETERMINISTIC.full_or_zero(
                verification.deterministic,
                format!("two runs agreed: {}", verification.deterministic),
            ),
            RULES_DOCUMENTED.full_or_zero(
                verification.rules_documented,
                format!("`{RULES_FILE}` declares all five rules with a severity"),
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
        let root = workspace::root(ID, run_id);
        let verification = verify(run_id).await;
        let invariants = vec![
            crate::scenarios::CapturedInvariant {
                id: "system_runs".to_string(),
                passed: verification.ran,
                reason: format!("exit status {:?}", verification.holdout_status),
            },
            crate::scenarios::CapturedInvariant {
                id: "findings_exact".to_string(),
                passed: verification.reported == verification.expected,
                reason: format!(
                    "{} reported, {} expected",
                    verification.reported.len(),
                    verification.expected.len()
                ),
            },
        ];
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "reported": verification
                    .reported
                    .iter()
                    .map(|(rule, file, line)| json!({ "rule": rule, "file": file, "line": line }))
                    .collect::<Vec<_>>(),
                "expected": verification
                    .expected
                    .iter()
                    .map(|(rule, file, line)| json!({ "rule": rule, "file": file, "line": line }))
                    .collect::<Vec<_>>(),
                "clean_findings": verification.clean_findings,
                "deterministic": verification.deterministic,
                "stderr_excerpt": verification.stderr,
                "entrypoint_present": workspace::read(&root, ENTRYPOINT).is_some(),
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_scanner_verification_before_cleanup",
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
    fn the_holdout_tree_plants_one_issue_per_rule() {
        let expected = repo::expected_findings(HOLDOUT, HOLDOUT_FILES);
        assert_eq!(expected.len(), RULES.len());
        for rule in RULES {
            assert!(
                expected.iter().any(|(planted, _, _)| planted == rule),
                "{rule} is never planted"
            );
        }
    }

    #[test]
    fn the_clean_tree_plants_nothing_to_find() {
        assert!(repo::expected_findings(CLEAN, CLEAN_FILES).is_empty());
    }

    #[test]
    fn the_sample_shares_no_path_with_the_holdout() {
        for sample in SAMPLE_FILES {
            assert!(
                !HOLDOUT_FILES.iter().any(|held| held.path == sample.path),
                "{} appears in both trees",
                sample.path
            );
        }
    }

    #[test]
    fn every_planted_line_exists_in_its_file() {
        for file in HOLDOUT_FILES.iter().chain(SAMPLE_FILES) {
            for (_, line) in file.findings {
                assert!(*line >= 1 && *line <= file.lines.len(), "{}", file.path);
            }
        }
    }
}
