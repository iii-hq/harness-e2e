//! A reviewer rejects a draft once per missing field. Convergence means
//! acting on each rejection and stopping the moment it is accepted.

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

pub const ID: &str = "orchestration.repair_convergence";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "repair_convergence_evidence";
const REQUIRED_FIELDS: [&str; 3] = ["title", "owner", "risk"];
const EXPECTED_SUBMISSIONS: usize = REQUIRED_FIELDS.len() + 1;
const MAXIMUM_SUBMISSIONS: usize = 6;

const CONVERGED: AssessmentSpec = AssessmentSpec::hard_gated(
    "converged",
    40,
    "The reviewer accepted a draft carrying every required field.",
);
const BOUNDED_SUBMISSIONS: AssessmentSpec = AssessmentSpec::hard_gated(
    "bounded_submissions",
    30,
    "Convergence took one submission per rejection plus the accepted one.",
);
const SUBMISSIONS_ADVANCED: AssessmentSpec = AssessmentSpec::hard_gated(
    "submissions_advanced",
    30,
    "No submission repeated the draft the reviewer had already rejected.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[CONVERGED, BOUNDED_SUBMISSIONS, SUBMISSIONS_ADVANCED];

fn review_id(run_id: &str) -> String {
    probe::id("review", run_id)
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> crate::scenarios::CleanupFuture<'a> {
    Box::pin(async move {
        let id = review_id(run_id);
        probe::register(
            context,
            id.clone(),
            "E2E temporary reviewer probe: names one missing field per rejection.",
            move |request: Value| {
                let id = id.clone();
                async move {
                    probe::record_hit(&id);
                    let draft = request.get("draft").cloned().unwrap_or(Value::Null);
                    let missing = REQUIRED_FIELDS.iter().find(|field| {
                        draft
                            .get(**field)
                            .and_then(Value::as_str)
                            .is_none_or(|value| value.trim().is_empty())
                    });
                    let ledger = probe::ledger(&id);
                    let mut entries = ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match missing {
                        Some(field) => Ok(json!({
                            "accepted": false,
                            "missing": field,
                            "reason": format!("draft is missing the `{field}` field"),
                        })),
                        None => {
                            entries.insert("accepted".into(), Value::from(1));
                            Ok(json!({ "accepted": true }))
                        }
                    }
                }
            },
        );
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let review = review_id(run_id);
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Get a change request through review.\n\n\
             1. Submit an empty draft: call `{review}` with payload {{\"draft\": {{}}}}.\n\
             2. Each rejection names exactly one missing field. Add that field with a short \
             non-empty string value and submit the whole draft again, keeping the fields you \
             already added.\n\
             3. Stop as soon as the reviewer accepts. Make at most {MAXIMUM_SUBMISSIONS} \
             submissions in total, and never submit a draft identical to one already rejected.\n\
             4. Reply with exactly one line: `ACCEPTED:true SUBMISSIONS:<n>`."
        ),
        filesystem_root: None,
        execution: kit::policy(16, 200_000, 300),
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
            "review_function": "e2etest::review_<run>",
            "required_fields": REQUIRED_FIELDS,
            "expected_submissions": EXPECTED_SUBMISSIONS,
            "maximum_submissions": MAXIMUM_SUBMISSIONS,
        }),
        super::loop_profile(2),
        &[],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["submissions", "accepted", "response"],
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

fn submitted_drafts(observation: &ScenarioObservation, review: &str) -> Vec<Value> {
    common::function_calls(&observation.transcript)
        .into_iter()
        .filter(|call| call.function_id == review)
        .map(|call| call.arguments.get("draft").cloned().unwrap_or(Value::Null))
        .collect()
}

fn each_submission_differs(drafts: &[Value]) -> bool {
    drafts.windows(2).all(|pair| pair[0] != pair[1])
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let review = review_id(run_id);
        let drafts = submitted_drafts(observation, &review);
        let accepted = probe::ledger_u64(&review, "accepted") == 1;
        let submissions = drafts.len();

        Ok(assessment::build_evaluation([
            CONVERGED.full_or_zero(
                accepted && observation.response.contains("ACCEPTED:true"),
                format!("reviewer acceptance recorded: {accepted}"),
            ),
            BOUNDED_SUBMISSIONS.full_or_zero(
                (EXPECTED_SUBMISSIONS..=MAXIMUM_SUBMISSIONS).contains(&submissions),
                format!(
                    "observed {submissions} submission(s), expected {EXPECTED_SUBMISSIONS} to {MAXIMUM_SUBMISSIONS}"
                ),
            ),
            SUBMISSIONS_ADVANCED.full_or_zero(
                submissions > 1 && each_submission_differs(&drafts),
                format!("observed {submissions} submission(s); each differs from the previous: {}",
                    each_submission_differs(&drafts)),
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
        let review = review_id(run_id);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "submissions": submitted_drafts(observation, &review).len(),
                "accepted": probe::ledger_u64(&review, "accepted") == 1,
                "response": observation.response,
            }),
            invariants,
            vec![
                kit::function_provenance(&review, "rejected_until_every_field_arrived"),
                kit::session_provenance(observation, "captured_repair_convergence_evidence"),
            ],
        )])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeated_draft_breaks_the_advance_check() {
        let first = json!({ "title": "t" });
        assert!(!each_submission_differs(&[first.clone(), first]));
        assert!(each_submission_differs(&[
            json!({}),
            json!({ "title": "t" })
        ]));
    }
}
