//! E6: an allow-`*` session's edges. A resent message is the same turn, the
//! invocation chokepoint denies the Harness's private functions whatever the
//! policy, the default working directory is stated, and a `pre_generate`
//! append is persisted once however often it is replayed.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{ask, customs, denied, invoke_as, messages, text, Attempt, Broken, Prompt, Run};
use crate::context::E2eContext;

pub const ID: &str = crate::scenarios::persistent_state::ID;
/// The origin annotation the appending hook stamps on the step it appends to.
pub const ANNOTATION: &str = "e2e_mechanism_probe";

/// The message the `pre_generate` probe appends once, on the first step; the
/// scenario's definition carries it, since every later prompt replays it.
pub fn marker(run_id: &str) -> String {
    format!("[e2e mechanism probe {run_id}] This line needs no action or reply.")
}

/// Private Harness functions an agent must never reach, even under `*`.
fn chokepoint_probes(run_id: &str) -> [(&'static str, Value); 4] {
    let absent = format!("e2e_probe_absent_{run_id}");
    [
        (
            "harness::state::get",
            json!({"scope": "e2e_probe", "key": absent}),
        ),
        (
            "harness::delete-session-tree",
            json!({"session_id": absent}),
        ),
        ("state::claim-namespace", json!({"namespace": absent})),
        (
            "harness::function::trigger",
            json!({"session_id": absent, "call": {"id": "c", "function_id": "state::get", "arguments": {}}}),
        ),
    ]
}

pub(super) async fn capture(
    context: &E2eContext,
    attempt: &Attempt<'_>,
    resend: Option<Value>,
    prompts: &[Prompt],
    probes: &mut BTreeMap<String, Value>,
) {
    let mut chokepoint = Vec::new();
    for (function_id, arguments) in chokepoint_probes(attempt.run_id) {
        let reply = invoke_as(context, attempt.session_id, function_id, arguments).await;
        chokepoint.push(json!({"function_id": function_id, "denied": denied(&reply),
                               "timed_out": reply["timed_out"] == true}));
    }
    probes.insert("chokepoint".into(), json!(chokepoint));
    probes.insert(
        "filesystem_info".into(),
        ask(context, "harness::filesystem::info", json!({})).await,
    );
    // Without a compose worker there is nothing to route to.
    if context
        .function_exists("compose::status")
        .await
        .unwrap_or(false)
    {
        let reply = invoke_as(context, attempt.session_id, "compose::status", json!({})).await;
        probes.insert(
            "compose_status".into(),
            json!({"denied": denied(&reply), "is_error": reply["is_error"],
                   "timed_out": reply["timed_out"] == true}),
        );
    }
    probes.insert("resend".into(), resend.unwrap_or(Value::Null));
    probes.insert("marker".into(), json!(marker(attempt.run_id)));
    probes.insert("observed_prompts".into(), json!(prompts.len()));
    probes.insert(
        "replayed_prompts".into(),
        json!(prompts
            .iter()
            .filter(|prompt| prompt.replays_append)
            .count()),
    );
}

pub(super) fn gates(run: &Run<'_>) -> Vec<Broken> {
    let mut broken = Vec::new();
    if let Some(resend) = run.probe("resend").filter(|resend| !resend.is_null()) {
        let reply = &resend["reply"];
        let sends = messages(run.transcript)
            .iter()
            .filter_map(|entry| text(entry, "/entry_id"))
            .filter(|id| id.starts_with("e_idem_"))
            .count();
        if reply["accepted"] != true
            || reply["turn_id"] != resend["first_turn_id"]
            || reply["queued"] == true
            || reply["merged"] == true
            || sends != 1
        {
            broken.push((
                "idempotent_resend",
                format!(
                    "the resend answered {reply} for turn {}; {sends} sent message(s)",
                    resend["first_turn_id"]
                ),
            ));
        }
    }
    for probe in run
        .probe("chokepoint")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if probe["denied"] != true || probe["timed_out"] == true {
            broken.push((
                "chokepoint_denies",
                format!(
                    "{} answered denied={} timed_out={} under allow *",
                    probe["function_id"], probe["denied"], probe["timed_out"]
                ),
            ));
        }
    }
    if let Some(error) = run
        .probe("filesystem_info")
        .and_then(|info| info.get("probe_error"))
    {
        broken.push((
            "default_root_aid",
            format!("harness::filesystem::info answered {error}"),
        ));
    }
    if let Some(root) = run
        .probe("filesystem_info")
        .and_then(|info| info["default_root"].as_str())
    {
        let line = format!(
            "Your working directory is {root} (default directory, not an access boundary)."
        );
        let stated = customs(run.transcript, "runtime_context")
            .filter_map(|entry| text(entry, "/custom/data/aid"))
            .any(|aid| aid.lines().any(|l| l == line));
        if !stated {
            broken.push((
                "default_root_aid",
                format!("no runtime context states `{line}`"),
            ));
        }
    }
    // Routed is what the chokepoint owns: compose's own failure (no project,
    // a stopped stack) is not the Harness's, and is kept only as evidence.
    if let Some(status) = run.probe("compose_status") {
        if status["denied"] == true || status["timed_out"] == true {
            broken.push((
                "compose_status_routed",
                format!("compose::status answered {status}"),
            ));
        }
    }
    if let Some(marker) = run.probe("marker").and_then(Value::as_str) {
        let notices = customs(run.transcript, "model_notice")
            .filter(|entry| text(entry, "/custom/data/text").is_some_and(|t| t.contains(marker)))
            .count();
        let annotated = messages(run.transcript)
            .iter()
            .any(|entry| entry.pointer(&format!("/origin/{ANNOTATION}")).is_some());
        let observed = run.probe("observed_prompts").and_then(Value::as_u64);
        let replayed = run.probe("replayed_prompts").and_then(Value::as_u64);
        let replayed_on_every_later_step = observed
            .zip(replayed)
            .is_none_or(|(observed, replayed)| replayed + 1 == observed);
        if notices != 1 || !annotated || !replayed_on_every_later_step {
            broken.push((
                "pre_generate_append_once",
                format!(
                    "one append left {notices} notice(s), annotated={annotated}, replayed on \
                     {replayed:?} of {observed:?} step(s) after the first"
                ),
            ));
        }
    }
    broken
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(notices: usize, annotated: bool) -> Value {
        let mut entries = vec![
            json!({"entry_id": "e_idem_e2e~3Ar~3Apersistent_state~3Asend~3A0", "message": {"role": "user"}}),
            json!({"entry_id": "e_t1_0_runtime_context", "custom": {"custom_type": "runtime_context", "data": {
                "aid": "Your session id is s.\nYour working directory is /tmp/p (default directory, not an access boundary).\nResponse language: English."}}}),
        ];
        for index in 0..notices {
            entries.push(
                json!({"entry_id": format!("e_t1_{index}_notice_1"), "custom": {
                "custom_type": "model_notice", "data": {"kind": "hook", "text": marker("r")}}}),
            );
        }
        let origin = if annotated {
            json!({ANNOTATION: true})
        } else {
            json!({})
        };
        entries.push(json!({"entry_id": "e_t1_0_assistant", "origin": origin, "message": {"role": "assistant"}}));
        json!({"messages": entries})
    }

    fn probes(denied: bool) -> BTreeMap<String, Value> {
        serde_json::from_value(json!({
            "resend": {"first_turn_id": "t1", "reply": {"accepted": true, "turn_id": "t1", "session_id": "s"}},
            "chokepoint": [{"function_id": "harness::state::get", "denied": denied, "timed_out": false}],
            "filesystem_info": {"default_root": "/tmp/p"},
            "compose_status": {"denied": false, "is_error": false},
            "marker": marker("r"),
            "observed_prompts": 3,
            "replayed_prompts": 2,
        }))
        .unwrap()
    }

    fn gates_of(transcript: &Value, probes: &BTreeMap<String, Value>) -> Vec<String> {
        gates(&Run {
            transcript,
            metrics: None,
            probes,
        })
        .into_iter()
        .map(|(gate, detail)| format!("{gate}: {detail}"))
        .collect()
    }

    #[test]
    fn an_allow_all_session_that_kept_its_edges_keeps_every_gate() {
        assert_eq!(
            gates_of(&transcript(1, true), &probes(true)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_compose_status_that_routed_but_failed_is_not_blamed_on_the_harness() {
        let mut probes = probes(true);
        probes.insert(
            "compose_status".into(),
            json!({"denied": false, "is_error": true}),
        );

        assert_eq!(
            gates_of(&transcript(1, true), &probes),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_reached_private_function_a_new_turn_or_a_stacked_append_is_flagged() {
        let mut probes = probes(false);
        probes.insert(
            "resend".into(),
            json!({"first_turn_id": "t1", "reply": {"accepted": true, "turn_id": "t2"}}),
        );
        probes.insert(
            "filesystem_info".into(),
            json!({"default_root": "/elsewhere"}),
        );
        probes.insert(
            "compose_status".into(),
            json!({"denied": true, "is_error": true}),
        );

        let gates: Vec<String> = gates_of(&transcript(2, false), &probes)
            .into_iter()
            .map(|broken| broken.split(':').next().unwrap().to_string())
            .collect();

        assert_eq!(
            gates,
            [
                "idempotent_resend",
                "chokepoint_denies",
                "default_root_aid",
                "compose_status_routed",
                "pre_generate_append_once",
            ]
        );
    }
}
