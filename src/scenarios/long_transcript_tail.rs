//! `long_transcript_tail` — a turn reads the whole transcript, past the
//! first page. `setup` seeds the subject's session with 600 notes through
//! `session::append-many`, each setting the current code: `OLD-<n>` until
//! the last one (entry 599, on the second 500-entry page) sets `TAIL-<run>`. The one exchange asks for the
//! current code; a Harness that read only the first page answers an `OLD-`.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::mechanism::{self, record};
use super::{
    async_trait, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation, Scenario,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "long_transcript_tail";
const DELIVERABLE_ID: &str = "long_transcript_checks";
/// Entries seeded, past `session::messages`' 500-entry page.
const SEEDED: usize = 600;

const TAIL_CODE: AssessmentSpec = AssessmentSpec::gate_in(
    "tail_code",
    60,
    "Does the reply give the code the last of the 600 seeded entries set, `TAIL-<run>`?",
    EvaluationDimension::Deliverable,
)
.labeled("The reply has the code from the transcript's tail");
const NO_STALE_CODE: AssessmentSpec = AssessmentSpec::gate(
    "no_stale_code",
    40,
    "Is the reply free of every superseded `OLD-` code?",
)
.labeled("No superseded code in the reply");
const ASSESSMENTS: &[AssessmentSpec] = &[TAIL_CODE, NO_STALE_CODE];

fn tail_code(run_id: &str) -> String {
    let hex = super::stable_seed(&format!("{ID}:{run_id}")) & 0xff_ffff;
    format!("TAIL-{hex:06x}")
}

/// The seeded notes, oldest first. User messages only: a seeded assistant
/// reply would count as a turn of an unpriced model in `harness::metrics`.
fn seeded(run_id: &str) -> Vec<Value> {
    (0..SEEDED)
        .map(|note| {
            let code = if note + 1 == SEEDED {
                tail_code(run_id)
            } else {
                format!("OLD-{note:03}")
            };
            json!({
                "role": "user",
                "content": [{ "type": "text", "text": format!("Note {note}: the current code is now {code}.") }],
                "timestamp": 1_700_000_000_000_i64 + note as i64,
            })
        })
        .collect()
}

pub struct LongTranscriptTail;

#[async_trait]
impl Scenario for LongTranscriptTail {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Read a long conversation to its end")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("A session already holds 600 short notes that keep changing a code; say the current one.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("A turn sees a long conversation through to its newest message, past the first page of the transcript. If it fails, agents in long sessions answer from stale history and ignore what the user said last.")
    }

    /// One retained cohort: the probes do not vary with a seed.
    fn case(&self, _seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            super::stable_seed(ID),
            json!({ "seeded_entries": SEEDED }),
            vec![Capability::E2eControlPlaneV1],
            mechanism::contract(DELIVERABLE_ID),
        )
    }

    fn canonical_seed_only(&self) -> bool {
        true
    }

    fn spec(&self, _run_id: &str) -> ScenarioSpec {
        ScenarioSpec {
            id: ID,
            prompt: "Earlier in this conversation I kept changing a code. What is the current \
                     code? Reply with exactly that code and nothing else, without calling any \
                     function."
                .to_string(),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(3),
                max_output_tokens: Some(1_024),
                max_total_tokens: Some(80_000),
                stuck_timeout_seconds: 180,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    fn allowed_functions(&self, _run_id: &str) -> Option<Vec<String>> {
        Some(Vec::new())
    }

    fn planted_state(&self, run_id: &str) -> Option<Value> {
        Some(json!({ "session_entries": seeded(run_id) }))
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        mechanism::reset(run_id);
        let session_id = mechanism::session_id(run_id);
        context
            .trigger_value(
                "session::ensure",
                json!({
                    "session_id": session_id,
                    "kind": crate::wire::E2E_SESSION_KIND,
                    "title": format!("Harness E2E: {ID}"),
                    "metadata": { "e2e_run_id": run_id, "e2e_scenario": ID },
                }),
            )
            .await?;
        let appended = context
            .trigger_value(
                "session::append-many",
                json!({ "session_id": session_id, "messages": seeded(run_id) }),
            )
            .await?;
        let count = appended["entry_ids"].as_array().map_or(0, Vec::len);
        anyhow::ensure!(
            count == SEEDED,
            "session::append-many seeded {count} entries, expected {SEEDED}"
        );
        Ok(())
    }

    async fn capture(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let reply = observation.response.trim();
        let code = tail_code(run_id);
        record(
            run_id,
            "tail_code",
            reply.contains(&code),
            format!("reply={reply:?} expected {code}"),
        );
        record(
            run_id,
            "no_stale_code",
            !reply.contains("OLD-"),
            format!("reply={reply:?}"),
        );
        Ok(vec![mechanism::deliverable(DELIVERABLE_ID, run_id)])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        if !observation.metrics.complete {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "metrics_complete",
                "subject metrics are incomplete",
            ));
        }
        let checks = mechanism::observed_checks(observation, run_id);
        Ok(assessment::build_evaluation([
            mechanism::grade(TAIL_CODE, &checks, &["tail_code"]),
            mechanism::grade(NO_STALE_CODE, &checks, &["no_stale_code"]),
        ]))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        mechanism::release(context, run_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_last_note_past_the_first_page_carries_the_tail_code() {
        let notes = seeded("run");
        assert_eq!(notes.len(), SEEDED);
        let code = tail_code("run");
        let carrying: Vec<usize> = notes
            .iter()
            .enumerate()
            .filter(|(_, note)| mechanism::text_of(note).contains(&code))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(carrying, [599]);
        assert!(mechanism::text_of(&notes[499]).contains("OLD-499"));
    }

    #[test]
    fn the_seeded_transcript_is_part_of_the_definition() {
        let planted = LongTranscriptTail
            .planted_state(super::super::CONTRACT_NAMESPACE)
            .unwrap();
        assert_eq!(planted["session_entries"].as_array().unwrap().len(), SEEDED);
        LongTranscriptTail.spec("run").validate().unwrap();
    }
}
