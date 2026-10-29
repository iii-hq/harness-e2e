//! `ask_handshake` — the agent asks the user through `harness::ask` and acts
//! on the answer. Exchange 0 asks for one pick: the call must be answered
//! `awaiting_answer` with the card, and the turn must end on it, with no
//! assistant entry after the card. Exchange 1 is the user's pick, sent the
//! way the card sends it (`Color: Amber`); the agent must store it with
//! `state::set`.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::mechanism::{self, record, Check};
use super::{
    async_trait, common, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation,
    Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "ask_handshake";
const DELIVERABLE_ID: &str = "ask_handshake_checks";
const ASK: &str = "harness::ask";
const KEY: &str = "color";
const PICK: &str = "Amber";
const DONE_MARKER: &str = "ASK-DONE";

const AWAITING: AssessmentSpec = AssessmentSpec::gate(
    "awaiting_answer",
    35,
    "Was the `harness::ask` call answered `awaiting_answer` with its question id, session, turn and the questions as asked?",
)
.labeled("The ask is answered awaiting_answer with its card");
const ENDS_ON_CARD: AssessmentSpec = AssessmentSpec::gate(
    "ends_on_card",
    35,
    "Did the asking turn end on the card, with no assistant entry after it and the turn completed before the answer was sent?",
)
.labeled("The turn ends on the card");
const ANSWER_STORED: AssessmentSpec = AssessmentSpec::gate_in(
    "answer_stored",
    30,
    "Did the agent store the user's pick from the next message as the requested state value?",
    EvaluationDimension::Deliverable,
)
.labeled("The user's pick is stored as state");
const ASSESSMENTS: &[AssessmentSpec] = &[AWAITING, ENDS_ON_CARD, ANSWER_STORED];

fn scope(run_id: &str) -> String {
    format!("e2e-ask-{run_id}")
}

fn prompts(run_id: &str) -> [String; 2] {
    let scope = scope(run_id);
    [
        format!(
            "I want to choose a color, and you must not choose it for me. Ask me with one \
             `harness::ask` call: one question with header `Color`, question `Which color should \
             I save?` and the options `Teal` and `Amber`. Then stop and wait for my answer. \
             When it arrives, store my pick with one `state::set` call in scope `{scope}` under \
             key `{KEY}` as `{{\"color\": \"<the label I picked>\"}}`, and reply with one line \
             containing `{DONE_MARKER}`."
        ),
        format!("Color: {PICK}"),
    ]
}

pub struct AskHandshake;

#[async_trait]
impl Scenario for AskHandshake {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Ask the user, then act on the answer")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Ask the user to pick a color with harness::ask, stop, then store the pick that arrives as the next message.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("An agent that needs a decision shows the user a card of options and waits for the pick instead of guessing or answering itself. If it fails, agents keep talking past their own questions or act on choices the user never made.")
    }

    /// One retained cohort: the probes do not vary with a seed.
    fn case(&self, _seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            super::stable_seed(ID),
            json!({ "key": KEY, "pick": PICK }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiFunctions,
                Capability::IiiState,
            ],
            mechanism::contract(DELIVERABLE_ID),
        )
    }

    fn canonical_seed_only(&self) -> bool {
        true
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let [prompt, _] = prompts(run_id);
        ScenarioSpec {
            id: ID,
            prompt,
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(6),
                max_output_tokens: Some(2_048),
                max_total_tokens: Some(80_000),
                stuck_timeout_seconds: 180,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    fn dialogue_followups(&self, run_id: &str) -> Vec<String> {
        prompts(run_id).into_iter().skip(1).collect()
    }

    fn allowed_functions(&self, _run_id: &str) -> Option<Vec<String>> {
        Some(vec![ASK.into(), "state::set".into()])
    }

    async fn setup(&self, _context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        mechanism::reset(run_id);
        Ok(())
    }

    async fn before_followup(
        &self,
        context: &E2eContext,
        run_id: &str,
        session_id: &str,
        _exchange: usize,
    ) {
        let status = context
            .trigger_value("harness::status", json!({ "session_id": session_id }))
            .await;
        record(
            run_id,
            "completed_before_answer",
            matches!(&status, Ok(status) if status["status"] == "completed"
                && status["pending_function_calls"].as_array().is_none_or(Vec::is_empty)),
            format!("{status:?}"),
        );
        if let Ok(status) = status {
            mechanism::note(run_id, "asking_turn", status["turn_id"].clone());
        }
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let stored = common::get_state(context, &scope(run_id), KEY)
            .await
            .unwrap_or(Value::Null);
        let asking_turn = mechanism::noted(run_id, "asking_turn")
            .and_then(|turn| turn.as_str().map(str::to_string))
            .unwrap_or_default();
        for check in checks(
            &observation.transcript,
            run_id,
            &asking_turn,
            &stored,
            &observation.response,
        ) {
            record(run_id, &check.id, check.passed, check.detail);
        }
        Ok(vec![mechanism::deliverable(DELIVERABLE_ID, run_id)])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        let checks = mechanism::observed_checks(observation, run_id);
        if !observation.metrics.complete {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "metrics_complete",
                "subject metrics are incomplete",
            ));
        }
        if !common::function_calls(&observation.transcript)
            .iter()
            .any(|call| call.function_id == ASK)
        {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "ask_called",
                "the subject never called harness::ask",
            ));
        }
        Ok(assessment::build_evaluation([
            mechanism::grade(AWAITING, &checks, &["awaiting_answer"]),
            mechanism::grade(
                ENDS_ON_CARD,
                &checks,
                &["completed_before_answer", "no_reply_after_card"],
            ),
            mechanism::grade(ANSWER_STORED, &checks, &["answer_stored"]),
        ]))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        let _ = context
            .trigger_value(
                "state::delete",
                json!({ "scope": scope(run_id), "key": KEY }),
            )
            .await;
        mechanism::release(context, run_id).await
    }
}

fn checks(
    transcript: &Value,
    run_id: &str,
    asking_turn: &str,
    stored: &Value,
    response: &str,
) -> Vec<Check> {
    let session_id = mechanism::session_id(run_id);
    let asks: Vec<_> = common::function_outcomes(transcript)
        .into_iter()
        .filter(|call| call.function_id == ASK)
        .collect();
    let card = asks
        .first()
        .and_then(|ask| ask.details.clone())
        .unwrap_or(Value::Null);
    let awaiting = asks.len() == 1
        && asks[0].is_error == Some(false)
        && card["status"] == "awaiting_answer"
        && card["question_id"].as_str() == asks[0].call_id.as_deref()
        && card["session_id"] == session_id.as_str()
        && card["turn_id"] == asking_turn
        && card["questions"][0]["header"] == "Color"
        && card["questions"][0]["options"]
            .as_array()
            .is_some_and(|options| options.len() == 2);
    // Everything the asking turn wrote after the card's result entry.
    let card_entry = asks
        .first()
        .and_then(|ask| ask.call_id.as_deref())
        .map(|call_id| format!("e_{asking_turn}_{}", mechanism::entry_safe(call_id)));
    let after_card: Vec<&str> = mechanism::entries(transcript)
        .filter_map(|entry| entry["entry_id"].as_str())
        .skip_while(|id| Some(*id) != card_entry.as_deref())
        .skip(1)
        .filter(|id| id.starts_with(&format!("e_{asking_turn}_")) && id.ends_with("_assistant"))
        .collect();
    let card_written = card_entry
        .as_deref()
        .is_some_and(|card| mechanism::entries(transcript).any(|entry| entry["entry_id"] == card));
    vec![
        Check {
            id: "awaiting_answer".into(),
            passed: awaiting,
            detail: format!("{} ask call(s), card={card}", asks.len()),
        },
        Check {
            id: "no_reply_after_card".into(),
            passed: card_written && after_card.is_empty(),
            detail: format!("card entry {card_entry:?} written={card_written}, assistant entries after it: {after_card:?}"),
        },
        Check {
            id: "answer_stored".into(),
            passed: *stored == json!({ "color": PICK }) && response.contains(DONE_MARKER),
            detail: format!("stored={stored} done_marker={}", response.contains(DONE_MARKER)),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = "control";

    fn healthy() -> Value {
        let session = mechanism::session_id(RUN);
        json!({ "messages": [
            { "entry_id": "e_idem_0", "message": { "role": "user", "content": [{ "type": "text", "text": "ask" }] } },
            { "entry_id": "e_t_a_0_assistant", "message": { "role": "assistant", "content": [
                { "type": "function_call", "id": "c1", "function_id": ASK, "arguments": {} },
            ] } },
            { "entry_id": "e_t_a_c1", "message": {
                "role": "function_result", "function_call_id": "c1", "function_id": ASK, "is_error": false,
                "details": { "status": "awaiting_answer", "question_id": "c1", "session_id": session, "turn_id": "t_a",
                    "questions": [{ "header": "Color", "options": [{ "label": "Teal" }, { "label": "Amber" }] }] },
            } },
            { "entry_id": "e_idem_1", "message": { "role": "user", "content": [{ "type": "text", "text": "Color: Amber" }] } },
            { "entry_id": "e_t_b_0_assistant", "message": { "role": "assistant", "content": [{ "type": "text", "text": "ASK-DONE" }] } },
        ] })
    }

    fn failing(transcript: &Value, stored: &Value) -> Vec<String> {
        checks(transcript, RUN, "t_a", stored, "Saved. ASK-DONE")
            .into_iter()
            .filter(|check| !check.passed)
            .map(|check| check.id)
            .collect()
    }

    #[test]
    fn a_healthy_handshake_passes() {
        assert_eq!(
            failing(&healthy(), &json!({ "color": "Amber" })),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_reply_after_the_card_or_a_guessed_pick_is_caught() {
        let mut transcript = healthy();
        transcript["messages"].as_array_mut().unwrap().insert(
            3,
            json!({ "entry_id": "e_t_a_1_assistant", "message": { "role": "assistant", "content": [{ "type": "text", "text": "Which color?" }] } }),
        );
        assert_eq!(
            failing(&transcript, &json!({ "color": "Amber" })),
            ["no_reply_after_card"]
        );
        assert_eq!(
            failing(&healthy(), &json!({ "color": "Teal" })),
            ["answer_stored"]
        );
    }

    #[test]
    fn a_card_for_another_turn_is_caught() {
        let mut transcript = healthy();
        transcript["messages"][2]["message"]["details"]["turn_id"] = json!("t_b");
        assert_eq!(
            failing(&transcript, &json!({ "color": "Amber" })),
            ["awaiting_answer"]
        );
    }

    #[test]
    fn criteria_weights_total_one_hundred() {
        AskHandshake.spec("run").validate().unwrap();
    }
}
