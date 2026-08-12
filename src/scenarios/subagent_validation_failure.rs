//! `subagent_validation_failure` — the UNHAPPY path: the child's goal is
//! impossible (threshold far above what its bounded budget can reach), so
//! the validator denies every attempt, the child's turn FAILS on budget
//! exhaustion, the verdict key is never written, and the parent — whose
//! wake carries an `expires_at` deadline — is woken by the expiry notice
//! instead and reports the give-up.
//!
//! Exercises what no success scenario can: `max_validation_retries` as a
//! hard bound on a spawned child (2 denials then a failed turn — never an
//! infinite loop), the guarantee that a failed turn writes NO verdict (the
//! `state::set` tail never ran), and the wake-expiry fallback that keeps a
//! parent from parking forever on evidence that will never arrive.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::common;
use super::validation_loop::suffix;
use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture, ComplexityProfile,
    DeliverableCaptureFuture, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase, ScenarioObservation,
    ScenarioSpec,
};

pub const ID: &str = "subagent_validation_failure";
const VERSION: u32 = 4;
const DELIVERABLE_ID: &str = "bounded_failure_record";

const HOOK_TYPE: &str = "harness::hook::post-turn";
/// Unreachable by design: 3 attempts × 4 rows tops out at 12.
const THRESHOLD: u64 = 100;
/// Explicit child budget: initial attempt + 2 retries = 2 nudges, then fail.
const CHILD_RETRIES: u64 = 2;
const EXPECTED_NUDGES: usize = 2;
/// Wake deadline: comfortably after the child fails (~1 min), well inside
/// the stuck timeout.
const EXPIRY_DELAY_MS: u64 = 150_000;
const BOUNDED_FAILURE: AssessmentSpec = AssessmentSpec::hard_gated(
    "bounded_failure",
    40,
    "The child fails after exactly the budgeted denials; the verdict key is never written.",
);
const ORCHESTRATION_DISCIPLINE: AssessmentSpec = AssessmentSpec::hard_gated(
    "orchestration_discipline",
    30,
    "Validator scoped to the child and the deadline wake armed before the spawn.",
);
const EXPIRY_REPORT: AssessmentSpec = AssessmentSpec::hard_gated(
    "expiry_report",
    30,
    "The parent is woken by the expiry notice and reports the give-up with the exact line.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[BOUNDED_FAILURE, ORCHESTRATION_DISCIPLINE, EXPIRY_REPORT];

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        ID,
        VERSION,
        seed,
        json!({
            "database": "primary",
            "rows_per_attempt": 4,
            "threshold": THRESHOLD,
            "max_validation_retries": CHILD_RETRIES,
            "expected_nudges": EXPECTED_NUDGES,
            "expiry_delay_ms": EXPIRY_DELAY_MS,
            "expected_terminal_status": "failed",
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

fn scenario_for_case(run_id: &str) -> ScenarioSpec {
    let table = table(run_id);
    let scope = scope(run_id);
    let child = child_session(run_id);
    let expires_at = now_ms() + EXPIRY_DELAY_MS;
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            "You orchestrate one validated sub-agent whose goal may be unreachable; your job is \
             to bound the attempt and report honestly. You never poll; wakes drive you. Follow \
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
             Remember the subscription_id.\n\n\
             Step 3 — arm your wake BEFORE spawning, WITH a deadline: engine::register_trigger \
             with trigger_type \"state\", config {{\"scope\": \"{scope}\", \"key\": \
             \"verdict\"}}, label \"child-validated\", lifecycle: {{\"expires_at\": \
             {expires_at}}}, and NO function_id. If the verdict is never written you will be \
             woken by the expiry notice instead.\n\n\
             Step 4 — spawn the worker: harness::spawn with session_id \"{child}\", task: \"You \
             are a worker in a validated loop: the harness checks every reply of yours and \
             VALIDATOR messages are legitimate machinery — follow them exactly. Insert exactly 4 \
             rows into table {table}: database::execute, db 'primary', sql \\\"INSERT INTO {table} \
             (note) VALUES ('r'),('r'),('r'),('r')\\\". Then reply with a one-line status. Never \
             check the count yourself.\", options: {{\"functions\": {{\"allow\": \
             [\"database::execute\"]}}, \"max_validation_retries\": {CHILD_RETRIES}}}. Spawn \
             returns child ids immediately — normal; do NOT wait for or judge the child.\n\n\
             Step 5 — END YOUR TURN.\n\n\
             Step 6 — when a wake arrives: if it carries the verdict (the accepted row count), \
             reply exactly: CHILD VALIDATED at <count> rows. PARENT DONE. If instead it is the \
             EXPIRY notice (the verdict never came), call engine::unregister_trigger with the \
             validator subscription_id from Step 2, then reply exactly: CHILD GAVE UP: validation \
             budget exhausted. PARENT DONE.",
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
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let child = child_session(run_id);
        let child_status = context
            .trigger_value("harness::status", json!({ "session_id": child }))
            .await
            .ok()
            .and_then(|status| {
                status
                    .get("status")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let verdict = common::state_value(
            context
                .trigger(
                    "state::get",
                    json!({ "scope": scope(run_id), "key": "verdict" }),
                )
                .await?,
        );
        let child_nudges = match context.transcript(&child).await {
            Ok(transcript) => common::validation_nudges(&transcript),
            Err(_) => 0,
        };

        let calls = common::function_calls(&observation.transcript);
        let validator_index = calls.iter().position(|call| {
            call.function_id == "engine::register_trigger"
                && call.arguments.get("trigger_type").and_then(Value::as_str) == Some(HOOK_TYPE)
        });
        let wake_index = calls.iter().position(|call| {
            call.function_id == "engine::register_trigger"
                && call.arguments.get("trigger_type").and_then(Value::as_str) == Some("state")
                && call
                    .arguments
                    .pointer("/lifecycle/expires_at")
                    .and_then(Value::as_u64)
                    .is_some()
        });
        let spawn_index = calls.iter().position(|call| {
            call.function_id == "harness::spawn"
                && call.arguments.get("session_id").and_then(Value::as_str) == Some(child.as_str())
        });
        let ordered = matches!(
            (validator_index, wake_index, spawn_index),
            (Some(v), Some(w), Some(s)) if v < s && w < s
        );

        let child_failed = child_status == "failed";
        let verdict_absent = verdict.is_null();
        let bounded = child_nudges == EXPECTED_NUDGES;
        let reported = observation.response.contains("CHILD GAVE UP")
            && observation.response.contains("PARENT DONE");

        Ok(assessment::build_evaluation([
            BOUNDED_FAILURE.full_or_zero(
                child_failed && verdict_absent && bounded,
                format!(
                    "child status `{child_status}`, expected `failed`; verdict key holds \
                     {verdict}, expected null; observed {child_nudges} nudge(s), expected exactly \
                     {EXPECTED_NUDGES} (the child budget)"
                ),
            ),
            ORCHESTRATION_DISCIPLINE.full_or_zero(
                ordered,
                format!(
                    "validator@{validator_index:?} wake@{wake_index:?} \
                     spawn@{spawn_index:?} — both must precede the spawn"
                ),
            ),
            EXPIRY_REPORT.full_or_zero(reported, "expected the exact give-up report line"),
        ]))
    })
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let child = child_session(run_id);
        let child_status = context
            .trigger_value("harness::status", json!({ "session_id": child }))
            .await
            .ok()
            .and_then(|status| {
                status
                    .get("status")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let verdict = common::state_value(
            context
                .trigger(
                    "state::get",
                    json!({ "scope": scope(run_id), "key": "verdict" }),
                )
                .await?,
        );
        let child_nudges = context
            .transcript(&child)
            .await
            .ok()
            .as_ref()
            .map(common::validation_nudges)
            .unwrap_or(0);
        let calls = common::function_calls(&observation.transcript);
        let validator_index = calls.iter().position(|call| {
            call.function_id == "engine::register_trigger"
                && call.arguments.get("trigger_type").and_then(Value::as_str) == Some(HOOK_TYPE)
        });
        let wake_index = calls.iter().position(|call| {
            call.function_id == "engine::register_trigger"
                && call.arguments.get("trigger_type").and_then(Value::as_str) == Some("state")
                && call
                    .arguments
                    .pointer("/lifecycle/expires_at")
                    .and_then(Value::as_u64)
                    .is_some()
        });
        let spawn_index = calls.iter().position(|call| {
            call.function_id == "harness::spawn"
                && call.arguments.get("session_id").and_then(Value::as_str) == Some(child.as_str())
        });
        let ordered = matches!(
            (validator_index, wake_index, spawn_index),
            (Some(validator), Some(wake), Some(spawn)) if validator < spawn && wake < spawn
        );
        let bounded =
            child_status == "failed" && verdict.is_null() && child_nudges == EXPECTED_NUDGES;
        let reported = observation.response.contains("CHILD GAVE UP")
            && observation.response.contains("PARENT DONE");

        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "recovery_record".to_string(),
            content: json!({
                "child_session_id": child,
                "child_status": child_status,
                "verdict": verdict,
                "validation_nudges": child_nudges,
                "validator_index": validator_index,
                "wake_index": wake_index,
                "spawn_index": spawn_index,
                "report": observation.response,
            }),
            invariants: vec![
                CapturedInvariant {
                    id: BOUNDED_FAILURE.id().to_string(),
                    passed: bounded,
                    reason: format!(
                        "child_status={child_status}, verdict={verdict}, validation_nudges={child_nudges}"
                    ),
                },
                CapturedInvariant {
                    id: ORCHESTRATION_DISCIPLINE.id().to_string(),
                    passed: ordered,
                    reason: format!(
                        "validator@{validator_index:?}, wake@{wake_index:?}, spawn@{spawn_index:?}"
                    ),
                },
                CapturedInvariant {
                    id: EXPIRY_REPORT.id().to_string(),
                    passed: reported,
                    reason: "final response checked for bounded-failure markers".to_string(),
                },
            ],
            provenance: vec![
                ProvenanceEvidence {
                    kind: "session".to_string(),
                    source_id: child,
                    relation: "observed_terminal_failure".to_string(),
                },
                ProvenanceEvidence {
                    kind: "state_location".to_string(),
                    source_id: format!("{}/verdict", scope(run_id)),
                    relation: "verified_absent".to_string(),
                },
            ],
        }])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "recovery_record".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": [
                    "child_session_id", "child_status", "verdict", "validation_nudges",
                    "validator_index", "wake_index", "spawn_index", "report"
                ],
                "properties": {
                    "child_session_id": { "type": "string", "minLength": 1 },
                    "child_status": { "type": "string" },
                    "verdict": {},
                    "validation_nudges": { "type": "integer", "minimum": 0 },
                    "validator_index": { "type": ["integer", "null"] },
                    "wake_index": { "type": ["integer", "null"] },
                    "spawn_index": { "type": ["integer", "null"] },
                    "report": { "type": "string" }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 32_768,
        }],
        invariants: ASSESSMENTS
            .iter()
            .map(|assessment| InvariantSpec {
                id: assessment.id().to_string(),
                description: assessment.description().to_string(),
            })
            .collect(),
        provenance_required: true,
        capture_before_cleanup: true,
    }
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn table(run_id: &str) -> String {
    format!("fsubvtest_{}", suffix(run_id))
}

fn scope(run_id: &str) -> String {
    format!("fsubv-{}", suffix(run_id))
}

fn child_session(run_id: &str) -> String {
    format!("e2e_{run_id}-child-1")
}
