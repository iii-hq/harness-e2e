//! `deepwork_scope_fallout` — a deepwork dev card renames a field inside its
//! own `Files:`, and a test fixture outside that line still builds the old
//! field. Fixing the fixture is fallout of the card: the run has to make it
//! and accept it without failing a batch on scope alone, and must not loosen
//! the test that reads the fixture. Every gate reads durable state (kanban
//! tickets, state documents) and the run branch, never the orchestrator's
//! prose.
//!
//! Fixture: `src/order.py` (`Order.amount`, `total`), `src/report.py`,
//! `src/greet.py`; `tests/fixtures.py` builds orders with `amount=` and
//! `tests/test_report.py` asserts their summary. Card A renames `amount` to
//! `cents` with `Files: src/order.py` and `Tests: tests/test_order.py`,
//! written beforehand like a team's tickets, so the planner cannot widen them;
//! card B (`src/greet.py`) is independent, so both land in one wave.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::deepwork_batch_repair::{
    find_project, git, has_label, non_empty, project_tickets, remove_board, state_get,
    ticket_summary, wall_minutes,
};
use super::{
    async_trait, common, ArtifactExpectation, Capability, CapturedDeliverable, DeliverableContract,
    ExecutionPolicy, ObjectiveEvaluation, ProvenanceEvidence, Scenario, ScenarioCase,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "deepwork_scope_fallout";
const DELIVERABLE_ID: &str = "protocol_evidence";
const AGENT: &str = "deepwork";
/// Hard cost cap of the whole session tree (see `suite::subject_cost_cap_usd`).
pub const COST_CAP_USD: f64 = 15.0;
const ROUNDS: u32 = 3;
const WAVES: u32 = 3;
const TURN_BUDGET: u64 = 60;

const FIRST_BATCH_VERIFIED: AssessmentSpec = AssessmentSpec::gate(
    "first_batch_verified",
    30,
    "Was the first batch, the one holding the rename card, verified, so no batch failed on its scope?",
)
.labeled("Rename card verified in its first batch");
const FALLOUT_MADE: AssessmentSpec = AssessmentSpec::gate(
    "fallout_made_test_kept",
    25,
    "Does the run branch build the fixture orders with `cents=` while tests/test_report.py still asserts the same summary?",
)
.labeled("Fixture updated, test unchanged");
const RUN_COMPLETED: AssessmentSpec = AssessmentSpec::gate(
    "run_completed",
    30,
    "Do both cards end done and verified, the run ticket done, and the suite pass on the run branch?",
)
.labeled("Both cards verified, suite green");
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
const ORCHESTRATOR_TURNS: AssessmentSpec = AssessmentSpec::scored(
    "orchestrator_turns",
    5,
    "Did the orchestrator session need at most 60 turns?",
)
.labeled("Orchestrator needed at most 60 turns");
const ASSESSMENTS: &[AssessmentSpec] = &[
    FIRST_BATCH_VERIFIED,
    FALLOUT_MADE,
    RUN_COMPLETED,
    COST_WITHIN_BUDGET,
    WALL_TIME,
    ORCHESTRATOR_TURNS,
];

const CARD_FILES: [&str; 2] = ["src/order.py", "src/greet.py"];
const REPORT_ASSERT: &str = r#"self.assertEqual(summary(sample_orders()), "2 orders, 400 cents")"#;

const ORDER: &str = "from dataclasses import dataclass\n\n\n@dataclass\nclass Order:\n    sku: str\n    amount: int\n\n\ndef total(orders):\n    return sum(order.amount for order in orders)\n";
const REPORT: &str = "from order import total\n\n\ndef summary(orders):\n    return f\"{len(orders)} orders, {total(orders)} cents\"\n";
const GREET: &str = "def greet():\n    raise NotImplementedError(\"greet\")\n";
const FIXTURES: &str = "from order import Order\n\n\ndef sample_orders():\n    return [Order(sku=\"a\", amount=150), Order(sku=\"b\", amount=250)]\n";
const TEST_REPORT: &str = "import os\nimport sys\nimport unittest\n\nsys.path.insert(0, os.path.join(os.path.dirname(__file__), \"..\", \"src\"))\nsys.path.insert(0, os.path.dirname(__file__))\n\nfrom fixtures import sample_orders  # noqa: E402\nfrom report import summary  # noqa: E402\n\n\nclass Report(unittest.TestCase):\n    def test_summary(self):\n        self.assertEqual(summary(sample_orders()), \"2 orders, 400 cents\")\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n";
const README: &str = "# fixture\n\nOrders, a report over them and a greeting under `src/`. Tests: `python3 -m unittest discover -s tests`.\nTests import the modules with `sys.path.insert(0, \"src\")`, as tests/test_report.py does.\n";

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
    format!("e2e-deepwork-fallout-{run_id}")
}

fn args_line(project: &str) -> String {
    format!(
        "flow=dev isolation=ticket attempts=1 max_rounds={ROUNDS} approve=auto critic=off lead_model=same concurrency=2 deadline_min=8 project={project}"
    )
}

pub struct DeepworkScopeFallout;

#[async_trait]
impl Scenario for DeepworkScopeFallout {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Deepwork keeps a card's fallout in scope")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Give the Deepwork orchestration agent two code cards in a tiny git repository. Card A renames a field inside its Files line, and a test fixture outside that line still builds the old field. The run must update the fixture as the card's fallout, keep the test that reads it unchanged, and verify both cards in their first batch.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("The Deepwork kit's batch scope: a card's fallout (another use of a name it renamed, a regenerated golden, a registry entry) is in scope. If it fails, a batch whose checks are green is rejected on scope alone and every contract change costs a repair round.")
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
                "cards": CARD_FILES,
                "args": args_line("<project>"),
                "fallout": "tests/fixtures.py",
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
                r#"Make two small changes to this Python repository (your working folder; tests: `python3 -m unittest discover -s tests`).

Plan exactly two code tickets, one per change, and write each card with these Files, Tests and Verify lines verbatim (the cards were agreed beforehand, as tickets a team already wrote):

1. src/order.py: rename the field `amount` of `Order` to `cents`, and make `total()` sum `cents`.
   Files: src/order.py
   Tests: tests/test_order.py: `total()` sums the `cents` of every order
   Verify: shell::exec {{"command":"python3","args":["-c","import sys; sys.path.insert(0, 'src'); from order import Order, total; assert total([Order(sku='x', cents=5)]) == 5"]}} -> exit code 0

2. src/greet.py: make `greet()` return the string "hello".
   Files: src/greet.py
   Tests: none
   Verify: shell::exec {{"command":"python3","args":["-c","import sys; sys.path.insert(0, 'src'); from greet import greet; assert greet() == 'hello'"]}} -> exit code 0

The two tickets touch different files and need nothing from each other, so they run side by side.

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
            ("src/order.py", ORDER),
            ("src/report.py", REPORT),
            ("src/greet.py", GREET),
            ("tests/fixtures.py", FIXTURES),
            ("tests/test_report.py", TEST_REPORT),
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
        let (first, fallout, completed) = judge(&evidence);
        let cost = observation.metrics.totals.cost_usd;
        let turns = evidence["orchestrator_turns"]
            .as_u64()
            .unwrap_or(observation.metrics.totals.turns);
        let minutes = evidence["wall_minutes"].as_f64();
        Ok(assessment::build_evaluation([
            FIRST_BATCH_VERIFIED.full_or_zero(first.0, first.1),
            FALLOUT_MADE.full_or_zero(fallout.0, fallout.1),
            RUN_COMPLETED.full_or_zero(completed.0, completed.1),
            COST_WITHIN_BUDGET.full_or_zero(
                cost.is_some_and(|cost| cost <= 10.0),
                format!("cost_usd={cost:?}"),
            ),
            WALL_TIME.full_or_zero(
                minutes.is_some_and(|minutes| minutes <= 45.0),
                format!("run_ticket_minutes={minutes:?}"),
            ),
            ORCHESTRATOR_TURNS
                .full_or_zero(turns <= TURN_BUDGET, format!("orchestrator_turns={turns}")),
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

/// The `Files:` line decides which card a ticket is, whatever the title says.
fn card_index(ticket: &Value) -> Option<usize> {
    let files = ticket["description"]
        .as_str()?
        .lines()
        .map(str::trim)
        .find(|line| {
            line.trim_start_matches(['-', '*', ' '])
                .starts_with("Files:")
        })?;
    CARD_FILES.iter().position(|file| files.contains(file))
}

/// Everything the gates read, gathered once; a failed read is recorded, never
/// fatal, so one missing document hides nothing else.
async fn collect(context: &E2eContext, observation: &ScenarioObservation, run_id: &str) -> Value {
    let own = observation.metrics.root_session_id.clone();
    let mut evidence = json!({
        "orchestrator_session": own,
        "project": project_name(run_id),
        "orchestrator_turns": observation
            .metrics
            .by_session
            .iter()
            .find(|row| row.session_id == own)
            .map(|row| row.turns),
    });
    let repo = repo_root(run_id);
    evidence["branch"] = run_branch(&repo).await;

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
        evidence["tickets"] = json!(tickets.iter().map(ticket_summary).collect::<Vec<_>>());
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
    let mut batches = Vec::new();
    for round in 1..=ROUNDS {
        for wave in 0..=WAVES {
            let scope = format!("{run}-r{round}-w{wave}-batch");
            let manifest = state_get(context, &scope, "manifest").await;
            if !non_empty(&manifest) {
                continue;
            }
            let batch = state_get(context, &scope, "batch").await;
            let mut verdicts = serde_json::Map::new();
            if let Some(check) = batch["check_from"]["scope"].as_str() {
                for key in batch["check_from"]["keys"].as_array().into_iter().flatten() {
                    if let Some(key) = key.as_str() {
                        verdicts.insert(key.to_string(), state_get(context, check, key).await);
                    }
                }
            }
            batches.push(json!({
                "round": round,
                "wave": wave,
                "scope": scope,
                "manifest": manifest,
                "batch": batch,
                "check_verdicts": verdicts,
            }));
        }
    }
    evidence["run"] = json!(run);
    evidence["batches"] = json!(batches);
    evidence
}

/// The run branch's fixture and report test, and the suite run in its worktree.
async fn run_branch(repo: &std::path::Path) -> Value {
    let Ok(branch) = git(
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
    let Some(branch) = branch.lines().find(|line| !line.trim().is_empty()) else {
        return json!({ "error": "no deepwork branch" });
    };
    let show = |path: &str| format!("{branch}:{path}");
    let fixtures = git(repo, &["show", &show("tests/fixtures.py")])
        .await
        .unwrap_or_default();
    let report_test = git(repo, &["show", &show("tests/test_report.py")])
        .await
        .unwrap_or_default();
    let changed = git(repo, &["diff", "--name-only", "main", branch])
        .await
        .unwrap_or_default();
    // The suite on the branch's own tree, in a scratch worktree of ours.
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
    json!({
        "name": branch,
        "fixtures": fixtures,
        "report_test": report_test,
        "changed": changed.lines().collect::<Vec<_>>(),
        "suite": suite,
    })
}

fn done_verified(card: &Value) -> bool {
    card["status"].as_str() == Some("done") && has_label(card, "verified")
}

fn judge(evidence: &Value) -> ((bool, String), (bool, String), (bool, String)) {
    let rename = evidence["cards"][0]["key"].as_str().map(str::to_string);
    let batches: Vec<&Value> = evidence["batches"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let first = rename.as_ref().and_then(|key| {
        batches.iter().find(|batch| {
            batch["manifest"]["tickets"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|ticket| ticket["key"].as_str() == Some(key))
        })
    });
    let outcome = first.and_then(|batch| batch["batch"]["outcome"].as_str());
    let failed: Vec<String> = batches
        .iter()
        .filter(|batch| batch["batch"]["outcome"].as_str() != Some("verified"))
        .map(|batch| {
            format!(
                "{} {} {}",
                batch["scope"], batch["batch"]["outcome"], batch["batch"]["reasons"]
            )
        })
        .collect();
    let first_batch = (
        outcome == Some("verified"),
        format!(
            "rename_card={rename:?} first_batch={:?} outcome={outcome:?} batches={} not_verified={failed:?} error={}",
            first.map(|batch| &batch["scope"]),
            batches.len(),
            evidence["error"],
        ),
    );

    let branch = &evidence["branch"];
    let fixtures = branch["fixtures"].as_str().unwrap_or_default();
    let report_test = branch["report_test"].as_str().unwrap_or_default();
    let fixture_ok = fixtures.contains("cents=150") && !fixtures.contains("amount");
    let test_kept = report_test.contains(REPORT_ASSERT);
    let fallout = (
        fixture_ok && test_kept,
        format!(
            "branch={} fixture_uses_cents={fixture_ok} report_assert_kept={test_kept} changed={}",
            branch["name"], branch["changed"]
        ),
    );

    let cards_done = (0..2).all(|index| done_verified(&evidence["cards"][index]));
    let run_done = evidence["run_ticket"]["status"].as_str() == Some("done");
    let suite = branch["suite"]["passed"] == true;
    let completed = (
        cards_done && run_done && suite,
        format!(
            "cards={:?} run_status={} suite={}",
            (0..2)
                .map(|index| (
                    evidence["cards"][index]["key"].clone(),
                    evidence["cards"][index]["status"].clone(),
                    evidence["cards"][index]["labels"].clone()
                ))
                .collect::<Vec<_>>(),
            evidence["run_ticket"]["status"],
            branch["suite"],
        ),
    );
    (first_batch, fallout, completed)
}
