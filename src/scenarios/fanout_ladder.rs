//! `fanout_ladder` — dose-response fan-out capacity, one rung per case.
//!
//! The same minimal coordination workload runs at fan-out 2, 4, 8, and 16;
//! the case seed selects the rung (`RUNGS`), so every rung is its own
//! longitudinally comparable case. The rung ladder turns pass/fail into a
//! capability frontier: a Harness version is characterized by the largest
//! rung it sustains, and version-over-version comparison reads as frontier
//! movement instead of a single bit.
//!
//! Per rung the coordinator must, in ONE live turn, arm a single named-set
//! barrier wake over the run scope and then spawn all N leaf workers in the
//! same response. Each worker writes exactly one keyed row; the barrier
//! retires after the N-th row and wakes the coordinator, which reports the
//! rung marker plus every worker token verbatim. Correct rows prove the
//! deliverable; per-child transcripts prove the rows were produced by N
//! distinct direct children rather than by the coordinator itself.

use std::collections::BTreeSet;

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

pub const ID: &str = "fanout_ladder";
const VERSION: u32 = 1;
const ROWS_DELIVERABLE_ID: &str = "worker_rows";
const REPORT_DELIVERABLE_ID: &str = "fanout_report";

/// One ladder rung. The seed is the case selector: running the ladder means
/// running the scenario once per rung seed, and each rung keeps its own
/// longitudinal series.
#[derive(Debug, Clone, Copy)]
struct Rung {
    seed: u64,
    fan_out: u8,
}

pub const CANONICAL_SEED: u64 = 2001;
const RUNGS: [Rung; 4] = [
    Rung {
        seed: 2001,
        fan_out: 2,
    },
    Rung {
        seed: 2002,
        fan_out: 4,
    },
    Rung {
        seed: 2003,
        fan_out: 8,
    },
    Rung {
        seed: 2004,
        fan_out: 16,
    },
];

const PARALLEL_FAN_OUT: AssessmentSpec = AssessmentSpec::hard_gated(
    "parallel_fan_out",
    30,
    "All N workers are spawned directly, in one coordinator response, as N distinct leaf sessions.",
);
const WORKER_DELIVERABLES: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "worker_deliverables",
    30,
    "Every worker row is exact and was written by its own direct leaf session with a single state write.",
    EvaluationDimension::Deliverable,
);
const BARRIER_FAN_IN: AssessmentSpec = AssessmentSpec::hard_gated(
    "barrier_fan_in",
    25,
    "One named-set barrier wake is armed before any spawn, expects exactly the N worker keys, and retires on the N-th row.",
);
const AGGREGATED_REPORT: AssessmentSpec = AssessmentSpec::hard_gated(
    "aggregated_report",
    15,
    "The barrier-woken report carries the rung marker and every worker token verbatim, with no binding left armed.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    PARALLEL_FAN_OUT,
    WORKER_DELIVERABLES,
    BARRIER_FAN_IN,
    AGGREGATED_REPORT,
];

fn rung(seed: u64) -> Rung {
    RUNGS
        .iter()
        .copied()
        .find(|rung| rung.seed == seed)
        .unwrap_or_else(|| RUNGS[(seed as usize) % RUNGS.len()])
}

fn worker_key(index: u8) -> String {
    format!("worker-{index:02}")
}

fn worker_keys(fan_out: u8) -> Vec<String> {
    (0..fan_out).map(worker_key).collect()
}

/// Run-scoped worker tokens: opaque to the subject before the run, but
/// handed to the coordinator inline, so the provenance gates — not token
/// secrecy — are what force real delegation.
fn worker_token(run_id: &str, index: u8) -> String {
    format!(
        "FAN-{:016x}",
        super::stable_seed(&format!("{ID}:{run_id}:worker-{index:02}"))
    )
}

fn report_marker(fan_out: u8) -> String {
    format!("FANOUT-{fan_out}-COMPLETE")
}

fn expected_row(run_id: &str, index: u8) -> Value {
    json!({
        "worker": index,
        "token": worker_token(run_id, index),
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
            "fan_out": rung.fan_out,
            "worker_keys": worker_keys(rung.fan_out),
            "report_marker": report_marker(rung.fan_out),
            "token_derivation": "run-scoped",
            "ladder_rungs": RUNGS.iter().map(|rung| rung.fan_out).collect::<Vec<_>>(),
        }),
        complexity_profile(rung.fan_out),
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::state".to_string(),
            "iii::triggers".to_string(),
            "e2e::subagents".to_string(),
        ],
        deliverable_contract(rung.fan_out),
    )?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(namespace, rung),
        case,
        capture: Some(capture),
    })
}

/// The rung is the profile: fan-out 2 derives L3Concurrent and every larger
/// rung derives L4Coordinated, so the ladder spans the concurrency tiers on
/// purpose.
fn complexity_profile(fan_out: u8) -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 2,
        dependency_depth: 2,
        parallel_branches: fan_out,
        state_transitions: u16::from(fan_out),
        wake_cycles: 1,
        artifact_count: 2,
        coordination_edges: u16::from(fan_out),
        ambiguity_level: 1,
        ..ComplexityProfile::default()
    }
}

fn scenario_for_case(run_id: &str, rung: Rung) -> ScenarioSpec {
    let names = Names::new(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: prompt(&names, run_id, rung.fan_out),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 16 + 2 * u32::from(rung.fan_out),
            max_output_tokens: Some(16_384),
            max_total_tokens: Some(400_000 + 100_000 * u64::from(rung.fan_out)),
            stuck_timeout_seconds: 360,
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

fn prompt(names: &Names, run_id: &str, fan_out: u8) -> String {
    let assignments = (0..fan_out)
        .map(|index| {
            format!(
                "- `{}` → token `{}`",
                worker_key(index),
                worker_token(run_id, index)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let keys = worker_keys(fan_out)
        .iter()
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"Run a fan-out rung with exactly {fan_out} parallel workers in the isolated state scope `{scope}`.

The control plane is parent-owned: a reaction may wake this coordinator, but it never starts an
agent. Every worker must be spawned directly from a live coordinator turn.

Do all of the following in ONE response, in this order:

1. Arm exactly one one-shot state wake over the whole scope, with top-level label
   `{complete_label}`. Gate it with the shipped named-set barrier using id `{barrier_id}`,
   expecting exactly these keys: {keys}, and carry each event's new value. The registration is
   wake-only: omit any function target. Register nothing else.
2. Directly spawn all {fan_out} leaf workers in parallel — every spawn in this same response.
   Give each worker its assignment inline:

{assignments}

Each worker must write exactly one state value at `{scope}` / its assigned key:

`{{ "worker": <index>, "token": "<its token>" }}`

where `<index>` is the number in its key (for example `worker-03` writes `"worker": 3`). Narrow
each worker to function discovery plus the single state-write capability. Workers must not
spawn, register reactions, read state, or coordinate. End the coordinator turn immediately
after the spawns.

When the `{complete_label}` barrier wake arrives, report in this same session; do not spawn a
reporter. Return a single short report that starts with `{marker}` and contains every worker
token verbatim, one per line. Do not answer before the barrier wake, and leave no binding
armed."#,
        scope = names.scope,
        complete_label = names.complete_label,
        barrier_id = names.barrier_id,
        marker = report_marker(fan_out),
    )
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move { evaluate_rung(context, observation, run_id).await })
}

async fn evaluate_rung(
    context: &E2eContext,
    observation: &ScenarioObservation,
    run_id: &str,
) -> anyhow::Result<ObjectiveEvaluation> {
    let names = Names::new(run_id);
    let fan_out = case_fan_out(&observation.case)?;
    let calls = common::function_calls(&observation.transcript);

    let registrations: Vec<_> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.function_id == "engine::register_trigger")
        .collect();
    let barrier_watch = registrations
        .iter()
        .find(|(_, call)| is_completion_watch(call, &names, fan_out))
        .map(|(position, _)| *position);
    let spawns: Vec<_> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.function_id == "harness::spawn")
        .collect();
    let armed_before_spawns = barrier_watch.is_some_and(|watch| {
        registrations.len() == 1 && spawns.iter().all(|(position, _)| *position > watch)
    });

    let worker_sessions: BTreeSet<_> = observation
        .metrics
        .by_session
        .iter()
        .filter(|session| {
            session.depth == 1
                && session.parent_session_id.as_deref() == Some(names.root_session.as_str())
        })
        .map(|session| session.session_id.clone())
        .collect();
    let single_response_spawns = max_parallel_spawns(&observation.transcript) == usize::from(fan_out);
    let sessions_direct = observation.metrics.totals.sessions == u64::from(fan_out) + 1
        && worker_sessions.len() == usize::from(fan_out);
    let fanned_out =
        spawns.len() == usize::from(fan_out) && single_response_spawns && sessions_direct;

    let audit = worker_audit(context, &names, run_id, fan_out, &worker_sessions).await?;

    let records = common::trigger_fired_records(&observation.transcript);
    let completion_records: Vec<_> = records
        .iter()
        .filter(|record| {
            record.get("label").and_then(Value::as_str) == Some(names.complete_label.as_str())
        })
        .collect();
    let barrier_woke = completion_records.len() == usize::from(fan_out)
        && completion_records
            .iter()
            .filter(|record| record.get("retired").and_then(Value::as_bool) == Some(false))
            .count()
            == usize::from(fan_out) - 1
        && completion_records
            .iter()
            .filter(|record| record.get("retired").and_then(Value::as_bool) == Some(true))
            .count()
            == 1;

    let active_bindings = common::active_binding_count(context, &names.root_session).await?;
    let report_aggregates = report_aggregates(&observation.response, run_id, fan_out);
    let no_errors = observation.metrics.totals.function_call_errors == 0;

    Ok(assessment::build_evaluation([
        PARALLEL_FAN_OUT.full_or_zero(
            fanned_out,
            format!(
                "spawns={}, single_response_spawns={single_response_spawns}, \
                 direct_sessions={}/{fan_out}, total_sessions={}",
                spawns.len(),
                worker_sessions.len(),
                observation.metrics.totals.sessions
            ),
        ),
        WORKER_DELIVERABLES.full_or_zero(
            audit.rows_exact && audit.direct_provenance,
            format!(
                "exact_rows={}/{fan_out}, distinct_writers={}, leaf_discipline={}",
                audit.exact_rows, audit.distinct_writers, audit.leaf_discipline
            ),
        ),
        BARRIER_FAN_IN.full_or_zero(
            armed_before_spawns && barrier_woke,
            format!(
                "registrations={}, armed_before_spawns={armed_before_spawns}, \
                 completion_records={}, barrier_woke={barrier_woke}",
                registrations.len(),
                completion_records.len()
            ),
        ),
        AGGREGATED_REPORT.full_or_zero(
            report_aggregates && active_bindings == 0 && no_errors,
            format!(
                "report_aggregates={report_aggregates}, active_bindings={active_bindings}, \
                 function_errors={}",
                observation.metrics.totals.function_call_errors
            ),
        ),
    ]))
}

struct WorkerAudit {
    exact_rows: usize,
    rows_exact: bool,
    distinct_writers: usize,
    leaf_discipline: bool,
    direct_provenance: bool,
}

/// Verify every worker row and its provenance: each of the N keys holds the
/// exact row, and each row was written by its own direct leaf session with a
/// single exact state write and nothing beyond discovery.
async fn worker_audit(
    context: &E2eContext,
    names: &Names,
    run_id: &str,
    fan_out: u8,
    worker_sessions: &BTreeSet<String>,
) -> anyhow::Result<WorkerAudit> {
    let mut exact_rows = 0usize;
    for index in 0..fan_out {
        let observed = get_state(context, &names.scope, &worker_key(index)).await?;
        if observed == expected_row(run_id, index) {
            exact_rows += 1;
        }
    }
    let rows_exact = exact_rows == usize::from(fan_out);

    let mut written_keys = BTreeSet::new();
    let mut leaf_discipline = true;
    let mut single_writes = true;
    for session_id in worker_sessions {
        let transcript = context.transcript(session_id).await?;
        let session_calls = common::function_calls(&transcript);
        let writes: Vec<_> = session_calls
            .iter()
            .filter(|call| call.function_id == "state::set")
            .collect();
        leaf_discipline &= session_calls.iter().all(|call| {
            call.function_id == "state::set" || call.function_id.starts_with("engine::functions::")
        });
        if writes.len() != 1 {
            single_writes = false;
            continue;
        }
        let arguments = &writes[0].arguments;
        let Some(key) = arguments.get("key").and_then(Value::as_str) else {
            single_writes = false;
            continue;
        };
        let Some(index) = parse_worker_index(key, fan_out) else {
            single_writes = false;
            continue;
        };
        single_writes &= arguments
            == &json!({
                "scope": names.scope,
                "key": key,
                "value": expected_row(run_id, index),
            });
        written_keys.insert(key.to_string());
    }
    let expected_keys: BTreeSet<String> = worker_keys(fan_out).into_iter().collect();
    let distinct_writers = written_keys.len();
    let direct_provenance = worker_sessions.len() == usize::from(fan_out)
        && leaf_discipline
        && single_writes
        && written_keys == expected_keys;

    Ok(WorkerAudit {
        exact_rows,
        rows_exact,
        distinct_writers,
        leaf_discipline,
        direct_provenance,
    })
}

fn parse_worker_index(key: &str, fan_out: u8) -> Option<u8> {
    let index = key.strip_prefix("worker-")?.parse::<u8>().ok()?;
    (index < fan_out && key == worker_key(index)).then_some(index)
}

fn report_aggregates(response: &str, run_id: &str, fan_out: u8) -> bool {
    response
        .trim_start()
        .starts_with(&report_marker(fan_out))
        && (0..fan_out).all(|index| response.contains(&worker_token(run_id, index)))
}

fn case_fan_out(case: &ScenarioCase) -> anyhow::Result<u8> {
    case.inputs
        .get("fan_out")
        .and_then(Value::as_u64)
        .and_then(|fan_out| u8::try_from(fan_out).ok())
        .filter(|fan_out| *fan_out > 0)
        .ok_or_else(|| anyhow::anyhow!("fanout_ladder case is missing a positive fan_out input"))
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let names = Names::new(run_id);
        let fan_out = case_fan_out(&observation.case)?;
        let worker_sessions: BTreeSet<_> = observation
            .metrics
            .by_session
            .iter()
            .filter(|session| {
                session.depth == 1
                    && session.parent_session_id.as_deref() == Some(names.root_session.as_str())
            })
            .map(|session| session.session_id.clone())
            .collect();
        let audit = worker_audit(context, &names, run_id, fan_out, &worker_sessions).await?;
        let mut rows = Vec::new();
        for index in 0..fan_out {
            let key = worker_key(index);
            let value = get_state(context, &names.scope, &key).await?;
            rows.push(json!({ "key": key, "value": value }));
        }
        let active_bindings = common::active_binding_count(context, &names.root_session).await?;
        let aggregated = report_aggregates(&observation.response, run_id, fan_out);

        Ok(vec![
            CapturedDeliverable {
                id: ROWS_DELIVERABLE_ID.to_string(),
                kind: "state_bundle".to_string(),
                content: json!({ "fan_out": fan_out, "workers": rows }).into(),
                invariants: vec![
                    CapturedInvariant {
                        id: "worker_rows_exact".to_string(),
                        passed: audit.rows_exact,
                        reason: format!("observed {}/{fan_out} exact worker row(s)", audit.exact_rows),
                    },
                    CapturedInvariant {
                        id: "direct_worker_provenance".to_string(),
                        passed: audit.direct_provenance,
                        reason: format!(
                            "observed {} direct worker session(s) writing {} distinct key(s); \
                             leaf_discipline={}",
                            worker_sessions.len(),
                            audit.distinct_writers,
                            audit.leaf_discipline
                        ),
                    },
                ],
                provenance: worker_keys(fan_out)
                    .into_iter()
                    .map(|key| ProvenanceEvidence {
                        kind: "state_location".to_string(),
                        source_id: format!("{}/{}", names.scope, key),
                        relation: "written_by_worker".to_string(),
                    })
                    .collect(),
            },
            CapturedDeliverable {
                id: REPORT_DELIVERABLE_ID.to_string(),
                kind: "fanout_report".to_string(),
                content: json!({ "content": observation.response }).into(),
                invariants: vec![
                    CapturedInvariant {
                        id: "report_aggregates".to_string(),
                        passed: aggregated,
                        reason: format!(
                            "report must start with `{}` and carry all {fan_out} worker token(s)",
                            report_marker(fan_out)
                        ),
                    },
                    CapturedInvariant {
                        id: "bindings_retired".to_string(),
                        passed: active_bindings == 0,
                        reason: format!("observed {active_bindings} active binding(s)"),
                    },
                ],
                provenance: vec![ProvenanceEvidence {
                    kind: "session".to_string(),
                    source_id: names.root_session,
                    relation: "reported_after_barrier".to_string(),
                }],
            },
        ])
    })
}

fn deliverable_contract(fan_out: u8) -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![
            ArtifactExpectation {
                id: ROWS_DELIVERABLE_ID.to_string(),
                kind: "state_bundle".to_string(),
                media_type: "application/json".to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["fan_out", "workers"],
                    "properties": {
                        "fan_out": { "const": fan_out },
                        "workers": {
                            "type": "array",
                            "minItems": fan_out,
                            "maxItems": fan_out,
                            "items": {
                                "type": "object",
                                "required": ["key", "value"],
                                "properties": {
                                    "key": { "type": "string" },
                                    "value": {}
                                },
                                "additionalProperties": false
                            }
                        }
                    },
                    "additionalProperties": false
                }),
                max_size_bytes: 16_384,
            },
            ArtifactExpectation {
                id: REPORT_DELIVERABLE_ID.to_string(),
                kind: "fanout_report".to_string(),
                media_type: "application/json".to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["content"],
                    "properties": { "content": { "type": "string" } },
                    "additionalProperties": false
                }),
                max_size_bytes: 16_384,
            },
        ],
        invariants: vec![
            InvariantSpec {
                id: "worker_rows_exact".to_string(),
                description: "Every worker key holds the exact assigned row.".to_string(),
            },
            InvariantSpec {
                id: "direct_worker_provenance".to_string(),
                description:
                    "Each row was written by its own direct leaf session with a single exact write."
                        .to_string(),
            },
            InvariantSpec {
                id: "report_aggregates".to_string(),
                description:
                    "The barrier-woken report carries the rung marker and every worker token verbatim."
                        .to_string(),
            },
            InvariantSpec {
                id: "bindings_retired".to_string(),
                description: "No fan-out binding remains active after the report.".to_string(),
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
        // Delete every key the ladder can address (the largest rung), not just
        // the current rung's, so a misbehaving run cannot leak rows between
        // attempts sharing a scope prefix.
        let widest = RUNGS
            .iter()
            .map(|rung| rung.fan_out)
            .max()
            .unwrap_or_default();
        for key in worker_keys(widest) {
            let _: Value = context
                .trigger("state::delete", json!({ "scope": names.scope, "key": key }))
                .await?;
        }
        // The barrier record is the state worker's private bookkeeping
        // (`RESERVED_SCOPE`); it is per-run and not ours to delete.
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

fn is_completion_watch(call: &common::ObservedFunctionCall, names: &Names, fan_out: u8) -> bool {
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
        && call.arguments.get("label").and_then(Value::as_str)
            == Some(names.complete_label.as_str())
        && common::requested_once(&call.arguments)
        && common::is_wake_registration(&call.arguments)
        && has_named_barrier(&call.arguments, names, fan_out)
}

fn has_named_barrier(arguments: &Value, names: &Names, fan_out: u8) -> bool {
    let expected: BTreeSet<String> = worker_keys(fan_out).into_iter().collect();
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
    complete_label: String,
    barrier_id: String,
}

impl Names {
    fn new(run_id: &str) -> Self {
        Self {
            scope: format!("e2e:fanout:{run_id}"),
            root_session: format!("e2e_{run_id}"),
            complete_label: format!("fanout-complete:{run_id}"),
            barrier_id: format!("fanout:{run_id}:workers"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_seeds_select_their_rungs_and_fall_back_by_modulo() {
        assert_eq!(rung(2001).fan_out, 2);
        assert_eq!(rung(2002).fan_out, 4);
        assert_eq!(rung(2003).fan_out, 8);
        assert_eq!(rung(2004).fan_out, 16);
        assert_eq!(rung(313).fan_out, RUNGS[313 % RUNGS.len()].fan_out);
        assert_eq!(rung(CANONICAL_SEED).fan_out, RUNGS[0].fan_out);
    }

    #[test]
    fn worker_tokens_are_run_scoped_and_stable() {
        assert_eq!(worker_token("attempt-a", 3), worker_token("attempt-a", 3));
        assert_ne!(worker_token("attempt-a", 3), worker_token("attempt-a", 4));
        assert_ne!(worker_token("attempt-a", 3), worker_token("attempt-b", 3));
        assert!(worker_token("attempt-a", 0).starts_with("FAN-"));
    }

    #[test]
    fn the_smallest_rung_is_concurrent_and_larger_rungs_are_coordinated() {
        use super::super::ComplexityTier;

        let smallest = materialize("attempt-a", 2001).unwrap();
        assert_eq!(
            smallest.case.complexity.tier,
            ComplexityTier::L3Concurrent
        );
        for seed in [2002, 2003, 2004] {
            let rung_case = materialize("attempt-a", seed).unwrap();
            assert_eq!(
                rung_case.case.complexity.tier,
                ComplexityTier::L4Coordinated,
                "seed {seed}"
            );
        }
    }

    #[test]
    fn every_rung_publishes_a_reproducible_case_with_its_fan_out() {
        for rung_spec in RUNGS {
            let first = materialize("attempt-a", rung_spec.seed).unwrap();
            let retry = materialize("attempt-b", rung_spec.seed).unwrap();
            assert_eq!(first.case.case_id, retry.case.case_id);
            assert_eq!(first.case.inputs, retry.case.inputs);
            assert_eq!(
                first.case.inputs.get("fan_out").and_then(Value::as_u64),
                Some(u64::from(rung_spec.fan_out))
            );
            assert_eq!(
                first
                    .case
                    .inputs
                    .get("worker_keys")
                    .and_then(Value::as_array)
                    .map(Vec::len),
                Some(usize::from(rung_spec.fan_out))
            );
            assert_eq!(first.case.deliverable_contract.artifacts.len(), 2);
            assert_ne!(first.spec.prompt, retry.spec.prompt);
        }
    }

    #[test]
    fn report_validation_requires_the_marker_and_every_token() {
        let run_id = "report-run";
        let tokens = (0..4)
            .map(|index| worker_token(run_id, index))
            .collect::<Vec<_>>()
            .join("\n");
        let complete = format!("{}\n{tokens}", report_marker(4));
        assert!(report_aggregates(&complete, run_id, 4));

        let missing_marker = tokens.clone();
        assert!(!report_aggregates(&missing_marker, run_id, 4));

        let missing_token = format!(
            "{}\n{}",
            report_marker(4),
            (0..3)
                .map(|index| worker_token(run_id, index))
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert!(!report_aggregates(&missing_token, run_id, 4));
    }

    #[test]
    fn worker_keys_parse_back_to_their_indexes_only_within_the_rung() {
        assert_eq!(parse_worker_index("worker-03", 4), Some(3));
        assert_eq!(parse_worker_index("worker-04", 4), None);
        assert_eq!(parse_worker_index("worker-3", 4), None);
        assert_eq!(parse_worker_index("other-01", 4), None);
    }
}
