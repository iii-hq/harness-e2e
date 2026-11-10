//! `restart_recovery` — a session and a held turn survive worker restarts.
//!
//! Phase 1, between the first two exchanges: the runner restarts
//! session-manager (`compose::restart`), and the session's meta and entries
//! read back byte-identical; the second exchange then continues on them.
//!
//! Phase 2, during the third exchange: the subject calls a run-scoped hold
//! that does not answer, and while it is held the runner restarts the Harness.
//! The new Harness announces `harness::ready` once, the held turn completes
//! within 240 s of the restart with one result per call and one turn
//! completion, and `session::get` reads `done` before the runner calls
//! `harness::status`, which would repair the status itself: the read runs in
//! `after_dialogue`, and counts only when the runner's wait ended on the
//! turn's terminal event rather than on a status sample. The hold fixture
//! counts its invocations, and they must equal the hold calls in the
//! transcript: a restart that ran the held call again adds an invocation
//! without a call.
//!
//! Inside that restart step, right before it restarts the Harness, the runner
//! plays one scripted turn on a side session of its own and arms two one-shot
//! bindings there, each calling a run-scoped sink: `state::on-changed` on a
//! key nobody wrote, and `harness::on-timer` due 20 s later. Their observers
//! die with the Harness's connection. The boot replay (`bindings/gc.rs`)
//! observes every surviving record again, a timer at its same instant, and
//! reads the key of a one-shot state binding that never fired, to deliver a
//! value written while nobody observed it. The runner writes the key when it
//! hears the restarted Harness's `harness::ready`, and the sink must count one
//! delivery of each binding. A run that never calls the hold arms nothing,
//! waits for nothing and leaves the check unevaluated; the subject's session
//! owns none of this.
//!
//! The ready, turn-completed and hold registrations are made before either
//! restart, so the run first checks that they survive one: a binding the
//! engine did not hand to the restarted Harness never hears `harness::ready`.
//!
//! The restarts touch the whole stack, so the test needs a stack group of its
//! own: every regression test has one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use iii_sdk::channel::{ChannelWriter, StreamChannelRef};
use iii_sdk::runtime::FunctionRef;
use iii_sdk::ObserverHandle;
use iii_sdk::{IIIClient, RegisterFunction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::context::E2eContext;
use crate::wire::{TurnCompletedPayload, TurnStatus};

use super::assessment::{self, AssessmentSpec};
use super::mechanism;
use super::{
    async_trait, common, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation,
    ProvenanceEvidence, Scenario, ScenarioCase, ScenarioExecutionKind, ScenarioObservation,
    ScenarioSpec,
};

pub const ID: &str = "restart_recovery";
const DELIVERABLE_ID: &str = "restart_record";
const SESSION_MANAGER: &str = "session-manager";
const HARNESS: &str = "harness";
/// The held turn must complete this long after the Harness restart.
const RECOVERY_LIMIT: Duration = Duration::from_secs(240);
const SESSION_MANAGER_RETURN: Duration = Duration::from_secs(120);
/// How long the projected status may take to land after the turn completes.
const STATUS_PROJECTION: Duration = Duration::from_secs(10);
const OBSERVE: &str = "harness::observe";
const STATE_CHANGED: &str = "state::on-changed";
const TIMER: &str = "harness::on-timer";
/// The key the state binding watches; the runner writes it after the restart.
const BINDING_KEY: &str = "binding";
/// Armed right before the restart and due after it: the recorded restarts
/// took about 3 s. One that outlasts the timer leaves the check unevaluated.
const TIMER_MS: u64 = 20_000;
/// How long arming may hold the restart back.
const ARM_LIMIT: Duration = Duration::from_secs(20);
/// How long after `harness::ready` and the timer's instant a delivery may
/// take. The boot replay starts at that ready, and a healthy one takes
/// milliseconds. Its waits are a store call (`session_timeout_ms`, 10 s by
/// default), 5 s to take over a held observer id (`REPLAY_READY_WAIT`,
/// `bindings/gc.rs`) and 5 s for a late timer's observer to become active
/// (`ACTIVE_WAIT`, `timer.rs`): one of each, and 10 s to spare.
const DELIVERY_LIMIT: Duration = Duration::from_secs(30);
const SIDE_MODEL: &str = "e2e-restart-model";

const STORE_SURVIVES: AssessmentSpec = AssessmentSpec::gate(
    "store_survives_restart",
    15,
    "After the first exchange wrote its row, did a session-manager restart leave the session's meta and entries byte-identical?",
)
.labeled("Session store unchanged by a session-manager restart");
const SESSION_CONTINUES: AssessmentSpec = AssessmentSpec::gate(
    "session_continues",
    10,
    "Did the exchanges before and after the session-manager restart both store their exact rows?",
)
.labeled("The session continues after the restart");
const READY_ONCE: AssessmentSpec = AssessmentSpec::gate(
    "ready_once",
    15,
    "Did a binding registered before the Harness restart hear harness::ready exactly once after it, with the hold function still registered?",
)
.labeled("harness::ready once after the restart");
const HELD_TURN_RECOVERS: AssessmentSpec = AssessmentSpec::gate(
    "held_turn_recovers",
    30,
    "Did the turn held across the Harness restart complete within 240 s of it, with one turn completion, one result per hold call, no hold call run twice, and the hold's token stored?",
)
.labeled("Held turn completes once after the restart");
const STATUS_DONE: AssessmentSpec = AssessmentSpec::gate(
    "status_done",
    15,
    "Did session::get read the session as done after the recovered turn, before harness::status could repair it?",
)
.labeled("Session status done without a repair");
const BINDINGS_REARMED: AssessmentSpec = AssessmentSpec::gate(
    "bindings_rearmed",
    15,
    "Did a one-shot state binding and a timer binding, armed on a runner-owned session right before the Harness restart, each call their target exactly once after the Harness was ready again?",
)
.labeled("Armed bindings deliver once after restart");
const ASSESSMENTS: &[AssessmentSpec] = &[
    STORE_SURVIVES,
    SESSION_CONTINUES,
    READY_ONCE,
    HELD_TURN_RECOVERS,
    STATUS_DONE,
    BINDINGS_REARMED,
];

fn suffix(run_id: &str) -> String {
    format!(
        "{:08x}",
        super::stable_seed(&format!("{ID}:{run_id}")) & 0xffff_ffff
    )
}

fn scope(run_id: &str) -> String {
    format!("e2e-restart-{run_id}")
}

fn row_key(phase: u8) -> String {
    format!("phase-{phase}")
}

fn token(run_id: &str, phase: u8) -> String {
    format!(
        "RST-{:012x}",
        super::stable_seed(&format!("{ID}:{run_id}:{phase}")) & 0xffff_ffff_ffff
    )
}

fn expected_row(run_id: &str, phase: u8) -> Value {
    json!({ "phase": phase, "token": token(run_id, phase) })
}

fn hold_function(run_id: &str) -> String {
    format!("e2etest::restart_hold_{}", suffix(run_id))
}

fn ready_sink(run_id: &str) -> String {
    format!("e2e::restart-ready::{}", suffix(run_id))
}

fn turn_sink(run_id: &str) -> String {
    format!("e2e::restart-turns::{}", suffix(run_id))
}

/// What the two bindings call. Not `e2e::`: the side session's policy must
/// be able to name it.
fn binding_sink(run_id: &str) -> String {
    format!("e2etest::restart_bind_{}", suffix(run_id))
}

/// The runner's own session, owner of the two bindings.
fn side_session(run_id: &str) -> String {
    format!("{}_bindings", mechanism::session_id(run_id))
}

fn side_provider(run_id: &str) -> String {
    format!("e2e-restart-{}", suffix(run_id))
}

/// Phase 1 as the runner saw it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct StoreCheck {
    meta_identical: bool,
    entries_identical: bool,
    /// The entries the restart had to keep hold the first exchange's write.
    wrote_first_row: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Completion {
    turn_id: String,
    status: TurnStatus,
    terminal: bool,
    after_restart_ms: u64,
}

/// What the probes heard, captured into the deliverable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct ProbeRecord {
    store: Option<StoreCheck>,
    /// The runner could not take phase 1's measurement: an infrastructure
    /// failure, not the Harness's.
    store_error: Option<String>,
    harness_restarted: bool,
    restart_error: Option<String>,
    ready_after_restart: usize,
    hold_invocations: usize,
    hold_listed_after_restart: Option<bool>,
    completions_after_restart: Vec<Completion>,
    status_after_restart: Option<String>,
    /// Both bindings were armed on the side session before the restart.
    bindings_armed: bool,
    /// The runner could not arm the bindings or write their key.
    bindings_error: Option<String>,
    state_deliveries: usize,
    /// The state delivery was the boot replay's own read of the key
    /// (`replayed`), not the observer's event. Either one counts.
    state_caught_up: bool,
    timer_deliveries: usize,
    /// The timer was delivered before the restarted Harness was ready: the
    /// process that armed it was still alive, so the restart tested nothing.
    timer_before_ready: bool,
}

#[derive(Default)]
struct Probe {
    record: ProbeRecord,
    root_session_id: Option<String>,
    restarted_at: Option<Instant>,
    /// When the runner heard the restarted Harness's first `harness::ready`.
    ready_at: Option<Instant>,
}

struct Runtime {
    functions: Vec<FunctionRef>,
    bindings: Vec<ObserverHandle>,
    probe: Arc<Mutex<Probe>>,
}

static RUNTIMES: OnceLock<Mutex<HashMap<String, Runtime>>> = OnceLock::new();

fn runtimes() -> &'static Mutex<HashMap<String, Runtime>> {
    RUNTIMES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn probe(run_id: &str) -> Option<Arc<Mutex<Probe>>> {
    common::lock_unpoisoned(runtimes())
        .get(run_id)
        .map(|runtime| Arc::clone(&runtime.probe))
}

fn release_runtime(run_id: &str) {
    if let Some(runtime) = common::lock_unpoisoned(runtimes()).remove(run_id) {
        runtime
            .bindings
            .into_iter()
            .for_each(crate::observe::release);
        runtime
            .functions
            .iter()
            .for_each(|function| function.unregister());
    }
}

/// `compose::restart` for one container, followed to its outcome when the
/// daemon only accepts the operation.
async fn restart(client: &IIIClient, container: &str) -> anyhow::Result<()> {
    let accepted = common::invoke(
        client,
        "compose::restart",
        json!({ "container": container }),
    )
    .await?;
    restart_outcome(container, &accepted)?;
    let Some(operation_id) = accepted.get("operation_id").and_then(Value::as_str) else {
        return Ok(());
    };
    for _ in 0..240 {
        let operation = common::invoke(
            client,
            "compose::operation",
            json!({ "operation_id": operation_id }),
        )
        .await?;
        match operation.get("status").and_then(Value::as_str) {
            Some("accepted" | "running") => tokio::time::sleep(Duration::from_millis(500)).await,
            _ => return restart_outcome(container, &operation),
        }
    }
    anyhow::bail!("compose::restart {container} did not finish in 120 s")
}

/// A `MutationOutcome` or a settled operation: `status: failed` is a restart
/// Compose refused or could not finish.
fn restart_outcome(container: &str, outcome: &Value) -> anyhow::Result<()> {
    if outcome.get("status").and_then(Value::as_str) != Some("failed") {
        return Ok(());
    }
    let detail = outcome
        .pointer("/error/message")
        .or_else(|| outcome.pointer("/last_event/detail"))
        .and_then(Value::as_str)
        .unwrap_or("no detail");
    anyhow::bail!("compose::restart {container} failed: {detail}")
}

/// The session's meta and entries, read once they stopped changing: the
/// status projection of the turn that just ended may still be landing.
async fn settled_store(context: &E2eContext, session_id: &str) -> anyhow::Result<(Value, Value)> {
    let read = || async {
        let meta = context
            .trigger_value("session::get", json!({ "session_id": session_id }))
            .await?;
        let entries = context.transcript(session_id).await?;
        anyhow::Ok((meta, entries))
    };
    let mut previous = read().await?;
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let current = read().await?;
        if current == previous {
            return Ok(current);
        }
        previous = current;
    }
    anyhow::bail!("session {session_id} kept changing for 10 s before the restart")
}

async fn store_check(
    context: &E2eContext,
    run_id: &str,
    session_id: &str,
) -> anyhow::Result<StoreCheck> {
    let (meta, entries) = settled_store(context, session_id).await?;
    restart(context.client(), SESSION_MANAGER).await?;
    let deadline = Instant::now() + SESSION_MANAGER_RETURN;
    let (meta_after, entries_after) = loop {
        let read = async {
            let meta = context
                .trigger_value("session::get", json!({ "session_id": session_id }))
                .await?;
            let entries = context.transcript(session_id).await?;
            anyhow::Ok((meta, entries))
        };
        match read.await {
            Ok(after) => break after,
            Err(error) if Instant::now() >= deadline => {
                return Err(error.context("session-manager did not come back after its restart"))
            }
            Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    };
    Ok(StoreCheck {
        meta_identical: serde_json::to_vec(&meta)? == serde_json::to_vec(&meta_after)?,
        entries_identical: serde_json::to_vec(&entries)? == serde_json::to_vec(&entries_after)?,
        wrote_first_row: first_row_written(&entries, run_id),
    })
}

/// Whether the entries hold the first exchange's `state::set`, judged by the
/// value state stored: a model may send the row as a JSON string.
fn first_row_written(entries: &Value, run_id: &str) -> bool {
    let row = expected_row(run_id, 1);
    common::function_outcomes(entries).iter().any(|outcome| {
        outcome.function_id == "state::set"
            && outcome.arguments.get("scope") == Some(&json!(scope(run_id)))
            && outcome.arguments.get("key") == Some(&json!(row_key(1)))
            && outcome
                .details
                .as_ref()
                .and_then(|details| details.get("new_value"))
                == Some(&row)
    })
}

/// How many results each hold call has in the transcript.
fn hold_results(transcript: &Value, hold: &str) -> Vec<usize> {
    let messages: Vec<&Value> = transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("message"))
        .collect();
    common::function_invocations(transcript)
        .into_iter()
        .filter(|invocation| invocation.call.function_id == hold)
        .map(|invocation| {
            messages
                .iter()
                .filter(|message| {
                    message.get("role").and_then(Value::as_str) == Some("function_result")
                        && message.get("function_call_id").and_then(Value::as_str)
                            == invocation.call_id.as_deref()
                })
                .count()
        })
        .collect()
}

/// The two `harness::observe` requests: one-shot call bindings on the sink.
fn binding_requests(run_id: &str) -> [Value; 2] {
    let target =
        |tag: &str| json!({ "function_id": binding_sink(run_id), "payload": { "tag": tag } });
    [
        json!({
            "function_id": STATE_CHANGED,
            "filters": {
                "input.scope": { "$eq": scope(run_id) }, "input.key": { "$eq": BINDING_KEY },
            },
            "target": target("state"),
            "once": true,
        }),
        json!({
            "function_id": TIMER,
            "args": { "in_ms": TIMER_MS },
            "target": target("timer"),
            "once": true,
        }),
    ]
}

/// What the side session's provider streams for any request: `DONE`.
fn done_frames(provider: &str, model: &Value) -> Vec<Value> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64);
    let message = |content: Value| {
        json!({
            "role": "assistant",
            "content": content,
            "stop_reason": "end",
            "model": model,
            "provider": provider,
            "timestamp": timestamp,
        })
    };
    let done = message(json!([{ "type": "text", "text": "DONE" }]));
    vec![
        json!({ "type": "start", "partial": message(json!([])) }),
        json!({ "type": "text_start", "partial": message(json!([])) }),
        json!({ "type": "text_delta", "delta": "DONE" }),
        json!({ "type": "text_end", "partial": done }),
        json!({ "type": "done", "message": done }),
    ]
}

/// The side session's provider, played by the runner so no model stands
/// between the restart and the bindings.
/// shortcut: a third copy of `steer_and_stop`'s scripted stream, and the
/// router keeps `e2e-restart-<run>` after the run; share one in `mechanism`
/// when the three scenarios can change together.
fn register_side_stream(client: &IIIClient, run_id: &str) -> FunctionRef {
    let address = client.address().to_string();
    let provider = side_provider(run_id);
    client.register_function(
        format!("provider::{provider}::stream"),
        RegisterFunction::new_async(move |input: Value| {
            let (address, provider) = (address.clone(), provider.clone());
            async move {
                let writer_ref: StreamChannelRef =
                    serde_json::from_value(input["writer_ref"].clone())
                        .map_err(|error| iii_sdk::errors::Error::Serde(error.to_string()))?;
                let writer = ChannelWriter::new(&address, &writer_ref);
                for frame in done_frames(&provider, &input["model"]) {
                    writer
                        .send_message(&frame.to_string())
                        .await
                        .map_err(|error| iii_sdk::errors::Error::Handler(error.to_string()))?;
                }
                let _ = writer.close().await;
                Ok::<Value, iii_sdk::errors::Error>(json!({ "ok": true }))
            }
        })
        .description("E2E scripted side-session provider stream."),
    )
}

/// One scripted turn on the side session, for the turn record whose policy
/// `harness::function::call` reads, then the two bindings armed as it.
async fn arm_bindings(client: &IIIClient, run_id: &str) -> anyhow::Result<()> {
    let (session_id, provider) = (side_session(run_id), side_provider(run_id));
    mechanism::call(
        client,
        "router::provider::register",
        json!({
            "id": provider,
            "display_name": "E2E restart_recovery side session",
            "models": [{
                "id": SIDE_MODEL,
                "provider": provider,
                "context_window": 100_000,
                "max_output_tokens": 4_096,
                "supports_tools": true,
            }],
        }),
    )
    .await?;
    // Enrolled before it exists, so cleanup deletes it whatever fails after.
    mechanism::keep_session(run_id, &session_id);
    mechanism::call(
        client,
        "harness::send",
        json!({
            "session_id": session_id,
            "message": "Reply with exactly `DONE`.",
            "model": SIDE_MODEL,
            "provider": provider,
            "session": {
                "kind": crate::wire::E2E_SESSION_KIND,
                "title": "Harness E2E: restart_recovery bindings",
            },
            "options": {
                "max_turns": 1,
                "functions": {
                    "allow": [OBSERVE, STATE_CHANGED, TIMER, binding_sink(run_id)],
                    "deny": [],
                },
            },
        }),
    )
    .await?;
    let completed = mechanism::poll(ARM_LIMIT, || async {
        let status = mechanism::call(
            client,
            "harness::status",
            json!({ "session_id": session_id }),
        )
        .await
        .ok()?;
        (status["status"] == "completed").then_some(())
    })
    .await;
    anyhow::ensure!(
        completed.is_some(),
        "the side session's scripted turn did not complete"
    );
    for request in binding_requests(run_id) {
        let answer = mechanism::call(
            client,
            "harness::function::call",
            json!({
                "session_id": session_id,
                "call": { "id": "e2e-restart-binding", "function_id": OBSERVE, "arguments": request },
            }),
        )
        .await?;
        anyhow::ensure!(
            answer.get("is_error") != Some(&json!(true))
                && answer
                    .pointer("/details/observer_id")
                    .is_some_and(Value::is_string),
            "{OBSERVE} did not arm a binding: {answer}"
        );
    }
    Ok(())
}

/// Waits for the two deliveries of a run that armed the bindings and heard
/// the restarted Harness, then a second more for a repeat. Any other run
/// returns at once.
async fn await_bindings(probe: &Mutex<Probe>) {
    let deadline = {
        let probe = common::lock_unpoisoned(probe);
        match probe.ready_at {
            Some(ready) if probe.record.bindings_armed => {
                ready + Duration::from_millis(TIMER_MS) + DELIVERY_LIMIT
            }
            _ => return,
        }
    };
    let delivered = || {
        let probe = common::lock_unpoisoned(probe);
        probe.record.state_deliveries >= 1 && probe.record.timer_deliveries >= 1
    };
    while !delivered() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
}

/// Whether each binding armed before the Harness restart delivered once
/// after it; `Err` says why this run could not tell.
fn bindings_survive(probe: &ProbeRecord) -> Result<bool, String> {
    if !probe.harness_restarted {
        return Err("the run never reached the Harness restart".into());
    }
    if !probe.bindings_armed || probe.bindings_error.is_some() {
        return Err(format!(
            "the runner could not arm or fire the bindings: {:?}",
            probe.bindings_error
        ));
    }
    if probe.ready_after_restart == 0 {
        return Err(
            "the restarted Harness never announced ready, so the key was never written".into(),
        );
    }
    if probe.state_deliveries != 1 || probe.timer_deliveries > 1 {
        return Ok(false);
    }
    if probe.timer_before_ready {
        return Err(
            "the timer fired before the restarted Harness was ready: the restart outlasted it"
                .into(),
        );
    }
    Ok(probe.timer_deliveries == 1)
}

fn evaluation(record: &Value, run_id: &str) -> anyhow::Result<ObjectiveEvaluation> {
    let probe: ProbeRecord = serde_json::from_value(record["probe"].clone())?;
    if let Some(error) = probe.store_error.as_ref().or(probe.restart_error.as_ref()) {
        let mut evaluation =
            assessment::prerequisite_failure(ASSESSMENTS, "restart", error.clone());
        evaluation.infrastructure_error = Some(format!(
            "restart_recovery could not restart a worker: {error}"
        ));
        return Ok(evaluation);
    }
    let row = |phase: u8| record["rows"][row_key(phase)] == expected_row(run_id, phase);
    let store = probe.store.clone().unwrap_or_default();
    let store_survives = store.wrote_first_row && store.meta_identical && store.entries_identical;

    let ready_once = probe.harness_restarted
        && probe.ready_after_restart == 1
        && probe.hold_listed_after_restart == Some(true);

    let hold_results: Vec<usize> = serde_json::from_value(record["hold_results"].clone())?;
    let terminal: Vec<&Completion> = probe
        .completions_after_restart
        .iter()
        .filter(|completion| completion.terminal)
        .collect();
    let completed_in_time = matches!(terminal.as_slice(), [completion]
        if completion.status == TurnStatus::Completed
            && completion.after_restart_ms <= RECOVERY_LIMIT.as_millis() as u64);
    let one_result_each =
        !hold_results.is_empty() && hold_results.iter().all(|results| *results == 1);
    // One invocation per call in the transcript: the restart re-ran none.
    let no_call_rerun = probe.hold_invocations == hold_results.len();
    let recovered =
        probe.harness_restarted && completed_in_time && one_result_each && no_call_rerun && row(3);

    // Only a wait that ended on the terminal event reached `after_dialogue`
    // without a `harness::status` sample of the finished turn, which would
    // have re-projected the status itself.
    let status_done = probe.harness_restarted
        && !terminal.is_empty()
        && probe.status_after_restart.as_deref() == Some("done");

    let delivered = format!(
        "bindings_armed={}, error={:?}, state_deliveries={}, state_caught_up={}, \
         timer_deliveries={}, timer_before_ready={}",
        probe.bindings_armed,
        probe.bindings_error,
        probe.state_deliveries,
        probe.state_caught_up,
        probe.timer_deliveries,
        probe.timer_before_ready
    );
    let bindings = match bindings_survive(&probe) {
        Ok(once_each) => BINDINGS_REARMED.full_or_zero(once_each, delivered),
        Err(why) => BINDINGS_REARMED.unverified(format!("not evaluated: {why}; {delivered}")),
    };

    Ok(assessment::build_evaluation([
        STORE_SURVIVES.full_or_zero(
            store_survives,
            format!(
                "first_row_written={}, meta_identical={}, entries_identical={}",
                store.wrote_first_row, store.meta_identical, store.entries_identical
            ),
        ),
        SESSION_CONTINUES.full_or_zero(
            row(1) && row(2),
            format!("phase_1_row={}, phase_2_row={}", row(1), row(2)),
        ),
        READY_ONCE.full_or_zero(
            ready_once,
            format!(
                "harness_restarted={}, ready_after_restart={}, hold_listed_after_restart={:?}",
                probe.harness_restarted, probe.ready_after_restart, probe.hold_listed_after_restart
            ),
        ),
        HELD_TURN_RECOVERS.full_or_zero(
            recovered,
            format!(
                "terminal_completions_after_restart={:?}, results_per_hold_call={hold_results:?}, \
                 hold_invocations={}, phase_3_row={}",
                terminal
                    .iter()
                    .map(|completion| (completion.status, completion.after_restart_ms))
                    .collect::<Vec<_>>(),
                probe.hold_invocations,
                row(3)
            ),
        ),
        STATUS_DONE.full_or_zero(
            status_done,
            format!(
                "session_get_status={:?}, terminal_event_heard={}",
                probe.status_after_restart,
                !terminal.is_empty()
            ),
        ),
        bindings,
    ]))
}

fn register_probes(context: &E2eContext, run_id: &str) -> anyhow::Result<Runtime> {
    let probe = Arc::new(Mutex::new(Probe::default()));
    let release = Arc::new(Notify::new());
    let client = context.client().clone();
    let hold = hold_function(run_id);
    let mut functions = Vec::new();

    // The hold answers its first call only once the restarted Harness is
    // ready (or the recovery limit passed); a redriven or repeated call
    // answers at once.
    let (held, released, restarter, answer, run) = (
        Arc::clone(&probe),
        Arc::clone(&release),
        client.clone(),
        token(run_id, 3),
        run_id.to_string(),
    );
    functions.push(
        context.client().register_function(
            hold.clone(),
            RegisterFunction::new_async(move |_arguments: Value| {
                let (held, released, restarter, answer, run) = (
                    Arc::clone(&held),
                    Arc::clone(&released),
                    restarter.clone(),
                    answer.clone(),
                    run.clone(),
                );
                async move {
                    let first = {
                        let mut probe = common::lock_unpoisoned(&held);
                        probe.record.hold_invocations += 1;
                        let first = probe.restarted_at.is_none();
                        if first {
                            probe.restarted_at = Some(Instant::now());
                            probe.record.harness_restarted = true;
                        }
                        first
                    };
                    if first {
                        // Armed here and nowhere earlier: a run that never
                        // calls the hold arms nothing.
                        let armed = tokio::time::timeout(ARM_LIMIT, arm_bindings(&restarter, &run))
                            .await
                            .unwrap_or_else(|_| {
                                Err(anyhow::anyhow!("arming took over {ARM_LIMIT:?}"))
                            });
                        {
                            let mut probe = common::lock_unpoisoned(&held);
                            probe.record.bindings_armed = armed.is_ok();
                            probe.record.bindings_error =
                                armed.err().map(|error| format!("{error:#}"));
                            // The recovery limit runs from the restart itself.
                            probe.restarted_at = Some(Instant::now());
                        }
                        let (failed, unblocked) = (Arc::clone(&held), Arc::clone(&released));
                        tokio::spawn(async move {
                            if let Err(error) = restart(&restarter, HARNESS).await {
                                common::lock_unpoisoned(&failed).record.restart_error =
                                    Some(format!("{error:#}"));
                                unblocked.notify_one();
                            }
                        });
                        let _ = tokio::time::timeout(RECOVERY_LIMIT, released.notified()).await;
                    }
                    Ok::<Value, iii_sdk::errors::Error>(json!({ "token": answer }))
                }
            })
            .description(
                "E2E run-scoped hold. Input: {}. Answers with a token after a while; wait for it.",
            ),
        ),
    );

    let (heard, released, lister, hold_id, watched) = (
        Arc::clone(&probe),
        Arc::clone(&release),
        client.clone(),
        hold.clone(),
        scope(run_id),
    );
    let ready_id = ready_sink(run_id);
    functions.push(
        context.client().register_function(
            ready_id.clone(),
            RegisterFunction::new_async(move |_payload: Value| {
                let (heard, released, lister, hold_id, watched) = (
                    Arc::clone(&heard),
                    Arc::clone(&released),
                    lister.clone(),
                    hold_id.clone(),
                    watched.clone(),
                );
                async move {
                    let (first_after_restart, armed) = {
                        let mut probe = common::lock_unpoisoned(&heard);
                        if probe.restarted_at.is_some() {
                            probe.record.ready_after_restart += 1;
                        }
                        let first =
                            probe.record.ready_after_restart == 1 && probe.restarted_at.is_some();
                        if first {
                            probe.ready_at = Some(Instant::now());
                        }
                        (first, probe.record.bindings_armed)
                    };
                    if first_after_restart && armed {
                        // The state binding's event, written only now: whatever
                        // delivers it is the restarted Harness.
                        let (writer, failed) = (lister.clone(), Arc::clone(&heard));
                        tokio::spawn(async move {
                            let written = common::invoke(
                                &writer,
                                "state::set",
                                json!({ "scope": watched, "key": BINDING_KEY, "value": true }),
                            )
                            .await;
                            if let Err(error) = written {
                                common::lock_unpoisoned(&failed).record.bindings_error =
                                    Some(format!("{error:#}"));
                            }
                        });
                    }
                    if first_after_restart {
                        tokio::spawn(async move {
                            let listed = common::invoke(
                                &lister,
                                "engine::functions::list",
                                json!({ "include_internal": true }),
                            )
                            .await
                            .ok()
                            .map(|listed| listed.to_string().contains(&format!("\"{hold_id}\"")));
                            common::lock_unpoisoned(&heard)
                                .record
                                .hold_listed_after_restart = listed;
                            released.notify_one();
                        });
                    }
                    Ok::<Value, iii_sdk::errors::Error>(json!({ "ok": true }))
                }
            })
            .description("Counts harness::ready for the E2E restart test.")
            .metadata(json!({ "internal": true })),
        ),
    );

    let heard = Arc::clone(&probe);
    let turns_id = turn_sink(run_id);
    functions.push(
        context.client().register_function(
            turns_id.clone(),
            RegisterFunction::new_async(move |payload: Value| {
                let heard = Arc::clone(&heard);
                async move {
                    let Ok(event) = serde_json::from_value::<TurnCompletedPayload>(payload) else {
                        return Ok::<Value, iii_sdk::errors::Error>(json!({ "ok": false }));
                    };
                    let mut probe = common::lock_unpoisoned(&heard);
                    let root = probe.root_session_id.as_deref() == Some(event.session_id.as_str());
                    if let Some(restarted_at) = probe.restarted_at.filter(|_| root) {
                        probe.record.completions_after_restart.push(Completion {
                            turn_id: event.turn_id,
                            status: event.status,
                            terminal: event.terminal,
                            after_restart_ms: restarted_at.elapsed().as_millis() as u64,
                        });
                    }
                    Ok(json!({ "ok": true }))
                }
            })
            .description("Records the root's turn completions for the E2E restart test.")
            .metadata(json!({ "internal": true })),
        ),
    );

    let delivered = Arc::clone(&probe);
    functions.push(
        context.client().register_function(
            binding_sink(run_id),
            RegisterFunction::new_async(move |delivery: Value| {
                let delivered = Arc::clone(&delivered);
                async move {
                    let mut probe = common::lock_unpoisoned(&delivered);
                    let before_ready = probe.record.ready_after_restart == 0;
                    match delivery["tag"].as_str() {
                        Some("state") => {
                            probe.record.state_deliveries += 1;
                            probe.record.state_caught_up |=
                                delivery.pointer("/event/replayed") == Some(&json!(true));
                        }
                        Some("timer") => {
                            probe.record.timer_deliveries += 1;
                            probe.record.timer_before_ready |= before_ready;
                        }
                        _ => {}
                    }
                    Ok::<Value, iii_sdk::errors::Error>(json!({ "received": true }))
                }
            })
            .description("Counts the binding deliveries of the E2E restart test.")
            .metadata(json!({ "internal": true })),
        ),
    );
    functions.push(register_side_stream(context.client(), run_id));

    let bindings = vec![
        context
            .client()
            .observe(crate::observe::event("harness::on-ready"), ready_id),
        context.client().observe(
            crate::observe::event(crate::observe::TURN_COMPLETED_TRIGGER),
            turns_id,
        ),
    ];
    Ok(Runtime {
        functions,
        bindings,
        probe,
    })
}

pub struct RestartRecovery;

#[async_trait]
impl Scenario for RestartRecovery {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Recovery across worker restarts")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Store a row, survive a session-manager restart and store another, then wait on a held call while the Harness restarts and store the token it returns.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("Sessions and in-flight turns survive restarts of session-manager and the Harness: the transcript is kept as it was, the Harness comes back ready once, a turn held across the restart finishes once without running its call again, its session is marked done, and bindings armed before the restart still deliver once. If it fails, a deploy or a crash loses conversations, repeats a call's side effects, leaves turns hanging or sessions stuck as working, or drops the triggers and timers a session was waiting on.")
    }

    fn execution_kind(&self) -> ScenarioExecutionKind {
        ScenarioExecutionKind::ScriptedDialogue
    }

    fn case(&self, seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "restarted": [SESSION_MANAGER, HARNESS],
                "recovery_limit_seconds": RECOVERY_LIMIT.as_secs(),
                "rows": [row_key(1), row_key(2), row_key(3)],
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::HarnessScriptedDialogueV1,
                Capability::IiiCompose,
                Capability::IiiFunctions,
                Capability::IiiState,
                Capability::IiiTriggers,
                Capability::E2eRunScopedFixtures,
            ],
            super::validation_loop::validation_contract(
                DELIVERABLE_ID,
                "restart_record",
                json!({
                    "type": "object",
                    "required": ["probe", "rows", "hold_results"],
                    "additionalProperties": true
                }),
            ),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let scope = scope(run_id);
        let first = expected_row(run_id, 1);
        ScenarioSpec {
            id: ID,
            prompt: format!(
                "Write exactly `{first}` with `state::set` to scope `{scope}`, key `{key}`. Then \
                 reply with one line containing `RESTART-1`. Make no other function call except \
                 function discovery if it is necessary.",
                key = row_key(1),
            ),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(16),
                max_output_tokens: Some(4_096),
                max_total_tokens: Some(200_000),
                // The held exchange waits out a Harness restart.
                stuck_timeout_seconds: 420,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    fn dialogue_followups(&self, run_id: &str) -> Vec<String> {
        let scope = scope(run_id);
        vec![
            format!(
                "Write exactly `{second}` to scope `{scope}`, key `{key}`. Then reply with one \
                 line containing `RESTART-2`.",
                second = expected_row(run_id, 2),
                key = row_key(2),
            ),
            format!(
                "Call `{hold}` once with `{{}}`. It answers with a token after a while; wait for \
                 it, and if the call fails, call it once more. Then write exactly \
                 `{{\"phase\": 3, \"token\": \"<the token it returned>\"}}` to scope `{scope}`, \
                 key `{key}`, and reply with one line containing `RESTART-3`.",
                hold = hold_function(run_id),
                key = row_key(3),
            ),
        ]
    }

    /// A stack Compose did not start cannot restart its workers: a setup
    /// failure, not a Harness that did not recover.
    fn required_functions(&self, _run_id: &str) -> Vec<String> {
        vec!["compose::restart".to_string()]
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        release_runtime(run_id);
        let runtime = register_probes(context, run_id)?;
        common::lock_unpoisoned(runtimes()).insert(run_id.to_string(), runtime);
        Ok(())
    }

    async fn before_followup(
        &self,
        context: &E2eContext,
        run_id: &str,
        session_id: &str,
        exchange: usize,
    ) {
        let Some(probe) = probe(run_id) else {
            return;
        };
        common::lock_unpoisoned(&probe).root_session_id = Some(session_id.to_string());
        if exchange != 0 {
            return;
        }
        let checked = store_check(context, run_id, session_id).await;
        let mut probe = common::lock_unpoisoned(&probe);
        match checked {
            Ok(check) => probe.record.store = Some(check),
            Err(error) => probe.record.store_error = Some(format!("{error:#}")),
        }
    }

    /// Reads the status the Harness projected for the recovered turn before
    /// the runner's `harness::status`, which repairs a terminal turn's
    /// projection and so would hide one the restart lost.
    async fn after_dialogue(&self, context: &E2eContext, run_id: &str, session_id: &str) {
        let Some(probe) = probe(run_id) else {
            return;
        };
        let heard_terminal = common::lock_unpoisoned(&probe)
            .record
            .completions_after_restart
            .iter()
            .any(|completion| completion.terminal);
        if !heard_terminal {
            return;
        }
        let deadline = Instant::now() + STATUS_PROJECTION;
        let status = loop {
            let status = context
                .trigger_value("session::get", json!({ "session_id": session_id }))
                .await
                .ok()
                .and_then(|got| {
                    got.pointer("/meta/status")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            if status.as_deref().is_some_and(|status| status != "working")
                || Instant::now() >= deadline
            {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        };
        common::lock_unpoisoned(&probe).record.status_after_restart = status;
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let probe = probe(run_id).context("restart_recovery has no probes for this run")?;
        await_bindings(&probe).await;
        let probe = common::lock_unpoisoned(&probe).record.clone();
        let scope = scope(run_id);
        let mut rows = serde_json::Map::new();
        for phase in 1..=3 {
            rows.insert(
                row_key(phase),
                common::get_state(context, &scope, &row_key(phase)).await?,
            );
        }
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "restart_record".to_string(),
            content: json!({
                "probe": probe,
                "rows": rows,
                "hold_results": hold_results(&observation.transcript, &hold_function(run_id)),
            })
            .into(),
            invariants: Vec::new(),
            provenance: vec![
                ProvenanceEvidence {
                    kind: "state_location".to_string(),
                    source_id: scope,
                    relation: "captured_phase_rows".to_string(),
                },
                ProvenanceEvidence {
                    kind: "session".to_string(),
                    source_id: observation.metrics.root_session_id.clone(),
                    relation: "captured_restart_probes".to_string(),
                },
            ],
        }])
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
        let record = observation
            .deliverables
            .iter()
            .find(|deliverable| deliverable.id == DELIVERABLE_ID)
            .and_then(|deliverable| deliverable.content.as_json())
            .context("restart_recovery captured no restart record")?;
        evaluation(record, run_id)
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        release_runtime(run_id);
        // The side session, when the run reached the restart and made one.
        let side = mechanism::release(context, run_id).await;
        let scope = scope(run_id);
        for key in (1..=3).map(row_key).chain([BINDING_KEY.to_string()]) {
            let _: Value = context
                .trigger_value("state::delete", json!({ "scope": scope, "key": key }))
                .await?;
        }
        side
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `deliverable` as `capture` records it, scored by `evaluate`.
    async fn evaluated(
        scenario: &dyn Scenario,
        id: crate::scenarios::ScenarioId,
        deliverable: Value,
        response: &str,
    ) -> ObjectiveEvaluation {
        let context = E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let observation = ScenarioObservation::for_test(
            id,
            "control",
            json!({ "messages": [] }),
            response,
            vec![CapturedDeliverable {
                id: DELIVERABLE_ID.to_string(),
                kind: "record".to_string(),
                content: deliverable.into(),
                invariants: Vec::new(),
                provenance: Vec::new(),
            }],
        );
        scenario
            .evaluate(&context, &observation, "control")
            .await
            .unwrap()
    }

    /// The null delivery (the runner still restarts session-manager) and a
    /// sound stack.
    pub(crate) async fn controls() -> [ObjectiveEvaluation; 2] {
        let id = crate::scenarios::ScenarioId::RestartRecovery;
        [
            evaluated(&RestartRecovery, id, null(), "done").await,
            evaluated(&RestartRecovery, id, reference(), "RESTART-3").await,
        ]
    }

    fn null() -> Value {
        let probe = ProbeRecord {
            store: Some(StoreCheck {
                meta_identical: true,
                entries_identical: true,
                wrote_first_row: false,
            }),
            ..ProbeRecord::default()
        };
        json!({ "probe": probe, "rows": {}, "hold_results": [] })
    }

    const RUN: &str = "control";

    fn completion(status: TurnStatus, after_restart_ms: u64) -> Completion {
        Completion {
            turn_id: "turn-3".into(),
            status,
            terminal: true,
            after_restart_ms,
        }
    }

    /// The run a sound stack leaves behind.
    fn reference() -> Value {
        let probe = ProbeRecord {
            store: Some(StoreCheck {
                meta_identical: true,
                entries_identical: true,
                wrote_first_row: true,
            }),
            store_error: None,
            harness_restarted: true,
            restart_error: None,
            ready_after_restart: 1,
            hold_invocations: 2,
            hold_listed_after_restart: Some(true),
            completions_after_restart: vec![completion(TurnStatus::Completed, 95_000)],
            status_after_restart: Some("done".into()),
            bindings_armed: true,
            bindings_error: None,
            state_deliveries: 1,
            state_caught_up: true,
            timer_deliveries: 1,
            timer_before_ready: false,
        };
        json!({
            "probe": probe,
            "rows": {
                "phase-1": expected_row(RUN, 1),
                "phase-2": expected_row(RUN, 2),
                "phase-3": expected_row(RUN, 3),
            },
            // The held call closes as interrupted and the subject calls again.
            "hold_results": [1, 1],
        })
    }

    fn awarded(record: &Value) -> Vec<Option<u8>> {
        evaluation(record, RUN)
            .unwrap()
            .awards
            .iter()
            .map(|award| award.awarded)
            .collect()
    }

    #[test]
    fn a_sound_stack_earns_every_point() {
        assert_eq!(
            awarded(&reference()),
            [Some(15), Some(10), Some(15), Some(30), Some(15), Some(15)]
        );
    }

    /// The runner restarts session-manager whatever the subject does, so the
    /// store gate also needs the subject's first write. It never reaches the
    /// Harness restart, so nothing was armed and the bindings are not judged.
    #[test]
    fn a_null_agent_earns_nothing() {
        let mut expected = vec![Some(0); 5];
        expected.push(None);
        assert_eq!(awarded(&null()), expected);
    }

    #[test]
    fn a_lost_or_a_doubled_delivery_fails_the_bindings_gate() {
        for binding in ["state_deliveries", "timer_deliveries"] {
            for deliveries in [0, 2] {
                let mut record = reference();
                record["probe"][binding] = json!(deliveries);
                assert_eq!(awarded(&record)[5], Some(0), "{binding}={deliveries}");
            }
        }
        // Whichever of the replay's read and the observer delivered it.
        let mut by_observer = reference();
        by_observer["probe"]["state_caught_up"] = json!(false);
        assert_eq!(awarded(&by_observer)[5], Some(15));
    }

    /// A run that never put the bindings through a restart says nothing
    /// about them: no points, and no failure either.
    #[test]
    fn a_run_that_could_not_test_the_bindings_leaves_them_unevaluated() {
        let unevaluated = |change: fn(&mut Value)| {
            let mut record = reference();
            change(&mut record["probe"]);
            awarded(&record)[5]
        };
        assert_eq!(
            unevaluated(|probe| probe["harness_restarted"] = json!(false)),
            None
        );
        assert_eq!(
            unevaluated(|probe| probe["bindings_armed"] = json!(false)),
            None
        );
        assert_eq!(
            unevaluated(|probe| probe["bindings_error"] = json!("state::set timed out")),
            None
        );
        assert_eq!(
            unevaluated(|probe| probe["ready_after_restart"] = json!(0)),
            None
        );
        // The old process outlived the timer and fired it itself.
        assert_eq!(
            unevaluated(|probe| probe["timer_before_ready"] = json!(true)),
            None
        );
        // Still a failure: the state binding had its restart and lost it,
        // and a timer delivered twice is twice whoever fired first.
        assert_eq!(
            unevaluated(|probe| {
                probe["timer_before_ready"] = json!(true);
                probe["state_deliveries"] = json!(0);
            }),
            Some(0)
        );
        assert_eq!(
            unevaluated(|probe| {
                probe["timer_before_ready"] = json!(true);
                probe["timer_deliveries"] = json!(2);
            }),
            Some(0)
        );
    }

    #[test]
    fn the_bindings_are_one_shot_calls_of_the_sink() {
        let [state, timer] = binding_requests(RUN);
        for (request, tag) in [(&state, "state"), (&timer, "timer")] {
            assert_eq!(request["once"], json!(true));
            assert_eq!(request["target"]["function_id"], json!(binding_sink(RUN)));
            assert_eq!(request["target"]["payload"]["tag"], json!(tag));
        }
        // The `$eq` pair the boot replay reads the watched key from.
        assert_eq!(state["filters"]["input.scope"]["$eq"], json!(scope(RUN)));
        assert_eq!(state["filters"]["input.key"]["$eq"], json!(BINDING_KEY));
        assert_eq!(timer["args"], json!({ "in_ms": TIMER_MS }));
        assert!(!binding_sink(RUN).starts_with("harness::"));
        assert_ne!(side_session(RUN), mechanism::session_id(RUN));
    }

    #[test]
    fn a_changed_entry_fails_the_store_gate() {
        let mut record = reference();
        record["probe"]["store"]["entries_identical"] = json!(false);
        assert_eq!(awarded(&record)[0], Some(0));
    }

    #[test]
    fn a_second_ready_or_a_lost_binding_fails_the_ready_gate() {
        for ready in [0, 2] {
            let mut record = reference();
            record["probe"]["ready_after_restart"] = json!(ready);
            assert_eq!(awarded(&record)[2], Some(0), "ready={ready}");
        }
    }

    #[test]
    fn a_late_a_doubled_or_a_failed_completion_fails_the_recovery_gate() {
        let late = vec![completion(TurnStatus::Completed, 241_000)];
        let doubled = vec![
            completion(TurnStatus::Completed, 90_000),
            completion(TurnStatus::Completed, 91_000),
        ];
        let failed = vec![completion(TurnStatus::Failed, 90_000)];
        for completions in [late, doubled, failed] {
            let mut record = reference();
            record["probe"]["completions_after_restart"] = json!(completions);
            assert_eq!(awarded(&record)[3], Some(0), "{completions:?}");
        }
        let mut two_results = reference();
        two_results["hold_results"] = json!([2, 1]);
        assert_eq!(awarded(&two_results)[3], Some(0));
    }

    /// A Harness that ran the held call again instead of closing it: one call
    /// in the transcript, two invocations at the fixture.
    #[test]
    fn a_hold_call_run_twice_fails_the_recovery_gate() {
        let mut record = reference();
        record["hold_results"] = json!([1]);
        assert_eq!(awarded(&record)[3], Some(0));
    }

    #[test]
    fn a_session_left_working_fails_the_status_gate() {
        let mut record = reference();
        record["probe"]["status_after_restart"] = json!("working");
        assert_eq!(awarded(&record)[4], Some(0));
    }

    /// Without the terminal event the runner's wait ended on a
    /// `harness::status` sample, which re-projects the status itself.
    #[test]
    fn a_status_read_without_the_terminal_event_fails_the_status_gate() {
        let mut record = reference();
        record["probe"]["completions_after_restart"] = json!([]);
        assert_eq!(awarded(&record)[4], Some(0));
    }

    #[test]
    fn a_restart_compose_refused_is_an_error() {
        let failed = json!({
            "changed": false,
            "status": "failed",
            "error": { "code": "unknown_container", "message": "no container session-manager" },
        });
        let error = restart_outcome(SESSION_MANAGER, &failed).unwrap_err();
        assert!(error.to_string().contains("no container session-manager"));
        let operation = json!({ "status": "failed", "last_event": { "detail": "exit 1" } });
        assert!(restart_outcome(HARNESS, &operation).is_err());
        assert!(restart_outcome(HARNESS, &json!({ "changed": true, "status": "ok" })).is_ok());
    }

    #[test]
    fn a_restart_the_runner_could_not_make_is_an_infrastructure_error() {
        let mut record = reference();
        record["probe"]["store_error"] = json!("compose::restart session-manager failed");
        let evaluation = evaluation(&record, RUN).unwrap();
        assert!(evaluation.infrastructure_error.is_some());
    }

    /// Seen live: the value sent as a JSON string, stored as the object.
    #[test]
    fn the_first_row_counts_by_what_state_stored() {
        let row = expected_row(RUN, 1);
        let entries = |value: Value, stored: Value| {
            json!({ "messages": [
                { "message": { "role": "assistant", "content": [{
                    "type": "function_call", "id": "call-0", "function_id": "agent_trigger",
                    "arguments": { "function": "state::set", "payload": {
                        "scope": scope(RUN), "key": row_key(1), "value": value,
                    } },
                }] } },
                { "message": {
                    "role": "function_result", "function_call_id": "call-0",
                    "function_id": "state::set", "is_error": false,
                    "details": { "new_value": stored, "old_value": null },
                } },
            ] })
        };
        assert!(first_row_written(&entries(row.clone(), row.clone()), RUN));
        assert!(first_row_written(
            &entries(json!(row.to_string()), row.clone()),
            RUN
        ));
        assert!(!first_row_written(
            &entries(row.clone(), json!({ "phase": 1 })),
            RUN
        ));
    }

    #[test]
    fn hold_results_count_the_results_of_each_hold_call() {
        let hold = hold_function(RUN);
        let transcript = json!({ "messages": [
            { "message": { "role": "assistant", "content": [{
                "type": "function_call", "id": "call-0", "function_id": hold, "arguments": {},
            }] } },
            { "message": { "role": "function_result", "function_call_id": "call-0", "function_id": hold, "is_error": false } },
            { "message": { "role": "assistant", "content": [{
                "type": "function_call", "id": "call-1", "function_id": "state::set", "arguments": {},
            }] } },
        ] });
        assert_eq!(hold_results(&transcript, &hold), [1]);
    }

    #[test]
    fn the_dialogue_names_its_hold_and_rows() {
        let spec = RestartRecovery.spec(RUN);
        spec.validate().unwrap();
        let followups = RestartRecovery.dialogue_followups(RUN);
        assert_eq!(followups.len(), 2);
        assert!(followups[1].contains(&hold_function(RUN)));
        assert!(
            !hold_function(RUN).starts_with("e2e::"),
            "the subject is denied e2e::*"
        );
    }
}
