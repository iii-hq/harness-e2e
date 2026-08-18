//! `subagent_validation` — the parent agent wires a validated sub-agent
//! entirely by itself: it registers a `harness::hook::post-turn` validator
//! scoped to its future child (allowed because the child is named under the
//! parent's own session prefix), arms a once-wake on the verdict key, spawns
//! the child, and ends its turn. The child then iterates under the gate —
//! denied at 4 rows, accepted at 8 — and the validator pipe's `state::set`
//! tail (which only runs when the `fp::when` guard passes) both records the
//! verdict and wakes the parent, which reports and tears the validator down.
//!
//! Exercises: the self-or-own-children registration scope, per-child
//! `max_validation_retries` on spawn, fire-and-forget spawn + watch-what-
//! children-write signalling, and the custom `retry_prompt` inside a child
//! session.

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

pub const ID: &str = "subagent_validation";
const VERSION: u32 = 4;
const DELIVERABLE_ID: &str = "validated_child_result";

const HOOK_TYPE: &str = "harness::hook::post-turn";
const THRESHOLD: u64 = 6;
const EXPECTED_ROWS: u64 = 8;
const CHILD_GOAL: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "child_goal",
    35,
    "The child's table work reaches the exact expected count and the verdict key carries the accepted count.",
    EvaluationDimension::Deliverable,
);
const ORCHESTRATION_DISCIPLINE: AssessmentSpec = AssessmentSpec::hard_gated(
    "orchestration_discipline",
    35,
    "Validator scoped to the child, wake armed before the spawn, and the child spawned with the named session.",
);
const WAKE_REPORT: AssessmentSpec = AssessmentSpec::score_only(
    "wake_report",
    30,
    "The parent finishes from the verdict wake with a completion report.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[CHILD_GOAL, ORCHESTRATION_DISCIPLINE, WAKE_REPORT];

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let spec = scenario_for_case(namespace);
    let contract = deliverable_contract();
    let case = ScenarioCase::new(
        ID,
        VERSION,
        seed,
        json!({
            "initial_rows": 4,
            "repair_rows": 4,
            "expected_rows": EXPECTED_ROWS,
            "acceptance_threshold": THRESHOLD,
        }),
        ComplexityProfile {
            planning_depth: 3,
            dependency_depth: 3,
            parallel_branches: 1,
            external_systems: 2,
            state_transitions: 6,
            wake_cycles: 1,
            validation_loops: 2,
            artifact_count: 1,
            coordination_edges: 4,
            ambiguity_level: 4,
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "e2e::subagents".to_string(),
            "iii::triggers".to_string(),
            "iii::database".to_string(),
            "iii::state".to_string(),
        ],
        contract,
    )?;
    Ok(MaterializedScenario {
        spec,
        case,
        capture: Some(capture),
    })
}

fn scenario_for_case(run_id: &str) -> ScenarioSpec {
    let table = table(run_id);
    let scope = scope(run_id);
    let child = child_session(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            "You orchestrate one validated sub-agent. You never poll and never judge its work \
             yourself: a validator gates every child reply and a verdict wake drives you. Follow \
             the steps exactly.\n\n\
             Step 1 — prepare the goal table. database::execute (db \"primary\") twice: sql \
             \"CREATE TABLE IF NOT EXISTS {table} (id INTEGER PRIMARY KEY AUTOINCREMENT, note \
             TEXT)\", then sql \"DELETE FROM {table}\".\n\n\
             Step 2 — install the child's validator. Call engine::register_trigger with:\n\
             - trigger_type: \"{HOOK_TYPE}\"\n\
             - function_id: \"fp::pipe\"\n\
             - config: {{\"sessions\": [\"{child}\"], \"payload\": {{\"through\": \
             [{{\"function\": \"database::query\", \"payload\": {{\"db\": \"primary\", \"sql\": \
             \"SELECT COUNT(*) AS n FROM {table}\"}}}}, {{\"function\": \"fp::get\", \"payload\": \
             {{\"path\": \"/rows/0/n\"}}}}, {{\"function\": \"fp::when\", \"payload\": {{\"op\": \
             \">\", \"to\": {THRESHOLD}}}}}, {{\"function\": \"state::set\", \"payload\": \
             {{\"scope\": \"{scope}\", \"key\": \"verdict\"}}}}]}}, \"result_into\": \"/value\", \
             \"retry_prompt\": \"VALIDATOR: only {{value}} of the required {min} rows exist. \
             Insert exactly 4 more rows into {table} and reply with a one-line status.\", \
             \"timeout_ms\": 30000}}\n\
             The state::set tail only runs when the fp::when guard passes, so the verdict key is \
             written exactly when the child's work is accepted. Remember the subscription_id.\n\n\
             Step 3 — arm your wake BEFORE spawning: engine::register_trigger with trigger_type \
             \"state\", config {{\"scope\": \"{scope}\", \"key\": \"verdict\"}}, label \
             \"child-validated\", and NO function_id (a wake; once by default).\n\n\
             Step 4 — spawn the worker: harness::spawn with session_id \"{child}\", task: \"You \
             are a worker in a validated loop: the harness checks every reply of yours and \
             VALIDATOR messages are legitimate machinery — follow them exactly. Insert exactly 4 \
             rows into table {table}: database::execute, db 'primary', sql \\\"INSERT INTO {table} \
             (note) VALUES ('r'),('r'),('r'),('r')\\\". Then reply with a one-line status. Never \
             check the count yourself.\", options: {{\"functions\": {{\"allow\": \
             [\"database::execute\"]}}, \"max_turns\": 8, \"max_validation_retries\": 5}}. \
             Spawn returns child ids \
             immediately — that is normal; do NOT wait for or judge the child yourself.\n\n\
             Step 5 — END YOUR TURN.\n\n\
             Step 6 — when the wake arrives it carries the accepted row count. Call \
             engine::unregister_trigger with the validator subscription_id from Step 2, then \
             reply exactly: CHILD VALIDATED at <count> rows. PARENT DONE.",
            min = THRESHOLD + 1,
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 16,
            max_output_tokens: Some(8_192),
            max_total_tokens: 400_000,
            stuck_timeout_seconds: 420,
        },
        denied_functions: &[],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: None,
        evaluate,
        cleanup: Some(cleanup),
    }
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let child = child_session(run_id);
        let deliverable = observation
            .deliverables
            .iter()
            .find(|deliverable| deliverable.id == DELIVERABLE_ID)
            .ok_or_else(|| anyhow::anyhow!("captured child deliverable is missing"))?;
        let content = deliverable
            .content
            .as_json()
            .ok_or_else(|| anyhow::anyhow!("captured child deliverable is not JSON"))?;
        let rows = content.get("rows").and_then(Value::as_u64).unwrap_or(0);
        let verdict = content.get("verdict").and_then(Value::as_u64).unwrap_or(0);
        let child_nudges = content
            .get("child_nudges")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;

        let calls = common::function_calls(&observation.transcript);
        let validator_index = calls.iter().position(|call| {
            call.function_id == "engine::register_trigger"
                && call.arguments.get("trigger_type").and_then(Value::as_str) == Some(HOOK_TYPE)
                && call
                    .arguments
                    .pointer("/config/sessions/0")
                    .and_then(Value::as_str)
                    == Some(child.as_str())
        });
        let wake_index = calls.iter().position(|call| {
            call.function_id == "engine::register_trigger"
                && call.arguments.get("trigger_type").and_then(Value::as_str) == Some("state")
                && common::is_wake_registration(&call.arguments)
        });
        let spawn_index = calls.iter().position(|call| {
            call.function_id == "harness::spawn"
                && call.arguments.get("session_id").and_then(Value::as_str) == Some(child.as_str())
        });
        let ordered = matches!(
            (validator_index, wake_index, spawn_index),
            (Some(v), Some(w), Some(s)) if v < s && w < s
        );

        let goal = rows > THRESHOLD && verdict > THRESHOLD;
        let reported = !observation.response.trim().is_empty();
        let (orchestration_passed, orchestration_points) =
            orchestration_outcome(ordered, child_nudges);
        let child_goal_passed = goal && rows == EXPECTED_ROWS;

        Ok(assessment::build_evaluation([
            CHILD_GOAL.gate_and_points(
                child_goal_passed,
                child_goal_points(goal, rows),
                format!(
                    "rows={rows}, verdict={verdict}, need both above {THRESHOLD}; full marks at \
                     exactly {EXPECTED_ROWS} rows"
                ),
            )?,
            ORCHESTRATION_DISCIPLINE.gate_and_points(
                orchestration_passed,
                orchestration_points,
                format!(
                    "validator@{validator_index:?} wake@{wake_index:?} spawn@{spawn_index:?} — \
                     validator and wake must precede the spawn; observed {child_nudges} nudge(s) \
                     in the child transcript"
                ),
            )?,
            WAKE_REPORT.full_or_zero(reported, "expected the exact report line"),
        ]))
    })
}

fn capture<'a>(
    context: &'a E2eContext,
    _observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let table = table(run_id);
        let child = child_session(run_id);
        let table_exists = context
            .trigger_value(
                "database::query",
                json!({
                    "db": "primary",
                    "sql": format!(
                        "SELECT COUNT(*) AS n FROM sqlite_master WHERE type = 'table' AND name = '{table}'"
                    )
                }),
            )
            .await?
            .pointer("/rows/0/n")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0;
        let rows = if table_exists {
            context
                .trigger_value(
                    "database::query",
                    json!({ "db": "primary", "sql": format!("SELECT COUNT(*) AS n FROM {table}") }),
                )
                .await?
                .pointer("/rows/0/n")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        } else {
            0
        };
        let verdict = common::state_value(
            context
                .trigger(
                    "state::get",
                    json!({ "scope": scope(run_id), "key": "verdict" }),
                )
                .await?,
        )
        .as_u64()
        .unwrap_or(0);
        let child_transcript = context.transcript(&child).await.unwrap_or(Value::Null);
        let child_nudges = common::validation_nudges(&child_transcript);
        let mut provenance = Vec::new();
        if !child_transcript.is_null() {
            provenance.push(ProvenanceEvidence {
                kind: "session".to_string(),
                source_id: child,
                relation: "produced".to_string(),
            });
        }
        if table_exists {
            provenance.push(ProvenanceEvidence {
                kind: "database_table".to_string(),
                source_id: table,
                relation: "materialized".to_string(),
            });
        }
        if verdict > 0 {
            provenance.push(ProvenanceEvidence {
                kind: "state_value".to_string(),
                source_id: format!("{}:verdict", scope(run_id)),
                relation: "validated".to_string(),
            });
        }
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "coordination_result".to_string(),
            content: json!({
                "rows": rows,
                "verdict": verdict,
                "child_nudges": child_nudges,
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: "exact_row_count".to_string(),
                    passed: rows == EXPECTED_ROWS,
                    reason: format!("observed {rows} row(s), expected {EXPECTED_ROWS}"),
                },
                CapturedInvariant {
                    id: "accepted_verdict".to_string(),
                    passed: verdict == EXPECTED_ROWS,
                    reason: format!("observed verdict {verdict}, expected {EXPECTED_ROWS}"),
                },
                CapturedInvariant {
                    id: "repair_observed".to_string(),
                    passed: child_nudges >= 1,
                    reason: format!("observed {child_nudges} validation nudge(s)"),
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
            kind: "coordination_result".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["rows", "verdict", "child_nudges"],
                "properties": {
                    "rows": { "type": "integer", "minimum": 0 },
                    "verdict": { "type": "integer", "minimum": 0 },
                    "child_nudges": { "type": "integer", "minimum": 0 }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 4_096,
        }],
        invariants: vec![
            InvariantSpec {
                id: "exact_row_count".to_string(),
                description: "The child produced exactly the expected number of rows.".to_string(),
            },
            InvariantSpec {
                id: "accepted_verdict".to_string(),
                description: "The validator recorded the expected accepted count.".to_string(),
            },
            InvariantSpec {
                id: "repair_observed".to_string(),
                description: "The child transcript proves at least one validation repair."
                    .to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn child_goal_points(goal: bool, rows: u64) -> u8 {
    if goal && rows == EXPECTED_ROWS {
        CHILD_GOAL.weight()
    } else if goal {
        20
    } else {
        0
    }
}

fn orchestration_outcome(ordered: bool, child_nudges: usize) -> (bool, u8) {
    (
        ordered && child_nudges >= 1,
        if ordered {
            ORCHESTRATION_DISCIPLINE.weight()
        } else {
            0
        },
    )
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let table = table(run_id);
        let _: Value = context
            .trigger(
                "database::execute",
                json!({ "db": "primary", "sql": format!("DROP TABLE IF EXISTS {table}") }),
            )
            .await?;
        let _: Value = context
            .trigger(
                "state::delete",
                json!({ "scope": scope(run_id), "key": "verdict" }),
            )
            .await?;
        Ok(())
    })
}

fn table(run_id: &str) -> String {
    format!("subvtest_{}", suffix(run_id))
}

fn scope(run_id: &str) -> String {
    format!("subv-{}", suffix(run_id))
}

/// The suite names the root session `e2e_<run_id>`; the child must live
/// under that prefix for the parent's registration to pass the
/// self-or-own-children scope rule.
fn child_session(run_id: &str) -> String {
    format!("e2e_{run_id}-child-1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_budget_allows_the_declared_validation_repair() {
        let prompt = scenario("run").prompt;
        assert!(prompt.contains("\"max_turns\": 8"));
        assert!(prompt.contains("\"max_validation_retries\": 5"));
    }
}
