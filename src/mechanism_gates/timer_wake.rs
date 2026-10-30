//! E3: a timer registered by the session wakes it once, on time, as one
//! notification in the session's own model, and retires.

use serde_json::Value;

use super::{customs, message_text, messages, text, Broken, Run};
use crate::scenarios::common::function_outcomes;

pub const ID: &str = crate::scenarios::timer_wake::ID;
/// How late a timer may fire.
const LATE_MS: i64 = 5_000;
/// How early, against its registration, a timer may fire.
const EARLY_MS: i64 = 1_000;

/// `sub_` and 32 lowercase hex digits.
pub(super) fn subscription_id(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix).is_some_and(|hex| {
        hex.len() == 32 && hex.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
    })
}

/// When the assistant entry that made call `call_id` was written.
fn called_at(transcript: &Value, call_id: &str) -> Option<i64> {
    messages(transcript)
        .iter()
        .find(|entry| {
            entry
                .pointer("/message/content")
                .and_then(Value::as_array)
                .is_some_and(|blocks| blocks.iter().any(|block| block["id"] == call_id))
        })?
        .pointer("/message/timestamp")?
        .as_i64()
}

pub(super) fn gates(run: &Run<'_>) -> Vec<Broken> {
    let mut broken = Vec::new();
    let Some(timer) = function_outcomes(run.transcript)
        .into_iter()
        .find(|outcome| {
            outcome.function_id == "engine::register_trigger"
                && outcome.arguments["trigger_type"] == "timer"
                && outcome.is_error == Some(false)
        })
    else {
        // No timer was armed: the scenario's own criteria judge that.
        return broken;
    };
    let sub = timer
        .details
        .as_ref()
        .and_then(|details| details["subscription_id"].as_str())
        .unwrap_or_default();
    let label = timer.arguments["label"].as_str().unwrap_or_default();
    if !subscription_id(sub, "sub_") {
        broken.push((
            "timer_subscription_id",
            format!("registration returned {sub:?}"),
        ));
    }

    let notifications: Vec<&Value> = messages(run.transcript)
        .iter()
        .filter(|entry| entry.pointer("/origin/notification") == Some(&Value::Bool(true)))
        .filter(|entry| text(entry, "/origin/binding") == Some(sub))
        .collect();
    let fire_id = format!("e_fire_{sub}_1");
    // A registration without a label notifies with the payload alone.
    let prefix = match label {
        "" => "[notification] ".to_string(),
        label => format!("[notification] {label}:"),
    };
    match notifications.as_slice() {
        [entry]
            if text(entry, "/entry_id") == Some(fire_id.as_str())
                && message_text(entry).starts_with(&prefix) => {}
        _ => broken.push((
            "timer_single_notification",
            format!(
                "expected one entry {fire_id} starting `{prefix}`, found {:?}",
                notifications
                    .iter()
                    .map(|entry| text(entry, "/entry_id"))
                    .collect::<Vec<_>>()
            ),
        )),
    }

    let delivered: Vec<&Value> = customs(run.transcript, "trigger_fired")
        .map(|entry| &entry["custom"]["data"])
        .filter(|data| data["subscription_id"] == sub && data["outcome"] == "delivered")
        .collect();
    match delivered.as_slice() {
        [data] => {
            if data["retired"] != true || data["retirement_reason"] != "once_consumed" {
                broken.push((
                    "timer_once_consumed",
                    format!(
                        "retired={}, retirement_reason={}",
                        data["retired"], data["retirement_reason"]
                    ),
                ));
            }
            let actual = data["payload"]["actual_at"].as_i64();
            let scheduled = data["payload"]["scheduled_at"].as_i64();
            let in_ms = timer.arguments["config"]["in_ms"].as_i64();
            let armed = timer
                .call_id
                .as_deref()
                .and_then(|id| called_at(run.transcript, id));
            if let (Some(actual), Some(scheduled)) = (actual, scheduled) {
                if actual - scheduled > LATE_MS {
                    broken.push((
                        "timer_on_time",
                        format!("fired {} ms after its deadline", actual - scheduled),
                    ));
                }
            }
            if let (Some(actual), Some(in_ms), Some(armed)) = (actual, in_ms, armed) {
                if actual < armed + in_ms - EARLY_MS {
                    broken.push((
                        "timer_on_time",
                        format!(
                            "fired {} ms after it was armed, asked for {in_ms}",
                            actual - armed
                        ),
                    ));
                }
            }
        }
        _ => broken.push((
            "timer_once_consumed",
            format!("{} delivered fire(s) for {sub}", delivered.len()),
        )),
    }

    let assistants: Vec<(usize, &Value)> = messages(run.transcript)
        .iter()
        .enumerate()
        .filter(|(_, entry)| text(entry, "/message/role") == Some("assistant"))
        .collect();
    let fired_at = messages(run.transcript)
        .iter()
        .position(|entry| text(entry, "/entry_id") == Some(fire_id.as_str()));
    if let (Some((_, first)), Some(fired_at)) = (assistants.first(), fired_at) {
        let route = |entry: &Value| {
            (
                entry["message"]["model"].clone(),
                entry["message"]["provider"].clone(),
            )
        };
        let strays: Vec<_> = assistants
            .iter()
            .filter(|(index, entry)| *index > fired_at && route(entry) != route(first))
            .map(|(_, entry)| route(entry))
            .collect();
        if !strays.is_empty() {
            broken.push((
                "wake_inherits_route",
                format!(
                    "the wake ran on {strays:?}, the session on {:?}",
                    route(first)
                ),
            ));
        }
    }

    let contexts = customs(run.transcript, "runtime_context").count();
    let noticed = messages(run.transcript)
        .iter()
        .any(|entry| text(entry, "/custom/data/kind") == Some("runtime-context"));
    if contexts != 1 || noticed {
        broken.push((
            "single_runtime_context",
            format!("{contexts} runtime_context entr(ies), runtime-context notice={noticed}"),
        ));
    }
    broken
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;

    const SUB: &str = "sub_c0291f93a175411eb91aa668d4c696bf";

    fn assistant(id: &str, model: &str, calls: Value) -> Value {
        json!({"entry_id": id, "message": {"role": "assistant", "model": model,
            "provider": "deepseek", "timestamp": 1_000, "content": calls}})
    }

    /// The shape of a stored run that woke on time.
    fn woken(sub: &str, actual_at: i64, wake_model: &str) -> Vec<Value> {
        vec![
            json!({"entry_id": "e_t1_0_runtime_context", "custom": {"custom_type": "runtime_context", "data": {}}}),
            assistant(
                "e_t1_0_assistant",
                "deepseek-flash",
                json!([{
                    "type": "function_call", "id": "c1", "function_id": "agent_trigger",
                    "arguments": {"function": "engine::register_trigger", "payload": {
                        "trigger_type": "timer", "label": "timer-wake:r", "once": true,
                        "config": {"in_ms": 6_000}}},
                }]),
            ),
            json!({"entry_id": "e_t1_c1", "message": {"role": "function_result",
                "function_call_id": "c1", "function_id": "engine::register_trigger",
                "is_error": false, "details": {"subscription_id": sub, "once": true}}}),
            json!({"entry_id": format!("e_fire_{sub}_1"), "origin": {"binding": sub, "notification": true},
                "message": {"role": "user", "content": [{"type": "text",
                    "text": "[notification] timer-wake:r: {\"trigger\":\"timer\"}"}]}}),
            json!({"entry_id": format!("e_trigfired_{sub}_1"), "custom": {"custom_type": "trigger_fired", "data": {
                "subscription_id": sub, "outcome": "delivered", "retired": true,
                "retirement_reason": "once_consumed",
                "payload": {"actual_at": actual_at, "scheduled_at": 7_000}}}}),
            assistant("e_t2_0_assistant", wake_model, json!([])),
        ]
    }

    fn gates_of(entries: Vec<Value>) -> Vec<String> {
        let transcript = json!({"messages": entries});
        let probes = BTreeMap::new();
        gates(&Run {
            transcript: &transcript,
            metrics: None,
            probes: &probes,
        })
        .into_iter()
        .map(|(gate, detail)| format!("{gate}: {detail}"))
        .collect()
    }

    #[test]
    fn a_timer_that_woke_once_on_time_keeps_every_gate() {
        assert_eq!(
            gates_of(woken(SUB, 7_001, "deepseek-flash")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_late_early_rerouted_or_malformed_wake_is_flagged() {
        let late = gates_of(woken(SUB, 12_500, "deepseek-flash"));
        let early = gates_of(woken(SUB, 5_000, "deepseek-flash"));
        let rerouted = gates_of(woken(SUB, 7_001, "deepseek-pro"));
        let malformed = gates_of(woken("sub_1", 7_001, "deepseek-flash"));

        assert_eq!(late, ["timer_on_time: fired 5500 ms after its deadline"]);
        assert_eq!(
            early,
            ["timer_on_time: fired 4000 ms after it was armed, asked for 6000"]
        );
        assert!(
            rerouted[0].starts_with("wake_inherits_route"),
            "{rerouted:?}"
        );
        assert!(
            malformed[0].starts_with("timer_subscription_id"),
            "{malformed:?}"
        );
    }

    #[test]
    fn a_second_notification_or_runtime_context_is_flagged() {
        let mut entries = woken(SUB, 7_001, "deepseek-flash");
        entries.push(json!({"entry_id": format!("e_fire_{SUB}_2"),
            "origin": {"binding": SUB, "notification": true}, "message": {"role": "user", "content": []}}));
        entries.push(json!({"entry_id": "e_t2_0_runtime_context", "custom": {"custom_type": "runtime_context", "data": {}}}));

        let broken = gates_of(entries);

        assert!(
            broken[0].starts_with("timer_single_notification"),
            "{broken:?}"
        );
        assert_eq!(
            broken[1],
            "single_runtime_context: 2 runtime_context entr(ies), runtime-context notice=false"
        );
    }

    #[test]
    fn an_unlabeled_timer_notifies_with_its_payload_alone() {
        let mut entries = woken(SUB, 7_001, "deepseek-flash");
        entries[1]["message"]["content"][0]["arguments"]["payload"]
            .as_object_mut()
            .unwrap()
            .remove("label");
        entries[3]["message"]["content"][0]["text"] =
            json!("[notification] {\"trigger\":\"timer\"}");

        assert_eq!(gates_of(entries), Vec::<String>::new());
    }

    #[test]
    fn a_run_that_armed_no_timer_is_left_to_the_criteria() {
        assert!(gates_of(Vec::new()).is_empty());
    }
}
