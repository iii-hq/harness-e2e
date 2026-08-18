//! Redelivery is normal. Applying the same keyed operation twice must leave
//! the same total, and the agent must verify that rather than assume it.

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

pub const ID: &str = "reliability.idempotent_apply";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "idempotent_apply_evidence";
const AMOUNT: u64 = 5;
const IDEMPOTENCY_KEY: &str = "E2E-APPLY-1";

const DUPLICATE_SUPPRESSED: AssessmentSpec = AssessmentSpec::hard_gated(
    "duplicate_suppressed",
    40,
    "The ledger applied the keyed amount exactly once across both deliveries.",
);
const BOTH_DELIVERIES_SENT: AssessmentSpec = AssessmentSpec::hard_gated(
    "both_deliveries_sent",
    30,
    "Both deliveries carried the same idempotency key, as instructed.",
);
const OUTCOME_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "outcome_reported",
    30,
    "The final response states the resulting total and that the duplicate was suppressed.",
);
const ASSESSMENTS: &[AssessmentSpec] =
    &[DUPLICATE_SUPPRESSED, BOTH_DELIVERIES_SENT, OUTCOME_REPORTED];

fn apply_id(run_id: &str) -> String {
    probe::id("apply", run_id)
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> crate::scenarios::CleanupFuture<'a> {
    Box::pin(async move {
        let id = apply_id(run_id);
        probe::register(
            context,
            id.clone(),
            "E2E temporary ledger probe: applies an amount once per idempotency key.",
            move |request: Value| {
                let id = id.clone();
                async move {
                    probe::record_hit(&id);
                    let key = request
                        .get("key")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let amount = request.get("amount").and_then(Value::as_u64).unwrap_or(0);
                    if key.trim().is_empty() {
                        return Err(probe::handler_error("key is required"));
                    }
                    let ledger = probe::ledger(&id);
                    let mut entries = ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let total = entries.get("total").and_then(Value::as_u64).unwrap_or(0);
                    let seen = entries.contains_key(&format!("key:{key}"));
                    let total = if seen { total } else { total + amount };
                    entries.insert(format!("key:{key}"), Value::Bool(true));
                    entries.insert("total".into(), Value::from(total));
                    let applications = entries
                        .get("applications")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let applications = if seen { applications } else { applications + 1 };
                    entries.insert("applications".into(), Value::from(applications));
                    Ok(json!({ "applied": !seen, "total": total }))
                }
            },
        );
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let apply = apply_id(run_id);
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "A queue can deliver the same message twice. Prove the ledger tolerates it.\n\n\
             1. Call `{apply}` with payload {{\"key\": \"{IDEMPOTENCY_KEY}\", \"amount\": {AMOUNT}}}.\n\
             2. Call `{apply}` a second time with the exact same payload, simulating a \
             redelivery. Do not change the key.\n\
             3. Read the `total` and `applied` fields both calls returned.\n\
             4. Reply with exactly one line: `TOTAL:<total> DUPLICATE_SUPPRESSED:<true|false>` \
             where the second value is true when the redelivery did not change the total."
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
            "apply_function": "e2etest::apply_<run>",
            "idempotency_key": IDEMPOTENCY_KEY,
            "amount": AMOUNT,
            "expected_total": AMOUNT,
        }),
        super::probe_profile(1, 2),
        &[],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["deliveries", "total", "applications", "response"],
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

fn keyed_deliveries(calls: &[common::ObservedFunctionCall], apply: &str) -> usize {
    calls
        .iter()
        .filter(|call| {
            call.function_id == apply
                && call.arguments.get("key").and_then(Value::as_str) == Some(IDEMPOTENCY_KEY)
        })
        .count()
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let apply = apply_id(run_id);
        let calls = common::function_calls(&observation.transcript);
        let deliveries = keyed_deliveries(&calls, &apply);
        let total = probe::ledger_u64(&apply, "total");
        let applications = probe::ledger_u64(&apply, "applications");
        let total_token = format!("TOTAL:{AMOUNT}");
        let missing = kit::missing_tokens(
            &observation.response,
            &[total_token.as_str(), "DUPLICATE_SUPPRESSED:true"],
        );

        Ok(assessment::build_evaluation([
            DUPLICATE_SUPPRESSED.full_or_zero(
                total == AMOUNT && applications == 1,
                format!("ledger total={total} (expected {AMOUNT}); applications={applications} (expected 1)"),
            ),
            BOTH_DELIVERIES_SENT.full_or_zero(
                deliveries == 2,
                format!("observed {deliveries} keyed delivery(ies), expected exactly 2"),
            ),
            OUTCOME_REPORTED.full_or_zero(
                missing.is_empty(),
                format!("missing response token(s): {missing:?}"),
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
        let apply = apply_id(run_id);
        let calls = common::function_calls(&observation.transcript);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "deliveries": keyed_deliveries(&calls, &apply),
                "total": probe::ledger_u64(&apply, "total"),
                "applications": probe::ledger_u64(&apply, "applications"),
                "response": observation.response,
            }),
            invariants,
            vec![
                kit::function_provenance(&apply, "deduplicated_the_redelivery"),
                kit::session_provenance(observation, "captured_idempotent_apply_evidence"),
            ],
        )])
    })
}
