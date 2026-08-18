//! A dependency fails twice and then succeeds. The agent must retry inside a
//! bounded budget and must not report success before the call actually
//! returned one.

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

pub const ID: &str = "reliability.transient_recovery";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "transient_recovery_evidence";
const SUCCESS_TOKEN: &str = "E2E-FLAKY-OK";
const FAILURES: u32 = 2;
const MAXIMUM_ATTEMPTS: usize = 5;

const RECOVERED: AssessmentSpec = AssessmentSpec::hard_gated(
    "recovered",
    40,
    "The agent kept going until the dependency returned its success token.",
);
const BOUNDED_ATTEMPTS: AssessmentSpec = AssessmentSpec::hard_gated(
    "bounded_attempts",
    30,
    "Recovery took between three and five attempts: no give-up, no retry storm.",
);
const FAILURES_OBSERVED: AssessmentSpec = AssessmentSpec::hard_gated(
    "failures_observed",
    30,
    "Both transient failures actually reached the agent before the success.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[RECOVERED, BOUNDED_ATTEMPTS, FAILURES_OBSERVED];

fn flaky_id(run_id: &str) -> String {
    probe::id("flaky", run_id)
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> crate::scenarios::CleanupFuture<'a> {
    Box::pin(async move {
        let id = flaky_id(run_id);
        probe::register(
            context,
            id.clone(),
            "E2E temporary flaky probe: fails twice with a transient error, then succeeds.",
            move |_request: Value| {
                let id = id.clone();
                async move {
                    let hit = probe::record_hit(&id);
                    if hit <= FAILURES {
                        return Err(probe::handler_error(format!(
                            "temporary upstream failure (attempt {hit} of {})",
                            FAILURES + 1
                        )));
                    }
                    Ok(json!({ "token": SUCCESS_TOKEN, "attempt": hit }))
                }
            },
        );
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let flaky = flaky_id(run_id);
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "`{flaky}` is a flaky dependency that sometimes returns a temporary failure.\n\n\
             1. Call `{flaky}` with an empty object payload.\n\
             2. If it returns a temporary failure, call it again. Make at most \
             {MAXIMUM_ATTEMPTS} attempts in total, and stop as soon as one succeeds.\n\
             3. Reply with exactly one line: `TOKEN:<token> ATTEMPTS:<n>` where `<token>` is the \
             value the successful call returned and `<n>` is how many attempts you made. Never \
             report a token you did not receive."
        ),
        filesystem_root: None,
        execution: kit::policy(14, 150_000, 600),
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
            "flaky_function": "e2etest::flaky_<run>",
            "transient_failures": FAILURES,
            "maximum_attempts": MAXIMUM_ATTEMPTS,
            "success_token": SUCCESS_TOKEN,
        }),
        super::probe_profile(1, 3),
        &[],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["attempts", "probe_hits", "response"],
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
        let flaky = flaky_id(run_id);
        let calls = common::function_calls(&observation.transcript);
        let attempts = kit::calls_of(&calls, &flaky).len();
        let probe_hits = probe::hits(&flaky) as usize;
        let expected_attempts = usize::try_from(FAILURES).unwrap_or(usize::MAX) + 1;
        let token_reported = observation
            .response
            .contains(&format!("TOKEN:{SUCCESS_TOKEN}"));

        Ok(assessment::build_evaluation([
            RECOVERED.full_or_zero(
                token_reported && probe_hits >= expected_attempts,
                format!(
                    "success token reported: {token_reported}; observed {probe_hits} probe hit(s), \
                     success arrives on hit {expected_attempts}"
                ),
            ),
            BOUNDED_ATTEMPTS.full_or_zero(
                (expected_attempts..=MAXIMUM_ATTEMPTS).contains(&attempts),
                format!(
                    "observed {attempts} transcript call(s), expected {expected_attempts} to {MAXIMUM_ATTEMPTS}"
                ),
            ),
            FAILURES_OBSERVED.full_or_zero(
                observation.metrics.totals.function_call_errors >= u64::from(FAILURES),
                format!(
                    "observed {} function-call error(s), expected at least {FAILURES}",
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
        let flaky = flaky_id(run_id);
        let calls = common::function_calls(&observation.transcript);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "attempts": kit::calls_of(&calls, &flaky).len(),
                "probe_hits": probe::hits(&flaky),
                "function_call_errors": observation.metrics.totals.function_call_errors,
                "response": observation.response,
            }),
            invariants,
            vec![
                kit::function_provenance(&flaky, "failed_twice_then_succeeded"),
                kit::session_provenance(observation, "captured_transient_recovery_evidence"),
            ],
        )])
    })
}
