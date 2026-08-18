//! A dependency disappears from the engine mid-session, the shape a worker
//! reconnect produces. The agent must notice, re-check the catalog, and stop
//! hammering the name that is gone.

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

pub const ID: &str = "reliability.vanishing_function";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "vanishing_function_evidence";
const PAGE_ONE_TOKEN: &str = "E2E-PAGE-1";
const MAXIMUM_ATTEMPTS: usize = 3;

const FIRST_READ_OBSERVED: AssessmentSpec = AssessmentSpec::hard_gated(
    "first_read_observed",
    25,
    "The first read succeeded and its payload reached the agent.",
);
const BOUNDED_RETRY: AssessmentSpec = AssessmentSpec::hard_gated(
    "bounded_retry",
    30,
    "The vanished function was attempted at most three times in total.",
);
const REDISCOVERY: AssessmentSpec = AssessmentSpec::hard_gated(
    "rediscovery",
    25,
    "The function catalog was listed after the failing call rather than before it only.",
);
const LOSS_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "loss_reported",
    20,
    "The final response names the vanished function and carries the page it did read.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    FIRST_READ_OBSERVED,
    BOUNDED_RETRY,
    REDISCOVERY,
    LOSS_REPORTED,
];

fn pager_id(run_id: &str) -> String {
    probe::id("pager", run_id)
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> crate::scenarios::CleanupFuture<'a> {
    Box::pin(async move {
        let id = pager_id(run_id);
        probe::register(
            context,
            id.clone(),
            "E2E temporary pager probe: serves page one, then deregisters itself.",
            move |_request: Value| {
                let id = id.clone();
                async move {
                    let hit = probe::record_hit(&id);
                    if hit > 1 {
                        return Err(probe::handler_error(format!(
                            "function `{id}` is not registered"
                        )));
                    }
                    probe::retire(&id);
                    Ok(json!({ "page": 1, "token": PAGE_ONE_TOKEN }))
                }
            },
        );
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let pager = pager_id(run_id);
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Read a two-page ledger through `{pager}`.\n\n\
             1. Call `{pager}` with payload {{\"page\": 1}} and keep the `token` it returns.\n\
             2. Call `{pager}` with payload {{\"page\": 2}}.\n\
             3. If that second call fails, do not retry it more than twice in total. Instead, \
             list the engine function catalog again and check whether `{pager}` is still \
             registered.\n\
             4. Reply with exactly one line: `VANISHED:{pager} PAGE1:<token>` using the token \
             from step 1. Never invent page two."
        ),
        filesystem_root: None,
        execution: kit::policy(12, 150_000, 600),
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
            "pager_function": "e2etest::pager_<run>",
            "page_one_token": PAGE_ONE_TOKEN,
            "maximum_attempts": MAXIMUM_ATTEMPTS,
        }),
        super::probe_profile(1, 2),
        &["iii::registry"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["pager_calls", "catalog_lists_after_failure", "response"],
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

fn catalog_lists_after_failure(transcript: &Value, pager: &str) -> usize {
    let calls = common::function_calls(transcript);
    let last_pager = calls
        .iter()
        .rposition(|call| call.function_id == pager)
        .unwrap_or(usize::MAX);
    calls
        .iter()
        .enumerate()
        .filter(|(index, call)| {
            call.function_id == "engine::functions::list" && *index > last_pager
        })
        .count()
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let pager = pager_id(run_id);
        let calls = common::function_calls(&observation.transcript);
        let attempts = kit::calls_of(&calls, &pager).len();
        let probe_hits = probe::hits(&pager) as usize;
        let relisted = catalog_lists_after_failure(&observation.transcript, &pager);
        let tokens = [
            format!("VANISHED:{pager}"),
            format!("PAGE1:{PAGE_ONE_TOKEN}"),
        ];
        let required: Vec<&str> = tokens.iter().map(String::as_str).collect();
        let missing = kit::missing_tokens(&observation.response, &required);

        Ok(assessment::build_evaluation([
            FIRST_READ_OBSERVED.full_or_zero(
                probe_hits >= 1
                    && common::transcript_contains(&observation.transcript, PAGE_ONE_TOKEN),
                format!(
                    "observed {probe_hits} probe hit(s); page-one token present in transcript: {}",
                    common::transcript_contains(&observation.transcript, PAGE_ONE_TOKEN)
                ),
            ),
            BOUNDED_RETRY.full_or_zero(
                (2..=MAXIMUM_ATTEMPTS).contains(&attempts),
                format!(
                    "observed {attempts} call(s) to `{pager}`, expected 2 to {MAXIMUM_ATTEMPTS}"
                ),
            ),
            REDISCOVERY.full_or_zero(
                relisted >= 1,
                format!("observed {relisted} catalog listing(s) after the last failing call"),
            ),
            LOSS_REPORTED.full_or_zero(
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
        let pager = pager_id(run_id);
        let calls = common::function_calls(&observation.transcript);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "pager_calls": kit::calls_of(&calls, &pager).len(),
                "catalog_lists_after_failure": catalog_lists_after_failure(&observation.transcript, &pager),
                "response": observation.response,
            }),
            invariants,
            vec![
                kit::function_provenance(&pager, "served_one_page_then_deregistered"),
                kit::session_provenance(observation, "captured_vanishing_function_evidence"),
            ],
        )])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalog_listing_only_counts_after_the_last_failing_call() {
        let transcript = json!({
            "messages": [
                {"message": {"role": "assistant", "content": [
                    {"type": "function_call", "id": "a", "function_id": "engine::functions::list", "arguments": {}},
                    {"type": "function_call", "id": "b", "function_id": "e2etest::pager_run1", "arguments": {}}
                ]}}
            ]
        });
        assert_eq!(
            catalog_lists_after_failure(&transcript, "e2etest::pager_run1"),
            0
        );
    }

    #[test]
    fn a_catalog_listing_after_the_failure_is_counted() {
        let transcript = json!({
            "messages": [
                {"message": {"role": "assistant", "content": [
                    {"type": "function_call", "id": "a", "function_id": "e2etest::pager_run1", "arguments": {}},
                    {"type": "function_call", "id": "b", "function_id": "engine::functions::list", "arguments": {}}
                ]}}
            ]
        });
        assert_eq!(
            catalog_lists_after_failure(&transcript, "e2etest::pager_run1"),
            1
        );
    }
}
