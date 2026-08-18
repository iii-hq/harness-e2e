//! A counted loop with real per-iteration work. Overrunning and stopping
//! short both fail; the accumulated result proves each iteration happened.

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

pub const ID: &str = "orchestration.exact_iteration_budget";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "exact_iteration_budget_evidence";
const ITERATIONS: u32 = 5;
const STEP: u32 = 3;
const EXPECTED_SUM: u32 = STEP * ITERATIONS * (ITERATIONS + 1) / 2;

const EXACT_ITERATIONS: AssessmentSpec = AssessmentSpec::hard_gated(
    "exact_iterations",
    45,
    "The loop ran exactly the requested number of iterations.",
);
const SUM_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "sum_reported",
    35,
    "The reported total is the sum of the values the iterations returned.",
);
const CLEAN_COMPLETION: AssessmentSpec = AssessmentSpec::hard_gated(
    "clean_completion",
    20,
    "The loop finished without function-call errors.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[EXACT_ITERATIONS, SUM_REPORTED, CLEAN_COMPLETION];

fn tick_id(run_id: &str) -> String {
    probe::id("tick", run_id)
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> crate::scenarios::CleanupFuture<'a> {
    Box::pin(async move {
        let id = tick_id(run_id);
        probe::register(
            context,
            id.clone(),
            "E2E temporary tick probe: returns a value that grows with each call.",
            move |_request: Value| {
                let id = id.clone();
                async move {
                    let hit = probe::record_hit(&id);
                    Ok(json!({ "iteration": hit, "value": hit * STEP }))
                }
            },
        );
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let tick = tick_id(run_id);
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Run a counted loop against `{tick}`.\n\n\
             1. Call `{tick}` with an empty object payload exactly {ITERATIONS} times. Not four, \
             not six.\n\
             2. Each call returns a `value`. Add them up as you go.\n\
             3. Reply with exactly one line: `TICKS:{ITERATIONS} SUM:<total>` where the total is \
             the sum of the values you actually received."
        ),
        filesystem_root: None,
        execution: kit::policy(14, 160_000, 600),
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
            "tick_function": "e2etest::tick_<run>",
            "iterations": ITERATIONS,
            "step": STEP,
            "expected_sum": EXPECTED_SUM,
        }),
        super::loop_profile(1),
        &[],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["iterations", "probe_hits", "response"],
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
        let tick = tick_id(run_id);
        let calls = common::function_calls(&observation.transcript);
        let iterations = kit::calls_of(&calls, &tick).len();
        let probe_hits = probe::hits(&tick);
        let expected = usize::try_from(ITERATIONS).unwrap_or(usize::MAX);
        let missing = kit::missing_tokens(
            &observation.response,
            &[
                format!("TICKS:{ITERATIONS}").as_str(),
                format!("SUM:{EXPECTED_SUM}").as_str(),
            ],
        );

        Ok(assessment::build_evaluation([
            EXACT_ITERATIONS.full_or_zero(
                iterations == expected && probe_hits == ITERATIONS,
                format!(
                    "observed {iterations} transcript call(s) and {probe_hits} probe hit(s), expected {ITERATIONS} of each"
                ),
            ),
            SUM_REPORTED.full_or_zero(
                missing.is_empty(),
                format!("missing response token(s): {missing:?}"),
            ),
            CLEAN_COMPLETION.full_or_zero(
                observation.metrics.totals.function_call_errors == 0,
                format!(
                    "observed {} function-call error(s), expected none",
                    observation.metrics.totals.function_call_errors
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
        let tick = tick_id(run_id);
        let calls = common::function_calls(&observation.transcript);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "iterations": kit::calls_of(&calls, &tick).len(),
                "probe_hits": probe::hits(&tick),
                "expected_sum": EXPECTED_SUM,
                "response": observation.response,
            }),
            invariants,
            vec![
                kit::function_provenance(&tick, "served_each_iteration"),
                kit::session_provenance(observation, "captured_exact_iteration_budget_evidence"),
            ],
        )])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_expected_sum_matches_the_probe_sequence() {
        let observed: u32 = (1..=ITERATIONS).map(|hit| hit * STEP).sum();
        assert_eq!(observed, EXPECTED_SUM);
    }
}
