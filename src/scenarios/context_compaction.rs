//! `context_compaction` — explicit compaction, held to its contract on a live
//! session. Four short exchanges; the first calls a fixture that returns a
//! long result, the last calls a fixture the runner holds open. The runner:
//!
//! - after exchange 0: `harness::compact` finds nothing to summarise
//!   (`empty`), and refuses a session that does not exist;
//! - after exchange 2: two concurrent `harness::compact` calls, one `busy`;
//! - while the exchange-3 call is held: `/compact` through `harness::send`
//!   starts a `cmp_*` compaction on the live turn without a turn or a user
//!   entry, and a resend with the same key is deduplicated;
//! - in `capture`: one `e_<cmp>_outcome` record with `source: command`, the
//!   `harness::compaction` events started then completed with no summary,
//!   the held call still paired with its result, `context::prune` called
//!   on the stored transcript aging out the long early result (the prune
//!   the Harness runs during context assembly is not observed), and a side session whose prompt overflows the window
//!   failing as `harness.context_overflow` before any model call.

use std::sync::Arc;
use std::time::Duration;

use iii_sdk::protocol::RegisterTriggerInput;
use iii_sdk::RegisterFunction;
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::mechanism::{self, call, record, Check};
use super::{
    async_trait, common, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation,
    Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "context_compaction";
const DELIVERABLE_ID: &str = "compaction_checks";
/// The early result: big enough to be worth pruning, small enough that the
/// first exchange alone still fits the verbatim tail.
const BULK_BYTES: usize = 16_000;
/// Past the usable window of any catalog model (DeepSeek: 1M - 384k output).
const OVERFLOW_BYTES: usize = 4 * 1024 * 1024;
const COMPACTION_WAIT: Duration = Duration::from_secs(150);
const COMPACTION_EVENT: &str = "harness::compaction";

const COMMAND: AssessmentSpec = AssessmentSpec::gate(
    "compact_command",
    25,
    "Did `/compact` sent during a held call start a `cmp_*` compaction on the live turn, with no user entry and no extra turn, and did a resend with the same key come back deduplicated?",
)
.labeled("/compact mid-turn compacts without a turn or a user entry");
const RECORD: AssessmentSpec = AssessmentSpec::gate(
    "compaction_record",
    15,
    "Is there exactly one `e_<cmp>_outcome` record with `source: command`, the model and a headed summary, and did `harness::compaction` fire started then completed without the summary?",
)
.labeled("One command record, events started then completed");
const EMPTY_AND_BUSY: AssessmentSpec = AssessmentSpec::gate(
    "empty_and_busy",
    15,
    "Did an early `harness::compact` return `empty`, a missing session get refused as not existing, and one of two concurrent compactions return `busy`?",
)
.labeled("Early compact is empty, a concurrent one is busy");
const LIVE_TURN: AssessmentSpec = AssessmentSpec::gate(
    "live_turn_safety",
    15,
    "Did the turn whose call was held through the compaction keep that call paired with its result, inside the verbatim tail, with no error entry?",
)
.labeled("A compaction keeps the held call paired");
const OVERFLOW: AssessmentSpec = AssessmentSpec::gate(
    "context_overflow",
    15,
    "Did a side session whose prompt cannot fit the window fail as `harness.context_overflow` in `context_assembly` before any input token was billed?",
)
.labeled("An oversized prompt fails as context overflow");
const PRUNE: AssessmentSpec = AssessmentSpec::gate(
    "aged_prune",
    15,
    "Did `context::prune`, called by the runner on the stored transcript with fixed options, replace the long early result with a placeholder while keeping the latest result and every message?",
)
.labeled("Aged prune drops the old long result only");
const ASSESSMENTS: &[AssessmentSpec] =
    &[COMMAND, RECORD, EMPTY_AND_BUSY, LIVE_TURN, OVERFLOW, PRUNE];

fn bulk_function(run_id: &str) -> String {
    format!("e2etest::compaction_bulk_{}", mechanism::short(run_id))
}

fn hold_function(run_id: &str) -> String {
    format!("e2etest::compaction_hold_{}", mechanism::short(run_id))
}

fn sink_function(run_id: &str) -> String {
    format!("e2e::compaction::events::{}", mechanism::short(run_id))
}

/// The code each exchange's reply carries.
fn code(run_id: &str, exchange: usize) -> String {
    let hex = super::stable_seed(&format!("{ID}:{run_id}")) & 0xff_ffff;
    format!("CMP-{hex:06x}-{exchange}")
}

fn compact_key(run_id: &str) -> String {
    format!("e2e:{run_id}:{ID}:compact")
}

fn overflow_session(run_id: &str) -> String {
    format!("{}_overflow", mechanism::session_id(run_id))
}

fn prompts(run_id: &str) -> [String; 4] {
    let [c0, c1, c2, c3] = [0, 1, 2, 3].map(|exchange| code(run_id, exchange));
    let bulk = bulk_function(run_id);
    let hold = hold_function(run_id);
    [
        format!(
            "Call `{bulk}` once with arguments `{{\"label\": \"{c0}\"}}`. It returns a long \
             inventory: do not repeat it. Then reply with exactly `ACK {c0}` and nothing else."
        ),
        format!("Reply with exactly `ACK {c1}` and nothing else. Do not call any function."),
        format!("Reply with exactly `ACK {c2}` and nothing else. Do not call any function."),
        format!(
            "Call `{hold}` once with arguments `{{\"label\": \"{c3}\"}}` and wait for it to \
             return. Then reply with exactly `ACK {c3}` and nothing else."
        ),
    ]
}

pub struct ContextCompaction;

#[async_trait]
impl Scenario for ContextCompaction {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Compact a live conversation on command")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Four one-line exchanges, one with a long tool result and one with a call the runner holds open; the runner compacts the session around them.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("`/compact` and `harness::compact` summarise a session safely while a turn runs, report the same outcome to every client, refuse what they cannot do, and an oversized prompt fails cleanly. If it fails, long sessions lose context, orphan a tool call, or compact twice.")
    }

    /// One retained cohort: the probes do not vary with a seed.
    fn case(&self, _seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            super::stable_seed(ID),
            json!({ "bulk_bytes": BULK_BYTES, "overflow_bytes": OVERFLOW_BYTES }),
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

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let [prompt, ..] = prompts(run_id);
        ScenarioSpec {
            id: ID,
            prompt,
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(6),
                max_output_tokens: Some(2_048),
                max_total_tokens: Some(300_000),
                stuck_timeout_seconds: 300,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    fn dialogue_followups(&self, run_id: &str) -> Vec<String> {
        prompts(run_id).into_iter().skip(1).collect()
    }

    fn required_functions(&self, run_id: &str) -> Vec<String> {
        vec![bulk_function(run_id), hold_function(run_id)]
    }

    fn allowed_functions(&self, run_id: &str) -> Option<Vec<String>> {
        Some(self.required_functions(run_id))
    }

    /// What the runner sends the session and the overflow side session; the
    /// 4 MiB overflow prompt by its digest.
    fn planted_state(&self, _run_id: &str) -> Option<Value> {
        Some(json!({ "runner_messages": {
            "compact": "/compact",
            "overflow_sha256": crate::artifact::sha256_bytes(overflow_message().as_bytes()),
        } }))
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        mechanism::reset(run_id);
        let bulk = mechanism::marked_text("CMP-BULK", BULK_BYTES);
        let function = context.client().register_function(
            bulk_function(run_id),
            RegisterFunction::new_async(move |input: Value| {
                let bulk = bulk.clone();
                async move {
                    Ok::<Value, iii_sdk::errors::Error>(
                        json!({ "label": input["label"], "inventory": bulk }),
                    )
                }
            })
            .description("E2E run-scoped inventory: returns a long listing for a label."),
        );
        mechanism::keep_function(run_id, function);

        let client = Arc::new(context.client().clone());
        let owner = run_id.to_string();
        let function = context.client().register_function(
            hold_function(run_id),
            RegisterFunction::new_async(move |input: Value| {
                let client = Arc::clone(&client);
                let owner = owner.clone();
                async move {
                    compact_while_held(&client, &owner).await;
                    Ok::<Value, iii_sdk::errors::Error>(
                        json!({ "label": input["label"], "released": true }),
                    )
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
                    mechanism::append(&owner, "events", event);
                    Ok::<Value, iii_sdk::errors::Error>(json!({ "accepted": true }))
                }
            })
            .description("E2E context_compaction: records harness::compaction events.")
            .metadata(json!({ "internal": true })),
        );
        mechanism::keep_function(run_id, function);
        let trigger = context
            .client()
            .register_trigger(RegisterTriggerInput::new(
                COMPACTION_EVENT,
                sink_function(run_id),
                json!({ "session_id": mechanism::session_id(run_id) }),
            ))?;
        mechanism::keep_trigger(run_id, trigger);
        Ok(())
    }

    async fn before_followup(
        &self,
        context: &E2eContext,
        run_id: &str,
        session_id: &str,
        exchange: usize,
    ) {
        let client = context.client();
        match exchange {
            0 => {
                let early = call(
                    client,
                    "harness::compact",
                    json!({ "session_id": session_id }),
                )
                .await;
                record(
                    run_id,
                    "early_compact_empty",
                    matches!(&early, Ok(outcome) if outcome["status"] == "empty"),
                    format!("{early:?}"),
                );
                let missing = call(
                    client,
                    "harness::compact",
                    json!({ "session_id": format!("{session_id}_missing"), "model": "any" }),
                )
                .await;
                record(
                    run_id,
                    "missing_session_refused",
                    matches!(&missing, Err(error) if format!("{error:#}").contains("does not exist")),
                    format!("{missing:?}"),
                );
            }
            2 => {
                let request = json!({ "session_id": session_id });
                let (first, second) = tokio::join!(
                    call(client, "harness::compact", request.clone()),
                    call(client, "harness::compact", request),
                );
                let mut statuses: Vec<String> = [&first, &second]
                    .into_iter()
                    .map(|outcome| match outcome {
                        Ok(outcome) => outcome["status"].as_str().unwrap_or("?").to_string(),
                        Err(error) => format!("error: {error:#}"),
                    })
                    .collect();
                statuses.sort();
                record(
                    run_id,
                    "concurrent_compact_busy",
                    statuses == ["busy", "ok"],
                    format!("statuses={statuses:?}"),
                );
            }
            _ => {}
        }
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let compaction_id = mechanism::noted(run_id, "compaction_id")
            .and_then(|id| id.as_str().map(str::to_string))
            .unwrap_or_default();
        let events = mechanism::noted(run_id, "events").unwrap_or_else(|| json!([]));
        for check in transcript_checks(&observation.transcript, run_id, &compaction_id, &events) {
            record(run_id, &check.id, check.passed, check.detail);
        }
        prune_check(context, &observation.transcript, run_id).await;
        overflow_check(context, &observation.transcript, run_id).await;
        Ok(vec![mechanism::deliverable(DELIVERABLE_ID, run_id)])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        Ok(evaluate_checks(
            observation,
            run_id,
            &mechanism::observed_checks(observation, run_id),
        ))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        mechanism::release(context, run_id).await
    }
}

/// The probes made while the exchange-3 call is held: `/compact` on the live
/// turn, its deduplicated resend, and the wait for its outcome row.
async fn compact_while_held(client: &iii_sdk::IIIClient, run_id: &str) {
    let session_id = mechanism::session_id(run_id);
    let live_turn = call(
        client,
        "harness::status",
        json!({ "session_id": session_id }),
    )
    .await
    .ok()
    .and_then(|status| status["turn_id"].as_str().map(str::to_string));
    let send = json!({
        "session_id": session_id,
        "message": "/compact",
        "idempotency_key": compact_key(run_id),
    });
    let started = call(client, "harness::send", send.clone()).await;
    let compaction_id = started
        .as_ref()
        .ok()
        .and_then(|response| response["compaction"]["compaction_id"].as_str())
        .unwrap_or_default()
        .to_string();
    record(
        run_id,
        "command_started",
        compaction_id.starts_with("cmp_")
            && matches!(&started, Ok(response) if response["accepted"] == true
                && response["turn_id"].as_str() == live_turn.as_deref()),
        format!("live_turn={live_turn:?} response={started:?}"),
    );
    mechanism::note(run_id, "compaction_id", json!(compaction_id));
    let again = call(client, "harness::send", send).await;
    record(
        run_id,
        "command_deduplicated",
        matches!(&again, Ok(response) if response["deduplicated"] == true
            && response["compaction"].is_null()),
        format!("{again:?}"),
    );
    let outcome = format!("e_{}_outcome", mechanism::entry_safe(&compaction_id));
    let finished = mechanism::poll(COMPACTION_WAIT, || {
        let outcome = outcome.clone();
        let session_id = session_id.clone();
        async move {
            let page = call(
                client,
                "session::messages",
                json!({ "session_id": session_id, "limit": 500, "include_custom": true }),
            )
            .await
            .ok()?;
            let custom_type = mechanism::entries(&page)
                .find(|entry| entry["entry_id"] == outcome.as_str())
                .map(|entry| entry["custom"]["custom_type"].clone());
            custom_type
        }
    })
    .await;
    record(
        run_id,
        "command_finished",
        finished.as_ref() == Some(&json!("compaction")),
        format!("outcome row {outcome}: {finished:?}"),
    );
}

/// What the stored transcript and the recorded events show.
fn transcript_checks(
    transcript: &Value,
    run_id: &str,
    compaction_id: &str,
    events: &Value,
) -> Vec<Check> {
    let check = |id: &str, passed: bool, detail: String| Check {
        id: id.to_string(),
        passed,
        detail,
    };
    let users = mechanism::user_texts(transcript);
    let expected = prompts(run_id);
    let turns = mechanism::assistant_turns(transcript);
    let outcome_id = format!("e_{}_outcome", mechanism::entry_safe(compaction_id));
    let records: Vec<(&str, &Value)> = mechanism::customs(transcript, "compaction")
        .into_iter()
        .filter(|(id, _)| *id == outcome_id)
        .collect();
    let notice_id = format!("e_idem_{}", mechanism::entry_safe(&compact_key(run_id)));
    let notices = mechanism::customs(transcript, "notice")
        .into_iter()
        .filter(|(id, data)| *id == notice_id && data["compaction_id"] == compaction_id)
        .count();
    let record_ok = match records.as_slice() {
        [(_, data)] => {
            data["source"] == "command"
                && data["compaction_id"] == compaction_id
                && data["model"]
                    .as_str()
                    .is_some_and(|model| !model.is_empty())
                && data["summary"]
                    .as_str()
                    .is_some_and(|summary| summary.contains("## "))
        }
        _ => false,
    };
    let ours: Vec<&Value> = events
        .as_array()
        .into_iter()
        .flatten()
        .filter(|event| !compaction_id.is_empty() && event["compaction_id"] == compaction_id)
        .collect();
    let phases: Vec<&str> = ours
        .iter()
        .map(|event| event["phase"].as_str().unwrap_or("?"))
        .collect();
    let events_ok = phases == ["started", "completed"]
        && ours.iter().all(|event| event.get("summary").is_none())
        && ours[1]["status"] == "ok";

    let hold = hold_function(run_id);
    let calls: Vec<_> = common::function_outcomes(transcript)
        .into_iter()
        .filter(|call| call.function_id == hold)
        .collect();
    let index_of = |entry_id: &str| {
        mechanism::entries(transcript).position(|entry| entry["entry_id"] == entry_id)
    };
    let call_index = calls.first().and_then(|call| {
        let call_id = call.call_id.as_deref()?;
        mechanism::entries(transcript).position(|entry| {
            entry["message"]["role"] == "assistant"
                && entry["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|block| block["id"] == call_id)
        })
    });
    let tail_index = records
        .first()
        .and_then(|(_, data)| data["tail_start_entry_id"].as_str())
        .and_then(index_of);
    let errors = mechanism::customs(transcript, "error").len();
    let paired = calls.len() == 1 && calls[0].is_error == Some(false);
    vec![
        check(
            "no_command_entry",
            users.iter().all(|text| text.trim() != "/compact"),
            format!("user entries: {}", users.len()),
        ),
        check(
            "one_turn_per_exchange",
            users == expected && turns.len() == expected.len(),
            format!(
                "user entries={} (prompts {}), turns={turns:?}",
                users.len(),
                expected.len()
            ),
        ),
        check(
            "start_notice",
            notices == 1,
            format!("{notices} `{notice_id}` notice(s) for {compaction_id}"),
        ),
        check(
            "outcome_record",
            record_ok,
            format!("{} `{outcome_id}` record(s): {records:?}", records.len()),
        ),
        check(
            "events_in_order",
            events_ok,
            format!("phases={phases:?} events={ours:?}"),
        ),
        check(
            "pending_call_paired",
            paired
                && errors == 0
                && matches!((tail_index, call_index), (Some(tail), Some(call)) if tail <= call),
            format!(
                "calls={} is_error={:?} errors={errors} tail_index={tail_index:?} call_index={call_index:?}",
                calls.len(),
                calls.first().map(|call| call.is_error)
            ),
        ),
    ]
}

/// `context::prune` over the transcript's messages: the long result of the
/// first exchange is aged out, the held call's result is kept.
async fn prune_check(context: &E2eContext, transcript: &Value, run_id: &str) {
    let messages: Vec<Value> = mechanism::entries(transcript)
        .filter_map(|entry| entry.get("message").cloned())
        .collect();
    let pruned = context
        .trigger_value(
            "context::prune",
            json!({
                "messages": messages,
                "options": {
                    "min_free_tokens": 0,
                    "protect_recent_tokens": 1_000,
                    "protected_user_turns": 2,
                },
            }),
        )
        .await;
    let (passed, detail) = match &pruned {
        Ok(response) => prune_outcome(&messages, response, run_id),
        Err(error) => (false, format!("context::prune: {error:#}")),
    };
    record(run_id, "aged_prune", passed, detail);
}

fn prune_outcome(messages: &[Value], response: &Value, run_id: &str) -> (bool, String) {
    let after = response["messages"].as_array().cloned().unwrap_or_default();
    let result_of = |list: &[Value], function: &str| {
        list.iter()
            .find(|message| {
                message["role"] == "function_result" && message["function_id"] == function
            })
            .map(|message| message["content"].to_string())
    };
    let bulk = bulk_function(run_id);
    let hold = hold_function(run_id);
    let bulk_pruned = result_of(&after, &bulk)
        .is_some_and(|content| !content.contains("CMP-BULK-T"))
        && result_of(messages, &bulk).is_some_and(|content| content.contains("CMP-BULK-T"));
    let hold_kept = result_of(&after, &hold).is_some()
        && result_of(&after, &hold) == result_of(messages, &hold);
    (
        bulk_pruned && hold_kept && after.len() == messages.len(),
        format!(
            "bulk_pruned={bulk_pruned} hold_kept={hold_kept} messages {}->{} pruned_parts={}",
            messages.len(),
            after.len(),
            response["pruned_parts"]
        ),
    )
}

fn overflow_message() -> String {
    (0..OVERFLOW_BYTES / 9)
        .map(|index| format!("{:08x} ", super::stable_seed(&index.to_string()) as u32))
        .collect()
}

/// A fresh side session whose one prompt cannot fit the window.
async fn overflow_check(context: &E2eContext, transcript: &Value, run_id: &str) {
    let Some((model, provider)) = mechanism::subject_route(transcript) else {
        record(
            run_id,
            "overflow_failure",
            false,
            "no assistant reply to take the route from",
        );
        return;
    };
    let session_id = overflow_session(run_id);
    mechanism::keep_session(run_id, &session_id);
    let message = overflow_message();
    let sent = context
        .trigger_value(
            "harness::send",
            json!({
                "session_id": session_id,
                "message": message,
                "model": model,
                "provider": provider,
                "session": { "kind": crate::wire::E2E_SESSION_KIND, "title": "Harness E2E: context_compaction overflow" },
                "options": { "max_turns": 1, "functions": { "allow": [], "deny": [] } },
            }),
        )
        .await;
    if let Err(error) = sent {
        record(
            run_id,
            "overflow_failure",
            false,
            format!("harness::send: {error:#}"),
        );
        return;
    }
    let status = mechanism::poll(Duration::from_secs(120), || {
        let session_id = session_id.clone();
        async move {
            let status = context
                .trigger_value("harness::status", json!({ "session_id": session_id }))
                .await
                .ok()?;
            matches!(
                status["status"].as_str(),
                Some("failed" | "completed" | "cancelled")
            )
            .then_some(status)
        }
    })
    .await;
    let side = context.transcript(&session_id).await.unwrap_or(Value::Null);
    let errors: Vec<Value> = mechanism::customs(&side, "error")
        .into_iter()
        .map(|(_, data)| json!({ "code": data["code"], "phase": data["phase"] }))
        .collect();
    let input_tokens = context
        .metrics(&session_id)
        .await
        .map(|metrics| metrics.into_normalized().totals.input_tokens.unwrap_or(0))
        .ok();
    let failed = status
        .as_ref()
        .is_some_and(|status| status["status"] == "failed");
    record(
        run_id,
        "overflow_failure",
        failed
            && errors
                == [json!({ "code": "harness.context_overflow", "phase": "context_assembly" })]
            && input_tokens == Some(0),
        format!(
            "status={:?} errors={errors:?} input_tokens={input_tokens:?}",
            status.as_ref().map(|status| status["status"].clone())
        ),
    );
}

fn evaluate_checks(
    observation: &ScenarioObservation,
    run_id: &str,
    checks: &[Check],
) -> ObjectiveEvaluation {
    if !observation.metrics.complete {
        return assessment::prerequisite_failure(
            ASSESSMENTS,
            "metrics_complete",
            "subject metrics are incomplete",
        );
    }
    let called = |function: String| {
        common::function_calls(&observation.transcript)
            .iter()
            .any(|call| call.function_id == function)
    };
    // The probes ride on the subject's calls: a subject that never made them
    // says nothing about compaction.
    if !called(bulk_function(run_id)) || !called(hold_function(run_id)) {
        return assessment::prerequisite_failure(
            ASSESSMENTS,
            "fixture_calls",
            "the subject did not call both run-scoped fixtures",
        );
    }
    assessment::build_evaluation([
        mechanism::grade(
            COMMAND,
            checks,
            &[
                "command_started",
                "command_deduplicated",
                "command_finished",
                "no_command_entry",
                "one_turn_per_exchange",
                "start_notice",
            ],
        ),
        mechanism::grade(RECORD, checks, &["outcome_record", "events_in_order"]),
        mechanism::grade(
            EMPTY_AND_BUSY,
            checks,
            &[
                "early_compact_empty",
                "missing_session_refused",
                "concurrent_compact_busy",
            ],
        ),
        mechanism::grade(LIVE_TURN, checks, &["pending_call_paired"]),
        mechanism::grade(OVERFLOW, checks, &["overflow_failure"]),
        mechanism::grade(PRUNE, checks, &["aged_prune"]),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = "control";

    fn user(text: &str) -> Value {
        json!({ "entry_id": "e_idem_u", "message": { "role": "user", "content": [{ "type": "text", "text": text }] } })
    }

    fn reply(turn: &str, content: Value) -> Value {
        json!({ "entry_id": format!("e_{turn}_0_assistant"), "message": { "role": "assistant", "model": "m", "provider": "p", "content": content } })
    }

    fn custom(entry_id: &str, custom_type: &str, data: Value) -> Value {
        json!({ "entry_id": entry_id, "custom": { "custom_type": custom_type, "data": data } })
    }

    /// The run as a healthy Harness leaves it.
    fn healthy() -> (Value, Value) {
        let [p0, p1, p2, p3] = prompts(RUN);
        let hold = hold_function(RUN);
        let cmp = "cmp_1";
        let transcript = json!({ "messages": [
            user(&p0), reply("t_0", json!([{ "type": "text", "text": "ACK" }])),
            user(&p1), reply("t_1", json!([{ "type": "text", "text": "ACK" }])),
            user(&p2), reply("t_2", json!([{ "type": "text", "text": "ACK" }])),
            user(&p3),
            reply("t_3", json!([{ "type": "function_call", "id": "c1", "function_id": hold, "arguments": {} }])),
            custom(&format!("e_idem_{}", mechanism::entry_safe(&compact_key(RUN))), "notice", json!({ "compaction_id": cmp })),
            custom("e_cmp_1_outcome", "compaction", json!({
                "source": "command", "compaction_id": cmp, "model": "m",
                "summary": "## Goal\n- ack", "tail_start_entry_id": "e_t_2_0_assistant",
            })),
            { "entry_id": "e_t_3_c1", "message": { "role": "function_result", "function_call_id": "c1", "function_id": hold, "is_error": false, "content": [] } },
        ] });
        let events = json!([
            { "compaction_id": cmp, "phase": "started" },
            { "compaction_id": cmp, "phase": "completed", "status": "ok" },
        ]);
        (transcript, events)
    }

    fn failing(transcript: &Value, events: &Value) -> Vec<String> {
        transcript_checks(transcript, RUN, "cmp_1", events)
            .into_iter()
            .filter(|check| !check.passed)
            .map(|check| check.id)
            .collect()
    }

    #[test]
    fn a_healthy_transcript_passes_every_check() {
        let (transcript, events) = healthy();
        assert_eq!(failing(&transcript, &events), Vec::<String>::new());
    }

    #[test]
    fn a_compact_command_sent_to_the_model_is_caught() {
        let (mut transcript, events) = healthy();
        transcript["messages"]
            .as_array_mut()
            .unwrap()
            .push(user("/compact"));
        assert_eq!(
            failing(&transcript, &events),
            ["no_command_entry", "one_turn_per_exchange"]
        );
    }

    #[test]
    fn a_summary_in_the_event_or_a_missing_completion_is_caught() {
        let (transcript, mut events) = healthy();
        events[1]["summary"] = json!("leaked");
        assert_eq!(failing(&transcript, &events), ["events_in_order"]);
        let (transcript, events) = healthy();
        let started = json!([events[0].clone()]);
        assert_eq!(failing(&transcript, &started), ["events_in_order"]);
    }

    #[test]
    fn a_second_record_or_an_orphaned_call_is_caught() {
        let (mut transcript, events) = healthy();
        let messages = transcript["messages"].as_array_mut().unwrap();
        let duplicate = messages[9].clone();
        messages.insert(10, duplicate);
        assert_eq!(failing(&transcript, &events), ["outcome_record"]);

        let (mut transcript, events) = healthy();
        transcript["messages"].as_array_mut().unwrap().pop();
        assert_eq!(failing(&transcript, &events), ["pending_call_paired"]);
    }

    #[test]
    fn the_prune_outcome_wants_the_old_result_gone_and_the_new_one_kept() {
        let result = |function: String, text: &str| json!({ "role": "function_result", "function_id": function, "content": [{ "type": "text", "text": text }] });
        let before = vec![
            result(bulk_function(RUN), "CMP-BULK-H ... CMP-BULK-T"),
            result(hold_function(RUN), "released"),
        ];
        let pruned = json!({ "messages": [
            result(bulk_function(RUN), "[pruned]"),
            result(hold_function(RUN), "released"),
        ], "pruned_parts": 1 });
        assert!(prune_outcome(&before, &pruned, RUN).0);
        let untouched = json!({ "messages": before, "pruned_parts": 0 });
        assert!(!prune_outcome(&before, &untouched, RUN).0);
    }

    #[test]
    fn a_subject_that_skips_the_fixtures_leaves_the_run_undetermined() {
        let observation = ScenarioObservation::for_test(
            crate::scenarios::ScenarioId::ContextCompaction,
            RUN,
            json!({ "messages": [] }),
            "ACK",
            Vec::new(),
        );
        let evaluation = evaluate_checks(&observation, RUN, &[]);
        assert_eq!(
            evaluation.completion,
            Some(crate::report::CompletionState::Undetermined)
        );
    }

    #[test]
    fn the_runner_messages_are_part_of_the_definition() {
        let planted = ContextCompaction.planted_state(RUN).unwrap();
        assert_eq!(planted["runner_messages"]["compact"], "/compact");
        assert_eq!(
            planted["runner_messages"]["overflow_sha256"],
            crate::artifact::sha256_bytes(overflow_message().as_bytes())
        );
    }

    #[test]
    fn criteria_weights_total_one_hundred() {
        ContextCompaction.spec("run").validate().unwrap();
    }
}
