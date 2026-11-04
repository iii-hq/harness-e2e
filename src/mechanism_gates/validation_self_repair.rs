//! E5: the post-turn validator binding. It is bound to the session that
//! registered it and survives its fires, every denial is one nudge the status
//! counts, and unregistering it removes it exactly once.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{invoke_as, messages, text, Attempt, Broken, Run};
use crate::context::E2eContext;
use crate::scenarios::common::function_outcomes;

pub const ID: &str = crate::scenarios::validation_self_repair::ID;
const HOOK: &str = "harness::hook::on-post-turn";
/// The Harness's own ceiling when the run sets none.
const DEFAULT_MAX_RETRIES: u64 = 2;

fn validator(transcript: &Value) -> Option<(String, Value)> {
    function_outcomes(transcript)
        .into_iter()
        .find_map(|outcome| {
            (outcome.function_id == "harness::observe"
                && outcome.arguments["function_id"] == HOOK
                && outcome.is_error == Some(false))
            .then(|| {
                let details = outcome.details.unwrap_or_default();
                (
                    details["observer_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    details,
                )
            })
        })
}

/// Whether the subject itself removed this validator: a failed call, or one
/// for another observer, leaves it for the probes.
fn unregistered_by_subject(transcript: &Value, observer: &str) -> bool {
    function_outcomes(transcript).iter().any(|outcome| {
        outcome.function_id == "harness::unobserve"
            && outcome.arguments["id"] == observer
            && outcome.is_error == Some(false)
            && outcome
                .details
                .as_ref()
                .is_some_and(|details| details["removed"] == true)
    })
}

/// Unobserves the validator twice, unless the subject already did.
pub(super) async fn capture(
    context: &E2eContext,
    attempt: &Attempt<'_>,
    probes: &mut BTreeMap<String, Value>,
) {
    let status = attempt.terminal_status.clone().unwrap_or_default();
    probes.insert(
        "status".into(),
        json!({
            "turn_id": status["turn_id"],
            "validation_retries": status["validation_retries"],
            "max_validation_retries": status["max_validation_retries"],
        }),
    );
    let Some((observer, _)) = validator(attempt.transcript) else {
        return;
    };
    if unregistered_by_subject(attempt.transcript, &observer) {
        return;
    }
    // The session's own path: the console's `harness::observers::dispose`
    // does not reach an agent-observed validator.
    let mut replies = Vec::new();
    for _ in 0..2 {
        replies.push(
            invoke_as(
                context,
                attempt.session_id,
                "harness::unobserve",
                json!({"id": observer}),
            )
            .await,
        );
    }
    probes.insert("unregister".into(), json!(replies));
}

pub(super) fn gates(run: &Run<'_>) -> Vec<Broken> {
    let mut broken = Vec::new();
    let Some((observer, details)) = validator(run.transcript) else {
        return broken;
    };
    // The hook registry hands back the SDK observer's id, a UUID.
    if uuid::Uuid::parse_str(&observer).is_err()
        || details["once"] != false
        || !details["note"]
            .as_str()
            .is_some_and(|note| note.contains("bound to THIS session"))
    {
        broken.push((
            "validator_binding",
            format!("registration returned {details}"),
        ));
    }
    if let Some(replies) = run.probe("unregister") {
        let removed: Vec<&Value> = replies
            .as_array()
            .into_iter()
            .flatten()
            .map(|reply| &reply["details"]["removed"])
            .collect();
        if removed != [&Value::Bool(true), &Value::Bool(false)] {
            broken.push((
                "validator_unregistered_once",
                format!("two unregisters answered {replies}"),
            ));
        }
    }
    let status = run.probe("status");
    if status.is_some_and(|status| status["turn_id"].is_null()) {
        broken.push((
            "validation_retries_counted",
            "the terminal status reported no turn_id".into(),
        ));
    } else if let Some(status) = status {
        let turn = status["turn_id"].as_str();
        let nudges = messages(run.transcript)
            .iter()
            .filter(|entry| entry.pointer("/origin/validation") == Some(&Value::Bool(true)))
            .filter(|entry| text(entry, "/origin/turn_id") == turn)
            .count() as u64;
        let counted = status["validation_retries"].as_u64().unwrap_or_default();
        let ceiling = status["max_validation_retries"]
            .as_u64()
            .unwrap_or(DEFAULT_MAX_RETRIES);
        if nudges != counted || nudges > ceiling {
            broken.push((
                "validation_retries_counted",
                format!("{nudges} nudge(s) in the last turn, status counts {counted} of at most {ceiling}"),
            ));
        }
    }
    broken
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The subject's `harness::observe` of the post-turn hook, as Harness
    /// 1.9.1 answers it.
    fn transcript(observer: &str, nudges: usize) -> Value {
        let mut entries = vec![
            json!({"entry_id": "e_t1_0_assistant", "message": {"role": "assistant", "content": [{
                "type": "function_call", "id": "c1", "function_id": "agent_call",
                "arguments": {"function": "harness::observe", "payload": {
                    "function_id": "harness::hook::on-post-turn",
                    "target": {"function_id": "e2etest::audit_x"}, "args": {"timeout_ms": 30000}}}}]}}),
            json!({"entry_id": "e_t1_c1", "message": {"role": "function_result", "function_call_id": "c1",
                "function_id": "harness::observe", "is_error": false, "details": {
                    "observer_id": observer, "once": false,
                    "note": "post-turn validator bound to THIS session only (s); unobserve it with this id when the goal is met."}}}),
        ];
        for index in 0..nudges {
            entries.push(json!({"entry_id": format!("e_t1_nudge_{index}"),
                "origin": {"turn_id": "t1", "validation": true}, "message": {"role": "user", "content": []}}));
        }
        json!({"messages": entries})
    }

    fn gates_of(transcript: &Value, probes: Value) -> Vec<String> {
        let probes = serde_json::from_value(probes).unwrap();
        gates(&Run {
            transcript,
            metrics: None,
            probes: &probes,
        })
        .into_iter()
        .map(|(gate, detail)| format!("{gate}: {detail}"))
        .collect()
    }

    /// The SDK observer id the hook registry hands back.
    const POSTHOOK: &str = "ea4785c0-f297-4298-a73a-0b27879ace30";

    #[test]
    fn a_counted_session_bound_validator_removed_once_keeps_every_gate() {
        let probes = json!({"unregister": [{"details": {"removed": true}}, {"details": {"removed": false}}],
            "status": {"turn_id": "t1", "validation_retries": 1, "max_validation_retries": 2}});

        assert_eq!(
            gates_of(&transcript(POSTHOOK, 1), probes),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_miscounted_unbound_or_sticky_validator_is_flagged() {
        let probes = json!({"unregister": [{"details": {"removed": true}}, {"details": {"removed": true}}],
            "status": {"turn_id": "t1", "validation_retries": 1, "max_validation_retries": 2}});

        let broken = gates_of(&transcript("sub_1", 3), probes);

        assert_eq!(broken.len(), 3, "{broken:?}");
        assert!(broken[0].starts_with("validator_binding"));
        assert_eq!(
            broken[1],
            r#"validator_unregistered_once: two unregisters answered [{"details":{"removed":true}},{"details":{"removed":true}}]"#
        );
        assert_eq!(
            broken[2],
            "validation_retries_counted: 3 nudge(s) in the last turn, status counts 1 of at most 2"
        );
    }

    fn with_unregister(mut transcript: Value, id: &str, is_error: bool, removed: bool) -> Value {
        transcript["messages"].as_array_mut().unwrap().extend([
            json!({"entry_id": "e_t1_1_assistant", "message": {"role": "assistant", "content": [{
                "type": "function_call", "id": "c2", "function_id": "agent_call",
                "arguments": {"function": "harness::unobserve", "payload": {"id": id}}}]}}),
            json!({"entry_id": "e_t1_c2", "message": {"role": "function_result", "function_call_id": "c2",
                "function_id": "harness::unobserve", "is_error": is_error,
                "details": {"removed": removed}}}),
        ]);
        transcript
    }

    #[test]
    fn only_the_subjects_own_successful_removal_skips_the_probes() {
        let removed = with_unregister(transcript(POSTHOOK, 0), POSTHOOK, false, true);
        let failed = with_unregister(transcript(POSTHOOK, 0), POSTHOOK, true, false);
        let other = with_unregister(transcript(POSTHOOK, 0), "sub_other", false, true);

        assert!(unregistered_by_subject(&removed, POSTHOOK));
        assert!(!unregistered_by_subject(&failed, POSTHOOK));
        assert!(!unregistered_by_subject(&other, POSTHOOK));
    }

    #[test]
    fn a_timed_out_unregister_keeps_its_cause() {
        let probes = json!({"unregister": [{"details": {"removed": true}}, {"timed_out": true}]});

        assert_eq!(
            gates_of(&transcript(POSTHOOK, 0), probes),
            vec![
                r#"validator_unregistered_once: two unregisters answered [{"details":{"removed":true}},{"timed_out":true}]"#
            ]
        );
    }

    #[test]
    fn a_status_without_a_turn_id_breaks_the_retry_gate() {
        let probes = json!({"status": {"turn_id": null, "validation_retries": 0}});

        assert_eq!(
            gates_of(&transcript(POSTHOOK, 1), probes),
            vec!["validation_retries_counted: the terminal status reported no turn_id"]
        );
    }

    #[test]
    fn a_replayed_run_is_judged_on_its_transcript_alone() {
        assert!(gates_of(&transcript(POSTHOOK, 2), json!({})).is_empty());
    }
}
