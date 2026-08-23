//! `wake_chain_soak` — long-horizon wake-chain endurance, one rung per case.
//!
//! A chain of N one-shot timer wakes: each woken turn advances one durable
//! counter and arms the next timer, so a rung of N ticks keeps a session
//! alive for roughly N x 5 seconds of scheduler real time. No other scenario
//! in this registry runs past minutes; this ladder measures whether the
//! scheduler and session manager stay reliable over long horizons. The case
//! seed selects the rung (`RUNGS`: 5, 20, or 50 ticks), so every rung is its
//! own longitudinally comparable case, exactly like `fanout_ladder`.
//!
//! Label convention (single repeated label): every timer registration carries
//! the same top-level label `soak-tick:{run_id}` — the run-scoped prefix with
//! no per-tick suffix — and fired records are matched on that exact label.
//! The audits enforce this one convention end to end.
//!
//! Interleaving anchor for `monotonic_progress`: `trigger_fired` records live
//! in the same transcript `messages` array as the assistant's calls, so the
//! audit walks that array once and emits Write/Arm/Fired events in transcript
//! order. When all N fired records appear in the walk, the `fired-positional`
//! anchor gates: every arming call must trail the wake record two slots
//! behind it (a one-slot tolerance, because a transport may post a wake
//! record just after the turn it woke instead of just before), which rejects
//! front-loading the whole chain in one burst. When fired records lack
//! usable positions in the walk, the audit falls back to the
//! `call-alternation` anchor: counter writes and registrations must strictly
//! alternate `W(0), A, W(1), A, ..., A, W(N)` — sound because a turn can only
//! follow a wake and a second arm without an intervening write breaks the
//! pattern, while `wake_integrity` still demands exactly N retired one-shot
//! fired records.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::{
    common, ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture,
    ComplexityProfile, DeliverableCaptureFuture, DeliverableContract, EvaluationFuture,
    ExecutionPolicy, InvariantSpec, MaterializedScenario, ObjectiveEvaluation, ProvenanceEvidence,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "wake_chain_soak";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "soak_trace";

const COUNTER_KEY: &str = "chain-counter";
/// The interval the prompt requests for every tick.
const INTERVAL_MS: u64 = 5_000;
/// Accepted per-registration bounds around the requested interval.
const MIN_TICK_INTERVAL_MS: u64 = 3_000;
const MAX_TICK_INTERVAL_MS: u64 = 10_000;
/// The completion reply is one marker line; anything long is chain noise.
const MAX_REPORT_CHARS: usize = 200;

/// One ladder rung: how many timer ticks the chain must sustain. The seed is
/// the case selector, exactly like `fanout_ladder` and `context_pressure`.
#[derive(Debug, Clone, Copy)]
struct Rung {
    seed: u64,
    ticks: u8,
}

pub const CANONICAL_SEED: u64 = 5001;
const RUNGS: [Rung; 3] = [
    Rung {
        seed: 5001,
        ticks: 5,
    },
    Rung {
        seed: 5002,
        ticks: 20,
    },
    Rung {
        seed: 5003,
        ticks: 50,
    },
];

const CHAIN_COMPLETED: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "chain_completed",
    35,
    "The durable counter reaches exactly the rung's tick count and the completion marker is reported.",
    EvaluationDimension::Deliverable,
);
const WAKE_INTEGRITY: AssessmentSpec = AssessmentSpec::hard_gated(
    "wake_integrity",
    30,
    "Every tick is one one-shot wake-only timer with the tick label that fires once, retires, and leaves no binding armed.",
);
const MONOTONIC_PROGRESS: AssessmentSpec = AssessmentSpec::hard_gated(
    "monotonic_progress",
    20,
    "The counter advances 0..=N in order with no skips or repeats, and each next timer is armed only after the previous wake.",
);
const QUIET_CHAIN: AssessmentSpec = AssessmentSpec::score_only(
    "quiet_chain",
    15,
    "The chain runs without function errors or stray calls and reports in a single short line.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    CHAIN_COMPLETED,
    WAKE_INTEGRITY,
    MONOTONIC_PROGRESS,
    QUIET_CHAIN,
];

fn rung(seed: u64) -> Rung {
    RUNGS
        .iter()
        .copied()
        .find(|rung| rung.seed == seed)
        .unwrap_or_else(|| RUNGS[(seed as usize) % RUNGS.len()])
}

fn report_marker(ticks: u8) -> String {
    format!("SOAK-COMPLETE {ticks}")
}

/// The response must carry this rung's marker as a whole token: a following
/// digit would make it another rung's marker (`SOAK-COMPLETE 5` is not
/// satisfied by `SOAK-COMPLETE 50`).
fn report_completes(response: &str, ticks: u8) -> bool {
    let marker = report_marker(ticks);
    response.match_indices(&marker).any(|(position, matched)| {
        response[position + matched.len()..]
            .chars()
            .next()
            .is_none_or(|next| !next.is_ascii_digit())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id, rung(CANONICAL_SEED))
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let rung = rung(seed);
    let case = ScenarioCase::new(
        ID,
        VERSION,
        seed,
        json!({
            "ticks": rung.ticks,
            "interval_ms": INTERVAL_MS,
            "counter_key": COUNTER_KEY,
            "report_marker": report_marker(rung.ticks),
            "ladder_rungs": RUNGS.iter().map(|rung| rung.ticks).collect::<Vec<_>>(),
        }),
        complexity_profile(rung.ticks),
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::state".to_string(),
            "iii::triggers".to_string(),
        ],
        deliverable_contract(rung.ticks),
    )?
    // Two calls per tick (the counter write and the next arm) plus the
    // initial write and the final report: the chain itself is the workload.
    .with_minimum_expected_work(2 + 2 * u64::from(rung.ticks))?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(namespace, rung),
        case,
        capture: Some(capture),
    })
}

/// The rung scales the profile, but never its tier: the ladder tops out at
/// 50 ticks (well inside `u8`), `wake_cycles` alone never lifts a profile
/// past L2Stateful, and `ambiguity_level` stays 0 so the L5Adaptive branch
/// is unreachable by construction. Every rung derives L2Stateful.
fn complexity_profile(ticks: u8) -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 1,
        dependency_depth: 1,
        external_systems: 1,
        state_transitions: u16::from(ticks) + 1,
        wake_cycles: ticks,
        artifact_count: 1,
        ..ComplexityProfile::default()
    }
}

fn scenario_for_case(run_id: &str, rung: Rung) -> ScenarioSpec {
    let names = Names::new(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: prompt(&names, rung.ticks),
        filesystem_root: None,
        execution: ExecutionPolicy {
            // One live turn plus one woken turn per tick, with slack for
            // discovery and the odd re-prompt.
            max_turns: 8 + 2 * u32::from(rung.ticks),
            max_output_tokens: Some(4_096),
            // Unbounded on purpose: a soak run is long because the subject
            // waits, not because it spends. Capping total tokens would turn
            // scheduler endurance into a token-budget test; spend still shows
            // up in the Efficiency dimension.
            max_total_tokens: None,
            // Each tick resolves in about five seconds, so 120 seconds with
            // no observable progress is decisively stuck rather than waiting.
            stuck_timeout_seconds: 120,
            max_validation_retries: None,
        },
        denied_functions: &[],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: None,
        evaluate,
        cleanup: Some(cleanup),
    }
}

fn prompt(names: &Names, ticks: u8) -> String {
    format!(
        r#"Run a sustained wake-chain soak of {ticks} timer ticks in the isolated state scope `{scope}`.

The control plane is parent-owned: every timer is a one-shot wake of this session, and the
durable counter at `{scope}` / `{counter_key}` is the chain's only state. Follow the tick
protocol exactly.

Starting now, in this first turn and in this order:

1. Write exactly `{{ "count": 0 }}` to `{scope}` / `{counter_key}`.
2. Arm exactly one timer: trigger type `timer`, `in_ms: {interval_ms}` (about five seconds),
   one-shot, top-level label exactly `{tick_label}`, and no function target so it wakes this
   session. Register nothing else.
3. End the turn immediately without replying.

Each time a timer wake arrives, in that woken turn and in this order:

1. Advance the counter exactly once: a single write of `{{ "count": <k> }}` to
   `{scope}` / `{counter_key}`, where `<k>` is one more than the stored count. You may first
   read the counter once with `state::get`.
2. If `<k>` is less than {ticks}, arm exactly one next timer the same way — same trigger type,
   same `in_ms`, same label `{tick_label}`, one-shot, wake-only — and end the turn without
   replying.
3. If `<k>` equals {ticks}, arm nothing and reply with exactly one line, `{marker}`, and
   nothing else.

Never arm two timers at once, never arm the next timer before that turn's counter write, and
never write the counter outside a woken turn — the initial `{{ "count": 0 }}` is the only
exception. Call nothing besides these counter reads and writes and these timer registrations."#,
        scope = names.scope,
        counter_key = COUNTER_KEY,
        interval_ms = INTERVAL_MS,
        tick_label = names.tick_label,
        marker = report_marker(ticks),
    )
}

fn case_ticks(case: &ScenarioCase) -> anyhow::Result<u8> {
    case.inputs
        .get("ticks")
        .and_then(Value::as_u64)
        .and_then(|ticks| u8::try_from(ticks).ok())
        .filter(|ticks| *ticks > 0)
        .ok_or_else(|| anyhow::anyhow!("wake_chain_soak case is missing a positive ticks input"))
}

async fn observed_counter(context: &E2eContext, names: &Names) -> anyhow::Result<Value> {
    Ok(common::state_value(
        context
            .trigger_value(
                "state::get",
                json!({ "scope": names.scope, "key": COUNTER_KEY }),
            )
            .await?,
    ))
}

/// The tick-label convention's timer matcher: one-shot, wake-only, labeled
/// exactly `soak-tick:{run_id}`, with `in_ms` inside the accepted band.
fn is_tick_timer_registration(function_id: &str, arguments: &Value, names: &Names) -> bool {
    function_id == "engine::register_trigger"
        && arguments.get("trigger_type").and_then(Value::as_str) == Some("timer")
        && arguments
            .pointer("/config/in_ms")
            .and_then(Value::as_u64)
            .is_some_and(|in_ms| (MIN_TICK_INTERVAL_MS..=MAX_TICK_INTERVAL_MS).contains(&in_ms))
        && arguments.get("label").and_then(Value::as_str) == Some(names.tick_label.as_str())
        && common::requested_once(arguments)
        && common::is_wake_registration(arguments)
}

fn tick_write(function_id: &str, arguments: &Value, names: &Names) -> Option<Value> {
    (function_id == "state::set"
        && arguments.get("scope").and_then(Value::as_str) == Some(names.scope.as_str())
        && arguments.get("key").and_then(Value::as_str) == Some(COUNTER_KEY))
    .then(|| arguments.get("value").cloned().unwrap_or(Value::Null))
}

fn is_allowed_call(function_id: &str, arguments: &Value, names: &Names) -> bool {
    // Function discovery is legitimate machinery, not chain noise — the same
    // exemption every discipline audit in this registry grants.
    if function_id.starts_with("engine::functions::") {
        return true;
    }
    // Registration shape and count are wake_integrity's business.
    if function_id == "engine::register_trigger" {
        return true;
    }
    (function_id == "state::set" || function_id == "state::get")
        && arguments.get("scope").and_then(Value::as_str) == Some(names.scope.as_str())
        && arguments.get("key").and_then(Value::as_str) == Some(COUNTER_KEY)
}

#[derive(Debug, Clone, PartialEq)]
enum ChainEvent {
    Write(Value),
    Arm,
    Fired,
}

/// One pass over the transcript's `messages` array, in order: counter writes
/// and tick-timer registrations from assistant function-call blocks, plus the
/// tick-labeled `trigger_fired` records — all as positioned events.
fn chain_events(transcript: &Value, names: &Names) -> Vec<ChainEvent> {
    let mut events = Vec::new();
    for entry in transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(custom) = entry.get("custom") {
            if custom.get("custom_type").and_then(Value::as_str) == Some("trigger_fired")
                && custom.pointer("/data/label").and_then(Value::as_str)
                    == Some(names.tick_label.as_str())
            {
                events.push(ChainEvent::Fired);
            }
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
            if let Some(value) = tick_write(function_id, arguments, names) {
                events.push(ChainEvent::Write(value));
            } else if is_tick_timer_registration(function_id, arguments, names) {
                events.push(ChainEvent::Arm);
            }
        }
    }
    events
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

struct ChainAudit {
    write_count: usize,
    arm_count: usize,
    fired_count: usize,
    /// Counter writes and arms strictly alternate `W(0), A, W(1), ..., A,
    /// W(N)` with the exact values 0..=N — the call-alternation anchor.
    ordered_calls: bool,
    /// The fired-positional anchor (when all N fired records have walk
    /// positions): arm j must trail fired j-2, a one-slot tolerance that
    /// still rejects arming the whole chain before any wake.
    causal_arms: bool,
    /// The unshifted ideal `W(0), [A, F, W(k)] for k in 1..=N` — reported for
    /// observability, never required on its own.
    strict_interleave: bool,
    anchor: &'static str,
    passed: bool,
}

fn chain_audit(events: &[ChainEvent], ticks: u8) -> ChainAudit {
    let ticks = usize::from(ticks);
    let write_count = events
        .iter()
        .filter(|event| matches!(event, ChainEvent::Write(_)))
        .count();
    let arm_positions: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| matches!(event, ChainEvent::Arm))
        .map(|(position, _)| position)
        .collect();
    let fired_positions: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| matches!(event, ChainEvent::Fired))
        .map(|(position, _)| position)
        .collect();

    let call_events: Vec<&ChainEvent> = events
        .iter()
        .filter(|event| !matches!(event, ChainEvent::Fired))
        .collect();
    let ordered_calls = call_events.len() == 2 * ticks + 1
        && call_events.iter().enumerate().all(|(position, event)| {
            if position % 2 == 0 {
                matches!(event, ChainEvent::Write(value) if *value == json!({ "count": position / 2 }))
            } else {
                matches!(event, ChainEvent::Arm)
            }
        });

    let positions_usable = fired_positions.len() == ticks;
    let causal_arms = !positions_usable
        || arm_positions
            .iter()
            .enumerate()
            .all(|(index, position)| match index.checked_sub(2) {
                None => true,
                Some(fired_index) => fired_positions
                    .get(fired_index)
                    .is_some_and(|fired_position| position > fired_position),
            });

    let strict_interleave = events.len() == 1 + 3 * ticks
        && events.iter().enumerate().all(|(position, event)| {
            if position == 0 {
                return matches!(event, ChainEvent::Write(value) if *value == json!({ "count": 0 }));
            }
            match (position - 1) % 3 {
                0 => matches!(event, ChainEvent::Arm),
                1 => matches!(event, ChainEvent::Fired),
                _ => {
                    let count = (position - 1) / 3 + 1;
                    matches!(event, ChainEvent::Write(value) if *value == json!({ "count": count }))
                }
            }
        });

    ChainAudit {
        write_count,
        arm_count: arm_positions.len(),
        fired_count: fired_positions.len(),
        ordered_calls,
        causal_arms,
        strict_interleave,
        anchor: if positions_usable {
            "fired-positional"
        } else {
            "call-alternation"
        },
        passed: ordered_calls && causal_arms,
    }
}

struct WakeAudit {
    registration_calls: usize,
    tick_registrations: usize,
    tick_fired: usize,
    retired_fired: usize,
    registrations_exact: bool,
    fired_exact: bool,
}

fn wake_audit(transcript: &Value, names: &Names, ticks: u8) -> WakeAudit {
    let calls = common::function_calls(transcript);
    let registration_calls = calls
        .iter()
        .filter(|call| call.function_id == "engine::register_trigger")
        .count();
    let tick_registrations = calls
        .iter()
        .filter(|call| is_tick_timer_registration(&call.function_id, &call.arguments, names))
        .count();
    let tick_records: Vec<&Value> = common::trigger_fired_records(transcript)
        .into_iter()
        .filter(|record| {
            record.get("label").and_then(Value::as_str) == Some(names.tick_label.as_str())
        })
        .collect();
    let retired_fired = tick_records
        .iter()
        .filter(|record| record.get("retired").and_then(Value::as_bool) == Some(true))
        .count();
    let tick_fired = tick_records.len();
    WakeAudit {
        registrations_exact: registration_calls == usize::from(ticks)
            && tick_registrations == registration_calls,
        fired_exact: tick_fired == usize::from(ticks) && retired_fired == tick_fired,
        registration_calls,
        tick_registrations,
        tick_fired,
        retired_fired,
    }
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move { evaluate_chain(context, observation, run_id).await })
}

async fn evaluate_chain(
    context: &E2eContext,
    observation: &ScenarioObservation,
    run_id: &str,
) -> anyhow::Result<ObjectiveEvaluation> {
    let names = Names::new(run_id);
    let ticks = case_ticks(&observation.case)?;

    let observed = observed_counter(context, &names).await?;
    let counter_final = observed == json!({ "count": ticks });
    let reported = report_completes(&observation.response, ticks);

    let wake = wake_audit(&observation.transcript, &names, ticks);
    let active_bindings = common::active_binding_count(context, &names.root_session).await?;

    let chain = chain_audit(&chain_events(&observation.transcript, &names), ticks);

    let calls = common::function_calls(&observation.transcript);
    let disciplined = calls
        .iter()
        .all(|call| is_allowed_call(&call.function_id, &call.arguments, &names));
    let errors = observation.metrics.totals.function_call_errors;
    let response_chars = observation.response.chars().count();
    let quiet = errors == 0 && disciplined && response_chars <= MAX_REPORT_CHARS;

    Ok(assessment::build_evaluation([
        CHAIN_COMPLETED.full_or_zero(
            counter_final && reported,
            format!("final_counter={observed}, expected_count={ticks}, marker_present={reported}"),
        ),
        WAKE_INTEGRITY.full_or_zero(
            wake.registrations_exact && wake.fired_exact && active_bindings == 0,
            format!(
                "registrations={}/{ticks} (tick-labeled={}), fired={}/{ticks} (retired={}), \
                 active_bindings={active_bindings}",
                wake.registration_calls,
                wake.tick_registrations,
                wake.tick_fired,
                wake.retired_fired
            ),
        ),
        MONOTONIC_PROGRESS.full_or_zero(
            chain.passed,
            format!(
                "writes={}, arms={}, fired_events={}, ordered_calls={}, causal_arms={}, \
                 strict_interleave={}, anchor={}",
                chain.write_count,
                chain.arm_count,
                chain.fired_count,
                chain.ordered_calls,
                chain.causal_arms,
                chain.strict_interleave,
                chain.anchor
            ),
        ),
        QUIET_CHAIN.full_or_zero(
            quiet,
            format!(
                "function_errors={errors}, disciplined={disciplined}, \
                 response_chars={response_chars}/{MAX_REPORT_CHARS}"
            ),
        ),
    ]))
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let names = Names::new(run_id);
        let ticks = case_ticks(&observation.case)?;
        let observed = observed_counter(context, &names).await?;
        let final_count = observed.get("count").and_then(Value::as_i64).unwrap_or(-1);
        let wake = wake_audit(&observation.transcript, &names, ticks);
        let active_bindings = common::active_binding_count(context, &names.root_session).await?;
        let counter_final = observed == json!({ "count": ticks });
        let reported = report_completes(&observation.response, ticks);
        let chain_completed = counter_final && reported;
        let wake_integrity = wake.registrations_exact && wake.fired_exact && active_bindings == 0;
        let provenance = if chain_completed && wake_integrity {
            vec![ProvenanceEvidence {
                kind: "session".to_string(),
                source_id: names.root_session.clone(),
                relation: "sustained_wake_chain".to_string(),
            }]
        } else {
            Vec::new()
        };

        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "soak_trace".to_string(),
            content: json!({
                "ticks": ticks,
                "final_count": final_count,
                "registrations": wake.tick_registrations,
                "fired": wake.tick_fired,
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: "chain_completed".to_string(),
                    passed: chain_completed,
                    reason: format!(
                        "final_counter={observed}, expected_count={ticks}, \
                         marker_present={reported}"
                    ),
                },
                CapturedInvariant {
                    id: "wake_integrity".to_string(),
                    passed: wake_integrity,
                    reason: format!(
                        "registrations={}/{ticks} (tick-labeled={}), fired={}/{ticks} \
                         (retired={}), active_bindings={active_bindings}",
                        wake.registration_calls,
                        wake.tick_registrations,
                        wake.tick_fired,
                        wake.retired_fired
                    ),
                },
            ],
            provenance,
        }])
    })
}

fn deliverable_contract(ticks: u8) -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "soak_trace".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["ticks", "final_count", "registrations", "fired"],
                "properties": {
                    "ticks": { "const": ticks },
                    "final_count": { "type": "integer" },
                    "registrations": { "type": "integer", "minimum": 0 },
                    "fired": { "type": "integer", "minimum": 0 }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 4_096,
        }],
        invariants: vec![
            InvariantSpec {
                id: "chain_completed".to_string(),
                description:
                    "The durable counter reached the rung's tick count and the completion marker \
                     was reported."
                        .to_string(),
            },
            InvariantSpec {
                id: "wake_integrity".to_string(),
                description:
                    "Every tick was a one-shot wake-only timer that fired once, retired, and left \
                     no binding armed."
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
        let _: Value = context
            .trigger(
                "state::delete",
                json!({ "scope": names.scope, "key": COUNTER_KEY }),
            )
            .await?;
        Ok(())
    })
}

struct Names {
    scope: String,
    root_session: String,
    /// The single repeated tick label: every registration and every fired
    /// record carries exactly this label, with no per-tick suffix.
    tick_label: String,
}

impl Names {
    fn new(run_id: &str) -> Self {
        Self {
            scope: format!("e2e:soak:{run_id}"),
            root_session: format!("e2e_{run_id}"),
            tick_label: format!("soak-tick:{run_id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_seeds_select_their_rungs_and_fall_back_by_modulo() {
        assert_eq!(rung(5001).ticks, 5);
        assert_eq!(rung(5002).ticks, 20);
        assert_eq!(rung(5003).ticks, 50);
        assert_eq!(rung(313).ticks, RUNGS[313 % RUNGS.len()].ticks);
        assert_eq!(rung(CANONICAL_SEED).ticks, RUNGS[0].ticks);
    }

    #[test]
    fn report_requires_the_exact_completion_marker() {
        assert!(report_completes("SOAK-COMPLETE 5", 5));
        assert!(report_completes("SOAK-COMPLETE 5\n", 5));
        assert!(report_completes("chain done\nSOAK-COMPLETE 20", 20));
        assert!(report_completes("SOAK-COMPLETE 50", 50));
        assert!(!report_completes("SOAK-COMPLETE 50", 5));
        assert!(!report_completes("SOAK-COMPLETE 4", 5));
        assert!(!report_completes("all done", 5));
    }

    fn registration(in_ms: u64, label: &str) -> Value {
        json!({
            "trigger_type": "timer",
            "config": { "in_ms": in_ms },
            "label": label,
            "once": true,
        })
    }

    #[test]
    fn timer_matcher_enforces_bounds_label_once_and_wake_only() {
        let names = Names::new("match-run");
        let label = names.tick_label.as_str();
        let ok = registration(5_000, label);
        assert!(is_tick_timer_registration(
            "engine::register_trigger",
            &ok,
            &names
        ));
        for in_ms in [3_000, 10_000] {
            assert!(is_tick_timer_registration(
                "engine::register_trigger",
                &registration(in_ms, label),
                &names
            ));
        }
        for in_ms in [2_999, 10_001] {
            assert!(!is_tick_timer_registration(
                "engine::register_trigger",
                &registration(in_ms, label),
                &names
            ));
        }
        assert!(!is_tick_timer_registration(
            "engine::register_trigger",
            &registration(5_000, "soak-tick:other-run"),
            &names
        ));
        assert!(!is_tick_timer_registration("state::set", &ok, &names));

        let mut not_once = registration(5_000, label);
        not_once["once"] = json!(false);
        assert!(!is_tick_timer_registration(
            "engine::register_trigger",
            &not_once,
            &names
        ));

        let mut targeted = registration(5_000, label);
        targeted["target"] = json!({ "function_id": "state::set" });
        assert!(!is_tick_timer_registration(
            "engine::register_trigger",
            &targeted,
            &names
        ));
    }

    #[test]
    fn quiet_chain_allows_only_counter_traffic_and_registrations() {
        let names = Names::new("quiet-run");
        let counter = json!({
            "scope": names.scope,
            "key": COUNTER_KEY,
            "value": { "count": 1 },
        });
        let read = json!({ "scope": names.scope, "key": COUNTER_KEY });
        assert!(is_allowed_call("state::set", &counter, &names));
        assert!(is_allowed_call("state::get", &read, &names));
        assert!(is_allowed_call(
            "engine::register_trigger",
            &json!({}),
            &names
        ));
        assert!(is_allowed_call(
            "engine::functions::list",
            &json!({}),
            &names
        ));
        let elsewhere = json!({ "scope": "elsewhere", "key": COUNTER_KEY });
        assert!(!is_allowed_call("state::set", &elsewhere, &names));
        assert!(!is_allowed_call("state::delete", &counter, &names));
        assert!(!is_allowed_call("harness::spawn", &json!({}), &names));
    }

    fn write(count: usize) -> ChainEvent {
        ChainEvent::Write(json!({ "count": count }))
    }

    fn chain(ticks: usize, with_fired: bool) -> Vec<ChainEvent> {
        let mut events = vec![write(0)];
        for count in 1..=ticks {
            events.push(ChainEvent::Arm);
            if with_fired {
                events.push(ChainEvent::Fired);
            }
            events.push(write(count));
        }
        events
    }

    #[test]
    fn monotonic_audit_accepts_the_exact_chain_on_either_anchor() {
        let strict = chain_audit(&chain(5, true), 5);
        assert!(strict.ordered_calls);
        assert!(strict.causal_arms);
        assert!(strict.strict_interleave);
        assert!(strict.passed);
        assert_eq!(strict.anchor, "fired-positional");

        let calls_only = chain_audit(&chain(5, false), 5);
        assert!(calls_only.ordered_calls);
        assert!(calls_only.passed);
        assert!(!calls_only.strict_interleave);
        assert_eq!(calls_only.anchor, "call-alternation");
    }

    #[test]
    fn monotonic_audit_rejects_skips_repeats_and_double_arms() {
        // Skip: the counter jumps from 1 to 3.
        let mut skipped = chain(5, true);
        let position = skipped.iter().position(|event| *event == write(2)).unwrap();
        skipped[position] = write(3);
        assert!(!chain_audit(&skipped, 5).passed);

        // Repeat: the counter writes 1 twice.
        let mut repeated = chain(5, true);
        let position = repeated
            .iter()
            .position(|event| *event == write(2))
            .unwrap();
        repeated[position] = write(1);
        assert!(!chain_audit(&repeated, 5).passed);

        // Double arm before the first wake.
        let mut double_armed = chain(5, true);
        double_armed.insert(1, ChainEvent::Arm);
        assert!(!chain_audit(&double_armed, 5).passed);

        // Truncated chain: the final write never lands.
        let mut truncated = chain(5, true);
        truncated.pop();
        assert!(!chain_audit(&truncated, 5).passed);

        // A trailing arm after the final write leaves a timer armed.
        let mut trailing = chain(5, true);
        trailing.push(ChainEvent::Arm);
        assert!(!chain_audit(&trailing, 5).passed);
    }

    #[test]
    fn monotonic_audit_rejects_a_front_loaded_chain() {
        // Every write and arm in one burst with all wake records trailing:
        // the counter looks right, but no tick was produced by a woken turn.
        let mut events = vec![write(0)];
        for count in 1..=5 {
            events.push(ChainEvent::Arm);
            events.push(write(count));
        }
        events.extend((0..5).map(|_| ChainEvent::Fired));
        let audit = chain_audit(&events, 5);
        assert!(audit.ordered_calls);
        assert!(!audit.causal_arms);
        assert!(!audit.passed);
        assert_eq!(audit.anchor, "fired-positional");
    }

    #[test]
    fn monotonic_audit_tolerates_wake_records_posted_after_their_turn() {
        // The transport posts each fired record after the turn it woke:
        // W0 A | W1 A | F1 | W2 A | F2 | ... | W5 | F5.
        let mut events = vec![write(0), ChainEvent::Arm];
        for count in 1..=5 {
            events.push(write(count));
            if count < 5 {
                events.push(ChainEvent::Arm);
            }
            events.push(ChainEvent::Fired);
        }
        let audit = chain_audit(&events, 5);
        assert!(audit.passed);
        assert!(!audit.strict_interleave);
        assert_eq!(audit.anchor, "fired-positional");
    }

    #[test]
    fn chain_events_walks_calls_and_wake_records_in_transcript_order() {
        let names = Names::new("walk-run");
        let transcript = json!({
            "messages": [
                { "message": { "role": "assistant", "content": [
                    { "type": "function_call", "id": "c0", "function_id": "agent_trigger",
                      "arguments": { "function": "state::set", "payload": {
                          "scope": names.scope, "key": COUNTER_KEY, "value": { "count": 0 } } } },
                    { "type": "function_call", "id": "c1",
                      "function_id": "engine::register_trigger",
                      "arguments": { "trigger_type": "timer", "config": { "in_ms": 5_000 },
                                     "label": names.tick_label, "once": true } }
                ]}},
                { "custom": { "custom_type": "trigger_fired",
                              "data": { "label": names.tick_label, "retired": true,
                                        "once": true } } },
                { "custom": { "custom_type": "trigger_fired",
                              "data": { "label": "someone-else", "retired": true } } },
                { "message": { "role": "assistant", "content": [
                    { "type": "text", "text": "SOAK-COMPLETE 1" },
                    { "type": "function_call", "id": "c2", "function_id": "state::set",
                      "arguments": { "scope": names.scope, "key": COUNTER_KEY,
                                     "value": { "count": 1 } } }
                ]}}
            ]
        });
        let events = chain_events(&transcript, &names);
        assert_eq!(
            events,
            vec![write(0), ChainEvent::Arm, ChainEvent::Fired, write(1)]
        );
        let audit = chain_audit(&events, 1);
        assert!(audit.passed);
        assert!(audit.strict_interleave);
    }

    #[test]
    fn every_rung_publishes_a_reproducible_l2_case_scaled_to_its_ticks() {
        use super::super::ComplexityTier;

        for rung_spec in RUNGS {
            let first = materialize("attempt-a", rung_spec.seed).unwrap();
            let retry = materialize("attempt-b", rung_spec.seed).unwrap();
            first.validate().unwrap();
            assert_eq!(first.case.case_id, retry.case.case_id);
            assert_eq!(first.case.inputs, retry.case.inputs);
            assert_ne!(first.spec.prompt, retry.spec.prompt);
            assert_eq!(first.case.complexity.tier, ComplexityTier::L2Stateful);
            assert_eq!(
                first.case.work.minimum_expected_work,
                2 + 2 * u64::from(rung_spec.ticks)
            );
            assert_eq!(
                first.spec.execution.max_turns,
                8 + 2 * u32::from(rung_spec.ticks)
            );
            assert_eq!(
                first.case.inputs.get("ticks").and_then(Value::as_u64),
                Some(u64::from(rung_spec.ticks))
            );
            assert_eq!(
                first.case.inputs.get("ladder_rungs"),
                Some(&json!([5, 20, 50]))
            );
            assert_eq!(first.case.deliverable_contract.artifacts.len(), 1);
            assert_eq!(
                usize::from(first.case.complexity.profile.artifact_count),
                first.case.deliverable_contract.artifacts.len()
            );
        }
    }
}
