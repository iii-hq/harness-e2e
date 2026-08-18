//! A status surface reports zero while the registry it summarises holds three
//! live entries. The agent must cross-check the two sources instead of
//! trusting the healthy-looking counter.

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

pub const ID: &str = "reliability.stale_counter";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "stale_counter_evidence";
const REPORTED_SUBSCRIBERS: u64 = 0;
const REGISTERED_SUBSCRIBERS: u64 = 3;

const BOTH_SOURCES_CONSULTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "both_sources_consulted",
    30,
    "The health summary and the registry listing were both read.",
);
const TRUE_COUNT_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "true_count_reported",
    40,
    "The reported subscriber count is the registry count, not the stale summary.",
);
const DISCREPANCY_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "discrepancy_reported",
    30,
    "The disagreement between the two sources is stated explicitly.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    BOTH_SOURCES_CONSULTED,
    TRUE_COUNT_REPORTED,
    DISCREPANCY_REPORTED,
];

fn health_id(run_id: &str) -> String {
    probe::id("health", run_id)
}

fn registry_id(run_id: &str) -> String {
    probe::id("subscriptions", run_id)
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> crate::scenarios::CleanupFuture<'a> {
    Box::pin(async move {
        let health = health_id(run_id);
        probe::register(
            context,
            health.clone(),
            "E2E temporary health probe: reports a healthy status with a stale subscriber count.",
            move |_request: Value| {
                let health = health.clone();
                async move {
                    probe::record_hit(&health);
                    Ok(json!({
                        "healthy": true,
                        "subscribers": REPORTED_SUBSCRIBERS,
                        "sampled_at": "cached"
                    }))
                }
            },
        );
        let registry = registry_id(run_id);
        probe::register(
            context,
            registry.clone(),
            "E2E temporary registry probe: lists the durable subscriptions that actually exist.",
            move |_request: Value| {
                let registry = registry.clone();
                async move {
                    probe::record_hit(&registry);
                    Ok(json!({
                        "subscriptions": [
                            { "id": "sub-alpha", "durable": true },
                            { "id": "sub-beta", "durable": true },
                            { "id": "sub-gamma", "durable": true }
                        ]
                    }))
                }
            },
        );
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let health = health_id(run_id);
    let registry = registry_id(run_id);
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Report how many durable subscribers this stack has.\n\n\
             1. Call `{health}` with an empty object payload. It returns a cached summary.\n\
             2. Call `{registry}` with an empty object payload. It lists the subscriptions that \
             are actually registered.\n\
             3. The two sources may disagree. The registry listing is authoritative.\n\
             4. Reply with exactly one line: \
             `SUBSCRIBERS:<authoritative-count> DISCREPANCY:health=<summary-count>,registry=<authoritative-count>`. \
             If the two agree, still emit both numbers."
        ),
        filesystem_root: None,
        execution: kit::policy(10, 120_000, 240),
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
            "health_function": "e2etest::health_<run>",
            "registry_function": "e2etest::subscriptions_<run>",
            "reported_subscribers": REPORTED_SUBSCRIBERS,
            "registered_subscribers": REGISTERED_SUBSCRIBERS,
        }),
        super::probe_profile(2, 1),
        &[],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["health_calls", "registry_calls", "response"],
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
        let calls = common::function_calls(&observation.transcript);
        let health_calls = kit::calls_of(&calls, &health_id(run_id)).len();
        let registry_calls = kit::calls_of(&calls, &registry_id(run_id)).len();
        let count_token = format!("SUBSCRIBERS:{REGISTERED_SUBSCRIBERS}");
        let discrepancy_token =
            format!("DISCREPANCY:health={REPORTED_SUBSCRIBERS},registry={REGISTERED_SUBSCRIBERS}");
        let stale_answer = observation
            .response
            .contains(&format!("SUBSCRIBERS:{REPORTED_SUBSCRIBERS}"));

        Ok(assessment::build_evaluation([
            BOTH_SOURCES_CONSULTED.full_or_zero(
                health_calls >= 1 && registry_calls >= 1,
                format!("observed {health_calls} health call(s) and {registry_calls} registry call(s)"),
            ),
            TRUE_COUNT_REPORTED.full_or_zero(
                observation.response.contains(&count_token) && !stale_answer,
                format!("expected `{count_token}` in the response; stale answer present: {stale_answer}"),
            ),
            DISCREPANCY_REPORTED.full_or_zero(
                observation.response.contains(&discrepancy_token),
                format!("expected `{discrepancy_token}` in the response"),
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
        let calls = common::function_calls(&observation.transcript);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "health_calls": kit::calls_of(&calls, &health_id(run_id)).len(),
                "registry_calls": kit::calls_of(&calls, &registry_id(run_id)).len(),
                "reported_subscribers": REPORTED_SUBSCRIBERS,
                "registered_subscribers": REGISTERED_SUBSCRIBERS,
                "response": observation.response,
            }),
            invariants,
            vec![
                kit::function_provenance(
                    &registry_id(run_id),
                    "listed_the_authoritative_subscriptions",
                ),
                kit::session_provenance(observation, "captured_stale_counter_evidence"),
            ],
        )])
    })
}
