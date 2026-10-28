//! `harness_contract_probe` — what `harness::send` refuses, and the two
//! sends it must accept. The subject answers "OK" twice on an unmetered
//! root: first with a 10,000,000-token output cap, then in a bare follow-up
//! that names no cap at all; both must complete with an output allocation
//! the model's catalog row can honour. Everything else is a direct send the
//! Harness must refuse with its pinned error, or one that fails its budget
//! preflight and so spends nothing: a wrong role, a read-only session, a
//! profile on an existing session, a reasoning preset beside a thinking
//! level, a missing model with and without a session id, a zero, a changed
//! and a late budget, and a structured message whose attachments must keep
//! their ids.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::common;
use super::probe::{self, Answer, Probes};
use super::{
    async_trait, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation, Scenario,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "harness_contract_probe";
const DELIVERABLE_ID: &str = "harness_contract_probes";
const PROMPT: &str = "Reply with exactly: OK";
const TEN_MILLION: u64 = 10_000_000;

const MESSAGE_REFUSALS: AssessmentSpec = AssessmentSpec::gate(
    "message_refusals",
    20,
    "Are an assistant-role message and a message to a read-only session refused with their pinned errors?",
)
.labeled("Wrong role and read-only session refused");
const SESSION_START_RULES: AssessmentSpec = AssessmentSpec::gate(
    "session_start_rules",
    25,
    "Are a profile on an existing session, a reasoning preset beside a thinking level, and a send with no model (with and without a session id, creating no session) refused as pinned?",
)
.labeled("Session-start rules refuse what they must");
const BUDGET_RULES: AssessmentSpec = AssessmentSpec::gate(
    "budget_rules",
    30,
    "Are a zero token budget, a changed frozen budget and a budget added to an unmetered session refused?",
)
.labeled("Budgets: no zero, frozen once set, never added late");
const OUTPUT_CAP: AssessmentSpec = AssessmentSpec::scored_in(
    "output_cap",
    15,
    "Do the 10,000,000-token and the omitted output cap both complete with an output allocation no larger than the model's maximum?",
    EvaluationDimension::Deliverable,
)
.labeled("Huge and omitted output caps both complete");
const ATTACHMENTS: AssessmentSpec = AssessmentSpec::scored(
    "attachments",
    10,
    "Does a structured message with a file and an image keep both attachment ids in the transcript?",
)
.labeled("Attachments keep their ids");
const ASSESSMENTS: &[AssessmentSpec] = &[
    MESSAGE_REFUSALS,
    SESSION_START_RULES,
    BUDGET_RULES,
    OUTPUT_CAP,
    ATTACHMENTS,
];

/// A probe session of this run: `e2e_<run>_<name>`, torn down in cleanup.
fn probe_session(run_id: &str, name: &str) -> String {
    format!("e2e_{run_id}_{name}")
}

const PROBE_SESSIONS: &[&str] = &["read_only", "unrouted", "zero_budget", "budgeted"];

/// What the root reported after the 10,000,000-token turn, before the bare
/// follow-up replaced it.
fn first_turn() -> &'static Mutex<HashMap<String, Value>> {
    static FIRST: OnceLock<Mutex<HashMap<String, Value>>> = OnceLock::new();
    FIRST.get_or_init(Default::default)
}

pub struct HarnessContractProbe;

#[async_trait]
impl Scenario for HarnessContractProbe {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Sends the Harness must refuse")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Answer OK twice, once under a 10,000,000-token output cap and once in a follow-up that names nothing; meanwhile the runner sends the Harness the messages, profiles, models and budgets it must refuse.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("harness::send refuses malformed and unsafe requests with a clear error before it creates anything or spends a token, and accepts any output cap by clamping it to the model. If it fails, clients start sessions they did not mean to, bypass budgets, or get turns that die on a provider limit.")
    }

    fn dialogue_followups(&self, _run_id: &str) -> Vec<String> {
        vec![PROMPT.to_string()]
    }

    fn bare_followup(&self, _followup: usize) -> bool {
        true
    }

    fn case(&self, seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "max_output_tokens": [TEN_MILLION, null],
                "metered": false,
                "probe_sessions": PROBE_SESSIONS,
            }),
            vec![Capability::E2eControlPlaneV1],
            probe::deliverable_contract(DELIVERABLE_ID, "harness_probes"),
        )
    }

    fn spec(&self, _run_id: &str) -> ScenarioSpec {
        ScenarioSpec {
            id: ID,
            prompt: PROMPT.to_string(),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(2),
                max_output_tokens: Some(TEN_MILLION),
                // Unmetered on purpose: a budget added later must be refused.
                max_total_tokens: None,
                stuck_timeout_seconds: 180,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    async fn before_followup(
        &self,
        context: &E2eContext,
        run_id: &str,
        session_id: &str,
        _exchange: usize,
    ) {
        let status = probe::call(
            context,
            "harness::status",
            json!({ "session_id": session_id, "verbose": true }),
        )
        .await
        .unwrap_or_else(|error| json!({ "error": error }));
        common::lock_unpoisoned(first_turn()).insert(run_id.to_string(), status);
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let root = observation.metrics.root_session_id.clone();
        let (model, provider) = probe::subject_route(&observation.transcript)
            .ok_or_else(|| anyhow::anyhow!("the root transcript names no model"))?;
        let mut probes = Probes::default();

        // The output caps: both turns completed, each fit to the model.
        let catalog = probe::call(
            context,
            "router::models::get",
            json!({ "id": model, "provider": provider }),
        )
        .await;
        let model_max = catalog
            .as_ref()
            .ok()
            .and_then(|row| row.pointer("/model/max_output_tokens"))
            .and_then(Value::as_u64);
        let first = common::lock_unpoisoned(first_turn())
            .remove(run_id)
            .unwrap_or(Value::Null);
        let last = probe::call(
            context,
            "harness::status",
            json!({ "session_id": root, "verbose": true }),
        )
        .await
        .unwrap_or_else(|error| json!({ "error": error }));
        for (id, status) in [("output.ten_million", &first), ("output.omitted", &last)] {
            probes.check(
                id,
                fits_the_model(status, model_max),
                json!({
                    "status": status.get("status"),
                    "effective_max_output_tokens": status.pointer("/context/effective_max_output_tokens"),
                    "model_max_output_tokens": model_max,
                }),
            );
        }

        // Messages.
        let assistant = probe::refused_send(
            context,
            json!({
                "session_id": root,
                "message": {
                    "role": "assistant", "content": [{ "type": "text", "text": "OK" }],
                    "stop_reason": "end", "model": model, "provider": provider, "timestamp": 1,
                },
            }),
        )
        .await;
        probes.check(
            "send.assistant_role",
            probe::refused_with(&assistant, "harness/invalid_message_role:"),
            &assistant,
        );
        let read_only = probe_session(run_id, "read_only");
        let ensured = probe::call(
            context,
            "session::ensure",
            json!({
                "session_id": read_only,
                "kind": "e2e",
                "title": "Harness E2E: harness_contract_probe read-only",
                "metadata": { "read_only": true, "e2e_run_id": run_id, "e2e_scenario": ID },
            }),
        )
        .await;
        let refused = probe::refused_send(
            context,
            json!({ "session_id": read_only, "message": PROMPT, "model": model, "provider": provider }),
        )
        .await;
        probes.check(
            "send.read_only",
            ensured.is_ok()
                && probe::refused_with(&refused, "is read-only and cannot accept messages"),
            json!({ "ensure": ensured, "send": refused }),
        );

        // Session-start rules.
        let agent = probe::refused_send(
            context,
            json!({ "session_id": root, "message": PROMPT, "options": { "agent": "e2e-any-profile" } }),
        )
        .await;
        probes.check(
            "send.agent_on_existing_session",
            probe::refused_with(&agent, "applies only when starting a session"),
            &agent,
        );
        let reasoning = probe::refused_send(
            context,
            json!({
                "session_id": root, "message": PROMPT,
                "options": { "reasoning": "lowest", "thinking_level": "low" },
            }),
        )
        .await;
        probes.check(
            "send.reasoning_with_thinking_level",
            probe::refused_with(&reasoning, "options.reasoning names the effort itself"),
            &reasoning,
        );
        let unrouted = probe_session(run_id, "unrouted");
        let named = probe::refused_send(
            context,
            json!({ "session_id": unrouted, "message": PROMPT }),
        )
        .await;
        let created = probe::call(context, "session::get", json!({ "session_id": unrouted })).await;
        probes.check(
            "send.new_session_id_without_model",
            probe::refused_with(
                &named,
                "inherits from the session's prior turn, but this session has none",
            ) && created.as_ref().is_ok_and(Value::is_null),
            json!({ "send": named, "session": created }),
        );
        let marker = json!({ "e2e_run_id": run_id, "e2e_probe": "no_session_id" });
        let anonymous = probe::refused_send(
            context,
            json!({ "message": PROMPT, "session": { "kind": "e2e", "metadata": marker } }),
        )
        .await;
        let listed = probe::call(
            context,
            "session::list",
            json!({ "kinds": ["e2e"], "metadata": marker, "limit": 5 }),
        )
        .await;
        probes.check(
            "send.no_session_id_without_model",
            probe::refused_with(&anonymous, "requires `model`") && lists_none(&listed),
            json!({ "send": anonymous, "sessions": listed }),
        );

        // Budgets.
        let zero = probe::refused_send(
            context,
            json!({
                "session_id": probe_session(run_id, "zero_budget"), "message": PROMPT,
                "model": model, "provider": provider,
                "session": { "kind": "e2e", "metadata": { "e2e_run_id": run_id, "e2e_scenario": ID } },
                "options": { "max_total_tokens": 0 },
            }),
        )
        .await;
        probes.check(
            "send.zero_budget",
            probe::refused_with(&zero, "max_total_tokens must be greater than zero"),
            &zero,
        );
        let late = probe::refused_send(
            context,
            json!({ "session_id": root, "message": PROMPT, "options": { "max_total_tokens": 50_000 } }),
        )
        .await;
        probes.check(
            "send.budget_on_unmetered_session",
            probe::refused_with(
                &late,
                "cannot add an execution budget to an existing unmetered session",
            ),
            &late,
        );
        budgeted_probes(context, &mut probes, run_id, &model, &provider).await;

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
            probe::award(
                MESSAGE_REFUSALS,
                &probes,
                &["send.assistant_role", "send.read_only"],
            ),
            probe::award(
                SESSION_START_RULES,
                &probes,
                &[
                    "send.agent_on_existing_session",
                    "send.reasoning_with_thinking_level",
                    "send.new_session_id_without_model",
                    "send.no_session_id_without_model",
                ],
            ),
            probe::award(
                BUDGET_RULES,
                &probes,
                &[
                    "send.zero_budget",
                    "send.frozen_budget",
                    "send.budget_on_unmetered_session",
                ],
            ),
            probe::award(
                OUTPUT_CAP,
                &probes,
                &["output.ten_million", "output.omitted"],
            ),
            probe::award(ATTACHMENTS, &probes, &["send.attachment_ids"]),
        ]))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        common::lock_unpoisoned(first_turn()).remove(run_id);
        for name in PROBE_SESSIONS {
            let _ = context.teardown(&probe_session(run_id, name)).await;
        }
        Ok(())
    }
}

/// A budgeted session whose every turn fails its preflight, so nothing here
/// spends a token: a send that changes the budget is refused, and one that
/// keeps it lands its attachments in the transcript before failing.
async fn budgeted_probes(
    context: &E2eContext,
    probes: &mut Probes,
    run_id: &str,
    model: &str,
    provider: &str,
) {
    let session_id = probe_session(run_id, "budgeted");
    let opened = probe::call(
        context,
        "harness::send",
        json!({
            "session_id": session_id, "message": PROMPT, "model": model, "provider": provider,
            "session": { "kind": "e2e", "metadata": { "e2e_run_id": run_id, "e2e_scenario": ID } },
            "options": { "max_turns": 1, "max_total_tokens": 1_000, "max_output_tokens": 4_096 },
        }),
    )
    .await;
    let settled = settle(context, &session_id, &opened).await;
    let changed = probe::refused_send(
        context,
        json!({ "session_id": session_id, "message": PROMPT, "options": { "max_total_tokens": 2_000 } }),
    )
    .await;
    probes.check(
        "send.frozen_budget",
        settled.is_some() && probe::refused_with(&changed, "execution budgets are frozen"),
        json!({ "first": opened, "first_status": settled, "changed": changed }),
    );

    let attached = probe::call(
        context,
        "harness::send",
        json!({
            "session_id": session_id,
            "message": {
                "role": "user", "timestamp": 1,
                "content": [
                    { "type": "text", "text": "Read the attached notes." },
                    { "type": "file", "attachment_id": "att_e2e_file", "name": "notes.txt",
                      "mime": "text/plain", "size": 5 },
                    { "type": "image", "mime": "image/png", "data": "iVBORw0KGgo=",
                      "attachment_id": "att_e2e_image" },
                ],
            },
        }),
    )
    .await;
    let settled = settle(context, &session_id, &attached).await;
    let kept = match context.transcript(&session_id).await {
        Ok(transcript) => attachment_ids(&transcript),
        Err(error) => vec![format!("transcript: {error:#}")],
    };
    let spent = probe::call(
        context,
        "harness::metrics",
        json!({ "root_session_id": session_id }),
    )
    .await
    .ok()
    .and_then(|metrics| metrics.get("totals").cloned());
    let unspent = spent.as_ref().is_some_and(|totals| {
        ["input_tokens", "output_tokens"]
            .iter()
            .all(|field| totals.get(field).and_then(Value::as_u64).unwrap_or(0) == 0)
    });
    probes.check(
        "send.attachment_ids",
        settled.is_some() && unspent && kept == ["att_e2e_file", "att_e2e_image"],
        json!({ "send": attached, "status": settled, "attachment_ids": kept, "totals": spent }),
    );
}

/// The status of the turn `accepted` started, once it settled.
async fn settle(context: &E2eContext, session_id: &str, accepted: &Answer) -> Option<Value> {
    let turn_id = accepted.as_ref().ok()?.get("turn_id")?.as_str()?;
    probe::settled_status(context, session_id, Some(turn_id), Duration::from_secs(60))
        .await
        .ok()
}

fn fits_the_model(status: &Value, model_max: Option<u64>) -> bool {
    let effective = status
        .pointer("/context/effective_max_output_tokens")
        .and_then(Value::as_u64);
    status.get("status").and_then(Value::as_str) == Some("completed")
        && matches!((effective, model_max), (Some(effective), Some(max)) if effective > 0 && effective <= max)
}

fn lists_none(listed: &Answer) -> bool {
    listed.as_ref().is_ok_and(|page| {
        page.get("sessions")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    })
}

/// Every attachment id the transcript's user messages carry, in order.
fn attachment_ids(transcript: &Value) -> Vec<String> {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("message"))
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .filter_map(|block| block.get("attachment_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::probe::Probe;
    use crate::scenarios::ScenarioId;

    #[test]
    fn an_output_cap_passes_only_when_the_turn_completed_within_the_model() {
        let status = |state: &str, effective: u64| json!({ "status": state, "context": { "effective_max_output_tokens": effective } });
        assert!(fits_the_model(&status("completed", 384_000), Some(384_000)));
        assert!(!fits_the_model(
            &status("completed", 10_000_000),
            Some(384_000)
        ));
        assert!(!fits_the_model(&status("failed", 1_000), Some(384_000)));
        assert!(!fits_the_model(&status("completed", 1_000), None));
        assert!(!fits_the_model(
            &json!({ "status": "completed" }),
            Some(384_000)
        ));
    }

    #[test]
    fn attachment_ids_come_from_user_messages_only() {
        let transcript = json!({ "messages": [
            { "message": { "role": "user", "content": [
                { "type": "text", "text": "x" },
                { "type": "file", "attachment_id": "a" },
                { "type": "image", "attachment_id": "b" },
            ] } },
            { "message": { "role": "assistant", "content": [{ "attachment_id": "c" }] } },
        ] });
        assert_eq!(attachment_ids(&transcript), ["a", "b"]);
        assert!(lists_none(&Ok(json!({ "sessions": [] }))));
        assert!(!lists_none(&Ok(json!({ "sessions": [{}] }))));
        assert!(!lists_none(&Err("down".into())));
    }

    #[tokio::test]
    async fn each_criterion_reads_its_own_probes() {
        let context = E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let ids = [
            "output.ten_million",
            "output.omitted",
            "send.assistant_role",
            "send.read_only",
            "send.agent_on_existing_session",
            "send.reasoning_with_thinking_level",
            "send.new_session_id_without_model",
            "send.no_session_id_without_model",
            "send.zero_budget",
            "send.budget_on_unmetered_session",
            "send.frozen_budget",
            "send.attachment_ids",
        ];
        let observe = |failing: Option<&str>| {
            let probes = ids
                .iter()
                .map(|id| Probe {
                    id: (*id).into(),
                    passed: Some(*id) != failing,
                    observed: Value::Null,
                })
                .collect();
            ScenarioObservation::for_test(
                ScenarioId::HarnessContractProbe,
                "control",
                json!({ "messages": [] }),
                "OK",
                vec![Probes(probes).deliverable(DELIVERABLE_ID, "harness_probes", "e2e_control")],
            )
        };
        let all = HarnessContractProbe
            .evaluate(&context, &observe(None), "control")
            .await
            .unwrap();
        let points: u16 = all
            .awards
            .iter()
            .filter_map(|award| award.awarded)
            .map(u16::from)
            .sum();
        assert_eq!(points, 100);
        let frozen = HarnessContractProbe
            .evaluate(&context, &observe(Some("send.frozen_budget")), "control")
            .await
            .unwrap();
        let budget = frozen
            .awards
            .iter()
            .find(|award| award.id == "budget_rules")
            .unwrap();
        assert_eq!(budget.awarded, Some(0));
        assert_eq!(
            assessment::completion(ASSESSMENTS, &frozen),
            crate::report::CompletionState::TaskIncomplete
        );
    }

    #[test]
    fn the_follow_up_is_bare_and_the_root_unmetered() {
        let spec = HarnessContractProbe.spec("run");
        assert_eq!(spec.execution.max_output_tokens, Some(TEN_MILLION));
        assert_eq!(spec.execution.max_total_tokens, None);
        assert!(crate::scenarios::sends_bare(&HarnessContractProbe, 1));
    }
}
