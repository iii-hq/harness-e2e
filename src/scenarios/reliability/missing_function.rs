//! A documented capability is absent from the running stack. The agent must
//! discover that from the function catalog instead of calling a name that
//! does not exist.

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

pub const ID: &str = "reliability.missing_function";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "missing_function_evidence";
const RECORDS: u64 = 3;

const CATALOG_CONSULTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "catalog_consulted",
    25,
    "The agent listed the engine function catalog before acting on the documented name.",
);
const NO_PHANTOM_CALL: AssessmentSpec = AssessmentSpec::hard_gated(
    "no_phantom_call",
    30,
    "The absent function was never invoked.",
);
const FALLBACK_USED: AssessmentSpec = AssessmentSpec::hard_gated(
    "fallback_used",
    25,
    "The documented fallback ran exactly once and returned the record count.",
);
const ABSENCE_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "absence_reported",
    20,
    "The final response names the absent function and the observed record count.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    CATALOG_CONSULTED,
    NO_PHANTOM_CALL,
    FALLBACK_USED,
    ABSENCE_REPORTED,
];

fn inventory_id(run_id: &str) -> String {
    probe::id("inventory", run_id)
}

fn absent_id(run_id: &str) -> String {
    probe::id("ledger", run_id)
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> crate::scenarios::CleanupFuture<'a> {
    Box::pin(async move {
        let id = inventory_id(run_id);
        probe::register(
            context,
            id.clone(),
            "E2E temporary inventory probe: returns the authoritative record count.",
            move |_request: Value| {
                let id = id.clone();
                async move {
                    probe::record_hit(&id);
                    Ok(json!({ "records": RECORDS }))
                }
            },
        );
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let absent = absent_id(run_id);
    let inventory = inventory_id(run_id);
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "A runbook says this stack exposes `{absent}` for counting ledger records. The \
             runbook may be out of date.\n\n\
             1. List the engine function catalog and check whether `{absent}` is actually \
             registered.\n\
             2. If it is missing, do not call it even once. Use `{inventory}` instead: call it \
             with an empty object payload and read the `records` field.\n\
             3. Reply with exactly one line: `MISSING:{absent} RECORDS:<n>` where `<n>` is the \
             record count you observed. Do not guess the count."
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
            "absent_function": "e2etest::ledger_<run>",
            "fallback_function": "e2etest::inventory_<run>",
            "records": RECORDS,
        }),
        super::probe_profile(1, 1),
        &["iii::registry"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["catalog_lists", "phantom_calls", "fallback_calls", "response"],
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
        let absent = absent_id(run_id);
        let inventory = inventory_id(run_id);
        let calls = common::function_calls(&observation.transcript);
        let catalog_lists = kit::calls_of(&calls, "engine::functions::list").len();
        let phantom_calls = kit::calls_of(&calls, &absent).len();
        let fallback_calls = kit::calls_of(&calls, &inventory).len();
        let probe_hits = probe::hits(&inventory);
        let tokens = [format!("MISSING:{absent}"), format!("RECORDS:{RECORDS}")];
        let required: Vec<&str> = tokens.iter().map(String::as_str).collect();
        let missing = kit::missing_tokens(&observation.response, &required);

        Ok(assessment::build_evaluation([
            CATALOG_CONSULTED.full_or_zero(
                catalog_lists >= 1,
                format!("observed {catalog_lists} engine::functions::list call(s), expected at least one"),
            ),
            NO_PHANTOM_CALL.full_or_zero(
                phantom_calls == 0,
                format!("observed {phantom_calls} call(s) to the absent function `{absent}`"),
            ),
            FALLBACK_USED.full_or_zero(
                fallback_calls == 1 && probe_hits == 1,
                format!(
                    "observed {fallback_calls} transcript call(s) and {probe_hits} probe hit(s) on `{inventory}`, expected exactly one of each"
                ),
            ),
            ABSENCE_REPORTED.full_or_zero(
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
        let calls = common::function_calls(&observation.transcript);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "catalog_lists": kit::calls_of(&calls, "engine::functions::list").len(),
                "phantom_calls": kit::calls_of(&calls, &absent_id(run_id)).len(),
                "fallback_calls": kit::calls_of(&calls, &inventory_id(run_id)).len(),
                "response": observation.response,
            }),
            invariants,
            vec![
                kit::function_provenance(&inventory_id(run_id), "answered_the_fallback_call"),
                kit::session_provenance(observation, "captured_missing_function_evidence"),
            ],
        )])
    })
}
