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
const CASES: [&str; 7] = [
    "orphan",
    "prestream",
    "permanent",
    "midstream",
    "exhausted",
    "stop",
    "overflow",
];

const PAIRING: AssessmentSpec = AssessmentSpec::gate(
    "call_pairing",
    15,
    "After a step with a call to a missing function, did the next request carry a result for every call?",
)
.labeled("Every call has a result in the next request");
const PRESTREAM: AssessmentSpec = AssessmentSpec::gate(
    "prestream_retry",
    20,
    "Were two pre-stream rate limits retried invisibly (three stream calls, one reply, no error or recovery row), and did a permanent error fail after one call as not retryable?",
)
.labeled("Pre-stream errors retry invisibly, permanent ones fail once");
const RESUME: AssessmentSpec = AssessmentSpec::gate(
    "midstream_resume",
    25,
    "Did a stream cut mid-answer resume with recovering and recovered rows, keep the partial and run the earlier call once, and did a stream cut every time fail after three resumes?",
)
.labeled("A cut stream resumes once, gives up after three");
const STOP: AssessmentSpec = AssessmentSpec::gate(
    "stop_mid_stream",
    15,
    "Did `harness::stop` cancel a turn whose stream only pinged in under 5 s, with the router aborting the provider's `<turn>:<step>` request?",
)
.labeled("Stop aborts a live stream within 5 s");
const OVERFLOW: AssessmentSpec = AssessmentSpec::gate(
    "provider_overflow",
    10,
    "Did a provider `context_overflow` error fail the turn as `llm.context_overflow`?",
)
.labeled("A provider overflow is classified as such");
const HEAL: AssessmentSpec = AssessmentSpec::gate(
    "provider_heals",
    15,
    "Did a provider whose stream function went away fail a turn and read unavailable, then serve again after re-registering?",
)
.labeled("A provider that comes back is used again");
const ASSESSMENTS: &[AssessmentSpec] = &[PAIRING, PRESTREAM, RESUME, STOP, OVERFLOW, HEAL];

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

fn case_session(run_id: &str, case: &str) -> String {
    format!("{}_fault_{case}", mechanism::session_id(run_id))
}

/// Stream calls and side effects the fake provider saw, per attempt.
#[derive(Default)]
struct Ledger {
    calls: HashMap<String, usize>,
    /// The `messages` of every stream request, per case.
    requests: HashMap<String, Vec<Value>>,
    /// Side-effect fixture runs, by the case that asked for them.
    effects: HashMap<String, usize>,
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
        Some("One plain reply; beside it the runner plays a model provider that rate-limits, cuts streams, overflows, hangs and disappears, and checks how each turn ends.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("When the model provider rate-limits, drops a stream, overflows, hangs or goes away, agent turns retry, resume, stop or fail cleanly, and come back when the provider does. If it fails, provider hiccups lose work, repeat tool side effects, hang turns, or leave a recovered provider unused.")
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
            let sent = send(context, &session, &fault, FAULT_MODEL, run_id).await;
            let status = match sent {
                Ok(_) if case == "stop" => stop_case(context, run_id, &session).await,
                Ok(_) => terminal(context, &session, TURN_WAIT).await,
                Err(error) => {
                    record(run_id, case, false, format!("harness::send: {error:#}"));
                    continue;
                }
            };
            let transcript = context.transcript(&session).await.unwrap_or(Value::Null);
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
                    "max_turns": 10,
                    "functions": { "allow": [effect_function(run_id)], "deny": [] },
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
            state == "cancelled"
                && !turn.is_empty()
                && ledger
                    .aborts
                    .iter()
                    .any(|request| request.starts_with(&format!("{turn}:")))
                && ledger.stop_ended.is_some()
        }
        "overflow" => {
            state == "failed" && errors.len() == 1 && errors[0]["class"] == "llm.context_overflow"
        }
        _ => false,
    };
    let detail = if case == "stop" {
        format!(
            "{detail} aborts={:?} stream_ended={}",
            ledger.aborts,
            ledger.stop_ended.is_some()
        )
    } else {
        detail
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
    let down_sent = send(context, &down_session, &heal, HEAL_MODEL, run_id).await;
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
    let up = match send(context, &up_session, &heal, HEAL_MODEL, run_id).await {
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
