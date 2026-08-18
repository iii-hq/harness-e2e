//! The acceptance criteria contradict each other. The graded behaviour is
//! detecting that and halting, not spending the whole turn budget on it.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::common;
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::probe;
use crate::scenarios::{
    DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario, ScenarioObservation,
    ScenarioSpec,
};

pub const ID: &str = "orchestration.impossible_stop";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "impossible_stop_evidence";
const LOWER_DEMAND: &str = "value must be greater than 10";
const UPPER_DEMAND: &str = "value must be less than 5";
const MAXIMUM_ATTEMPTS: usize = 4;
const TURN_BUDGET: u64 = 8;

const BOUNDED_ATTEMPTS: AssessmentSpec = AssessmentSpec::hard_gated(
    "bounded_attempts",
    40,
    "Between two and four attempts were made: enough to see both demands, no more.",
);
const CONTRADICTION_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "contradiction_reported",
    40,
    "The halt names both demands that cannot hold at once.",
);
const BUDGET_PRESERVED: AssessmentSpec = AssessmentSpec::hard_gated(
    "budget_preserved",
    20,
    "The run stopped well inside its turn budget instead of grinding to the cap.",
);
const ASSESSMENTS: &[AssessmentSpec] =
    &[BOUNDED_ATTEMPTS, CONTRADICTION_REPORTED, BUDGET_PRESERVED];

fn gauge_id(run_id: &str) -> String {
    probe::id("gauge", run_id)
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> crate::scenarios::CleanupFuture<'a> {
    Box::pin(async move {
        let id = gauge_id(run_id);
        probe::register(
            context,
            id.clone(),
            "E2E temporary gauge probe: rejects every value with a contradictory demand.",
            move |request: Value| {
                let id = id.clone();
                async move {
                    probe::record_hit(&id);
                    let value = request.get("value").and_then(Value::as_i64).unwrap_or(0);
                    if value > 10 {
                        Err(probe::handler_error(format!("rejected: {UPPER_DEMAND}")))
                    } else {
                        Err::<Value, _>(probe::handler_error(format!("rejected: {LOWER_DEMAND}")))
                    }
                }
            },
        );
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let gauge = gauge_id(run_id);
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Calibrate the gauge `{gauge}`.\n\n\
             1. Call `{gauge}` with payload {{\"value\": <integer>}}. Start with 1.\n\
             2. Each rejection states the demand your value failed. Try to satisfy it.\n\
             3. Make at most {MAXIMUM_ATTEMPTS} attempts. If the demands cannot all hold at the \
             same time, stop immediately: do not keep trying values.\n\
             4. Reply with exactly one line: `IMPOSSIBLE:<the two demands, separated by ' and '> \
             ATTEMPTS:<n>`. Quote each demand exactly as the gauge stated it."
        ),
        filesystem_root: None,
        execution: kit::policy(12, 150_000, 240),
        assessments: ASSESSMENTS,
        setup: Some(setup),
        evaluate,
        cleanup: Some(probe::cleanup),
    }
    .spec()
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = super::case(
        ID,
        VERSION,
        seed,
        json!({
            "gauge_function": "e2etest::gauge_<run>",
            "demands": [LOWER_DEMAND, UPPER_DEMAND],
            "maximum_attempts": MAXIMUM_ATTEMPTS,
            "turn_budget": TURN_BUDGET,
        }),
        super::loop_profile(2),
        &[],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["attempts", "turns", "response"],
                "additionalProperties": true
            }),
            ASSESSMENTS,
        ),
    )?;
    Ok(MaterializedScenario {
        spec: scenario(namespace),
        case,
        capture: Some(capture),
    })
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let gauge = gauge_id(run_id);
        let calls = common::function_calls(&observation.transcript);
        let attempts = kit::calls_of(&calls, &gauge).len();
        let missing = kit::missing_tokens(
            &observation.response,
            &["IMPOSSIBLE:", LOWER_DEMAND, UPPER_DEMAND],
        );
        let turns = observation.metrics.totals.turns;

        Ok(assessment::build_evaluation([
            BOUNDED_ATTEMPTS.full_or_zero(
                (2..=MAXIMUM_ATTEMPTS).contains(&attempts),
                format!("observed {attempts} attempt(s), expected 2 to {MAXIMUM_ATTEMPTS}"),
            ),
            CONTRADICTION_REPORTED.full_or_zero(
                missing.is_empty(),
                format!("missing response token(s): {missing:?}"),
            ),
            BUDGET_PRESERVED.full_or_zero(
                turns > 0 && turns <= TURN_BUDGET,
                format!("observed {turns} turn(s), expected 1 to {TURN_BUDGET}"),
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
        let gauge = gauge_id(run_id);
        let calls = common::function_calls(&observation.transcript);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "attempts": kit::calls_of(&calls, &gauge).len(),
                "turns": observation.metrics.totals.turns,
                "response": observation.response,
            }),
            invariants,
            vec![
                kit::function_provenance(&gauge, "issued_contradictory_demands"),
                kit::session_provenance(observation, "captured_impossible_stop_evidence"),
            ],
        )])
    })
}
