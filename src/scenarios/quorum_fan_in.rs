//! `quorum_fan_in` — K-of-N completion: proceed on a 2-of-3 quorum and
//! actively stop the straggler instead of waiting for it.
//!
//! The coordinator arms one named-set barrier wake that deliberately expects
//! only the two quorum keys, then spawns three members in parallel. Members
//! 00 and 01 write their rows immediately; member 02 must first call a
//! suite-hosted straggle function that holds it for 25 seconds, so it cannot
//! finish before the quorum. When the barrier retires on the second row the
//! coordinator must stop the straggler's child session via `harness::stop`
//! (the session-stop surface pinned by
//! `tests/golden/schemas/harness.stop.json`) and report the two quorum
//! tokens — never the straggler's. The gates check that the stop happened
//! after the barrier retired and that the straggler key was never written:
//! the subject is measured on proceeding at quorum, not on draining the herd.

use std::collections::BTreeSet;
use std::time::Duration;

use iii_sdk::RegisterFunction;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::validation_loop::suffix;
use super::{
    common, ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture,
    ComplexityProfile, DeliverableCaptureFuture, DeliverableContract, EvaluationFuture,
    ExecutionPolicy, InvariantSpec, MaterializedScenario, ObjectiveEvaluation, ProvenanceEvidence,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "quorum_fan_in";
const VERSION: u32 = 2;
const DELIVERABLE_ID: &str = "quorum_record";

const MEMBER_COUNT: u8 = 3;
const QUORUM_INDEXES: [u8; 2] = [0, 1];
const STRAGGLER_INDEX: u8 = 2;
const REPORT_MARKER: &str = "QUORUM-2of3";
/// Long enough that the straggler can never beat the quorum, short enough
/// that a run which wrongly waits for it still finishes inside the stuck
/// timeout and fails on the gates instead of on the clock.
const STRAGGLE_DELAY: Duration = Duration::from_secs(25);

/// The session-stop surface confirmed by `tests/golden/schemas/harness.stop.json`
/// and `wire::CONTROL_PLANE`: `harness::stop` takes `{ "session_id": <id> }`
/// (plus an optional `turn_id`) and answers `{ "stopping": bool }`.
const STOP_FUNCTION_ID: &str = "harness::stop";

const QUORUM_REPORT: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "quorum_report",
    30,
    "The barrier-woken reply starts with the quorum marker and carries both quorum tokens verbatim while omitting the straggler token.",
    EvaluationDimension::Deliverable,
);
const QUORUM_WAKE: AssessmentSpec = AssessmentSpec::hard_gated(
    "quorum_wake",
    25,
    "Exactly one named-set barrier wake is armed before any spawn, expects exactly the two quorum keys, and retires on the second row.",
);
const STRAGGLER_STOPPED: AssessmentSpec = AssessmentSpec::hard_gated(
    "straggler_stopped",
    30,
    "After the barrier retires the coordinator stops a direct child session, and the straggler key is never written.",
);
const FAN_OUT_DISCIPLINE: AssessmentSpec = AssessmentSpec::score_only(
    "fan_out_discipline",
    15,
    "All three members are spawned in one coordinator response as direct children, with no function-call errors.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    QUORUM_REPORT,
    QUORUM_WAKE,
    STRAGGLER_STOPPED,
    FAN_OUT_DISCIPLINE,
];

/// A straggle call (the request body is ignored).
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct StraggleRequest {}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct StraggleResponse {
    pub status: String,
}

fn member_key(index: u8) -> String {
    format!("member-{index:02}")
}

/// Run-scoped member tokens: opaque to the subject before the run, but
/// handed to the coordinator inline, so the gates — not token secrecy — are
/// what force real delegation and a deliberate straggler omission.
fn member_token(run_id: &str, index: u8) -> String {
    format!(
        "QRM-{index}-{:012x}",
        super::stable_seed(&format!("{ID}:{run_id}:member-{index:02}")) & 0xffff_ffff_ffff
    )
}

fn expected_row(run_id: &str, index: u8) -> Value {
    json!({
        "member": index,
        "token": member_token(run_id, index),
    })
}

fn straggle_function_id(run_id: &str) -> String {
    format!("e2etest::straggle_{}", suffix(run_id))
}

/// The temporary straggler gate: registered on the suite's own engine
/// connection, alive exactly as long as this process.
fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        context.client().register_function(
            straggle_function_id(run_id),
            RegisterFunction::new_async(move |_request: StraggleRequest| async move {
                tokio::time::sleep(STRAGGLE_DELAY).await;
                Ok::<StraggleResponse, iii_sdk::errors::Error>(StraggleResponse {
                    status: "released".to_string(),
                })
            })
            .description(
                "E2E temporary straggler gate: holds the caller for 25 seconds before \
                 releasing, so a quorum must proceed without it.",
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
            "members": MEMBER_COUNT,
            "quorum": QUORUM_INDEXES.len(),
            "straggler": member_key(STRAGGLER_INDEX),
            "report_marker": REPORT_MARKER,
            "token_derivation": "run-scoped",
        }),
        complexity_profile(),
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::state".to_string(),
            "iii::triggers".to_string(),
            "e2e::subagents".to_string(),
        ],
        deliverable_contract(),
    )?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(namespace),
        case,
        capture: Some(capture),
    })
}

/// `coordination_edges` is deliberately 2: the quorum pair is what the
/// barrier coordinates, and the straggler is explicitly OUTSIDE the
/// coordinated set — it contributes a parallel branch but no fan-in edge.
/// That keeps the profile below the L4 threshold (`coordination_edges >= 3`
/// or `dependency_depth >= 3`), so `parallel_branches: 3` derives
/// L3Concurrent: three concurrent members under a two-edge fan-in contract.
fn complexity_profile() -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 2,
        dependency_depth: 2,
        parallel_branches: 3,
        external_systems: 1,
        state_transitions: 2,
        wake_cycles: 1,
        coordination_edges: 2,
        artifact_count: 1,
        ambiguity_level: 1,
        ..ComplexityProfile::default()
    }
}

fn scenario_for_case(run_id: &str) -> ScenarioSpec {
    let names = Names::new(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: prompt(&names, run_id),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 20,
            max_output_tokens: Some(8_192),
            max_total_tokens: Some(800_000),
            stuck_timeout_seconds: 360,
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

fn prompt(names: &Names, run_id: &str) -> String {
    format!(
        r#"Run a quorum fan-in with exactly 3 parallel members in the isolated state scope `{scope}`.

The control plane is parent-owned: a reaction may wake this coordinator, but it never starts an
agent. Every member must be spawned directly from a live coordinator turn. The rule of this run
is K-of-N: two members are a quorum, and the third must never hold you up.

Do all of the following in ONE response, in this order:

1. Arm exactly one one-shot state wake over the whole scope, with top-level label
   `{quorum_label}`. Gate it with the shipped named-set barrier using id `{barrier_id}`,
   expecting exactly these keys: `member-00`, `member-01`, and carry each event's new value.
   The barrier deliberately excludes `member-02`. The registration is wake-only: omit any
   function target. Register nothing else.
2. Directly spawn all 3 leaf members in parallel — every spawn in this same response — and note
   each spawn result's child session id; you will need the straggler's later. Give each member
   its assignment inline:

- `member-00` → token `{token_0}`: write exactly one state value at `{scope}` / `member-00`:
  `{{ "member": 0, "token": "{token_0}" }}`, then do nothing else.
- `member-01` → token `{token_1}`: write exactly one state value at `{scope}` / `member-01`:
  `{{ "member": 1, "token": "{token_1}" }}`, then do nothing else.
- `member-02` → token `{token_2}`: FIRST call the function `{straggle_function}` with `{{}}` and
  wait for its result; only after it returns, write `{scope}` / `member-02`:
  `{{ "member": 2, "token": "{token_2}" }}`. It must not write before that call returns.

Narrow each member to function discovery plus its listed calls. Members must not spawn,
register reactions, read state, or coordinate. End the coordinator turn immediately after the
spawns.

When the `{quorum_label}` barrier wake arrives, the quorum is met and the straggler is now
waste. First stop it: call `harness::stop` with the straggler's child session id from its
spawn result — `{{ "session_id": "<that id>" }}`. Then reply with a single line that starts
with `{marker}` and contains the tokens of `member-00` and `member-01` verbatim — and does NOT
contain the token of `member-02`. Do not answer before the barrier wake."#,
        scope = names.scope,
        quorum_label = names.quorum_label,
        barrier_id = names.barrier_id,
        straggle_function = straggle_function_id(run_id),
        token_0 = member_token(run_id, 0),
        token_1 = member_token(run_id, 1),
        token_2 = member_token(run_id, STRAGGLER_INDEX),
        marker = REPORT_MARKER,
    )
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move { evaluate_quorum(context, observation, run_id).await })
}

async fn evaluate_quorum(
    context: &E2eContext,
    observation: &ScenarioObservation,
    run_id: &str,
) -> anyhow::Result<ObjectiveEvaluation> {
    let names = Names::new(run_id);
    let calls = common::function_calls(&observation.transcript);

    let registrations: Vec<_> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.function_id == "engine::register_trigger")
        .collect();
    let quorum_watch = registrations
        .iter()
        .find(|(_, call)| is_quorum_watch(call, &names))
        .map(|(position, _)| *position);
    let spawns: Vec<_> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.function_id == "harness::spawn")
        .collect();
    let armed_before_spawns = quorum_watch.is_some_and(|watch| {
        registrations.len() == 1 && spawns.iter().all(|(position, _)| *position > watch)
    });

    let records = common::trigger_fired_records(&observation.transcript);
    let quorum_records: Vec<_> = records
        .iter()
        .filter(|record| {
            record.get("label").and_then(Value::as_str) == Some(names.quorum_label.as_str())
        })
        .collect();
    let retired = quorum_records
        .iter()
        .filter(|record| record.get("retired").and_then(Value::as_bool) == Some(true))
        .count();
    let pending = quorum_records
        .iter()
        .filter(|record| record.get("retired").and_then(Value::as_bool) == Some(false))
        .count();
    let barrier_woke = quorum_records.len() == 2 && retired == 1 && pending == 1;

    let children = depth_one_children(observation, &names);
    let audit = stop_audit(&observation.transcript, &names.quorum_label);
    let stopped_after_barrier = audit.stopped_child_after_barrier(&children);
    let straggler_value = get_state(context, &names.scope, &member_key(STRAGGLER_INDEX)).await?;
    let straggler_never_wrote = straggler_value.is_null();
    let three_children = children.len() == usize::from(MEMBER_COUNT);

    let reports = response_reports(&observation.response, run_id);

    let single_response_spawns =
        max_parallel_spawns(&observation.transcript) == usize::from(MEMBER_COUNT);
    let sessions_direct = observation.metrics.totals.sessions == u64::from(MEMBER_COUNT) + 1;
    let no_errors = observation.metrics.totals.function_call_errors == 0;

    Ok(assessment::build_evaluation([
        QUORUM_REPORT.full_or_zero(
            reports,
            format!(
                "report must start with `{REPORT_MARKER}`, carry both quorum tokens verbatim, \
                 and omit the straggler token"
            ),
        ),
        QUORUM_WAKE.full_or_zero(
            armed_before_spawns && barrier_woke,
            format!(
                "registrations={}, armed_before_spawns={armed_before_spawns}, \
                 quorum_records={}, barrier_woke={barrier_woke}",
                registrations.len(),
                quorum_records.len()
            ),
        ),
        STRAGGLER_STOPPED.full_or_zero(
            stopped_after_barrier && straggler_never_wrote && three_children,
            format!(
                "stop_after_barrier={stopped_after_barrier}, straggler_written={}, \
                 direct_children={}/{MEMBER_COUNT}, stop_calls={}",
                !straggler_never_wrote,
                children.len(),
                audit.stop_calls.len()
            ),
        ),
        FAN_OUT_DISCIPLINE.full_or_zero(
            single_response_spawns && sessions_direct && no_errors,
            format!(
                "single_response_spawns={single_response_spawns}, total_sessions={}, \
                 function_errors={}",
                observation.metrics.totals.sessions,
                observation.metrics.totals.function_call_errors
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
        let mut quorum_rows = Vec::new();
        let mut exact_rows = 0usize;
        for index in QUORUM_INDEXES {
            let key = member_key(index);
            let value = get_state(context, &names.scope, &key).await?;
            if value == expected_row(run_id, index) {
                exact_rows += 1;
            }
            quorum_rows.push(json!({ "key": key, "value": value }));
        }
        let straggler_value =
            get_state(context, &names.scope, &member_key(STRAGGLER_INDEX)).await?;
        let straggler_written = !straggler_value.is_null();
        let children = depth_one_children(observation, &names);
        let audit = stop_audit(&observation.transcript, &names.quorum_label);
        let stopped_after_barrier = audit.stopped_child_after_barrier(&children);
        let straggler_stopped = stopped_after_barrier
            && !straggler_written
            && children.len() == usize::from(MEMBER_COUNT);
        let rows_exact = exact_rows == QUORUM_INDEXES.len();

        // Provenance is attached only when the record actually proves the
        // quorum outcome — a failed run keeps its captured content but earns
        // no evidence chain.
        let provenance = if rows_exact && straggler_stopped {
            let mut evidence: Vec<ProvenanceEvidence> = QUORUM_INDEXES
                .into_iter()
                .map(|index| ProvenanceEvidence {
                    kind: "state_location".to_string(),
                    source_id: format!("{}/{}", names.scope, member_key(index)),
                    relation: "written_by_member".to_string(),
                })
                .collect();
            evidence.push(ProvenanceEvidence {
                kind: "session".to_string(),
                source_id: names.root_session.clone(),
                relation: "reported_after_quorum".to_string(),
            });
            evidence
        } else {
            Vec::new()
        };

        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "quorum_record".to_string(),
            content: json!({
                "quorum": quorum_rows,
                "straggler_written": straggler_written,
                "report": observation.response,
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: "quorum_rows_exact".to_string(),
                    passed: rows_exact,
                    reason: format!(
                        "observed {exact_rows}/{} exact quorum row(s)",
                        QUORUM_INDEXES.len()
                    ),
                },
                CapturedInvariant {
                    id: "straggler_stopped".to_string(),
                    passed: straggler_stopped,
                    reason: format!(
                        "stop_after_barrier={stopped_after_barrier}, \
                         straggler_written={straggler_written}, direct_children={}",
                        children.len()
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
            kind: "quorum_record".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["quorum", "straggler_written", "report"],
                "properties": {
                    "quorum": {
                        "type": "array",
                        "minItems": 2,
                        "maxItems": 2,
                        "items": {
                            "type": "object",
                            "required": ["key", "value"],
                            "properties": {
                                "key": { "type": "string" },
                                "value": {}
                            },
                            "additionalProperties": false
                        }
                    },
                    "straggler_written": { "type": "boolean" },
                    "report": { "type": "string" }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 8_192,
        }],
        invariants: vec![
            InvariantSpec {
                id: "quorum_rows_exact".to_string(),
                description: "Both quorum keys hold their exact assigned rows.".to_string(),
            },
            InvariantSpec {
                id: "straggler_stopped".to_string(),
                description:
                    "The straggler was stopped after the quorum barrier retired and never wrote \
                     its key."
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
        for index in 0..MEMBER_COUNT {
            let _: Value = context
                .trigger(
                    "state::delete",
                    json!({ "scope": names.scope, "key": member_key(index) }),
                )
                .await?;
        }
        // The barrier record is NOT ours to delete: `state_barrier` is the state
        // worker's private bookkeeping and every external write to it is refused
        // (`RESERVED_SCOPE`). Nothing leaks either — the id is per-run
        // (`quorum:<run_id>:members`) and this stack's store is in-memory.
        Ok(())
    })
}

async fn get_state(context: &E2eContext, scope: &str, key: &str) -> anyhow::Result<Value> {
    Ok(common::state_value(
        context
            .trigger_value("state::get", json!({ "scope": scope, "key": key }))
            .await?,
    ))
}

fn response_reports(response: &str, run_id: &str) -> bool {
    response.trim_start().starts_with(REPORT_MARKER)
        && QUORUM_INDEXES
            .into_iter()
            .all(|index| response.contains(&member_token(run_id, index)))
        && !response.contains(&member_token(run_id, STRAGGLER_INDEX))
}

fn depth_one_children(observation: &ScenarioObservation, names: &Names) -> BTreeSet<String> {
    observation
        .metrics
        .by_session
        .iter()
        .filter(|session| {
            session.depth == 1
                && session.parent_session_id.as_deref() == Some(names.root_session.as_str())
        })
        .map(|session| session.session_id.clone())
        .collect()
}

fn is_quorum_watch(call: &common::ObservedFunctionCall, names: &Names) -> bool {
    call.function_id == "engine::register_trigger"
        && call.arguments.get("trigger_type").and_then(Value::as_str) == Some("state")
        && call
            .arguments
            .pointer("/config/scope")
            .and_then(Value::as_str)
            == Some(names.scope.as_str())
        && call
            .arguments
            .pointer("/config/key")
            .and_then(Value::as_str)
            .is_none()
        && call.arguments.get("label").and_then(Value::as_str) == Some(names.quorum_label.as_str())
        && common::requested_once(&call.arguments)
        && common::is_wake_registration(&call.arguments)
        && has_named_barrier(&call.arguments, names)
}

fn has_named_barrier(arguments: &Value, names: &Names) -> bool {
    let expected: BTreeSet<String> = QUORUM_INDEXES.into_iter().map(member_key).collect();
    arguments
        .get("conditions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|condition| {
            condition.get("function_id").and_then(Value::as_str) == Some("state::barrier")
                && condition.pointer("/config/id").and_then(Value::as_str)
                    == Some(names.barrier_id.as_str())
                && condition.pointer("/config/carry").and_then(Value::as_str) == Some("/new_value")
                && condition
                    .pointer("/config/expect")
                    .and_then(Value::as_array)
                    .map(|keys| {
                        keys.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<BTreeSet<_>>()
                    })
                    == Some(expected.clone())
        })
}

#[derive(Debug)]
struct StopAudit {
    barrier_retired_at: Option<usize>,
    stop_calls: Vec<(usize, String)>,
}

impl StopAudit {
    fn stopped_child_after_barrier(&self, children: &BTreeSet<String>) -> bool {
        self.barrier_retired_at.is_some_and(|retired_at| {
            self.stop_calls
                .iter()
                .any(|(position, target)| *position > retired_at && children.contains(target))
        })
    }
}

/// Order the coordinator's `harness::stop` calls against the barrier's
/// retirement point using durable transcript entry positions: a stop only
/// proves quorum discipline when it happens AFTER the quorum wake retired.
fn stop_audit(transcript: &Value, label: &str) -> StopAudit {
    let mut barrier_retired_at = None;
    let mut stop_calls = Vec::new();
    for (position, entry) in transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let retired_here = entry.get("custom").is_some_and(|custom| {
            custom.get("custom_type").and_then(Value::as_str) == Some("trigger_fired")
                && custom.pointer("/data/label").and_then(Value::as_str) == Some(label)
                && custom.pointer("/data/retired").and_then(Value::as_bool) == Some(true)
        });
        if retired_here && barrier_retired_at.is_none() {
            barrier_retired_at = Some(position);
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
            if function_id != STOP_FUNCTION_ID {
                continue;
            }
            if let Some(session_id) = arguments.get("session_id").and_then(Value::as_str) {
                stop_calls.push((position, session_id.to_string()));
            }
        }
    }
    StopAudit {
        barrier_retired_at,
        stop_calls,
    }
}

fn max_parallel_spawns(transcript: &Value) -> usize {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("message"))
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .map(|message| {
            message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|block| {
                    normalized_block_call(block)
                        .is_some_and(|(function, _)| function == "harness::spawn")
                })
                .count()
        })
        .max()
        .unwrap_or(0)
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

struct Names {
    scope: String,
    root_session: String,
    quorum_label: String,
    barrier_id: String,
}

impl Names {
    fn new(run_id: &str) -> Self {
        Self {
            scope: format!("e2e:quorum:{run_id}"),
            root_session: format!("e2e_{run_id}"),
            quorum_label: format!("quorum-met:{run_id}"),
            barrier_id: format!("quorum:{run_id}:members"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_tokens_are_run_scoped_and_stable() {
        assert_eq!(member_token("attempt-a", 1), member_token("attempt-a", 1));
        assert_ne!(member_token("attempt-a", 0), member_token("attempt-a", 1));
        assert_ne!(member_token("attempt-a", 1), member_token("attempt-b", 1));
        let token = member_token("attempt-a", 2);
        let hex = token.strip_prefix("QRM-2-").unwrap();
        assert_eq!(hex.len(), 12);
        assert!(hex.chars().all(|character| character.is_ascii_hexdigit()));
    }

    #[test]
    fn report_validation_requires_the_marker_and_only_quorum_tokens() {
        let run_id = "report-run";
        let good = format!(
            "{REPORT_MARKER} {} {}",
            member_token(run_id, 0),
            member_token(run_id, 1)
        );
        assert!(response_reports(&good, run_id));
        assert!(response_reports(&format!("  {good}"), run_id));

        let with_straggler = format!("{good} {}", member_token(run_id, STRAGGLER_INDEX));
        assert!(!response_reports(&with_straggler, run_id));

        let missing_marker = format!("{} {}", member_token(run_id, 0), member_token(run_id, 1));
        assert!(!response_reports(&missing_marker, run_id));

        let missing_token = format!("{REPORT_MARKER} {}", member_token(run_id, 0));
        assert!(!response_reports(&missing_token, run_id));
    }

    fn barrier_registration(names: &Names, expect: &[&str]) -> Value {
        json!({
            "trigger_type": "state",
            "label": names.quorum_label,
            "once": true,
            "config": { "scope": names.scope },
            "conditions": [{
                "function_id": "state::barrier",
                "config": {
                    "id": names.barrier_id,
                    "expect": expect,
                    "carry": "/new_value",
                }
            }]
        })
    }

    #[test]
    fn barrier_matcher_expects_exactly_the_two_quorum_keys() {
        let names = Names::new("barrier-run");
        assert!(has_named_barrier(
            &barrier_registration(&names, &["member-00", "member-01"]),
            &names
        ));
        assert!(!has_named_barrier(
            &barrier_registration(&names, &["member-00", "member-01", "member-02"]),
            &names
        ));
        assert!(!has_named_barrier(
            &barrier_registration(&names, &["member-00"]),
            &names
        ));

        let watch = common::ObservedFunctionCall {
            function_id: "engine::register_trigger".to_string(),
            arguments: barrier_registration(&names, &["member-00", "member-01"]),
        };
        assert!(is_quorum_watch(&watch, &names));
    }

    fn fired(label: &str, retired: bool) -> Value {
        json!({
            "custom": {
                "custom_type": "trigger_fired",
                "data": { "label": label, "retired": retired }
            }
        })
    }

    fn assistant_call(function_id: &str, arguments: Value) -> Value {
        json!({
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "function_call",
                    "id": "call-1",
                    "function_id": function_id,
                    "arguments": arguments
                }]
            }
        })
    }

    #[test]
    fn stop_audit_only_counts_child_stops_after_the_barrier_retires() {
        let names = Names::new("stop-run");
        let children = BTreeSet::from(["child-02".to_string()]);
        let stop = assistant_call(STOP_FUNCTION_ID, json!({ "session_id": "child-02" }));
        let spawn = assistant_call("harness::spawn", json!({ "task": "member" }));

        let ordered = json!({ "messages": [
            spawn,
            fired(&names.quorum_label, false),
            fired(&names.quorum_label, true),
            stop.clone(),
        ]});
        let audit = stop_audit(&ordered, &names.quorum_label);
        assert_eq!(audit.barrier_retired_at, Some(2));
        assert!(audit.stopped_child_after_barrier(&children));
        assert!(!audit.stopped_child_after_barrier(&BTreeSet::from(["other".to_string()])));

        let premature = json!({ "messages": [
            stop,
            fired(&names.quorum_label, true),
        ]});
        let audit = stop_audit(&premature, &names.quorum_label);
        assert!(!audit.stopped_child_after_barrier(&children));

        let unretired = json!({ "messages": [
            fired(&names.quorum_label, false),
            assistant_call(STOP_FUNCTION_ID, json!({ "session_id": "child-02" })),
        ]});
        let audit = stop_audit(&unretired, &names.quorum_label);
        assert!(!audit.stopped_child_after_barrier(&children));

        let wrapped = json!({ "messages": [
            fired(&names.quorum_label, true),
            assistant_call(
                "agent_trigger",
                json!({ "function": STOP_FUNCTION_ID, "payload": { "session_id": "child-02" } }),
            ),
        ]});
        let audit = stop_audit(&wrapped, &names.quorum_label);
        assert!(audit.stopped_child_after_barrier(&children));
    }

    #[test]
    fn materialization_is_reproducible_and_derives_the_concurrent_tier() {
        use super::super::ComplexityTier;

        let first = materialize("attempt-a", 77).unwrap();
        let retry = materialize("attempt-b", 77).unwrap();
        first.validate().unwrap();
        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs, retry.case.inputs);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert_ne!(first.spec.prompt, retry.spec.prompt);

        // The quorum pair is the coordinated set and the straggler stays
        // outside it, so the profile derives L3Concurrent, not L4Coordinated.
        assert_eq!(first.case.complexity.tier, ComplexityTier::L3Concurrent);
        assert_eq!(
            usize::from(first.case.complexity.profile.artifact_count),
            first.case.deliverable_contract.artifacts.len()
        );
        assert!(first.case.deliverable_contract.capture_before_cleanup);
        assert!(first.case.deliverable_contract.provenance_required);
        assert!(first.capture.is_some());
    }
}
