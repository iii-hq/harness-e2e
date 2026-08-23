//! `depth_ladder` — dose-response delegation depth, one rung per case.
//!
//! The same single-lane relay workload runs at depth 2, 4, and 6; the case
//! seed selects the rung (`RUNGS`), so every rung is its own longitudinally
//! comparable case. The rung ladder turns pass/fail into a capability
//! frontier: a Harness version is characterized by the deepest relay it
//! sustains, and version-over-version comparison reads as frontier movement
//! instead of a single bit.
//!
//! Per rung the coordinator (level 0) arms one one-shot wake on the terminal
//! relay key, spawns exactly ONE child, and ends its turn with a dispatch
//! note. Each level writes its own keyed row and spawns exactly one deeper
//! child, until level N writes its row and spawns nothing. The terminal
//! write wakes the coordinator, which reports the completion marker plus the
//! terminal token. Exact rows prove the deliverable; the per-depth session
//! chain and per-child transcripts prove each row was produced at its own
//! depth rather than by the coordinator itself.
//!
//! Tier straddle, on purpose: the depth-2 rung derives `L2Stateful`
//! (coordination_edges 2 < 3 and dependency_depth 2 < 3), while the depth-4
//! and depth-6 rungs derive `L4Coordinated` (coordination_edges >= 3). The
//! ladder therefore spans the stateful-to-coordinated boundary honestly
//! instead of pinning every rung to one tier.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;
use crate::wire::SessionUsage;

use super::assessment::{self, AssessmentSpec};
use super::{
    common, ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture,
    ComplexityProfile, DeliverableCaptureFuture, DeliverableContract, EvaluationFuture,
    ExecutionPolicy, InvariantSpec, MaterializedScenario, ObjectiveEvaluation, ProvenanceEvidence,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "depth_ladder";
const VERSION: u32 = 1;
const ROWS_DELIVERABLE_ID: &str = "relay_rows";
const MAX_REPORT_CHARS: usize = 300;

/// One ladder rung. The seed is the case selector: running the ladder means
/// running the scenario once per rung seed, and each rung keeps its own
/// longitudinal series.
#[derive(Debug, Clone, Copy)]
struct Rung {
    seed: u64,
    depth: u8,
}

pub const CANONICAL_SEED: u64 = 4001;
const RUNGS: [Rung; 3] = [
    Rung {
        seed: 4001,
        depth: 2,
    },
    Rung {
        seed: 4002,
        depth: 4,
    },
    Rung {
        seed: 4003,
        depth: 6,
    },
];

const RELAY_DELIVERED: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "relay_delivered",
    35,
    "Every relay level row is exact in state and the woken report carries the completion marker \
     and the terminal token.",
    EvaluationDimension::Deliverable,
);
const DEPTH_PROVENANCE: AssessmentSpec = AssessmentSpec::hard_gated(
    "depth_provenance",
    30,
    "Exactly one session sits at each depth in one unbroken parent chain, each writing its own \
     row with a single state write; non-terminal levels spawn exactly once and the terminal \
     level spawns nothing.",
);
const SINGLE_LANE: AssessmentSpec = AssessmentSpec::hard_gated(
    "single_lane",
    20,
    "The run stays one lane wide: N+1 sessions in total, exactly one root spawn armed after the \
     single wake registration, and zero function-call errors.",
);
const DISPATCH_REPORT: AssessmentSpec = AssessmentSpec::score_only(
    "dispatch_report",
    15,
    "The final report stays a single compact line of at most 300 characters.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    RELAY_DELIVERED,
    DEPTH_PROVENANCE,
    SINGLE_LANE,
    DISPATCH_REPORT,
];

fn rung(seed: u64) -> Rung {
    RUNGS
        .iter()
        .copied()
        .find(|rung| rung.seed == seed)
        .unwrap_or_else(|| RUNGS[(seed as usize) % RUNGS.len()])
}

fn relay_key(level: u8) -> String {
    format!("relay-{level:02}")
}

fn relay_keys(depth: u8) -> Vec<String> {
    (1..=depth).map(relay_key).collect()
}

/// Run-scoped relay tokens: opaque to the subject before the run, but handed
/// to the coordinator inline, so the provenance gates — not token secrecy —
/// are what force the relay to actually descend.
fn relay_token(run_id: &str, level: u8) -> String {
    format!(
        "DPT-{:016x}",
        super::stable_seed(&format!("{ID}:{run_id}:{}", relay_key(level)))
    )
}

fn report_marker(depth: u8) -> String {
    format!("DEPTH-{depth}-COMPLETE")
}

fn expected_row(run_id: &str, level: u8) -> Value {
    json!({
        "level": level,
        "token": relay_token(run_id, level),
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
            "depth": rung.depth,
            "relay_keys": relay_keys(rung.depth),
            "report_marker": report_marker(rung.depth),
            "token_derivation": "run-scoped",
            "ladder_rungs": RUNGS.iter().map(|rung| rung.depth).collect::<Vec<_>>(),
        }),
        complexity_profile(rung.depth),
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::state".to_string(),
            "iii::triggers".to_string(),
            "e2e::subagents".to_string(),
        ],
        deliverable_contract(rung.depth),
    )?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(namespace, rung),
        case,
        capture: Some(capture),
    })
}

/// The rung is the profile: depth 2 derives `L2Stateful` (coordination_edges
/// 2 < 3 and dependency_depth 2 < 3) while depths 4 and 6 derive
/// `L4Coordinated` (coordination_edges >= 3), so the ladder straddles the
/// tier boundary on purpose.
fn complexity_profile(depth: u8) -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 2,
        dependency_depth: depth,
        state_transitions: u16::from(depth),
        wake_cycles: 1,
        artifact_count: 1,
        coordination_edges: u16::from(depth),
        ..ComplexityProfile::default()
    }
}

fn scenario_for_case(run_id: &str, rung: Rung) -> ScenarioSpec {
    let names = Names::new(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: prompt(&names, run_id, rung.depth),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 12 + 4 * u32::from(rung.depth),
            max_output_tokens: Some(8_192),
            max_total_tokens: Some(300_000 + 150_000 * u64::from(rung.depth)),
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

fn prompt(names: &Names, run_id: &str, depth: u8) -> String {
    let assignments = (1..=depth)
        .map(|level| {
            format!(
                "- level {level} → key `{}` → token `{}`",
                relay_key(level),
                relay_token(run_id, level)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"Run a delegation-depth rung: one relay lane exactly {depth} levels deep in the isolated
state scope `{scope}`.

The control plane is parent-owned: a reaction may wake this coordinator, but it never starts an
agent. Every relay level must be spawned by the level directly above it, starting from a live
coordinator turn. You are level 0 and you spawn only level 1.

Do all of the following in ONE response, in this order:

1. Arm exactly one one-shot state wake for key `{terminal_key}`, with top-level label
   `{complete_label}`. The registration is wake-only: omit any function target. Register
   nothing else.
2. Directly spawn exactly ONE child — relay level 1 — and hand it this complete assignment
   table inline:

{assignments}

3. Reply with a one-line dispatch note containing `DEPTH-{depth}-DISPATCHED`, then end the
   turn. The relay completes asynchronously; do not poll, and spawn nothing else.

The relay contract, which every level must follow and pass down verbatim: level `d` writes
exactly one state value at `{scope}` / its assigned key:

`{{ "level": <d>, "token": "<its token>" }}`

Then, while `d` is below {depth}, it spawns exactly one child for level `d`+1 and hands the
remaining assignment rows down verbatim; level {depth} writes its row and spawns nothing.
Levels must not read state, register reactions, report upward, or touch any other key. Narrow
each child to function discovery, its single state write, and — below level {depth} — its
single spawn.

When the `{complete_label}` wake arrives, report in this same session; do not spawn a reporter.
Return one line of at most {max_chars} characters containing `{marker}` and the level-{depth}
token `{terminal_token}` verbatim."#,
        scope = names.scope,
        terminal_key = relay_key(depth),
        complete_label = names.complete_label,
        max_chars = MAX_REPORT_CHARS,
        marker = report_marker(depth),
        terminal_token = relay_token(run_id, depth),
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
    let depth = case_depth(&observation.case)?;
    let calls = common::function_calls(&observation.transcript);

    let registrations: Vec<_> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.function_id == "engine::register_trigger")
        .collect();
    let completion_watch = registrations
        .iter()
        .find(|(_, call)| is_completion_watch(call, &names, depth))
        .map(|(position, _)| *position);
    let spawns: Vec<_> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.function_id == "harness::spawn")
        .collect();
    let wake_before_spawn = completion_watch.is_some_and(|watch| {
        registrations.len() == 1 && spawns.iter().all(|(position, _)| *position > watch)
    });

    let audit = relay_audit(
        context,
        &names,
        run_id,
        depth,
        &observation.metrics.by_session,
    )
    .await?;

    let report_complete = report_completes(&observation.response, run_id, depth);
    let sessions_expected = observation.metrics.totals.sessions == u64::from(depth) + 1;
    let no_errors = observation.metrics.totals.function_call_errors == 0;
    let compact = dispatch_report_compact(&observation.response);

    Ok(assessment::build_evaluation([
        RELAY_DELIVERED.full_or_zero(
            audit.rows_exact && report_complete,
            format!(
                "exact_rows={}/{depth}, report_complete={report_complete}",
                audit.exact_rows
            ),
        ),
        DEPTH_PROVENANCE.full_or_zero(
            audit.chain_provenance,
            format!(
                "lane_chained={}, single_writes={}, spawn_counts={}, lane_discipline={}",
                audit.lane_chained,
                audit.single_writes,
                audit.spawn_counts_ok,
                audit.lane_discipline
            ),
        ),
        SINGLE_LANE.full_or_zero(
            sessions_expected && spawns.len() == 1 && wake_before_spawn && no_errors,
            format!(
                "total_sessions={} (expected {}), root_spawns={}, \
                 wake_before_spawn={wake_before_spawn}, function_errors={}",
                observation.metrics.totals.sessions,
                u64::from(depth) + 1,
                spawns.len(),
                observation.metrics.totals.function_call_errors
            ),
        ),
        DISPATCH_REPORT.full_or_zero(
            compact,
            format!(
                "response_chars={} (limit {MAX_REPORT_CHARS})",
                observation.response.chars().count()
            ),
        ),
    ]))
}

struct RelayAudit {
    exact_rows: usize,
    rows_exact: bool,
    lane_chained: bool,
    single_writes: bool,
    spawn_counts_ok: bool,
    lane_discipline: bool,
    chain_provenance: bool,
}

/// Verify every relay row and its provenance: each of the N keys holds the
/// exact row, exactly one session sits at each depth in one unbroken parent
/// chain, each wrote only its own row with a single exact state write, and
/// every non-terminal level spawned exactly once while the terminal level
/// spawned nothing.
async fn relay_audit(
    context: &E2eContext,
    names: &Names,
    run_id: &str,
    depth: u8,
    by_session: &[SessionUsage],
) -> anyhow::Result<RelayAudit> {
    let mut exact_rows = 0usize;
    for level in 1..=depth {
        let observed = get_state(context, &names.scope, &relay_key(level)).await?;
        if observed == expected_row(run_id, level) {
            exact_rows += 1;
        }
    }
    let rows_exact = exact_rows == usize::from(depth);

    let chain = relay_chain(by_session, &names.root_session, depth);
    let lane_chained = chain.len() == usize::from(depth);
    let mut single_writes = lane_chained;
    let mut spawn_counts_ok = lane_chained;
    let mut lane_discipline = lane_chained;
    for (level, session_id) in (1..=depth).zip(chain.iter()) {
        let transcript = context.transcript(session_id).await?;
        let session_calls = common::function_calls(&transcript);
        // Discovery through `engine::functions::*` is expected setup and is
        // exempt from the indiscipline count.
        lane_discipline &= session_calls.iter().all(|call| {
            call.function_id == "state::set"
                || call.function_id == "harness::spawn"
                || call.function_id.starts_with("engine::functions::")
        });
        let writes: Vec<_> = session_calls
            .iter()
            .filter(|call| call.function_id == "state::set")
            .collect();
        single_writes &= writes.len() == 1
            && writes[0]
                .arguments
                .get("key")
                .and_then(Value::as_str)
                .and_then(|key| parse_relay_level(key, depth))
                == Some(level)
            && writes[0].arguments
                == json!({
                    "scope": names.scope,
                    "key": relay_key(level),
                    "value": expected_row(run_id, level),
                });
        let spawn_count = session_calls
            .iter()
            .filter(|call| call.function_id == "harness::spawn")
            .count();
        spawn_counts_ok &= if level < depth {
            spawn_count == 1
        } else {
            spawn_count == 0
        };
    }
    let chain_provenance = lane_chained && single_writes && spawn_counts_ok && lane_discipline;

    Ok(RelayAudit {
        exact_rows,
        rows_exact,
        lane_chained,
        single_writes,
        spawn_counts_ok,
        lane_discipline,
        chain_provenance,
    })
}

/// Resolve the single relay session at each depth 1..=N into one unbroken
/// parent chain rooted at the coordinator. A missing, duplicated, or
/// mis-parented level truncates the chain at that point.
fn relay_chain(by_session: &[SessionUsage], root_session: &str, depth: u8) -> Vec<String> {
    let mut chain: Vec<String> = Vec::new();
    let mut parent = root_session.to_string();
    for level in 1..=depth {
        let candidates: Vec<_> = by_session
            .iter()
            .filter(|session| session.depth == u32::from(level))
            .collect();
        let [session] = candidates.as_slice() else {
            break;
        };
        if session.parent_session_id.as_deref() != Some(parent.as_str()) {
            break;
        }
        parent = session.session_id.clone();
        chain.push(session.session_id.clone());
    }
    chain
}

fn parse_relay_level(key: &str, depth: u8) -> Option<u8> {
    let level = key.strip_prefix("relay-")?.parse::<u8>().ok()?;
    ((1..=depth).contains(&level) && key == relay_key(level)).then_some(level)
}

fn report_completes(response: &str, run_id: &str, depth: u8) -> bool {
    response.contains(&report_marker(depth)) && response.contains(&relay_token(run_id, depth))
}

fn dispatch_report_compact(response: &str) -> bool {
    !response.trim().is_empty() && response.chars().count() <= MAX_REPORT_CHARS
}

fn case_depth(case: &ScenarioCase) -> anyhow::Result<u8> {
    case.inputs
        .get("depth")
        .and_then(Value::as_u64)
        .and_then(|depth| u8::try_from(depth).ok())
        .filter(|depth| *depth > 0)
        .ok_or_else(|| anyhow::anyhow!("depth_ladder case is missing a positive depth input"))
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let names = Names::new(run_id);
        let depth = case_depth(&observation.case)?;
        let audit = relay_audit(
            context,
            &names,
            run_id,
            depth,
            &observation.metrics.by_session,
        )
        .await?;
        let mut rows = Vec::new();
        for level in 1..=depth {
            let key = relay_key(level);
            let value = get_state(context, &names.scope, &key).await?;
            rows.push(json!({ "key": key, "value": value }));
        }
        // Location evidence is only attached once the audit establishes that
        // the rows are exact and were written at their own depths; failed
        // runs carry no unearned provenance.
        let provenance = if audit.rows_exact && audit.chain_provenance {
            (1..=depth)
                .map(|level| ProvenanceEvidence {
                    kind: "state_location".to_string(),
                    source_id: format!("{}/{}", names.scope, relay_key(level)),
                    relation: "written_by_relay_level".to_string(),
                })
                .collect()
        } else {
            Vec::new()
        };

        Ok(vec![CapturedDeliverable {
            id: ROWS_DELIVERABLE_ID.to_string(),
            kind: "state_bundle".to_string(),
            content: json!({ "depth": depth, "rows": rows }).into(),
            invariants: vec![
                CapturedInvariant {
                    id: "relay_rows_exact".to_string(),
                    passed: audit.rows_exact,
                    reason: format!("observed {}/{depth} exact relay row(s)", audit.exact_rows),
                },
                CapturedInvariant {
                    id: "depth_provenance".to_string(),
                    passed: audit.chain_provenance,
                    reason: format!(
                        "lane_chained={}, single_writes={}, spawn_counts={}, lane_discipline={}",
                        audit.lane_chained,
                        audit.single_writes,
                        audit.spawn_counts_ok,
                        audit.lane_discipline
                    ),
                },
            ],
            provenance,
        }])
    })
}

fn deliverable_contract(depth: u8) -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: ROWS_DELIVERABLE_ID.to_string(),
            kind: "state_bundle".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["depth", "rows"],
                "properties": {
                    "depth": { "const": depth },
                    "rows": {
                        "type": "array",
                        "minItems": depth,
                        "maxItems": depth,
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
        }],
        invariants: vec![
            InvariantSpec {
                id: "relay_rows_exact".to_string(),
                description: "Every relay level key holds the exact assigned row.".to_string(),
            },
            InvariantSpec {
                id: "depth_provenance".to_string(),
                description:
                    "Each row was written once by the single session at its own depth in one \
                     unbroken parent chain."
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
        // Delete every key the ladder can address (the deepest rung), not
        // just the current rung's, so a misbehaving run cannot leak rows
        // between attempts sharing a scope prefix.
        let deepest = RUNGS
            .iter()
            .map(|rung| rung.depth)
            .max()
            .unwrap_or_default();
        for key in relay_keys(deepest) {
            let _: Value = context
                .trigger("state::delete", json!({ "scope": names.scope, "key": key }))
                .await?;
        }
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

fn is_completion_watch(call: &common::ObservedFunctionCall, names: &Names, depth: u8) -> bool {
    let terminal_key = relay_key(depth);
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
            == Some(terminal_key.as_str())
        && call.arguments.get("label").and_then(Value::as_str)
            == Some(names.complete_label.as_str())
        && common::requested_once(&call.arguments)
        && common::is_wake_registration(&call.arguments)
}

struct Names {
    scope: String,
    root_session: String,
    complete_label: String,
}

impl Names {
    fn new(run_id: &str) -> Self {
        Self {
            scope: format!("e2e:depth:{run_id}"),
            root_session: format!("e2e_{run_id}"),
            complete_label: format!("depth-complete:{run_id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_seeds_select_their_rungs_and_fall_back_by_modulo() {
        assert_eq!(rung(4001).depth, 2);
        assert_eq!(rung(4002).depth, 4);
        assert_eq!(rung(4003).depth, 6);
        assert_eq!(rung(313).depth, RUNGS[313 % RUNGS.len()].depth);
        assert_eq!(rung(CANONICAL_SEED).depth, RUNGS[0].depth);
    }

    #[test]
    fn relay_tokens_are_run_scoped_and_stable() {
        assert_eq!(relay_token("attempt-a", 3), relay_token("attempt-a", 3));
        assert_ne!(relay_token("attempt-a", 3), relay_token("attempt-a", 4));
        assert_ne!(relay_token("attempt-a", 3), relay_token("attempt-b", 3));
        assert!(relay_token("attempt-a", 1).starts_with("DPT-"));
    }

    #[test]
    fn the_shallowest_rung_is_stateful_and_deeper_rungs_are_coordinated() {
        use super::super::ComplexityTier;

        let shallowest = materialize("attempt-a", 4001).unwrap();
        assert_eq!(shallowest.case.complexity.tier, ComplexityTier::L2Stateful);
        for seed in [4002, 4003] {
            let rung_case = materialize("attempt-a", seed).unwrap();
            assert_eq!(
                rung_case.case.complexity.tier,
                ComplexityTier::L4Coordinated,
                "seed {seed}"
            );
        }
    }

    #[test]
    fn every_rung_publishes_a_reproducible_case_with_its_depth() {
        for rung_spec in RUNGS {
            let first = materialize("attempt-a", rung_spec.seed).unwrap();
            let retry = materialize("attempt-b", rung_spec.seed).unwrap();
            assert_eq!(first.case.case_id, retry.case.case_id);
            assert_eq!(first.case.inputs, retry.case.inputs);
            assert_eq!(
                first.case.inputs.get("depth").and_then(Value::as_u64),
                Some(u64::from(rung_spec.depth))
            );
            assert_eq!(
                first
                    .case
                    .inputs
                    .get("relay_keys")
                    .and_then(Value::as_array)
                    .map(Vec::len),
                Some(usize::from(rung_spec.depth))
            );
            assert_eq!(first.case.deliverable_contract.artifacts.len(), 1);
            assert_ne!(first.spec.prompt, retry.spec.prompt);
        }
    }

    #[test]
    fn report_validation_requires_the_marker_and_the_terminal_token() {
        let run_id = "report-run";
        let complete = format!(
            "{} — terminal token {}",
            report_marker(4),
            relay_token(run_id, 4)
        );
        assert!(report_completes(&complete, run_id, 4));

        let missing_marker = relay_token(run_id, 4);
        assert!(!report_completes(&missing_marker, run_id, 4));

        let missing_token = report_marker(4);
        assert!(!report_completes(&missing_token, run_id, 4));

        let shallower_token = format!("{} {}", report_marker(4), relay_token(run_id, 3));
        assert!(!report_completes(&shallower_token, run_id, 4));
    }

    #[test]
    fn dispatch_reports_must_be_non_empty_and_within_the_limit() {
        assert!(dispatch_report_compact("DEPTH-4-COMPLETE DPT-0000000000000000"));
        assert!(dispatch_report_compact(&"x".repeat(MAX_REPORT_CHARS)));
        assert!(!dispatch_report_compact(&"x".repeat(MAX_REPORT_CHARS + 1)));
        assert!(!dispatch_report_compact("   "));
    }

    #[test]
    fn the_completion_watch_matcher_pins_scope_key_label_and_lifecycle() {
        let names = Names::new("wake-run");
        let call = |arguments: Value| common::ObservedFunctionCall {
            function_id: "engine::register_trigger".to_string(),
            arguments,
        };
        let matching = json!({
            "trigger_type": "state",
            "config": { "scope": names.scope, "key": relay_key(4) },
            "label": names.complete_label,
            "once": true,
        });
        assert!(is_completion_watch(&call(matching.clone()), &names, 4));

        let mut wrong_key = matching.clone();
        wrong_key["config"]["key"] = json!(relay_key(3));
        assert!(!is_completion_watch(&call(wrong_key), &names, 4));

        let mut wrong_label = matching.clone();
        wrong_label["label"] = json!("other-label");
        assert!(!is_completion_watch(&call(wrong_label), &names, 4));

        let mut not_once = matching.clone();
        not_once["once"] = json!(false);
        assert!(!is_completion_watch(&call(not_once), &names, 4));

        let mut targeted = matching;
        targeted["target"] = json!({ "function_id": "state::set" });
        assert!(!is_completion_watch(&call(targeted), &names, 4));
    }

    #[test]
    fn relay_keys_parse_back_to_their_levels_only_within_the_rung() {
        assert_eq!(parse_relay_level("relay-03", 4), Some(3));
        assert_eq!(parse_relay_level("relay-04", 4), Some(4));
        assert_eq!(parse_relay_level("relay-00", 4), None);
        assert_eq!(parse_relay_level("relay-05", 4), None);
        assert_eq!(parse_relay_level("relay-3", 4), None);
        assert_eq!(parse_relay_level("other-01", 4), None);
    }

    #[test]
    fn expected_rows_carry_the_level_and_its_run_scoped_token() {
        let row = expected_row("row-run", 5);
        assert_eq!(row.get("level").and_then(Value::as_u64), Some(5));
        assert_eq!(
            row.get("token").and_then(Value::as_str),
            Some(relay_token("row-run", 5).as_str())
        );
    }
}
