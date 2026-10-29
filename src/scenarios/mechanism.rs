//! What the mechanism scenarios share. Each one drives a few short subject
//! turns and probes the Harness itself beside them: from `setup`, from a
//! run-scoped fixture the subject calls (the probe runs while that call is
//! held open), between exchanges and in `capture`. Every probe is recorded
//! as a named check; `capture` stores the checks as the deliverable and a
//! criterion passes when every check it reads was recorded and passed, so a
//! stored run is graded again from its deliverable alone.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::runtime::FunctionRef;
use iii_sdk::trigger::Trigger;
use iii_sdk::IIIClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{AssessmentOutcome, AssessmentSpec};
use super::common::lock_unpoisoned;
use super::{CapturedDeliverable, DeliverableContract, ProvenanceEvidence, ScenarioObservation};

pub(super) const DELIVERABLE_KIND: &str = "mechanism_checks";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Check {
    pub id: String,
    pub passed: bool,
    pub detail: String,
}

/// What one attempt holds between `setup` and `cleanup`.
#[derive(Default)]
struct Run {
    checks: Vec<Check>,
    functions: Vec<FunctionRef>,
    triggers: Vec<Trigger>,
    /// Sessions the runner made beside the subject's, deleted on cleanup.
    sessions: Vec<String>,
    /// Values a later probe reads (the id a held call started, a turn id).
    notes: HashMap<String, Value>,
}

fn runs() -> &'static Mutex<HashMap<String, Run>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Run>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn with_run<T>(run_id: &str, f: impl FnOnce(&mut Run) -> T) -> T {
    f(lock_unpoisoned(runs())
        .entry(run_id.to_string())
        .or_default())
}

/// The runner names the subject session after the attempt.
pub(super) fn session_id(run_id: &str) -> String {
    format!("e2e_{run_id}")
}

pub(super) fn record(run_id: &str, id: &str, passed: bool, detail: impl Into<String>) {
    let detail = detail.into();
    tracing::info!(run_id, check = id, passed, %detail, "mechanism check");
    with_run(run_id, |run| {
        run.checks.push(Check {
            id: id.to_string(),
            passed,
            detail,
        })
    });
}

pub(super) fn checks(run_id: &str) -> Vec<Check> {
    with_run(run_id, |run| run.checks.clone())
}

pub(super) fn keep_function(run_id: &str, function: FunctionRef) {
    with_run(run_id, |run| run.functions.push(function));
}

pub(super) fn keep_trigger(run_id: &str, trigger: Trigger) {
    with_run(run_id, |run| run.triggers.push(trigger));
}

/// Enrolls a side session before it exists, so cleanup deletes it whatever
/// fails after.
pub(super) fn keep_session(run_id: &str, session_id: &str) {
    with_run(run_id, |run| run.sessions.push(session_id.to_string()));
}

pub(super) fn note(run_id: &str, key: &str, value: Value) {
    with_run(run_id, |run| run.notes.insert(key.to_string(), value));
}

pub(super) fn noted(run_id: &str, key: &str) -> Option<Value> {
    with_run(run_id, |run| run.notes.get(key).cloned())
}

/// Appends `value` to the array noted under `key`.
pub(super) fn append(run_id: &str, key: &str, value: Value) {
    with_run(run_id, |run| {
        let list = run
            .notes
            .entry(key.to_string())
            .or_insert_with(|| json!([]));
        if let Some(list) = list.as_array_mut() {
            list.push(value);
        }
    });
}

/// An id as the Harness writes it into an entry id (`ids::sanitize`).
pub(super) fn entry_safe(id: &str) -> String {
    id.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-') {
                char::from(byte).to_string()
            } else {
                format!("~{byte:02X}")
            }
        })
        .collect()
}

/// A short id of the attempt for run-scoped function ids.
pub(super) fn short(run_id: &str) -> String {
    let short: String = run_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect();
    format!("{short:0<8}")
}

/// Forgets a previous attempt under this id (an interrupted local run).
pub(super) fn reset(run_id: &str) {
    if let Some(run) = lock_unpoisoned(runs()).remove(run_id) {
        release_registrations(run);
    }
}

fn release_registrations(run: Run) {
    for trigger in run.triggers {
        trigger.unregister();
    }
    for function in run.functions {
        function.unregister();
    }
}

/// Unregisters what setup registered and deletes the side sessions. Safe to
/// call twice.
pub(super) async fn release(context: &E2eContext, run_id: &str) -> Result<()> {
    let Some(mut run) = lock_unpoisoned(runs()).remove(run_id) else {
        return Ok(());
    };
    let sessions = std::mem::take(&mut run.sessions);
    release_registrations(run);
    let mut failures = Vec::new();
    for session in sessions {
        let _ = context
            .trigger_value("harness::teardown", json!({"root_session_id": session}))
            .await;
        if let Err(error) = context
            .trigger_value("session::delete", json!({"session_id": session}))
            .await
        {
            let error = format!("{error:#}");
            // A session the probe never got to create is already gone.
            if !error.contains("not found") && !error.contains("does not exist") {
                failures.push(format!("{session}: {error}"));
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "side sessions not deleted: {}",
            failures.join("; ")
        ))
    }
}

/// A call from a fixture handler or probe, routed as `E2eContext` routes it.
pub(super) async fn call(client: &IIIClient, function_id: &str, payload: Value) -> Result<Value> {
    let (namespace, payload) =
        crate::context::inspection_routing(function_id, client.namespace(), payload);
    client
        .trigger(
            TriggerRequest {
                function_id: function_id.to_string(),
                payload,
                action: None,
                timeout_ms: Some(120_000),
            }
            .namespace(namespace),
        )
        .await
        .map_err(|error| anyhow!("{function_id}: {error}"))
}

/// Polls `probe` every 500 ms until it returns `Some` or `timeout` passes.
pub(super) async fn poll<T, F, Fut>(timeout: Duration, mut probe: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = probe().await {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The subject's last assistant `(model, provider)`, the route a side
/// session of the same attempt takes.
pub(super) fn subject_route(transcript: &Value) -> Option<(String, String)> {
    entries(transcript).rev().find_map(|entry| {
        let message = entry.get("message")?;
        (message.get("role")? == "assistant").then_some(())?;
        Some((
            message.get("model")?.as_str()?.to_string(),
            message.get("provider")?.as_str()?.to_string(),
        ))
    })
}

pub(super) fn entries(transcript: &Value) -> impl DoubleEndedIterator<Item = &Value> {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// The custom rows of `custom_type`, as `(entry_id, data)`.
pub(super) fn customs<'a>(transcript: &'a Value, custom_type: &str) -> Vec<(&'a str, &'a Value)> {
    entries(transcript)
        .filter(|entry| entry["custom"]["custom_type"] == custom_type)
        .map(|entry| {
            (
                entry["entry_id"].as_str().unwrap_or_default(),
                &entry["custom"]["data"],
            )
        })
        .collect()
}

/// The text of every user message, in order.
pub(super) fn user_texts(transcript: &Value) -> Vec<String> {
    entries(transcript)
        .filter_map(|entry| entry.get("message"))
        .filter(|message| message["role"] == "user")
        .map(text_of)
        .collect()
}

/// A message's text blocks, joined.
pub(super) fn text_of(message: &Value) -> String {
    message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect()
}

/// The `e_<turn>_<step>_assistant` turn ids, in first-seen order.
pub(super) fn assistant_turns(transcript: &Value) -> Vec<String> {
    let mut turns: Vec<String> = Vec::new();
    for entry in entries(transcript) {
        let Some(id) = entry["entry_id"].as_str() else {
            continue;
        };
        let Some(rest) = id.strip_prefix("e_").filter(|_| id.ends_with("_assistant")) else {
            continue;
        };
        let Some((turn, _)) = rest.trim_end_matches("_assistant").rsplit_once('_') else {
            continue;
        };
        if !turns.iter().any(|seen| seen == turn) {
            turns.push(turn.to_string());
        }
    }
    turns
}

/// A JSON body `size` bytes long whose head, middle and tail carry the
/// markers `<tag>-H`, `<tag>-M` and `<tag>-T`, with digits between.
pub(super) fn marked_text(tag: &str, size: usize) -> String {
    let head = format!("{tag}-H ");
    let middle = format!(" {tag}-M ");
    let tail = format!(" {tag}-T");
    let filler = size.saturating_sub(head.len() + middle.len() + tail.len());
    let digits = |count: usize, offset: usize| -> String {
        (0..count)
            .map(|i| char::from(b'0' + ((i + offset) % 10) as u8))
            .collect()
    };
    format!(
        "{head}{}{middle}{}{tail}",
        digits(filler / 2, 0),
        digits(filler - filler / 2, 5)
    )
}

/// Full points when every check in `ids` was recorded and each record passed.
pub(super) fn grade(spec: AssessmentSpec, checks: &[Check], ids: &[&str]) -> AssessmentOutcome {
    let mut failed = Vec::new();
    for id in ids {
        let records: Vec<&Check> = checks.iter().filter(|check| check.id == *id).collect();
        match records.iter().find(|check| !check.passed) {
            _ if records.is_empty() => failed.push(format!("{id}: never ran")),
            Some(check) => failed.push(format!("{id}: {}", check.detail)),
            None => {}
        }
    }
    if failed.is_empty() {
        spec.full_or_zero(true, format!("passed: {}", ids.join(", ")))
    } else {
        spec.full_or_zero(false, failed.join("; "))
    }
}

/// The checks `capture` stored, else the live registry's.
pub(super) fn observed_checks(observation: &ScenarioObservation, run_id: &str) -> Vec<Check> {
    observation
        .deliverables
        .iter()
        .find(|deliverable| deliverable.kind == DELIVERABLE_KIND)
        .and_then(|deliverable| deliverable.content.as_json())
        .and_then(|content| serde_json::from_value(content["checks"].clone()).ok())
        .unwrap_or_else(|| checks(run_id))
}

pub(super) fn deliverable(id: &str, run_id: &str) -> CapturedDeliverable {
    CapturedDeliverable {
        id: id.to_string(),
        kind: DELIVERABLE_KIND.to_string(),
        content: json!({ "checks": checks(run_id) }).into(),
        invariants: Vec::new(),
        provenance: vec![ProvenanceEvidence {
            kind: "session".to_string(),
            source_id: session_id(run_id),
            relation: "probed_harness_mechanism".to_string(),
        }],
    }
}

pub(super) fn contract(id: &str) -> DeliverableContract {
    super::validation_loop::validation_contract(
        id,
        DELIVERABLE_KIND,
        json!({
            "type": "object",
            "required": ["checks"],
            "properties": {
                "checks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["id", "passed", "detail"],
                        "properties": {
                            "id": { "type": "string" },
                            "passed": { "type": "boolean" },
                            "detail": { "type": "string" }
                        }
                    }
                }
            },
            "additionalProperties": true
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: AssessmentSpec = AssessmentSpec::gate("probe", 10, "Did the probe pass?");

    fn check(id: &str, passed: bool) -> Check {
        Check {
            id: id.into(),
            passed,
            detail: format!("{id} detail"),
        }
    }

    #[test]
    fn a_criterion_needs_every_check_recorded_and_passed() {
        let checks = [check("a", true), check("b", true), check("b", false)];
        let report = |ids: &[&str]| {
            let evaluation =
                super::super::assessment::build_evaluation([grade(SPEC, &checks, ids)]);
            (
                evaluation.awards[0].awarded,
                evaluation.awards[0].reason.clone(),
            )
        };
        assert_eq!(report(&["a"]).0, Some(10));
        assert_eq!(report(&["a", "b"]), (Some(0), "b: b detail".to_string()));
        assert_eq!(report(&["c"]), (Some(0), "c: never ran".to_string()));
    }

    #[test]
    fn marked_text_has_the_requested_size_and_markers_in_place() {
        let text = marked_text("RB", 10_000);
        assert_eq!(text.len(), 10_000);
        assert!(text.starts_with("RB-H "));
        assert!(text.ends_with(" RB-T"));
        let middle = text.find("RB-M").unwrap();
        assert!((4_000..6_000).contains(&middle), "{middle}");
    }

    #[test]
    fn entry_ids_escape_as_the_harness_escapes_them() {
        assert_eq!(entry_safe("e2e:a b_c-d"), "e2e~3Aa~20b_c-d");
        assert_eq!(short("ab-cd"), "abcd0000");
    }

    #[test]
    fn assistant_turns_reads_the_turn_of_each_step_once() {
        let transcript = json!({ "messages": [
            { "entry_id": "e_t_a_0_assistant" },
            { "entry_id": "e_t_a_call1" },
            { "entry_id": "e_t_a_1_assistant" },
            { "entry_id": "e_idem_x" },
            { "entry_id": "e_t_b_0_assistant" },
        ] });
        assert_eq!(assistant_turns(&transcript), ["t_a", "t_b"]);
    }
}
