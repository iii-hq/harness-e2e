//! E4: the dispatch policy the Harness enforces and states. What the
//! transcript says it dispatched is what the handlers received; the stale id
//! is denied with the current one as a hint, a registered id it denies gets
//! no hint, and the turn is told exactly which ids it may call.

use std::collections::BTreeMap;

use anyhow::Result;
use serde_json::{json, Value};

use super::{customs, error_code, messages, text, Attempt, Broken, Run};
use crate::context::E2eContext;
use crate::scenarios::common::function_outcomes;

pub const ID: &str = crate::scenarios::tool_contract_recovery::ID;

pub(super) async fn capture(
    context: &E2eContext,
    attempt: &Attempt<'_>,
    probes: &mut BTreeMap<String, Value>,
) -> Result<()> {
    let fixture = crate::scenarios::tool_contract_recovery::mechanism_probe(attempt.run_id);
    let mut registered = serde_json::Map::new();
    for outcome in function_outcomes(attempt.transcript) {
        let denied = outcome.details.as_ref().is_some_and(|details| {
            error_code(&json!({"details": details})) == Some("policy_denied")
        });
        if denied && !registered.contains_key(&outcome.function_id) {
            let exists = context.function_exists(&outcome.function_id).await?;
            registered.insert(outcome.function_id, json!(exists));
        }
    }
    for (name, value) in fixture.as_object().into_iter().flatten() {
        probes.insert(name.clone(), value.clone());
    }
    probes.insert("registered".into(), Value::Object(registered));
    Ok(())
}

/// Every field the handler decoded holds what the transcript sent.
fn received(handler: &Value, sent: &Value) -> bool {
    handler.as_object().is_some_and(|fields| {
        fields
            .iter()
            .all(|(name, value)| sent.get(name).unwrap_or(&Value::Null) == value)
    })
}

fn strings(value: Option<&Value>) -> Vec<&str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

pub(super) fn gates(run: &Run<'_>) -> Vec<Broken> {
    let mut broken = Vec::new();
    let outcomes = function_outcomes(run.transcript);
    let message = |details: &Option<Value>| {
        details
            .as_ref()
            .and_then(|details| details["message"].as_str())
            .unwrap_or_default()
            .to_string()
    };
    let denied = |details: &Option<Value>| {
        details.as_ref().is_some_and(|details| {
            error_code(&json!({"details": details})) == Some("policy_denied")
        })
    };

    let fixture_ids = strings(run.probe("fixture_ids"));
    if let Some(log) = run.probe("handler_log").and_then(Value::as_array) {
        let dispatched: Vec<_> = outcomes
            .iter()
            .filter(|outcome| fixture_ids.contains(&outcome.function_id.as_str()))
            .filter(|outcome| outcome.is_error == Some(false))
            .collect();
        let matched = dispatched.len() == log.len()
            && dispatched.iter().zip(log).all(|(outcome, record)| {
                record["function_id"] == outcome.function_id.as_str()
                    && received(&record["arguments"], &outcome.arguments)
            });
        if !matched {
            broken.push((
                "dispatch_matches_handler",
                format!(
                    "the transcript dispatched {:?}, the handlers received {:?}",
                    dispatched
                        .iter()
                        .map(|outcome| &outcome.function_id)
                        .collect::<Vec<_>>(),
                    log.iter()
                        .map(|record| &record["function_id"])
                        .collect::<Vec<_>>()
                ),
            ));
        }
    }

    let legacy = run.probe("legacy").and_then(Value::as_str);
    let replacement = run
        .probe("replacement")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let hint = format!("did you mean {replacement}?");
    for outcome in &outcomes {
        if Some(outcome.function_id.as_str()) == legacy {
            if !denied(&outcome.details) || !message(&outcome.details).contains(&hint) {
                broken.push((
                    "legacy_call_hinted",
                    format!(
                        "the stale call got {:?}, not policy_denied with `{hint}`",
                        message(&outcome.details)
                    ),
                ));
            }
        } else if denied(&outcome.details)
            && run
                .probe("registered")
                .and_then(|r| r.get(&outcome.function_id))
                == Some(&Value::Bool(true))
            && message(&outcome.details).contains("did you mean")
        {
            broken.push((
                "registered_denial_unhinted",
                format!(
                    "{} is registered yet its denial suggests another id",
                    outcome.function_id
                ),
            ));
        }
    }

    let allowed = strings(run.probe("allowed"));
    if !allowed.is_empty() {
        let line = format!(
            "Your dispatch policy allows ONLY these functions: {}.",
            allowed.join(", ")
        );
        let aids: Vec<&str> = customs(run.transcript, "runtime_context")
            .filter_map(|entry| text(entry, "/custom/data/aid"))
            .collect();
        if aids.is_empty()
            || !aids
                .iter()
                .all(|aid| aid.lines().any(|l| l.starts_with(&line)))
        {
            broken.push((
                "dispatch_policy_aid",
                format!("no runtime context states `{line}`"),
            ));
        }
        for outcome in outcomes
            .iter()
            .filter(|o| o.function_id == "engine::functions::list")
        {
            let listed: Vec<&str> = outcome
                .details
                .as_ref()
                .and_then(|details| details["functions"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|function| function["function_id"].as_str())
                .collect();
            let extra: Vec<&&str> = listed.iter().filter(|id| !allowed.contains(id)).collect();
            if !extra.is_empty() {
                broken.push((
                    "list_only_allowed",
                    format!("functions::list showed {extra:?}"),
                ));
            }
        }
    }
    let skills: Vec<&str> = messages(run.transcript)
        .iter()
        .filter_map(|entry| text(entry, "/entry_id"))
        .filter(|id| id.contains("_skills_"))
        .collect();
    if !skills.is_empty() && !allowed.is_empty() {
        broken.push((
            "no_skills_index",
            format!("a narrowed policy still got {skills:?}"),
        ));
    }
    broken
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALLOWED: [&str; 3] = [
        "e2etest::profile_v2_x",
        "e2etest::schedule_v2_x",
        "engine::functions::list",
    ];

    fn call(id: &str, function: &str, arguments: Value) -> Value {
        json!({"entry_id": format!("e_t1_{id}_assistant"), "message": {"role": "assistant", "content": [{
            "type": "function_call", "id": id, "function_id": "agent_trigger",
            "arguments": {"function": function, "payload": arguments}}]}})
    }

    fn result(id: &str, function: &str, is_error: bool, details: Value) -> Value {
        json!({"entry_id": format!("e_t1_{id}"), "message": {"role": "function_result",
            "function_call_id": id, "function_id": function, "is_error": is_error, "details": details}})
    }

    fn denial(function: &str, hint: Option<&str>) -> Value {
        let hint = hint
            .map(|id| format!(" did you mean {id}?"))
            .unwrap_or_default();
        json!({"error": "policy_denied", "message": format!("function {function} is not permitted.{hint}")})
    }

    fn aid() -> Value {
        json!({"entry_id": "e_t1_0_runtime_context", "custom": {"custom_type": "runtime_context", "data": {
            "aid": format!("Your session id is s.\nYour dispatch policy allows ONLY these functions: {}. Deny-listed on top: x.", ALLOWED.join(", "))}}})
    }

    fn probes(log: Value) -> BTreeMap<String, Value> {
        serde_json::from_value(json!({
            "allowed": ALLOWED, "legacy": "e2etest::schedule_v1_x", "replacement": "e2etest::schedule_v2_x",
            "fixture_ids": ["e2etest::profile_v2_x", "e2etest::schedule_v2_x"],
            "handler_log": log, "registered": {"state::set": true, "e2etest::shedule_v2_x": false},
        }))
        .unwrap()
    }

    fn gates_of(entries: Vec<Value>, probes: &BTreeMap<String, Value>) -> Vec<String> {
        let transcript = json!({"messages": entries});
        gates(&Run {
            transcript: &transcript,
            metrics: None,
            probes,
        })
        .into_iter()
        .map(|(gate, detail)| format!("{gate}: {detail}"))
        .collect()
    }

    fn recovered() -> Vec<Value> {
        vec![
            aid(),
            call("c1", "e2etest::schedule_v1_x", json!({})),
            result(
                "c1",
                "e2etest::schedule_v1_x",
                true,
                denial("e2etest::schedule_v1_x", Some("e2etest::schedule_v2_x")),
            ),
            call("c2", "state::set", json!({})),
            result("c2", "state::set", true, denial("state::set", None)),
            call(
                "c3",
                "e2etest::profile_v2_x",
                json!({"profile_key": "k", "extra": 1}),
            ),
            result("c3", "e2etest::profile_v2_x", false, json!({})),
            call("c4", "engine::functions::list", json!({})),
            result(
                "c4",
                "engine::functions::list",
                false,
                json!({"functions": [{"function_id": "e2etest::profile_v2_x"}]}),
            ),
        ]
    }

    #[test]
    fn a_run_whose_dispatches_reached_the_handlers_keeps_every_gate() {
        let log =
            json!([{"function_id": "e2etest::profile_v2_x", "arguments": {"profile_key": "k"}}]);

        assert_eq!(gates_of(recovered(), &probes(log)), Vec::<String>::new());
    }

    #[test]
    fn a_dispatch_the_handler_never_saw_is_flagged() {
        let broken = gates_of(recovered(), &probes(json!([])));

        assert_eq!(broken.len(), 1, "{broken:?}");
        assert!(broken[0].starts_with("dispatch_matches_handler"));
    }

    #[test]
    fn hints_aid_listing_and_skills_are_held_to_the_policy() {
        let mut entries = recovered();
        entries[0]["custom"]["data"]["aid"] =
            json!("Your dispatch policy allows ONLY these functions: x.");
        entries[2] = result(
            "c1",
            "e2etest::schedule_v1_x",
            true,
            denial("e2etest::schedule_v1_x", None),
        );
        entries[4] = result(
            "c2",
            "state::set",
            true,
            denial("state::set", Some("e2etest::schedule_v2_x")),
        );
        entries[8] = result(
            "c4",
            "engine::functions::list",
            false,
            json!({"functions": [{"function_id": "state::set"}]}),
        );
        entries
            .push(json!({"entry_id": "e_t1_skills_0", "message": {"role": "user", "content": []}}));
        let log =
            json!([{"function_id": "e2etest::profile_v2_x", "arguments": {"profile_key": "k"}}]);

        let gates: Vec<String> = gates_of(entries, &probes(log))
            .into_iter()
            .map(|broken| broken.split(':').next().unwrap().to_string())
            .collect();

        assert_eq!(
            gates,
            [
                "legacy_call_hinted",
                "registered_denial_unhinted",
                "dispatch_policy_aid",
                "list_only_allowed",
                "no_skills_index",
            ]
        );
    }
}
