//! `deepwork_same_file_waves` — two independent deepwork dev cards both
//! append a function to the same file, with `isolation=ticket`. Coded side by
//! side, the second cherry-pick conflicts at integration and that card loses
//! its work and waits a round. The run has to keep them in separate waves of
//! one round, so each is coded once and both land without a conflict. Every
//! gate reads durable state (kanban tickets, state documents) and the run
//! branch, never the orchestrator's prose.
//!
//! Fixture: `src/textutil.py` holds `normalize()` and says new helpers go at
//! its end. Card A adds `shout()`, card B adds `whisper()`; both name only
//! `src/textutil.py` in `Files:` and need nothing from each other.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::deepwork_batch_repair::{
    find_project, git, has_label, non_empty, project_tickets, remove_board, state_get, state_list,
    ticket_summary, wall_minutes,
};
use super::{
    async_trait, common, ArtifactExpectation, Capability, CapturedDeliverable, DeliverableContract,
    ExecutionPolicy, ObjectiveEvaluation, ProvenanceEvidence, Scenario, ScenarioCase,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "deepwork_same_file_waves";
const DELIVERABLE_ID: &str = "protocol_evidence";
const AGENT: &str = "deepwork";
/// Hard cost cap of the whole session tree (see `suite::subject_cost_cap_usd`).
pub const COST_CAP_USD: f64 = 15.0;
const ROUNDS: u32 = 2;
const WAVES: u32 = 4;

const NO_CONFLICT: AssessmentSpec = AssessmentSpec::gate(
    "no_integration_conflict",
    25,
    "Did every card integrate without a cherry-pick conflict?",
)
.labeled("No integration conflict");
const ONE_ROUND: AssessmentSpec = AssessmentSpec::gate(
    "one_round",
    20,
    "Did the run finish its work in round 1, with no wave or batch in round 2?",
)
.labeled("Both cards done in one round");
const BOTH_VERIFIED: AssessmentSpec = AssessmentSpec::gate(
    "both_verified",
    30,
    "Do both cards end done and verified, the run ticket done, the run branch hold both functions and its suite pass?",
)
.labeled("Both cards verified, suite green");
const CODED_ONCE: AssessmentSpec = AssessmentSpec::scored(
    "coded_once",
    15,
    "Did each card's code phase run exactly once?",
)
.labeled("Each card coded once");
const COST_WITHIN_BUDGET: AssessmentSpec = AssessmentSpec::scored(
    "cost_within_budget",
    5,
    "Did the whole session tree cost at most 10 US dollars?",
)
.labeled("Run cost stayed within 10 dollars");
const WALL_TIME: AssessmentSpec = AssessmentSpec::scored(
    "wall_time",
    5,
    "Did the run ticket go from creation to done in at most 45 minutes?",
)
.labeled("Run finished within 45 minutes");
const ASSESSMENTS: &[AssessmentSpec] = &[
    NO_CONFLICT,
    ONE_ROUND,
    BOTH_VERIFIED,
    CODED_ONCE,
    COST_WITHIN_BUDGET,
    WALL_TIME,
];

/// The function each card adds, which also tells the cards apart.
const CARD_FUNCTIONS: [&str; 2] = ["shout", "whisper"];

const TEXTUTIL: &str = "\"\"\"Text helpers, one function each. Add every new helper at the end of this file.\"\"\"\n\n\ndef normalize(text):\n    return \" \".join(text.split())\n";
const TEST_TEXTUTIL: &str = "import os\nimport sys\nimport unittest\n\nsys.path.insert(0, os.path.join(os.path.dirname(__file__), \"..\", \"src\"))\n\nfrom textutil import normalize  # noqa: E402\n\n\nclass Normalize(unittest.TestCase):\n    def test_collapses_spaces(self):\n        self.assertEqual(normalize(\"  a   b \"), \"a b\")\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n";
const README: &str = "# fixture\n\nText helpers in `src/textutil.py`. Tests: `python3 -m unittest discover -s tests`.\nTests import the module with `sys.path.insert(0, \"src\")`, as tests/test_textutil.py does.\n";

fn workspace_root(run_id: &str) -> PathBuf {
    let base = std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let base = std::fs::canonicalize(&base).unwrap_or(base);
    base.join("scenario-workspaces")
        .join(format!("{ID}-{run_id}"))
}

fn repo_root(run_id: &str) -> PathBuf {
    workspace_root(run_id).join("repo")
}

fn project_name(run_id: &str) -> String {
    format!("e2e-deepwork-waves-{run_id}")
}

fn args_line(project: &str) -> String {
    format!(
        "flow=dev isolation=ticket attempts=1 max_rounds={ROUNDS} approve=auto critic=off lead_model=same concurrency=2 deadline_min=8 project={project}"
    )
}

pub struct DeepworkSameFileWaves;

#[async_trait]
impl Scenario for DeepworkSameFileWaves {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Deepwork keeps same-file cards out of one wave")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Give the Deepwork orchestration agent two independent code cards that both append a function to the same file, with one worktree per card. The run must keep them in separate waves of one round, so each is coded once, both integrate without a conflict and both are verified.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("The Deepwork kit's wave planning with isolation=ticket: cards that change the same implementation file never code side by side. If it fails, the second card's cherry-pick conflicts, its work is lost and it waits a whole round, or is left undone in the last one.")
    }

    fn agent_profile(&self) -> Option<&'static str> {
        Some(AGENT)
    }

    fn evaluates_failed_subjects(&self) -> bool {
        true
    }

    fn required_functions(&self, _run_id: &str) -> Vec<String> {
        [
            "kanban::project::create",
            "kanban::project::list",
            "kanban::ticket::get",
            "kanban::ticket::list",
            "state::get",
            "state::list",
        ]
        .map(String::from)
        .to_vec()
    }

    fn case(&self, seed: u64) -> Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "agent": AGENT,
                "cards": CARD_FUNCTIONS,
                "shared_file": "src/textutil.py",
                "args": args_line("<project>"),
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiFunctions,
                Capability::IiiState,
                Capability::IiiShell,
                Capability::Git,
                Capability::Python3,
            ],
            DeliverableContract {
                artifacts: vec![ArtifactExpectation {
                    id: DELIVERABLE_ID.to_string(),
                    kind: "protocol_evidence".to_string(),
                    media_type: "application/json".to_string(),
                    schema: json!({ "type": "object" }),
                    max_size_bytes: 1_048_576,
                }],
                invariants: Vec::new(),
                provenance_required: true,
                capture_before_cleanup: true,
            },
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let project = project_name(run_id);
        ScenarioSpec {
            id: ID,
            prompt: format!(
                r#"Add two text helpers to this Python repository (your working folder; tests: `python3 -m unittest discover -s tests`).

Plan exactly two code tickets, one per helper, and write each card with these Files, Tests and Verify lines verbatim (the cards were agreed beforehand, as tickets a team already wrote):

1. src/textutil.py: add `shout(text)` at the end of the file, returning `normalize(text).upper() + "!"`.
   Files: src/textutil.py
   Tests: tests/test_textutil.py: `shout(" hi  there ")` returns "HI THERE!"
   Verify: shell::exec {{"command":"python3","args":["-c","import sys; sys.path.insert(0, 'src'); from textutil import shout; assert shout(' hi  there ') == 'HI THERE!'"]}} -> exit code 0

2. src/textutil.py: add `whisper(text)` at the end of the file, returning `normalize(text).lower() + "..."`.
   Files: src/textutil.py
   Tests: tests/test_textutil.py: `whisper(" Hi  THERE ")` returns "hi there..."
   Verify: shell::exec {{"command":"python3","args":["-c","import sys; sys.path.insert(0, 'src'); from textutil import whisper; assert whisper(' Hi  THERE ') == 'hi there...'"]}} -> exit code 0

The two tickets need nothing from each other.

args: {args}"#,
                args = args_line(&project),
            ),
            filesystem_root: Some(repo_root(run_id)),
            execution: ExecutionPolicy {
                max_turns: Some(150),
                max_output_tokens: Some(32_000),
                max_total_tokens: None,
                stuck_timeout_seconds: 1_500,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> Result<()> {
        let repo = repo_root(run_id);
        for (relative, content) in [
            ("README.md", README),
            (".gitignore", "__pycache__/\n"),
            ("src/textutil.py", TEXTUTIL),
            ("tests/test_textutil.py", TEST_TEXTUTIL),
        ] {
            let path = repo.join(relative);
            std::fs::create_dir_all(path.parent().expect("fixture paths have parents"))?;
            std::fs::write(&path, content).with_context(|| format!("write {}", path.display()))?;
        }
        for args in [
            &["init", "--quiet", "--template=", "-b", "main"][..],
            &["config", "user.name", "Harness E2E"],
            &["config", "user.email", "harness-e2e@example.invalid"],
            &["config", "commit.gpgsign", "false"],
            &["add", "-A"],
            &["commit", "--quiet", "-m", "chore: initial fixture"],
        ] {
            git(&repo, args).await?;
        }
        let name = project_name(run_id);
        if find_project(context, &name).await?.is_some() {
            bail!("kanban project {name} already exists");
        }
        let _: Value = context
            .trigger_value(
                "kanban::project::create",
                json!({
                    "name": name,
                    "description": format!("Created by the {ID} E2E scenario; removed at cleanup."),
                    "color": "amber",
                }),
            )
            .await
            .context("create the run's kanban project")?;
        Ok(())
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> Result<Vec<CapturedDeliverable>> {
        let evidence = collect(context, observation, run_id).await;
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "protocol_evidence".to_string(),
            content: evidence.into(),
            invariants: Vec::new(),
            provenance: vec![
                ProvenanceEvidence {
                    kind: "session".to_string(),
                    source_id: observation.metrics.root_session_id.clone(),
                    relation: "orchestrator_session".to_string(),
                },
                ProvenanceEvidence {
                    kind: "kanban_project".to_string(),
                    source_id: project_name(run_id),
                    relation: "run_board".to_string(),
                },
            ],
        }])
    }

    async fn evaluate(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> Result<ObjectiveEvaluation> {
        let evidence = match observation
            .deliverables
            .iter()
            .find(|deliverable| deliverable.id == DELIVERABLE_ID)
            .and_then(|deliverable| deliverable.content.as_json().cloned())
        {
            Some(evidence) => evidence,
            None => collect(context, observation, run_id).await,
        };
        let verdicts = judge(&evidence);
        let cost = observation.metrics.totals.cost_usd;
        let minutes = evidence["wall_minutes"].as_f64();
        Ok(assessment::build_evaluation([
            NO_CONFLICT.full_or_zero(verdicts.no_conflict.0, verdicts.no_conflict.1),
            ONE_ROUND.full_or_zero(verdicts.one_round.0, verdicts.one_round.1),
            BOTH_VERIFIED.full_or_zero(verdicts.verified.0, verdicts.verified.1),
            CODED_ONCE.full_or_zero(verdicts.coded_once.0, verdicts.coded_once.1),
            COST_WITHIN_BUDGET.full_or_zero(
                cost.is_some_and(|cost| cost <= 10.0),
                format!("cost_usd={cost:?}"),
            ),
            WALL_TIME.full_or_zero(
                minutes.is_some_and(|minutes| minutes <= 45.0),
                format!("run_ticket_minutes={minutes:?}"),
            ),
        ]))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> Result<()> {
        let mut failures = Vec::new();
        let root = workspace_root(run_id);
        common::kill_processes_under(&root).await;
        if let Err(error) = remove_board(context, &project_name(run_id)).await {
            failures.push(format!("board: {error:#}"));
        }
        if let Err(error) = common::remove_directory(&root) {
            failures.push(format!("workspace: {error:#}"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            bail!("{ID} cleanup: {}", failures.join("; "))
        }
    }
}

/// Which card a ticket is, from the helper its card names.
fn card_index(ticket: &Value) -> Option<usize> {
    let description = ticket["description"].as_str()?;
    CARD_FUNCTIONS
        .iter()
        .position(|name| description.contains(&format!("`{name}(")))
}

/// Everything the gates read, gathered once; a failed read is recorded, never
/// fatal, so one missing document hides nothing else.
async fn collect(context: &E2eContext, observation: &ScenarioObservation, run_id: &str) -> Value {
    let own = observation.metrics.root_session_id.clone();
    let mut evidence = json!({ "orchestrator_session": own, "project": project_name(run_id) });
    evidence["branch"] = run_branch(&repo_root(run_id)).await;

    let project_id = match find_project(context, &project_name(run_id)).await {
        Ok(Some(id)) => id,
        other => {
            evidence["error"] = json!(format!("run project not found: {other:?}"));
            return evidence;
        }
    };
    let tickets = match project_tickets(context, &project_id).await {
        Ok(tickets) => tickets,
        Err(error) => {
            evidence["error"] = json!(format!("read tickets: {error:#}"));
            return evidence;
        }
    };
    let Some(run_ticket) = tickets.iter().find(|ticket| has_label(ticket, "dw:run")) else {
        evidence["error"] = json!("no ticket labelled dw:run in the run project");
        return evidence;
    };
    let run_key = run_ticket["key"].as_str().unwrap_or_default().to_string();
    let mut cards: [Option<&Value>; 2] = [None, None];
    for ticket in &tickets {
        if ticket["key"] != run_ticket["key"] {
            if let Some(index) = card_index(ticket) {
                cards[index].get_or_insert(ticket);
            }
        }
    }
    evidence["run_key"] = json!(run_key);
    evidence["run_ticket"] = ticket_summary(run_ticket);
    evidence["wall_minutes"] = wall_minutes(run_ticket);
    evidence["cards"] = json!(cards
        .iter()
        .map(|card| card.map(ticket_summary).unwrap_or(Value::Null))
        .collect::<Vec<_>>());

    let tail: String = own
        .chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let run = format!("dw-{}-{tail}", run_key.to_lowercase());
    let mut waves = Vec::new();
    for round in 1..=ROUNDS {
        for wave in 0..=WAVES {
            let scope = format!("{run}-r{round}-w{wave}");
            let results = state_list(context, &scope).await;
            let batch = state_get(context, &format!("{scope}-batch"), "batch").await;
            if non_empty(&results) || non_empty(&batch) {
                waves.push(json!({
                    "round": round,
                    "wave": wave,
                    "scope": scope,
                    "results": results,
                    "batch_outcome": batch["outcome"],
                    "batch_cards": batch["cards"],
                }));
            }
        }
    }
    let mut code_phases = Vec::new();
    for card in cards.iter().flatten() {
        let key = card["key"].as_str().unwrap_or_default().to_lowercase();
        for round in 1..=ROUNDS {
            for attempt in 1..=2 {
                let scope = format!("{run}-r{round}-{key}-code{attempt}");
                if non_empty(&state_list(context, &scope).await) {
                    code_phases.push(json!({ "key": card["key"], "scope": scope }));
                }
            }
        }
    }
    evidence["run"] = json!(run);
    evidence["waves"] = json!(waves);
    evidence["code_phases"] = json!(code_phases);
    evidence
}

/// The run branch's helper module and the suite run on its tree.
async fn run_branch(repo: &std::path::Path) -> Value {
    let Ok(listed) = git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/deepwork/",
        ],
    )
    .await
    else {
        return json!({ "error": "no deepwork branch" });
    };
    let Some(branch) = listed.lines().find(|line| !line.trim().is_empty()) else {
        return json!({ "error": "no deepwork branch" });
    };
    let module = git(repo, &["show", &format!("{branch}:src/textutil.py")])
        .await
        .unwrap_or_default();
    let scratch = repo
        .parent()
        .map(|root| root.join("suite-check"))
        .unwrap_or_else(|| repo.join("suite-check"));
    let suite = match git(
        repo,
        &[
            "worktree",
            "add",
            "--detach",
            &scratch.to_string_lossy(),
            branch,
        ],
    )
    .await
    {
        Ok(_) => {
            let output = tokio::process::Command::new("python3")
                .args(["-m", "unittest", "discover", "-s", "tests"])
                .current_dir(&scratch)
                .output()
                .await;
            let _ = git(
                repo,
                &["worktree", "remove", "--force", &scratch.to_string_lossy()],
            )
            .await;
            match output {
                Ok(output) => json!({
                    "passed": output.status.success(),
                    "tail": String::from_utf8_lossy(&output.stderr).lines().rev().take(6).collect::<Vec<_>>(),
                }),
                Err(error) => json!({ "error": error.to_string() }),
            }
        }
        Err(error) => json!({ "error": format!("{error:#}") }),
    };
    json!({ "name": branch, "module": module, "suite": suite })
}

struct Verdicts {
    no_conflict: (bool, String),
    one_round: (bool, String),
    verified: (bool, String),
    coded_once: (bool, String),
}

fn judge(evidence: &Value) -> Verdicts {
    let waves: Vec<&Value> = evidence["waves"].as_array().into_iter().flatten().collect();
    // A conflict shows in a lead result (`conflict: <files> with the run
    // branch`), a card comment, or the run ticket's round comment.
    let mut conflicts: Vec<String> = Vec::new();
    for wave in &waves {
        for result in wave["results"].as_array().into_iter().flatten() {
            let text = result.to_string();
            if text.contains("conflict") {
                conflicts.push(format!(
                    "{}: {}",
                    wave["scope"],
                    &text[..text.len().min(200)]
                ));
            }
        }
    }
    let comments = |summary: &Value| -> Vec<String> {
        summary["comments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|comment| comment["body"].as_str().map(str::to_string))
            .collect()
    };
    let mut all_comments = comments(&evidence["run_ticket"]);
    for index in 0..2 {
        all_comments.extend(comments(&evidence["cards"][index]));
    }
    for body in &all_comments {
        if body.contains("conflict") {
            conflicts.push(body.chars().take(200).collect());
        }
    }
    let no_conflict = (
        conflicts.is_empty() && evidence["error"].is_null(),
        format!("conflicts={conflicts:?} error={}", evidence["error"]),
    );

    let rounds: Vec<u64> = waves
        .iter()
        .filter_map(|wave| wave["round"].as_u64())
        .collect();
    let second_round = rounds.iter().any(|round| *round > 1)
        || all_comments
            .iter()
            .any(|body| body.trim_start().starts_with("r2:"));
    let one_round = (
        !rounds.is_empty() && !second_round,
        format!(
            "waves={:?}",
            waves
                .iter()
                .map(|wave| (wave["scope"].clone(), wave["batch_outcome"].clone()))
                .collect::<Vec<_>>()
        ),
    );

    let done_verified = |index: usize| {
        let card = &evidence["cards"][index];
        card["status"].as_str() == Some("done") && has_label(card, "verified")
    };
    let module = evidence["branch"]["module"].as_str().unwrap_or_default();
    let both_in_module = module.contains("def shout") && module.contains("def whisper");
    let suite = evidence["branch"]["suite"]["passed"] == true;
    let run_done = evidence["run_ticket"]["status"].as_str() == Some("done");
    let verified = (
        done_verified(0) && done_verified(1) && run_done && both_in_module && suite,
        format!(
            "cards={:?} run_status={} both_functions={both_in_module} suite={}",
            (0..2)
                .map(|index| (
                    evidence["cards"][index]["key"].clone(),
                    evidence["cards"][index]["status"].clone(),
                    evidence["cards"][index]["labels"].clone()
                ))
                .collect::<Vec<_>>(),
            evidence["run_ticket"]["status"],
            evidence["branch"]["suite"],
        ),
    );

    let phases: Vec<&Value> = evidence["code_phases"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let per_card: Vec<usize> = (0..2)
        .map(|index| {
            let key = &evidence["cards"][index]["key"];
            phases.iter().filter(|phase| &phase["key"] == key).count()
        })
        .collect();
    let coded_once = (
        per_card == [1, 1],
        format!(
            "code_phases_per_card={per_card:?} scopes={:?}",
            phases
                .iter()
                .map(|phase| &phase["scope"])
                .collect::<Vec<_>>()
        ),
    );
    Verdicts {
        no_conflict,
        one_round,
        verified,
        coded_once,
    }
}
