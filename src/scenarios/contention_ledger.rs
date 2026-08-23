//! `contention_ledger` — atomic read-modify-write under real write contention.
//!
//! Three parallel writer children hammer one shared accumulator row in
//! database `primary`: each performs exactly five atomic
//! `UPDATE ... SET total = total + 1` increments, inserts one audit row per
//! increment, and only then writes its done key. The coordinator creates the
//! tables, arms a single barrier-gated state wake BEFORE spawning, sleeps
//! through the contention, and verifies the final truth after one wake:
//! accumulator 15 and exactly fifteen per-writer audit rows. A lost update
//! (read-then-write race) surfaces directly as an accumulator below 15 while
//! the audit still counts fifteen rows.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Context;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::{
    common, validation_loop, ArtifactExpectation, CapturedDeliverable, CapturedInvariant,
    CleanupFuture, ComplexityProfile, DeliverableCaptureFuture, DeliverableContract,
    EvaluationFuture, ExecutionPolicy, InvariantSpec, MaterializedScenario, ObjectiveEvaluation,
    ProvenanceEvidence, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "contention_ledger";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "ledger_totals";

const DATABASE: &str = "primary";
const WRITERS: u8 = 3;
const INCREMENTS_PER_WRITER: u8 = 5;
const EXPECTED_TOTAL: i64 = 15;
const REPORT_MARKER: &str = "LEDGER-BALANCED 15/15";
const MAX_REPORT_CHARS: usize = 300;
const DATABASE_WRITES: [&str; 3] = [
    "database::execute",
    "database::executeBatch",
    "database::transaction",
];

const BALANCED_LEDGER: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "balanced_ledger",
    35,
    "The accumulator reads exactly 15 and the audit holds exactly the fifteen per-writer sequence rows.",
    EvaluationDimension::Deliverable,
);
const CONTENDED_WRITES: AssessmentSpec = AssessmentSpec::hard_gated(
    "contended_writes",
    25,
    "Three direct writer children each perform their own five atomic increments and done-key write; the root increments nothing.",
);
const QUORUM_FREE_FAN_IN: AssessmentSpec = AssessmentSpec::hard_gated(
    "quorum_free_fan_in",
    20,
    "One barrier-gated wake over the three done keys is armed before the writers are spawned in a single response, and it retires on the third key.",
);
const VERIFIED_REPORT: AssessmentSpec = AssessmentSpec::score_only(
    "verified_report",
    20,
    "The final answer is one short LEDGER-BALANCED 15/15 line backed by an error-free run.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    BALANCED_LEDGER,
    CONTENDED_WRITES,
    QUORUM_FREE_FAN_IN,
    VERIFIED_REPORT,
];

fn done_key(index: u8) -> String {
    format!("done-{index:02}")
}

fn done_keys() -> Vec<String> {
    (0..WRITERS).map(done_key).collect()
}

fn parse_writer_index(key: &str) -> Option<u8> {
    let index = key.strip_prefix("done-")?.parse::<u8>().ok()?;
    (index < WRITERS && key == done_key(index)).then_some(index)
}

fn expected_sequences() -> BTreeMap<i64, BTreeSet<i64>> {
    (0..i64::from(WRITERS))
        .map(|writer| (writer, (1..=i64::from(INCREMENTS_PER_WRITER)).collect()))
        .collect()
}

fn sequences_balanced(pairs: &[(i64, i64)]) -> bool {
    if pairs.len() != usize::from(WRITERS) * usize::from(INCREMENTS_PER_WRITER) {
        return false;
    }
    let mut observed: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    for (writer, seq) in pairs {
        observed.entry(*writer).or_default().insert(*seq);
    }
    observed == expected_sequences()
}

fn report_is_verified(response: &str) -> bool {
    response.contains(REPORT_MARKER) && response.chars().count() <= MAX_REPORT_CHARS
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
            "writers": WRITERS,
            "increments_per_writer": INCREMENTS_PER_WRITER,
            "expected_total": EXPECTED_TOTAL,
            "report_marker": REPORT_MARKER,
            "token_derivation": "run-scoped",
        }),
        complexity_profile(),
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::database".to_string(),
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

/// The profile pins the case at L3Concurrent on purpose: three genuine
/// parallel writers racing one row is the point, so `coordination_edges`
/// stays at 2 (one barrier arm, one fan-in wake) and `dependency_depth` at 2.
/// The scenario measures write CONTENTION, not coordination fabric; raising
/// either value would misfile the case as L4Coordinated.
fn complexity_profile() -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 2,
        dependency_depth: 2,
        parallel_branches: 3,
        external_systems: 1,
        state_transitions: 3,
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
        prompt: prompt(&names),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 24,
            max_output_tokens: Some(8_192),
            max_total_tokens: Some(1_000_000),
            stuck_timeout_seconds: 420,
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

fn prompt(names: &Names) -> String {
    format!(
        r#"You are balancing one shared ledger under real write contention on this stack.

Three writer children each apply exactly {increments} increments to one shared accumulator
in database `primary`. An increment is one atomic read-modify-write — a single
`UPDATE {ledger} SET total = total + 1` statement, never a read followed by a separate
write — plus exactly one audit row
`INSERT INTO {audit} (writer, seq) VALUES (<writer>, <seq>)` written with literal values
(no bound parameters).

Use this run-isolated database contract so the result can be evaluated:

- accumulator: `{ledger}` — single column `total` (INTEGER); exactly one row, seeded with
  `total = 0`
- audit: `{audit}` — columns `writer` (INTEGER, 0-2) and `seq` (INTEGER, 1-{increments});
  one row per increment

Do all of the following in ONE response, in this order:

1. Create both tables and insert the single accumulator row with `total = 0`.
2. Arm exactly one one-shot wake-only state wake over the whole scope `{scope}` with
   top-level label `{complete_label}`. Gate it with the shipped named-set barrier using id
   `{barrier_id}`, expecting exactly these keys: `done-00`, `done-01`, `done-02`, and carry
   each event's new value. The registration is wake-only: omit any function target.
   Register nothing else.
3. Directly spawn all three writers in parallel — every spawn in this same response — as
   sessions `{w0}` (writer 0), `{w1}` (writer 1), and `{w2}` (writer 2). Then end the turn.

Each writer must, in its own session, perform its {increments} increments for `seq` 1
through {increments} — each one atomic accumulator UPDATE plus one audit INSERT carrying
its own writer index — and only AFTER all of them write its own done key: set the state
value `"done"` at scope `{scope}`, key `done-0<its writer index>` (writer 0 writes
`done-00`). Writers must not spawn, register reactions, read other writers' rows, or touch
any done key except their own. You, the coordinator, must never run an increment or insert
an audit row yourself.

When the `{complete_label}` barrier wake arrives, run one verification query reading the
accumulator total and counting the audit rows. Then answer with a single line: exactly
`{marker}` only if your own query shows total {total} and {total} audit rows; otherwise one
honest line naming the discrepancy. Keep the final answer at or under 300 characters and
leave both tables in place for evaluation."#,
        increments = INCREMENTS_PER_WRITER,
        ledger = names.ledger,
        audit = names.audit,
        scope = names.scope,
        complete_label = names.complete_label,
        barrier_id = names.barrier_id,
        w0 = names.writer_sessions[0],
        w1 = names.writer_sessions[1],
        w2 = names.writer_sessions[2],
        marker = REPORT_MARKER,
        total = EXPECTED_TOTAL,
    )
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        if !context.function_exists("database::query").await? {
            return Ok(missing_database());
        }
        let databases = available_databases(context).await?;
        if !databases.contains(DATABASE) {
            return Ok(missing_primary(&databases));
        }

        let names = Names::new(run_id);
        let snapshot = ledger_snapshot(context, &names).await?;
        let sequences_exact = sequences_balanced(&snapshot.audit_pairs);
        let balanced = snapshot.balanced();

        let calls = common::function_calls(&observation.transcript);
        let audit = writer_audit(context, observation, &names).await?;
        let root_clean = root_avoids_increments(&calls, &names);
        let contended = audit.all_children_contended && audit.no_extra_sessions && root_clean;

        let registrations: Vec<_> = calls
            .iter()
            .enumerate()
            .filter(|(_, call)| call.function_id == "engine::register_trigger")
            .collect();
        let barrier_watch = registrations
            .iter()
            .find(|(_, call)| is_completion_watch(call, &names))
            .map(|(position, _)| *position);
        let spawns: Vec<_> = calls
            .iter()
            .enumerate()
            .filter(|(_, call)| call.function_id == "harness::spawn")
            .collect();
        let armed_before_spawns = barrier_watch.is_some_and(|watch| {
            registrations.len() == 1 && spawns.iter().all(|(position, _)| *position > watch)
        });
        let spawn_sessions: BTreeSet<&str> = spawns
            .iter()
            .filter_map(|(_, call)| call.arguments.get("session_id").and_then(Value::as_str))
            .collect();
        let expected_sessions: BTreeSet<&str> =
            names.writer_sessions.iter().map(String::as_str).collect();
        let single_response_spawns =
            max_parallel_spawns(&observation.transcript) == usize::from(WRITERS);

        let records = common::trigger_fired_records(&observation.transcript);
        let completion_records: Vec<_> = records
            .iter()
            .filter(|record| {
                record.get("label").and_then(Value::as_str) == Some(names.complete_label.as_str())
            })
            .collect();
        let retired = completion_records
            .iter()
            .filter(|record| record.get("retired").and_then(Value::as_bool) == Some(true))
            .count();
        let pending = completion_records
            .iter()
            .filter(|record| record.get("retired").and_then(Value::as_bool) == Some(false))
            .count();
        let barrier_woke = completion_records.len() == usize::from(WRITERS)
            && retired == 1
            && pending == usize::from(WRITERS) - 1;
        let fan_in = armed_before_spawns
            && spawns.len() == usize::from(WRITERS)
            && spawn_sessions == expected_sessions
            && single_response_spawns
            && barrier_woke;

        let report_verified = report_is_verified(&observation.response);
        let no_errors = observation.metrics.totals.function_call_errors == 0;

        Ok(assessment::build_evaluation([
            BALANCED_LEDGER.full_or_zero(
                balanced,
                format!(
                    "accumulator_rows={}, accumulator={}, audit_rows={}, \
                     sequences_exact={sequences_exact}",
                    snapshot.accumulator_rows,
                    snapshot.accumulator,
                    snapshot.audit_rows(),
                ),
            ),
            CONTENDED_WRITES.full_or_zero(
                contended,
                format!(
                    "children_in_tree={}/{WRITERS}, contended_children={}, \
                     no_extra_sessions={}, root_clean={root_clean}",
                    audit.children_in_tree, audit.contended_children, audit.no_extra_sessions,
                ),
            ),
            QUORUM_FREE_FAN_IN.full_or_zero(
                fan_in,
                format!(
                    "registrations={}, armed_before_spawns={armed_before_spawns}, spawns={}, \
                     single_response_spawns={single_response_spawns}, completion_records={}, \
                     barrier_woke={barrier_woke}",
                    registrations.len(),
                    spawns.len(),
                    completion_records.len(),
                ),
            ),
            VERIFIED_REPORT.full_or_zero(
                report_verified && no_errors,
                format!(
                    "report_verified={report_verified}, response_chars={}, function_errors={}",
                    observation.response.chars().count(),
                    observation.metrics.totals.function_call_errors,
                ),
            ),
        ]))
    })
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let names = Names::new(run_id);
        let database_available = context.function_exists("database::query").await?
            && available_databases(context).await?.contains(DATABASE);
        let snapshot = if database_available {
            ledger_snapshot(context, &names).await?
        } else {
            LedgerSnapshot::empty()
        };
        let balanced = snapshot.balanced();

        let audit = writer_audit(context, observation, &names).await?;
        let root_calls = common::function_calls(&observation.transcript);
        let root_clean = root_avoids_increments(&root_calls, &names);
        let contended = audit.all_children_contended && audit.no_extra_sessions && root_clean;

        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "database_snapshot".to_string(),
            content: json!({
                "accumulator": snapshot.accumulator,
                "audit_rows": snapshot.audit_rows(),
                "per_writer": per_writer_values(&snapshot.audit_pairs),
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: "balanced_ledger".to_string(),
                    passed: balanced,
                    reason: format!(
                        "accumulator_rows={}, accumulator={}, audit_rows={}",
                        snapshot.accumulator_rows,
                        snapshot.accumulator,
                        snapshot.audit_rows(),
                    ),
                },
                CapturedInvariant {
                    id: "contended_writes".to_string(),
                    passed: contended,
                    reason: format!(
                        "children_in_tree={}/{WRITERS}, contended_children={}, \
                         no_extra_sessions={}, root_clean={root_clean}",
                        audit.children_in_tree, audit.contended_children, audit.no_extra_sessions,
                    ),
                },
            ],
            // Provenance is certified only when the ledger balances AND the
            // rows provably came from the three contending children; a failed
            // or root-forged run must not carry provenance evidence.
            provenance: if balanced && contended {
                [names.ledger.as_str(), names.audit.as_str()]
                    .into_iter()
                    .map(|relation| ProvenanceEvidence {
                        kind: "database_relation".to_string(),
                        source_id: format!("{DATABASE}/{relation}"),
                        relation: "captured_before_cleanup".to_string(),
                    })
                    .collect()
            } else {
                Vec::new()
            },
        }])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "database_snapshot".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["accumulator", "audit_rows", "per_writer"],
                "properties": {
                    "accumulator": { "type": "integer" },
                    "audit_rows": { "type": "integer", "minimum": 0 },
                    "per_writer": {
                        "type": "array",
                        "minItems": 3,
                        "maxItems": 3,
                        "items": {
                            "type": "object",
                            "required": ["writer", "rows"],
                            "properties": {
                                "writer": { "type": "integer", "minimum": 0, "maximum": 2 },
                                "rows": { "type": "integer", "minimum": 0 }
                            },
                            "additionalProperties": false
                        }
                    }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 8_192,
        }],
        invariants: vec![
            InvariantSpec {
                id: "balanced_ledger".to_string(),
                description:
                    "The accumulator reads exactly 15 with exactly fifteen per-writer audit rows."
                        .to_string(),
            },
            InvariantSpec {
                id: "contended_writes".to_string(),
                description:
                    "Every increment and audit row was produced by one of three direct writer children, never the root."
                        .to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn missing_database() -> ObjectiveEvaluation {
    assessment::prerequisite_failure(
        ASSESSMENTS,
        "database_capability_available",
        "database::query is unavailable",
    )
}

fn missing_primary(databases: &BTreeSet<String>) -> ObjectiveEvaluation {
    let reason = format!(
        "database `primary` is unavailable; configured databases: {}",
        databases.iter().cloned().collect::<Vec<_>>().join(", ")
    );
    assessment::prerequisite_failure(ASSESSMENTS, "primary_database_available", reason)
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
        for key in done_keys() {
            let _: Value = context
                .trigger("state::delete", json!({ "scope": names.scope, "key": key }))
                .await?;
        }
        if !context.function_exists("database::query").await? {
            return Ok(());
        }
        if !available_databases(context).await?.contains(DATABASE) {
            return Ok(());
        }
        let objects = database_objects(context, &names).await?;
        for kind in ["trigger", "view", "table"] {
            for object in objects.values().filter(|object| object.kind == kind) {
                if !sql_safe_name(&object.name) {
                    continue;
                }
                let _: Value = context
                    .trigger(
                        "database::execute",
                        json!({
                            "db": DATABASE,
                            "sql": format!(
                                "DROP {} IF EXISTS \"{}\"",
                                kind.to_ascii_uppercase(),
                                object.name
                            ),
                        }),
                    )
                    .await?;
            }
        }
        Ok(())
    })
}

#[derive(Debug)]
struct LedgerSnapshot {
    accumulator_rows: usize,
    accumulator: i64,
    audit_pairs: Vec<(i64, i64)>,
}

impl LedgerSnapshot {
    fn empty() -> Self {
        Self {
            accumulator_rows: 0,
            accumulator: -1,
            audit_pairs: Vec::new(),
        }
    }

    fn audit_rows(&self) -> usize {
        self.audit_pairs.len()
    }

    fn balanced(&self) -> bool {
        self.accumulator_rows == 1
            && self.accumulator == EXPECTED_TOTAL
            && sequences_balanced(&self.audit_pairs)
    }
}

async fn ledger_snapshot(context: &E2eContext, names: &Names) -> anyhow::Result<LedgerSnapshot> {
    let objects = database_objects(context, names).await?;
    let has_ledger = objects
        .get(&names.ledger)
        .is_some_and(|object| object.kind == "table");
    let has_audit = objects
        .get(&names.audit)
        .is_some_and(|object| object.kind == "table");

    let accumulator_rows = if has_ledger {
        query_rows(context, &format!("SELECT total FROM {}", names.ledger)).await?
    } else {
        Vec::new()
    };
    let accumulator = match accumulator_rows.as_slice() {
        [row] => integer_field(row, "total").unwrap_or(-1),
        _ => -1,
    };
    let audit_pairs = if has_audit {
        query_rows(
            context,
            &format!(
                "SELECT writer, seq FROM {} ORDER BY writer, seq",
                names.audit
            ),
        )
        .await?
        .iter()
        .filter_map(|row| Some((integer_field(row, "writer")?, integer_field(row, "seq")?)))
        .collect()
    } else {
        Vec::new()
    };
    Ok(LedgerSnapshot {
        accumulator_rows: accumulator_rows.len(),
        accumulator,
        audit_pairs,
    })
}

fn per_writer_values(pairs: &[(i64, i64)]) -> Vec<Value> {
    (0..i64::from(WRITERS))
        .map(|writer| {
            json!({
                "writer": writer,
                "rows": pairs.iter().filter(|(candidate, _)| *candidate == writer).count(),
            })
        })
        .collect()
}

#[derive(Debug)]
struct ContentionAudit {
    children_in_tree: usize,
    contended_children: usize,
    all_children_contended: bool,
    no_extra_sessions: bool,
}

/// Audit each prescribed writer child through its own transcript: the child
/// must be a direct depth-1 session, and its calls must show exactly its own
/// five increments, its own five audit rows, and its own done key — nothing
/// belonging to another writer.
async fn writer_audit(
    context: &E2eContext,
    observation: &ScenarioObservation,
    names: &Names,
) -> anyhow::Result<ContentionAudit> {
    let mut children_in_tree = 0usize;
    let mut contended_children = 0usize;
    for (writer, session_id) in (0..WRITERS).zip(names.writer_sessions.iter()) {
        let in_tree = observation.metrics.by_session.iter().any(|session| {
            session.session_id == *session_id
                && session.parent_session_id.as_deref() == Some(names.root_session.as_str())
                && session.depth == 1
        });
        if !in_tree {
            continue;
        }
        children_in_tree += 1;
        let transcript = context.transcript(session_id).await?;
        let calls = common::function_calls(&transcript);
        if writer_calls_contended(&calls, names, writer) {
            contended_children += 1;
        }
    }
    Ok(ContentionAudit {
        all_children_contended: children_in_tree == usize::from(WRITERS)
            && contended_children == usize::from(WRITERS),
        no_extra_sessions: observation.metrics.by_session.len() == usize::from(WRITERS) + 1,
        children_in_tree,
        contended_children,
    })
}

/// Pure per-child check over an ordered call list: exactly five atomic
/// increments, exactly its own five `(writer, seq)` audit rows, and a single
/// own-done-key state write that comes only after the last database write.
fn writer_calls_contended(
    calls: &[common::ObservedFunctionCall],
    names: &Names,
    writer: u8,
) -> bool {
    let expected_pairs: BTreeSet<(i64, i64)> = (1..=i64::from(INCREMENTS_PER_WRITER))
        .map(|seq| (i64::from(writer), seq))
        .collect();
    let mut increments = 0usize;
    let mut last_database_write = None;
    let mut audit_row_count = 0usize;
    let mut pairs: BTreeSet<(i64, i64)> = BTreeSet::new();
    let mut unreadable_insert = false;
    for (position, call) in calls.iter().enumerate() {
        if !DATABASE_WRITES.contains(&call.function_id.as_str()) {
            continue;
        }
        for sql in sql_statements(&call.arguments) {
            if is_increment(sql, &names.ledger) {
                increments += 1;
                last_database_write = Some(position);
            }
            if inserts_into(sql, &names.audit) {
                last_database_write = Some(position);
                match audit_insert_pairs(sql) {
                    Some(parsed) => {
                        audit_row_count += parsed.len();
                        pairs.extend(parsed);
                    }
                    None => unreadable_insert = true,
                }
            }
        }
    }
    let scope_writes: Vec<(usize, Option<u8>)> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| {
            call.function_id == "state::set"
                && call.arguments.get("scope").and_then(Value::as_str) == Some(names.scope.as_str())
        })
        .map(|(position, call)| {
            (
                position,
                call.arguments
                    .get("key")
                    .and_then(Value::as_str)
                    .and_then(parse_writer_index),
            )
        })
        .collect();
    let own_done_only = scope_writes.len() == 1 && scope_writes[0].1 == Some(writer);
    let done_after_increments =
        own_done_only && last_database_write.is_some_and(|write| write < scope_writes[0].0);

    increments == usize::from(INCREMENTS_PER_WRITER)
        && !unreadable_insert
        && audit_row_count == usize::from(INCREMENTS_PER_WRITER)
        && pairs == expected_pairs
        && done_after_increments
}

/// The root may create tables and seed the accumulator row, but it must never
/// run an increment or insert an audit row itself.
fn root_avoids_increments(calls: &[common::ObservedFunctionCall], names: &Names) -> bool {
    calls
        .iter()
        .filter(|call| DATABASE_WRITES.contains(&call.function_id.as_str()))
        .flat_map(|call| sql_statements(&call.arguments))
        .all(|sql| !is_increment(sql, &names.ledger) && !inserts_into(sql, &names.audit))
}

fn is_completion_watch(call: &common::ObservedFunctionCall, names: &Names) -> bool {
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
        && has_named_barrier(&call.arguments, names)
}

fn has_named_barrier(arguments: &Value, names: &Names) -> bool {
    let expected: BTreeSet<String> = done_keys().into_iter().collect();
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

fn flatten_sql(sql: &str) -> String {
    sql.to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

/// One atomic read-modify-write in the shipped SQLite dialect: a single
/// UPDATE that re-derives the accumulator from itself
/// (`SET total = total + 1`), never a read followed by a separate write.
fn is_increment(sql: &str, ledger: &str) -> bool {
    let flat = flatten_sql(sql);
    let ledger = ledger.to_ascii_lowercase();
    (flat.contains(&format!("update{ledger}set"))
        || flat.contains(&format!("update\"{ledger}\"set")))
        && flat.contains("settotal=total+1")
}

fn inserts_into(sql: &str, table: &str) -> bool {
    let flat = flatten_sql(sql);
    let table = table.to_ascii_lowercase();
    flat.contains("insert")
        && (flat.contains(&format!("into{table}")) || flat.contains(&format!("into\"{table}\"")))
}

/// Parse the literal `(writer, seq)` tuples of one audit INSERT. Returns
/// `None` when the statement cannot be read as literal tuples in the
/// prescribed column order (bound parameters, reversed columns,
/// INSERT..SELECT) — an unreadable insert fails the provenance audit
/// deterministically.
fn audit_insert_pairs(sql: &str) -> Option<Vec<(i64, i64)>> {
    let flat = flatten_sql(sql);
    if flat.contains("(seq,writer)") {
        return None;
    }
    let (_, values) = flat.split_once("values")?;
    let mut pairs = Vec::new();
    for tuple in values.split('(').skip(1) {
        let tuple = tuple.split(')').next()?;
        let mut numbers = tuple.split(',');
        let writer = numbers.next()?.trim().parse::<i64>().ok()?;
        let seq = numbers.next()?.trim().parse::<i64>().ok()?;
        pairs.push((writer, seq));
    }
    (!pairs.is_empty()).then_some(pairs)
}

fn sql_statements(arguments: &Value) -> Vec<&str> {
    arguments
        .get("sql")
        .and_then(Value::as_str)
        .into_iter()
        .chain(
            arguments
                .get("statements")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|statement| {
                    statement
                        .as_str()
                        .or_else(|| statement.get("sql").and_then(Value::as_str))
                }),
        )
        .collect()
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

#[derive(Debug)]
struct DatabaseObject {
    name: String,
    kind: String,
}

async fn database_objects(
    context: &E2eContext,
    names: &Names,
) -> anyhow::Result<BTreeMap<String, DatabaseObject>> {
    let rows = query_rows(
        context,
        &format!(
            "SELECT type, name FROM sqlite_master \
             WHERE name LIKE '{}%' OR (type = 'trigger' AND sql LIKE '%{}%') \
             ORDER BY type, name",
            names.table_prefix, names.table_prefix,
        ),
    )
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let object = DatabaseObject {
                name: row.get("name")?.as_str()?.to_string(),
                kind: row.get("type")?.as_str()?.to_string(),
            };
            Some((object.name.clone(), object))
        })
        .collect())
}

async fn query_rows(context: &E2eContext, sql: &str) -> anyhow::Result<Vec<Value>> {
    context
        .trigger_value("database::query", json!({ "db": DATABASE, "sql": sql }))
        .await?
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .with_context(|| format!("database::query returned malformed rows for {sql}"))
}

async fn available_databases(context: &E2eContext) -> anyhow::Result<BTreeSet<String>> {
    Ok(context
        .trigger_value("database::listDatabases", json!({}))
        .await?
        .get("databases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|database| {
            database
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect())
}

fn integer_field(row: &Value, field: &str) -> Option<i64> {
    row.get(field)
        .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
}

fn sql_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

struct Names {
    table_prefix: String,
    root_session: String,
    ledger: String,
    audit: String,
    scope: String,
    complete_label: String,
    barrier_id: String,
    writer_sessions: [String; 3],
}

impl Names {
    fn new(run_id: &str) -> Self {
        let suffix = validation_loop::suffix(run_id);
        let table_prefix = format!("ledger_{suffix}");
        let run_label = format!("ledger-{suffix}");
        Self {
            root_session: format!("e2e_{run_id}"),
            ledger: table_prefix.clone(),
            audit: format!("{table_prefix}_audit"),
            scope: format!("e2e:ledger:{run_id}"),
            complete_label: format!("ledger-balanced:{run_id}"),
            barrier_id: format!("ledger:{run_id}:writers"),
            writer_sessions: [
                format!("{run_label}-w0"),
                format!("{run_label}-w1"),
                format!("{run_label}-w2"),
            ],
            table_prefix,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update_call(names: &Names) -> common::ObservedFunctionCall {
        common::ObservedFunctionCall {
            function_id: "database::execute".to_string(),
            arguments: json!({
                "db": DATABASE,
                "sql": format!("UPDATE {} SET total = total + 1", names.ledger),
            }),
        }
    }

    fn audit_call(names: &Names, writer: u8, seq: u8) -> common::ObservedFunctionCall {
        common::ObservedFunctionCall {
            function_id: "database::execute".to_string(),
            arguments: json!({
                "db": DATABASE,
                "sql": format!(
                    "INSERT INTO {} (writer, seq) VALUES ({writer}, {seq})",
                    names.audit
                ),
            }),
        }
    }

    fn done_call(names: &Names, writer: u8) -> common::ObservedFunctionCall {
        common::ObservedFunctionCall {
            function_id: "state::set".to_string(),
            arguments: json!({
                "scope": names.scope,
                "key": done_key(writer),
                "value": "done",
            }),
        }
    }

    fn contended_writer_calls(names: &Names, writer: u8) -> Vec<common::ObservedFunctionCall> {
        let mut calls = Vec::new();
        for seq in 1..=INCREMENTS_PER_WRITER {
            calls.push(update_call(names));
            calls.push(audit_call(names, writer, seq));
        }
        calls.push(done_call(names, writer));
        calls
    }

    #[test]
    fn report_validation_requires_the_marker_and_brevity() {
        assert!(report_is_verified(REPORT_MARKER));
        assert!(report_is_verified(
            "Verified: LEDGER-BALANCED 15/15 (accumulator 15, 15 audit rows)."
        ));
        assert!(!report_is_verified("LEDGER-BALANCED 14/15"));
        assert!(!report_is_verified(&format!(
            "{REPORT_MARKER} {}",
            "x".repeat(MAX_REPORT_CHARS)
        )));
    }

    #[test]
    fn expected_sequences_cover_every_writer_exactly() {
        assert_eq!(
            EXPECTED_TOTAL,
            i64::from(WRITERS) * i64::from(INCREMENTS_PER_WRITER)
        );
        let expected = expected_sequences();
        assert_eq!(expected.len(), usize::from(WRITERS));
        for writer in 0..i64::from(WRITERS) {
            assert_eq!(expected.get(&writer).map(BTreeSet::len), Some(5));
        }

        let balanced: Vec<(i64, i64)> = (0..i64::from(WRITERS))
            .flat_map(|writer| (1..=i64::from(INCREMENTS_PER_WRITER)).map(move |seq| (writer, seq)))
            .collect();
        assert!(sequences_balanced(&balanced));

        let mut duplicated = balanced.clone();
        duplicated[14] = (0, 1);
        assert!(!sequences_balanced(&duplicated));
        assert!(!sequences_balanced(&balanced[..14]));
    }

    #[test]
    fn done_keys_parse_back_to_their_writer_indexes_only() {
        assert_eq!(parse_writer_index("done-00"), Some(0));
        assert_eq!(parse_writer_index("done-02"), Some(2));
        assert_eq!(parse_writer_index("done-03"), None);
        assert_eq!(parse_writer_index("done-0"), None);
        assert_eq!(parse_writer_index("worker-00"), None);
        assert_eq!(done_keys(), vec!["done-00", "done-01", "done-02"]);
    }

    #[test]
    fn the_barrier_matcher_requires_exactly_the_three_done_keys() {
        let names = Names::new("barrier-run");
        let arguments = json!({
            "trigger_type": "state",
            "label": names.complete_label,
            "config": { "scope": names.scope },
            "once": true,
            "conditions": [{
                "function_id": "state::barrier",
                "config": {
                    "id": names.barrier_id,
                    "carry": "/new_value",
                    "expect": ["done-00", "done-01", "done-02"],
                },
            }],
        });
        let call = common::ObservedFunctionCall {
            function_id: "engine::register_trigger".to_string(),
            arguments: arguments.clone(),
        };
        assert!(is_completion_watch(&call, &names));

        let mut missing = arguments.clone();
        missing["conditions"][0]["config"]["expect"] = json!(["done-00", "done-01"]);
        assert!(!has_named_barrier(&missing, &names));

        let mut extra = arguments;
        extra["conditions"][0]["config"]["expect"] =
            json!(["done-00", "done-01", "done-02", "done-03"]);
        assert!(!has_named_barrier(&extra, &names));
    }

    #[test]
    fn increments_are_single_atomic_updates_on_the_accumulator_only() {
        assert!(is_increment(
            "UPDATE ledger_abcd SET total = total + 1",
            "ledger_abcd"
        ));
        assert!(is_increment(
            "update \"ledger_abcd\" set total=total+1 where 1=1",
            "ledger_abcd"
        ));
        assert!(!is_increment(
            "UPDATE ledger_abcd SET total = 7",
            "ledger_abcd"
        ));
        assert!(!is_increment(
            "UPDATE ledger_abcd_audit SET total = total + 1",
            "ledger_abcd"
        ));
        assert!(!is_increment(
            "INSERT INTO ledger_abcd (total) VALUES (0)",
            "ledger_abcd"
        ));
    }

    #[test]
    fn audit_inserts_parse_literal_writer_sequence_tuples() {
        assert_eq!(
            audit_insert_pairs("INSERT INTO t (writer, seq) VALUES (2, 4)"),
            Some(vec![(2, 4)])
        );
        assert_eq!(
            audit_insert_pairs("insert into t (writer, seq) values (0,1),(0,2)"),
            Some(vec![(0, 1), (0, 2)])
        );
        assert_eq!(
            audit_insert_pairs("INSERT INTO t (seq, writer) VALUES (1, 0)"),
            None
        );
        assert_eq!(
            audit_insert_pairs("INSERT INTO t (writer, seq) VALUES (?, ?)"),
            None
        );
        assert_eq!(
            audit_insert_pairs("INSERT INTO t SELECT * FROM other"),
            None
        );
    }

    #[test]
    fn writer_transcript_audit_accepts_only_its_own_ordered_writes() {
        let names = Names::new("audit-run");
        let calls = contended_writer_calls(&names, 1);
        assert!(writer_calls_contended(&calls, &names, 1));
        assert!(!writer_calls_contended(&calls, &names, 0));

        let mut early_done = contended_writer_calls(&names, 1);
        let done = early_done.pop().unwrap();
        early_done.insert(0, done);
        assert!(!writer_calls_contended(&early_done, &names, 1));

        let mut foreign_done = contended_writer_calls(&names, 1);
        foreign_done.push(done_call(&names, 2));
        assert!(!writer_calls_contended(&foreign_done, &names, 1));

        let mut short = contended_writer_calls(&names, 1);
        short.remove(0);
        assert!(!writer_calls_contended(&short, &names, 1));
    }

    #[test]
    fn the_root_may_seed_the_accumulator_but_never_increment() {
        let names = Names::new("root-run");
        let seed = common::ObservedFunctionCall {
            function_id: "database::execute".to_string(),
            arguments: json!({
                "db": DATABASE,
                "sql": format!("INSERT INTO {} (total) VALUES (0)", names.ledger),
            }),
        };
        assert!(root_avoids_increments(&[seed], &names));
        assert!(!root_avoids_increments(&[update_call(&names)], &names));
        assert!(!root_avoids_increments(&[audit_call(&names, 0, 1)], &names));
    }

    #[test]
    fn cases_materialize_reproducibly_at_the_concurrent_tier() {
        use super::super::ComplexityTier;

        let first = materialize("attempt-a", 313).unwrap();
        let retry = materialize("attempt-b", 313).unwrap();
        first.validate().unwrap();
        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs, retry.case.inputs);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert_ne!(first.spec.prompt, retry.spec.prompt);

        let other_seed = materialize("attempt-c", 314).unwrap();
        assert_ne!(first.case.case_id, other_seed.case.case_id);

        assert_eq!(first.case.complexity.tier, ComplexityTier::L3Concurrent);
        assert_eq!(
            usize::from(first.case.complexity.profile.artifact_count),
            first.case.deliverable_contract.artifacts.len()
        );
        assert!(first.capture.is_some());
        assert!(first.case.deliverable_contract.capture_before_cleanup);
        assert!(first.case.deliverable_contract.provenance_required);
    }
}
