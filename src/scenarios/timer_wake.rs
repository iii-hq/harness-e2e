use std::sync::Arc;
use std::time::{Duration, Instant};

use iii_sdk::RegisterFunction;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::{
    common, ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture,
    ComplexityProfile, DeliverableCaptureFuture, DeliverableContract, EvaluationFuture,
    ExecutionPolicy, InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "timer_wake";
const VERSION: u32 = 6;
const DELIVERABLE_ID: &str = "timer_result";

const RESULT_KEY: &str = "result";
const DELAY_MS: u64 = 6_000;
const READY_AFTER_SECONDS: u64 = 4;
const TIMER_ARMED: AssessmentSpec = AssessmentSpec::hard_gated(
    "timer_armed",
    20,
    "One wake-only relative timer is armed before any result write.",
);
const PARENT_WOKEN: AssessmentSpec = AssessmentSpec::hard_gated(
    "parent_woken",
    20,
    "The timer retires after waking the original session exactly once.",
);
const WAKE_ACTION: AssessmentSpec = AssessmentSpec::hard_gated(
    "wake_action",
    25,
    "The timer-woken turn persists the requested result.",
);
const NO_POLLING: AssessmentSpec = AssessmentSpec::hard_gated(
    "no_polling",
    20,
    "The gated status function is called exactly once after the timer wake, never before it.",
);
const CLEAN_COMPLETION: AssessmentSpec = AssessmentSpec::hard_gated(
    "clean_completion",
    15,
    "The root completes without children, errors, or surviving bindings.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    TIMER_ARMED,
    PARENT_WOKEN,
    WAKE_ACTION,
    NO_POLLING,
    CLEAN_COMPLETION,
];

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
struct SignalRequest {}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct SignalResponse {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
}

fn signal_token(run_id: &str) -> String {
    format!(
        "SIG-{:016x}",
        super::stable_seed(&format!("{ID}:{run_id}:signal"))
    )
}

fn signal_response(elapsed: Duration, token: &str) -> SignalResponse {
    if elapsed >= Duration::from_secs(READY_AFTER_SECONDS) {
        SignalResponse {
            status: "ready".to_string(),
            token: Some(token.to_string()),
        }
    } else {
        SignalResponse {
            status: "pending".to_string(),
            token: None,
        }
    }
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let names = Names::new(run_id);
        let token = signal_token(run_id);
        let registered_at = Arc::new(Instant::now());
        context.client().register_function(
            names.signal_function,
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
                "E2E gated timer signal: pending before the wake window and ready afterwards.",
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
            "delay_ms": DELAY_MS,
            "signal_ready_after_seconds": READY_AFTER_SECONDS,
            "result_key": RESULT_KEY,
            "expected": expected_result(),
        }),
        ComplexityProfile {
            planning_depth: 2,
            dependency_depth: 1,
            external_systems: 1,
            state_transitions: 4,
            wake_cycles: 1,
            artifact_count: 1,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::state".to_string(),
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
            r#"Test the parent-owned timer control plane in isolated state scope `{scope}`.

Register exactly one wake-only timer for roughly six seconds from now:

- use trigger type `timer` with `in_ms: {delay_ms}`;
- use the exact top-level label `{timer_label}` and mark it once;
- omit every function target so it wakes this session;
- do not spawn a child.

The gated function `{signal_function}` is already registered. Calling it before the wake is
polling and fails this task. After the timer registration succeeds, end the turn immediately
without calling it or writing state.

When the timer notification starts a new turn, call `{signal_function}` exactly once with `{{}}`.
It will return a ready `SIG-...` token. Then write exactly `{{ "status": "fired" }}` to `{scope}` /
`{result_key}` and respond briefly with both `timer fired` and the exact signal token. Leave no
binding armed."#,
            scope = names.scope,
            delay_ms = DELAY_MS,
            timer_label = names.timer_label,
            signal_function = names.signal_function,
            result_key = RESULT_KEY,
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 24,
            max_output_tokens: Some(8_192),
            max_total_tokens: Some(400_000),
            stuck_timeout_seconds: 120,
            max_validation_retries: None,
        },
        denied_functions: &[],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CallSite {
    entry: usize,
    position: usize,
}

#[derive(Debug, Default)]
struct TimerAudit {
    registrations: usize,
    matching_timers: usize,
    timer_anchor: Option<CallSite>,
    status_sites: Vec<CallSite>,
    fired_entry: Option<usize>,
    state_writes: usize,
    spawns: usize,
}

impl TimerAudit {
    fn early_status_calls(&self) -> usize {
        self.status_sites
            .iter()
            .filter(|site| self.fired_entry.is_none_or(|fired| site.entry < fired))
            .count()
    }

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
            && self.state_writes == 1
            && self.spawns == 0
    }
}

fn timer_audit(transcript: &Value, names: &Names) -> TimerAudit {
    let mut audit = TimerAudit::default();
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
                    if is_timer_arguments(arguments, &names.timer_label) {
                        audit.matching_timers += 1;
                        if audit.timer_anchor.is_none() {
                            audit.timer_anchor = Some(site);
                        }
                    }
                }
                "state::set" => audit.state_writes += 1,
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

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let names = Names::new(run_id);
        let token = signal_token(run_id);
        let audit = timer_audit(&observation.transcript, &names);
        let expected = observation
            .case
            .inputs
            .get("expected")
            .cloned()
            .unwrap_or(Value::Null);
        let observed = common::state_value(
            context
                .trigger_value(
                    "state::get",
                    json!({ "scope": names.scope, "key": RESULT_KEY }),
                )
                .await?,
        );
        let calls = common::function_calls(&observation.transcript);
        let writes: Vec<_> = calls
            .iter()
            .enumerate()
            .filter(|(_, call)| call.function_id == "state::set")
            .collect();
        let exact_write = writes.len() == 1
            && writes[0].1.arguments
                == json!({ "scope": names.scope, "key": RESULT_KEY, "value": expected });
        let timer_armed = audit.registrations == 1
            && audit.matching_timers == 1
            && writes.len() == 1
            && audit
                .timer_anchor
                .is_some_and(|timer| timer.position < writes[0].0);

        let records = common::trigger_fired_records(&observation.transcript)
            .into_iter()
            .filter(|record| {
                record.get("label").and_then(Value::as_str) == Some(names.timer_label.as_str())
            })
            .collect::<Vec<_>>();
        let timer_fired = records.len() == 1
            && records[0].get("retired").and_then(Value::as_bool) == Some(true)
            && records[0].get("once").and_then(Value::as_bool) == Some(true)
            && records[0].get("target").and_then(Value::as_str) == Some("harness::send");
        let root_only = observation.metrics.totals.sessions == 1
            && calls
                .iter()
                .all(|call| call.function_id != "harness::spawn");
        let active_bindings = common::active_binding_count(context, &names.root_session).await?;
        let no_errors = observation.metrics.totals.function_call_errors == 0;
        let response = observation.response.to_ascii_lowercase();
        let signal_reported = observation.response.contains(&token);
        let confirmed = response.contains("timer") && response.contains("fired") && signal_reported;

        let parent_woken = timer_fired && root_only;
        let wake_action = exact_write && observed == expected;
        let no_polling = audit.disciplined();
        let clean_completion = active_bindings == 0 && no_errors && confirmed;

        Ok(assessment::build_evaluation(
            if confirmed {
                crate::report::CompletionState::Completed
            } else {
                crate::report::CompletionState::TaskIncomplete
            },
            [
            TIMER_ARMED.full_or_zero(
                timer_armed,
                format!(
                    "registrations={}, timers={}, writes={}",
                    audit.registrations,
                    audit.matching_timers,
                    writes.len()
                ),
            ),
            PARENT_WOKEN.full_or_zero(
                parent_woken,
                format!("timer_fired={timer_fired}, root_only={root_only}"),
            ),
            WAKE_ACTION.full_or_zero(
                wake_action,
                format!("exact_write={exact_write}, observed={observed}"),
            ),
            NO_POLLING.full_or_zero(
                no_polling,
                format!(
                    "status_calls={}, early_status_calls={}, after_wake={}, state_writes={}, spawns={}",
                    audit.status_sites.len(),
                    audit.early_status_calls(),
                    audit.single_status_after_wake(),
                    audit.state_writes,
                    audit.spawns
                ),
            ),
            CLEAN_COMPLETION.full_or_zero(
                clean_completion,
                format!(
                    "active_bindings={active_bindings}, function_errors={}, confirmed={confirmed}",
                    observation.metrics.totals.function_call_errors
                ),
            ),
            ],
        ))
    })
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let names = Names::new(run_id);
        let token = signal_token(run_id);
        let audit = timer_audit(&observation.transcript, &names);
        let expected = observation
            .case
            .inputs
            .get("expected")
            .cloned()
            .unwrap_or(Value::Null);
        let observed = common::state_value(
            context
                .trigger_value(
                    "state::get",
                    json!({ "scope": names.scope, "key": RESULT_KEY }),
                )
                .await?,
        );
        let calls = common::function_calls(&observation.transcript);
        let writes = calls
            .iter()
            .filter(|call| call.function_id == "state::set")
            .collect::<Vec<_>>();
        let exact_write = writes.len() == 1
            && writes[0].arguments
                == json!({ "scope": names.scope, "key": RESULT_KEY, "value": expected });
        let records = common::trigger_fired_records(&observation.transcript);
        let timer_records = records
            .iter()
            .filter(|record| {
                record.get("label").and_then(Value::as_str) == Some(names.timer_label.as_str())
                    && record.get("target").and_then(Value::as_str) == Some("harness::send")
            })
            .collect::<Vec<_>>();
        let one_shot_wake = timer_records.len() == 1
            && timer_records[0].get("retired").and_then(Value::as_bool) == Some(true)
            && timer_records[0].get("once").and_then(Value::as_bool) == Some(true);
        let signal_reported = observation.response.contains(&token);
        let no_polling = audit.disciplined();
        let mut provenance = vec![ProvenanceEvidence {
            kind: "state_location".to_string(),
            source_id: format!("{}/{}", names.scope, RESULT_KEY),
            relation: "captured_after_timer_wake".to_string(),
        }];
        if no_polling && signal_reported {
            provenance.push(ProvenanceEvidence {
                kind: "function".to_string(),
                source_id: names.signal_function.clone(),
                relation: "called_once_after_timer_wake".to_string(),
            });
        }

        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "timer_result".to_string(),
            content: json!({
                "result": observed.clone(),
                "signal_token": signal_reported.then_some(token),
                "status_calls": audit.status_sites.len(),
                "polled_early": audit.early_status_calls() > 0,
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: "matches_expected_result".to_string(),
                    passed: observed == expected,
                    reason: format!("expected {expected}, observed {observed}"),
                },
                CapturedInvariant {
                    id: "single_wake_write".to_string(),
                    passed: exact_write,
                    reason: format!("observed {} state::set call(s)", writes.len()),
                },
                CapturedInvariant {
                    id: "one_shot_timer_retired".to_string(),
                    passed: one_shot_wake,
                    reason: format!("observed {} timer wake record(s)", timer_records.len()),
                },
                CapturedInvariant {
                    id: "no_polling".to_string(),
                    passed: no_polling,
                    reason: format!(
                        "status_calls={}, early_status_calls={}, after_wake={}",
                        audit.status_sites.len(),
                        audit.early_status_calls(),
                        audit.single_status_after_wake()
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
            kind: "timer_result".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["result", "signal_token", "status_calls", "polled_early"],
                "properties": {
                    "result": { "const": expected_result() },
                    "signal_token": { "type": "string", "pattern": "^SIG-[0-9a-f]{16}$" },
                    "status_calls": { "const": 1 },
                    "polled_early": { "const": false }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 4_096,
        }],
        invariants: vec![
            InvariantSpec {
                id: "matches_expected_result".to_string(),
                description: "The timer-woken turn persisted the exact result.".to_string(),
            },
            InvariantSpec {
                id: "single_wake_write".to_string(),
                description: "Exactly one state write followed the timer wake.".to_string(),
            },
            InvariantSpec {
                id: "one_shot_timer_retired".to_string(),
                description: "The timer delivered once and retired itself.".to_string(),
            },
            InvariantSpec {
                id: "no_polling".to_string(),
                description:
                    "The gated status function was called once, only after the timer wake."
                        .to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn expected_result() -> Value {
    json!({ "status": "fired" })
}

fn is_timer_arguments(arguments: &Value, label: &str) -> bool {
    arguments.get("trigger_type").and_then(Value::as_str) == Some("timer")
        && arguments
            .pointer("/config/in_ms")
            .and_then(Value::as_u64)
            .is_some_and(|in_ms| (3_000..=15_000).contains(&in_ms))
        && arguments.get("label").and_then(Value::as_str) == Some(label)
        && common::requested_once(arguments)
        && common::is_wake_registration(arguments)
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
        let _: Value = context
            .trigger(
                "state::delete",
                json!({ "scope": names.scope, "key": RESULT_KEY }),
            )
            .await?;
        Ok(())
    })
}

struct Names {
    scope: String,
    root_session: String,
    timer_label: String,
    signal_function: String,
}

impl Names {
    fn new(run_id: &str) -> Self {
        Self {
            scope: format!("e2e:timer:{run_id}"),
            root_session: format!("e2e_{run_id}"),
            timer_label: format!("timer-wake:{run_id}"),
            signal_function: format!(
                "e2etest::timer_signal_{}",
                super::validation_loop::suffix(run_id)
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn timer_arguments(label: &str) -> Value {
        json!({
            "trigger_type": "timer",
            "label": label,
            "once": true,
            "config": { "in_ms": DELAY_MS },
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

    fn state_write(names: &Names) -> Value {
        call_block(
            "state::set",
            json!({ "scope": names.scope, "key": RESULT_KEY, "value": expected_result() }),
        )
    }

    #[test]
    fn signal_is_gated_and_run_scoped() {
        let first = signal_token("attempt-a");
        assert_eq!(first, signal_token("attempt-a"));
        assert_ne!(first, signal_token("attempt-b"));
        assert_eq!(
            signal_response(Duration::from_secs(1), &first).status,
            "pending"
        );
        assert_eq!(
            signal_response(Duration::from_secs(READY_AFTER_SECONDS), &first)
                .token
                .as_deref(),
            Some(first.as_str())
        );
    }

    #[test]
    fn timer_matcher_requires_the_exact_once_wake() {
        let names = Names::new("run");
        assert!(is_timer_arguments(
            &timer_arguments(&names.timer_label),
            &names.timer_label
        ));
        let mut targeted = timer_arguments(&names.timer_label);
        targeted["function_id"] = json!("state::set");
        assert!(!is_timer_arguments(&targeted, &names.timer_label));
        assert!(!is_timer_arguments(
            &timer_arguments("other"),
            &names.timer_label
        ));
    }

    #[test]
    fn audit_accepts_one_status_call_after_the_wake() {
        let names = Names::new("run");
        let transcript = json!({ "messages": [
            assistant_entry(vec![call_block(
                "engine::register_trigger",
                timer_arguments(&names.timer_label),
            )]),
            fired_entry(&names.timer_label),
            assistant_entry(vec![
                call_block(&names.signal_function, json!({})),
                state_write(&names),
            ]),
        ] });
        assert!(timer_audit(&transcript, &names).disciplined());
    }

    #[test]
    fn audit_rejects_early_or_repeated_status_calls() {
        let names = Names::new("run");
        let early = json!({ "messages": [
            assistant_entry(vec![
                call_block(&names.signal_function, json!({})),
                call_block("engine::register_trigger", timer_arguments(&names.timer_label)),
            ]),
            fired_entry(&names.timer_label),
            assistant_entry(vec![state_write(&names)]),
        ] });
        assert!(!timer_audit(&early, &names).disciplined());

        let repeated = json!({ "messages": [
            assistant_entry(vec![call_block(
                "engine::register_trigger",
                timer_arguments(&names.timer_label),
            )]),
            fired_entry(&names.timer_label),
            assistant_entry(vec![
                call_block(&names.signal_function, json!({})),
                call_block(&names.signal_function, json!({})),
                state_write(&names),
            ]),
        ] });
        assert!(!timer_audit(&repeated, &names).disciplined());
    }
}
