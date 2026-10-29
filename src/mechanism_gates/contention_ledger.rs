//! E2: the sub-agent machinery under a fan-out. Spawns keep their contract
//! and their linkage, writers are leaves behind the control-plane wall with
//! self-limiting wakes, each finished cleanly on the root's route, the shared
//! budget settled, and the barrier woke the root once.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{ask, customs, denied, invoke_as, messages, text, Attempt, Broken, Run};
use crate::context::E2eContext;
use crate::scenarios::common::{function_outcomes, state_value};
use crate::wire::E2E_SESSION_KIND;

pub const ID: &str = crate::scenarios::contention_ledger::ID;
/// The control plane a leaf loses (the Harness's `CONTROL_PLANE_DENY`).
const WALLED: [&str; 4] = [
    "harness::spawn",
    "harness::send",
    "engine::unregister_trigger",
    "engine::registered-triggers::list",
];
/// The Harness's patience for pending work, which caps a leaf's wake.
const LEAF_WAKE_CAP_MS: i64 = 30 * 60 * 1_000;
/// The scenario's own turn ceiling, which a child's may not exceed.
const MAX_TURNS: u64 = 24;
/// A wake long enough that only the leaf cap can shorten it.
const PROBE_WAKE_MS: u64 = 2 * 60 * 60 * 1_000;

/// The successful `harness::spawn` calls: `(turn_id, call_id, details)`.
fn spawns(transcript: &Value) -> Vec<(String, String, Value)> {
    let turn_of = |call_id: &str| {
        messages(transcript)
            .iter()
            .find(|entry| {
                entry
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .is_some_and(|blocks| blocks.iter().any(|block| block["id"] == call_id))
            })
            .and_then(|entry| text(entry, "/origin/turn_id"))
            .unwrap_or_default()
            .to_string()
    };
    function_outcomes(transcript)
        .into_iter()
        .filter(|outcome| {
            outcome.function_id == "harness::spawn" && outcome.is_error == Some(false)
        })
        .map(|outcome| {
            let call_id = outcome.call_id.unwrap_or_default();
            (
                turn_of(&call_id),
                call_id,
                outcome.details.unwrap_or_default(),
            )
        })
        .collect()
}

pub(super) async fn capture(
    context: &E2eContext,
    attempt: &Attempt<'_>,
    probes: &mut BTreeMap<String, Value>,
) {
    let (label, scope, done_keys) =
        crate::scenarios::contention_ledger::barrier_watch(attempt.run_id);
    probes.insert("barrier_label".into(), json!(label));
    let mut ready = true;
    for key in done_keys {
        let done =
            state_value(ask(context, "state::get", json!({"scope": scope, "key": key})).await);
        ready &= !done.is_null() && done.get("probe_error").is_none();
    }
    probes.insert("barrier_ready".into(), json!(ready));
    let raw_metrics = serde_json::to_value(attempt.metrics).unwrap_or_default();
    let route = |session: &Value| {
        session["by_model"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|model| json!([model["model"], model["provider"]]))
            .collect::<Vec<_>>()
    };
    let mut children = Vec::new();
    for session in raw_metrics["by_session"].as_array().into_iter().flatten() {
        let session_id = session["session_id"].as_str().unwrap_or_default();
        if session_id == attempt.session_id {
            probes.insert("root_route".into(), json!(route(session)));
            continue;
        }
        let meta =
            ask(context, "session::get", json!({"session_id": session_id})).await["meta"].clone();
        let status = ask(
            context,
            "harness::status",
            json!({"session_id": session_id, "verbose": true}),
        )
        .await;
        let transcript = super::answer(context.transcript(session_id).await);
        let head = messages(&transcript).first().cloned().unwrap_or_default();
        children.push(json!({
            "session_id": session_id, "meta": meta, "route": route(session),
            "status": {"status": status["status"], "stop_reason": status["stop_reason"],
                       "max_turns": status["max_turns"]},
            "head": {"entry_id": head["entry_id"], "spawn": head.pointer("/origin/spawn").is_some()},
            "tokens": session["input_tokens"].as_u64().unwrap_or_default()
                + session["output_tokens"].as_u64().unwrap_or_default(),
        }));
    }
    if let Some(leaf) = children
        .first()
        .and_then(|c| c["session_id"].as_str())
        .map(str::to_string)
    {
        let mut wall = Vec::new();
        for function_id in WALLED {
            let reply = invoke_as(context, &leaf, function_id, json!({})).await;
            wall.push(json!({"function_id": function_id, "denied": denied(&reply)}));
        }
        // Every child keeps contract discovery, whatever its spawn narrowed.
        let control = invoke_as(
            context,
            &leaf,
            "engine::functions::info",
            json!({"function_id": "state::get"}),
        )
        .await;
        wall.push(json!({
            "function_id": "engine::functions::info", "denied": denied(&control), "control": true,
        }));
        probes.insert("leaf_wall".into(), json!(wall));
        probes.insert("leaf_bindings".into(), leaf_bindings(context, &leaf).await);
    }
    let ledger = ask(
        context,
        "state::get",
        json!({"scope": "harness_budget", "key": attempt.session_id}),
    )
    .await;
    let ledger = ledger
        .get("value")
        .filter(|value| value.is_object())
        .unwrap_or(&ledger)
        .clone();
    probes.insert("budget".into(), ledger);
    probes.insert("children".into(), json!(children));
    probes.insert("tree_complete".into(), raw_metrics["complete"].clone());
}

/// Registers, as the leaf, a mechanical binding and a long wake, reads what
/// the Harness kept, and removes both, each removal tried whatever the last
/// one answered.
async fn leaf_bindings(context: &E2eContext, leaf: &str) -> Value {
    let timer = |target: Option<&str>| {
        let mut request = json!({"trigger_type": "timer", "config": {"in_ms": PROBE_WAKE_MS},
                                 "once": true, "label": "e2e-mechanism-probe"});
        if let Some(target) = target {
            request["function_id"] = json!(target);
        }
        request
    };
    let mechanical = invoke_as(
        context,
        leaf,
        "engine::register_trigger",
        timer(Some("state::get")),
    )
    .await;
    let wake = invoke_as(context, leaf, "engine::register_trigger", timer(None)).await;
    let listed = ask(
        context,
        "harness::triggers::list",
        json!({"session_id": leaf}),
    )
    .await;
    let probes: Vec<&Value> = listed["subscriptions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["label"] == "e2e-mechanism-probe")
        .collect();
    let wake_row = probes.iter().find(|row| row["target"].is_null());
    let window =
        wake_row.and_then(|row| Some(row["expires_at"].as_i64()? - row["created_at"].as_i64()?));
    let mut unregistered = true;
    for row in &probes {
        let reply = ask(
            context,
            "harness::triggers::unregister",
            json!({"session_id": leaf, "subscription_id": row["subscription_id"]}),
        )
        .await;
        unregistered &= reply.get("probe_error").is_none();
    }
    json!({
        "mechanical_refused": mechanical["is_error"] == true,
        "mechanical_kept": probes.iter().any(|row| !row["target"].is_null()),
        "wake_registered": wake["is_error"] == false,
        "wake_denied": denied(&wake),
        "wake_window_ms": window,
        "unregistered": unregistered,
    })
}

pub(super) fn gates(run: &Run<'_>) -> Vec<Broken> {
    let mut broken = Vec::new();
    let spawned = spawns(run.transcript);
    for (_, call_id, details) in &spawned {
        let named = |field: &str| details[field].as_str().is_some_and(|id| !id.is_empty());
        if details["fire_and_forget"] != true
            || details["reused"] != false
            || !named("child_session_id")
            || !named("child_turn_id")
        {
            broken.push((
                "spawn_contract",
                format!("spawn {call_id} returned {details}"),
            ));
        }
    }
    let children: Vec<&Value> = run
        .probe("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    for child in &children {
        let id = &child["session_id"];
        let head = &child["head"];
        if !head["entry_id"]
            .as_str()
            .is_some_and(|entry| entry.starts_with("e_spawn_"))
            || head["spawn"] != true
        {
            broken.push((
                "spawn_contract",
                format!("{id} opens with {head}, not an e_spawn_ entry with origin.spawn"),
            ));
        }
        let metadata = &child["meta"]["metadata"];
        let linked = spawned.iter().any(|(turn, call_id, details)| {
            details["child_session_id"] == *id
                && metadata["parent_turn_id"] == turn.as_str()
                && metadata["function_call_id"] == call_id.as_str()
        });
        if !linked || metadata["spawned_by"] != "agent" || child["meta"]["kind"] != E2E_SESSION_KIND
        {
            broken.push((
                "linkage_complete",
                format!(
                    "{id} records kind {} and metadata {metadata}, no spawn call matches",
                    child["meta"]["kind"]
                ),
            ));
        }
        let status = &child["status"];
        let turns = status["max_turns"].as_u64().unwrap_or_default();
        if status["status"] != "completed"
            || !status["stop_reason"].is_null()
            || !(1..=MAX_TURNS).contains(&turns)
        {
            broken.push(("child_finished", format!("{id} ended {status}")));
        }
        if let Some(root) = run.probe("root_route") {
            if &child["route"] != root {
                broken.push((
                    "child_route",
                    format!("{id} ran on {}, the root on {root}", child["route"]),
                ));
            }
        }
    }
    if let Some(complete) = run.probe("tree_complete") {
        let nodes = run.metrics.map_or(0, |metrics| metrics.by_session.len());
        if complete != true || nodes != 1 + spawned.len() {
            broken.push((
                "linkage_complete",
                format!(
                    "the session tree is complete={complete} with {nodes} node(s) for {} spawn(s)",
                    spawned.len()
                ),
            ));
        }
    }
    for probe in run
        .probe("leaf_wall")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let control = probe["control"] == true;
        if (probe["denied"] == true) == control {
            broken.push((
                "leaf_wall_enforced",
                format!(
                    "a leaf's {} answered denied={}",
                    probe["function_id"], probe["denied"]
                ),
            ));
        }
    }
    if let Some(bindings) = run.probe("leaf_bindings") {
        // A spawn may narrow a leaf's policy past registering anything.
        let bounded = bindings["wake_denied"] == true
            || (bindings["wake_registered"] == true
                && bindings["wake_window_ms"]
                    .as_i64()
                    .is_some_and(|ms| ms <= LEAF_WAKE_CAP_MS));
        if bindings["mechanical_refused"] != true
            || bindings["mechanical_kept"] != false
            || !bounded
            || bindings["unregistered"] == false
        {
            broken.push((
                "leaf_bindings_bounded",
                format!("a leaf's bindings: {bindings}"),
            ));
        }
    }
    if let Some(budget) = run.probe("budget") {
        let children_tokens: u64 = children.iter().filter_map(|c| c["tokens"].as_u64()).sum();
        if budget["reserved_tokens"] != 0
            || budget["used_tokens"].as_u64().unwrap_or_default() < children_tokens
        {
            broken.push((
                "budget_settled",
                format!(
                    "the shared ledger reads {budget}; the children used {children_tokens} tokens"
                ),
            ));
        }
    }
    if let Some(label) = run.probe("barrier_label").and_then(Value::as_str) {
        let ready = run.probe("barrier_ready") == Some(&Value::Bool(true));
        broken.extend(barrier(run.transcript, label, ready));
    }
    broken
}

/// The barrier wake that woke the root skipped the first two done keys
/// without a failed condition, fired once and retired, and its fire is the
/// one notification it left. A registration the model got wrong and
/// replaced is the criteria's business, not the Harness's; a barrier that
/// watched (it skipped a fire) and never woke once every done key was
/// written is the Harness's.
fn barrier(transcript: &Value, label: &str, ready: bool) -> Vec<Broken> {
    let fires: Vec<&Value> = customs(transcript, "trigger_fired")
        .map(|entry| &entry["custom"]["data"])
        .filter(|data| data["label"] == label)
        .collect();
    let Some(sub) = fires
        .iter()
        .find(|data| data["outcome"] == "delivered")
        .and_then(|data| data["subscription_id"].as_str())
    else {
        // A subscription whose condition failed is the model's wrong one.
        let failed = |sub: &str| {
            messages(transcript)
                .iter()
                .filter_map(|entry| text(entry, "/entry_id"))
                .any(|id| id.starts_with(&format!("e_condfail_{sub}")))
        };
        let watched = fires.iter().any(|data| {
            data["outcome"] == "skipped"
                && data["subscription_id"]
                    .as_str()
                    .is_some_and(|sub| !failed(sub))
        });
        return if ready && watched {
            vec![(
                "barrier_fired_once",
                format!(
                    "every done key is written, yet the barrier that skipped {} fire(s) never woke the root",
                    fires.len()
                ),
            )]
        } else {
            Vec::new()
        };
    };
    let fires: Vec<&&Value> = fires
        .iter()
        .filter(|data| data["subscription_id"] == sub)
        .collect();
    let skipped: Vec<&&&Value> = fires
        .iter()
        .filter(|data| data["outcome"] == "skipped")
        .collect();
    let delivered: Vec<&&&Value> = fires
        .iter()
        .filter(|data| data["outcome"] == "delivered")
        .collect();
    let notifications = messages(transcript)
        .iter()
        .filter(|entry| text(entry, "/origin/binding") == Some(sub))
        .count();
    let condition_failures = messages(transcript)
        .iter()
        .filter_map(|entry| text(entry, "/entry_id"))
        .filter(|id| id.starts_with(&format!("e_condfail_{sub}")))
        .count();
    let barrier_skips = skipped.iter().all(|data| {
        data["note"]
            .as_str()
            .is_some_and(|note| note.contains("state::barrier"))
    });
    let consumed =
        matches!(delivered.as_slice(), [data] if data["retirement_reason"] == "once_consumed");
    if skipped.len() != 2
        || !barrier_skips
        || !consumed
        || notifications != 1
        || condition_failures != 0
    {
        return vec![(
            "barrier_fired_once",
            format!(
                "{sub}: {} skip(s) (all by state::barrier: {barrier_skips}), {} delivery(ies) \
                 once_consumed={consumed}, {notifications} notification(s), \
                 {condition_failures} condition failure(s)",
                skipped.len(),
                delivered.len()
            ),
        )];
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{Observed, SessionMetricsPayload, SessionUsage, SessionUsageTotals};

    const SUB: &str = "sub_db8026022599493e93435fce9ebc981f";
    const LABEL: &str = "ledger-balanced:r";

    fn transcript() -> Value {
        let mut entries = vec![
            json!({"entry_id": "e_t1_5_assistant", "origin": {"turn_id": "t1"},
            "message": {"role": "assistant", "content": [
                {"type": "function_call", "id": "c0", "function_id": "harness::spawn", "arguments": {}},
            ]}}),
        ];
        entries.push(json!({"entry_id": "e_t1_c0", "message": {"role": "function_result",
            "function_call_id": "c0", "function_id": "harness::spawn", "is_error": false,
            "details": {"child_session_id": "w0", "child_turn_id": "tw0", "fire_and_forget": true, "reused": false}}}));
        for (index, outcome) in ["skipped", "skipped", "delivered"].iter().enumerate() {
            entries.push(json!({"entry_id": format!("e_trig_{index}"), "custom": {"custom_type": "trigger_fired", "data": {
                "label": LABEL, "subscription_id": SUB, "outcome": outcome,
                "note": "`state::barrier` skipped this fire", "retirement_reason": "once_consumed"}}}));
        }
        entries.push(json!({"entry_id": format!("e_fire_{SUB}_1"), "origin": {"binding": SUB, "notification": true}}));
        json!({"messages": entries})
    }

    fn child() -> Value {
        json!({"session_id": "w0", "route": [["deepseek-flash", "deepseek"]], "tokens": 900,
            "head": {"entry_id": "e_spawn_abc", "spawn": true},
            "meta": {"kind": "e2e", "metadata": {"parent_turn_id": "t1", "function_call_id": "c0", "spawned_by": "agent"}},
            "status": {"status": "completed", "stop_reason": null, "max_turns": 24}})
    }

    fn probes(child: Value) -> BTreeMap<String, Value> {
        serde_json::from_value(json!({
            "barrier_label": LABEL, "root_route": [["deepseek-flash", "deepseek"]],
            "children": [child], "tree_complete": true,
            "leaf_wall": [
                {"function_id": "harness::spawn", "denied": true},
                {"function_id": "engine::functions::info", "denied": false, "control": true},
            ],
            "leaf_bindings": {"mechanical_refused": true, "mechanical_kept": false,
                              "wake_registered": true, "wake_window_ms": 1_800_000},
            "budget": {"reserved_tokens": 0, "used_tokens": 5_000},
        }))
        .unwrap()
    }

    fn metrics(nodes: usize) -> crate::wire::SessionMetricsResponse {
        let session = |id: String| SessionUsage {
            session_id: id,
            parent_session_id: None,
            depth: 0,
            turns: 1,
            function_calls: 0,
            function_call_errors: 0,
            validation_retries: None,
            transient_resumes: None,
            wake_resumes: None,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            cost_usd: None,
            context: None,
            by_model: Vec::new(),
        };
        Observed::from_normalized(SessionMetricsPayload {
            root_session_id: "root".into(),
            complete: true,
            totals: SessionUsageTotals::default(),
            by_session: (0..nodes).map(|i| session(format!("s{i}"))).collect(),
            traces: None,
        })
    }

    fn gates_of(transcript: &Value, probes: &BTreeMap<String, Value>, nodes: usize) -> Vec<String> {
        let metrics = metrics(nodes);
        gates(&Run {
            transcript,
            metrics: Some(&metrics),
            probes,
        })
        .into_iter()
        .map(|(gate, detail)| format!("{gate}: {detail}"))
        .collect()
    }

    #[test]
    fn a_fan_out_that_kept_every_contract_keeps_every_gate() {
        assert_eq!(
            gates_of(&transcript(), &probes(child()), 2),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_broken_child_wall_budget_or_barrier_is_flagged() {
        let mut transcript = transcript();
        transcript["messages"][1]["message"]["details"]["reused"] = json!(true);
        transcript["messages"][2]["custom"]["data"]["outcome"] = json!("delivered");
        let mut child = child();
        child["head"]["entry_id"] = json!("e_t_x_0_user");
        child["meta"]["metadata"]["function_call_id"] = json!("other");
        child["status"]["stop_reason"] = json!("max_turns");
        child["route"] = json!([["deepseek-pro", "deepseek"]]);
        let mut probes = probes(child);
        probes.insert(
            "leaf_wall".into(),
            json!([{"function_id": "harness::spawn", "denied": false}]),
        );
        probes.insert(
            "leaf_bindings".into(),
            json!({"mechanical_refused": true, "mechanical_kept": false,
            "wake_registered": true, "wake_window_ms": 7_200_000}),
        );
        probes.insert(
            "budget".into(),
            json!({"reserved_tokens": 12, "used_tokens": 5_000}),
        );

        let gates: Vec<String> = gates_of(&transcript, &probes, 3)
            .into_iter()
            .map(|broken| broken.split(':').next().unwrap().to_string())
            .collect();

        assert_eq!(
            gates,
            [
                "spawn_contract",
                "spawn_contract",
                "linkage_complete",
                "child_finished",
                "child_route",
                "linkage_complete",
                "leaf_wall_enforced",
                "leaf_bindings_bounded",
                "budget_settled",
                "barrier_fired_once",
            ]
        );
    }

    #[test]
    fn a_leaf_whose_spawn_narrowed_away_registration_is_not_asked_for_a_wake() {
        let mut probes = probes(child());
        probes.insert(
            "leaf_bindings".into(),
            json!({"mechanical_refused": true, "mechanical_kept": false,
                   "wake_registered": false, "wake_denied": true, "wake_window_ms": null}),
        );

        assert!(gates_of(&transcript(), &probes, 2).is_empty());
    }

    #[test]
    fn a_barrier_the_model_registered_wrong_and_replaced_is_not_judged() {
        let mut transcript = transcript();
        let messages = transcript["messages"].as_array_mut().unwrap();
        messages.push(json!({"entry_id": "e_condfail_sub_wrong"}));
        messages.push(
            json!({"entry_id": "e_trig_wrong", "custom": {"custom_type": "trigger_fired",
            "data": {"label": LABEL, "subscription_id": "sub_wrong", "outcome": "skipped",
                     "note": "`state::barrier`: condition call failed"}}}),
        );

        assert!(gates_of(&transcript, &probes(child()), 2).is_empty());
    }

    #[test]
    fn a_barrier_that_never_woke_the_root_after_every_done_key_is_flagged() {
        let mut transcript = transcript();
        transcript["messages"][4]["custom"]["data"]["outcome"] = json!("skipped");
        let mut probes = probes(child());

        // The writers never finished: the barrier had nothing to wake on.
        probes.insert("barrier_ready".into(), json!(false));
        assert!(gates_of(&transcript, &probes, 2).is_empty());

        probes.insert("barrier_ready".into(), json!(true));
        let broken = gates_of(&transcript, &probes, 2);
        assert_eq!(broken.len(), 1, "{broken:?}");
        assert!(broken[0].starts_with("barrier_fired_once"), "{broken:?}");
    }

    #[test]
    fn a_probe_wake_the_leaf_could_not_unregister_is_flagged() {
        let mut probes = probes(child());
        probes.insert(
            "leaf_bindings".into(),
            json!({"mechanical_refused": true, "mechanical_kept": false,
                   "wake_registered": true, "wake_window_ms": 1_800_000,
                   "unregistered": false}),
        );

        let broken = gates_of(&transcript(), &probes, 2);

        assert_eq!(broken.len(), 1, "{broken:?}");
        assert!(broken[0].starts_with("leaf_bindings_bounded"), "{broken:?}");
    }

    #[test]
    fn a_replayed_run_is_judged_on_its_spawns_and_barrier() {
        let probes = BTreeMap::from([("barrier_label".to_string(), json!(LABEL))]);

        assert!(gates_of(&transcript(), &probes, 2).is_empty());
    }
}
