//! `turn_failure_surface` — how a turn the Harness refuses fails, at zero
//! tokens. The root turn starts with a 1,000-token budget and a 4,096-token
//! output cap, so the budget preflight refuses its first generation before
//! any provider call. The capture then checks every surface that failure
//! must reach (the turn status, one error entry, no usage, the session
//! status), that a second send cannot change the frozen budget, and that a
//! `pre_turn` hook's veto fails a probe session's turn the same way.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use iii_sdk::protocol::RegisterTriggerInput;
use iii_sdk::runtime::FunctionRef;
use iii_sdk::RegisterFunction;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;
use crate::wire::TurnStatus;

use super::assessment::{self, AssessmentSpec};
use super::common;
use super::probe::{self, Probes};
use super::validation_loop::suffix;
use super::{
    async_trait, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation, Scenario,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "turn_failure_surface";
const DELIVERABLE_ID: &str = "turn_failure_probes";
const MAX_TOTAL_TOKENS: u64 = 1_000;
const MAX_OUTPUT_TOKENS: u64 = 4_096;
const PRE_TURN: &str = "harness::hook::pre-turn";
const DENY_REASON: &str = "turn_failure_surface vetoes this turn";

const PREFLIGHT_REFUSED: AssessmentSpec = AssessmentSpec::gate_in(
    "preflight_refused",
    40,
    "Did the budget preflight fail the root turn before generation: status failed, one error entry coded harness.budget_exceeded in phase budget_preflight and not retryable, and no tokens used?",
    EvaluationDimension::Deliverable,
)
.labeled("Over-budget turn fails before any generation");
const FAILURE_PROJECTED: AssessmentSpec = AssessmentSpec::scored(
    "failure_projected",
    15,
    "Does the root session's own status read error after the failed turn?",
)
.labeled("Session status reads error");
const BUDGET_FROZEN: AssessmentSpec = AssessmentSpec::scored(
    "budget_frozen",
    15,
    "Is a second send naming a different budget refused because execution budgets are frozen?",
)
.labeled("A later send cannot change the budget");
const PRE_TURN_DENIED: AssessmentSpec = AssessmentSpec::gate(
    "pre_turn_denied",
    30,
    "Does a pre_turn hook's deny fail the turn as harness.pre_turn_denied with its reason, before any generation?",
)
.labeled("A pre_turn veto fails the turn unspent");
const ASSESSMENTS: &[AssessmentSpec] = &[
    PREFLIGHT_REFUSED,
    FAILURE_PROJECTED,
    BUDGET_FROZEN,
    PRE_TURN_DENIED,
];

fn deny_hook_id(run_id: &str) -> String {
    format!("e2etest::pre_turn_deny_{}", suffix(run_id))
}

/// The session whose turn the `pre_turn` hook vetoes; the hook is bound to
/// it alone.
fn vetoed_session(run_id: &str) -> String {
    format!("e2e_{run_id}_pre_turn")
}

pub struct TurnFailureSurface;

#[async_trait]
impl Scenario for TurnFailureSurface {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("A refused turn fails cleanly, unspent")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Start a turn whose token budget cannot fit one generation and one a pre_turn hook vetoes; both must fail before the provider is called, and say why everywhere a client looks.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("A turn the Harness refuses (over its budget, or vetoed by a hook) fails at once, spends nothing, and reports the reason in its status, its transcript and its session. If it fails, budgets stop protecting spend, or a refused run looks stuck or silently succeeds.")
    }

    fn expected_root_outcome(&self) -> TurnStatus {
        TurnStatus::Failed
    }

    fn required_functions(&self, run_id: &str) -> Vec<String> {
        vec![deny_hook_id(run_id)]
    }

    fn case(&self, seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "max_total_tokens": MAX_TOTAL_TOKENS,
                "max_output_tokens": MAX_OUTPUT_TOKENS,
                "expected_root_outcome": "failed",
                "pre_turn_deny_reason": DENY_REASON,
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::E2eRunScopedFixtures,
            ],
            probe::deliverable_contract(DELIVERABLE_ID, "harness_probes"),
        )
    }

    fn spec(&self, _run_id: &str) -> ScenarioSpec {
        ScenarioSpec {
            id: ID,
            prompt: "Reply with exactly: OK".to_string(),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(1),
                max_output_tokens: Some(MAX_OUTPUT_TOKENS),
                // Below the output cap on purpose: the preflight must refuse it.
                max_total_tokens: Some(MAX_TOTAL_TOKENS),
                stuck_timeout_seconds: 120,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        // The hook is bound only while its probe runs; the function the
        // binding names must exist first.
        let function = context.client().register_function(
            deny_hook_id(run_id),
            RegisterFunction::new_async(move |_input: Value| async move {
                Ok::<Value, iii_sdk::errors::Error>(
                    json!({ "decision": "deny", "reason": DENY_REASON }),
                )
            })
            .description("E2E turn_failure_surface: vetoes every turn it is bound to.")
            .metadata(json!({ "internal": true })),
        );
        common::lock_unpoisoned(fixtures()).insert(run_id.to_string(), function);
        Ok(())
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let root = observation.metrics.root_session_id.clone();
        let mut probes = Probes::default();

        let status = context
            .trigger_value(
                "harness::status",
                json!({ "session_id": root, "verbose": true }),
            )
            .await?;
        let turn_id = status
            .get("turn_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let error = probe::custom_entry(&observation.transcript, &format!("e_{turn_id}_error"));
        let totals = &observation.metrics.totals;
        probes.check(
            "root.preflight",
            status.get("status").and_then(Value::as_str) == Some("failed")
                && status
                    .get("result_error")
                    .and_then(Value::as_str)
                    .is_some_and(|reason| reason.contains("budget"))
                && error.is_some_and(is_preflight_refusal)
                && error_entries(&observation.transcript) == 1
                && totals.input_tokens.unwrap_or(0) == 0
                && totals.output_tokens.unwrap_or(0) == 0,
            json!({
                "status": status.get("status"),
                "result_error": status.get("result_error"),
                "error_entry": error,
                "error_entries": error_entries(&observation.transcript),
                "input_tokens": totals.input_tokens,
                "output_tokens": totals.output_tokens,
            }),
        );

        let session = probe::call(context, "session::get", json!({ "session_id": root })).await;
        probes.check(
            "root.session_status",
            session
                .as_ref()
                .is_ok_and(|session| session.pointer("/meta/status") == Some(&json!("error"))),
            &session,
        );

        let frozen = probe::refused_send(
            context,
            json!({
                "session_id": root,
                "message": "Reply with exactly: OK",
                "options": {
                    "max_total_tokens": MAX_TOTAL_TOKENS * 2,
                    "max_output_tokens": MAX_OUTPUT_TOKENS / 4,
                },
            }),
        )
        .await;
        probes.check(
            "root.budget_frozen",
            probe::refused_with(&frozen, "execution budgets are frozen"),
            &frozen,
        );

        let vetoed = veto_probe(context, observation, run_id).await;
        probes.check(
            "pre_turn.denied",
            vetoed.as_ref().is_ok_and(is_pre_turn_veto),
            vetoed.unwrap_or_else(|error| json!({ "error": format!("{error:#}") })),
        );

        Ok(vec![probes.deliverable(
            DELIVERABLE_ID,
            "harness_probes",
            &root,
        )])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        _run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        let Some(probes) = probe::captured(observation, DELIVERABLE_ID) else {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "probes_captured",
                "the capture recorded no probes",
            ));
        };
        Ok(assessment::build_evaluation([
            probe::award(PREFLIGHT_REFUSED, &probes, &["root.preflight"]),
            probe::award(FAILURE_PROJECTED, &probes, &["root.session_status"]),
            probe::award(BUDGET_FROZEN, &probes, &["root.budget_frozen"]),
            probe::award(PRE_TURN_DENIED, &probes, &["pre_turn.denied"]),
        ]))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        if let Some(function) = common::lock_unpoisoned(fixtures()).remove(run_id) {
            function.unregister();
        }
        let _ = context.teardown(&vetoed_session(run_id)).await;
        Ok(())
    }
}

fn fixtures() -> &'static Mutex<HashMap<String, FunctionRef>> {
    static FIXTURES: OnceLock<Mutex<HashMap<String, FunctionRef>>> = OnceLock::new();
    FIXTURES.get_or_init(Default::default)
}

fn error_entries(transcript: &Value) -> usize {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .map_or(0, |entries| {
            entries
                .iter()
                .filter(|entry| entry.pointer("/custom/custom_type") == Some(&json!("error")))
                .count()
        })
}

fn is_preflight_refusal(error: &Value) -> bool {
    error.get("code") == Some(&json!("harness.budget_exceeded"))
        && error.get("phase") == Some(&json!("budget_preflight"))
        && error.get("retryable") == Some(&json!(false))
}

/// The vetoed session's turn, as `veto_probe` saw it settle.
fn is_pre_turn_veto(seen: &Value) -> bool {
    seen.pointer("/status/status") == Some(&json!("failed"))
        && seen
            .pointer("/status/result_error")
            .and_then(Value::as_str)
            .is_some_and(|reason| reason.contains(DENY_REASON))
        && seen.pointer("/error/code") == Some(&json!("harness.pre_turn_denied"))
        && seen.pointer("/error/phase") == Some(&json!("pre_turn"))
        && seen
            .pointer("/totals/input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            == 0
        && seen
            .pointer("/totals/output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            == 0
}

/// Binds the deny hook to a fresh session only, starts a turn there on the
/// subject's route and reads how it settled; the binding goes away whatever
/// the probe saw.
async fn veto_probe(
    context: &E2eContext,
    observation: &ScenarioObservation,
    run_id: &str,
) -> anyhow::Result<Value> {
    let (model, provider) = probe::subject_route(&observation.transcript)
        .ok_or_else(|| anyhow::anyhow!("the root transcript names no model"))?;
    let session_id = vetoed_session(run_id);
    let binding = context
        .client()
        .register_trigger(RegisterTriggerInput::new(
            PRE_TURN,
            deny_hook_id(run_id),
            json!({ "sessions": [session_id], "timeout_ms": 5000 }),
        ))?;
    let seen = async {
        // The engine hands the binding to the Harness asynchronously.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let accepted = context
            .trigger_value(
                "harness::send",
                json!({
                    "session_id": session_id,
                    "message": "Reply with exactly: OK",
                    "model": model,
                    "provider": provider,
                    "session": {
                        "kind": "e2e",
                        "title": "Harness E2E: turn_failure_surface pre_turn veto",
                        "metadata": { "e2e_run_id": run_id, "e2e_scenario": ID },
                    },
                    "options": { "max_turns": 1, "max_output_tokens": 256 },
                }),
            )
            .await?;
        let turn_id = accepted
            .get("turn_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let status =
            probe::settled_status(context, &session_id, Some(&turn_id), Duration::from_secs(60))
                .await?;
        let transcript = context.transcript(&session_id).await?;
        let metrics = context
            .trigger_value("harness::metrics", json!({ "root_session_id": session_id }))
            .await?;
        anyhow::Ok(json!({
            "status": { "status": status.get("status"), "result_error": status.get("result_error") },
            "error": probe::custom_entry(&transcript, &format!("e_{turn_id}_error")),
            "totals": metrics.get("totals"),
        }))
    }
    .await;
    binding.unregister();
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::probe::Probe;
    use crate::scenarios::ScenarioId;

    fn observation(probes: Vec<Probe>) -> ScenarioObservation {
        ScenarioObservation::for_test(
            ScenarioId::TurnFailureSurface,
            "control",
            json!({ "messages": [] }),
            "",
            vec![Probes(probes).deliverable(DELIVERABLE_ID, "harness_probes", "e2e_control")],
        )
    }

    fn passing(id: &str) -> Probe {
        Probe {
            id: id.into(),
            passed: true,
            observed: Value::Null,
        }
    }

    #[tokio::test]
    async fn every_probe_passing_completes_and_a_failed_gate_does_not() {
        let context = E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let ids = [
            "root.preflight",
            "root.session_status",
            "root.budget_frozen",
            "pre_turn.denied",
        ];
        let all = observation(ids.iter().map(|id| passing(id)).collect());
        let evaluation = TurnFailureSurface
            .evaluate(&context, &all, "control")
            .await
            .unwrap();
        assert!(evaluation
            .awards
            .iter()
            .all(|award| award.awarded.is_some_and(|points| points > 0)));
        assert_eq!(
            assessment::completion(ASSESSMENTS, &evaluation),
            crate::report::CompletionState::Completed
        );

        let mut probes: Vec<Probe> = ids.iter().map(|id| passing(id)).collect();
        probes[3].passed = false;
        let vetoed = TurnFailureSurface
            .evaluate(&context, &observation(probes), "control")
            .await
            .unwrap();
        assert_eq!(
            assessment::completion(ASSESSMENTS, &vetoed),
            crate::report::CompletionState::TaskIncomplete
        );
    }

    #[test]
    fn the_preflight_refusal_and_the_veto_are_read_from_their_error_entries() {
        let refusal = json!({
            "code": "harness.budget_exceeded", "phase": "budget_preflight", "retryable": false,
        });
        assert!(is_preflight_refusal(&refusal));
        assert!(!is_preflight_refusal(&json!({
            "code": "harness.budget_exceeded", "phase": "budget_preflight", "retryable": true,
        })));
        let veto = json!({
            "status": { "status": "failed", "result_error": format!("pre_turn hook denied: {DENY_REASON}") },
            "error": { "code": "harness.pre_turn_denied", "phase": "pre_turn" },
            "totals": { "input_tokens": null, "output_tokens": 0 },
        });
        assert!(is_pre_turn_veto(&veto));
        let mut spent = veto.clone();
        spent["totals"]["input_tokens"] = json!(12);
        assert!(!is_pre_turn_veto(&spent));
    }

    #[test]
    fn the_root_budget_cannot_fit_its_own_output_cap() {
        let spec = TurnFailureSurface.spec("run");
        assert!(spec.execution.max_total_tokens < spec.execution.max_output_tokens);
        assert_eq!(
            TurnFailureSurface.expected_root_outcome(),
            TurnStatus::Failed
        );
        ScenarioId::TurnFailureSurface
            .materialize("run", ScenarioId::TurnFailureSurface.canonical_seed())
            .unwrap();
    }
}
