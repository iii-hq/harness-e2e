//! `router_fault_contract` — the Harness and the real llm-router against a
//! provider that fails on cue. The runner registers `e2e-fault-<run>` with
//! the router and plays its `provider::<id>::stream`, scripted per side
//! session, so no model token is spent on the faults:
//!
//! - `orphan`: two calls in one step, one to a function that does not exist;
//!   the next request must carry a result for every call;
//! - `prestream`: two pre-stream `rate_limited` errors, then the answer; the
//!   router retries invisibly (three stream calls, one reply, no error or
//!   recovery row); `permanent`: one call and a failed, non-retryable turn;
//! - `midstream`: a fixture call, then a partial cut by a `transient` error;
//!   the Harness resumes (`recovering`, `recovered`), keeps the partial and
//!   runs the fixture once; `exhausted`: every attempt cut, failing after
//!   three resumes;
//! - `stop`: a stream that only pings; `harness::stop` cancels the turn in
//!   under 5 s and the router aborts the provider's `<turn>:<step>` request;
//! - `overflow`: a `context_overflow` error fails as `llm.context_overflow`;
//! - `heal`: a second provider whose stream function goes away fails a turn
//!   and is marked unavailable, then serves again once it re-registers.
//!
//! The same scripted provider drives three Harness limits no model would hit
//! on cue:
//!
//! - `loop`: every step answers with a fixture call under `max_turns` 3; the
//!   turn gets three streams, a `max_turns` notice, and ends `completed`
//!   with stop reason `max_turns`;
//! - `typed`: a typed fixture is called with a boolean and an array written
//!   as strings (repaired before dispatch, the result marked
//!   `origin.reconciled`), with valid arguments (left unmarked), and with a
//!   value no parse fixes (dispatched as written, refused by the fixture, and
//!   answered with the schema violation named by argument);
//! - `fanout`: `max_children` + 1 `harness::spawn` calls in one step; only
//!   the last is refused, as `harness/spawn_fanout_exceeded`, and exactly
//!   `max_children` child sessions exist.
//!
//! The root's one exchange is a plain reply on the subject's model.
//! shortcut: the router has no way to forget a provider, so every run leaves
//! its two `e2e-*-<run>` registrations behind (unavailable once the runner
//! disconnects); run it on a disposable stack until the router can drop one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iii_sdk::channel::{ChannelWriter, StreamChannelRef};
use iii_sdk::runtime::FunctionRef;
use iii_sdk::{IIIClient, RegisterFunction};
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::common::lock_unpoisoned;
use super::mechanism::{self, record};
use super::{
    async_trait, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation, Scenario,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "router_fault_contract";
const DELIVERABLE_ID: &str = "router_fault_checks";
const FAULT_MODEL: &str = "e2e-fault-model";
const HEAL_MODEL: &str = "e2e-heal-model";
const TURN_WAIT: Duration = Duration::from_secs(120);
/// What every fault side session is sent; the fake provider plays the rest.
const CASE_MESSAGE: &str = "Run the scripted exchange.";
const STOP_WITHIN: Duration = Duration::from_secs(5);
/// The `max_turns` the loop case is sent with.
const LOOP_TURNS: usize = 3;
const FANOUT_CODE: &str = "harness/spawn_fanout_exceeded";
/// `harness/src/config.rs` `default_max_children`, for an entry that leaves it out.
const DEFAULT_MAX_CHILDREN: usize = 8;
const CASES: [&str; 10] = [
    "orphan",
    "prestream",
    "permanent",
    "midstream",
    "exhausted",
    "stop",
    "overflow",
    "loop",
    "typed",
    "fanout",
];

const PAIRING: AssessmentSpec = AssessmentSpec::gate(
    "call_pairing",
    10,
    "After a step with a call to a missing function, did the next request carry a result for every call?",
)
.labeled("Every call is answered in the next request");
const PRESTREAM: AssessmentSpec = AssessmentSpec::gate(
    "prestream_retry",
    15,
    "Were two pre-stream rate limits retried invisibly (three stream calls, one reply, no error or recovery row), and did a permanent error fail after one call as not retryable?",
)
.labeled("Pre-stream errors retry invisibly, permanent ones fail once");
const RESUME: AssessmentSpec = AssessmentSpec::gate(
    "midstream_resume",
    15,
    "Did a stream cut mid-answer resume with recovering and recovered rows, keep the partial and run the earlier call once, and did a stream cut every time fail after three resumes?",
)
.labeled("A cut stream resumes, gives up after three");
const STOP: AssessmentSpec = AssessmentSpec::gate(
    "stop_mid_stream",
    10,
    "Did `harness::stop` cancel a turn whose stream only pinged in under 5 s, with the router aborting the provider's `<turn>:<step>` request?",
)
.labeled("Stop aborts a live stream within 5 s");
const OVERFLOW: AssessmentSpec = AssessmentSpec::gate(
    "provider_overflow",
    5,
    "Did a provider `context_overflow` error fail the turn as `llm.context_overflow`?",
)
.labeled("A provider overflow is classified as such");
const HEAL: AssessmentSpec = AssessmentSpec::gate(
    "provider_heals",
    10,
    "Did a provider whose stream function went away fail a turn and read unavailable, then serve again after re-registering?",
)
.labeled("A provider that comes back is used again");
const TURN_CAP: AssessmentSpec = AssessmentSpec::gate(
    "turn_cap",
    10,
    "Did a provider that answers every step with a call get exactly three streams under `max_turns` 3, with a `max_turns` notice and a turn that ended `completed` with stop reason `max_turns`?",
)
.labeled("The step cap ends a looping turn");
const RECONCILE: AssessmentSpec = AssessmentSpec::gate(
    "argument_reconciliation",
    15,
    "Did a boolean and an array sent as strings reach a typed fixture as a boolean and an array, with the result marked `origin.reconciled`, a valid call left unmarked, and a value no parse can fix refused with the schema violation named by argument?",
)
.labeled("Stringified call arguments are repaired before dispatch");
const FANOUT: AssessmentSpec = AssessmentSpec::gate(
    "spawn_fanout_cap",
    10,
    "With `max_children` + 1 spawns in one step, was only the last refused as `harness/spawn_fanout_exceeded`, leaving exactly `max_children` child sessions?",
)
.labeled("One step spawns at most max_children sessions");
const ASSESSMENTS: &[AssessmentSpec] = &[
    PAIRING, PRESTREAM, RESUME, STOP, OVERFLOW, HEAL, TURN_CAP, RECONCILE, FANOUT,
];

fn fault_provider(run_id: &str) -> String {
    format!("e2e-fault-{}", mechanism::short(run_id))
}

fn heal_provider(run_id: &str) -> String {
    format!("e2e-heal-{}", mechanism::short(run_id))
}

fn effect_function(run_id: &str) -> String {
    format!("e2etest::fault_effect_{}", mechanism::short(run_id))
}

fn missing_function(run_id: &str) -> String {
    format!("e2etest::fault_missing_{}", mechanism::short(run_id))
}

fn typed_function(run_id: &str) -> String {
    format!("e2etest::fault_typed_{}", mechanism::short(run_id))
}

fn case_session(run_id: &str, case: &str) -> String {
    format!("{}_fault_{case}", mechanism::session_id(run_id))
}

/// The session the fan-out case's `n`-th spawn names, counting from 1.
fn fanout_child(run_id: &str, n: usize) -> String {
    format!("{}_c{n}", case_session(run_id, "fanout"))
}

/// What the typed fixture records for a call it accepted; also a valid payload.
fn typed_received(tag: &str, flag: bool, items: &[&str]) -> Value {
    json!({ "tag": tag, "flag": flag, "items": items })
}

/// The typed fixture's request: the SDK publishes its schema and refuses
/// any other shape, so a call it records arrived with these types.
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct TypedArgs {
    tag: String,
    flag: bool,
    items: Vec<String>,
}

/// The stack's `max_children`, from the entry the Harness registered for itself.
async fn max_children(context: &E2eContext) -> anyhow::Result<usize> {
    let listed = context
        .trigger_value("configuration::list", json!({}))
        .await?;
    let id = listed["configurations"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["metadata"]["ui_form"] == "harness")
        .and_then(|entry| entry["id"].as_str())
        .ok_or_else(|| anyhow::anyhow!("no Harness configuration entry is registered"))?;
    let entry = context
        .trigger_value("configuration::get", json!({ "id": id }))
        .await?;
    Ok(max_children_of(&entry["value"]))
}

fn max_children_of(configuration: &Value) -> usize {
    configuration["max_children"]
        .as_u64()
        .map_or(DEFAULT_MAX_CHILDREN, |cap| cap as usize)
}

/// Stream calls and side effects the fake provider saw, per attempt.
#[derive(Default)]
struct Ledger {
    calls: HashMap<String, usize>,
    /// The `messages` of every stream request, per case.
    requests: HashMap<String, Vec<Value>>,
    /// Side-effect fixture runs, by the case that asked for them.
    effects: HashMap<String, usize>,
    /// What the typed fixture was handed, per call it accepted.
    typed: Vec<Value>,
    /// The stack's `max_children`, read before the fan-out case.
    max_children: usize,
    /// The fan-out case's named child sessions that exist.
    children: Vec<String>,
    aborts: Vec<String>,
    stop_started: bool,
    stop_ended: Option<Instant>,
    heal_stream: Option<FunctionRef>,
    heal_token: Option<String>,
}

fn ledgers() -> &'static Mutex<HashMap<String, Arc<Mutex<Ledger>>>> {
    static LEDGERS: OnceLock<Mutex<HashMap<String, Arc<Mutex<Ledger>>>>> = OnceLock::new();
    LEDGERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ledger(run_id: &str) -> Arc<Mutex<Ledger>> {
    Arc::clone(
        lock_unpoisoned(ledgers())
            .entry(run_id.to_string())
            .or_default(),
    )
}

fn assistant(provider: &str, model: &Value, content: Value, stop_reason: &str) -> Value {
    json!({
        "role": "assistant",
        "content": content,
        "stop_reason": stop_reason,
        "model": model,
        "provider": provider,
        "timestamp": now_ms(),
    })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn text(value: &str) -> Value {
    json!([{ "type": "text", "text": value }])
}

fn agent_call(id: &str, function: &str, payload: Value) -> Value {
    json!({
        "type": "function_call",
        "id": id,
        "function_id": "agent_trigger",
        "arguments": { "function": function, "description": "E2E fault fixture", "payload": payload },
    })
}

/// What the fake provider streams for `case`'s `attempt`-th request.
fn script(run_id: &str, case: &str, attempt: usize, messages: &Value) -> Vec<Frame> {
    let effect = effect_function(run_id);
    match (case, attempt) {
        ("orphan", 1) => vec![Frame::Done(
            json!([
                agent_call("fc_orphan_1", &effect, json!({ "case": "orphan" })),
                agent_call("fc_orphan_2", &missing_function(run_id), json!({})),
            ]),
            "function_call",
        )],
        ("orphan", _) => {
            let unpaired = unpaired_calls(messages);
            let reply = if unpaired.is_empty() {
                "PAIRED".to_string()
            } else {
                format!("ORPHANED {}", unpaired.join(","))
            };
            vec![Frame::Text(vec![reply])]
        }
        ("prestream", 1 | 2) => vec![Frame::Error(Value::Array(Vec::new()), "rate_limited")],
        ("prestream", _) => vec![Frame::Text(vec!["PRESTREAM-".into(), "OK".into()])],
        ("permanent", _) => vec![Frame::Error(Value::Array(Vec::new()), "permanent")],
        ("midstream", 1) => vec![Frame::Done(
            json!([agent_call(
                "fc_mid_1",
                &effect,
                json!({ "case": "midstream" })
            )]),
            "function_call",
        )],
        ("midstream", 2) | ("exhausted", _) => {
            vec![Frame::Error(text("PART-RESUME "), "transient")]
        }
        ("midstream", _) => vec![Frame::Text(vec!["RESUMED".into()])],
        ("stop", _) => vec![Frame::PingUntilClosed],
        ("overflow", _) => vec![Frame::Error(Value::Array(Vec::new()), "context_overflow")],
        ("loop", _) => vec![Frame::Done(
            json!([agent_call(
                &format!("fc_loop_{attempt}"),
                &effect,
                json!({ "case": "loop", "step": attempt })
            )]),
            "function_call",
        )],
        ("typed", 1) => {
            let typed = typed_function(run_id);
            let stringified = json!({ "tag": "repaired", "flag": "true", "items": r#"["a","b"]"# });
            let unparseable = json!({ "tag": "refused", "flag": "maybe", "items": ["d"] });
            vec![Frame::Done(
                json!([
                    agent_call("fc_typed_repaired", &typed, stringified),
                    agent_call(
                        "fc_typed_control",
                        &typed,
                        typed_received("control", false, &["c"])
                    ),
                    agent_call("fc_typed_refused", &typed, unparseable),
                ]),
                "function_call",
            )]
        }
        ("typed", _) => vec![Frame::Text(vec!["TYPED-DONE".into()])],
        ("fanout", 1) => {
            let cap = lock_unpoisoned(&ledger(run_id)).max_children;
            let spawns = (1..=cap + 1)
                .map(|n| {
                    // Named, and on the fake provider: a child costs no token.
                    let request = json!({
                        "task": "Reply with one word.",
                        "session_id": fanout_child(run_id, n),
                        "model": FAULT_MODEL,
                        "provider": fault_provider(run_id),
                    });
                    agent_call(&format!("fc_fan_{n}"), "harness::spawn", request)
                })
                .collect();
            vec![Frame::Done(Value::Array(spawns), "function_call")]
        }
        ("fanout", _) => vec![Frame::Text(vec!["FANOUT-DONE".into()])],
        _ => vec![Frame::Text(vec!["UNSCRIPTED".into()])],
    }
}

/// Call ids in `messages` with no `function_result` after them.
fn unpaired_calls(messages: &Value) -> Vec<String> {
    let list = messages.as_array().cloned().unwrap_or_default();
    let mut unpaired = Vec::new();
    for (index, message) in list.iter().enumerate() {
        for block in message["content"].as_array().into_iter().flatten() {
            if block["type"] != "function_call" {
                continue;
            }
            let id = block["id"].as_str().unwrap_or_default();
            let answered = list[index + 1..]
                .iter()
                .any(|later| later["role"] == "function_result" && later["function_call_id"] == id);
            if !answered {
                unpaired.push(id.to_string());
            }
        }
    }
    unpaired
}

#[derive(Debug, Clone, PartialEq)]
enum Frame {
    /// A text answer streamed as these deltas, 150 ms apart.
    Text(Vec<String>),
    /// A finished message with this content and stop reason.
    Done(Value, &'static str),
    /// A terminal error after this partial content.
    Error(Value, &'static str),
    /// Pings until the channel closes.
    PingUntilClosed,
}

async fn stream(
    address: &str,
    provider: &str,
    input: &Value,
    frames: Vec<Frame>,
    on_ping: impl Fn(),
) -> Result<(), iii_sdk::errors::Error> {
    let writer_ref: StreamChannelRef = serde_json::from_value(input["writer_ref"].clone())
        .map_err(|error| iii_sdk::errors::Error::Serde(error.to_string()))?;
    let writer = ChannelWriter::new(address, &writer_ref);
    let model = &input["model"];
    let send = |frame: Value| {
        let writer = &writer;
        async move {
            writer
                .send_message(&frame.to_string())
                .await
                .map_err(|error| iii_sdk::errors::Error::Handler(error.to_string()))
        }
    };
    for frame in frames {
        match frame {
            Frame::Text(deltas) => {
                send(json!({ "type": "start", "partial": assistant(provider, model, json!([]), "end") })).await?;
                send(json!({ "type": "text_start", "partial": assistant(provider, model, json!([]), "end") })).await?;
                for delta in &deltas {
                    send(json!({ "type": "text_delta", "delta": delta })).await?;
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
                let message = assistant(provider, model, text(&deltas.concat()), "end");
                send(json!({ "type": "text_end", "partial": message })).await?;
                send(json!({ "type": "done", "message": message })).await?;
            }
            Frame::Done(content, stop_reason) => {
                send(json!({ "type": "start", "partial": assistant(provider, model, json!([]), stop_reason) })).await?;
                send(json!({ "type": "done", "message": assistant(provider, model, content, stop_reason) })).await?;
            }
            Frame::Error(partial, kind) => {
                if let Some(partial_text) = partial[0]["text"].as_str() {
                    send(json!({ "type": "start", "partial": assistant(provider, model, json!([]), "end") })).await?;
                    send(json!({ "type": "text_start", "partial": assistant(provider, model, json!([]), "end") })).await?;
                    send(json!({ "type": "text_delta", "delta": partial_text })).await?;
                }
                let mut error = assistant(provider, model, partial, "error");
                error["error_kind"] = json!(kind);
                error["error_message"] = json!(format!("e2e fault: {kind}"));
                send(json!({ "type": "error", "error": error })).await?;
            }
            Frame::PingUntilClosed => {
                send(json!({ "type": "start", "partial": assistant(provider, model, json!([]), "end") })).await?;
                let started = Instant::now();
                while started.elapsed() < Duration::from_secs(60) {
                    on_ping();
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    if send(json!({ "type": "ping" })).await.is_err() {
                        break;
                    }
                }
                return Ok(());
            }
        }
    }
    let _ = writer.close().await;
    Ok(())
}

fn declaration(provider: &str, model: &str, token: Option<&str>) -> Value {
    let mut declaration = json!({
        "id": provider,
        "display_name": "E2E fault provider",
        "models": [{
            "id": model,
            "provider": provider,
            "context_window": 100_000,
            "max_output_tokens": 4_096,
            "supports_tools": true,
        }],
    });
    if let Some(token) = token {
        declaration["token"] = json!(token);
    }
    declaration
}

fn register_stream(client: &IIIClient, run_id: &str, provider: &str, heal: bool) -> FunctionRef {
    let address = client.address().to_string();
    let owner = run_id.to_string();
    let provider_id = provider.to_string();
    client.register_function(
        format!("provider::{provider}::stream"),
        RegisterFunction::new_async(move |input: Value| {
            let (address, owner, provider_id) =
                (address.clone(), owner.clone(), provider_id.clone());
            async move {
                let session = input["session_id"].as_str().unwrap_or_default().to_string();
                let case = if heal {
                    "heal".to_string()
                } else {
                    CASES
                        .iter()
                        .find(|case| session == case_session(&owner, case))
                        .map_or("unknown", |case| *case)
                        .to_string()
                };
                let ledger = ledger(&owner);
                let attempt = {
                    let mut ledger = lock_unpoisoned(&ledger);
                    ledger
                        .requests
                        .entry(case.clone())
                        .or_default()
                        .push(input["messages"].clone());
                    let calls = ledger.calls.entry(case.clone()).or_default();
                    *calls += 1;
                    *calls
                };
                let frames = if heal {
                    vec![Frame::Text(vec!["HEALED".into()])]
                } else {
                    script(&owner, &case, attempt, &input["messages"])
                };
                let ping_ledger = Arc::clone(&ledger);
                stream(&address, &provider_id, &input, frames, move || {
                    lock_unpoisoned(&ping_ledger).stop_started = true;
                })
                .await?;
                if case == "stop" {
                    lock_unpoisoned(&ledger).stop_ended = Some(Instant::now());
                }
                Ok::<Value, iii_sdk::errors::Error>(json!({ "ok": true }))
            }
        })
        .description("E2E scripted fault provider stream."),
    )
}

pub struct RouterFaultContract;

#[async_trait]
impl Scenario for RouterFaultContract {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Survive a provider that fails on cue")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("One plain reply; beside it the runner plays a model provider that rate-limits, cuts streams, overflows, hangs and disappears, then one that loops, sends stringified arguments and over-spawns, and checks how each turn ends.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("When the model provider rate-limits, drops a stream, overflows, hangs or goes away, agent turns retry, resume, stop or fail cleanly, and come back when the provider does. A model that loops is stopped at the step cap, call arguments written as strings are repaired before the call runs, and one step cannot start more sub-agents than configured. If it fails, provider hiccups lose work, repeat tool side effects, hang turns, or leave a recovered provider unused; a runaway turn burns budget without limit, or a mistyped argument runs as something the model did not ask for.")
    }

    /// One retained cohort: the probes do not vary with a seed.
    fn case(&self, _seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            super::stable_seed(ID),
            json!({ "cases": CASES, "heal": true }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::E2eRunScopedFixtures,
                Capability::IiiFunctions,
            ],
            mechanism::contract(DELIVERABLE_ID),
        )
    }

    fn canonical_seed_only(&self) -> bool {
        true
    }

    fn spec(&self, _run_id: &str) -> ScenarioSpec {
        ScenarioSpec {
            id: ID,
            prompt: "Reply with exactly `ACK FAULTS` and nothing else. Do not call any function."
                .to_string(),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(3),
                max_output_tokens: Some(1_024),
                max_total_tokens: Some(40_000),
                stuck_timeout_seconds: 180,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    fn allowed_functions(&self, _run_id: &str) -> Option<Vec<String>> {
        Some(Vec::new())
    }

    fn planted_state(&self, _run_id: &str) -> Option<Value> {
        Some(json!({ "runner_messages": { "case": CASE_MESSAGE } }))
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        mechanism::reset(run_id);
        lock_unpoisoned(ledgers()).remove(run_id);
        for case in CASES.iter().chain(&["heal1", "heal2"]) {
            mechanism::keep_session(run_id, &case_session(run_id, case));
        }
        let client = context.client();
        let owner = run_id.to_string();
        let effect = client.register_function(
            effect_function(run_id),
            RegisterFunction::new_async(move |input: Value| {
                let owner = owner.clone();
                async move {
                    let case = input["case"].as_str().unwrap_or_default().to_string();
                    *lock_unpoisoned(&ledger(&owner))
                        .effects
                        .entry(case)
                        .or_default() += 1;
                    Ok::<Value, iii_sdk::errors::Error>(json!({ "applied": true }))
                }
            })
            .description("E2E run-scoped side effect: counts its invocations."),
        );
        mechanism::keep_function(run_id, effect);
        let owner = run_id.to_string();
        let typed = client.register_function(
            typed_function(run_id),
            RegisterFunction::new_async(move |input: TypedArgs| {
                let owner = owner.clone();
                async move {
                    let items: Vec<&str> = input.items.iter().map(String::as_str).collect();
                    lock_unpoisoned(&ledger(&owner))
                        .typed
                        .push(typed_received(&input.tag, input.flag, &items));
                    Ok::<Value, iii_sdk::errors::Error>(json!({ "applied": true }))
                }
            })
            .description("E2E run-scoped typed fixture: records the arguments it was handed."),
        );
        mechanism::keep_function(run_id, typed);
        let fault = fault_provider(run_id);
        mechanism::keep_function(run_id, register_stream(client, run_id, &fault, false));
        let owner = run_id.to_string();
        let abort = client.register_function(
            format!("provider::{fault}::abort"),
            RegisterFunction::new_async(move |input: Value| {
                let owner = owner.clone();
                async move {
                    let request = input["request_id"].as_str().unwrap_or_default().to_string();
                    lock_unpoisoned(&ledger(&owner)).aborts.push(request);
                    Ok::<Value, iii_sdk::errors::Error>(json!({ "aborted": true }))
                }
            })
            .description("E2E scripted fault provider abort."),
        );
        mechanism::keep_function(run_id, abort);
        context
            .trigger_value(
                "router::provider::register",
                declaration(&fault, FAULT_MODEL, None),
            )
            .await?;

        let heal = heal_provider(run_id);
        let stream = register_stream(client, run_id, &heal, true);
        lock_unpoisoned(&ledger(run_id)).heal_stream = Some(stream);
        let registered = context
            .trigger_value(
                "router::provider::register",
                declaration(&heal, HEAL_MODEL, None),
            )
            .await?;
        lock_unpoisoned(&ledger(run_id)).heal_token = registered["registration_token"]
            .as_str()
            .map(str::to_string);
        Ok(())
    }

    async fn capture(
        &self,
        context: &E2eContext,
        _observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let fault = fault_provider(run_id);
        for case in CASES {
            let session = case_session(run_id, case);
            if case == "fanout" {
                match max_children(context).await {
                    Ok(cap) => lock_unpoisoned(&ledger(run_id)).max_children = cap,
                    Err(error) => {
                        record(run_id, case, false, format!("max_children: {error:#}"));
                        continue;
                    }
                }
            }
            let max_turns = if case == "loop" { LOOP_TURNS } else { 10 };
            let sent = send(context, &session, &fault, FAULT_MODEL, run_id, max_turns).await;
            let status = match sent {
                Ok(_) if case == "stop" => stop_case(context, run_id, &session).await,
                Ok(_) => terminal(context, &session, TURN_WAIT).await,
                Err(error) => {
                    record(run_id, case, false, format!("harness::send: {error:#}"));
                    continue;
                }
            };
            let transcript = context.transcript(&session).await.unwrap_or(Value::Null);
            if case == "fanout" {
                let children = fanout_children(context, run_id).await;
                lock_unpoisoned(&ledger(run_id)).children = children;
            }
            let (passed, detail) = judge(run_id, case, status.as_ref(), &transcript);
            record(run_id, case, passed, detail);
        }
        heal_case(context, run_id).await;
        Ok(vec![mechanism::deliverable(DELIVERABLE_ID, run_id)])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        if !observation.metrics.complete {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "metrics_complete",
                "subject metrics are incomplete",
            ));
        }
        let checks = mechanism::observed_checks(observation, run_id);
        Ok(assessment::build_evaluation([
            mechanism::grade(PAIRING, &checks, &["orphan"]),
            mechanism::grade(PRESTREAM, &checks, &["prestream", "permanent"]),
            mechanism::grade(RESUME, &checks, &["midstream", "exhausted"]),
            mechanism::grade(STOP, &checks, &["stop"]),
            mechanism::grade(OVERFLOW, &checks, &["overflow"]),
            mechanism::grade(HEAL, &checks, &["heal"]),
            mechanism::grade(TURN_CAP, &checks, &["loop"]),
            mechanism::grade(RECONCILE, &checks, &["typed"]),
            mechanism::grade(FANOUT, &checks, &["fanout"]),
        ]))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        if let Some(ledger) = lock_unpoisoned(ledgers()).remove(run_id) {
            if let Some(stream) = lock_unpoisoned(&ledger).heal_stream.take() {
                stream.unregister();
            }
        }
        mechanism::release(context, run_id).await
    }
}

async fn send(
    context: &E2eContext,
    session_id: &str,
    provider: &str,
    model: &str,
    run_id: &str,
    max_turns: usize,
) -> anyhow::Result<Value> {
    context
        .trigger_value(
            "harness::send",
            json!({
                "session_id": session_id,
                "message": CASE_MESSAGE,
                "model": model,
                "provider": provider,
                "session": { "kind": crate::wire::E2E_SESSION_KIND, "title": "Harness E2E: router fault" },
                "options": {
                    "max_turns": max_turns,
                    "functions": {
                        "allow": [effect_function(run_id), typed_function(run_id), "harness::spawn"],
                        "deny": [],
                    },
                },
            }),
        )
        .await
}

async fn terminal(context: &E2eContext, session_id: &str, timeout: Duration) -> Option<Value> {
    mechanism::poll(timeout, || async {
        let status = context
            .trigger_value("harness::status", json!({ "session_id": session_id }))
            .await
            .ok()?;
        matches!(
            status["status"].as_str(),
            Some("completed" | "failed" | "cancelled")
        )
        .then_some(status)
    })
    .await
}

/// Stops the pinging stream once it runs; the status once cancelled.
async fn stop_case(context: &E2eContext, run_id: &str, session_id: &str) -> Option<Value> {
    let ledger = ledger(run_id);
    let started = mechanism::poll(Duration::from_secs(60), || {
        let ledger = Arc::clone(&ledger);
        async move { lock_unpoisoned(&ledger).stop_started.then_some(()) }
    })
    .await;
    started?;
    let clock = Instant::now();
    let stopped = context
        .trigger_value("harness::stop", json!({ "session_id": session_id }))
        .await;
    let status = mechanism::poll(STOP_WITHIN, || async {
        let status = context
            .trigger_value("harness::status", json!({ "session_id": session_id }))
            .await
            .ok()?;
        (status["status"] == "cancelled").then_some(status)
    })
    .await;
    // The provider's side shares the stop's 5 s; `judge` reads what it left.
    if let Some(turn) = status
        .as_ref()
        .and_then(|status| status["turn_id"].as_str())
    {
        provider_saw_stop(run_id, turn, STOP_WITHIN.saturating_sub(clock.elapsed())).await;
    }
    mechanism::note(
        run_id,
        "stop",
        json!({
            "stopping": stopped.ok().map(|response| response["stopping"].clone()),
            "cancelled_ms": status.as_ref().map(|_| clock.elapsed().as_millis() as u64),
        }),
    );
    status.or_else(|| Some(json!({ "status": "not cancelled within 5 s" })))
}

/// Whether the provider was told to abort `turn`'s request and its stream ended.
fn stop_reached_provider(ledger: &Ledger, turn: &str) -> bool {
    !turn.is_empty()
        && ledger
            .aborts
            .iter()
            .any(|request| request.starts_with(&format!("{turn}:")))
        && ledger.stop_ended.is_some()
}

/// Waits for the provider's side of a stop. The router calls the provider's
/// abort detached and the fake finds its channel closed only on its next
/// ping, so both can land after the turn already reads `cancelled`.
async fn provider_saw_stop(run_id: &str, turn: &str, within: Duration) -> bool {
    let ledger = ledger(run_id);
    mechanism::poll(within, || {
        let ledger = Arc::clone(&ledger);
        async move { stop_reached_provider(&lock_unpoisoned(&ledger), turn).then_some(()) }
    })
    .await
    .is_some()
}

fn rows<'a>(transcript: &'a Value, custom_type: &str) -> Vec<&'a Value> {
    mechanism::customs(transcript, custom_type)
        .into_iter()
        .map(|(_, data)| data)
        .collect()
}

fn assistant_texts(transcript: &Value) -> Vec<String> {
    mechanism::entries(transcript)
        .filter_map(|entry| entry.get("message"))
        .filter(|message| message["role"] == "assistant")
        .map(mechanism::text_of)
        .filter(|text| !text.is_empty())
        .collect()
}

/// The `function_result` entry answering `call_id`.
fn result_of<'a>(transcript: &'a Value, call_id: &str) -> Option<&'a Value> {
    mechanism::entries(transcript).find(|entry| {
        entry["message"]["role"] == "function_result"
            && entry["message"]["function_call_id"] == call_id
    })
}

/// The fan-out case's named sessions that exist, kept for cleanup.
async fn fanout_children(context: &E2eContext, run_id: &str) -> Vec<String> {
    let cap = lock_unpoisoned(&ledger(run_id)).max_children;
    let mut existing = Vec::new();
    for n in 1..=cap + 1 {
        let child = fanout_child(run_id, n);
        // `session::get` answers `null` for an unknown session.
        let found = context
            .trigger_value("session::get", json!({ "session_id": child }))
            .await;
        if found.is_ok_and(|session| !session.is_null()) {
            mechanism::keep_session(run_id, &child);
            existing.push(child);
        }
    }
    existing
}

/// Whether `case` ended as its script says, and why.
fn judge(run_id: &str, case: &str, status: Option<&Value>, transcript: &Value) -> (bool, String) {
    let ledger = ledger(run_id);
    let ledger = lock_unpoisoned(&ledger);
    let calls = ledger.calls.get(case).copied().unwrap_or(0);
    let state = status.map_or(Value::Null, |status| status["status"].clone());
    let errors = rows(transcript, "error");
    let recoveries = rows(transcript, "recovery");
    let texts = assistant_texts(transcript);
    let detail = format!(
        "status={state} stream_calls={calls} effects={:?} texts={texts:?} errors={:?} recoveries={:?}",
        ledger.effects,
        errors.iter().map(|error| json!({ "code": error["code"], "class": error["class"], "retryable": error["retryable"], "recovery": error["recovery"] })).collect::<Vec<_>>(),
        recoveries.iter().map(|row| json!({ "status": row["status"], "attempt": row["attempt"], "partial": row["partial_result_available"] })).collect::<Vec<_>>(),
    );
    let passed = match case {
        "orphan" => {
            state == "completed"
                && calls == 2
                && texts.last().is_some_and(|text| text == "PAIRED")
                && ledger
                    .requests
                    .get(case)
                    .and_then(|requests| requests.get(1))
                    .is_some_and(|messages| unpaired_calls(messages).is_empty())
        }
        "prestream" => {
            state == "completed"
                && calls == 3
                && texts == ["PRESTREAM-OK"]
                && errors.is_empty()
                && recoveries.is_empty()
        }
        "permanent" => {
            state == "failed" && calls == 1 && errors.len() == 1 && errors[0]["retryable"] == false
        }
        "midstream" => {
            let statuses: Vec<&Value> = recoveries.iter().map(|row| &row["status"]).collect();
            state == "completed"
                && calls == 3
                && statuses == ["recovering", "recovered"]
                && recoveries[0]["partial_result_available"] == true
                && texts.iter().any(|text| text.contains("PART-RESUME"))
                && texts.last().is_some_and(|text| text == "RESUMED")
                && errors.is_empty()
                && ledger.effects.get("midstream") == Some(&1)
        }
        "exhausted" => {
            state == "failed"
                && calls == 4
                && errors.len() == 1
                && errors[0]["recovery"]["attempted"] == 3
                && errors[0]["recovery"]["outcome"] == "exhausted"
        }
        "stop" => {
            let turn = status
                .and_then(|status| status["turn_id"].as_str())
                .unwrap_or_default();
            state == "cancelled" && stop_reached_provider(&ledger, turn)
        }
        "overflow" => {
            state == "failed" && errors.len() == 1 && errors[0]["class"] == "llm.context_overflow"
        }
        "loop" => {
            let notices = rows(transcript, "notice");
            state == "completed"
                && status.is_some_and(|status| status["stop_reason"] == "max_turns")
                && calls == LOOP_TURNS
                && ledger.effects.get("loop") == Some(&LOOP_TURNS)
                && notices
                    .iter()
                    .filter(|notice| notice["reason"] == "max_turns")
                    .count()
                    == 1
                && errors.is_empty()
        }
        "typed" => {
            let unmarked = |entry: &Value| entry["origin"].get("reconciled").is_none();
            let mut parsed: Vec<&str> = result_of(transcript, "fc_typed_repaired")
                .and_then(|entry| entry["origin"]["reconciled"].as_array())
                .into_iter()
                .flatten()
                .filter(|change| change["kind"] == "parsed")
                .filter_map(|change| change["path"].as_str())
                .collect();
            parsed.sort_unstable();
            state == "completed"
                && calls == 2
                && ledger.typed
                    == [
                        typed_received("repaired", true, &["a", "b"]),
                        typed_received("control", false, &["c"]),
                    ]
                && parsed == ["/flag", "/items"]
                && result_of(transcript, "fc_typed_control").is_some_and(unmarked)
                && result_of(transcript, "fc_typed_refused").is_some_and(|entry| {
                    let diagnosis = mechanism::text_of(&entry["message"]);
                    unmarked(entry)
                        && entry["message"]["is_error"] == true
                        && diagnosis.contains("[harness] These arguments do not match")
                        && diagnosis.contains("`flag`")
                })
        }
        "fanout" => {
            let cap = ledger.max_children;
            let spawn = |n: usize| {
                result_of(transcript, &format!("fc_fan_{n}")).map(|entry| &entry["message"])
            };
            let children: Vec<String> = (1..=cap).map(|n| fanout_child(run_id, n)).collect();
            state == "completed"
                && children.iter().zip(1..).all(|(child, n)| {
                    spawn(n).is_some_and(|result| {
                        result["is_error"] != true
                            && result["details"]["child_session_id"] == child.as_str()
                    })
                })
                && spawn(cap + 1).is_some_and(|result| {
                    result["is_error"] == true && result["details"]["error"] == FANOUT_CODE
                })
                && ledger.children == children
        }
        _ => false,
    };
    let outcome = |call_id: &str| {
        result_of(transcript, call_id).map(|entry| {
            json!({
                "is_error": entry["message"]["is_error"],
                "error": entry["message"]["details"]["error"],
                "reconciled": entry["origin"]["reconciled"],
                "text": mechanism::text_of(&entry["message"]),
            })
        })
    };
    let detail = match case {
        "stop" => format!(
            "{detail} aborts={:?} stream_ended={}",
            ledger.aborts,
            ledger.stop_ended.is_some()
        ),
        "loop" => format!(
            "{detail} stop_reason={} notices={:?}",
            status.map_or(&Value::Null, |status| &status["stop_reason"]),
            rows(transcript, "notice"),
        ),
        "typed" => format!(
            "{detail} received={:?} results={:?}",
            ledger.typed,
            ["fc_typed_repaired", "fc_typed_control", "fc_typed_refused"].map(outcome),
        ),
        "fanout" => format!(
            "{detail} max_children={} children={:?} spawns={:?}",
            ledger.max_children,
            ledger.children,
            (1..=ledger.max_children + 1)
                .map(|n| outcome(&format!("fc_fan_{n}")))
                .collect::<Vec<_>>(),
        ),
        _ => detail,
    };
    (passed, detail)
}

/// The heal provider: its stream function goes away, a turn fails and the
/// router marks it down; it re-registers and serves the next turn.
async fn heal_case(context: &E2eContext, run_id: &str) {
    let heal = heal_provider(run_id);
    let (stream, token) = {
        let ledger = ledger(run_id);
        let mut ledger = lock_unpoisoned(&ledger);
        (ledger.heal_stream.take(), ledger.heal_token.clone())
    };
    let (Some(stream), Some(token)) = (stream, token) else {
        record(run_id, "heal", false, "the heal provider never registered");
        return;
    };
    stream.unregister();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let down_session = case_session(run_id, "heal1");
    let down_sent = send(context, &down_session, &heal, HEAL_MODEL, run_id, 10).await;
    let down = match &down_sent {
        Ok(_) => terminal(context, &down_session, TURN_WAIT).await,
        Err(_) => None,
    };
    let listed_down = available(context, &heal).await;

    let stream = register_stream(context.client(), run_id, &heal, true);
    lock_unpoisoned(&ledger(run_id)).heal_stream = Some(stream);
    let reregistered = context
        .trigger_value(
            "router::provider::register",
            declaration(&heal, HEAL_MODEL, Some(&token)),
        )
        .await;
    let listed_up = available(context, &heal).await;
    let up_session = case_session(run_id, "heal2");
    let up = match send(context, &up_session, &heal, HEAL_MODEL, run_id, 10).await {
        Ok(_) => terminal(context, &up_session, TURN_WAIT).await,
        Err(_) => None,
    };
    let up_texts = assistant_texts(&context.transcript(&up_session).await.unwrap_or(Value::Null));
    let refused_or_failed = down_sent.is_err()
        || down
            .as_ref()
            .is_some_and(|status| status["status"] == "failed");
    record(
        run_id,
        "heal",
        refused_or_failed
            && listed_down == Some(false)
            && reregistered.is_ok()
            && listed_up == Some(true)
            && up.as_ref().is_some_and(|status| status["status"] == "completed")
            && up_texts == ["HEALED"],
        format!(
            "down: send_ok={} status={:?} listed_available={listed_down:?}; re-register={:?} listed_available={listed_up:?}; up: status={:?} texts={up_texts:?}",
            down_sent.is_ok(),
            down.map(|status| status["status"].clone()),
            reregistered.map(|_| "ok"),
            up.map(|status| status["status"].clone()),
        ),
    );
}

async fn available(context: &E2eContext, provider: &str) -> Option<bool> {
    let listed = context
        .trigger_value("router::provider::list", json!({}))
        .await
        .ok()?;
    listed["providers"]
        .as_array()?
        .iter()
        .find(|entry| entry["id"] == provider)?["available"]
        .as_bool()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpaired_calls_finds_a_call_without_a_later_result() {
        let messages = json!([
            { "role": "assistant", "content": [
                { "type": "function_call", "id": "a" }, { "type": "function_call", "id": "b" },
            ] },
            { "role": "function_result", "function_call_id": "a" },
        ]);
        assert_eq!(unpaired_calls(&messages), ["b"]);
        let mut paired = messages.clone();
        paired
            .as_array_mut()
            .unwrap()
            .push(json!({ "role": "function_result", "function_call_id": "b" }));
        assert!(unpaired_calls(&paired).is_empty());
    }

    #[test]
    fn the_scripts_fail_where_each_case_says() {
        let run = "control";
        let empty = json!([]);
        assert_eq!(
            script(run, "prestream", 2, &empty),
            [Frame::Error(json!([]), "rate_limited")]
        );
        assert_eq!(
            script(run, "prestream", 3, &empty),
            [Frame::Text(vec!["PRESTREAM-".into(), "OK".into()])]
        );
        assert_eq!(
            script(run, "exhausted", 4, &empty),
            [Frame::Error(text("PART-RESUME "), "transient")]
        );
        assert_eq!(
            script(run, "midstream", 3, &empty),
            [Frame::Text(vec!["RESUMED".into()])]
        );
        assert!(matches!(
            &script(run, "orphan", 1, &empty)[0],
            Frame::Done(_, "function_call")
        ));
        let unpaired =
            json!([{ "role": "assistant", "content": [{ "type": "function_call", "id": "x" }] }]);
        assert_eq!(
            script(run, "orphan", 2, &unpaired),
            [Frame::Text(vec!["ORPHANED x".into()])]
        );
    }

    fn row(custom_type: &str, data: Value) -> Value {
        json!({ "entry_id": "e_x", "custom": { "custom_type": custom_type, "data": data } })
    }

    fn reply(text_value: &str) -> Value {
        json!({ "message": { "role": "assistant", "content": text(text_value) } })
    }

    /// `judge` on one case: `calls` stream requests, the given transcript
    /// rows and status, and whatever else `ledger` adds.
    fn judged(
        case: &str,
        calls: usize,
        state: &str,
        messages: Vec<Value>,
        ledger_with: impl FnOnce(&mut Ledger),
    ) -> bool {
        let run = format!("judge-{case}-{calls}-{state}-{}", messages.len());
        {
            let shared = ledger(&run);
            let mut shared = lock_unpoisoned(&shared);
            shared.calls.insert(case.to_string(), calls);
            ledger_with(&mut shared);
        }
        let status = json!({ "status": state, "turn_id": "t_1" });
        let (passed, _) = judge(&run, case, Some(&status), &json!({ "messages": messages }));
        lock_unpoisoned(ledgers()).remove(&run);
        passed
    }

    #[test]
    fn each_fault_case_passes_on_its_healthy_ending_only() {
        let paired = |ledger: &mut Ledger| {
            ledger
                .requests
                .insert("orphan".into(), vec![json!([]), json!([])]);
        };
        assert!(judged(
            "orphan",
            2,
            "completed",
            vec![reply("PAIRED")],
            paired
        ));
        assert!(!judged(
            "orphan",
            2,
            "completed",
            vec![reply("ORPHANED fc_orphan_2")],
            paired
        ));

        assert!(judged(
            "prestream",
            3,
            "completed",
            vec![reply("PRESTREAM-OK")],
            |_| {}
        ));
        let visible = vec![reply("PRESTREAM-OK"), row("error", json!({ "code": "x" }))];
        assert!(!judged("prestream", 3, "completed", visible, |_| {}));
        assert!(!judged(
            "prestream",
            4,
            "completed",
            vec![reply("PRESTREAM-OK")],
            |_| {}
        ));

        let permanent = || vec![row("error", json!({ "retryable": false }))];
        assert!(judged("permanent", 1, "failed", permanent(), |_| {}));
        assert!(!judged("permanent", 2, "failed", permanent(), |_| {}));

        let resumed = || {
            vec![
                reply("PART-RESUME "),
                row(
                    "recovery",
                    json!({ "status": "recovering", "partial_result_available": true }),
                ),
                row("recovery", json!({ "status": "recovered" })),
                reply("RESUMED"),
            ]
        };
        let once = |ledger: &mut Ledger| {
            ledger.effects.insert("midstream".into(), 1);
        };
        let twice = |ledger: &mut Ledger| {
            ledger.effects.insert("midstream".into(), 2);
        };
        assert!(judged("midstream", 3, "completed", resumed(), once));
        assert!(!judged("midstream", 3, "completed", resumed(), twice));

        let exhausted = |attempted: u64| {
            vec![row(
                "error",
                json!({ "recovery": { "attempted": attempted, "outcome": "exhausted" } }),
            )]
        };
        assert!(judged("exhausted", 4, "failed", exhausted(3), |_| {}));
        assert!(!judged("exhausted", 4, "failed", exhausted(2), |_| {}));
        assert!(!judged("exhausted", 3, "failed", exhausted(3), |_| {}));

        let aborted = |request: &'static str| {
            move |ledger: &mut Ledger| {
                ledger.aborts.push(request.into());
                ledger.stop_ended = Some(Instant::now());
            }
        };
        assert!(judged("stop", 1, "cancelled", vec![], aborted("t_1:0")));
        assert!(!judged("stop", 1, "cancelled", vec![], aborted("t_0:0")));
        assert!(!judged("stop", 1, "completed", vec![], aborted("t_1:0")));
        let still_streaming = |ledger: &mut Ledger| ledger.aborts.push("t_1:0".into());
        assert!(!judged("stop", 1, "cancelled", vec![], still_streaming));

        let overflow = |class: &str| vec![row("error", json!({ "class": class }))];
        assert!(judged(
            "overflow",
            1,
            "failed",
            overflow("llm.context_overflow"),
            |_| {}
        ));
        assert!(!judged(
            "overflow",
            1,
            "failed",
            overflow("llm.provider_error"),
            |_| {}
        ));
    }

    fn calls_of(frames: &[Frame]) -> Vec<Value> {
        match frames {
            [Frame::Done(content, "function_call")] => content.as_array().cloned().unwrap(),
            other => panic!("not one step of calls: {other:?}"),
        }
    }

    #[test]
    fn the_cap_cases_script_their_calls() {
        let run = "cap-scripts";
        let empty = json!([]);
        for attempt in [1, 2, 7] {
            let calls = calls_of(&script(run, "loop", attempt, &empty));
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0]["arguments"]["function"], effect_function(run));
        }

        let typed = calls_of(&script(run, "typed", 1, &empty));
        let payloads: Vec<&Value> = typed
            .iter()
            .map(|call| &call["arguments"]["payload"])
            .collect();
        assert_eq!(payloads[0]["flag"], "true");
        assert_eq!(payloads[0]["items"], r#"["a","b"]"#);
        assert_eq!(payloads[1], &typed_received("control", false, &["c"]));
        assert_eq!(payloads[2]["flag"], "maybe");
        assert_eq!(
            script(run, "typed", 2, &empty),
            [Frame::Text(vec!["TYPED-DONE".into()])]
        );

        lock_unpoisoned(&ledger(run)).max_children = 2;
        let spawns = calls_of(&script(run, "fanout", 1, &empty));
        assert_eq!(spawns.len(), 3);
        for (index, spawn) in spawns.iter().enumerate() {
            assert_eq!(spawn["arguments"]["function"], "harness::spawn");
            let payload = &spawn["arguments"]["payload"];
            assert_eq!(payload["session_id"], fanout_child(run, index + 1));
            assert_eq!(payload["provider"], fault_provider(run));
            assert_eq!(payload["model"], FAULT_MODEL);
        }
        lock_unpoisoned(ledgers()).remove(run);
    }

    fn result(call_id: &str, origin: Value, message: Value) -> Value {
        let mut message = message;
        message["role"] = json!("function_result");
        message["function_call_id"] = json!(call_id);
        json!({ "origin": origin, "message": message })
    }

    /// `judge` on a case whose status carries more than a state.
    fn judged_status(
        run: &str,
        case: &str,
        calls: usize,
        status: Value,
        messages: Vec<Value>,
        ledger_with: impl FnOnce(&mut Ledger),
    ) -> bool {
        {
            let shared = ledger(run);
            let mut shared = lock_unpoisoned(&shared);
            shared.calls.insert(case.to_string(), calls);
            ledger_with(&mut shared);
        }
        let (passed, _) = judge(run, case, Some(&status), &json!({ "messages": messages }));
        lock_unpoisoned(ledgers()).remove(run);
        passed
    }

    #[test]
    fn the_step_cap_case_wants_three_streams_a_notice_and_the_stop_reason() {
        let capped = || json!({ "status": "completed", "stop_reason": "max_turns" });
        let notice = || {
            vec![
                row("notice", json!({ "reason": "registry_changed" })),
                row("notice", json!({ "reason": "max_turns" })),
            ]
        };
        let ran = |times: usize| {
            move |ledger: &mut Ledger| {
                ledger.effects.insert("loop".into(), times);
            }
        };
        let judged = |calls, status, messages, effects| {
            judged_status("judge-loop", "loop", calls, status, messages, ran(effects))
        };
        assert!(judged(3, capped(), notice(), 3));
        assert!(!judged(4, capped(), notice(), 4));
        assert!(!judged(3, capped(), notice(), 2));
        assert!(!judged(3, capped(), vec![], 3));
        assert!(!judged(3, json!({ "status": "completed" }), notice(), 3));
        let failed = json!({ "status": "failed", "stop_reason": "max_turns" });
        assert!(!judged(3, failed, notice(), 3));
    }

    #[test]
    fn the_reconciliation_case_wants_the_repair_the_mark_and_the_diagnosis() {
        let parsed = |path: &str| json!({ "path": path, "kind": "parsed" });
        let marked =
            || json!({ "turn_id": "t_1", "reconciled": [parsed("/flag"), parsed("/items")] });
        let diagnosis = "[harness] These arguments do not match e2etest::x's request schema: `flag`: \"maybe\" is not of type \"boolean\".";
        let refusal = |text_value: &str| json!({ "is_error": true, "content": text(text_value) });
        let transcript = |mark: Value, control_origin: Value, refused: Value| {
            vec![
                result("fc_typed_repaired", mark, json!({ "is_error": false })),
                result(
                    "fc_typed_control",
                    control_origin,
                    json!({ "is_error": false }),
                ),
                result("fc_typed_refused", json!({ "turn_id": "t_1" }), refused),
            ]
        };
        let received = |flag: Value| {
            move |ledger: &mut Ledger| {
                let mut repaired = typed_received("repaired", true, &["a", "b"]);
                repaired["flag"] = flag;
                ledger.typed = vec![repaired, typed_received("control", false, &["c"])];
            }
        };
        let judged = |messages, flag: Value| {
            let status = json!({ "status": "completed" });
            judged_status("judge-typed", "typed", 2, status, messages, received(flag))
        };
        let plain = || json!({ "turn_id": "t_1" });
        assert!(judged(
            transcript(marked(), plain(), refusal(diagnosis)),
            json!(true)
        ));
        assert!(!judged(
            transcript(marked(), plain(), refusal(diagnosis)),
            json!("true")
        ));
        assert!(!judged(
            transcript(plain(), plain(), refusal(diagnosis)),
            json!(true)
        ));
        assert!(!judged(
            transcript(marked(), marked(), refusal(diagnosis)),
            json!(true)
        ));
        let bare = refusal("serialization error: invalid type");
        assert!(!judged(transcript(marked(), plain(), bare), json!(true)));
        let accepted = json!({ "is_error": false, "content": text(diagnosis) });
        assert!(!judged(
            transcript(marked(), plain(), accepted),
            json!(true)
        ));
    }

    #[test]
    fn the_fan_out_case_wants_the_cap_refused_once_and_the_cap_in_children() {
        let run = "judge-fanout";
        let spawned = |n: usize| {
            let details = json!({ "child_session_id": fanout_child(run, n) });
            result(
                &format!("fc_fan_{n}"),
                json!({}),
                json!({ "is_error": false, "details": details }),
            )
        };
        let refused = |n: usize, code: &str| {
            let details = json!({ "error": code });
            result(
                &format!("fc_fan_{n}"),
                json!({}),
                json!({ "is_error": true, "details": details }),
            )
        };
        let children = |count: usize| {
            move |ledger: &mut Ledger| {
                ledger.max_children = 2;
                ledger.children = (1..=count).map(|n| fanout_child(run, n)).collect();
            }
        };
        let judged = |messages, existing| {
            let status = json!({ "status": "completed" });
            judged_status(run, "fanout", 2, status, messages, children(existing))
        };
        let capped = || vec![spawned(1), spawned(2), refused(3, FANOUT_CODE)];
        assert!(judged(capped(), 2));
        assert!(!judged(capped(), 3));
        assert!(!judged(capped(), 1));
        assert!(!judged(vec![spawned(1), spawned(2), spawned(3)], 2));
        let early = vec![spawned(1), refused(2, FANOUT_CODE), refused(3, FANOUT_CODE)];
        assert!(!judged(early, 2));
        let depth = vec![
            spawned(1),
            spawned(2),
            refused(3, "harness/spawn_depth_exceeded"),
        ];
        assert!(!judged(depth, 2));
    }

    #[test]
    fn max_children_falls_back_to_the_harness_default() {
        assert_eq!(max_children_of(&json!({ "max_children": 3 })), 3);
        assert_eq!(max_children_of(&json!({})), DEFAULT_MAX_CHILDREN);
        assert_eq!(max_children_of(&Value::Null), DEFAULT_MAX_CHILDREN);
    }

    /// The pilot's lost round: the turn read `cancelled` before the fake's
    /// next ping found its channel closed.
    #[tokio::test]
    async fn the_stop_case_waits_for_the_provider_to_see_the_stop() {
        let run = "stop-settles";
        let shared = ledger(run);
        let late = Arc::clone(&shared);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let mut late = lock_unpoisoned(&late);
            late.aborts.push("t_1:0".into());
            late.stop_ended = Some(Instant::now());
        });
        assert!(provider_saw_stop(run, "t_1", Duration::from_secs(5)).await);
        lock_unpoisoned(&shared).stop_ended = None;
        assert!(!provider_saw_stop(run, "t_1", Duration::ZERO).await);
        lock_unpoisoned(ledgers()).remove(run);
    }

    #[test]
    fn the_case_message_is_part_of_the_definition() {
        let planted = RouterFaultContract.planted_state("run").unwrap();
        assert_eq!(planted["runner_messages"]["case"], CASE_MESSAGE);
    }

    #[test]
    fn criteria_weights_total_one_hundred() {
        RouterFaultContract.spec("run").validate().unwrap();
    }
}
