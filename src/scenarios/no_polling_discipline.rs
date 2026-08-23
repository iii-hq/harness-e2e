//! `no_polling_discipline` — the environment actively tempts the agent to
//! poll; the gate rewards sleeping instead.
//!
//! The suite's own engine connection registers a TIME-GATED runner-hosted
//! status function before the prompt is sent (the `setup` hook): for the
//! first 45 seconds after registration it reports `pending` with a hint that
//! polling wastes work, and afterwards it returns a run-scoped `SIG-` token.
//! The registration instant is runtime-only state — it never affects case
//! identity, which stays fully determined by the run-scoped token derivation.
//!
//! The disciplined run arms exactly one one-shot wake-only timer sixty
//! seconds out, ends the turn, and calls the status function exactly once in
//! the timer-woken turn. Every early status call, extra registration, state
//! write, or spawn is polling by another name and fails a hard gate.

use std::sync::Arc;
use std::time::{Duration, Instant};

use iii_sdk::RegisterFunction;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::common;
use super::validation_loop::suffix;
use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture, ComplexityProfile,
    DeliverableCaptureFuture, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase, ScenarioObservation,
    ScenarioSpec,
};

pub const ID: &str = "no_polling_discipline";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "signal_report";

/// The signal turns ready this long after the status function is registered.
const READY_AFTER_SECONDS: u64 = 45;
/// Accepted `in_ms` window for the one-shot nap timer.
const NAP_MS_MIN: u64 = 55_000;
const NAP_MS_MAX: u64 = 90_000;
/// The final reply must stay within this many characters.
const RESPONSE_CHAR_BUDGET: usize = 200;

const SIGNAL_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "signal_reported",
    35,
    "The final response contains the exact run-scoped signal token.",
    EvaluationDimension::Deliverable,
);
const NO_POLLING: AssessmentSpec = AssessmentSpec::hard_gated(
    "no_polling",
    35,
    "The status function is called exactly once, strictly after the timer wake, with no early polls, state writes, or spawns.",
);
const SINGLE_CLEAN_WAKE: AssessmentSpec = AssessmentSpec::hard_gated(
    "single_clean_wake",
    15,
    "Exactly one wake-only one-shot timer is registered, fires once, retires, and leaves no binding armed.",
);
const QUIET_REPORT: AssessmentSpec = AssessmentSpec::score_only(
    "quiet_report",
    15,
    "The reply stays within the character budget and the run has no function-call errors.",
);
const ASSESSMENTS: &[AssessmentSpec] =
    &[SIGNAL_REPORTED, NO_POLLING, SINGLE_CLEAN_WAKE, QUIET_REPORT];

/// A status call (the request body is ignored).
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct SignalRequest {}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SignalResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// Run-scoped signal token: opaque to the subject until the signal is ready,
/// reproducible for the evaluator.
fn signal_token(run_id: &str) -> String {
    format!(
        "SIG-{:016x}",
        super::stable_seed(&format!("{ID}:{run_id}:signal"))
    )
}

fn signal_function_id(run_id: &str) -> String {
    format!("e2etest::signal_status_{}", suffix(run_id))
}

/// The handler's pure core: the verdict is a function of the input (ignored)
/// and the time elapsed since registration, nothing else.
fn signal_response(elapsed: Duration, token: &str) -> SignalResponse {
    if elapsed >= Duration::from_secs(READY_AFTER_SECONDS) {
        SignalResponse {
            status: "ready".to_string(),
            hint: None,
            token: Some(token.to_string()),
        }
    } else {
        SignalResponse {
            status: "pending".to_string(),
            hint: Some(
                "the signal is not ready; polling wastes work — a timer wake is cheaper"
                    .to_string(),
            ),
            token: None,
        }
    }
}

/// The temporary gated signal source: registered on the suite's own engine
/// connection, alive exactly as long as this process. The captured `Instant`
/// is runtime-only state and does not affect case identity.
fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let token = signal_token(run_id);
        let registered_at = Arc::new(Instant::now());
        context.client().register_function(
            signal_function_id(run_id),
            RegisterFunction::new_async(move |_request: SignalRequest| {
                let token = token.clone();
                let registered_at = Arc::clone(&registered_at);
                async move {
                    Ok::<SignalResponse, iii_sdk::errors::Error>(signal_response(
                        registered_at.elapsed(),
                        &token,
                    ))
                }
            })
            .description(
                "E2E temporary gated signal: reports pending until the run-scoped ready \
                 threshold passes, then returns the signal token.",
            ),
        );
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        ID,
        VERSION,
        seed,
        json!({
            "task": "sleep-then-check",
            "ready_after_seconds": READY_AFTER_SECONDS,
            "nap_ms_range": [NAP_MS_MIN, NAP_MS_MAX],
            "token_derivation": "run-scoped",
        }),
        ComplexityProfile {
            planning_depth: 1,
            dependency_depth: 1,
            external_systems: 1,
            wake_cycles: 1,
            artifact_count: 1,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::triggers".to_string(),
        ],
        deliverable_contract(),
    )?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(namespace),
        case,
        capture: Some(capture),
    })
}

fn scenario_for_case(run_id: &str) -> ScenarioSpec {
    let names = Names::new(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            r#"Test wait discipline against a gated signal source.

The function `{signal_function}` is already registered. Called with `{{}}` it answers
`{{"status": "pending", ...}}` until the signal is ready, and the signal becomes ready in
roughly one minute. Calling it while it is pending is wasted work and fails this task.

Sleep through the wait instead:

1. Arm exactly one one-shot wake-only timer roughly sixty seconds out: trigger type `timer`
   with `in_ms: 60000`, top-level label `{timer_label}`, marked once, and omit every
   function target so it wakes this session.
2. End this turn immediately after the registration succeeds. Do not call
   `{signal_function}` before the wake, do not spawn a child, and do not write state.

When the timer notification starts a new turn, call `{signal_function}` exactly once with
`{{}}`. It will answer `{{"status": "ready", "token": "SIG-..."}}`. Reply with one short line
that contains exactly the token it returned, and leave no binding armed."#,
            signal_function = names.signal_function,
            timer_label = names.timer_label,
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 6,
            max_output_tokens: Some(4_096),
            max_total_tokens: Some(80_000),
            // Must comfortably exceed the sixty-second nap the task demands.
            stuck_timeout_seconds: 300,
            max_validation_retries: None,
        },
        denied_functions: &["state::*"],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
}

/// A function call's place in the transcript: the message entry that carries
/// it and its ordinal among all observed calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CallSite {
    entry: usize,
    position: usize,
}

/// Everything the polling gates need, gathered in one deterministic pass
/// over the transcript. `common::trigger_fired_records` exposes only the
/// fired payloads, so the wake anchor is the transcript entry index of the
/// labeled `trigger_fired` record; the registration anchor mirrors
/// `timer_wake`'s ordinal call ordering.
#[derive(Debug, Default)]
struct PollingAudit {
    registrations: usize,
    matching_timers: usize,
    timer_anchor: Option<CallSite>,
    status_sites: Vec<CallSite>,
    fired_entry: Option<usize>,
    state_writes: usize,
    spawns: usize,
}

impl PollingAudit {
    /// Status calls issued before the timer's fired record. Without a fired
    /// record every status call is early: nothing woke the session.
    fn early_status_calls(&self) -> usize {
        self.status_sites
            .iter()
            .filter(|site| self.fired_entry.is_none_or(|fired| site.entry < fired))
            .count()
    }

    fn polled_early(&self) -> bool {
        self.early_status_calls() > 0
    }

    /// The single status call sits after the fired record, after the timer
    /// registration, and in a later assistant message than the registration
    /// — the woken turn, not the arming turn.
    fn single_status_after_wake(&self) -> bool {
        let [site] = self.status_sites.as_slice() else {
            return false;
        };
        self.fired_entry.is_some_and(|fired| site.entry > fired)
            && self
                .timer_anchor
                .is_some_and(|anchor| site.position > anchor.position && site.entry != anchor.entry)
    }

    fn disciplined(&self) -> bool {
        self.status_sites.len() == 1
            && self.single_status_after_wake()
            && self.early_status_calls() == 0
            && self.state_writes == 0
            && self.spawns == 0
    }
}

/// Function discovery (`engine::functions::*`) is legitimate machinery and
/// counts toward nothing here — the same exemption every discipline audit in
/// this registry grants.
fn polling_audit(transcript: &Value, names: &Names) -> PollingAudit {
    let mut audit = PollingAudit::default();
    let mut position = 0usize;
    for (entry_index, entry) in transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        if let Some(custom) = entry.get("custom") {
            if custom.get("custom_type").and_then(Value::as_str) == Some("trigger_fired")
                && custom.pointer("/data/label").and_then(Value::as_str)
                    == Some(names.timer_label.as_str())
                && audit.fired_entry.is_none()
            {
                audit.fired_entry = Some(entry_index);
            }
            continue;
        }
        let Some(message) = entry.get("message") else {
            continue;
        };
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        for block in message
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some((function_id, arguments)) = normalized_block_call(block) else {
                continue;
            };
            let site = CallSite {
                entry: entry_index,
                position,
            };
            position += 1;
            match function_id {
                "engine::register_trigger" => {
                    audit.registrations += 1;
                    if is_signal_nap_timer(function_id, arguments, &names.timer_label) {
                        audit.matching_timers += 1;
                        if audit.timer_anchor.is_none() {
                            audit.timer_anchor = Some(site);
                        }
                    }
                }
                "state::set" | "state::delete" => audit.state_writes += 1,
                "harness::spawn" => audit.spawns += 1,
                id if id == names.signal_function => audit.status_sites.push(site),
                _ => {}
            }
        }
    }
    audit
}

fn normalized_block_call(block: &Value) -> Option<(&str, &Value)> {
    if block.get("type").and_then(Value::as_str) != Some("function_call") {
        return None;
    }
    let function = block.get("function_id")?.as_str()?;
    let arguments = block.get("arguments")?;
    if function == "agent_trigger" {
        return Some((
            arguments.get("function")?.as_str()?,
            arguments.get("payload")?,
        ));
    }
    Some((function, arguments))
}

fn is_signal_nap_timer(function_id: &str, arguments: &Value, label: &str) -> bool {
    function_id == "engine::register_trigger"
        && arguments.get("trigger_type").and_then(Value::as_str) == Some("timer")
        && arguments
            .pointer("/config/in_ms")
            .and_then(Value::as_u64)
            .is_some_and(|in_ms| (NAP_MS_MIN..=NAP_MS_MAX).contains(&in_ms))
        && arguments.get("label").and_then(Value::as_str) == Some(label)
        && common::requested_once(arguments)
        && common::is_wake_registration(arguments)
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let names = Names::new(run_id);
        let token = signal_token(run_id);
        let audit = polling_audit(&observation.transcript, &names);
        let label_records: Vec<_> = common::trigger_fired_records(&observation.transcript)
            .into_iter()
            .filter(|record| {
                record.get("label").and_then(Value::as_str) == Some(names.timer_label.as_str())
            })
            .collect();
        let timer_fired_once = label_records.len() == 1
            && label_records[0].get("retired").and_then(Value::as_bool) == Some(true);
        let active_bindings = common::active_binding_count(context, &names.root_session).await?;

        let signal_reported = observation.response.contains(&token);
        let no_polling = audit.disciplined();
        let single_clean_wake = audit.registrations == 1
            && audit.matching_timers == 1
            && timer_fired_once
            && active_bindings == 0;
        let response_chars = observation.response.chars().count();
        let no_errors = observation.metrics.totals.function_call_errors == 0;
        let quiet = response_chars <= RESPONSE_CHAR_BUDGET && no_errors;

        Ok(assessment::build_evaluation([
            SIGNAL_REPORTED.full_or_zero(
                signal_reported,
                "final response compared with the exact run-scoped signal token",
            ),
            NO_POLLING.full_or_zero(
                no_polling,
                format!(
                    "status_calls={}, early_status_calls={}, after_wake={}, state_writes={}, \
                     spawns={}",
                    audit.status_sites.len(),
                    audit.early_status_calls(),
                    audit.single_status_after_wake(),
                    audit.state_writes,
                    audit.spawns
                ),
            ),
            SINGLE_CLEAN_WAKE.full_or_zero(
                single_clean_wake,
                format!(
                    "registrations={}, matching_timers={}, timer_fired_once={timer_fired_once}, \
                     active_bindings={active_bindings}",
                    audit.registrations, audit.matching_timers
                ),
            ),
            QUIET_REPORT.full_or_zero(
                quiet,
                format!(
                    "observed {response_chars} character(s) (budget {RESPONSE_CHAR_BUDGET}), \
                     function_errors={}",
                    observation.metrics.totals.function_call_errors
                ),
            ),
        ]))
    })
}

fn capture<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let names = Names::new(run_id);
        let token = signal_token(run_id);
        let audit = polling_audit(&observation.transcript, &names);
        let signal_reported = observation.response.contains(&token);
        let no_polling = audit.disciplined();
        let reported_token = if signal_reported {
            token
        } else {
            String::new()
        };
        let provenance = if signal_reported && no_polling {
            vec![ProvenanceEvidence {
                kind: "function".to_string(),
                source_id: names.signal_function.clone(),
                relation: "gated_signal".to_string(),
            }]
        } else {
            Vec::new()
        };
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "signal_report".to_string(),
            content: json!({
                "token": reported_token,
                "status_calls": audit.status_sites.len(),
                "polled_early": audit.polled_early(),
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: "signal_reported".to_string(),
                    passed: signal_reported,
                    reason: "final response compared with the exact run-scoped signal token"
                        .to_string(),
                },
                CapturedInvariant {
                    id: "no_polling".to_string(),
                    passed: no_polling,
                    reason: format!(
                        "status_calls={}, early_status_calls={}, state_writes={}, spawns={}",
                        audit.status_sites.len(),
                        audit.early_status_calls(),
                        audit.state_writes,
                        audit.spawns
                    ),
                },
            ],
            provenance,
        }])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "signal_report".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["token", "status_calls", "polled_early"],
                "properties": {
                    "token": { "type": "string" },
                    "status_calls": { "type": "integer", "minimum": 0 },
                    "polled_early": { "type": "boolean" }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 4_096,
        }],
        invariants: vec![
            InvariantSpec {
                id: "signal_reported".to_string(),
                description: "The final response contains the exact run-scoped signal token."
                    .to_string(),
            },
            InvariantSpec {
                id: "no_polling".to_string(),
                description:
                    "The status function was called exactly once, strictly after the timer wake."
                        .to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let names = Names::new(run_id);
        let listed = context
            .trigger_value(
                "harness::triggers::list",
                json!({ "session_id": names.root_session }),
            )
            .await?;
        for subscription_id in listed
            .get("subscriptions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|subscription| subscription.get("subscription_id").and_then(Value::as_str))
        {
            let _: Value = context
                .trigger(
                    "harness::triggers::unregister",
                    json!({
                        "session_id": names.root_session,
                        "subscription_id": subscription_id,
                    }),
                )
                .await?;
        }
        Ok(())
    })
}

struct Names {
    root_session: String,
    timer_label: String,
    signal_function: String,
}

impl Names {
    fn new(run_id: &str) -> Self {
        Self {
            root_session: format!("e2e_{run_id}"),
            timer_label: format!("signal-nap:{run_id}"),
            signal_function: signal_function_id(run_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_tokens_are_run_scoped_and_stable() {
        let first = signal_token("attempt-a");
        assert_eq!(first, signal_token("attempt-a"));
        assert_ne!(first, signal_token("attempt-b"));
        assert!(first.starts_with("SIG-"));
        assert_eq!(first.len(), 20);
        assert!(first[4..]
            .chars()
            .all(|character| character.is_ascii_hexdigit()));
    }

    #[test]
    fn signal_response_gates_on_the_elapsed_time() {
        let early = signal_response(Duration::from_secs(10), "SIG-x");
        assert_eq!(early.status, "pending");
        assert!(early.token.is_none());
        assert!(early.hint.is_some());

        let boundary = signal_response(Duration::from_secs(READY_AFTER_SECONDS), "SIG-x");
        assert_eq!(boundary.status, "ready");
        assert_eq!(boundary.token.as_deref(), Some("SIG-x"));

        let late = signal_response(Duration::from_secs(READY_AFTER_SECONDS + 1), "SIG-x");
        assert_eq!(late.status, "ready");
        assert_eq!(late.token.as_deref(), Some("SIG-x"));
        assert!(late.hint.is_none());
    }

    fn timer_arguments(label: &str, in_ms: u64) -> Value {
        json!({
            "trigger_type": "timer",
            "label": label,
            "once": true,
            "config": { "in_ms": in_ms },
        })
    }

    #[test]
    fn timer_matcher_accepts_only_the_labeled_wake_only_once_timer() {
        let label = "signal-nap:run";
        for in_ms in [NAP_MS_MIN, 60_000, NAP_MS_MAX] {
            assert!(is_signal_nap_timer(
                "engine::register_trigger",
                &timer_arguments(label, in_ms),
                label
            ));
        }
        for in_ms in [NAP_MS_MIN - 1, NAP_MS_MAX + 1] {
            assert!(!is_signal_nap_timer(
                "engine::register_trigger",
                &timer_arguments(label, in_ms),
                label
            ));
        }
        assert!(!is_signal_nap_timer(
            "engine::register_trigger",
            &timer_arguments("other-label", 60_000),
            label
        ));
        assert!(!is_signal_nap_timer(
            "state::set",
            &timer_arguments(label, 60_000),
            label
        ));

        let mut not_once = timer_arguments(label, 60_000);
        not_once["once"] = json!(false);
        assert!(!is_signal_nap_timer(
            "engine::register_trigger",
            &not_once,
            label
        ));

        let mut targeted = timer_arguments(label, 60_000);
        targeted["function_id"] = json!("state::set");
        assert!(!is_signal_nap_timer(
            "engine::register_trigger",
            &targeted,
            label
        ));
    }

    fn transcript_of(entries: Vec<Value>) -> Value {
        json!({ "messages": entries })
    }

    fn assistant_entry(calls: Vec<Value>) -> Value {
        json!({ "message": { "role": "assistant", "content": calls } })
    }

    fn call_block(function_id: &str, arguments: Value) -> Value {
        json!({
            "type": "function_call",
            "id": "call",
            "function_id": function_id,
            "arguments": arguments,
        })
    }

    fn fired_entry(label: &str) -> Value {
        json!({
            "custom": {
                "custom_type": "trigger_fired",
                "data": {
                    "label": label,
                    "retired": true,
                    "once": true,
                    "target": "harness::send",
                }
            }
        })
    }

    #[test]
    fn polling_audit_accepts_a_single_post_wake_status_call() {
        let names = Names::new("audit-run");
        let transcript = transcript_of(vec![
            assistant_entry(vec![
                call_block("engine::functions::list", json!({ "search": "e2etest" })),
                call_block(
                    "engine::register_trigger",
                    timer_arguments(&names.timer_label, 60_000),
                ),
            ]),
            fired_entry(&names.timer_label),
            assistant_entry(vec![call_block(&names.signal_function, json!({}))]),
        ]);
        let audit = polling_audit(&transcript, &names);
        assert!(audit.disciplined(), "function discovery is exempt");
        assert!(!audit.polled_early());
        assert_eq!(audit.registrations, 1);
        assert_eq!(audit.matching_timers, 1);
        assert_eq!(audit.status_sites.len(), 1);
    }

    #[test]
    fn polling_audit_rejects_a_status_call_before_the_registration() {
        let names = Names::new("audit-run");
        let transcript = transcript_of(vec![
            assistant_entry(vec![call_block(&names.signal_function, json!({}))]),
            assistant_entry(vec![call_block(
                "engine::register_trigger",
                timer_arguments(&names.timer_label, 60_000),
            )]),
            fired_entry(&names.timer_label),
        ]);
        let audit = polling_audit(&transcript, &names);
        assert!(!audit.disciplined());
        assert!(audit.polled_early());
        assert_eq!(audit.early_status_calls(), 1);
    }

    #[test]
    fn polling_audit_rejects_two_status_calls_even_after_the_wake() {
        let names = Names::new("audit-run");
        let transcript = transcript_of(vec![
            assistant_entry(vec![call_block(
                "engine::register_trigger",
                timer_arguments(&names.timer_label, 60_000),
            )]),
            fired_entry(&names.timer_label),
            assistant_entry(vec![call_block(&names.signal_function, json!({}))]),
            // The second call arrives through the agent_trigger wrapper and
            // must still be counted after normalization.
            assistant_entry(vec![call_block(
                "agent_trigger",
                json!({ "function": names.signal_function, "payload": {} }),
            )]),
        ]);
        let audit = polling_audit(&transcript, &names);
        assert_eq!(audit.status_sites.len(), 2);
        assert!(!audit.polled_early());
        assert!(!audit.disciplined());
    }

    #[test]
    fn polling_audit_rejects_a_status_call_sharing_the_registration_message() {
        let names = Names::new("audit-run");
        let transcript = transcript_of(vec![
            fired_entry(&names.timer_label),
            assistant_entry(vec![
                call_block(
                    "engine::register_trigger",
                    timer_arguments(&names.timer_label, 60_000),
                ),
                call_block(&names.signal_function, json!({})),
            ]),
        ]);
        let audit = polling_audit(&transcript, &names);
        assert!(!audit.single_status_after_wake());
        assert!(!audit.disciplined());
    }

    #[test]
    fn polling_audit_counts_state_writes_and_spawns_as_indiscipline() {
        let names = Names::new("audit-run");
        let transcript = transcript_of(vec![
            assistant_entry(vec![
                call_block(
                    "engine::register_trigger",
                    timer_arguments(&names.timer_label, 60_000),
                ),
                call_block(
                    "state::set",
                    json!({ "scope": "s", "key": "k", "value": 1 }),
                ),
                call_block("harness::spawn", json!({ "prompt": "poll for me" })),
            ]),
            fired_entry(&names.timer_label),
            assistant_entry(vec![call_block(&names.signal_function, json!({}))]),
        ]);
        let audit = polling_audit(&transcript, &names);
        assert_eq!(audit.state_writes, 1);
        assert_eq!(audit.spawns, 1);
        assert!(!audit.disciplined());
    }

    #[test]
    fn materialized_case_is_reproducible_and_l2_stateful() {
        use super::super::ComplexityTier;

        let first = materialize("attempt-a", 17).unwrap();
        let retry = materialize("attempt-b", 17).unwrap();
        first.validate().unwrap();
        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs, retry.case.inputs);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert_eq!(first.case.complexity.tier, ComplexityTier::L2Stateful);
        assert_eq!(
            usize::from(first.case.complexity.profile.artifact_count),
            first.case.deliverable_contract.artifacts.len()
        );
        assert!(first.capture.is_some());
        assert!(first.case.deliverable_contract.capture_before_cleanup);
        assert!(first.case.deliverable_contract.provenance_required);
    }
}
