//! The onboarding tour a new user takes after `iii project init --learn-iii`,
//! as one scripted dialogue under the `harness` template's Onboarding profile.
//!
//! The `onboarding` stack is that project: the `harness` template plus the
//! tour's worker, which serves the `learn-iii` tour. The dialogue sends each
//! step's prompt as the tour card's button does, in tour order, and this
//! module plays the card itself: `setup` closes the button-only steps as their
//! buttons would (`stay-in-touch` is skipped, never subscribed), binds every
//! other step's condition the way the card binds it, and closes that step with
//! `learn-iii::steps::complete` when its trigger fires. `capture` then checks
//! what each step should have left behind, and adds two expenses through the
//! agent's tracker as the user would. The objectives, gates marked:
//!
//! - Tour: every step closed (gate); the tour served is the fixture's.
//! - Composability: the `database` worker runs (gate).
//! - Discoverability: the answer says what `database`, `state` and
//!   `harness` can do, in words of the functions they registered.
//! - Extensibility: a new worker with at least two functions (gate), its
//!   Console page, an expense's four fields, and a total still to be
//!   reimbursed that counts the unreimbursed expense and not the other.
//! - Reactivity: the tracker's own trigger type, that trigger firing for
//!   each expense the runner adds (gate), and a trigger the chat bound on it
//!   (gate).
//! - Clean up: the `onboarding` worker gone (gate).
//!
//! `tests/fixtures/onboarding-tour/learn-iii.json` is the tour this test was
//! written against: its steps, prompts, conditions and profile, projected from
//! `learn-iii::tours::get`. A tour the worker serves differently costs
//! `tour.current` and nothing else; when the tour changes on purpose,
//! regenerate the fixture from the worker's `src/tours.mjs` with
//! `scripts/onboarding_tour_fixture.mjs`.
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
const TOUR_ID: &str = "learn-iii";
/// The profile the test fixes: "Onboarding" in the `harness` template, hidden
/// from the new-chat gallery. The tour's first step starts its chat under it,
/// and every later prompt goes to that chat.
const PROFILE: &str = "ade-worker-builder";
const TOUR_WORKER: &str = "onboarding";
/// Measurement, not part of the system the agent is asked to describe.
const RUNNER_WORKER: &str = "harness-e2e";
const FIXTURE: &str = include_str!("../../tests/fixtures/onboarding-tour/learn-iii.json");
/// The workers the Discoverability prompt asks about.
const DISCOVERED: [&str; 3] = ["database", "state", "harness"];
/// How long an added expense may take to ring the tracker's trigger.
const FIRE_WAIT: Duration = Duration::from_secs(10);
/// How long the user waits on a step the agent is still working on before
/// clicking on to the next one.
const STEP_WAIT: Duration = Duration::from_secs(20 * 60);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Tour {
    id: String,
    steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Step {
    id: String,
    /// The prompt the step's button sends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ask: Option<String>,
    /// The profile whose new chat the button sends it to; without one it goes
    /// to the chat the user is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
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
        steps: live["steps"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|step| Step {
                id: text(&step["id"]).unwrap_or_default(),
                ask: text(&step["ask"]["text"]),
                agent: text(&step["ask"]["agent"]),
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

/// The step whose prompt opens an exchange.
fn step_of(exchange: usize) -> Option<&'static Step> {
    tour()
        .steps
        .iter()
        .filter(|step| step.ask.is_some())
        .nth(exchange)
}

/// Whether the agent is still on its last prompt: a turn running or queued,
/// or a wake it armed still to come.
fn still_working(status: &Value) -> (bool, bool) {
    let running = matches!(
        status["status"].as_str(),
        Some("running" | "awaiting_functions")
    ) || items(status, "queued").next().is_some();
    (running, status["expects_wake"] == true)
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
        label: "Every step of the tour closed",
        question: "When the dialogue ends, does learn-iii::progress::get show all ten steps of the tour complete?",
    },
    Metric {
        id: "composability.database",
        weight: 8,
        ratio: false,
        gate: Some(8),
        label: "The database worker is running",
        question: "Is a `database` worker running in the project: a ready container or a connected worker?",
    },
    Metric {
        id: "discoverability.functions",
        weight: 8,
        ratio: true,
        gate: None,
        label: "The answer says what each worker can do",
        question: "For each of `database`, `state` and `harness`, does a line of the Discoverability answer that names the worker describe something it can do: a word starting as one of the functions it had registered does (`query` for queries, `update` for updates)?",
    },
    Metric {
        id: "extensibility.worker",
        weight: 12,
        ratio: false,
        gate: Some(12),
        label: "A new worker runs with at least two functions",
        question: "Is a worker the dialogue added, other than the database and the tour, connected with at least two functions? The expense tracker is the new worker whose functions take an expense.",
    },
    Metric {
        id: "extensibility.page",
        weight: 8,
        ratio: false,
        gate: None,
        label: "The tracker has a Console page",
        question: "Does the expense tracker inject a page into the Console (a console:script trigger)?",
    },
    Metric {
        id: "extensibility.fields",
        weight: 4,
        ratio: true,
        gate: None,
        label: "An expense has the four fields asked for",
        question: "Of description, amount, category and whether it was reimbursed, how many does the tracker take for an expense or give back for the one the runner added?",
    },
    Metric {
        id: "extensibility.total",
        weight: 6,
        ratio: false,
        gate: None,
        label: "The total still to be reimbursed is right",
        question: "Does a total the tracker reports grow by the amount of an expense not yet reimbursed, and hold when an expense already reimbursed is added?",
    },
    Metric {
        id: "reactivity.trigger_type",
        weight: 4,
        ratio: false,
        gate: None,
        label: "The tracker registers a trigger type",
        question: "Did the expense tracker register a trigger type of its own?",
    },
    Metric {
        id: "reactivity.fires",
        weight: 14,
        ratio: true,
        gate: Some(14),
        label: "Each new expense fires the tracker's trigger",
        question: "When the runner adds each of two expenses through the tracker, as its page would, does one of the tracker's trigger types fire within ten seconds?",
    },
    Metric {
        id: "reactivity.chat_trigger",
        weight: 6,
        ratio: false,
        gate: Some(6),
        label: "The chat bound a trigger on new expenses",
        question: "Did the chat register a trigger on one of the tracker's trigger types (an accepted engine::register_trigger in the transcript), or is such a binding, by anything but the tracker, still live?",
    },
    Metric {
        id: "cleanup.removed",
        weight: 8,
        ratio: false,
        gate: Some(8),
        label: "The onboarding worker is gone",
        question: "After Clean up, is the `onboarding` container out of the project and its worker disconnected?",
    },
    Metric {
        id: "tour.current",
        weight: 2,
        ratio: false,
        gate: None,
        label: "The tour is the one this test knows",
        question: "Does the worker serve the tour in tests/fixtures/onboarding-tour/learn-iii.json: same steps, prompts, conditions and profile?",
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

/// What the card records while the dialogue runs: the steps it closed, the
/// tour's progress after each, and the functions the workers the agent was
/// asked about had when it answered.
#[derive(Default, Serialize)]
struct Observed {
    closed: Vec<Value>,
    #[serde(skip)]
    closed_ids: HashSet<String>,
    progress: Option<Value>,
    discoverability_functions: Option<Value>,
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

/// Close one step as the card does, then snapshot the tour's progress.
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
    let closed = call(client, "learn-iii::steps::complete", request).await;
    let progress = call(client, "learn-iii::progress::get", json!({})).await;
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
        let functions = call(
            client,
            "engine::functions::list",
            json!({"workers": DISCOVERED}),
        )
        .await;
        let mut observed = lock_unpoisoned(observed);
        match functions {
            Ok(functions) => observed.discoverability_functions = Some(functions),
            Err(error) => observed
                .errors
                .push(format!("functions at discoverability: {error:#}")),
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
        .description("The onboarding tour card's condition for one step, played by the E2E runner.")
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

/// The workers the dialogue added: connected now, not there before, and
/// neither the database, the tour nor the runner. (name, function count)
fn new_workers(baseline: &Value, workers: &Value) -> Vec<(String, u64)> {
    let before = connected_workers(&baseline["workers"]);
    items(workers, "workers")
        .filter(|worker| worker["status"] == "connected")
        .filter_map(|worker| {
            let name = worker["name"].as_str()?;
            (!before.iter().any(|known| known == name)
                && ![TOUR_WORKER, "database", RUNNER_WORKER].contains(&name))
            .then(|| {
                (
                    name.to_string(),
                    worker["function_count"].as_u64().unwrap_or(0),
                )
            })
        })
        .collect()
}

/// The expense tracker among the new workers, each with its described
/// functions: the one whose functions take an expense best, then the one
/// with a Console page, then the one with the most functions.
fn tracker_of<'a>(
    candidates: &'a [(String, u64, Value)],
    registered: &Value,
) -> Option<&'a (String, u64, Value)> {
    let with_page: HashSet<&str> = items(registered, "registered_triggers")
        .filter(|trigger| trigger["trigger_type"] == "console:script")
        .filter_map(|trigger| trigger["worker_name"].as_str())
        .collect();
    candidates
        .iter()
        .max_by_key(|(name, functions, described)| {
            let takes = adder(described).map_or(0, |(_, schema)| 1 + expense_fields(&schema).len());
            (
                takes,
                with_page.contains(name.as_str()),
                *functions,
                name.clone(),
            )
        })
}

fn function_ids(listed: &Value, worker: &str) -> Vec<String> {
    items(listed, "functions")
        .filter(|function| function["worker_name"] == worker)
        .filter_map(|function| function["function_id"].as_str().map(str::to_owned))
        .collect()
}

/// The words of a function's name, its camelCase split too: `listTables`
/// is `list` and `tables`.
fn name_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    for c in name.chars() {
        if (!c.is_ascii_alphanumeric() || c.is_ascii_uppercase()) && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        if c.is_ascii_alphanumeric() {
            word.push(c.to_ascii_lowercase());
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// The functions of `worker` an answer speaks of: on a line that names the
/// worker, a word that starts as a word of the function's name does, four
/// letters or more (`query` in "queries", `update` in "partial updates").
/// The prompt asks what the workers do, not for their ids.
fn named_functions(answer: &str, worker: &str, ids: &[String]) -> Vec<String> {
    let words: HashSet<String> = answer
        .lines()
        .map(str::to_ascii_lowercase)
        .filter(|line| line.contains(worker))
        .flat_map(|line| name_words(&line))
        .collect();
    let prefix = format!("{worker}::");
    ids.iter()
        .filter(|id| {
            name_words(id.strip_prefix(&prefix).unwrap_or(id))
                .iter()
                .filter(|part| part.len() >= 4 && part.as_str() != worker)
                .any(|part| words.iter().any(|word| word.starts_with(&part[..4])))
        })
        .cloned()
        .collect()
}

/// Every function a worker registered, each with its request schema.
async fn described_functions(context: &E2eContext, worker: &str) -> Result<Value> {
    let listed = context
        .trigger_value("engine::functions::list", json!({"worker": worker}))
        .await?;
    let ids: Vec<String> = function_ids(&listed, worker).into_iter().take(32).collect();
    if ids.is_empty() {
        return Ok(json!({"functions": []}));
    }
    context
        .trigger_value("engine::functions::info", json!({"function_ids": ids}))
        .await
}

/// The four things the Extensibility prompt says an expense has, each with
/// the words a property naming it uses, matched in this order.
const FIELDS: [(&str, &[&str]); 4] = [
    (
        "reimbursed",
        &["reimburs", "refund", "repaid", "paid", "settled"],
    ),
    ("amount", &["amount", "cost", "price", "sum", "value"]),
    ("category", &["categor", "kind", "type", "tag", "group"]),
    (
        "description",
        &["desc", "title", "name", "note", "memo", "label"],
    ),
];

/// Which of the four fields each property of a request schema says, by its
/// name: (field, property).
fn expense_fields(schema: &Value) -> Vec<(&'static str, String)> {
    fields_named(
        schema["properties"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, _)| name.as_str()),
    )
}

/// Which of the four fields each name says: (field, name).
fn fields_named<'a>(names: impl Iterator<Item = &'a str>) -> Vec<(&'static str, String)> {
    let mut found: Vec<(&'static str, String)> = Vec::new();
    for property in names {
        let name = property.to_ascii_lowercase();
        let field = FIELDS.iter().find(|(field, words)| {
            !found.iter().any(|(taken, _)| taken == field)
                && words.iter().any(|word| name.contains(word))
        });
        if let Some((field, _)) = field {
            found.push((field, property.to_string()));
        }
    }
    found
}

/// The tracker's function that adds an expense: the one whose request names
/// the most of the four fields, at least two, an add-like name breaking a
/// tie. A tracker that declares no request schema is not held to one: its
/// add-like function is taken by name.
fn adder(described: &Value) -> Option<(String, Value)> {
    const ADDS: [&str; 7] = ["add", "create", "insert", "record", "new", "log", "track"];
    const OTHER_WRITES: [&str; 6] = ["update", "delete", "remove", "clear", "reset", "set"];
    items(described, "functions")
        .filter_map(|function| {
            let id = function["function_id"].as_str()?.to_string();
            let schema = function["request_schema"].clone();
            let fields = expense_fields(&schema).len();
            let name = id.rsplit("::").next().unwrap_or(&id).to_ascii_lowercase();
            let add_like = ADDS.iter().any(|word| name.contains(word))
                && !OTHER_WRITES.iter().any(|word| name.contains(word));
            let undeclared = schema["properties"]
                .as_object()
                .is_none_or(|properties| properties.is_empty());
            (fields >= 2 || (add_like && undeclared)).then_some((
                (fields, add_like, id.contains("expense"), id.clone()),
                id,
                schema,
            ))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, id, schema)| (id, schema))
}

/// The tracker's functions a page could read a total from: callable with no
/// arguments, named for reading and not for writing, a `total` first.
fn readers(described: &Value, adder: Option<&str>) -> Vec<String> {
    const READS: [&str; 8] = [
        "total",
        "outstanding",
        "summary",
        "balance",
        "owed",
        "stats",
        "list",
        "get",
    ];
    const WRITES: [&str; 9] = [
        "delete", "remove", "clear", "reset", "add", "create", "update", "set", "insert",
    ];
    let mut found: Vec<String> = items(described, "functions")
        .filter(|function| {
            items(&function["request_schema"], "required")
                .next()
                .is_none()
        })
        .filter_map(|function| function["function_id"].as_str())
        .filter(|id| Some(*id) != adder)
        .filter(|id| {
            let name = id.rsplit("::").next().unwrap_or(id).to_ascii_lowercase();
            READS.iter().any(|word| name.contains(word))
                && !WRITES.iter().any(|word| name.contains(word))
        })
        .map(str::to_owned)
        .collect();
    found.sort_by_key(|id| !id.to_ascii_lowercase().contains("total"));
    found.truncate(4);
    found
}

struct Expense {
    description: &'static str,
    amount: f64,
    category: &'static str,
    reimbursed: bool,
}

/// What the runner adds as the user would: one still to be reimbursed and one
/// already reimbursed, so the total still to be reimbursed moves by the first
/// one's amount alone.
const EXPENSES: [Expense; 2] = [
    Expense {
        description: "E2E check: team lunch",
        amount: 12.5,
        category: "meals",
        reimbursed: false,
    },
    Expense {
        description: "E2E check: train ticket",
        amount: 40.0,
        category: "travel",
        reimbursed: true,
    },
];

/// A plain value for a property the request requires and the four fields do
/// not cover.
fn plain(kind: &str) -> Value {
    match kind {
        "string" => json!("e2e"),
        "integer" | "number" => json!(1),
        "boolean" => json!(false),
        "array" => json!([]),
        "object" => json!({}),
        _ => Value::Null,
    }
}

fn reimbursed_value(property: &Value, reimbursed: bool) -> Value {
    if property["type"] != "string" {
        return json!(reimbursed);
    }
    let says_yes = |text: &str| {
        let text = text.to_ascii_lowercase();
        !text.starts_with("un")
            && !text.starts_with("not")
            && ["reimburs", "paid", "yes", "true", "done", "settled"]
                .iter()
                .any(|word| text.contains(word))
    };
    items(property, "enum")
        .filter_map(Value::as_str)
        .find(|value| says_yes(value) == reimbursed)
        .map(|value| json!(value))
        .unwrap_or_else(|| json!(reimbursed.to_string()))
}

/// An expense as the tracker's page would send it, and the amount it carried:
/// the four fields where the schema has them, in cents where the amount is a
/// whole number, and a plain value for anything else the schema requires.
fn expense_payload(schema: &Value, expense: &Expense) -> (Value, Option<f64>) {
    if schema["properties"]
        .as_object()
        .is_none_or(|properties| properties.is_empty())
    {
        // Nothing declared: the four fields under the prompt's own words.
        return (
            json!({
                "description": expense.description,
                "amount": expense.amount,
                "category": expense.category,
                "reimbursed": expense.reimbursed,
            }),
            Some(expense.amount),
        );
    }
    let fields = expense_fields(schema);
    let required: Vec<&str> = items(schema, "required")
        .filter_map(Value::as_str)
        .collect();
    let mut payload = serde_json::Map::new();
    let mut sent = None;
    for (property, spec) in schema["properties"].as_object().into_iter().flatten() {
        let kind = spec["type"].as_str().unwrap_or("");
        let field = fields
            .iter()
            .find(|(_, name)| name == property)
            .map(|(field, _)| *field);
        let value = match field {
            Some("description") => json!(expense.description),
            Some("category") => json!(expense.category),
            Some("amount") => {
                let amount = if kind == "integer" {
                    (expense.amount * 100.0).round()
                } else {
                    expense.amount
                };
                sent = Some(amount);
                match kind {
                    "integer" => json!(amount as i64),
                    "string" => json!(amount.to_string()),
                    _ => json!(amount),
                }
            }
            Some(_) => reimbursed_value(spec, expense.reimbursed),
            None if required.contains(&property.as_str()) => plain(kind),
            None => continue,
        };
        payload.insert(property.clone(), value);
    }
    (Value::Object(payload), sent)
}

/// Every number in an answer by its path, numeric strings included; array
/// elements carry `[index]`.
fn numbers(value: &Value, path: &str, out: &mut HashMap<String, f64>) {
    match value {
        Value::Number(number) => {
            if let Some(number) = number.as_f64() {
                out.insert(path.to_string(), number);
            }
        }
        Value::String(text) => {
            if let Ok(number) = text.trim().parse::<f64>() {
                out.insert(path.to_string(), number);
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                numbers(value, &format!("{path}[{index}]"), out);
            }
        }
        Value::Object(fields) => {
            for (name, value) in fields {
                numbers(value, &format!("{path}/{name}"), out);
            }
        }
        _ => {}
    }
}

/// The paths whose number grew by `delta` from one answer to the next, in the
/// unit it was sent or a hundred times either way (an amount in units kept in
/// cents, or the reverse). A field the first answer did not have counts from
/// zero, an array element never: a new expense in a list is not a total.
fn moved_by(before: &Value, after: &Value, delta: f64) -> Vec<String> {
    let (mut old, mut new) = (HashMap::new(), HashMap::new());
    numbers(before, "", &mut old);
    numbers(after, "", &mut new);
    let mut moved: Vec<String> = new
        .into_iter()
        .filter(|(path, value)| {
            let start = old
                .get(path)
                .copied()
                .or_else(|| (!path.contains('[')).then_some(0.0));
            start.is_some_and(|start| {
                [delta, delta * 100.0, delta / 100.0]
                    .iter()
                    .any(|delta| (value - start - delta).abs() < 1e-6)
            })
        })
        .map(|(path, _)| path)
        .collect();
    moved.sort();
    moved
}

/// The paths whose number is the same in both answers.
fn held(before: &Value, after: &Value) -> HashSet<String> {
    let (mut old, mut new) = (HashMap::new(), HashMap::new());
    numbers(before, "", &mut old);
    numbers(after, "", &mut new);
    new.into_iter()
        .filter(|(path, value)| old.get(path).is_some_and(|old| (value - old).abs() < 1e-6))
        .map(|(path, _)| path)
        .collect()
}

/// The keys of the object in an answer that holds this description: the
/// expense as the tracker kept it.
fn record_keys(value: &Value, description: &str) -> Option<Vec<String>> {
    match value {
        Value::Object(fields) => {
            if fields
                .values()
                .any(|field| field.as_str() == Some(description))
            {
                return Some(fields.keys().cloned().collect());
            }
            fields
                .values()
                .find_map(|field| record_keys(field, description))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|value| record_keys(value, description)),
        _ => None,
    }
}

/// The triggers the chat bound on these types, from the transcript: an
/// `engine::register_trigger` call the engine accepted. A wake armed `once`
/// is gone once it fires, so the live bindings alone miss it.
fn chat_bindings(transcript: &Value, types: &[String]) -> Vec<Value> {
    let accepted: HashSet<&str> = items(transcript, "messages")
        .map(|entry| &entry["message"])
        .filter(|message| {
            message["role"] == "function_result"
                && message["function_id"] == "engine::register_trigger"
                && message["is_error"] != true
        })
        .filter_map(|message| message["function_call_id"].as_str())
        .collect();
    items(transcript, "messages")
        .flat_map(|entry| items(&entry["message"], "content"))
        .filter(|block| block["type"] == "function_call")
        .filter_map(|block| {
            let arguments = &block["arguments"];
            // Through the Harness's `agent_trigger`, or called directly.
            let (function, payload) = if block["function_id"] == "agent_trigger" {
                (&arguments["function"], &arguments["payload"])
            } else {
                (&block["function_id"], arguments)
            };
            let kind = payload["trigger_type"].as_str()?;
            (function == "engine::register_trigger"
                && types.iter().any(|own| own == kind)
                && block["id"].as_str().is_some_and(|id| accepted.contains(id)))
            .then(|| {
                json!({
                    "call": block["id"],
                    "trigger_type": kind,
                    "config": payload["config"],
                    "once": payload["once"],
                })
            })
        })
        .collect()
}

async fn read_all(context: &E2eContext, readers: &[String]) -> Vec<Option<Value>> {
    let mut answers = Vec::new();
    for reader in readers {
        answers.push(context.trigger_value(reader, json!({})).await.ok());
    }
    answers
}

/// What adding the two expenses showed.
struct Added {
    /// How many of them the tracker's own trigger answered.
    fired: usize,
    /// The totals that grew by the first one's amount and held for the second.
    total: Vec<String>,
    /// The names the tracker took or gave back for the first one.
    names: Vec<String>,
    evidence: Value,
}

/// Add the two expenses through the tracker as its page would, one at a
/// time: listen for the tracker's trigger after each, and read its totals
/// before, between and after, so each total is read seconds from the add it
/// answers rather than after whatever the chat's trigger set off.
async fn add_expenses(
    context: &E2eContext,
    run_id: &str,
    adder: &(String, Value),
    readers: &[String],
    trigger_types: &[String],
) -> Added {
    let heard = Arc::new(Mutex::new(Vec::<u128>::new()));
    let function_id = format!("e2e::onboarding::{}::expense-added", &digest(run_id)[..12]);
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
        .description("Hears the expense tracker's trigger while the E2E runner adds expenses.")
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
                tracing::warn!(%trigger_type, %error, "could not bind the tracker's trigger")
            }
        }
    }
    let mut totals = vec![read_all(context, readers).await];
    let mut added = Vec::new();
    let mut answers = Vec::new();
    let mut fired = 0;
    let mut owed = None;
    for expense in &EXPENSES {
        let (payload, sent) = expense_payload(&adder.1, expense);
        if !expense.reimbursed {
            owed = sent;
        }
        let before = lock_unpoisoned(&heard).len();
        let answer = context.trigger_value(&adder.0, payload.clone()).await;
        let deadline = tokio::time::Instant::now() + FIRE_WAIT;
        while answer.is_ok()
            && lock_unpoisoned(&heard).len() == before
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let answered = lock_unpoisoned(&heard).len() > before;
        fired += usize::from(answered);
        totals.push(read_all(context, readers).await);
        added.push(json!({
            "payload": payload,
            "added": answer.is_ok(),
            "error": answer.as_ref().err().map(|error| format!("{error:#}")),
            "fired": answered,
        }));
        answers.push(answer.ok());
    }
    for binding in bindings {
        binding.unregister();
    }
    function.unregister();

    let mut total = Vec::new();
    for (index, reader) in readers.iter().enumerate() {
        let read = |at: usize| totals.get(at).and_then(|answers| answers[index].as_ref());
        if let (Some(start), Some(owing), Some(end), Some(owed)) = (read(0), read(1), read(2), owed)
        {
            let unmoved = held(owing, end);
            total.extend(
                moved_by(start, owing, owed)
                    .into_iter()
                    .filter(|path| unmoved.contains(path))
                    .map(|path| format!("{reader}{path}")),
            );
        }
    }
    // The first expense as the tracker kept it: in its answer to the add, or
    // in what a reader gives back after.
    let description = EXPENSES[0].description;
    let kept = answers
        .first()
        .and_then(|answer| answer.as_ref())
        .and_then(|answer| record_keys(answer, description))
        .or_else(|| {
            totals
                .last()
                .into_iter()
                .flatten()
                .flatten()
                .find_map(|answer| record_keys(answer, description))
        })
        .unwrap_or_default();
    let readings: Vec<Value> = readers
        .iter()
        .enumerate()
        .map(|(index, reader)| {
            json!({
                "function": reader,
                "before": totals[0][index],
                "after_owed": totals.get(1).map(|answers| &answers[index]),
                "after_reimbursed": totals.get(2).map(|answers| &answers[index]),
            })
        })
        .collect();
    Added {
        fired,
        total,
        names: kept,
        evidence: json!({"function": adder.0, "added": added, "owed": owed, "totals": readings}),
    }
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
        Some("On the project `iii project init --learn-iii` creates, take the learn-iii tour step by step as a new user does: say hey, add the database, ask what three workers do, build an expense tracker, make it fire a trigger for each new expense with one in the chat waiting on it, then remove the tour.")
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
            // The card closes the steps and the user signs up, never the agent.
            denied_functions: &["learn-iii::steps::*", "learn-iii::subscribe", "approval::*"],
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

    /// The card moves on when a step closes. While the agent is still on it
    /// (a turn running, or a wake it armed still to come), the user waits,
    /// as the card's "Waiting for the agent to report this step done" asks;
    /// once the agent is idle the next button is clicked, closed or not.
    async fn before_followup(
        &self,
        context: &E2eContext,
        run_id: &str,
        session_id: &str,
        exchange: usize,
    ) {
        let Some(step) = step_of(exchange).filter(|step| step.condition.is_some()) else {
            return;
        };
        let Some(observed) = lock_unpoisoned(runs())
            .get(run_id)
            .map(|run| run.observed.clone())
        else {
            return;
        };
        let deadline = tokio::time::Instant::now() + STEP_WAIT;
        loop {
            let closed = lock_unpoisoned(&observed).closed_ids.contains(&step.id);
            let status = context
                .trigger_value("harness::status", json!({"session_id": session_id}))
                .await;
            let (running, waking) = match &status {
                Ok(status) => still_working(status),
                // Unread, it is not held against the agent: the user moves on.
                Err(_) => (false, false),
            };
            if !running && (closed || !waking) {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(step = %step.id, "the agent was still on the step when the wait ran out");
                return;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
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
            .trigger_value("learn-iii::tours::get", json!({"id": TOUR_ID}))
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
        let (progress, closed, discoverability_functions, errors) = {
            let observed = lock_unpoisoned(&observed);
            (
                observed.progress.clone(),
                observed.closed.clone(),
                observed.discoverability_functions.clone(),
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
        let snapshot = discoverability_functions.is_some();
        let functions_then = match discoverability_functions {
            Some(functions) => functions,
            None => context
                .trigger_value("engine::functions::list", json!({"workers": DISCOVERED}))
                .await
                .unwrap_or_default(),
        };
        let named: Vec<Value> = DISCOVERED
            .iter()
            .map(|worker| {
                let ids = function_ids(&functions_then, worker);
                json!({"worker": worker, "named": named_functions(&answer, worker, &ids)})
            })
            .collect();
        observations.push(ratio(
            "discoverability.functions",
            named
                .iter()
                .filter(|worker| {
                    worker["named"]
                        .as_array()
                        .is_some_and(|named| !named.is_empty())
                })
                .count(),
            DISCOVERED.len(),
            json!({"workers": named, "snapshot": snapshot}),
        ));

        let mut candidates = Vec::new();
        for (name, functions) in new_workers(&baseline, &workers).into_iter().take(4) {
            let described = described_functions(context, &name)
                .await
                .unwrap_or_else(|error| json!({"error": format!("{error:#}")}));
            candidates.push((name, functions, described));
        }
        let tracker = tracker_of(&candidates, &registered);
        let tracker_name = tracker.map(|(name, _, _)| name.clone());
        let by_tracker = |trigger: &&Value| {
            tracker_name.is_some() && trigger["worker_name"].as_str() == tracker_name.as_deref()
        };
        let function_count = tracker.map_or(0, |(_, functions, _)| *functions);
        observations.push(binary(
            "extensibility.worker",
            function_count >= 2,
            json!({
                "worker": tracker_name,
                "function_count": function_count,
                "new_workers": candidates.iter().map(|(name, functions, _)| json!({"name": name, "functions": functions})).collect::<Vec<_>>(),
            }),
        ));
        let page = items(&registered, "registered_triggers")
            .filter(by_tracker)
            .any(|trigger| trigger["trigger_type"] == "console:script");
        observations.push(binary(
            "extensibility.page",
            page,
            json!({"worker": tracker_name}),
        ));

        let described = tracker.map_or(json!({"functions": []}), |(_, _, described)| {
            described.clone()
        });
        let adds = adder(&described);
        let own_types: Vec<String> = items(&trigger_types, "triggers")
            .filter(by_tracker)
            .filter_map(|trigger| trigger["id"].as_str().map(str::to_owned))
            .collect();
        // Bound before this check binds its own listener on the same types.
        let waiting: Vec<Value> = items(&registered, "registered_triggers")
            .filter(|trigger| {
                trigger["trigger_type"]
                    .as_str()
                    .is_some_and(|kind| own_types.iter().any(|own| own == kind))
                    && trigger["worker_name"].as_str() != tracker_name.as_deref()
            })
            .cloned()
            .collect();
        let added = match &adds {
            Some(adds) => {
                let readers = readers(&described, Some(adds.0.as_str()));
                add_expenses(context, run_id, adds, &readers, &own_types).await
            }
            None => Added {
                fired: 0,
                total: Vec::new(),
                names: Vec::new(),
                evidence: json!({"error": "the tracker has no function that adds an expense", "described": described}),
            },
        };

        // What the tracker takes for an expense, and what it gave back for the
        // one the runner added.
        let declared = adds
            .as_ref()
            .map(|(_, schema)| expense_fields(schema))
            .unwrap_or_default();
        let kept = fields_named(added.names.iter().map(String::as_str));
        let mut fields: Vec<&str> = declared
            .iter()
            .chain(&kept)
            .map(|(field, _)| *field)
            .collect();
        fields.sort_unstable();
        fields.dedup();
        observations.push(ratio(
            "extensibility.fields",
            fields.len(),
            FIELDS.len(),
            json!({
                "function": adds.as_ref().map(|(id, _)| id),
                "fields": fields,
                "declared": declared.iter().map(|(field, name)| json!({"field": field, "name": name})).collect::<Vec<_>>(),
                "kept": kept.iter().map(|(field, name)| json!({"field": field, "name": name})).collect::<Vec<_>>(),
            }),
        ));
        observations.push(binary(
            "extensibility.total",
            !added.total.is_empty(),
            json!({"totals": added.total, "expenses": added.evidence}),
        ));
        observations.push(binary(
            "reactivity.trigger_type",
            !own_types.is_empty(),
            json!({"trigger_types": own_types}),
        ));
        observations.push(ratio(
            "reactivity.fires",
            added.fired,
            EXPENSES.len(),
            json!({"trigger_types": own_types, "expenses": added.evidence}),
        ));
        let from_chat = chat_bindings(&observation.transcript, &own_types);
        observations.push(binary(
            "reactivity.chat_trigger",
            !waiting.is_empty() || !from_chat.is_empty(),
            json!({"bindings": waiting, "from_chat": from_chat}),
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
        assert_eq!(prompts[0], "hey");
        assert!(prompts[5].starts_with("Remove the onboarding worker"));
        assert_eq!(exchange_of("discoverability"), Some(2));
        // The tour starts its chat under the profile the test fixes.
        assert_eq!(tour().steps[0].agent.as_deref(), Some(PROFILE));
        let asked = tour()
            .steps
            .iter()
            .find(|step| step.id == "discoverability");
        let asked = asked
            .and_then(|step| step.ask.as_deref())
            .unwrap_or_default();
        assert!(DISCOVERED.iter().all(|worker| asked.contains(worker)));
    }

    #[test]
    fn the_served_tour_projects_to_the_fixture_shape() {
        let live = json!({
            "id": "learn-iii",
            "title": "Find your way around",
            "steps": [
                {"id": "message", "title": "Send a message",
                 "ask": {"text": "hey", "label": "Send a message", "agent": PROFILE},
                 "condition": {"type": "harness::turn-completed", "config": {}, "label": "Waiting"}},
                {"id": "tabs", "title": "Workspaces", "anchors": [".learn-iii-tabs"]},
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
    fn the_tracker_is_the_new_worker_whose_functions_take_an_expense() {
        let baseline = json!({"workers": {"workers": [{"name": "state", "status": "connected"}]}});
        let workers = json!({"workers": [
            {"name": "state", "status": "connected", "function_count": 9},
            {"name": "database", "status": "connected", "function_count": 30},
            {"name": "helper", "status": "connected", "function_count": 12},
            {"name": "expenses", "status": "connected", "function_count": 4},
        ]});
        let new: Vec<String> = new_workers(&baseline, &workers)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(new, ["helper", "expenses"]);
        // The helper has more functions and a page, but takes no expense.
        let registered = json!({"registered_triggers": [
            {"trigger_type": "console:script", "worker_name": "helper"},
        ]});
        let candidates = vec![
            (
                "helper".to_string(),
                12,
                json!({"functions": [{"function_id": "helper::ping"}]}),
            ),
            ("expenses".to_string(), 4, described()),
        ];
        assert_eq!(
            tracker_of(&candidates, &registered).map(|c| c.0.as_str()),
            Some("expenses")
        );
    }

    #[test]
    fn a_tracker_that_declares_no_schema_is_added_to_by_name() {
        let described = json!({"functions": [
            {"function_id": "expenses::list"},
            {"function_id": "expenses::set-reimbursed"},
            {"function_id": "expenses::create-expense"},
        ]});
        let (id, schema) = adder(&described).expect("an add-like function");
        assert_eq!(id, "expenses::create-expense");
        let (payload, sent) = expense_payload(&schema, &EXPENSES[0]);
        assert_eq!(payload["description"], "E2E check: team lunch");
        assert_eq!(payload["reimbursed"], false);
        assert_eq!(sent, Some(12.5));
    }

    #[test]
    fn an_answer_speaks_of_functions_on_the_worker_s_own_line() {
        let ids = [
            "database::query".to_string(),
            "database::listTables".to_string(),
            "database::get".to_string(),
            "state::update".to_string(),
            "harness::status".to_string(),
        ];
        // The local run's answer, in its table rows; each worker is held to
        // its own functions, as `capture` holds it.
        let answer = "| **database** | SQL access: queries, writes, transactions, schema inspection. |\n\
                      | **state** | JSON values by scope and key: reads, writes, partial updates. |\n\
                      | **harness** | The agent's turn loop: tracks turn status and usage. |";
        let own = |worker: &str| -> Vec<String> {
            ids.iter()
                .filter(|id| id.starts_with(&format!("{worker}::")))
                .cloned()
                .collect()
        };
        assert_eq!(
            named_functions(answer, "database", &own("database")),
            ["database::query"]
        );
        assert_eq!(
            named_functions(answer, "state", &own("state")),
            ["state::update"]
        );
        assert_eq!(
            named_functions(answer, "harness", &own("harness")),
            ["harness::status"]
        );
        // Three letters are not a word of the name: `get` is not "gets".
        assert!(named_functions("database gets rows", "database", &own("database")).is_empty());
        assert_eq!(name_words("listTables"), ["list", "tables"]);
    }

    #[test]
    fn the_expense_as_kept_is_found_by_its_description() {
        let answer = json!({"expense": {"id": "e1", "description": "E2E check: team lunch", "amount_cents": 1250, "category": "meals", "reimbursed": false}});
        let keys = record_keys(&answer, "E2E check: team lunch").expect("the record");
        let fields: Vec<&str> = fields_named(keys.iter().map(String::as_str))
            .into_iter()
            .map(|(field, _)| field)
            .collect();
        assert_eq!(fields.len(), 4);
    }

    fn described() -> Value {
        json!({"functions": [
            {"function_id": "expenses::list", "request_schema": {"type": "object", "properties": {}}},
            {"function_id": "expenses::total-unreimbursed", "request_schema": null},
            {"function_id": "expenses::update", "request_schema": {"type": "object",
                "properties": {"id": {"type": "string"}, "reimbursed": {"type": "boolean"}, "amount": {"type": "number"}},
                "required": ["id"]}},
            {"function_id": "expenses::add", "request_schema": {"type": "object",
                "properties": {
                    "description": {"type": "string"}, "amount_cents": {"type": "integer"},
                    "category": {"type": "string", "enum": ["meals", "travel"]},
                    "status": {"type": "string", "enum": ["pending", "reimbursed"]},
                    "currency": {"type": "string"}, "note": {"type": "string"},
                },
                "required": ["description", "amount_cents", "currency"]}},
            {"function_id": "expenses::clear", "request_schema": {"type": "object", "properties": {}}},
        ]})
    }

    #[test]
    fn the_add_function_is_the_one_that_takes_an_expense() {
        let described = described();
        let (id, schema) = adder(&described).expect("an add function");
        assert_eq!(id, "expenses::add");
        // `status` says nothing reimbursed by name; `note` comes second to `description`.
        let fields: Vec<&str> = expense_fields(&schema)
            .into_iter()
            .map(|(field, _)| field)
            .collect();
        assert_eq!(fields.len(), 3);
        assert!(!fields.contains(&"reimbursed"));
        assert_eq!(
            readers(&described, Some(&id)),
            ["expenses::total-unreimbursed", "expenses::list"]
        );
    }

    #[test]
    fn an_expense_is_sent_in_the_shape_its_schema_asks_for() {
        let schema = json!({"type": "object",
            "properties": {
                "title": {"type": "string"}, "amount": {"type": "integer"},
                "reimbursement": {"type": "string", "enum": ["unreimbursed", "reimbursed"]},
                "currency": {"type": "string"}, "receipts": {"type": "array"},
            },
            "required": ["title", "amount", "currency"]});
        let (payload, sent) = expense_payload(&schema, &EXPENSES[0]);
        assert_eq!(
            payload,
            json!({
                "title": "E2E check: team lunch", "amount": 1250,
                "reimbursement": "unreimbursed", "currency": "e2e",
            })
        );
        assert_eq!(sent, Some(1250.0));
        let (payload, _) = expense_payload(&schema, &EXPENSES[1]);
        assert_eq!(payload["reimbursement"], "reimbursed");
    }

    #[test]
    fn the_card_waits_on_a_running_turn_or_an_armed_wake() {
        assert_eq!(
            step_of(3).map(|step| step.id.as_str()),
            Some("extensibility")
        );
        assert_eq!(step_of(5).map(|step| step.id.as_str()), Some("clean-up"));
        assert_eq!(still_working(&json!({"status": "running"})), (true, false));
        assert_eq!(
            still_working(&json!({"status": "completed", "expects_wake": true})),
            (false, true)
        );
        assert_eq!(
            still_working(&json!({"status": "completed", "queued": [{"id": "q"}]})),
            (true, false)
        );
        assert_eq!(
            still_working(&json!({"status": "completed"})),
            (false, false)
        );
    }

    #[test]
    fn a_trigger_the_chat_bound_counts_once_the_engine_accepted_it() {
        let call = |id: &str, kind: &str| {
            json!({"message": {"role": "assistant", "content": [{
                "type": "function_call", "id": id, "function_id": "agent_trigger",
                "arguments": {"function": "engine::register_trigger",
                              "payload": {"trigger_type": kind, "config": {"event": "created"}, "once": true}},
            }]}})
        };
        let result = |id: &str, error: bool| {
            json!({"message": {
                "role": "function_result", "function_call_id": id,
                "function_id": "engine::register_trigger", "is_error": error,
            }})
        };
        let transcript = json!({"messages": [
            call("a", "expense-tracker:change"), result("a", false),
            call("b", "expense-tracker:change"), result("b", true),
            call("c", "state"), result("c", false),
        ]});
        let bound = chat_bindings(&transcript, &["expense-tracker:change".to_string()]);
        assert_eq!(bound.len(), 1);
        assert_eq!(bound[0]["call"], "a");
    }

    #[test]
    fn a_total_is_a_number_that_grew_by_the_amount_still_owed() {
        let before = json!({"total": "27.50", "expenses": [{"amount": 15}], "count": 1});
        let right = json!({"total": "40.00", "expenses": [{"amount": 12.5}, {"amount": 40}, {"amount": 15}], "count": 3});
        assert_eq!(moved_by(&before, &right, 12.5), ["/total"]);
        // Counting the reimbursed one too, or only a new list entry, is not it.
        let wrong = json!({"total": 80.0, "expenses": [{"amount": 12.5}], "count": 3});
        assert!(moved_by(&before, &wrong, 12.5).is_empty());
        // A total the first answer did not have yet starts from zero.
        assert_eq!(
            moved_by(&json!({}), &json!({"owed": 12.5}), 12.5),
            ["/owed"]
        );
        // An amount sent in units and kept in cents, as execution 8951e88f's
        // tracker did.
        let before = json!({"totals": {"unreimbursed_cents": 6140}});
        let after = json!({"totals": {"unreimbursed_cents": 7390}});
        assert_eq!(
            moved_by(&before, &after, 12.5),
            ["/totals/unreimbursed_cents"]
        );
        let all = json!({"totals": {"unreimbursed_cents": 11390}});
        assert!(moved_by(&before, &all, 12.5).is_empty());
        // Adding the reimbursed one after must leave it where it was.
        assert!(held(&after, &after).contains("/totals/unreimbursed_cents"));
        assert!(!held(&after, &all).contains("/totals/unreimbursed_cents"));
    }
}
