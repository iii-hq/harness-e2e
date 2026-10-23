//! `deepwork_batch_repair` — the deepwork orchestration kit (0.5.0 contract)
//! runs a three-card dev goal in a tiny git repository whose first batch is
//! rejected on purpose. The protocol must repair that batch, not re-implement
//! it, and still finish the run. Every gate reads durable state (kanban
//! tickets and their activity log, state documents, session task messages),
//! never the orchestrator's prose.
//!
//! Fixture: `alpha.py`/`bravo.py` are independent cards that land in one wave
//! (concurrency 2); `charlie.py` depends on bravo. Card alpha's `Verify:` is a
//! script that fails its first run and passes afterwards; its run counter sits
//! in the run root, outside the repository, so no batch can commit it.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::process::Command;

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::{
    async_trait, common, ArtifactExpectation, Capability, CapturedDeliverable, DeliverableContract,
    ExecutionPolicy, ObjectiveEvaluation, ProvenanceEvidence, Scenario, ScenarioCase,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "deepwork_batch_repair";
const DELIVERABLE_ID: &str = "protocol_evidence";
const AGENT: &str = "deepwork";
/// Hard cost cap of the whole session tree (see `suite::subject_cost_cap_usd`).
pub const COST_CAP_USD: f64 = 15.0;
const WALL_BUDGET_MINUTES: i64 = 90;
const TURN_BUDGET: u64 = 60;
const ROUNDS: u32 = 3;
const WAVES: u32 = 4;

const FIRST_BATCH_REJECTED: AssessmentSpec = AssessmentSpec::gate(
    "first_batch_rejected_run_continues",
    20,
    "Was the first batch (cards A and B) rejected because of card A's first verify, and did the run keep going: the run ticket reached done only after card C did?",
)
.labeled("Rejected first batch did not end the run");
const VERIFIED_CARD_CREDITED: AssessmentSpec = AssessmentSpec::gate(
    "verified_card_credited",
    15,
    "Did the rejected first batch still mark card B verified in its cards map, and did B reach done/verified before card A?",
)
.labeled("Passing card B credited despite the failed batch");
const REPAIR_NOT_REIMPLEMENTATION: AssessmentSpec = AssessmentSpec::gate(
    "repair_not_reimplementation",
    25,
    "Is there a round-2 wave-0 repair batch manifest holding card A only, with Verified naming B at a real commit and a Retry line in its lead task, while card A's code phase ran in round 1 only?",
)
.labeled("Batch repaired, card A not re-implemented");
const RUN_COMPLETED: AssessmentSpec = AssessmentSpec::gate(
    "run_completed",
    25,
    "Do cards A, B and C end done and verified, the run ticket done, the run pointer owned by the orchestrator session and its batch scope null?",
)
.labeled("All cards verified, run closed cleanly");
const COST_WITHIN_BUDGET: AssessmentSpec = AssessmentSpec::scored(
    "cost_within_budget",
    5,
    "Did the whole session tree cost at most 10 US dollars?",
)
.labeled("Run cost stayed within 10 dollars");
const WALL_TIME: AssessmentSpec = AssessmentSpec::scored(
    "wall_time",
    5,
    "Did the run ticket go from creation to done in at most 60 minutes?",
)
.labeled("Run finished within an hour");
const ORCHESTRATOR_TURNS: AssessmentSpec = AssessmentSpec::scored(
    "orchestrator_turns",
    5,
    "Did the orchestrator session need at most 60 turns?",
)
.labeled("Orchestrator needed at most 60 turns");
const ASSESSMENTS: &[AssessmentSpec] = &[
    FIRST_BATCH_REJECTED,
    VERIFIED_CARD_CREDITED,
    REPAIR_NOT_REIMPLEMENTATION,
    RUN_COMPLETED,
    COST_WITHIN_BUDGET,
    WALL_TIME,
    ORCHESTRATOR_TURNS,
];

/// The three cards: module file, function, expected value, test-visible name.
const CARD_FILES: [&str; 3] = ["src/alpha.py", "src/bravo.py", "src/charlie.py"];

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
    format!("e2e-deepwork-{run_id}")
}

fn verify_script(run_id: &str) -> PathBuf {
    workspace_root(run_id).join("verify_alpha.sh")
}

fn counter_file(run_id: &str) -> PathBuf {
    workspace_root(run_id).join("verify_alpha.runs")
}

pub struct DeepworkBatchRepair;

#[async_trait]
impl Scenario for DeepworkBatchRepair {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Deepwork repairs a rejected batch")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Give the Deepwork orchestration agent a three-card goal in a tiny git repository. Cards A and B share a wave and card A's first verify fails, card C depends on B. The run must keep going, credit B, repair the batch as wave 0 of round 2 without re-implementing A, and finish with every card verified.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("The Deepwork kit's batch protocol: per-card verdicts, repair-first rounds and the run pointer. If it fails, a single failed check either ends a run early, re-does finished work or leaves cards stuck in progress.")
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
                "alpha_first_verify": "fails",
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
        let script = verify_script(run_id);
        let project = project_name(run_id);
        ScenarioSpec {
            id: ID,
            prompt: format!(
                r#"Make three small changes to this Python repository (your working folder; tests: `python3 -m unittest discover -s tests`).

Plan exactly three code tickets, one per change, and write each card with these Files and Verify lines verbatim:

1. src/alpha.py: make `alpha()` return the string "alpha".
   Files: src/alpha.py
   Verify: shell::exec {{"command":"sh","args":["{script}"]}} -> exit code 0

2. src/bravo.py: make `bravo()` return the string "bravo".
   Files: src/bravo.py
   Verify: shell::exec {{"command":"python3","args":["-c","import sys; sys.path.insert(0, 'src'); from bravo import bravo; assert bravo() == 'bravo'"]}} -> exit code 0

3. src/charlie.py: make `charlie()` return `bravo() + "+charlie"` (it imports `bravo` from bravo.py, so it needs ticket 2 done first: its card opens with `Depends on:` and the key of ticket 2).
   Files: src/charlie.py
   Verify: shell::exec {{"command":"python3","args":["-c","import sys; sys.path.insert(0, 'src'); from charlie import charlie; assert charlie() == 'bravo+charlie'"]}} -> exit code 0

Tickets 1 and 2 touch different files and need nothing from each other, so they run side by side.

args: {args}"#,
                script = script.display(),
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
        prepare_workspace(
            &workspace_root(run_id),
            &verify_script(run_id),
            &counter_file(run_id),
        )
        .await?;
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
        let evidence = match captured_evidence(observation) {
            Some(evidence) => evidence,
            None => collect(context, observation, run_id).await,
        };
        let verdicts = judge(&evidence);
        let totals = &observation.metrics.totals;
        let cost = totals.cost_usd;
        let turns = evidence["orchestrator_turns"]
            .as_u64()
            .unwrap_or(totals.turns);
        let minutes = evidence["wall_minutes"].as_f64();
        Ok(assessment::build_evaluation([
            FIRST_BATCH_REJECTED.full_or_zero(verdicts.first_batch.0, verdicts.first_batch.1),
            VERIFIED_CARD_CREDITED.full_or_zero(verdicts.credited.0, verdicts.credited.1),
            REPAIR_NOT_REIMPLEMENTATION.full_or_zero(verdicts.repair.0, verdicts.repair.1),
            RUN_COMPLETED.full_or_zero(verdicts.completed.0, verdicts.completed.1),
            COST_WITHIN_BUDGET.full_or_zero(
                cost.is_some_and(|cost| cost <= 10.0),
                format!("cost_usd={cost:?}"),
            ),
            WALL_TIME.full_or_zero(
                minutes.is_some_and(|minutes| minutes <= 60.0),
                format!("run_ticket_minutes={minutes:?} (budget {WALL_BUDGET_MINUTES})"),
            ),
            ORCHESTRATOR_TURNS
                .full_or_zero(turns <= TURN_BUDGET, format!("orchestrator_turns={turns}")),
        ]))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> Result<()> {
        let mut failures = Vec::new();
        let root = workspace_root(run_id);
        common::kill_processes_under(&root).await;
        if let Err(error) = remove_board(context, run_id).await {
            failures.push(format!("board: {error:#}"));
        }
        // The repository, its worktrees and every deepwork branch live in the
        // run root, so removing the directory removes all three.
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

fn args_line(project: &str) -> String {
    format!(
        "flow=dev isolation=ticket attempts=1 max_rounds={ROUNDS} approve=auto critic=off lead_model=same concurrency=2 deadline_min=8 project={project}"
    )
}

const ALPHA: &str = "def alpha():\n    raise NotImplementedError(\"alpha\")\n";
const BRAVO: &str = "def bravo():\n    raise NotImplementedError(\"bravo\")\n";
const CHARLIE: &str =
    "from bravo import bravo\n\n\ndef charlie():\n    raise NotImplementedError(\"charlie\")\n";
const SMOKE: &str = "import os\nimport sys\nimport unittest\n\nsys.path.insert(0, os.path.join(os.path.dirname(__file__), \"..\", \"src\"))\n\n\nclass Smoke(unittest.TestCase):\n    def test_truth(self):\n        self.assertTrue(True)\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n";
const README: &str = "# fixture\n\nThree stub modules under `src/`. Tests: `python3 -m unittest discover -s tests`.\nTests import the modules with `sys.path.insert(0, \"src\")`, as tests/test_smoke.py does.\n";

async fn prepare_workspace(root: &Path, script: &Path, counter: &Path) -> Result<()> {
    let repo = root.join("repo");
    for (relative, content) in [
        ("README.md", README),
        (".gitignore", "__pycache__/\n"),
        ("src/alpha.py", ALPHA),
        ("src/bravo.py", BRAVO),
        ("src/charlie.py", CHARLIE),
        ("tests/test_smoke.py", SMOKE),
    ] {
        let path = repo.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture paths have parents"))?;
        std::fs::write(&path, content).with_context(|| format!("write {}", path.display()))?;
    }
    // Outside the repository: no commit, worktree or `git add -A` can carry it.
    std::fs::write(
        script,
        format!(
            "#!/bin/sh\ncounter=\"{counter}\"\nn=$(cat \"$counter\" 2>/dev/null || echo 0)\nn=$((n + 1))\necho \"$n\" > \"$counter\"\nif [ \"$n\" -eq 1 ]; then\n  echo \"verify_alpha: FAIL (run $n)\" >&2\n  exit 1\nfi\nexec python3 -c \"import sys; sys.path.insert(0, 'src'); from alpha import alpha; assert alpha() == 'alpha', alpha()\"\n",
            counter = counter.display()
        ),
    )?;
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
    Ok(())
}

async fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("start git {}", args.first().copied().unwrap_or("")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// ---------------------------------------------------------------- board

async fn find_project(context: &E2eContext, name: &str) -> Result<Option<String>> {
    let listed = context
        .trigger_value("kanban::project::list", json!({ "include_archived": true }))
        .await?;
    Ok(listed["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|project| project["name"].as_str() == Some(name))
        .and_then(|project| project["id"].as_str().map(str::to_string)))
}

/// Every live ticket of the run's project, read in full.
async fn project_tickets(context: &E2eContext, project_id: &str) -> Result<Vec<Value>> {
    let listed = context
        .trigger_value(
            "kanban::ticket::list",
            json!({ "project_id": project_id, "limit": 200 }),
        )
        .await?;
    let mut tickets = Vec::new();
    for row in listed["tickets"].as_array().into_iter().flatten() {
        let Some(key) = row["key"].as_str() else {
            continue;
        };
        tickets.push(
            context
                .trigger_value("kanban::ticket::get", json!({ "id": key }))
                .await?,
        );
    }
    Ok(tickets)
}

/// Deletes only what this run created: the tickets of its own project, then
/// archives the project (kanban has no project delete).
async fn remove_board(context: &E2eContext, run_id: &str) -> Result<()> {
    let name = project_name(run_id);
    let Some(project_id) = find_project(context, &name).await? else {
        return Ok(());
    };
    let listed = context
        .trigger_value(
            "kanban::ticket::list",
            json!({ "project_id": project_id, "limit": 200, "compact": true }),
        )
        .await?;
    let mut failures = Vec::new();
    for row in listed["tickets"].as_array().into_iter().flatten() {
        if let Some(key) = row["key"].as_str() {
            if let Err(error) = context
                .trigger_value(
                    "kanban::ticket::delete",
                    json!({ "id": key, "actor": "harness-e2e" }),
                )
                .await
            {
                failures.push(format!("{key}: {error:#}"));
            }
        }
    }
    if let Err(error) = context
        .trigger_value("kanban::project::archive", json!({ "id": project_id }))
        .await
    {
        failures.push(format!("archive {name}: {error:#}"));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        bail!("{}", failures.join("; "))
    }
}

fn activity_status_moves(ticket: &Value) -> Vec<(String, String)> {
    ticket["activity"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| {
            let at = entry["at"].as_str().unwrap_or_default().to_string();
            entry["changes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|change| change["field"].as_str() == Some("status"))
                .map(move |change| {
                    (
                        at.clone(),
                        change["to"].as_str().unwrap_or_default().to_string(),
                    )
                })
        })
        .collect()
}

/// The `Files:` line decides which card a ticket is, whatever the title says.
fn card_index(ticket: &Value) -> Option<usize> {
    let description = ticket["description"].as_str()?;
    let files = description.lines().map(str::trim).find(|line| {
        line.trim_start_matches(['-', '*', ' '])
            .starts_with("Files:")
    })?;
    CARD_FILES.iter().position(|file| files.contains(file))
}

fn has_label(ticket: &Value, label: &str) -> bool {
    ticket["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|candidate| candidate.as_str() == Some(label))
}

// ---------------------------------------------------------------- state

async fn state_get(context: &E2eContext, scope: &str, key: &str) -> Value {
    match context
        .trigger_value("state::get", json!({ "scope": scope, "key": key }))
        .await
    {
        Ok(value) => common::state_value(value),
        Err(error) => json!({ "read_error": format!("{error:#}") }),
    }
}

async fn state_list(context: &E2eContext, scope: &str) -> Value {
    match context
        .trigger_value("state::list", json!({ "scope": scope }))
        .await
    {
        Ok(Value::Array(values)) => Value::Array(values),
        Ok(other) => other,
        Err(error) => json!({ "read_error": format!("{error:#}") }),
    }
}

fn non_empty(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty() && !fields.contains_key("read_error"),
        _ => true,
    }
}

// ------------------------------------------------------------- sessions

fn message_text(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The user-role messages of a session, i.e. the tasks its parent gave it.
async fn session_tasks(context: &E2eContext, session_id: &str) -> Value {
    match context.transcript(session_id).await {
        Ok(transcript) => {
            let tasks: Vec<String> = transcript["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.get("message"))
                .filter(|message| message["role"].as_str() == Some("user"))
                .map(message_text)
                .collect();
            json!({ "tasks": tasks })
        }
        Err(error) => json!({ "read_error": format!("{error:#}") }),
    }
}

fn tasks_of(session: &Value) -> Vec<String> {
    session["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|task| task.as_str().map(str::to_string))
        .collect()
}

// ------------------------------------------------------------- evidence

fn captured_evidence(observation: &ScenarioObservation) -> Option<Value> {
    observation
        .deliverables
        .iter()
        .find(|deliverable| deliverable.id == DELIVERABLE_ID)?
        .content
        .as_json()
        .cloned()
}

/// Everything the gates read, gathered once. Reads that fail are recorded as
/// `read_error` values, so one missing document never hides the others.
async fn collect(context: &E2eContext, observation: &ScenarioObservation, run_id: &str) -> Value {
    let own = observation.metrics.root_session_id.clone();
    let mut evidence = json!({
        "orchestrator_session": own,
        "project": project_name(run_id),
        "verify_alpha_runs": std::fs::read_to_string(counter_file(run_id))
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok()),
        "orchestrator_turns": observation
            .metrics
            .by_session
            .iter()
            .find(|row| row.session_id == own)
            .map(|row| row.turns),
    });
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
    let run_ticket = tickets.iter().find(|ticket| has_label(ticket, "dw:run"));
    let Some(run_ticket) = run_ticket else {
        evidence["error"] = json!("no ticket labelled dw:run in the run project");
        evidence["tickets"] = json!(tickets.iter().map(ticket_summary).collect::<Vec<_>>());
        return evidence;
    };
    let run_key = run_ticket["key"].as_str().unwrap_or_default().to_string();
    let mut cards: [Option<&Value>; 3] = [None, None, None];
    for ticket in &tickets {
        if ticket["key"] == run_ticket["key"] {
            continue;
        }
        if let Some(index) = card_index(ticket) {
            cards[index].get_or_insert(ticket);
        }
    }
    evidence["run_ticket"] = ticket_summary(run_ticket);
    evidence["run_key"] = json!(run_key);
    evidence["cards"] = json!(cards
        .iter()
        .map(|card| card.map(ticket_summary).unwrap_or(Value::Null))
        .collect::<Vec<_>>());
    evidence["other_tickets"] = json!(tickets
        .iter()
        .filter(|ticket| ticket["key"] != run_ticket["key"] && card_index(ticket).is_none())
        .map(ticket_summary)
        .collect::<Vec<_>>());
    evidence["wall_minutes"] = wall_minutes(run_ticket);

    let pointer_scope = format!("dw-{}", run_key.to_lowercase());
    evidence["pointer"] = json!({
        "scope": pointer_scope,
        "owner": state_get(context, &pointer_scope, "owner").await,
        "batch": state_get(context, &pointer_scope, "batch").await,
    });

    let tail = own.chars().rev().take(6).collect::<Vec<_>>();
    let tail: String = tail.into_iter().rev().collect();
    let run = format!("dw-{}-{tail}", run_key.to_lowercase());
    evidence["run"] = json!(run);

    let mut batches = Vec::new();
    for round in 1..=ROUNDS {
        for wave in 0..=WAVES {
            let wave_scope = format!("{run}-r{round}-w{wave}");
            let scope = format!("{wave_scope}-batch");
            let manifest = state_get(context, &scope, "manifest").await;
            if !non_empty(&manifest) {
                continue;
            }
            let session = manifest["session"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("{own}-batch-r{round}w{wave}"));
            batches.push(json!({
                "round": round,
                "wave": wave,
                "scope": scope,
                "manifest": manifest,
                "batch": state_get(context, &scope, "batch").await,
                "wave_results": state_list(context, &wave_scope).await,
                "lead_tasks": tasks_of(&session_tasks(context, &session).await),
            }));
        }
    }
    evidence["batches"] = json!(batches);

    // Card A: every code phase scope that could exist, and the sessions that
    // received a task for it.
    if let Some(alpha) = cards[0].and_then(|card| card["key"].as_str()) {
        let lower = alpha.to_lowercase();
        let mut code_scopes = Vec::new();
        for round in 1..=ROUNDS {
            for attempt in 1..=2 {
                let scope = format!("{run}-r{round}-{lower}-code{attempt}");
                let documents = state_list(context, &scope).await;
                code_scopes.push(json!({
                    "round": round,
                    "attempt": attempt,
                    "scope": scope,
                    "ran": non_empty(&documents),
                }));
            }
        }
        let lead = format!("{own}-{lower}");
        evidence["alpha"] = json!({
            "key": alpha,
            "code_scopes": code_scopes,
            "lead_tasks": tasks_of(&session_tasks(context, &lead).await),
            "code_tasks": tasks_of(&session_tasks(context, &format!("{lead}-code")).await),
        });
    }

    // Does the SHA the repair batch names exist in the repository?
    let repo = repo_root(run_id);
    for batch in evidence["batches"].as_array_mut().into_iter().flatten() {
        let commits: Vec<String> = batch["manifest"]["verified"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry["commit"].as_str().map(str::to_string))
            .collect();
        let mut known = Vec::new();
        for commit in commits {
            let exists = git(&repo, &["cat-file", "-e", &format!("{commit}^{{commit}}")])
                .await
                .is_ok();
            known.push(json!({ "commit": commit, "exists": exists }));
        }
        batch["verified_commits"] = json!(known);
    }
    evidence
}

fn ticket_summary(ticket: &Value) -> Value {
    json!({
        "key": ticket["key"],
        "title": ticket["title"],
        "status": ticket["status"],
        "labels": ticket["labels"],
        "assignee": ticket["assignee"],
        "created_at": ticket["created_at"],
        "status_moves": activity_status_moves(ticket)
            .into_iter()
            .map(|(at, to)| json!({ "at": at, "to": to }))
            .collect::<Vec<_>>(),
        "comments": ticket["comments"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|comment| json!({
                "author": comment["author"],
                "created_at": comment["created_at"],
                "body": comment["body"].as_str().map(|body| body.chars().take(300).collect::<String>()),
            }))
            .collect::<Vec<_>>(),
    })
}

fn wall_minutes(run_ticket: &Value) -> Value {
    let parse = |text: &str| chrono::DateTime::parse_from_rfc3339(text).ok();
    let created = run_ticket["created_at"].as_str().and_then(parse);
    let done = activity_status_moves(run_ticket)
        .into_iter()
        .rev()
        .find(|(_, to)| to == "done")
        .and_then(|(at, _)| parse(&at));
    match (created, done) {
        (Some(created), Some(done)) => json!((done - created).num_seconds() as f64 / 60.0),
        _ => Value::Null,
    }
}

// --------------------------------------------------------------- gates

struct Verdicts {
    first_batch: (bool, String),
    credited: (bool, String),
    repair: (bool, String),
    completed: (bool, String),
}

fn card(evidence: &Value, index: usize) -> &Value {
    &evidence["cards"][index]
}

fn key_of(evidence: &Value, index: usize) -> Option<String> {
    card(evidence, index)["key"].as_str().map(str::to_string)
}

fn done_verified(card: &Value) -> bool {
    card["status"].as_str() == Some("done") && has_label(card, "verified")
}

/// The `{at,to}` status moves `ticket_summary` kept.
fn moves(summary: &Value) -> Vec<(String, String)> {
    summary["status_moves"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|mv| {
            (
                mv["at"].as_str().unwrap_or_default().to_string(),
                mv["to"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn done_at(summary: &Value) -> Option<String> {
    moves(summary)
        .into_iter()
        .find(|(_, to)| to == "done")
        .map(|(at, _)| at)
}

fn batches_of(evidence: &Value) -> Vec<&Value> {
    evidence["batches"]
        .as_array()
        .into_iter()
        .flatten()
        .collect()
}

fn manifest_keys(batch: &Value) -> Vec<String> {
    batch["manifest"]["tickets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|ticket| ticket["key"].as_str().map(str::to_string))
        .collect()
}

fn judge(evidence: &Value) -> Verdicts {
    let missing = |what: &str| (false, format!("{what}; error={}", evidence["error"]));
    let (Some(a), Some(b), Some(c)) = (
        key_of(evidence, 0),
        key_of(evidence, 1),
        key_of(evidence, 2),
    ) else {
        let reason = format!(
            "cards not all found on the board (A={:?} B={:?} C={:?})",
            key_of(evidence, 0),
            key_of(evidence, 1),
            key_of(evidence, 2)
        );
        return Verdicts {
            first_batch: missing(&reason),
            credited: missing(&reason),
            repair: missing(&reason),
            completed: missing(&reason),
        };
    };
    let run_ticket = &evidence["run_ticket"];
    let batches = batches_of(evidence);

    // 1. the first batch holds A and B, was rejected for A, and the run went on.
    let first = batches
        .iter()
        .find(|batch| manifest_keys(batch).contains(&a));
    let first_outcome = first.and_then(|batch| batch["batch"]["outcome"].as_str());
    let first_cards = first.map(|batch| &batch["batch"]["cards"]);
    let first_holds_both = first.is_some_and(|batch| {
        let keys = manifest_keys(batch);
        keys.contains(&a) && keys.contains(&b)
    });
    let a_rejected = first_cards
        .and_then(|cards| cards[&a].as_str())
        .is_some_and(|verdict| verdict != "verified");
    let rejected =
        first_outcome.is_some_and(|outcome| outcome != "verified" && outcome != "accepted");
    let c_done = done_at(card(evidence, 2));
    let r_first_end = moves(run_ticket)
        .iter()
        .find(|(_, to)| to == "done" || to == "backlog")
        .map(|(at, _)| at.clone());
    let outlived_c = match (&r_first_end, &c_done) {
        (Some(end), Some(c)) => end > c,
        _ => false,
    };
    let first_batch = (
        first_holds_both && rejected && a_rejected && outlived_c,
        format!(
            "first_batch_scope={:?} holds_A_and_B={first_holds_both} outcome={first_outcome:?} card_A_verdict={:?} run_first_closed_at={r_first_end:?} C_done_at={c_done:?} verify_alpha_runs={}",
            first.map(|batch| &batch["scope"]),
            first_cards.map(|cards| &cards[&a]),
            evidence["verify_alpha_runs"],
        ),
    );

    // 2. B was credited by the rejected batch and finished before A.
    let b_credit = first_cards
        .and_then(|cards| cards[&b].as_str())
        .is_some_and(|verdict| verdict == "verified");
    let b_done = done_at(card(evidence, 1));
    let a_done = done_at(card(evidence, 0));
    let b_before_a = match (&b_done, &a_done) {
        (Some(b), Some(a)) => b < a,
        _ => false,
    };
    let credited = (
        b_credit && done_verified(card(evidence, 1)) && b_before_a,
        format!(
            "first_batch_cards[B]={:?} B_status={} B_labels={} B_done_at={b_done:?} A_done_at={a_done:?}",
            first_cards.map(|cards| &cards[&b]),
            card(evidence, 1)["status"],
            card(evidence, 1)["labels"],
        ),
    );

    // 3. repair batch r2-w0: A only, Verified B@sha, Retry line; A coded once.
    let repair_batch = batches
        .iter()
        .find(|batch| batch["round"] == 2 && batch["wave"] == 0);
    let repair_keys = repair_batch
        .map(|batch| manifest_keys(batch))
        .unwrap_or_default();
    let verified_in_manifest: Vec<(String, String)> = repair_batch
        .map(|batch| {
            batch["manifest"]["verified"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| {
                    Some((
                        entry["key"].as_str()?.to_string(),
                        entry["commit"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let b_sha = verified_in_manifest
        .iter()
        .find(|(key, _)| *key == b)
        .map(|(_, sha)| sha.clone());
    let sha_exists = repair_batch.is_some_and(|batch| {
        batch["verified_commits"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|entry| entry["exists"] == true && entry["commit"].as_str() == b_sha.as_deref())
    });
    let task = repair_batch
        .map(|batch| tasks_of_value(&batch["lead_tasks"]))
        .and_then(|tasks| tasks.into_iter().next())
        .unwrap_or_default();
    let has_retry = task
        .lines()
        .any(|line| line.trim_start().starts_with("Retry:"));
    let verified_line = b_sha.as_ref().is_some_and(|sha| {
        task.lines().any(|line| {
            line.trim_start().starts_with("Verified:") && line.contains(&format!("{b}@{sha}"))
        })
    });
    let alpha = &evidence["alpha"];
    let ran: Vec<(u64, u64)> = alpha["code_scopes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|scope| scope["ran"] == true)
        .map(|scope| {
            (
                scope["round"].as_u64().unwrap_or(0),
                scope["attempt"].as_u64().unwrap_or(0),
            )
        })
        .collect();
    let coded_round_one_only =
        ran.iter().any(|(round, _)| *round == 1) && ran.iter().all(|(round, _)| *round == 1);
    let code_tasks = tasks_of_value(&alpha["code_tasks"]).len();
    let later_lead_tasks = tasks_of_value(&alpha["lead_tasks"])
        .iter()
        .filter(|task| task.contains(", round 2") || task.contains(", round 3"))
        .count();
    let repair = (
        repair_batch.is_some()
            && repair_keys == [a.clone()]
            && sha_exists
            && has_retry
            && verified_line
            && coded_round_one_only
            && code_tasks == 1
            && later_lead_tasks == 0,
        format!(
            "r2w0_manifest_tickets={repair_keys:?} verified={verified_in_manifest:?} sha_in_repo={sha_exists} task_has_Retry={has_retry} task_has_Verified_B@sha={verified_line} A_code_phases_ran(round,attempt)={ran:?} A_code_session_tasks={code_tasks} A_lead_tasks_for_round_2plus={later_lead_tasks}"
        ),
    );

    // 4. completion.
    let session = evidence["orchestrator_session"].as_str();
    let owner = evidence["pointer"]["owner"]["session"].as_str();
    let batch_scope_null =
        non_empty(&evidence["pointer"]["batch"]) && evidence["pointer"]["batch"]["scope"].is_null();
    let c_after_b = match (&b_done, &c_done) {
        (Some(b), Some(c)) => b <= c,
        _ => false,
    };
    let completed = (
        (0..3).all(|index| done_verified(card(evidence, index)))
            && run_ticket["status"].as_str() == Some("done")
            && owner.is_some()
            && owner == session
            && batch_scope_null
            && c_after_b,
        format!(
            "cards=[A {} {}, B {} {}, C {} {}] run_status={} pointer_owner={owner:?} orchestrator={session:?} pointer_batch={} C_done_after_B={c_after_b}",
            a, card(evidence, 0)["status"], b, card(evidence, 1)["status"], c, card(evidence, 2)["status"],
            run_ticket["status"],
            evidence["pointer"]["batch"],
        ),
    );
    Verdicts {
        first_batch,
        credited,
        repair,
        completed,
    }
}

fn tasks_of_value(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|task| task.as_str().map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(key: &str, status: &str, labels: &[&str], moves: &[(&str, &str)]) -> Value {
        json!({
            "key": key,
            "status": status,
            "labels": labels,
            "status_moves": moves.iter().map(|(at, to)| json!({"at": at, "to": to})).collect::<Vec<_>>(),
        })
    }

    fn passing_evidence() -> Value {
        json!({
            "orchestrator_session": "e2e_abc123",
            "verify_alpha_runs": 2,
            "run_ticket": summary("KAN-1", "done", &["dw:run"], &[("2026-01-01T03:00:00Z", "done")]),
            "cards": [
                summary("KAN-2", "done", &["verified"], &[("2026-01-01T02:00:00Z", "done")]),
                summary("KAN-3", "done", &["verified"], &[("2026-01-01T01:00:00Z", "done")]),
                summary("KAN-4", "done", &["verified"], &[("2026-01-01T02:30:00Z", "done")]),
            ],
            "pointer": {"owner": {"session": "e2e_abc123"}, "batch": {"item": "batch", "scope": null}},
            "alpha": {
                "code_scopes": [
                    {"round": 1, "attempt": 1, "ran": true},
                    {"round": 2, "attempt": 1, "ran": false},
                ],
                "lead_tasks": ["Lead ticket KAN-2, run x, round 1."],
                "code_tasks": ["Code phase of ticket KAN-2"],
            },
            "batches": [
                {"round": 1, "wave": 1, "scope": "r1w1", "manifest": {"tickets": [{"key": "KAN-2"}, {"key": "KAN-3"}]},
                 "batch": {"outcome": "failed", "cards": {"KAN-2": "failed", "KAN-3": "verified"}}, "lead_tasks": []},
                {"round": 2, "wave": 0, "scope": "r2w0",
                 "manifest": {"tickets": [{"key": "KAN-2"}], "verified": [{"key": "KAN-3", "commit": "abc"}]},
                 "verified_commits": [{"commit": "abc", "exists": true}],
                 "batch": {"outcome": "verified"},
                 "lead_tasks": ["Lead ticket KAN-1\nVerified: KAN-3@abc\nRetry: verify_alpha failed."]},
            ],
        })
    }

    #[test]
    fn the_contract_run_passes_every_gate() {
        let verdicts = judge(&passing_evidence());
        for (name, (passed, reason)) in [
            ("first", verdicts.first_batch),
            ("credited", verdicts.credited),
            ("repair", verdicts.repair),
            ("completed", verdicts.completed),
        ] {
            assert!(passed, "{name}: {reason}");
        }
    }

    #[test]
    fn a_reimplemented_card_and_an_early_close_fail_their_gates() {
        let mut evidence = passing_evidence();
        evidence["alpha"]["code_scopes"][1]["ran"] = json!(true);
        evidence["run_ticket"]["status_moves"][0]["at"] = json!("2026-01-01T01:30:00Z");
        let verdicts = judge(&evidence);
        assert!(!verdicts.repair.0, "{}", verdicts.repair.1);
        assert!(!verdicts.first_batch.0, "{}", verdicts.first_batch.1);
        assert!(verdicts.credited.0);
    }

    #[test]
    fn cards_are_told_apart_by_their_files_line() {
        let ticket = json!({"description": "Deliverable: x\nFiles: src/bravo.py, tests/test_bravo.py\nVerify: python3 charlie"});
        assert_eq!(card_index(&ticket), Some(1));
        assert_eq!(
            card_index(&json!({"description": "Goal: src/alpha.py"})),
            None
        );
    }

    #[tokio::test]
    async fn the_fixture_is_a_clean_repo_and_alpha_verify_fails_exactly_once() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("run");
        let script = root.join("verify_alpha.sh");
        let counter = root.join("verify_alpha.runs");
        prepare_workspace(&root, &script, &counter).await.unwrap();
        let repo = root.join("repo");
        assert_eq!(git(&repo, &["status", "--porcelain"]).await.unwrap(), "");
        let run = || {
            std::process::Command::new("sh")
                .arg(&script)
                .current_dir(&repo)
                .output()
                .unwrap()
        };
        assert_eq!(run().status.code(), Some(1));
        // From the second run on the script judges the code: the stub fails it,
        // the implementation passes it.
        assert_ne!(run().status.code(), Some(0));
        std::fs::write(
            repo.join("src/alpha.py"),
            "def alpha():\n    return \"alpha\"\n",
        )
        .unwrap();
        assert_eq!(run().status.code(), Some(0));
        assert_eq!(std::fs::read_to_string(&counter).unwrap().trim(), "3");
        assert_eq!(
            git(&repo, &["status", "--porcelain"]).await.unwrap(),
            "M src/alpha.py"
        );
    }

    #[test]
    fn the_scenario_materializes() {
        DeepworkBatchRepair.spec("run").validate().unwrap();
        crate::scenarios::ScenarioId::DeepworkBatchRepair
            .materialize("case", 3)
            .unwrap();
    }
}
