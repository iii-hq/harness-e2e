//! The onboarding tour a new user takes after `iii project init --learn-iii`,
//! as one scripted dialogue under the `harness` template's Onboarding profile.
//!
//! The `onboarding` stack is that project: the `harness` template plus the
//! tour's worker. The dialogue sends each step's prompt as the tour page's
//! button does, in tour order, and this module plays the page itself: `setup`
//! closes the button-only steps as their buttons would (`stay-in-touch` is
//! skipped, never subscribed), binds every other step's condition the way the
//! page binds it, and closes that step with `onboarding::steps::complete` when
//! its trigger fires. `capture` then checks what each step should have left
//! behind, and checks off the agent's TODO items as the user would.
//!
//! `tests/fixtures/onboarding-tour/console-basics.json` is the tour this test
//! was written against: its steps, prompts, conditions and profile, projected
//! from `onboarding::tours::get`. A tour the worker serves differently costs
//! `tour.current` and nothing else; regenerate the fixture from the worker's
//! `src/tours.mjs` when the tour changes on purpose.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use iii_sdk::protocol::{RegisterTriggerInput, TriggerRequest};
use iii_sdk::runtime::FunctionRef;
use iii_sdk::trigger::Trigger;
use iii_sdk::{IIIClient, RegisterFunction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::common::{atomic_awards, lock_unpoisoned};
use super::*;
use crate::context::E2eContext;
use crate::report::EvaluationDimension;

pub const ID: &str = "onboarding_tour";
const EVIDENCE_ID: &str = "onboarding_evidence";
const TOUR_ID: &str = "console-basics";
/// The profile the test fixes: "Onboarding" in the `harness` template, hidden
/// from the new-chat gallery, which the wizard's build examples run. The tour
/// itself names none yet; it writes into whatever chat is open.
const PROFILE: &str = "ade-worker-builder";
const TOUR_WORKER: &str = "onboarding";
/// Measurement, not part of the system the agent is asked to describe.
const RUNNER_WORKER: &str = "harness-e2e";
const FIXTURE: &str = include_str!("../../tests/fixtures/onboarding-tour/console-basics.json");
/// How long a checked-off item may take to ring the TODO worker's trigger.
const CHECK_OFF_WAIT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Tour {
    id: String,
    #[serde(default)]
    agent: Option<String>,
    steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Step {
    id: String,
    /// The prompt the step's button sends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ask: Option<String>,
    /// The trigger the step waits for; without one a button closes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    condition: Option<Condition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Condition {
    #[serde(rename = "type")]
    kind: String,
    config: Value,
}

fn tour() -> &'static Tour {
    static TOUR: OnceLock<Tour> = OnceLock::new();
    TOUR.get_or_init(|| serde_json::from_str(FIXTURE).expect("onboarding tour fixture"))
}

/// What this test reads of the tour the worker serves, in the fixture's shape.
fn project(live: &Value) -> Tour {
    let text = |value: &Value| value.as_str().map(str::to_owned);
    Tour {
        id: text(&live["id"]).unwrap_or_default(),
        agent: text(&live["agent"]),
        steps: live["steps"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|step| Step {
                id: text(&step["id"]).unwrap_or_default(),
                ask: text(&step["ask"]["text"]),
                condition: step["condition"].is_object().then(|| Condition {
                    kind: text(&step["condition"]["type"]).unwrap_or_default(),
                    config: match &step["condition"]["config"] {
                        Value::Null => json!({}),
                        config => config.clone(),
                    },
                }),
            })
            .collect(),
    }
}

fn prompts() -> Vec<String> {
    tour()
        .steps
        .iter()
        .filter_map(|step| step.ask.clone())
        .collect()
}

/// The exchange a step's prompt opens: its place among the prompts.
fn exchange_of(step_id: &str) -> Option<usize> {
    tour()
        .steps
        .iter()
        .filter(|step| step.ask.is_some())
        .position(|step| step.id == step_id)
}

struct Metric {
    id: &'static str,
    weight: u8,
    ratio: bool,
    /// Points at which the criterion passes as a gate; `None` only scores.
    gate: Option<u8>,
    label: &'static str,
    question: &'static str,
}

const METRICS: [Metric; 12] = [
    Metric {
        id: "tour.complete",
        weight: 20,
        ratio: true,
        gate: Some(20),
        label: "Every tour step closed",
        question: "Does the tour's own progress show every step of the tour complete?",
    },
    Metric {
        id: "composability.database",
        weight: 8,
        ratio: false,
        gate: Some(8),
        label: "The database worker runs",
        question: "Did the Composability step add a database worker that is running?",
    },
    Metric {
        id: "discoverability.workers",
        weight: 8,
        ratio: true,
        gate: Some(6),
        label: "The answer names the registered workers",
        question: "Does the Discoverability answer name the workers registered when it was given?",
    },
    Metric {
        id: "extensibility.worker",
        weight: 12,
        ratio: false,
        gate: Some(12),
        label: "A TODO worker with CRUD functions",
        question: "Did the Extensibility step add a worker of its own with at least four functions?",
    },
    Metric {
        id: "extensibility.page",
        weight: 8,
        ratio: false,
        gate: Some(8),
        label: "The TODO worker has a Console page",
        question: "Does that worker inject a page into the Console?",
    },
    Metric {
        id: "extensibility.row_changed",
        weight: 6,
        ratio: false,
        gate: None,
        label: "It reacts to database::row-changed",
        question: "Did that worker bind a database::row-changed trigger?",
    },
    Metric {
        id: "extensibility.trigger_type",
        weight: 4,
        ratio: false,
        gate: None,
        label: "It registers a trigger type",
        question: "Did that worker register a trigger type of its own?",
    },
    Metric {
        id: "reactivity.items",
        weight: 6,
        ratio: false,
        gate: Some(6),
        label: "Three open TODO items",
        question: "Does the TODO table hold at least three items not yet checked off?",
    },
    Metric {
        id: "reactivity.check_off",
        weight: 12,
        ratio: true,
        gate: Some(12),
        label: "Checking an item off rings a trigger",
        question: "When the user checks each of three items off, does the TODO worker's trigger fire?",
    },
    Metric {
        id: "reactivity.user_triggers",
        weight: 6,
        ratio: false,
        gate: None,
        label: "A trigger waits on the check-off",
        question: "Is a trigger bound to the TODO worker's trigger type, for the user's check-offs?",
    },
    Metric {
        id: "cleanup.removed",
        weight: 8,
        ratio: false,
        gate: Some(8),
        label: "The onboarding worker is removed",
        question: "Did the Clean up step remove the onboarding worker from the project?",
    },
    Metric {
        id: "tour.current",
        weight: 2,
        ratio: false,
        gate: None,
        label: "The tour is the one this test knows",
        question: "Does the worker serve the tour this test was written against: same steps, prompts, conditions and profile?",
    },
];

fn metrics() -> &'static [Value] {
    static ALL: OnceLock<Vec<Value>> = OnceLock::new();
    ALL.get_or_init(|| {
        METRICS
            .iter()
            .map(|m| {
                json!({
                    "id": m.id,
                    "weight": m.weight,
                    "measurement": if m.ratio { "ratio" } else { "binary" },
                })
            })
            .collect()
    })
}

fn binary(id: &str, passed: bool, observed: Value) -> Value {
    json!({"id": id, "status": "measured", "value": u8::from(passed), "observed": observed})
}

fn ratio(id: &str, numerator: usize, denominator: usize, observed: Value) -> Value {
    let numerator = numerator.min(denominator);
    json!({
        "id": id,
        "status": if denominator == 0 { "not_applicable" } else { "measured" },
        "numerator": numerator,
        "denominator": denominator,
        "observed": observed,
    })
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn digest(run_id: &str) -> String {
    crate::artifact::sha256_bytes(run_id.as_bytes()).replace("sha256:", "")
}

fn root(run_id: &str) -> PathBuf {
    std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("harness-e2e-onboarding")
        .join(format!("{ID}-{}", &digest(run_id)[..16]))
}

/// What the page records while the dialogue runs: the steps it closed, the
/// tour's progress after each, and what the system held when the agent said
/// what it can do.
#[derive(Default, Serialize)]
struct Observed {
    closed: Vec<Value>,
    #[serde(skip)]
    closed_ids: HashSet<String>,
    progress: Option<Value>,
    discoverability_workers: Option<Value>,
    errors: Vec<String>,
}

struct Run {
    functions: Vec<FunctionRef>,
    triggers: Vec<Trigger>,
    observed: Arc<Mutex<Observed>>,
    live_tour: Value,
    baseline: Value,
}

fn runs() -> &'static Mutex<HashMap<String, Run>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Run>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A call from a trigger handler, routed as `E2eContext` routes it.
async fn call(client: &IIIClient, function_id: &str, payload: Value) -> Result<Value> {
    let (namespace, payload) =
        crate::context::inspection_routing(function_id, client.namespace(), payload);
    client
        .trigger(
            TriggerRequest {
                function_id: function_id.to_string(),
                payload,
                action: None,
                timeout_ms: Some(30_000),
            }
            .namespace(namespace),
        )
        .await
        .map_err(|error| anyhow!("{function_id}: {error}"))
}

/// Close one step as the page does, then snapshot the tour's progress.
async fn close(
    client: &IIIClient,
    observed: &Mutex<Observed>,
    step_id: &str,
    fired: Option<Value>,
) {
    if !lock_unpoisoned(observed)
        .closed_ids
        .insert(step_id.to_string())
    {
        return;
    }
    let mut request = json!({"tour_id": TOUR_ID, "step_id": step_id});
    if let Some(fired) = &fired {
        request["fired"] = fired.clone();
    }
    let closed = call(client, "onboarding::steps::complete", request).await;
    let progress = call(client, "onboarding::progress::get", json!({})).await;
    let mut observed = lock_unpoisoned(observed);
    observed.closed.push(json!({
        "step": step_id,
        "at_ms": now_ms(),
        "trigger_type": fired.as_ref().map(|fired| fired["trigger_type"].clone()),
    }));
    match closed {
        Ok(_) => {}
        Err(error) => observed.errors.push(format!("close {step_id}: {error:#}")),
    }
    match progress {
        Ok(progress) => observed.progress = Some(progress),
        Err(error) => observed
            .errors
            .push(format!("progress after {step_id}: {error:#}")),
    }
}

/// A condition fired: close its step and, when the next step only sends a
/// prompt (Clean up), that one too, since its prompt goes out next and the
/// worker it would report to is the one being removed.
async fn on_condition(
    client: &IIIClient,
    observed: &Mutex<Observed>,
    step_id: &str,
    fired: Value,
    next_prompt_only: Option<&str>,
) {
    if lock_unpoisoned(observed).closed_ids.contains(step_id) {
        return;
    }
    if step_id == "discoverability" {
        let workers = call(client, "engine::workers::list", json!({})).await;
        let mut observed = lock_unpoisoned(observed);
        match workers {
            Ok(workers) => observed.discoverability_workers = Some(workers),
            Err(error) => observed
                .errors
                .push(format!("workers at discoverability: {error:#}")),
        }
    }
    close(client, observed, step_id, Some(fired)).await;
    if let Some(next) = next_prompt_only {
        close(client, observed, next, None).await;
    }
}

fn bind(
    context: &E2eContext,
    run_id: &str,
    step: &Step,
    condition: &Condition,
    next_prompt_only: Option<String>,
    observed: Arc<Mutex<Observed>>,
) -> Result<(FunctionRef, Trigger)> {
    let function_id = format!("e2e::onboarding::{}::{}", &digest(run_id)[..12], step.id);
    let client = context.client().clone();
    let step_id = step.id.clone();
    let trigger_type = condition.kind.clone();
    let handler_id = function_id.clone();
    let function = context.client().register_function(
        function_id.clone(),
        RegisterFunction::new_async(move |payload: Value| {
            let client = client.clone();
            let observed = observed.clone();
            let step_id = step_id.clone();
            let next = next_prompt_only.clone();
            let fired = json!({
                "trigger_type": trigger_type,
                "function_id": handler_id,
                "payload": payload,
            });
            async move {
                on_condition(&client, &observed, &step_id, fired, next.as_deref()).await;
                Ok::<Value, iii_sdk::errors::Error>(json!({"ok": true}))
            }
        })
        .description("The onboarding tour page's condition for one step, played by the E2E runner.")
        .metadata(json!({"internal": true})),
    );
    let trigger = context
        .client()
        .register_trigger(RegisterTriggerInput::new(
            condition.kind.clone(),
            function_id,
            condition.config.clone(),
        ))?;
    Ok((function, trigger))
}

fn items<'a>(value: &'a Value, field: &str) -> impl Iterator<Item = &'a Value> {
    value[field].as_array().into_iter().flatten()
}

fn connected_workers(listed: &Value) -> Vec<String> {
    items(listed, "workers")
        .filter(|worker| worker["status"] == "connected")
        .filter_map(|worker| worker["name"].as_str().map(str::to_owned))
        .collect()
}

fn ready_containers(status: &Value) -> Vec<String> {
    items(status, "containers")
        .filter(|container| container["state"] == "ready")
        .filter_map(|container| container["container"].as_str().map(str::to_owned))
        .collect()
}

fn declared_containers(status: &Value) -> Vec<String> {
    items(status, "containers")
        .filter_map(|container| container["container"].as_str().map(str::to_owned))
        .collect()
}

/// The assistant's text in each exchange: everything it wrote from one
/// runner message up to the next. Wake notifications open turns, not
/// exchanges.
fn exchange_texts(transcript: &Value) -> Vec<String> {
    let mut texts: Vec<String> = Vec::new();
    for entry in items(transcript, "messages") {
        let message = &entry["message"];
        let text = message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        match message["role"].as_str() {
            Some("user")
                if entry["origin"]["notification"] != true
                    && !text.trim_start().starts_with("[notification]") =>
            {
                texts.push(String::new())
            }
            Some("assistant") => {
                if let Some(last) = texts.last_mut() {
                    last.push_str(&text);
                    last.push('\n');
                }
            }
            _ => {}
        }
    }
    texts
}

/// A SQL identifier the agent chose, accepted only when it needs no quoting.
fn identifier(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    let mut chars = value.chars();
    (chars.next()?.is_ascii_alphabetic() || value.starts_with('_'))
        .then_some(())
        .filter(|()| value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .map(|()| value.to_string())
}

/// The flag that says an item is checked off, not the time it was.
fn is_done_column(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["done", "complet", "check", "finish"]
        .iter()
        .any(|word| name.contains(word))
        && !["_at", "_on", "time", "date"]
            .iter()
            .any(|stamp| name.ends_with(stamp))
}

fn is_open(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(done) => !done,
        Value::Number(number) => number.as_f64() == Some(0.0),
        Value::String(text) => matches!(
            text.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "f" | "no"
        ),
        _ => false,
    }
}

/// The TODO worker the Extensibility step added: a worker that was not
/// there before the dialogue, is not the database or the tour, and has a
/// Console page, else the one with the most functions.
fn todo_worker(baseline: &Value, workers: &Value, registered: &Value) -> Option<String> {
    let before = connected_workers(&baseline["workers"]);
    let with_page: HashSet<&str> = items(registered, "registered_triggers")
        .filter(|trigger| trigger["trigger_type"] == "console:script")
        .filter_map(|trigger| trigger["worker_name"].as_str())
        .collect();
    items(workers, "workers")
        .filter(|worker| worker["status"] == "connected")
        .filter_map(|worker| {
            let name = worker["name"].as_str()?;
            (!before.iter().any(|known| known == name)
                && ![TOUR_WORKER, "database", RUNNER_WORKER].contains(&name))
            .then(|| {
                (
                    with_page.contains(name),
                    worker["function_count"].as_u64().unwrap_or(0),
                    name.to_string(),
                )
            })
        })
        .max()
        .map(|(_, _, name)| name)
}

/// Check off up to three open items as the user would, each with a write
/// through the database worker (which rings `database::row-changed`), and
/// count those the TODO worker answered with its own trigger.
async fn check_off(
    context: &E2eContext,
    run_id: &str,
    db: &str,
    table: &str,
    trigger_types: &[String],
) -> Result<(usize, usize, Value)> {
    let described = context
        .trigger_value("database::describeTable", json!({"db": db, "table": table}))
        .await?;
    let columns: Vec<&Value> = items(&described, "columns")
        .chain(items(&described["table"], "columns"))
        .collect();
    let column = |pick: &dyn Fn(&Value) -> bool| {
        columns
            .iter()
            .find(|column| pick(column))
            .and_then(|column| identifier(column["name"].as_str()))
    };
    let key = column(&|column| column["primary_key"] == true)
        .context("the TODO table has no primary key")?;
    let done = column(&|column| column["name"].as_str().is_some_and(is_done_column))
        .context("the TODO table has no column that says an item is done")?;
    let boolean = columns.iter().any(|column| {
        column["name"].as_str() == Some(done.as_str())
            && column["type"]
                .as_str()
                .is_some_and(|ty| ty.to_ascii_lowercase().contains("bool"))
    });
    let rows = context
        .trigger_value(
            "database::query",
            json!({"db": db, "sql": format!("SELECT {key}, {done} FROM {table} LIMIT 200")}),
        )
        .await?;
    let open: Vec<Value> = items(&rows, "rows")
        .filter(|row| is_open(&row[&done]))
        .map(|row| row[&key].clone())
        .collect();

    let heard = Arc::new(Mutex::new(Vec::<u128>::new()));
    let function_id = format!("e2e::onboarding::{}::check-off", &digest(run_id)[..12]);
    let sink = heard.clone();
    let function = context.client().register_function(
        function_id.clone(),
        RegisterFunction::new_async(move |_payload: Value| {
            let sink = sink.clone();
            async move {
                lock_unpoisoned(&sink).push(now_ms());
                Ok::<Value, iii_sdk::errors::Error>(json!({"ok": true}))
            }
        })
        .description("Hears the TODO worker's trigger while the E2E runner checks items off.")
        .metadata(json!({"internal": true})),
    );
    let mut bindings = Vec::new();
    for trigger_type in trigger_types {
        match context.client().register_trigger(RegisterTriggerInput::new(
            trigger_type.clone(),
            function_id.clone(),
            json!({}),
        )) {
            Ok(binding) => bindings.push(binding),
            Err(error) => {
                tracing::warn!(%trigger_type, %error, "could not bind the TODO worker's trigger")
            }
        }
    }
    let mut checked = Vec::new();
    let mut rang = 0;
    for id in open.iter().take(3) {
        let before = lock_unpoisoned(&heard).len();
        let written = context
            .trigger_value(
                "database::execute",
                json!({
                    "db": db,
                    "sql": format!("UPDATE {table} SET {done} = ? WHERE {key} = ? RETURNING {key}"),
                    "params": [if boolean { json!(true) } else { json!(1) }, id],
                }),
            )
            .await;
        let deadline = tokio::time::Instant::now() + CHECK_OFF_WAIT;
        while written.is_ok()
            && lock_unpoisoned(&heard).len() == before
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let answered = lock_unpoisoned(&heard).len() > before;
        rang += usize::from(answered);
        checked.push(json!({
            "id": id,
            "written": written.as_ref().map(|_| true).unwrap_or(false),
            "error": written.err().map(|error| format!("{error:#}")),
            "answered": answered,
        }));
    }
    for binding in bindings {
        binding.unregister();
    }
    function.unregister();
    Ok((
        open.len(),
        rang,
        json!({"db": db, "table": table, "key": key, "done": done, "open": open.len(), "checked": checked}),
    ))
}

pub struct OnboardingTour;

#[async_trait]
impl Scenario for OnboardingTour {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Complete the onboarding tour")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("On the project `iii project init --learn-iii` creates, take the console tour step by step as a new user does: say hello, add a database, list what the system can do, build a reactive TODO worker with a Console page, check its items off, then remove the tour.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("The onboarding a new user is handed works end to end: the tour's prompts, run under the template's Onboarding profile on a fresh harness project, take an agent through every step. If it fails, a new user's first session with iii ends in a tour they cannot finish.")
    }

    fn execution_kind(&self) -> ScenarioExecutionKind {
        ScenarioExecutionKind::ScriptedDialogue
    }

    fn canonical_seed_only(&self) -> bool {
        true
    }

    fn agent_profile(&self) -> Option<&'static str> {
        Some(PROFILE)
    }

    fn case(&self, _seed: u64) -> Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            super::stable_seed(ID),
            json!({
                "tour": TOUR_ID,
                "agent": PROFILE,
                "steps": tour().steps.len(),
                "exchanges": prompts().len(),
                "tour_sha256": crate::artifact::sha256_bytes(FIXTURE.as_bytes()),
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiFunctions,
                Capability::IiiCompose,
                Capability::HarnessScriptedDialogueV1,
            ],
            DeliverableContract {
                artifacts: vec![ArtifactExpectation {
                    id: EVIDENCE_ID.into(),
                    kind: "application_audit".into(),
                    media_type: "application/json".into(),
                    schema: json!({"type":"object","required":["observations","tour","closed"]}),
                    max_size_bytes: crate::asset::DEFAULT_MAX_CAPTURE_BYTES,
                }],
                invariants: vec![],
                provenance_required: true,
                capture_before_cleanup: true,
            },
        )
    }

    fn spec(&self, _run_id: &str) -> ScenarioSpec {
        ScenarioSpec {
            id: ID,
            prompt: prompts()[0].clone(),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(200),
                max_output_tokens: Some(32_768),
                max_total_tokens: Some(4_000_000),
                stuck_timeout_seconds: 900,
                max_validation_retries: None,
            },
            // The page closes the steps and the user signs up, never the agent.
            denied_functions: &[
                "onboarding::steps::*",
                "onboarding::subscribe",
                "approval::*",
            ],
            criteria: METRICS
                .iter()
                .map(|m| {
                    let spec = CriterionSpec::scored(
                        m.id,
                        m.weight,
                        m.question,
                        EvaluationDimension::Deliverable,
                    )
                    .labeled(m.label);
                    match m.gate {
                        Some(points) if points < m.weight => spec.with_gate_minimum(points),
                        Some(_) => spec.with_gate(true),
                        None => spec,
                    }
                })
                .collect(),
        }
    }

    /// The prompts after the first, each sent once the one before has its
    /// answer, as the user clicks down the tour.
    fn dialogue_followups(&self, _run_id: &str) -> Vec<String> {
        prompts().into_iter().skip(1).collect()
    }

    /// The project folder, which a new chat in the ADE starts in.
    fn prepared_root(&self, run_id: &str) -> Result<Option<PathBuf>> {
        let path = root(run_id).join("project.json");
        if !path.is_file() {
            return Ok(None);
        }
        let project: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        Ok(project["project_root"].as_str().map(PathBuf::from))
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> Result<()> {
        let status = context
            .trigger_value("compose::status", json!({}))
            .await
            .context("compose::status failed; is the onboarding stack up?")?;
        if !ready_containers(&status)
            .iter()
            .any(|name| name == TOUR_WORKER)
        {
            bail!(
                "the {TOUR_WORKER} container is not ready; this test runs on the onboarding stack"
            );
        }
        let project_root = status["file"]
            .as_str()
            .map(PathBuf::from)
            .and_then(|file| file.parent().map(PathBuf::from))
            .context("compose::status did not name the project file")?;
        let live = context
            .trigger_value("onboarding::tours::get", json!({"id": TOUR_ID}))
            .await
            .context("the onboarding worker does not serve its tour")?;
        let baseline = json!({
            "compose": status,
            "workers": context.trigger_value("engine::workers::list", json!({})).await?,
            "registered_triggers": context.trigger_value("engine::registered-triggers::list", json!({})).await?,
        });
        let directory = root(run_id);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(
            directory.join("project.json"),
            serde_json::to_vec_pretty(&json!({"project_root": project_root}))?,
        )?;

        let observed = Arc::new(Mutex::new(Observed::default()));
        // Registered first, so `cleanup` unbinds whatever a failed setup bound.
        lock_unpoisoned(runs()).insert(
            run_id.to_string(),
            Run {
                functions: Vec::new(),
                triggers: Vec::new(),
                observed: observed.clone(),
                live_tour: live["tour"].clone(),
                baseline,
            },
        );
        let steps = &tour().steps;
        for (index, step) in steps.iter().enumerate() {
            match &step.condition {
                Some(condition) => {
                    let next_prompt_only = steps
                        .get(index + 1)
                        .filter(|next| next.ask.is_some() && next.condition.is_none())
                        .map(|next| next.id.clone());
                    let (function, trigger) = bind(
                        context,
                        run_id,
                        step,
                        condition,
                        next_prompt_only,
                        observed.clone(),
                    )?;
                    let mut runs = lock_unpoisoned(runs());
                    let run = runs.get_mut(run_id).expect("registered above");
                    run.functions.push(function);
                    run.triggers.push(trigger);
                }
                // A button the user clicks while reading: Continue, or Skip on
                // the sign-up box.
                None if step.ask.is_none() => {
                    close(context.client(), &observed, &step.id, None).await;
                }
                None => {}
            }
        }
        Ok(())
    }

    /// Steps the agent finished before a later one stalled still count.
    fn evaluates_failed_subjects(&self) -> bool {
        true
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> Result<Vec<CapturedDeliverable>> {
        let (observed, live_tour, baseline) = {
            let runs = lock_unpoisoned(runs());
            let run = runs.get(run_id).context("onboarding setup did not run")?;
            (
                run.observed.clone(),
                run.live_tour.clone(),
                run.baseline.clone(),
            )
        };
        let status = context.trigger_value("compose::status", json!({})).await?;
        let workers = context
            .trigger_value("engine::workers::list", json!({}))
            .await?;
        let trigger_types = context
            .trigger_value("engine::triggers::list", json!({}))
            .await?;
        let registered = context
            .trigger_value("engine::registered-triggers::list", json!({}))
            .await?;
        let (progress, closed, discoverability_workers, errors) = {
            let observed = lock_unpoisoned(&observed);
            (
                observed.progress.clone(),
                observed.closed.clone(),
                observed.discoverability_workers.clone(),
                observed.errors.clone(),
            )
        };
        let mut observations = Vec::new();

        let steps = &tour().steps;
        let complete = steps
            .iter()
            .filter(|step| {
                progress.as_ref().is_some_and(|progress| {
                    progress["tours"][TOUR_ID]["steps"][&step.id]["status"] == "complete"
                })
            })
            .count();
        observations.push(ratio(
            "tour.complete",
            complete,
            steps.len(),
            json!({"progress": progress}),
        ));

        let running = connected_workers(&workers);
        let ready = ready_containers(&status);
        observations.push(binary(
            "composability.database",
            ready.iter().any(|name| name == "database")
                || running.iter().any(|name| name == "database"),
            json!({"ready_containers": ready, "workers": running}),
        ));

        let answer = exchange_of("discoverability")
            .and_then(|index| exchange_texts(&observation.transcript).get(index).cloned())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let described = discoverability_workers.as_ref().unwrap_or(&workers);
        let registered_then: Vec<String> = connected_workers(described)
            .into_iter()
            .filter(|name| name != RUNNER_WORKER)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let named: Vec<&String> = registered_then
            .iter()
            .filter(|name| answer.contains(&name.to_ascii_lowercase()))
            .collect();
        observations.push(ratio(
            "discoverability.workers",
            named.len(),
            registered_then.len(),
            json!({"named": named, "registered": registered_then, "snapshot": discoverability_workers.is_some()}),
        ));

        let todo = todo_worker(&baseline, &workers, &registered);
        let by_todo =
            |trigger: &&Value| todo.is_some() && trigger["worker_name"].as_str() == todo.as_deref();
        let functions = todo
            .as_deref()
            .and_then(|name| {
                items(&workers, "workers")
                    .find(|worker| worker["name"].as_str() == Some(name))
                    .and_then(|worker| worker["function_count"].as_u64())
            })
            .unwrap_or(0);
        observations.push(binary(
            "extensibility.worker",
            functions >= 4,
            json!({"worker": todo, "function_count": functions}),
        ));
        let page = items(&registered, "registered_triggers")
            .filter(by_todo)
            .any(|trigger| trigger["trigger_type"] == "console:script");
        observations.push(binary("extensibility.page", page, json!({"worker": todo})));
        let row_changed: Vec<Value> = items(&registered, "registered_triggers")
            .filter(by_todo)
            .filter(|trigger| trigger["trigger_type"] == "database::row-changed")
            .cloned()
            .collect();
        observations.push(binary(
            "extensibility.row_changed",
            !row_changed.is_empty(),
            json!({"bindings": row_changed}),
        ));
        let own_types: Vec<String> = items(&trigger_types, "triggers")
            .filter(by_todo)
            .filter_map(|trigger| trigger["id"].as_str().map(str::to_owned))
            .collect();
        observations.push(binary(
            "extensibility.trigger_type",
            !own_types.is_empty(),
            json!({"trigger_types": own_types}),
        ));

        // Bound before this check binds its own listener on the same types.
        let waiting: Vec<Value> = items(&registered, "registered_triggers")
            .filter(|trigger| {
                trigger["trigger_type"]
                    .as_str()
                    .is_some_and(|kind| own_types.iter().any(|own| own == kind))
            })
            .cloned()
            .collect();
        let binding = row_changed.first().map(|binding| &binding["config"]);
        let db = binding
            .and_then(|config| identifier(config["db"].as_str()))
            .unwrap_or_else(|| "primary".into());
        let table = match binding.and_then(|config| identifier(config["table"].as_str())) {
            Some(table) => Some(table),
            None => context
                .trigger_value("database::listTables", json!({"db": db}))
                .await
                .ok()
                .and_then(|listed| {
                    items(&listed, "tables")
                        .filter_map(|table| table["name"].as_str())
                        .find(|name| name.to_ascii_lowercase().contains("todo"))
                        .and_then(|name| identifier(Some(name)))
                }),
        };
        let checked = match &table {
            Some(table) => check_off(context, run_id, &db, table, &own_types)
                .await
                .map_err(|error| format!("{error:#}")),
            None => Err("no TODO table in the database".into()),
        };
        let (open, rang, check_evidence) = match checked {
            Ok(checked) => checked,
            Err(error) => (0, 0, json!({"error": error})),
        };
        observations.push(binary(
            "reactivity.items",
            open >= 3,
            json!({"open": open, "table": table}),
        ));
        observations.push(ratio("reactivity.check_off", rang, 3, check_evidence));
        observations.push(binary(
            "reactivity.user_triggers",
            !waiting.is_empty(),
            json!({"bindings": waiting}),
        ));

        observations.push(binary(
            "cleanup.removed",
            !declared_containers(&status)
                .iter()
                .any(|name| name == TOUR_WORKER)
                && !running.iter().any(|name| name == TOUR_WORKER),
            json!({"containers": declared_containers(&status), "workers": running}),
        ));

        let served = project(&live_tour);
        observations.push(binary(
            "tour.current",
            &served == tour(),
            json!({"served": served}),
        ));

        Ok(vec![CapturedDeliverable {
            id: EVIDENCE_ID.into(),
            kind: "application_audit".into(),
            content: json!({
                "observations": observations,
                "tour": {"fixture": tour(), "served": live_tour},
                "closed": closed,
                "errors": errors,
                "baseline": baseline,
                "final": {"compose": status, "workers": workers, "registered_triggers": registered},
            })
            .into(),
            invariants: vec![],
            provenance: vec![ProvenanceEvidence {
                kind: "engine_state".into(),
                source_id: format!("onboarding:{run_id}"),
                relation: "validated_before_cleanup".into(),
            }],
        }])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        _run_id: &str,
    ) -> Result<ObjectiveEvaluation> {
        let evidence = observation
            .deliverables
            .iter()
            .find(|deliverable| deliverable.id == EVIDENCE_ID)
            .and_then(|deliverable| deliverable.content.as_json())
            .context("onboarding evidence deliverable was not captured")?;
        Ok(ObjectiveEvaluation {
            completion: None,
            awards: atomic_awards(metrics(), evidence)?,
            infrastructure_error: None,
        })
    }

    async fn cleanup(&self, _context: &E2eContext, run_id: &str) -> Result<()> {
        if let Some(run) = lock_unpoisoned(runs()).remove(run_id) {
            for trigger in run.triggers {
                trigger.unregister();
            }
            for function in run.functions {
                function.unregister();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialogue_is_the_tour_prompts_in_order() {
        let prompts = prompts();
        assert_eq!(prompts.len(), 6);
        assert_eq!(prompts[0], "hello");
        assert!(prompts[5].starts_with("Remove the onboarding worker"));
        assert_eq!(exchange_of("discoverability"), Some(2));
        // The tour does not name a profile yet; the test fixes its own.
        assert_eq!(tour().agent, None);
    }

    #[test]
    fn the_served_tour_projects_to_the_fixture_shape() {
        let live = json!({
            "id": "console-basics",
            "agent": PROFILE,
            "steps": [
                {"id": "message", "title": "Send a message", "ask": {"text": "hello", "label": "Say hi"},
                 "condition": {"type": "harness::turn-completed", "config": {}, "label": "Waiting"}},
                {"id": "tabs", "title": "Workspaces", "anchors": [".onboarding-tabs"]},
            ],
        });
        let served = project(&live);
        assert_eq!(served.steps[0], tour().steps[0]);
        assert_eq!(served.steps[1], tour().steps[1]);
        assert_ne!(&served, tour());
    }

    #[test]
    fn exchanges_split_on_runner_messages_not_notifications() {
        let say = |role: &str, text: &str| json!({"message": {"role": role, "content": [{"type": "text", "text": text}]}});
        let transcript = json!({"messages": [
            say("user", "hello"), say("assistant", "hi"),
            say("user", "list workers"), say("assistant", "state, database"),
            say("user", "[notification] wake"), say("assistant", "and http"),
            say("user", "build"), say("assistant", "done"),
        ]});
        let texts = exchange_texts(&transcript);
        assert_eq!(texts.len(), 3);
        assert!(texts[1].contains("database") && texts[1].contains("http"));
    }

    #[test]
    fn agent_chosen_names_are_used_only_when_plain() {
        assert_eq!(identifier(Some("todos")).as_deref(), Some("todos"));
        assert_eq!(
            identifier(Some("todo_items2")).as_deref(),
            Some("todo_items2")
        );
        assert_eq!(identifier(Some("todos; DROP TABLE x")), None);
        assert_eq!(identifier(Some("1todos")), None);
        assert!(
            is_done_column("completed") && is_done_column("is_done") && !is_done_column("title")
        );
        assert!(!is_done_column("completed_at") && !is_done_column("done_time"));
        assert!(
            is_open(&json!(0))
                && is_open(&json!(false))
                && !is_open(&json!(1))
                && !is_open(&json!(true))
        );
    }

    #[test]
    fn the_todo_worker_is_the_new_one_with_a_page() {
        let baseline = json!({"workers": {"workers": [{"name": "state", "status": "connected"}]}});
        let workers = json!({"workers": [
            {"name": "state", "status": "connected", "function_count": 9},
            {"name": "database", "status": "connected", "function_count": 30},
            {"name": "helper", "status": "connected", "function_count": 12},
            {"name": "todo-app", "status": "connected", "function_count": 6},
        ]});
        let registered = json!({"registered_triggers": [
            {"trigger_type": "console:script", "worker_name": "todo-app"},
        ]});
        assert_eq!(
            todo_worker(&baseline, &workers, &registered).as_deref(),
            Some("todo-app")
        );
    }
}
