//! Mechanism gates: what the Harness itself must have done on the scenarios
//! that exercise a mechanism (compaction, spawning, wakes, dispatch policy,
//! validators, the invocation chokepoint, filesystem stamping), whatever the
//! model delivered. `execute` arms the observers a scenario needs before the
//! prompt and runs its probes before teardown; a broken gate is a Harness
//! invariant violation, so it leaves a passed run incomplete instead of only
//! reporting like the advisory audit.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use iii_sdk::protocol::RegisterTriggerInput;
use iii_sdk::runtime::FunctionRef;
use iii_sdk::trigger::Trigger;
use iii_sdk::RegisterFunction;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::harness_invariants::{HarnessInvariant, HarnessViolation};
use crate::report::E2eRunReport;
use crate::scenarios::common::lock_unpoisoned;
use crate::wire::SessionMetricsResponse;

mod agent_profile;
mod contention_ledger;
mod context_pressure;
mod onboarding;
mod persistent_state;
mod shell_coder_sandbox;
mod timer_wake;
mod tool_contract_recovery;
mod validation_self_repair;

#[cfg(test)]
pub(crate) use context_pressure::TEMPLATE_HEADINGS;
pub(crate) use context_pressure::{
    compactions, measurements as context_measurements, summary_template,
};

/// The line the `pre_generate` probe adds to the scenario's prompt, which
/// makes it part of what the subject is asked (and so of its digest).
pub fn prompt_append(scenario: &str, run_id: &str) -> Option<String> {
    (scenario == persistent_state::ID).then(|| persistent_state::marker(run_id))
}

const PRE_GENERATE: &str = "harness::hook::pre-generate";
const PRE_TRIGGER: &str = "harness::hook::pre-trigger";
/// A probe the chokepoint must answer quickly: a wedged session lock is a
/// broken chokepoint, not a slow one.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// The call ids `invoke_as` gives its probes, which the observers tell apart
/// from the model's own calls by.
const PROBE_CALL_PREFIX: &str = "e2e_probe_";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MechanismEvidence {
    pub scenario: String,
    /// What the runner observed or probed for this scenario's gates, by name.
    /// A gate whose probe is absent is not judged: a replayed run has none.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub probes: BTreeMap<String, Value>,
}

/// One broken gate: its id and what was observed.
type Broken = (&'static str, String);

/// What a gate reads: the root transcript, the tree's metrics and the probes.
struct Run<'a> {
    transcript: &'a Value,
    metrics: Option<&'a SessionMetricsResponse>,
    probes: &'a BTreeMap<String, Value>,
}

impl Run<'_> {
    fn probe(&self, name: &str) -> Option<&Value> {
        self.probes.get(name)
    }
}

/// The broken gates of the run's scenario.
pub fn violations(report: &E2eRunReport) -> Vec<HarnessViolation> {
    let Some(evidence) = report.mechanism_evidence.as_ref() else {
        return Vec::new();
    };
    let run = Run {
        transcript: report.transcript.as_ref().unwrap_or(&Value::Null),
        metrics: report.metrics.as_ref(),
        probes: &evidence.probes,
    };
    let mut broken = match evidence.scenario.as_str() {
        context_pressure::ID => context_pressure::gates(&run),
        contention_ledger::ID => contention_ledger::gates(&run),
        timer_wake::ID => timer_wake::gates(&run),
        tool_contract_recovery::ID => tool_contract_recovery::gates(&run),
        validation_self_repair::ID => validation_self_repair::gates(&run),
        persistent_state::ID => persistent_state::gates(&run),
        shell_coder_sandbox::ID => shell_coder_sandbox::gates(&run),
        onboarding::ID => onboarding::gates(&run),
        _ => Vec::new(),
    };
    broken.extend(agent_profile::gates(&run));
    broken
        .into_iter()
        .map(|(gate, detail)| HarnessViolation {
            invariant: HarnessInvariant::MechanismGate,
            detail: format!("{gate}: {detail}"),
        })
        .collect()
}

/// One `pre_generate` call the observer saw for the root session.
#[derive(Debug, Clone, Default)]
struct Prompt {
    turn_id: String,
    step: u64,
    system_prompt: String,
    /// Function results the context-manager's emergency pass reduced.
    emergency_references: usize,
    /// Whether the messages replay the probe's earlier append.
    replays_append: bool,
}

/// The observers armed for one attempt; dropping it unbinds them, so an
/// attempt that fails anywhere leaves nothing behind on the stack.
pub struct Armed {
    scenario: String,
    agent: Option<String>,
    prompts: Arc<Mutex<Vec<Prompt>>>,
    calls: Arc<Mutex<Vec<Value>>>,
    resend: Option<Value>,
    functions: Vec<FunctionRef>,
    triggers: Vec<Trigger>,
}

impl Drop for Armed {
    fn drop(&mut self) {
        for trigger in self.triggers.drain(..) {
            trigger.unregister();
        }
        for function in self.functions.drain(..) {
            function.unregister();
        }
    }
}

impl Armed {
    /// Whether the first send is to be resent while its turn runs.
    pub fn resends_first_send(&self) -> bool {
        self.scenario == persistent_state::ID
    }

    /// Resends the first exchange's request with its idempotency key while
    /// the turn it started runs, and keeps the reply.
    pub async fn resend<I: Serialize + Send>(
        &mut self,
        context: &E2eContext,
        first_turn_id: &str,
        request: I,
    ) {
        let reply = context.trigger::<_, Value>("harness::send", request).await;
        self.resend = Some(json!({
            "first_turn_id": first_turn_id,
            "reply": reply.unwrap_or_else(|error| json!({ "error": format!("{error:#}") })),
        }));
    }
}

/// Binds the observers the scenario's gates read, before the prompt is sent.
pub async fn arm(
    context: &E2eContext,
    scenario: &str,
    session_id: &str,
    run_id: &str,
    agent: Option<&str>,
) -> Result<Armed> {
    let agent = agent_profile::gated(scenario, agent);
    let mut armed = Armed {
        scenario: scenario.into(),
        agent: agent.map(str::to_string),
        prompts: Arc::default(),
        calls: Arc::default(),
        resend: None,
        functions: Vec::new(),
        triggers: Vec::new(),
    };
    let append = prompt_append(scenario, run_id);
    if agent.is_some()
        || append.is_some()
        || [context_pressure::ID, onboarding::ID].contains(&scenario)
    {
        let prompts = armed.prompts.clone();
        let session = session_id.to_string();
        let function_id = format!("e2e::mechanism::pre-generate::{run_id}");
        armed.functions.push(
            context.client().register_function(
                function_id.clone(),
                RegisterFunction::new_async(move |input: Value| {
                    let prompts = prompts.clone();
                    let session = session.clone();
                    let append = append.clone();
                    async move {
                        Ok::<Value, iii_sdk::errors::Error>(observe_prompt(
                            &prompts,
                            &session,
                            append.as_deref(),
                            &input,
                        ))
                    }
                })
                .description("E2E mechanism gates: records the root session's prompts.")
                .metadata(json!({"internal": true})),
            ),
        );
        // Last in the chain, and open on error: it never stops a turn. The
        // Harness has no session filter for `pre_generate`, so every step on
        // the stack makes this round trip while the attempt runs; it is armed
        // only for the scenarios whose gates read prompts.
        armed.triggers.push(
            context
                .client()
                .register_trigger(RegisterTriggerInput::new(
                    PRE_GENERATE,
                    function_id,
                    json!({"priority": 1000, "timeout_ms": 2000, "on_error": "fail_open"}),
                ))?,
        );
    }
    if scenario == shell_coder_sandbox::ID {
        let calls = armed.calls.clone();
        let session = session_id.to_string();
        let function_id = format!("e2e::mechanism::pre-trigger::{run_id}");
        armed.functions.push(
            context.client().register_function(
                function_id.clone(),
                RegisterFunction::new_async(move |input: Value| {
                    let calls = calls.clone();
                    let session = session.clone();
                    async move {
                        if input["session_id"] == session.as_str() {
                            lock_unpoisoned(&calls).push(json!({
                                "call_id": input["call"]["id"],
                                "function_id": input["call"]["function_id"],
                                "fs_scope": input["call"]["arguments"]["fs_scope"],
                                "command": input["call"]["arguments"]["command"],
                            }));
                        }
                        Ok::<Value, iii_sdk::errors::Error>(json!({"decision": "continue"}))
                    }
                })
                .description("E2E mechanism gates: records the stamped filesystem scope.")
                .metadata(json!({"internal": true})),
            ),
        );
        armed.triggers.push(
            context
                .client()
                .register_trigger(RegisterTriggerInput::new(
                    PRE_TRIGGER,
                    function_id,
                    json!({
                        "functions": shell_coder_sandbox::OBSERVED,
                        "priority": 1000, "timeout_ms": 2000, "on_error": "fail_open",
                    }),
                ))?,
        );
    }
    Ok(armed)
}

/// Records the prompt `pre_generate` hands the root session's step, and
/// appends the probe's message when asked.
fn observe_prompt(
    prompts: &Mutex<Vec<Prompt>>,
    session_id: &str,
    append: Option<&str>,
    input: &Value,
) -> Value {
    if input["session_id"] != session_id {
        return json!({"decision": "continue"});
    }
    let messages = input["generate"]["messages"].to_string();
    let mut prompts = lock_unpoisoned(prompts);
    let first = prompts.is_empty();
    prompts.push(Prompt {
        turn_id: input["turn_id"].as_str().unwrap_or_default().into(),
        step: input["step"].as_u64().unwrap_or_default(),
        system_prompt: input["generate"]["system_prompt"]
            .as_str()
            .unwrap_or_default()
            .into(),
        emergency_references: messages
            .matches(context_pressure::EMERGENCY_REFERENCE)
            .count(),
        replays_append: append.is_some_and(|text| messages.contains(text)),
    });
    match append {
        // Appended once, on the first step: every later step must replay it
        // from the one notice the Harness persisted.
        Some(text) if first => json!({
            "decision": "continue",
            "mutations": {"append_messages": [{
                "role": "user",
                "content": [{"type": "text", "text": text}],
                "timestamp": 0,
            }]},
            "annotations": {persistent_state::ANNOTATION: true},
        }),
        _ => json!({"decision": "continue"}),
    }
}

/// What `capture` needs about the attempt besides the stack.
pub struct Attempt<'a> {
    pub run_id: &'a str,
    pub session_id: &'a str,
    pub filesystem_root: Option<&'a std::path::Path>,
    pub transcript: &'a Value,
    pub metrics: &'a SessionMetricsResponse,
    pub terminal_status: Option<Value>,
}

/// Runs the scenario's probes before teardown and gathers what the
/// observers saw. `None` when nothing about the scenario is gated.
pub async fn capture(
    context: &E2eContext,
    armed: Armed,
    attempt: &Attempt<'_>,
) -> Option<MechanismEvidence> {
    let prompts = std::mem::take(&mut *lock_unpoisoned(&armed.prompts));
    let mut probes = BTreeMap::new();
    match armed.scenario.as_str() {
        context_pressure::ID => context_pressure::capture(attempt, &prompts, &mut probes),
        contention_ledger::ID => contention_ledger::capture(context, attempt, &mut probes).await,
        tool_contract_recovery::ID => {
            tool_contract_recovery::capture(context, attempt, &mut probes).await
        }
        validation_self_repair::ID => {
            validation_self_repair::capture(context, attempt, &mut probes).await
        }
        persistent_state::ID => {
            persistent_state::capture(
                context,
                attempt,
                armed.resend.clone(),
                &prompts,
                &mut probes,
            )
            .await
        }
        shell_coder_sandbox::ID => {
            shell_coder_sandbox::capture(context, attempt, &armed.calls, &mut probes).await
        }
        onboarding::ID => onboarding::capture(&prompts, &mut probes),
        timer_wake::ID => {}
        _ if armed.agent.is_none() => return None,
        _ => {}
    }
    if let Some(agent) = armed.agent.as_deref() {
        agent_profile::capture(context, attempt, agent, &prompts, &mut probes).await;
    }
    Some(MechanismEvidence {
        scenario: armed.scenario.clone(),
        probes,
    })
}

/// A probe's reply, or the error the stack answered with: a Harness that
/// errs where it should answer is a broken gate, not an infrastructure
/// failure to re-run.
fn answer(reply: Result<Value>) -> Value {
    reply.unwrap_or_else(|error| json!({"probe_error": format!("{error:#}")}))
}

/// `function_id` asked directly, its error kept as the answer.
async fn ask(context: &E2eContext, function_id: &str, payload: Value) -> Value {
    answer(context.trigger_value(function_id, payload).await)
}

/// `harness::function::trigger` as `session_id`: the same pipeline a turn's
/// call goes through, policy and chokepoint included. A timeout or an error
/// is kept as the probe's answer.
async fn invoke_as(
    context: &E2eContext,
    session_id: &str,
    function_id: &str,
    arguments: Value,
) -> Value {
    let call = json!({
        "session_id": session_id,
        "call": {"id": format!("{PROBE_CALL_PREFIX}{}", uuid::Uuid::new_v4().simple()),
                 "function_id": function_id, "arguments": arguments},
    });
    match tokio::time::timeout(
        PROBE_TIMEOUT,
        context.trigger_value("harness::function::trigger", call),
    )
    .await
    {
        Ok(reply) => answer(reply),
        Err(_) => json!({"timed_out": true}),
    }
}

/// The error code a result carries: `policy_denied`, a worker's own code.
fn error_code(result: &Value) -> Option<&str> {
    [
        "/details/error",
        "/details/code",
        "/details/error/code",
        "/error_code",
    ]
    .iter()
    .find_map(|pointer| result.pointer(pointer)?.as_str())
}

fn denied(result: &Value) -> bool {
    error_code(result) == Some("policy_denied")
}

fn messages(transcript: &Value) -> &[Value] {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn text<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

/// The text blocks of an entry's message, joined.
fn message_text(entry: &Value) -> String {
    entry
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect()
}

/// Custom entries of `custom_type`, in transcript order.
fn customs<'a>(transcript: &'a Value, custom_type: &'a str) -> impl Iterator<Item = &'a Value> {
    messages(transcript)
        .iter()
        .filter(move |entry| text(entry, "/custom/custom_type") == Some(custom_type))
}

/// `(turn_id, step)` of a step entry `e_<turn>_<step>_<kind>`.
fn turn_step(entry_id: &str) -> Option<(&str, u64)> {
    let rest = entry_id.strip_prefix("e_")?;
    let (head, _kind) = rest.rsplit_once('_')?;
    let (turn, step) = head.rsplit_once('_')?;
    Some((turn, step.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_probe_the_stack_errs_on_answers_with_the_error() {
        let reply = answer(Err(anyhow::anyhow!("session lock poisoned")));

        assert_eq!(reply["probe_error"], "session lock poisoned");
        assert!(!denied(&reply));
    }

    #[test]
    fn step_entry_ids_name_their_turn_and_step() {
        assert_eq!(
            turn_step("e_t_bfd8cf6b74114be0a805115a4492d028_30_compaction"),
            Some(("t_bfd8cf6b74114be0a805115a4492d028", 30))
        );
        assert_eq!(turn_step("e_t_x_skills_1"), None);
    }

    #[test]
    fn the_observer_records_only_the_root_session_and_counts_emergency_references() {
        let prompts = Mutex::new(Vec::new());
        let input = |session: &str| {
            json!({"session_id": session, "turn_id": "t1", "step": 2, "generate": {
                "system_prompt": "base",
                "messages": [{"content": [{"type": "text",
                    "text": format!("{} {{}}", context_pressure::EMERGENCY_REFERENCE)}]}],
            }})
        };

        let other = observe_prompt(&prompts, "root", Some("probe"), &input("other"));
        let first = observe_prompt(&prompts, "root", Some("probe"), &input("root"));
        let later = observe_prompt(&prompts, "root", Some("probe"), &input("root"));

        assert_eq!(other, json!({"decision": "continue"}));
        assert_eq!(
            first["mutations"]["append_messages"][0]["content"][0]["text"],
            "probe"
        );
        assert_eq!(later, json!({"decision": "continue"}));
        let prompts = prompts.into_inner().unwrap();
        assert_eq!(prompts.len(), 2);
        assert_eq!((prompts[0].step, prompts[0].emergency_references), (2, 1));
    }
}
