//! `steer_and_stop` — steering a running turn and stopping a held tree. The
//! subject's one exchange calls a run-scoped `hold` fixture; while it is
//! held the runner sends two steers to the session, and checks they queue in
//! order (`queued`, one `harness::message-queued` each, `status.queued` in
//! order), edits the first in place (one more event) and unqueues the second. The released
//! turn must see only the edited steer and answer with its token.
//!
//! In `capture`, on side sessions:
//!
//! - two concurrent sends with identical options land on one turn, one
//!   seeding it and the other merged or queued into it;
//! - three times: a parent and its directly spawned child both held in the
//!   fixture, `harness::stop` on the parent, the holds released 50 ms later;
//!   both turns must end `cancelled` within 10 s, each held call closed with
//!   one `cancelled` result, and no new turn may start for 15 s;
//! - the merge session, its first turn done, is sent a second one that is
//!   held in the fixture; a stop naming the first turn answers
//!   `stopping: false` and the held turn keeps running.
//!
//! A side model that never calls the fixture leaves the run undetermined.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iii_sdk::protocol::RegisterTriggerInput;
use iii_sdk::{IIIClient, RegisterFunction};
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::common::lock_unpoisoned;
use super::mechanism::{self, call, record};
use super::{
    async_trait, common, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation,
    Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "steer_and_stop";
const DELIVERABLE_ID: &str = "steer_and_stop_checks";
const MESSAGE_QUEUED: &str = "harness::message-queued";
const STEER_LABEL: &str = "steer";
const STALE_LABEL: &str = "stale";
/// Recorded when a side session's model never called the hold fixture.
const SIDE_HOLD_SKIPPED: &str = "side_hold_skipped";
const STOP_ROUNDS: usize = 3;
const RELEASE_AFTER_STOP: Duration = Duration::from_millis(50);
const CANCEL_WITHIN: Duration = Duration::from_secs(10);
const QUIET_FOR: Duration = Duration::from_secs(15);
const HOLD_LIMIT: Duration = Duration::from_secs(90);

const SEED_OR_MERGE: AssessmentSpec = AssessmentSpec::gate(
    "seed_or_merge",
    15,
    "Did two concurrent sends with identical options land on one turn, one seeding it and the other merged or queued into it?",
)
.labeled("Concurrent sends share one turn");
const STEERING: AssessmentSpec = AssessmentSpec::gate(
    "steering_queue",
    35,
    "Did two steers sent during a held call queue in order with one message-queued event each, did edit_queued keep the first in place and unqueue remove the second, and did only the edited steer reach the transcript and the reply?",
)
.labeled("Steers queue in order, edit and unqueue work");
const STOP: AssessmentSpec = AssessmentSpec::gate(
    "stop_cascade",
    40,
    "In each of three rounds, did stopping a parent whose child was also held cancel both within 10 s, close each held call with one cancelled result, and start no new turn for 15 s?",
)
.labeled("Stop cancels a held parent and child, three times");
const STALE_STOP: AssessmentSpec = AssessmentSpec::gate(
    "stale_stop",
    10,
    "With a session's second turn held, did a stop naming its first turn answer `stopping: false` and leave the held turn running?",
)
.labeled("A stop for an old turn does nothing");
const ASSESSMENTS: &[AssessmentSpec] = &[SEED_OR_MERGE, STEERING, STOP, STALE_STOP];

fn hold_function(run_id: &str) -> String {
    format!("e2etest::steer_hold_{}", mechanism::short(run_id))
}

fn sink_function(run_id: &str) -> String {
    format!("e2e::steer::queued::{}", mechanism::short(run_id))
}

fn token(run_id: &str, label: &str) -> String {
    let hex = super::stable_seed(&format!("{ID}:{run_id}")) & 0xff_ffff;
    format!("{label}-{hex:06x}")
}

fn steer(run_id: &str, label: &str) -> String {
    format!(
        "Also include the token `{}` in your reply.",
        token(run_id, label)
    )
}

/// What a side session is told: park on the hold fixture under `label`.
fn side_task(run_id: &str, label: &str) -> String {
    format!(
        "Call `{}` once with `{{\"label\": \"{label}\"}}` and wait for it to return. Then reply with exactly `DONE`.",
        hold_function(run_id)
    )
}

fn merge_message(label: &str) -> String {
    format!("Reply with exactly `ACK {label}` and nothing else.")
}

fn side(run_id: &str, name: &str) -> String {
    format!("{}_{name}", mechanism::session_id(run_id))
}

/// Which held calls are parked and which the runner released, per attempt.
#[derive(Default)]
struct Board {
    held: HashSet<String>,
    released: HashSet<String>,
}

fn boards() -> &'static Mutex<HashMap<String, Arc<Mutex<Board>>>> {
    static BOARDS: OnceLock<Mutex<HashMap<String, Arc<Mutex<Board>>>>> = OnceLock::new();
    BOARDS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn board(run_id: &str) -> Arc<Mutex<Board>> {
    Arc::clone(
        lock_unpoisoned(boards())
            .entry(run_id.to_string())
            .or_default(),
    )
}

pub struct SteerAndStop;

#[async_trait]
impl Scenario for SteerAndStop {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Steer a running turn, stop a held one")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("One exchange whose tool call the runner holds while it steers the turn; side sessions are sent concurrently and stopped mid-call.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("A user can redirect a running agent, edit or withdraw what they queued, and stop it and its sub-agents mid-tool, and nothing restarts on its own. If it fails, steering messages get lost or reordered, stop leaves agents running, or a stopped tree wakes back up.")
    }

    /// One retained cohort: the probes do not vary with a seed.
    fn case(&self, _seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            super::stable_seed(ID),
            json!({ "stop_rounds": STOP_ROUNDS, "release_after_stop_ms": 50 }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::E2eRunScopedFixtures,
                Capability::E2eSubagents,
                Capability::IiiFunctions,
            ],
            mechanism::contract(DELIVERABLE_ID),
        )
    }

    fn canonical_seed_only(&self) -> bool {
        true
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let hold = hold_function(run_id);
        let ack = token(run_id, "ACK");
        ScenarioSpec {
            id: ID,
            prompt: format!(
                "Call `{hold}` once with `{{\"label\": \"{STEER_LABEL}\"}}` and wait for it to \
                 return. Then reply with one line: `{ack}` followed by every token I ask you to \
                 include."
            ),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(6),
                max_output_tokens: Some(2_048),
                max_total_tokens: Some(100_000),
                stuck_timeout_seconds: 240,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    fn required_functions(&self, run_id: &str) -> Vec<String> {
        vec![hold_function(run_id)]
    }

    fn allowed_functions(&self, run_id: &str) -> Option<Vec<String>> {
        Some(self.required_functions(run_id))
    }

    /// What the runner sends the subject and the side sessions is part of
    /// the task.
    fn planted_state(&self, run_id: &str) -> Option<Value> {
        let stop_tasks: Vec<String> = (1..=STOP_ROUNDS)
            .flat_map(|round| {
                [
                    side_task(run_id, &format!("stop-parent-{round}")),
                    side_task(run_id, &format!("stop-child-{round}")),
                ]
            })
            .collect();
        let steers = ["STEER-A", "STEER-B", "STEER-EDIT"].map(|label| steer(run_id, label));
        let merge = ["MERGE-1", "MERGE-2"].map(merge_message);
        Some(json!({ "runner_messages": {
            "steers": steers,
            "merge": merge,
            "stop_tasks": stop_tasks,
            "stale_task": side_task(run_id, STALE_LABEL),
        } }))
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        mechanism::reset(run_id);
        lock_unpoisoned(boards()).remove(run_id);
        mechanism::keep_session(run_id, &side(run_id, "merge"));
        for round in 1..=STOP_ROUNDS {
            mechanism::keep_session(run_id, &side(run_id, &format!("stop{round}_child")));
            mechanism::keep_session(run_id, &side(run_id, &format!("stop{round}")));
        }
        let client = context.client().clone();
        let owner = run_id.to_string();
        let function = context.client().register_function(
            hold_function(run_id),
            RegisterFunction::new_async(move |input: Value| {
                let client = client.clone();
                let owner = owner.clone();
                async move {
                    let label = input["label"].as_str().unwrap_or_default().to_string();
                    if label == STEER_LABEL {
                        steer_while_held(&client, &owner).await;
                    } else {
                        hold(&owner, &label).await;
                    }
                    Ok::<Value, iii_sdk::errors::Error>(json!({ "label": label, "released": true }))
                }
            })
            .description("E2E run-scoped checkpoint: returns once the runner releases it."),
        );
        mechanism::keep_function(run_id, function);

        let owner = run_id.to_string();
        let function = context.client().register_function(
            sink_function(run_id),
            RegisterFunction::new_async(move |event: Value| {
                let owner = owner.clone();
                async move {
                    mechanism::append(&owner, "queued_events", event);
                    Ok::<Value, iii_sdk::errors::Error>(json!({ "accepted": true }))
                }
            })
            .description("E2E steer_and_stop: records the session's message-queued events.")
            .metadata(json!({ "internal": true })),
        );
        mechanism::keep_function(run_id, function);
        let trigger = context
            .client()
            .register_trigger(RegisterTriggerInput::new(
                MESSAGE_QUEUED,
                sink_function(run_id),
                json!({ "session_id": mechanism::session_id(run_id) }),
            ))?;
        mechanism::keep_trigger(run_id, trigger);
        Ok(())
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        steering_outcome(observation, run_id);
        if let Some(route) = mechanism::subject_route(&observation.transcript) {
            let earlier = seed_or_merge(context, run_id, &route).await;
            stale_stop(context, run_id, &route, earlier).await;
            for round in 1..=STOP_ROUNDS {
                stop_round(context, run_id, &route, round).await;
            }
        } else {
            record(
                run_id,
                "side_route",
                false,
                "no assistant reply to take the route from",
            );
        }
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
        if !common::function_calls(&observation.transcript)
            .iter()
            .any(|call| call.function_id == hold_function(run_id))
        {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "hold_called",
                "the subject never called the hold fixture",
            ));
        }
        let checks = mechanism::observed_checks(observation, run_id);
        if let Some(skipped) = checks.iter().find(|check| check.id == SIDE_HOLD_SKIPPED) {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                SIDE_HOLD_SKIPPED,
                format!(
                    "a side session's model never called the hold fixture: {}",
                    skipped.detail
                ),
            ));
        }
        let stop_ids: Vec<String> = (1..=STOP_ROUNDS)
            .map(|round| format!("stop_round_{round}"))
            .collect();
        let stop_ids: Vec<&str> = stop_ids.iter().map(String::as_str).collect();
        Ok(assessment::build_evaluation([
            mechanism::grade(SEED_OR_MERGE, &checks, &["one_turn_for_concurrent_sends"]),
            mechanism::grade(
                STEERING,
                &checks,
                &[
                    "steers_queued",
                    "queued_in_order",
                    "edit_keeps_position",
                    "unqueue_removes",
                    "queued_events",
                    "only_edited_steer_delivered",
                ],
            ),
            mechanism::grade(STOP, &checks, &stop_ids),
            mechanism::grade(STALE_STOP, &checks, &["stale_stop_refused"]),
        ]))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        // Free any hold still parked so its handler returns.
        if let Some(board) = lock_unpoisoned(boards()).remove(run_id) {
            let mut board = lock_unpoisoned(&board);
            let held: Vec<String> = board.held.iter().cloned().collect();
            board.released.extend(held);
        }
        mechanism::release(context, run_id).await
    }
}

/// Parks a call until the runner releases its label, or `HOLD_LIMIT`.
async fn hold(run_id: &str, label: &str) {
    let board = board(run_id);
    lock_unpoisoned(&board).held.insert(label.to_string());
    let started = Instant::now();
    while !lock_unpoisoned(&board).released.contains(label) && started.elapsed() < HOLD_LIMIT {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn queued_entries(status: &Value) -> Vec<(String, String)> {
    status["queued"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| {
            (
                row["entry_id"].as_str().unwrap_or_default().to_string(),
                mechanism::text_of(&row["message"]),
            )
        })
        .collect()
}

/// The steering probes, made while the subject's call is held.
async fn steer_while_held(client: &IIIClient, run_id: &str) {
    let session_id = mechanism::session_id(run_id);
    let status = |client: &IIIClient| {
        let session_id = session_id.clone();
        let client = client.clone();
        async move {
            call(
                &client,
                "harness::status",
                json!({ "session_id": session_id, "verbose": true }),
            )
            .await
            .unwrap_or(Value::Null)
        }
    };
    let live_turn = status(client).await["turn_id"].clone();
    let mut sent = Vec::new();
    for label in ["STEER-A", "STEER-B"] {
        sent.push(
            call(
                client,
                "harness::send",
                json!({ "session_id": session_id, "message": steer(run_id, label) }),
            )
            .await,
        );
    }
    record(
        run_id,
        "steers_queued",
        sent.iter().all(|response| {
            matches!(response, Ok(response) if response["queued"] == true && response["turn_id"] == live_turn)
        }),
        format!("live_turn={live_turn} responses={sent:?}"),
    );
    let queued = queued_entries(&status(client).await);
    let texts: Vec<&str> = queued.iter().map(|(_, text)| text.as_str()).collect();
    record(
        run_id,
        "queued_in_order",
        texts == [steer(run_id, "STEER-A"), steer(run_id, "STEER-B")],
        format!("queued={queued:?}"),
    );
    let (Some((first, _)), Some((second, _))) = (queued.first(), queued.get(1)) else {
        record(
            run_id,
            "edit_keeps_position",
            false,
            "fewer than two queued steers",
        );
        record(
            run_id,
            "unqueue_removes",
            false,
            "fewer than two queued steers",
        );
        return;
    };
    mechanism::note(run_id, "steer_entries", json!([first, second]));
    let edited = call(
        client,
        "harness::edit_queued",
        json!({ "session_id": session_id, "entry_id": first, "message": steer(run_id, "STEER-EDIT") }),
    )
    .await;
    let after_edit = queued_entries(&status(client).await);
    record(
        run_id,
        "edit_keeps_position",
        matches!(&edited, Ok(response) if response["updated"] == true)
            && after_edit
                == [
                    (first.clone(), steer(run_id, "STEER-EDIT")),
                    (second.clone(), steer(run_id, "STEER-B")),
                ],
        format!("edit={edited:?} queued={after_edit:?}"),
    );
    let removed = call(
        client,
        "harness::unqueue",
        json!({ "session_id": session_id, "entry_id": second }),
    )
    .await;
    let after_unqueue = queued_entries(&status(client).await);
    record(
        run_id,
        "unqueue_removes",
        matches!(&removed, Ok(response) if response["removed"] == true)
            && after_unqueue.len() == 1
            && after_unqueue[0].0 == *first,
        format!("unqueue={removed:?} queued={after_unqueue:?}"),
    );
}

/// What the released turn did with the steers.
fn steering_outcome(observation: &ScenarioObservation, run_id: &str) {
    // One event per steer queued, and one more for the edit of the first.
    let events = mechanism::noted(run_id, "queued_events").unwrap_or_else(|| json!([]));
    let mut seen: Vec<String> = events
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|event| event["entry_id"].as_str().map(str::to_string))
        .collect();
    seen.sort();
    let mut wanted: Vec<String> = match mechanism::noted(run_id, "steer_entries") {
        Some(Value::Array(entries)) => entries
            .iter()
            .chain(entries.first())
            .filter_map(|entry| entry.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    };
    wanted.sort();
    record(
        run_id,
        "queued_events",
        !wanted.is_empty() && seen == wanted,
        format!("message-queued for {seen:?}, expected {wanted:?}"),
    );
    let users = mechanism::user_texts(&observation.transcript);
    let edited = token(run_id, "STEER-EDIT");
    let delivered = users.iter().filter(|text| text.contains(&edited)).count();
    let dropped = users.iter().any(|text| {
        text.contains(&token(run_id, "STEER-A")) || text.contains(&token(run_id, "STEER-B"))
    });
    let reply = observation.response.as_str();
    record(
        run_id,
        "only_edited_steer_delivered",
        delivered == 1
            && !dropped
            && reply.contains(&edited)
            && !reply.contains(&token(run_id, "STEER-B")),
        format!(
            "edited delivered {delivered}x, stale steer in transcript={dropped}, reply={reply:?}"
        ),
    );
}

/// Two concurrent sends with identical options to a new side session; the
/// turn they landed on.
async fn seed_or_merge(
    context: &E2eContext,
    run_id: &str,
    route: &(String, String),
) -> Option<String> {
    let session_id = side(run_id, "merge");
    let send = |label: &str| {
        json!({
            "session_id": session_id,
            "message": merge_message(label),
            "model": route.0,
            "provider": route.1,
            "session": { "kind": crate::wire::E2E_SESSION_KIND, "title": "Harness E2E: steer_and_stop merge" },
            "options": { "max_turns": 2, "functions": { "allow": [], "deny": [] } },
        })
    };
    let (first, second) = tokio::join!(
        context.trigger_value("harness::send", send("MERGE-1")),
        context.trigger_value("harness::send", send("MERGE-2")),
    );
    let folded = |response: &Value| response["merged"] == true || response["queued"] == true;
    let passed = match (&first, &second) {
        (Ok(a), Ok(b)) => {
            a["accepted"] == true
                && b["accepted"] == true
                && a["turn_id"] == b["turn_id"]
                && folded(a) != folded(b)
        }
        _ => false,
    };
    record(
        run_id,
        "one_turn_for_concurrent_sends",
        passed,
        format!("first={first:?} second={second:?}"),
    );
    let ended = terminal(context, &session_id, Duration::from_secs(120)).await?;
    ended["turn_id"].as_str().map(str::to_string)
}

/// Holds a second turn on the merge session and stops it by the first
/// turn's id: the stop must be refused and the held turn keep running.
async fn stale_stop(
    context: &E2eContext,
    run_id: &str,
    route: &(String, String),
    earlier: Option<String>,
) {
    let id = "stale_stop_refused";
    let Some(earlier) = earlier else {
        record(
            run_id,
            id,
            false,
            "the merge session's first turn never ended",
        );
        return;
    };
    let session_id = side(run_id, "merge");
    let hold = hold_function(run_id);
    let sent = context
        .trigger_value(
            "harness::send",
            json!({
                "session_id": session_id,
                "message": side_task(run_id, STALE_LABEL),
                "model": route.0,
                "provider": route.1,
                "options": { "max_turns": 3, "functions": { "allow": [hold], "deny": [] } },
            }),
        )
        .await;
    if let Err(error) = sent {
        record(run_id, id, false, format!("harness::send: {error:#}"));
        return;
    }
    if !held_or_skipped(context, run_id, id, &[(&session_id, STALE_LABEL)]).await {
        return;
    }
    let live = context
        .trigger_value("harness::status", json!({ "session_id": session_id }))
        .await
        .ok()
        .and_then(|status| status["turn_id"].as_str().map(str::to_string))
        .unwrap_or_default();
    let stopped = context
        .trigger_value(
            "harness::stop",
            json!({ "session_id": session_id, "turn_id": earlier }),
        )
        .await;
    let after = context
        .trigger_value("harness::status", json!({ "session_id": session_id }))
        .await
        .unwrap_or(Value::Null);
    lock_unpoisoned(&board(run_id))
        .released
        .insert(STALE_LABEL.to_string());
    let ended = terminal(context, &session_id, Duration::from_secs(120)).await;
    record(
        run_id,
        id,
        live != earlier && stale_stop_held(&stopped, &after, &live),
        format!(
            "earlier={earlier} live={live} stop={stopped:?} after: status={} turn={}; ended={:?}",
            after["status"],
            after["turn_id"],
            ended.map(|status| status["status"].clone())
        ),
    );
}

/// Whether a stop naming an earlier turn was refused and left `live_turn`
/// running.
fn stale_stop_held(stopped: &anyhow::Result<Value>, after: &Value, live_turn: &str) -> bool {
    matches!(stopped, Ok(response) if response["stopping"] == false)
        && !live_turn.is_empty()
        && after["turn_id"] == live_turn
        && !matches!(
            after["status"].as_str(),
            None | Some("completed" | "failed" | "cancelled")
        )
}

/// Waits until every `(session, label)` is parked in the hold fixture. When
/// one is not, records `SIDE_HOLD_SKIPPED` if its model never called the
/// fixture, else fails check `id`.
async fn held_or_skipped(
    context: &E2eContext,
    run_id: &str,
    id: &str,
    holds: &[(&str, &str)],
) -> bool {
    let board = board(run_id);
    let labels: Vec<String> = holds.iter().map(|(_, label)| label.to_string()).collect();
    let all_held = mechanism::poll(Duration::from_secs(90), || {
        let board = Arc::clone(&board);
        let labels = labels.clone();
        async move {
            let board = lock_unpoisoned(&board);
            labels
                .iter()
                .all(|label| board.held.contains(label))
                .then_some(())
        }
    })
    .await;
    if all_held.is_some() {
        return true;
    }
    let hold = hold_function(run_id);
    let mut skipped = Vec::new();
    for (session, label) in holds {
        let transcript = context.transcript(session).await.unwrap_or(Value::Null);
        if !common::function_calls(&transcript)
            .iter()
            .any(|call| call.function_id == hold && call.arguments["label"] == *label)
        {
            skipped.push(format!("{session} ({label})"));
        }
    }
    if skipped.is_empty() {
        record(
            run_id,
            id,
            false,
            "the hold was called but never reached the fixture",
        );
    } else {
        record(run_id, SIDE_HOLD_SKIPPED, false, skipped.join(", "));
    }
    false
}

/// The session's status once its turn is terminal.
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

/// One stop of a held parent and its held child.
async fn stop_round(context: &E2eContext, run_id: &str, route: &(String, String), round: usize) {
    let id = format!("stop_round_{round}");
    let (parent, child) = (
        side(run_id, &format!("stop{round}")),
        side(run_id, &format!("stop{round}_child")),
    );
    let (parent_label, child_label) = (
        format!("stop-parent-{round}"),
        format!("stop-child-{round}"),
    );
    let hold = hold_function(run_id);
    let task = |label: &str| side_task(run_id, label);
    let policy = json!({ "allow": [hold], "deny": [] });
    let started = context
        .trigger_value(
            "harness::send",
            json!({
                "session_id": parent,
                "message": task(&parent_label),
                "model": route.0,
                "provider": route.1,
                "session": { "kind": crate::wire::E2E_SESSION_KIND, "title": "Harness E2E: steer_and_stop parent" },
                "options": { "max_turns": 3, "functions": policy },
            }),
        )
        .await;
    let spawned = context
        .trigger_value(
            "harness::spawn",
            json!({
                "task": task(&child_label),
                "session_id": child,
                "parent_session_id": parent,
                "model": route.0,
                "provider": route.1,
                "display": { "name": "E2E held child" },
                "options": { "max_turns": 3, "functions": policy },
            }),
        )
        .await;
    if let Err(error) = started.as_ref().and(spawned.as_ref()) {
        record(run_id, &id, false, format!("start: {error:#}"));
        return;
    }
    let both = [
        (parent.as_str(), parent_label.as_str()),
        (child.as_str(), child_label.as_str()),
    ];
    if !held_or_skipped(context, run_id, &id, &both).await {
        return;
    }
    let board = board(run_id);
    let turns = |status: &Option<Value>| status.as_ref().map(|status| status["turn_id"].clone());
    let before = [
        context
            .trigger_value("harness::status", json!({ "session_id": parent }))
            .await
            .ok(),
        context
            .trigger_value("harness::status", json!({ "session_id": child }))
            .await
            .ok(),
    ];
    let clock = Instant::now();
    let release = {
        let board = Arc::clone(&board);
        let labels = [parent_label.clone(), child_label.clone()];
        async move {
            tokio::time::sleep(RELEASE_AFTER_STOP).await;
            lock_unpoisoned(&board).released.extend(labels);
        }
    };
    let (stopped, ()) = tokio::join!(
        context.trigger_value("harness::stop", json!({ "session_id": parent })),
        release,
    );
    let cancelled = mechanism::poll(CANCEL_WITHIN.saturating_sub(clock.elapsed()), || async {
        let mut statuses = Vec::new();
        for session in [&parent, &child] {
            let status = context
                .trigger_value("harness::status", json!({ "session_id": session }))
                .await
                .ok()?;
            if status["status"] != "cancelled" {
                return None;
            }
            statuses.push(status);
        }
        Some(statuses)
    })
    .await;
    let elapsed = clock.elapsed();
    tokio::time::sleep(QUIET_FOR).await;
    let mut detail = vec![format!("stop={stopped:?} cancelled_in={elapsed:?}")];
    let mut passed = matches!(&stopped, Ok(response) if response["stopping"] == true)
        && cancelled.is_some()
        && elapsed <= CANCEL_WITHIN;
    for (index, (session, label)) in [(&parent, &parent_label), (&child, &child_label)]
        .into_iter()
        .enumerate()
    {
        let status = context
            .trigger_value("harness::status", json!({ "session_id": session }))
            .await
            .ok();
        let quiet = status
            .as_ref()
            .is_some_and(|status| status["status"] == "cancelled")
            && turns(&status) == turns(&before[index]);
        let transcript = context.transcript(session).await.unwrap_or(Value::Null);
        let (closed, stopped_notice) = closed_cancelled(&transcript, &hold, label);
        passed &= quiet && closed && stopped_notice;
        detail.push(format!(
            "{session}: quiet_15s={quiet} one_cancelled_result={closed} stopped_notice={stopped_notice}"
        ));
    }
    record(run_id, &id, passed, detail.join("; "));
}

/// Whether the held call carrying `label` has exactly one result, the
/// `cancelled` one, and the turn wrote its `e_<turn>_stopped` notice.
fn closed_cancelled(transcript: &Value, hold: &str, label: &str) -> (bool, bool) {
    let calls: Vec<_> = common::function_outcomes(transcript)
        .into_iter()
        .filter(|call| call.function_id == hold && call.arguments["label"] == label)
        .collect();
    let results = calls
        .first()
        .and_then(|call| call.call_id.clone())
        .map_or(0, |call_id| {
            mechanism::entries(transcript)
                .filter(|entry| {
                    entry["message"]["role"] == "function_result"
                        && entry["message"]["function_call_id"] == call_id.as_str()
                })
                .count()
        });
    let closed = calls.len() == 1
        && results == 1
        && calls[0].is_error == Some(true)
        && calls[0]
            .details
            .as_ref()
            .is_some_and(|details| details["error"] == "cancelled");
    let stopped = mechanism::customs(transcript, "notice")
        .iter()
        .any(|(id, data)| id.ends_with("_stopped") && data["reason"] == "stopped");
    (closed, stopped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held_call(result: Option<Value>) -> Value {
        let mut messages = vec![
            json!({ "entry_id": "e_t_1_0_assistant", "message": { "role": "assistant", "content": [
            { "type": "function_call", "id": "c1", "function_id": "hold", "arguments": { "label": "p" } },
        ] } }),
        ];
        if let Some(result) = result {
            messages.push(json!({ "entry_id": "e_t_1_c1", "message": result }));
        }
        messages.push(json!({ "entry_id": "e_t_1_stopped", "custom": { "custom_type": "notice", "data": { "reason": "stopped" } } }));
        json!({ "messages": messages })
    }

    fn cancelled() -> Value {
        json!({ "role": "function_result", "function_call_id": "c1", "function_id": "hold", "is_error": true, "details": { "error": "cancelled" } })
    }

    #[test]
    fn a_held_call_is_closed_by_one_cancelled_result() {
        assert_eq!(
            closed_cancelled(&held_call(Some(cancelled())), "hold", "p"),
            (true, true)
        );
        let released = json!({ "role": "function_result", "function_call_id": "c1", "function_id": "hold", "is_error": false, "details": { "released": true } });
        assert_eq!(
            closed_cancelled(&held_call(Some(released)), "hold", "p"),
            (false, true)
        );
        assert_eq!(
            closed_cancelled(&held_call(None), "hold", "p"),
            (false, true)
        );
        let mut twice = held_call(Some(cancelled()));
        twice["messages"]
            .as_array_mut()
            .unwrap()
            .insert(2, json!({ "message": cancelled() }));
        assert_eq!(closed_cancelled(&twice, "hold", "p"), (false, true));
    }

    #[test]
    fn queued_rows_read_in_order_with_their_text() {
        let status = json!({ "queued": [
            { "entry_id": "e_q_1", "message": { "role": "user", "content": [{ "type": "text", "text": "a" }] } },
            { "entry_id": "e_q_2", "message": { "role": "user", "content": [{ "type": "text", "text": "b" }] } },
        ] });
        assert_eq!(
            queued_entries(&status),
            [
                ("e_q_1".to_string(), "a".to_string()),
                ("e_q_2".to_string(), "b".to_string())
            ]
        );
        assert!(queued_entries(&json!({})).is_empty());
    }

    /// A stop naming the previous turn must leave the live one running: a
    /// refusal alone is what a terminal session answers anyway.
    #[test]
    fn a_stale_stop_passes_only_when_the_live_turn_keeps_running() {
        let refused: anyhow::Result<Value> = Ok(json!({ "stopping": false }));
        let running = json!({ "status": "running", "turn_id": "t_2" });
        assert!(stale_stop_held(&refused, &running, "t_2"));
        let cancelled = json!({ "status": "cancelled", "turn_id": "t_2" });
        assert!(!stale_stop_held(&refused, &cancelled, "t_2"));
        let accepted: anyhow::Result<Value> = Ok(json!({ "stopping": true }));
        assert!(!stale_stop_held(&accepted, &running, "t_2"));
        assert!(!stale_stop_held(&refused, &running, "t_1"));
        assert!(!stale_stop_held(&refused, &running, ""));
    }

    #[tokio::test]
    async fn a_side_model_that_skipped_its_hold_leaves_the_run_undetermined() {
        let run = "side-skipped";
        let hold = hold_function(run);
        let transcript = json!({ "messages": [{ "message": { "role": "assistant", "content": [
            { "type": "function_call", "id": "c1", "function_id": hold, "arguments": { "label": STEER_LABEL } },
        ] } }] });
        let observation = ScenarioObservation::for_test(
            crate::scenarios::ScenarioId::SteerAndStop,
            "control",
            transcript,
            "ACK",
            Vec::new(),
        );
        record(run, SIDE_HOLD_SKIPPED, false, "stop1_child never called");
        let context = E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let evaluation = SteerAndStop
            .evaluate(&context, &observation, run)
            .await
            .unwrap();
        mechanism::reset(run);
        assert_eq!(
            evaluation.completion,
            Some(crate::report::CompletionState::Undetermined)
        );
    }

    fn steered(run: &str, reply: &str, steers: &[&str]) -> Vec<(String, bool)> {
        mechanism::reset(run);
        mechanism::note(run, "steer_entries", json!(["e_q_a", "e_q_b"]));
        for entry in ["e_q_a", "e_q_b", "e_q_a"] {
            mechanism::append(run, "queued_events", json!({ "entry_id": entry }));
        }
        let mut messages = vec![
            json!({ "message": { "role": "user", "content": [{ "type": "text", "text": "go" }] } }),
        ];
        for label in steers {
            messages.push(json!({ "message": { "role": "user", "content": [{ "type": "text", "text": steer(run, label) }] } }));
        }
        let observation = ScenarioObservation::for_test(
            crate::scenarios::ScenarioId::SteerAndStop,
            "control",
            json!({ "messages": messages }),
            reply,
            Vec::new(),
        );
        steering_outcome(&observation, run);
        let checks = mechanism::checks(run)
            .into_iter()
            .map(|check| (check.id, check.passed))
            .collect();
        mechanism::reset(run);
        checks
    }

    #[test]
    fn only_the_edited_steer_may_reach_the_turn() {
        let run = "steered";
        let edited = token(run, "STEER-EDIT");
        assert_eq!(
            steered(run, &format!("ACK {edited}"), &["STEER-EDIT"]),
            [
                ("queued_events".to_string(), true),
                ("only_edited_steer_delivered".to_string(), true)
            ]
        );
        let leaked = steered(run, &format!("ACK {edited}"), &["STEER-EDIT", "STEER-B"]);
        assert_eq!(
            leaked[1],
            ("only_edited_steer_delivered".to_string(), false)
        );
        let unedited = steered(run, "ACK", &["STEER-A"]);
        assert_eq!(
            unedited[1],
            ("only_edited_steer_delivered".to_string(), false)
        );
    }

    #[test]
    fn every_runner_message_is_part_of_the_definition() {
        let planted = SteerAndStop.planted_state("run").unwrap();
        let messages = &planted["runner_messages"];
        assert_eq!(
            messages["steers"],
            json!(["STEER-A", "STEER-B", "STEER-EDIT"].map(|label| steer("run", label)))
        );
        assert_eq!(messages["merge"][0], merge_message("MERGE-1"));
        assert_eq!(messages["stop_tasks"][0], side_task("run", "stop-parent-1"));
        assert_eq!(
            messages["stop_tasks"].as_array().unwrap().len(),
            2 * STOP_ROUNDS
        );
        assert_eq!(messages["stale_task"], side_task("run", STALE_LABEL));
    }

    #[test]
    fn criteria_weights_total_one_hundred() {
        SteerAndStop.spec("run").validate().unwrap();
    }
}
