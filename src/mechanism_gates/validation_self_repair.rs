//! E5: the post-turn validator binding. It is bound to the session that
//! registered it and survives its fires, every denial is one nudge the status
//! counts, and unregistering it removes it exactly once.

use std::collections::BTreeMap;

use anyhow::Result;
use serde_json::{json, Value};

use super::timer_wake::subscription_id;
use super::{invoke_as, messages, text, Attempt, Broken, Run};
use crate::context::E2eContext;
use crate::scenarios::common::function_outcomes;

pub const ID: &str = crate::scenarios::validation_self_repair::ID;
const HOOK_TYPE: &str = "harness::hook::post-turn";
/// The Harness's own ceiling when the run sets none.
const DEFAULT_MAX_RETRIES: u64 = 2;

fn validator(transcript: &Value) -> Option<(String, Value)> {
    function_outcomes(transcript)
        .into_iter()
        .find_map(|outcome| {
            (outcome.function_id == "engine::register_trigger"
                && outcome.arguments["trigger_type"] == HOOK_TYPE
                && outcome.is_error == Some(false))
            .then(|| {
                let details = outcome.details.unwrap_or_default();
                (
                    details["subscription_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    details,
                )
            })
        })
}

/// Unregisters the validator twice, unless the subject already did.
pub(super) async fn capture(
    context: &E2eContext,
    attempt: &Attempt<'_>,
    probes: &mut BTreeMap<String, Value>,
) -> Result<()> {
    let status = attempt.terminal_status.clone().unwrap_or_default();
    probes.insert(
        "status".into(),
        json!({
            "turn_id": status["turn_id"],
            "validation_retries": status["validation_retries"],
            "max_validation_retries": status["max_validation_retries"],
        }),
    );
    let unregistered_by_subject = function_outcomes(attempt.transcript)
        .iter()
        .any(|outcome| outcome.function_id.contains("unregister"));
    let Some((subscription, _)) = validator(attempt.transcript) else {
        return Ok(());
    };
    if unregistered_by_subject {
        return Ok(());
    }
    // The session's own path: the console's `harness::triggers::unregister`
    // does not reach an agent-registered validator.
    let mut removed = Vec::new();
    for _ in 0..2 {
        let reply = invoke_as(
            context,
            attempt.session_id,
            "engine::unregister_trigger",
            json!({"id": subscription}),
        )
        .await?;
        removed.push(reply["details"]["removed"].clone());
    }
    probes.insert("unregister".into(), json!(removed));
    Ok(())
}

pub(super) fn gates(run: &Run<'_>) -> Vec<Broken> {
    let mut broken = Vec::new();
    let Some((subscription, details)) = validator(run.transcript) else {
        return broken;
    };
    if !subscription_id(&subscription, "posthook_")
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
    if let Some(removed) = run.probe("unregister") {
        if removed != &json!([true, false]) {
            broken.push((
                "validator_unregistered_once",
                format!("two unregisters answered {removed}"),
            ));
        }
    }
    if let Some(status) = run
        .probe("status")
        .filter(|status| !status["turn_id"].is_null())
    {
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

    fn transcript(subscription: &str, nudges: usize) -> Value {
        let mut entries = vec![
            json!({"entry_id": "e_t1_0_assistant", "message": {"role": "assistant", "content": [{
                "type": "function_call", "id": "c1", "function_id": "agent_trigger",
                "arguments": {"function": "engine::register_trigger", "payload": {
                    "trigger_type": HOOK_TYPE, "function_id": "e2etest::audit_x"}}}]}}),
            json!({"entry_id": "e_t1_c1", "message": {"role": "function_result", "function_call_id": "c1",
                "function_id": "engine::register_trigger", "is_error": false, "details": {
                    "subscription_id": subscription, "once": false,
                    "note": "post-turn validator bound to THIS session only (s)."}}}),
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

    const POSTHOOK: &str = "posthook_0be3d9294f0245ebb4e12fdb512b6059";

    #[test]
    fn a_counted_session_bound_validator_removed_once_keeps_every_gate() {
        let probes = json!({"unregister": [true, false],
            "status": {"turn_id": "t1", "validation_retries": 1, "max_validation_retries": 2}});

        assert_eq!(
            gates_of(&transcript(POSTHOOK, 1), probes),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_miscounted_unbound_or_sticky_validator_is_flagged() {
        let probes = json!({"unregister": [true, true],
            "status": {"turn_id": "t1", "validation_retries": 1, "max_validation_retries": 2}});

        let broken = gates_of(&transcript("sub_1", 3), probes);

        assert_eq!(broken.len(), 3, "{broken:?}");
        assert!(broken[0].starts_with("validator_binding"));
        assert_eq!(
            broken[1],
            "validator_unregistered_once: two unregisters answered [true,true]"
        );
        assert_eq!(
            broken[2],
            "validation_retries_counted: 3 nudge(s) in the last turn, status counts 1 of at most 2"
        );
    }

    #[test]
    fn a_replayed_run_is_judged_on_its_transcript_alone() {
        assert!(gates_of(&transcript(POSTHOOK, 2), json!({})).is_empty());
    }
}
