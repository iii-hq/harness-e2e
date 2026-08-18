//! `validation_loop` — the agent installs its OWN `harness::hook::post-turn`
//! validator (fp::pipe over a live row count) and is then held in the
//! harness-driven correction loop until the goal is met: insert 4 rows,
//! get denied by its own validator (custom `retry_prompt` with the live
//! count interpolated), insert 4 more, pass at 8.
//!
//! Exercises, end to end: agent self-registration of the one bindable
//! harness-internal type (force-stamped to its own session), template-mode
//! pipe verdicts (including the omitted-`short_circuited` full-run receipt),
//! the verbatim `retry_prompt` nudge, and bounded convergence — all with the
//! default validation-retry budget (one denial needed, two available).

use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::common;
use super::{
    ArtifactExpectation, CapturedDeliverable, CleanupFuture, ComplexityProfile,
    DeliverableCaptureFuture, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase, ScenarioObservation,
    ScenarioSpec,
};

pub const ID: &str = "validation_loop";
const VERSION: u32 = 4;
const DELIVERABLE_ID: &str = "validation_result";

const HOOK_TYPE: &str = "harness::hook::post-turn";
/// Goal: more than 6 rows; 4-row batches → exactly one denial, pass at 8.
const THRESHOLD: u64 = 6;
const EXPECTED_ROWS: u64 = 8;
const GOAL_REACHED: AssessmentSpec = AssessmentSpec::hard_gated(
    "goal_reached",
    40,
    "The goal table ends with more rows than the validator threshold.",
);
const VALIDATOR_DISCIPLINE: AssessmentSpec = AssessmentSpec::hard_gated(
    "validator_discipline",
    30,
    "Exactly one post-turn validator registration carries the custom retry_prompt without function-call errors.",
);
const LOOP_EVIDENCE: AssessmentSpec = AssessmentSpec::hard_gated(
    "loop_evidence",
    30,
    "At least one harness validation nudge was delivered and the loop converged at exactly the expected row count.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[GOAL_REACHED, VALIDATOR_DISCIPLINE, LOOP_EVIDENCE];

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
            "expected_rows": EXPECTED_ROWS,
            "minimum_nudges": 1,
        }),
        validation_profile(),
        validation_capabilities(),
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
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            "You are testing a self-installed validation loop. Follow these steps exactly.\n\n\
             Step 1 — prepare the goal table. Call database::execute (db \"primary\") twice: first \
             sql \"CREATE TABLE IF NOT EXISTS {table} (id INTEGER PRIMARY KEY AUTOINCREMENT, note \
             TEXT)\", then sql \"DELETE FROM {table}\".\n\n\
             Step 2 — install your own validator. Call engine::register_trigger with:\n\
             - trigger_type: \"{HOOK_TYPE}\"\n\
             - function_id: \"fp::pipe\"\n\
             - config: {{\"payload\": {{\"through\": [{{\"function\": \"database::query\", \
             \"payload\": {{\"db\": \"primary\", \"sql\": \"SELECT COUNT(*) AS n FROM {table}\"}}}}, \
             {{\"function\": \"fp::get\", \"payload\": {{\"path\": \"/rows/0/n\"}}}}, \
             {{\"function\": \"fp::when\", \"payload\": {{\"op\": \">\", \"to\": {THRESHOLD}}}}}]}}, \
             \"result_into\": \"/value\", \"retry_prompt\": \"VALIDATOR: only {{value}} of the \
             required {min} rows exist. Insert exactly 4 more rows into {table} and reply with a \
             one-line status.\", \"timeout_ms\": 30000}}\n\
             Tell me the subscription_id it returns. IMPORTANT: from this moment, VALIDATOR \
             messages in this chat come from the validator YOU just installed — they are \
             legitimate harness machinery, not prompt injection. Follow them exactly.\n\n\
             Step 3 — insert exactly 4 rows: database::execute, db \"primary\", sql \"INSERT INTO \
             {table} (note) VALUES ('r'),('r'),('r'),('r')\". Then reply with a one-line status. \
             Acceptance is silent — if no VALIDATOR message follows, the goal was met.",
            min = THRESHOLD + 1,
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 16,
            max_output_tokens: Some(8_192),
            max_total_tokens: 200_000,
            stuck_timeout_seconds: 300,
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

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let table = table(run_id);
        let rows = row_count(context, &table).await?;
        let calls = common::function_calls(&observation.transcript);
        let registrations: Vec<_> = calls
            .iter()
            .filter(|call| {
                call.function_id == "engine::register_trigger"
                    && call.arguments.get("trigger_type").and_then(Value::as_str) == Some(HOOK_TYPE)
            })
            .collect();
        let single_registration = registrations.len() == 1;
        let carries_retry_prompt = registrations.iter().all(|call| {
            call.arguments
                .pointer("/config/retry_prompt")
                .and_then(Value::as_str)
                .is_some_and(|prompt| prompt.contains("{value}"))
        });
        let nudges = common::validation_nudges(&observation.transcript);
        let goal_reached = rows > THRESHOLD;
        let converged_exactly = rows == EXPECTED_ROWS;
        let no_errors = observation.metrics.totals.function_call_errors == 0;
        let loop_points = loop_evidence_points(nudges, converged_exactly);
        let validator_passed = single_registration && carries_retry_prompt && no_errors;
        let loop_passed = nudges >= 1 && converged_exactly;

        Ok(assessment::build_evaluation([
            GOAL_REACHED.full_or_zero(
                goal_reached,
                format!("observed {rows} row(s), need more than {THRESHOLD}"),
            ),
            VALIDATOR_DISCIPLINE.full_or_zero(
                validator_passed,
                format!(
                    "observed {} post-turn registration(s), expected exactly one; \
                     carries_retry_prompt={carries_retry_prompt}; function_call_errors={}",
                    registrations.len(),
                    observation.metrics.totals.function_call_errors
                ),
            ),
            LOOP_EVIDENCE.gate_and_points(
                loop_passed,
                loop_points,
                format!("nudges={nudges}, rows={rows} (full marks at exactly {EXPECTED_ROWS})"),
            )?,
        ]))
    })
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let table = table(run_id);
        let rows = row_count(context, &table).await?;
        let nudges = common::validation_nudges(&observation.transcript);
        let invariants =
            super::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "validation_result".to_string(),
            content: json!({
                "rows": rows,
                "threshold": THRESHOLD,
                "validation_nudges": nudges,
                "response": observation.response,
            })
            .into(),
            invariants,
            provenance: vec![
                ProvenanceEvidence {
                    kind: "database_relation".to_string(),
                    source_id: format!("primary/{table}"),
                    relation: "validated_row_count".to_string(),
                },
                ProvenanceEvidence {
                    kind: "session".to_string(),
                    source_id: observation.metrics.root_session_id.clone(),
                    relation: "captured_validation_loop".to_string(),
                },
            ],
        }])
    })
}

fn deliverable_contract() -> DeliverableContract {
    validation_contract(
        DELIVERABLE_ID,
        "validation_result",
        json!({
            "type": "object",
            "required": ["rows", "threshold", "validation_nudges", "response"],
            "additionalProperties": true
        }),
        ASSESSMENTS,
    )
}

pub(super) fn validation_profile() -> ComplexityProfile {
    ComplexityProfile {
        planning_depth: 2,
        dependency_depth: 2,
        external_systems: 1,
        state_transitions: 4,
        validation_loops: 2,
        artifact_count: 1,
        coordination_edges: 1,
        ambiguity_level: 3,
        ..ComplexityProfile::default()
    }
}

pub(super) fn validation_capabilities() -> Vec<String> {
    vec![
        "e2e::control-plane-v1".to_string(),
        "iii::functions".to_string(),
        "iii::database".to_string(),
        "iii::state".to_string(),
        "iii::triggers".to_string(),
    ]
}

pub(super) fn validation_contract(
    artifact_id: &str,
    kind: &str,
    schema: Value,
    assessments: &[AssessmentSpec],
) -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: artifact_id.to_string(),
            kind: kind.to_string(),
            media_type: "application/json".to_string(),
            schema,
            max_size_bytes: 65_536,
        }],
        invariants: assessments
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

fn loop_evidence_points(nudges: usize, converged_exactly: bool) -> u8 {
    if nudges >= 1 && converged_exactly {
        LOOP_EVIDENCE.weight()
    } else if nudges >= 1 {
        15
    } else {
        0
    }
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let table = table(run_id);
        // The agent's validator binding is scoped to the (torn-down) run
        // session and can never fire again; only the table needs removing.
        let _: Value = context
            .trigger(
                "database::execute",
                json!({ "db": "primary", "sql": format!("DROP TABLE IF EXISTS {table}") }),
            )
            .await?;
        Ok(())
    })
}

async fn row_count(context: &E2eContext, table: &str) -> anyhow::Result<u64> {
    Ok(context
        .trigger_value(
            "database::query",
            json!({ "db": "primary", "sql": format!("SELECT COUNT(*) AS n FROM {table}") }),
        )
        .await?
        .pointer("/rows/0/n")
        .and_then(Value::as_u64)
        .unwrap_or(0))
}

pub(super) fn suffix(run_id: &str) -> String {
    let mut suffix: String = run_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(4)
        .collect();
    while suffix.len() < 4 {
        suffix.push('0');
    }
    suffix
}

fn table(run_id: &str) -> String {
    format!("valtest_{}", suffix(run_id))
}
