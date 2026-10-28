//! `structured_output` — a turn that must deliver a JSON object. The send
//! carries `options.output` with a schema; the Harness picks the provider's
//! native structured output when `router::models::supports(…,
//! "structured_output")`, else it offers the synthetic `submit_result` tool.
//! The turn's `result` must be exactly `{answer: "STRUCT-OK", count: 3}` in
//! `harness::status` and on the `harness::turn-completed` event, and
//! `submit_result` must be called exactly when there is no native support.

use iii_sdk::protocol::RegisterTriggerInput;
use iii_sdk::RegisterFunction;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::mechanism::{self, record, Check};
use super::{
    async_trait, common, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation,
    Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "structured_output";
const DELIVERABLE_ID: &str = "structured_output_checks";
const SUBMIT_RESULT: &str = "submit_result";

const RESULT: AssessmentSpec = AssessmentSpec::gate_in(
    "result",
    40,
    "Is the turn's `result` in `harness::status` exactly `{\"answer\": \"STRUCT-OK\", \"count\": 3}`?",
    EvaluationDimension::Deliverable,
)
.labeled("The turn's result is the exact object");
const EVENT: AssessmentSpec = AssessmentSpec::gate(
    "completed_event",
    30,
    "Did the `harness::turn-completed` event of the turn carry the same `result`?",
)
.labeled("turn-completed carries the result");
const STRATEGY: AssessmentSpec = AssessmentSpec::gate(
    "strategy",
    30,
    "Was `submit_result` called exactly when the model has no native structured output, and never when it has?",
)
.labeled("submit_result only without native structured output");
const ASSESSMENTS: &[AssessmentSpec] = &[RESULT, EVENT, STRATEGY];

fn expected() -> Value {
    json!({ "answer": "STRUCT-OK", "count": 3 })
}

fn sink_function(run_id: &str) -> String {
    format!("e2e::structured::completed::{}", mechanism::short(run_id))
}

pub struct StructuredOutput;

#[async_trait]
impl Scenario for StructuredOutput {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Deliver a JSON result")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Return a two-field JSON object as the turn's result, through native structured output or the submit_result tool.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("A caller that asks a turn for JSON gets the validated object back as the turn's result, whichever way the model can produce it. If it fails, automations that read agent output as data get prose, nothing, or a result only some clients see.")
    }

    /// One retained cohort: the probes do not vary with a seed.
    fn case(&self, _seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            super::stable_seed(ID),
            json!({ "expected": expected() }),
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
            prompt: "Deliver the result object with `answer` set to exactly `STRUCT-OK` and \
                     `count` set to exactly 3. Do not call any other function."
                .to_string(),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(4),
                max_output_tokens: Some(2_048),
                max_total_tokens: Some(60_000),
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

    fn output_contract(&self) -> Option<Value> {
        Some(json!({
            "type": "json",
            "schema": {
                "type": "object",
                "required": ["answer", "count"],
                "properties": {
                    "answer": { "type": "string" },
                    "count": { "type": "integer" },
                },
                "additionalProperties": false,
            },
        }))
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        mechanism::reset(run_id);
        let owner = run_id.to_string();
        let function = context.client().register_function(
            sink_function(run_id),
            RegisterFunction::new_async(move |event: Value| {
                let owner = owner.clone();
                async move {
                    mechanism::append(&owner, "completed", event);
                    Ok::<Value, iii_sdk::errors::Error>(json!({ "accepted": true }))
                }
            })
            .description("E2E structured_output: records the session's turn-completed events.")
            .metadata(json!({ "internal": true })),
        );
        mechanism::keep_function(run_id, function);
        let trigger = context
            .client()
            .register_trigger(RegisterTriggerInput::new(
                crate::observe::TURN_COMPLETED_TRIGGER,
                sink_function(run_id),
                json!({ "session_id": mechanism::session_id(run_id) }),
            ))?;
        mechanism::keep_trigger(run_id, trigger);
        Ok(())
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let session_id = mechanism::session_id(run_id);
        let status = context
            .trigger_value(
                "harness::status",
                json!({ "session_id": session_id, "verbose": true }),
            )
            .await
            .unwrap_or(Value::Null);
        let native = match mechanism::subject_route(&observation.transcript) {
            Some((model, provider)) => context
                .trigger_value(
                    "router::models::supports",
                    json!({ "model": model, "provider": provider, "capability": "structured_output" }),
                )
                .await
                .ok()
                .and_then(|reply| reply["supported"].as_bool()),
            None => None,
        };
        let events = mechanism::noted(run_id, "completed").unwrap_or_else(|| json!([]));
        for check in checks(&observation.transcript, &status, &events, native) {
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
        Ok(assessment::build_evaluation([
            mechanism::grade(RESULT, &checks, &["status_result"]),
            mechanism::grade(EVENT, &checks, &["event_result"]),
            mechanism::grade(STRATEGY, &checks, &["submit_result_by_support"]),
        ]))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        mechanism::release(context, run_id).await
    }
}

fn checks(transcript: &Value, status: &Value, events: &Value, native: Option<bool>) -> Vec<Check> {
    let turn_id = status["turn_id"].as_str().unwrap_or_default();
    let event = events
        .as_array()
        .into_iter()
        .flatten()
        .filter(|event| event["turn_id"] == turn_id && event["status"] == "completed")
        .last();
    let submits = common::function_calls(transcript)
        .into_iter()
        .filter(|call| call.function_id == SUBMIT_RESULT)
        .count();
    let strategy = match native {
        Some(true) => submits == 0,
        Some(false) => submits >= 1,
        None => false,
    };
    vec![
        Check {
            id: "status_result".into(),
            passed: status["result"] == expected(),
            detail: format!("status={} result={}", status["status"], status["result"]),
        },
        Check {
            id: "event_result".into(),
            passed: event.is_some_and(|event| event["result"] == expected()),
            detail: format!(
                "event for {turn_id}: {:?}",
                event.map(|event| &event["result"])
            ),
        },
        Check {
            id: "submit_result_by_support".into(),
            passed: strategy,
            detail: format!("native={native:?} submit_result calls={submits}"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failing(
        transcript: &Value,
        status: &Value,
        events: &Value,
        native: Option<bool>,
    ) -> Vec<String> {
        checks(transcript, status, events, native)
            .into_iter()
            .filter(|check| !check.passed)
            .map(|check| check.id)
            .collect()
    }

    fn submitted() -> Value {
        json!({ "messages": [{ "message": { "role": "assistant", "content": [{
            "type": "function_call", "id": "c1", "function_id": SUBMIT_RESULT, "arguments": expected(),
        }] } }] })
    }

    #[test]
    fn the_result_must_reach_status_and_the_event() {
        let status = json!({ "turn_id": "t_1", "status": "completed", "result": expected() });
        let events = json!([{ "turn_id": "t_1", "status": "completed", "result": expected() }]);
        assert_eq!(
            failing(&submitted(), &status, &events, Some(false)),
            Vec::<String>::new()
        );
        let prose = json!({ "turn_id": "t_1", "status": "completed", "result": "STRUCT-OK, 3" });
        assert_eq!(
            failing(&submitted(), &prose, &events, Some(false)),
            ["status_result"]
        );
        let other_turn = json!([{ "turn_id": "t_0", "status": "completed", "result": expected() }]);
        assert_eq!(
            failing(&submitted(), &status, &other_turn, Some(false)),
            ["event_result"]
        );
    }

    #[test]
    fn submit_result_is_the_fallback_only() {
        let status = json!({ "turn_id": "t_1", "result": expected() });
        let events = json!([{ "turn_id": "t_1", "status": "completed", "result": expected() }]);
        let native = json!({ "messages": [] });
        assert_eq!(
            failing(&native, &status, &events, Some(true)),
            Vec::<String>::new()
        );
        assert_eq!(
            failing(&submitted(), &status, &events, Some(true)),
            ["submit_result_by_support"]
        );
        assert_eq!(
            failing(&native, &status, &events, Some(false)),
            ["submit_result_by_support"]
        );
    }

    #[test]
    fn the_send_asks_for_the_schema() {
        let output = StructuredOutput.output_contract().unwrap();
        assert_eq!(output["type"], "json");
        StructuredOutput.spec("run").validate().unwrap();
    }
}
