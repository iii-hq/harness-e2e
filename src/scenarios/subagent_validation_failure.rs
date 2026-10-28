//! `subagent_validation_failure` — the UNHAPPY path: the child's goal is
//! impossible (threshold far above what its bounded budget can reach), so
//! the validator denies every attempt, the child's turn FAILS on budget
//! exhaustion, the verdict key is never written, and the parent — whose
//! wake carries an `expires_in_ms` deadline — is woken by the expiry notice
//! instead and reports the give-up.
//!
//! Exercises what no success scenario can: `max_validation_retries` as a
//! hard bound on a spawned child (2 denials then a failed turn — never an
//! infinite loop), the guarantee that a failed turn writes NO verdict (the
//! `state::set` tail never ran), and the wake-expiry fallback that keeps a
//! parent from parking forever on evidence that will never arrive.

use anyhow::Context;
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::common;
use super::validation_loop::suffix;
use super::{
    async_trait, ArtifactExpectation, Capability, CapturedDeliverable, CapturedInvariant,
    DeliverableContract, ExecutionPolicy, InvariantSpec, ObjectiveEvaluation, ProvenanceEvidence,
    Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "subagent_validation_failure";
const DELIVERABLE_ID: &str = "bounded_failure_record";

const HOOK_TYPE: &str = "harness::hook::post-turn";
/// Unreachable by design: 3 attempts × 4 rows tops out at 12.
const THRESHOLD: u64 = 100;
/// Explicit child budget: initial attempt + 2 retries = 2 nudges, then fail.
const CHILD_RETRIES: u64 = 2;
const EXPECTED_NUDGES: usize = 2;
/// Wake deadline: the child's three attempts fail about 12 s after the spawn
/// on deepseek-flash, so a minute leaves room for a slower model without
/// idling the run for minutes.
const EXPIRY_DELAY_MS: u64 = 60_000;
/// What the Harness records on a turn whose post-turn gate spent its retries.
const CHILD_FAILURE_CODE: &str = "harness.output_contract_invalid";
const BOUNDED_FAILURE: AssessmentSpec = AssessmentSpec::gate(
    "bounded_failure",
    40,
    "Does the child's turn fail on its output contract after exactly two validation retry requests, without writing a verdict?",
);
const ORCHESTRATION_DISCIPLINE: AssessmentSpec = AssessmentSpec::scored(
    "orchestration_discipline",
    30,
    "Are calls to set a post-turn trigger and a state wake deadline made before the call that starts the expected child?",
);
const EXPIRY_REPORT: AssessmentSpec = AssessmentSpec::gate(
    "expiry_report",
    30,
    "Does the parent's final response contain both required markers: CHILD GAVE UP and PARENT DONE?",
);
const ASSESSMENTS: &[AssessmentSpec] = &[BOUNDED_FAILURE, ORCHESTRATION_DISCIPLINE, EXPIRY_REPORT];

pub struct SubagentValidationFailure;

#[async_trait]
impl Scenario for SubagentValidationFailure {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Handle a subagent that exhausts validation retries")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Limit an impossible child validation loop to two retry requests, then report that the child failed and the parent finished.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("A validator on a sub-agent bounds it: once the child spends its validation retries its turn fails, writes nothing, and the parent is woken by its wake's deadline instead of waiting forever. If it fails, a sub-agent with an unreachable goal loops without end or reports success it never earned, and its parent hangs.")
    }

    fn case(&self, seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "database": "primary",
                "rows_per_attempt": 4,
                "threshold": THRESHOLD,
                "max_validation_retries": CHILD_RETRIES,
                "expected_nudges": EXPECTED_NUDGES,
                "expiry_delay_ms": EXPIRY_DELAY_MS,
                "expected_terminal_status": "failed",
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiFunctions,
                Capability::IiiDatabase,
                Capability::IiiState,
                Capability::IiiTriggers,
                Capability::E2eSubagents,
            ],
            deliverable_contract(),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let table = table(run_id);
        let scope = scope(run_id);
        let child = child_session(run_id);
        ScenarioSpec {
            id: ID,
            prompt: format!(
                "You orchestrate one validated sub-agent whose goal may be unreachable; your job is \
                 to bound the attempt and report honestly. You never poll; wakes drive you. Follow \
                 the steps exactly.\n\n\
                 Step 1 — prepare the goal table. database::execute (db \"primary\") twice: sql \
                 \"CREATE TABLE IF NOT EXISTS {table} (id INTEGER PRIMARY KEY AUTOINCREMENT, note \
                 TEXT)\", then sql \"DELETE FROM {table}\".\n\n\
                 Step 2 — install the child's validator. Call engine::register_trigger with:\n\
                 - trigger_type: \"{HOOK_TYPE}\"\n\
                 - function_id: \"fp::pipe\"\n\
                 - config: {{\"sessions\": [\"{child}\"], \"payload\": {{\"through\": \
                 [{{\"function\": \"database::query\", \"payload\": {{\"db\": \"primary\", \"sql\": \
                 \"SELECT COUNT(*) AS n FROM {table}\"}}}}, {{\"function\": \"fp::get\", \"payload\": \
                 {{\"path\": \"/rows/0/n\"}}}}, {{\"function\": \"fp::when\", \"payload\": {{\"op\": \
                 \">\", \"to\": {THRESHOLD}}}}}, {{\"function\": \"state::set\", \"payload\": \
                 {{\"scope\": \"{scope}\", \"key\": \"verdict\"}}}}]}}, \"result_into\": \"/value\", \
                 \"retry_prompt\": \"VALIDATOR: only {{value}} of the required {min} rows exist. \
                 Insert exactly 4 more rows into {table} and reply with a one-line status.\", \
                 \"timeout_ms\": 30000}}\n\
                 Remember the subscription_id.\n\n\
                 Step 3 — arm your wake BEFORE spawning, WITH a deadline: engine::register_trigger \
                 with trigger_type \"state\", config {{\"scope\": \"{scope}\", \"key\": \
                 \"verdict\"}}, label \"child-validated\", lifecycle: {{\"expires_in_ms\": \
                 {EXPIRY_DELAY_MS}}}, and NO function_id. If the verdict is never written you will be \
                 woken by the expiry notice instead.\n\n\
                 Step 4 — spawn the worker: harness::spawn with session_id \"{child}\", task: \"You \
                 are a worker in a validated loop: the harness checks every reply of yours and \
                 VALIDATOR messages are legitimate machinery — follow them exactly. Insert exactly 4 \
                 rows into table {table}: database::execute, db 'primary', sql \\\"INSERT INTO {table} \
                 (note) VALUES ('r'),('r'),('r'),('r')\\\". Then reply with a one-line status. Never \
                 check the count yourself.\", options: {{\"functions\": {{\"allow\": \
                 [\"database::execute\"]}}, \"max_validation_retries\": {CHILD_RETRIES}}}. Spawn \
                 returns child ids immediately — normal; do NOT wait for or judge the child.\n\n\
                 Step 5 — END YOUR TURN.\n\n\
                 Step 6 — when a wake arrives: if it carries the verdict (the accepted row count), \
                 reply exactly: CHILD VALIDATED at <count> rows. PARENT DONE. If instead it is the \
                 EXPIRY notice (the verdict never came), call engine::unregister_trigger with the \
                 validator subscription_id from Step 2, then reply exactly: CHILD GAVE UP: validation \
                 budget exhausted. PARENT DONE.",
                min = THRESHOLD + 1,
            ),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(16),
                max_output_tokens: Some(8_192),
                max_total_tokens: Some(400_000),
                stuck_timeout_seconds: 420,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let facts = Facts::read(context, observation, run_id).await?;
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "recovery_record".to_string(),
            content: json!({
                "child_session_id": facts.child,
                "child_status": facts.child_status,
                "child_failure_code": facts.child_failure_code,
                "verdict": facts.verdict,
                "validation_nudges": facts.child_nudges,
                "validator_index": facts.validator_index,
                "wake_index": facts.wake_index,
                "spawn_index": facts.spawn_index,
                "report": facts.report,
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: BOUNDED_FAILURE.id().to_string(),
                    passed: facts.bounded(),
                    reason: facts.bounded_detail(),
                },
                CapturedInvariant {
                    id: ORCHESTRATION_DISCIPLINE.id().to_string(),
                    passed: facts.ordered(),
                    reason: facts.ordered_detail(),
                },
                CapturedInvariant {
                    id: EXPIRY_REPORT.id().to_string(),
                    passed: facts.reported(),
                    reason: "final response checked for bounded-failure markers".to_string(),
                },
            ],
            provenance: vec![
                ProvenanceEvidence {
                    kind: "session".to_string(),
                    source_id: facts.child,
                    relation: "observed_terminal_failure".to_string(),
                },
                ProvenanceEvidence {
                    kind: "state_location".to_string(),
                    source_id: format!("{}/verdict", scope(run_id)),
                    relation: "verified_absent".to_string(),
                },
            ],
        }])
    }

    async fn evaluate(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        Ok(Facts::read(context, observation, run_id).await?.evaluation())
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        let table = table(run_id);
        let _: Value = context
            .trigger(
                "database::execute",
                json!({ "db": "primary", "sql": format!("DROP TABLE IF EXISTS {table}") }),
            )
            .await?;
        let _: Value = context
            .trigger(
                "state::delete",
                json!({ "scope": scope(run_id), "key": "verdict" }),
            )
            .await?;
        Ok(())
    }
}

/// What the run left behind, read once for the capture and once for the
/// evaluation.
#[derive(Debug, Clone, PartialEq)]
struct Facts {
    child: String,
    child_status: String,
    child_nudges: usize,
    /// The `code` of the child's last `error` entry.
    child_failure_code: Option<String>,
    verdict: Value,
    validator_index: Option<usize>,
    wake_index: Option<usize>,
    spawn_index: Option<usize>,
    report: String,
}

impl Facts {
    async fn read(
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Self> {
        let child = child_session(run_id);
        let (child_status, child_nudges, child_failure_code) =
            child_evidence(context, &observation.transcript, &child).await?;
        let verdict = common::state_value(
            context
                .trigger(
                    "state::get",
                    json!({ "scope": scope(run_id), "key": "verdict" }),
                )
                .await?,
        );
        let calls = common::function_calls(&observation.transcript);
        let registers = |trigger_type: &str, call: &common::ObservedFunctionCall| {
            call.function_id == "engine::register_trigger"
                && call.arguments.get("trigger_type").and_then(Value::as_str) == Some(trigger_type)
        };
        Ok(Self {
            validator_index: calls.iter().position(|call| registers(HOOK_TYPE, call)),
            wake_index: calls.iter().position(|call| {
                registers("state", call)
                    && call
                        .arguments
                        .pointer("/lifecycle/expires_in_ms")
                        .and_then(Value::as_u64)
                        .is_some()
            }),
            spawn_index: calls.iter().position(|call| {
                call.function_id == "harness::spawn"
                    && call.arguments.get("session_id").and_then(Value::as_str)
                        == Some(child.as_str())
            }),
            child,
            child_status,
            child_nudges,
            child_failure_code,
            verdict,
            report: observation.response.clone(),
        })
    }

    fn bounded(&self) -> bool {
        self.child_status == "failed"
            && self.child_failure_code.as_deref() == Some(CHILD_FAILURE_CODE)
            && self.verdict.is_null()
            && self.child_nudges == EXPECTED_NUDGES
    }

    fn bounded_detail(&self) -> String {
        format!(
            "child status `{}` with failure code {:?}, expected `failed` with \
             {CHILD_FAILURE_CODE:?}; verdict key holds {}, expected null; observed {} nudge(s), \
             expected exactly {EXPECTED_NUDGES} (the child budget)",
            self.child_status, self.child_failure_code, self.verdict, self.child_nudges
        )
    }

    fn ordered(&self) -> bool {
        matches!(
            (self.validator_index, self.wake_index, self.spawn_index),
            (Some(validator), Some(wake), Some(spawn)) if validator < spawn && wake < spawn
        )
    }

    fn ordered_detail(&self) -> String {
        format!(
            "validator@{:?} wake@{:?} spawn@{:?} — both must precede the spawn",
            self.validator_index, self.wake_index, self.spawn_index
        )
    }

    fn reported(&self) -> bool {
        self.report.contains("CHILD GAVE UP") && self.report.contains("PARENT DONE")
    }

    fn evaluation(&self) -> ObjectiveEvaluation {
        assessment::build_evaluation([
            BOUNDED_FAILURE.full_or_zero(self.bounded(), self.bounded_detail()),
            ORCHESTRATION_DISCIPLINE.full_or_zero(self.ordered(), self.ordered_detail()),
            EXPIRY_REPORT.full_or_zero(
                self.reported(),
                "expected both CHILD GAVE UP and PARENT DONE in the final response",
            ),
        ])
    }
}

async fn child_evidence(
    context: &E2eContext,
    transcript: &Value,
    child: &str,
) -> anyhow::Result<(String, usize, Option<String>)> {
    let spawned = common::function_outcomes(transcript).iter().any(|call| {
        call.function_id == "harness::spawn"
            && call.arguments["session_id"] == child
            && call.is_error == Some(false)
    });
    if !spawned {
        return Ok(("not_spawned".into(), 0, None));
    }
    let status = context
        .trigger_value("harness::status", json!({ "session_id": child }))
        .await?
        .get("status")
        .and_then(Value::as_str)
        .context("child status is missing")?
        .to_string();
    let child_transcript = context.transcript(child).await?;
    Ok((
        status,
        common::validation_nudges(&child_transcript),
        failure_code(&child_transcript),
    ))
}

/// The `code` of the last `error` entry the Harness appended to a transcript.
fn failure_code(transcript: &Value) -> Option<String> {
    transcript
        .get("messages")?
        .as_array()?
        .iter()
        .rev()
        .find(|entry| entry.pointer("/custom/custom_type").and_then(Value::as_str) == Some("error"))?
        .pointer("/custom/data/code")?
        .as_str()
        .map(str::to_string)
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "recovery_record".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": [
                    "child_session_id", "child_status", "child_failure_code", "verdict",
                    "validation_nudges",
                    "validator_index", "wake_index", "spawn_index", "report"
                ],
                "properties": {
                    "child_session_id": { "type": "string", "minLength": 1 },
                    "child_status": { "type": "string" },
                    "child_failure_code": { "type": ["string", "null"] },
                    "verdict": {},
                    "validation_nudges": { "type": "integer", "minimum": 0 },
                    "validator_index": { "type": ["integer", "null"] },
                    "wake_index": { "type": ["integer", "null"] },
                    "spawn_index": { "type": ["integer", "null"] },
                    "report": { "type": "string" }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 32_768,
        }],
        invariants: ASSESSMENTS
            .iter()
            .map(|assessment| InvariantSpec {
                id: assessment.id().to_string(),
                description: assessment.description().to_string(),
            })
            .collect(),
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn table(run_id: &str) -> String {
    format!("fsubvtest_{}", suffix(run_id))
}

fn scope(run_id: &str) -> String {
    format!("fsubv-{}", suffix(run_id))
}

fn child_session(run_id: &str) -> String {
    format!("e2e_{run_id}-child-1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_that_does_not_fail_within_its_budget_leaves_the_run_incomplete() {
        let spec = SubagentValidationFailure.spec("run");
        let gates: Vec<&str> = spec
            .criteria
            .iter()
            .filter(|criterion| criterion.gate)
            .map(|criterion| criterion.id)
            .collect();
        assert_eq!(gates, ["bounded_failure", "expiry_report"]);
    }

    fn facts() -> Facts {
        Facts {
            child: "child".into(),
            child_status: "failed".into(),
            child_nudges: EXPECTED_NUDGES,
            child_failure_code: Some(CHILD_FAILURE_CODE.into()),
            verdict: Value::Null,
            validator_index: Some(2),
            wake_index: Some(3),
            spawn_index: Some(4),
            report: "CHILD GAVE UP: validation budget exhausted. PARENT DONE.".into(),
        }
    }

    fn awarded(facts: &Facts) -> Vec<Option<u8>> {
        facts
            .evaluation()
            .awards
            .iter()
            .map(|award| award.awarded)
            .collect()
    }

    #[test]
    fn the_bounded_give_up_earns_every_point() {
        assert_eq!(awarded(&facts()), [Some(40), Some(30), Some(30)]);
    }

    /// The null-agent canary: an agent that answers "done" and calls nothing
    /// spawns no child, so no criterion pays.
    #[test]
    fn an_agent_that_does_nothing_scores_zero() {
        let idle = Facts {
            child_status: "not_spawned".into(),
            child_nudges: 0,
            child_failure_code: None,
            validator_index: None,
            wake_index: None,
            spawn_index: None,
            report: "done".into(),
            ..facts()
        };
        assert_eq!(awarded(&idle), [Some(0), Some(0), Some(0)]);
    }

    #[test]
    fn a_child_failed_by_anything_but_its_output_contract_is_not_bounded() {
        for code in [None, Some("router/provider_transient".to_string())] {
            let other = Facts {
                child_failure_code: code,
                ..facts()
            };
            assert_eq!(awarded(&other)[0], Some(0));
            assert!(other.bounded_detail().contains(CHILD_FAILURE_CODE));
        }
    }

    #[test]
    fn the_failure_code_is_the_last_error_entry_of_the_transcript() {
        let error = |code: &str| json!({"custom": {"custom_type": "error", "data": {"code": code}}});
        let transcript = json!({"messages": [
            error("router/provider_transient"),
            {"message": {"role": "user"}},
            error(CHILD_FAILURE_CODE),
            {"custom": {"custom_type": "notice", "data": {"code": "other"}}},
        ]});
        assert_eq!(
            failure_code(&transcript).as_deref(),
            Some(CHILD_FAILURE_CODE)
        );
        assert_eq!(failure_code(&json!({"messages": []})), None);
    }

    #[tokio::test]
    async fn only_a_created_child_requires_live_evidence() {
        let context = E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let child = "expected-child";
        assert_eq!(
            child_evidence(&context, &json!({"messages":[]}), child)
                .await
                .unwrap(),
            ("not_spawned".into(), 0, None)
        );
        for is_error in [true, false] {
            let transcript = json!({"messages": [
                {"message": {"role":"assistant", "content":[{
                    "type":"function_call", "id":"spawn", "function_id":"harness::spawn",
                    "arguments":{"session_id":child}
                }]}},
                {"message":{"role":"function_result", "function_call_id":"spawn",
                    "function_id":"harness::spawn", "is_error":is_error, "details":{}}}
            ]});
            let evidence = child_evidence(&context, &transcript, child).await;
            if is_error {
                assert_eq!(evidence.unwrap(), ("not_spawned".into(), 0, None));
            } else {
                let error = evidence.unwrap_err();
                assert_eq!(
                    crate::context::transport_failure_code(&error),
                    Some("transport_not_connected")
                );
            }
        }
        context.shutdown().await;
    }
}
