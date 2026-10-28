//! `session_continuity` — a send that names only the session keeps what the
//! session holds. Exchange 0 is a full send: model, provider, a dispatch
//! policy that allows one run-scoped fixture and `coder::read-file`, and an
//! attempt-owned `fs_scope` root holding `continuity.txt`. Then the runner
//! sends `{session_id, message, idempotency_key}` and nothing else, asking
//! for `coder::read-file continuity.txt`, the allowed fixture and a second
//! fixture outside the policy; the same key is resent while that turn runs
//! and after it ends. The last exchange is a bare runner follow-up.
//!
//! The keyed turn must run on the same model and provider, read the marker
//! through the inherited root, reach the allowed fixture and be refused the
//! other (`policy_denied`); both resends must be deduplicated onto its turn,
//! with one `e_idem_<key>` entry in the transcript.

use std::path::PathBuf;
use std::time::Duration;

use iii_sdk::RegisterFunction;
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::mechanism::{self, record, Check};
use super::{
    async_trait, common, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation,
    Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "session_continuity";
const DELIVERABLE_ID: &str = "session_continuity_checks";
const MARKER_FILE: &str = "continuity.txt";
const READ_FILE: &str = "coder::read-file";
const TURN_WAIT: Duration = Duration::from_secs(180);

const ROUTE: AssessmentSpec = AssessmentSpec::gate(
    "route_kept",
    25,
    "Did the keyed send that named only the session, and the bare follow-up, run on the model and provider of the first exchange?",
)
.labeled("A send naming only the session keeps its model");
const POLICY: AssessmentSpec = AssessmentSpec::gate(
    "policy_kept",
    25,
    "In the keyed turn, did the allowed fixture run while the fixture outside the first exchange's policy was refused as `policy_denied`?",
)
.labeled("The dispatch policy stays armed");
const FS_SCOPE: AssessmentSpec = AssessmentSpec::gate(
    "fs_scope_kept",
    20,
    "Did `coder::read-file continuity.txt` in the keyed turn read the marker from the first exchange's filesystem root?",
)
.labeled("The filesystem root carries over");
const IDEMPOTENT: AssessmentSpec = AssessmentSpec::gate(
    "idempotent_send",
    30,
    "Were the resends of the same key, during and after its turn, deduplicated onto that turn, with exactly one `e_idem_<key>` entry?",
)
.labeled("A resent key lands once");
const ASSESSMENTS: &[AssessmentSpec] = &[ROUTE, POLICY, FS_SCOPE, IDEMPOTENT];

fn allowed_function(run_id: &str) -> String {
    format!("e2etest::continuity_allowed_{}", mechanism::short(run_id))
}

fn denied_function(run_id: &str) -> String {
    format!("e2etest::continuity_denied_{}", mechanism::short(run_id))
}

fn code(run_id: &str, label: &str) -> String {
    let hex = super::stable_seed(&format!("{ID}:{run_id}")) & 0xff_ffff;
    format!("CONT-{hex:06x}-{label}")
}

fn root(run_id: &str) -> PathBuf {
    let base = std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let base = std::fs::canonicalize(&base).unwrap_or(base);
    base.join("scenario-workspaces")
        .join(format!("{ID}-{run_id}"))
}

fn idempotency_key(run_id: &str) -> String {
    format!("e2e:{run_id}:{ID}:keyed")
}

fn keyed_message(run_id: &str) -> String {
    let (allowed, denied) = (allowed_function(run_id), denied_function(run_id));
    format!(
        "Make these three calls one at a time, each exactly once: `{READ_FILE}` with \
         `{{\"path\": \"{MARKER_FILE}\"}}`, then `{allowed}` with `{{}}`, then `{denied}` with \
         `{{}}`. The last one may be refused: call it anyway and do not retry it. Then reply \
         with one line containing `ACK {}`.",
        code(run_id, "1")
    )
}

fn prompts(run_id: &str) -> [String; 2] {
    [0, 2].map(|exchange| {
        format!(
            "Reply with exactly `ACK {}` and nothing else. Do not call any function.",
            code(run_id, &exchange.to_string())
        )
    })
}

pub struct SessionContinuity;

#[async_trait]
impl Scenario for SessionContinuity {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Continue a session from its id alone")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("After one full exchange, sends that name only the session read a file, call an allowed and a refused fixture, and are resent with the same key.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("A client that continues a session by its id alone keeps its model, its permissions and its working directory, and a retried send lands once. If it fails, follow-up messages switch models, escape their policy or directory, or run twice.")
    }

    /// One retained cohort: the probes do not vary with a seed.
    fn case(&self, _seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            super::stable_seed(ID),
            json!({ "marker_file": MARKER_FILE }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::E2eRunScopedFixtures,
                Capability::E2eFilesystem,
                Capability::IiiFunctions,
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
            filesystem_root: Some(root(run_id)),
            execution: ExecutionPolicy {
                max_turns: Some(6),
                max_output_tokens: Some(2_048),
                // No token budget: a send naming only the session does not
                // keep `max_output_tokens`, so its preflight reserves the
                // model's whole output and a budget would refuse the turn.
                max_total_tokens: None,
                stuck_timeout_seconds: 240,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    fn dialogue_followups(&self, run_id: &str) -> Vec<String> {
        prompts(run_id).into_iter().skip(1).collect()
    }

    fn bare_followup(&self, _followup: usize) -> bool {
        true
    }

    fn required_functions(&self, run_id: &str) -> Vec<String> {
        vec![allowed_function(run_id), denied_function(run_id)]
    }

    fn allowed_functions(&self, run_id: &str) -> Option<Vec<String>> {
        Some(vec![allowed_function(run_id), READ_FILE.to_string()])
    }

    fn planted_state(&self, run_id: &str) -> Option<Value> {
        Some(json!({ "files": { MARKER_FILE: code(run_id, "file") } }))
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        mechanism::reset(run_id);
        let root = root(run_id);
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join(MARKER_FILE), code(run_id, "file"))?;
        for (function, token) in [
            (allowed_function(run_id), code(run_id, "allowed")),
            (denied_function(run_id), code(run_id, "denied")),
        ] {
            let registered = context.client().register_function(
                function,
                RegisterFunction::new_async(move |_: Value| {
                    let token = token.clone();
                    async move { Ok::<Value, iii_sdk::errors::Error>(json!({ "token": token })) }
                })
                .description("E2E run-scoped token: returns this attempt's token."),
            );
            mechanism::keep_function(run_id, registered);
        }
        Ok(())
    }

    async fn before_followup(
        &self,
        context: &E2eContext,
        run_id: &str,
        session_id: &str,
        _exchange: usize,
    ) {
        keyed_exchange(context, run_id, session_id).await;
    }

    async fn capture(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let keyed_turn = mechanism::noted(run_id, "keyed_turn")
            .and_then(|turn| turn.as_str().map(str::to_string))
            .unwrap_or_default();
        for check in checks(
            &observation.transcript,
            run_id,
            &keyed_turn,
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
        if !observation.metrics.complete {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "metrics_complete",
                "subject metrics are incomplete",
            ));
        }
        let calls = common::function_calls(&observation.transcript);
        let called = |function: &str| calls.iter().any(|call| call.function_id == function);
        // The policy and the root are read off the subject's calls: one that
        // skipped them says nothing about what the session kept.
        if !called(READ_FILE)
            || !called(&allowed_function(run_id))
            || !called(&denied_function(run_id))
        {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "keyed_calls",
                "the subject did not make the three calls of the keyed exchange",
            ));
        }
        let checks = mechanism::observed_checks(observation, run_id);
        Ok(assessment::build_evaluation([
            mechanism::grade(ROUTE, &checks, &["keyed_route", "bare_route"]),
            mechanism::grade(POLICY, &checks, &["allowed_ran", "outside_denied"]),
            mechanism::grade(FS_SCOPE, &checks, &["marker_read"]),
            mechanism::grade(
                IDEMPOTENT,
                &checks,
                &["dedup_during", "dedup_after", "one_idem_entry"],
            ),
        ]))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        common::remove_directory(&root(run_id))?;
        mechanism::release(context, run_id).await
    }
}

/// The send that names only the session and a key, resent while its turn
/// runs and after it ends.
async fn keyed_exchange(context: &E2eContext, run_id: &str, session_id: &str) {
    let send = json!({
        "session_id": session_id,
        "message": keyed_message(run_id),
        "idempotency_key": idempotency_key(run_id),
    });
    let first = context.trigger_value("harness::send", send.clone()).await;
    let turn = first
        .as_ref()
        .ok()
        .and_then(|response| response["turn_id"].as_str())
        .unwrap_or_default()
        .to_string();
    mechanism::note(run_id, "keyed_turn", json!(turn));
    let same_turn = |response: &anyhow::Result<Value>| {
        matches!(response, Ok(response) if response["deduplicated"] == true
            && response["turn_id"] == turn.as_str())
    };
    let during = context.trigger_value("harness::send", send.clone()).await;
    record(
        run_id,
        "dedup_during",
        !turn.is_empty() && same_turn(&during),
        format!("first={first:?} resend={during:?}"),
    );
    let ended = mechanism::poll(TURN_WAIT, || async {
        let status = context
            .trigger_value("harness::status", json!({ "session_id": session_id }))
            .await
            .ok()?;
        (status["turn_id"] == turn.as_str()
            && matches!(
                status["status"].as_str(),
                Some("completed" | "failed" | "cancelled")
            ))
        .then_some(status["status"].clone())
    })
    .await;
    let after = context.trigger_value("harness::send", send).await;
    record(
        run_id,
        "dedup_after",
        ended.is_some() && same_turn(&after),
        format!("turn {turn} ended {ended:?}; resend={after:?}"),
    );
}

fn checks(transcript: &Value, run_id: &str, keyed_turn: &str, response: &str) -> Vec<Check> {
    let check = |id: &str, passed: bool, detail: String| Check {
        id: id.to_string(),
        passed,
        detail,
    };
    let route_of = |entry: &Value| {
        (
            entry["message"]["model"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            entry["message"]["provider"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        )
    };
    let assistants: Vec<(String, (String, String))> = mechanism::entries(transcript)
        .filter(|entry| entry["message"]["role"] == "assistant")
        .map(|entry| {
            (
                entry["entry_id"].as_str().unwrap_or_default().to_string(),
                route_of(entry),
            )
        })
        .collect();
    let first = assistants.first().map(|(_, route)| route.clone());
    let keyed_prefix = format!("e_{keyed_turn}_");
    let keyed: Vec<&(String, String)> = assistants
        .iter()
        .filter(|(id, _)| !keyed_turn.is_empty() && id.starts_with(&keyed_prefix))
        .map(|(_, route)| route)
        .collect();
    let last = assistants
        .last()
        .filter(|(id, _)| !id.starts_with(&keyed_prefix));
    let outcomes = common::function_outcomes(transcript);
    let outcome = |function: String| outcomes.iter().find(|call| call.function_id == function);
    let allowed = outcome(allowed_function(run_id));
    let denied = outcome(denied_function(run_id));
    let read = mechanism::entries(transcript)
        .filter_map(|entry| entry.get("message"))
        .find(|message| message["role"] == "function_result" && message["function_id"] == READ_FILE)
        .cloned()
        .unwrap_or(Value::Null);
    let marker = code(run_id, "file");
    let idem = format!("e_idem_{}", mechanism::entry_safe(&idempotency_key(run_id)));
    let idem_entries: Vec<&Value> = mechanism::entries(transcript)
        .filter(|entry| entry["entry_id"] == idem.as_str())
        .collect();
    let bare_code = code(run_id, "2");
    vec![
        check(
            "keyed_route",
            first.is_some()
                && !keyed.is_empty()
                && keyed.iter().all(|route| Some(*route) == first.as_ref()),
            format!("first={first:?} keyed turn {keyed_turn}: {keyed:?}"),
        ),
        check(
            "bare_route",
            last.is_some_and(|(_, route)| Some(route) == first.as_ref())
                && response.contains(&bare_code),
            format!("last={last:?} reply={response:?}"),
        ),
        check(
            "allowed_ran",
            allowed.is_some_and(|call| call.is_error == Some(false)),
            format!("{:?}", allowed.map(|call| (&call.is_error, &call.details))),
        ),
        check(
            "outside_denied",
            denied.is_some_and(|call| {
                call.is_error == Some(true)
                    && call
                        .details
                        .as_ref()
                        .is_some_and(|details| details["error"] == "policy_denied")
            }),
            format!("{:?}", denied.map(|call| (&call.is_error, &call.details))),
        ),
        check(
            "marker_read",
            read["is_error"] == false && read["content"].to_string().contains(&marker),
            format!(
                "is_error={} contains {marker}={}",
                read["is_error"],
                read["content"].to_string().contains(&marker)
            ),
        ),
        check(
            "one_idem_entry",
            idem_entries.len() == 1
                && mechanism::text_of(&idem_entries[0]["message"]) == keyed_message(run_id),
            format!("{} `{idem}` entr(y/ies)", idem_entries.len()),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = "control";

    fn assistant(id: &str, model: &str) -> Value {
        json!({ "entry_id": id, "message": { "role": "assistant", "model": model, "provider": "p", "content": [] } })
    }

    fn result(function: String, is_error: bool, details: Value, text: &str) -> Value {
        json!({ "message": {
            "role": "function_result", "function_id": function, "function_call_id": "x",
            "is_error": is_error, "details": details, "content": [{ "type": "text", "text": text }],
        } })
    }

    fn call(id: &str, function: String) -> Value {
        json!({ "message": { "role": "assistant", "model": "m", "provider": "p", "content": [
            { "type": "function_call", "id": id, "function_id": function, "arguments": {} },
        ] } })
    }

    fn healthy() -> Value {
        let idem = format!("e_idem_{}", mechanism::entry_safe(&idempotency_key(RUN)));
        let marker = code(RUN, "file");
        json!({ "messages": [
            assistant("e_t_0_0_assistant", "m"),
            { "entry_id": idem, "message": { "role": "user", "content": [{ "type": "text", "text": keyed_message(RUN) }] } },
            assistant("e_t_1_0_assistant", "m"),
            call("c1", allowed_function(RUN)),
            { "message": { "role": "function_result", "function_id": allowed_function(RUN), "function_call_id": "c1", "is_error": false, "details": {} } },
            call("c2", denied_function(RUN)),
            { "message": { "role": "function_result", "function_id": denied_function(RUN), "function_call_id": "c2", "is_error": true, "details": { "error": "policy_denied" } } },
            result(READ_FILE.into(), false, json!({}), &marker),
            assistant("e_t_2_0_assistant", "m"),
        ] })
    }

    fn failing(transcript: &Value) -> Vec<String> {
        checks(transcript, RUN, "t_1", &format!("ACK {}", code(RUN, "2")))
            .into_iter()
            .filter(|check| !check.passed)
            .map(|check| check.id)
            .collect()
    }

    #[test]
    fn a_healthy_continuation_passes() {
        assert_eq!(failing(&healthy()), Vec::<String>::new());
    }

    #[test]
    fn a_switched_model_or_a_disarmed_policy_is_caught() {
        let mut transcript = healthy();
        transcript["messages"][2] = assistant("e_t_1_0_assistant", "other");
        assert_eq!(failing(&transcript), ["keyed_route"]);
        let mut transcript = healthy();
        transcript["messages"][6]["message"]["is_error"] = json!(false);
        transcript["messages"][6]["message"]["details"] = json!({ "token": "x" });
        assert_eq!(failing(&transcript), ["outside_denied"]);
    }

    #[test]
    fn a_lost_root_or_a_duplicate_key_is_caught() {
        let mut transcript = healthy();
        transcript["messages"][7] = result(READ_FILE.into(), true, json!({}), "no such file");
        assert_eq!(failing(&transcript), ["marker_read"]);
        let mut transcript = healthy();
        let duplicate = transcript["messages"][1].clone();
        transcript["messages"]
            .as_array_mut()
            .unwrap()
            .insert(2, duplicate);
        assert_eq!(failing(&transcript), ["one_idem_entry"]);
    }

    #[test]
    fn criteria_weights_total_one_hundred() {
        SessionContinuity.spec("run").validate().unwrap();
    }
}
