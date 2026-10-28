//! Trigger bindings across the session tree. Every delivery attempt and
//! lifecycle ending writes a `trigger_fired` record into the owner's
//! transcript (`harness/src/subscriptions/fired.rs`); a delivered wake also
//! injects a user entry `e_fire_<binding>_<n>` with `origin.binding` into its
//! destination session (`functions/trigger_deliver.rs`). `harness::teardown`
//! must leave no binding behind, in the binding store or on the engine.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::harness_invariants::{HarnessInvariant, HarnessViolation};
use crate::wire::SessionMetricsPayload;

/// `TriggerOutcome` in `harness/src/subscriptions/fired.rs`.
const OUTCOMES: [&str; 6] = [
    "delivered",
    "delivery_failed",
    "skipped",
    "expired",
    "unregistered",
    "invalidated",
];
/// The engine function every agent binding registers as; its metadata is only
/// the `__binding` pointer.
const DELIVER_FUNCTION: &str = "harness::trigger::deliver";
/// How long teardown's engine-side unregistrations may take to land.
const TEARDOWN_SETTLE: Duration = Duration::from_secs(5);
const TEARDOWN_POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TriggerBindings {
    /// Every binding the session tree owned before teardown or named in a
    /// `trigger_fired` record or a wake.
    pub subscriptions: BTreeSet<String>,
    /// The tree's `trigger_fired` records by `outcome` (`missing` without one).
    pub fired: BTreeMap<String, u64>,
    /// The bindings `harness::teardown` removed; absent when it did not answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teardown_removed: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub violations: Vec<HarnessViolation>,
}

/// The root and every other session in `metrics.by_session`.
pub fn sessions(root_session_id: &str, metrics: Option<&SessionMetricsPayload>) -> Vec<String> {
    let mut sessions = vec![root_session_id.to_string()];
    for session in metrics.into_iter().flat_map(|metrics| &metrics.by_session) {
        if !sessions.contains(&session.session_id) {
            sessions.push(session.session_id.clone());
        }
    }
    sessions
}

/// Reads every session's transcript and live bindings before teardown.
pub async fn collect(
    context: &E2eContext,
    root_session_id: &str,
    root_transcript: &Value,
    metrics: &SessionMetricsPayload,
) -> Result<TriggerBindings> {
    let sessions = sessions(root_session_id, Some(metrics));
    let mut transcripts = vec![root_transcript.clone()];
    for session in &sessions[1..] {
        transcripts.push(context.transcript(session).await?);
    }
    let listed = listed(context, &sessions).await?;
    Ok(observe(
        &transcripts,
        listed.into_iter().map(|(_, subscription)| subscription),
    ))
}

pub fn observe(transcripts: &[Value], listed: impl IntoIterator<Item = String>) -> TriggerBindings {
    let entries = transcripts
        .iter()
        .filter_map(|transcript| transcript.get("messages").and_then(Value::as_array))
        .flatten()
        .collect::<Vec<_>>();
    let mut bindings = TriggerBindings {
        subscriptions: listed.into_iter().collect(),
        ..TriggerBindings::default()
    };
    let mut wake_delivered = BTreeMap::<&str, u64>::new();
    let mut once_delivered = BTreeMap::<&str, u64>::new();
    let mut woken = BTreeMap::<&str, u64>::new();
    for entry in &entries {
        let entry_id = text(entry, "/entry_id").unwrap_or("an unnamed entry");
        // The `e_fire_` wake only: expiry, claim and condition notices carry
        // the same origin without a delivery.
        if text(entry, "/message/role") == Some("user") && entry_id.starts_with("e_fire_") {
            if let Some(binding) = text(entry, "/origin/binding") {
                *woken.entry(binding).or_default() += 1;
                bindings.subscriptions.insert(binding.to_string());
            }
        }
        if text(entry, "/custom/custom_type") != Some("trigger_fired") {
            continue;
        }
        let Some(data) = entry.pointer("/custom/data") else {
            continue;
        };
        let binding = text(data, "/subscription_id").unwrap_or("an unnamed binding");
        bindings.subscriptions.insert(binding.to_string());
        let outcome = text(data, "/outcome");
        *bindings
            .fired
            .entry(outcome.unwrap_or("missing").to_string())
            .or_default() += 1;
        if !outcome.is_some_and(|outcome| OUTCOMES.contains(&outcome)) {
            bindings.violations.push(HarnessViolation {
                invariant: HarnessInvariant::MalformedTriggerRecord,
                detail: format!("record {entry_id} of {binding} has outcome {outcome:?}"),
            });
        }
        if data.get("retired").and_then(Value::as_bool) == Some(true)
            && text(data, "/retirement_reason").is_none()
        {
            bindings.violations.push(HarnessViolation {
                invariant: HarnessInvariant::MalformedTriggerRecord,
                detail: format!("record {entry_id} retired {binding} without a retirement_reason"),
            });
        }
        if outcome != Some("delivered") {
            continue;
        }
        // Records written before the delivery hop name a wake `notify`.
        if matches!(text(data, "/target"), Some("harness::send" | "notify")) {
            *wake_delivered.entry(binding).or_default() += 1;
        }
        if data.get("once").and_then(Value::as_bool) == Some(true) {
            *once_delivered.entry(binding).or_default() += 1;
        }
    }
    for binding in wake_delivered
        .keys()
        .chain(woken.keys())
        .collect::<BTreeSet<_>>()
    {
        let delivered = wake_delivered.get(binding).copied().unwrap_or(0);
        let wakes = woken.get(binding).copied().unwrap_or(0);
        if delivered != wakes {
            bindings.violations.push(HarnessViolation {
                invariant: HarnessInvariant::WakeDeliveryMismatch,
                detail: format!(
                    "{binding} recorded {delivered} delivered wakes and woke {wakes} times"
                ),
            });
        }
    }
    for (binding, delivered) in once_delivered {
        if delivered > 1 {
            bindings.violations.push(HarnessViolation {
                invariant: HarnessInvariant::OnceBindingRedelivered,
                detail: format!("once binding {binding} delivered {delivered} times"),
            });
        }
    }
    bindings
}

/// Polls until teardown left nothing behind or the settle time ran out, and
/// records what is still there as leaks.
pub async fn verify_teardown(
    context: &E2eContext,
    sessions: &[String],
    bindings: &mut TriggerBindings,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + TEARDOWN_SETTLE;
    loop {
        // No binding of the run can be on the engine when it never saw one.
        let delivering = if bindings.subscriptions.is_empty() {
            Vec::new()
        } else {
            delivering(context).await?
        };
        let found = leaks(
            &bindings.subscriptions,
            &delivering,
            &listed(context, sessions).await?,
        );
        if found.is_empty() || tokio::time::Instant::now() >= deadline {
            bindings.violations.extend(found);
            return Ok(());
        }
        tokio::time::sleep(TEARDOWN_POLL).await;
    }
}

/// Engine triggers that deliver one of the run's bindings, and bindings a
/// session still lists.
pub fn leaks(
    subscriptions: &BTreeSet<String>,
    delivering: &[(String, String)],
    listed: &[(String, String)],
) -> Vec<HarnessViolation> {
    let engine = delivering
        .iter()
        .filter(|(_, binding)| subscriptions.contains(binding))
        .map(|(trigger, binding)| format!("engine trigger {trigger} still delivers {binding}"));
    let store = listed
        .iter()
        .map(|(session, binding)| format!("session {session} still lists {binding}"));
    engine
        .chain(store)
        .map(|detail| HarnessViolation {
            invariant: HarnessInvariant::LeakedTriggerBinding,
            detail: format!("after harness::teardown, {detail}"),
        })
        .collect()
}

/// `(engine trigger id, binding)` of every delivery-hop trigger. The list
/// omits metadata, so each row's `info` names its binding.
async fn delivering(context: &E2eContext) -> Result<Vec<(String, String)>> {
    let listed = context
        .trigger_value(
            "engine::registered-triggers::list",
            json!({ "function_id": DELIVER_FUNCTION, "include_pending": true }),
        )
        .await?;
    let mut delivering = Vec::new();
    for id in listed
        .get("registered_triggers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("id").and_then(Value::as_str))
    {
        // A row unregistered between the list and its info is gone.
        let Ok(info) = context
            .trigger_value("engine::registered-triggers::info", json!({ "id": id }))
            .await
        else {
            continue;
        };
        if let Some(binding) = text(&info, "/metadata/__binding") {
            delivering.push((id.to_string(), binding.to_string()));
        }
    }
    Ok(delivering)
}

/// `(session, binding)` for every binding `harness::triggers::list` returns.
async fn listed(context: &E2eContext, sessions: &[String]) -> Result<Vec<(String, String)>> {
    let mut listed = Vec::new();
    for session in sessions {
        let response = context
            .trigger_value("harness::triggers::list", json!({ "session_id": session }))
            .await?;
        listed.extend(
            response
                .get("subscriptions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|row| text(row, "/subscription_id"))
                .map(|binding| (session.clone(), binding.to_string())),
        );
    }
    Ok(listed)
}

fn text<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fired(binding: &str, ordinal: u64, data: Value) -> Value {
        let mut record = json!({
            "subscription_id": binding, "target": "harness::send", "outcome": "delivered",
            "once": false, "fires": ordinal, "retired": false,
        });
        record
            .as_object_mut()
            .unwrap()
            .extend(data.as_object().unwrap().clone());
        json!({ "entry_id": format!("e_trigfired_{binding}_{ordinal}"),
                "custom": { "custom_type": "trigger_fired", "data": record } })
    }

    fn wake(binding: &str, ordinal: u64) -> Value {
        json!({ "entry_id": format!("e_fire_{binding}_{ordinal}"),
                "origin": { "notification": true, "binding": binding },
                "message": { "role": "user" } })
    }

    fn transcript(entries: Vec<Value>) -> Value {
        json!({ "messages": entries })
    }

    fn invariants(bindings: &TriggerBindings) -> Vec<HarnessInvariant> {
        bindings.violations.iter().map(|v| v.invariant).collect()
    }

    #[test]
    fn records_are_counted_by_outcome_and_each_delivered_wake_woke_its_destination() {
        // The child registered the wake; it lands in the root.
        let root = transcript(vec![wake("sub_a", 0), wake("sub_a", 1)]);
        let child = transcript(vec![
            fired("sub_a", 1, json!({})),
            fired("sub_a", 2, json!({})),
            fired("sub_a", 2, json!({ "outcome": "skipped" })),
            // An expiry notice carries the origin without a delivery.
            json!({ "entry_id": "e_expire_sub_a", "origin": { "binding": "sub_a" },
                    "message": { "role": "user" } }),
            fired(
                "sub_a",
                2,
                json!({ "outcome": "expired", "retired": true,
                                      "retirement_reason": "expired" }),
            ),
            fired("sub_call", 1, json!({ "target": "state::set" })),
        ]);

        let bindings = observe(&[root, child], ["sub_live".to_string()]);

        assert!(bindings.violations.is_empty(), "{:?}", bindings.violations);
        assert_eq!(
            bindings.fired,
            BTreeMap::from([
                ("delivered".into(), 3),
                ("expired".into(), 1),
                ("skipped".into(), 1),
            ])
        );
        assert_eq!(
            bindings.subscriptions,
            BTreeSet::from(["sub_a".into(), "sub_call".into(), "sub_live".into()])
        );
    }

    #[test]
    fn an_unknown_outcome_or_a_retirement_without_reason_is_malformed() {
        let bindings = observe(
            &[transcript(vec![
                fired(
                    "sub_a",
                    1,
                    json!({ "outcome": "fired", "target": "state::set" }),
                ),
                json!({ "custom": { "custom_type": "trigger_fired", "data": {
                    "subscription_id": "sub_b", "target": "state::set", "retired": false,
                } } }),
                fired(
                    "sub_c",
                    1,
                    json!({ "outcome": "unregistered", "retired": true }),
                ),
            ])],
            [],
        );

        assert_eq!(
            invariants(&bindings),
            [HarnessInvariant::MalformedTriggerRecord; 3]
        );
        assert!(bindings.violations[0].detail.contains("\"fired\""));
        assert_eq!(bindings.fired.get("missing"), Some(&1));
        assert!(bindings.violations[2]
            .detail
            .contains("without a retirement_reason"));
    }

    #[test]
    fn a_delivered_wake_without_its_entry_or_an_entry_without_a_delivery_is_a_mismatch() {
        let bindings = observe(
            &[transcript(vec![
                fired("sub_lost", 1, json!({})),
                wake("sub_extra", 0),
            ])],
            [],
        );

        assert_eq!(
            invariants(&bindings),
            [HarnessInvariant::WakeDeliveryMismatch; 2]
        );
        assert!(bindings.violations[0]
            .detail
            .contains("sub_extra recorded 0"));
        assert!(bindings.violations[1]
            .detail
            .contains("sub_lost recorded 1"));
    }

    #[test]
    fn a_once_binding_delivered_twice_is_redelivered() {
        let bindings = observe(
            &[transcript(vec![
                fired(
                    "sub_once",
                    1,
                    json!({ "once": true, "target": "state::set" }),
                ),
                fired(
                    "sub_once",
                    2,
                    json!({ "once": true, "target": "state::set" }),
                ),
            ])],
            [],
        );

        assert_eq!(
            invariants(&bindings),
            [HarnessInvariant::OnceBindingRedelivered]
        );
    }

    #[test]
    fn a_run_binding_left_on_the_engine_or_in_a_session_is_a_leak() {
        let subscriptions = BTreeSet::from(["sub_a".to_string()]);

        let violations = leaks(
            &subscriptions,
            &[
                ("t1".into(), "sub_a".into()),
                // Another run's binding on a shared stack.
                ("t2".into(), "sub_other".into()),
            ],
            &[("child".into(), "sub_b".into())],
        );

        assert_eq!(
            violations.iter().map(|v| v.invariant).collect::<Vec<_>>(),
            [HarnessInvariant::LeakedTriggerBinding; 2]
        );
        assert!(violations[0]
            .detail
            .contains("engine trigger t1 still delivers sub_a"));
        assert!(violations[1]
            .detail
            .contains("session child still lists sub_b"));
        assert!(leaks(&subscriptions, &[("t2".into(), "sub_other".into())], &[]).is_empty());
    }
}
