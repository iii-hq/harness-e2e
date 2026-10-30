//! E7: the Harness stamps the session's own filesystem scope on every
//! scoped call, even over a forged one, and its grants are a deduplicated
//! set that a revoke empties.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::{json, Value};

use super::{ask, invoke_as, Attempt, Broken, Run, PROBE_CALL_PREFIX};
use crate::context::E2eContext;
use crate::scenarios::common::lock_unpoisoned;

pub const ID: &str = crate::scenarios::shell_coder_sandbox::ID;
/// The calls the `pre_trigger` observer sees: the scoped ones and the two
/// compositions that carry the stamp for their steps.
pub const OBSERVED: [&str; 4] = ["shell::*", "coder::*", "fp::pipe", "fp::batch"];
/// The command the forged probe runs; it reads nothing and writes nothing.
const FORGED_COMMAND: &str = "pwd";

pub(super) async fn capture(
    context: &E2eContext,
    attempt: &Attempt<'_>,
    calls: &Mutex<Vec<Value>>,
    probes: &mut BTreeMap<String, Value>,
) {
    let forged = invoke_as(
        context,
        attempt.session_id,
        "shell::exec",
        json!({"command": FORGED_COMMAND, "args": [],
               "fs_scope": {"root": "/etc", "grants": ["/"], "boundary": "configured_roots"}}),
    )
    .await;
    probes.insert(
        "forged_reply".into(),
        json!({"is_error": forged["is_error"], "details": forged["details"]}),
    );
    probes.insert(
        "stamped".into(),
        json!(std::mem::take(&mut *lock_unpoisoned(calls))),
    );
    probes.insert(
        "workspace".into(),
        json!(attempt
            .filesystem_root
            .map(|root| root.display().to_string())),
    );
    probes.insert(
        "boundary".into(),
        ask(context, "harness::filesystem::info", json!({})).await["boundary"].clone(),
    );
    let root = format!("/tmp/e2e-probe-grant-{}", attempt.run_id);
    let grant = json!({"session_id": attempt.session_id, "root": root});
    let mut grants = Vec::new();
    for function_id in [
        "harness::filesystem::grant",
        "harness::filesystem::grant",
        "harness::filesystem::revoke",
    ] {
        let reply = ask(context, function_id, grant.clone()).await;
        grants.push(json!(reply["roots"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|granted| granted.as_str() == Some(&root))
            .count()));
    }
    probes.insert("grants".into(), json!(grants));
}

pub(super) fn gates(run: &Run<'_>) -> Vec<Broken> {
    let mut broken = Vec::new();
    let Some(workspace) = run.probe("workspace").and_then(Value::as_str) else {
        return broken;
    };
    let boundary = run.probe("boundary").unwrap_or(&Value::Null);
    let stamped: Vec<&Value> = run
        .probe("stamped")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        // The Harness stamps no scope on these (its `is_scoped_function`):
        // they take their own paths.
        .filter(|call| {
            !call["function_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("shell::filesystem::"))
        })
        .collect();
    if run.probe("stamped").is_some() && stamped.is_empty() {
        broken.push((
            "fs_scope_stamped",
            "the pre_trigger observer saw no scoped call".into(),
        ));
    }
    for call in &stamped {
        let scope = &call["fs_scope"];
        if scope["root"] != workspace
            || scope["grants"] != json!([])
            || &scope["boundary"] != boundary
        {
            broken.push((
                "fs_scope_stamped",
                format!("{} ran under {scope}, expected root {workspace}, no grants, boundary {boundary}", call["function_id"]),
            ));
        }
    }
    if let Some(reply) = run.probe("forged_reply") {
        // The probe's own call, not a `pwd` the model ran.
        let rewritten = stamped.iter().any(|call| {
            call["call_id"]
                .as_str()
                .is_some_and(|id| id.starts_with(PROBE_CALL_PREFIX))
                && call["command"] == FORGED_COMMAND
                && call["fs_scope"]["root"] == workspace
        });
        if !rewritten || reply["is_error"] != false {
            broken.push((
                "forged_scope_rewritten",
                format!(
                    "the probe's forged /etc scope was not replaced by the workspace \
                     (rewritten={rewritten}, reply {reply})"
                ),
            ));
        }
    }
    if let Some(grants) = run.probe("grants") {
        if grants != &json!([1, 1, 0]) {
            broken.push((
                "filesystem_grants",
                format!("grant, grant again and revoke left the root listed {grants} time(s), expected [1,1,0]"),
            ));
        }
    }
    broken
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(function_id: &str, command: Value, root: &str) -> Value {
        let call_id = if command == json!(FORGED_COMMAND) {
            "e2e_probe_1"
        } else {
            "call_1"
        };
        json!({"function_id": function_id, "command": command, "call_id": call_id,
               "fs_scope": {"root": root, "grants": [], "boundary": "workspace"}})
    }

    fn gates_of(stamped: Value, grants: Value) -> Vec<String> {
        gates_with(stamped, grants, json!({"is_error": false}))
    }

    fn gates_with(stamped: Value, grants: Value, forged_reply: Value) -> Vec<String> {
        let probes = serde_json::from_value(json!({
            "workspace": "/w", "boundary": "workspace", "forged_reply": forged_reply,
            "stamped": stamped, "grants": grants,
        }))
        .unwrap();
        let transcript = json!({"messages": []});
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
    fn calls_stamped_with_the_workspace_and_a_rewritten_forgery_keep_every_gate() {
        let stamped = json!([
            call("coder::read-file", Value::Null, "/w"),
            call("shell::filesystem::ls", Value::Null, "/elsewhere"),
            call("shell::exec", json!(FORGED_COMMAND), "/w"),
        ]);

        assert_eq!(gates_of(stamped, json!([1, 1, 0])), Vec::<String>::new());
    }

    #[test]
    fn a_widened_scope_a_kept_forgery_or_duplicated_grants_are_flagged() {
        let stamped = json!([
            call("coder::read-file", Value::Null, "/"),
            call("shell::exec", json!(FORGED_COMMAND), "/etc"),
        ]);

        let gates: Vec<String> = gates_of(stamped, json!([1, 2, 0]))
            .into_iter()
            .map(|broken| broken.split(':').next().unwrap().to_string())
            .collect();

        assert_eq!(
            gates,
            [
                "fs_scope_stamped",
                "fs_scope_stamped",
                "forged_scope_rewritten",
                "filesystem_grants"
            ]
        );
    }

    #[test]
    fn the_models_own_pwd_does_not_stand_in_for_the_forged_probe() {
        let mut models = call("shell::exec", json!(FORGED_COMMAND), "/w");
        models["call_id"] = json!("call_7");
        let stamped = json!([call("coder::read-file", Value::Null, "/w"), models]);

        let broken = gates_of(stamped, json!([1, 1, 0]));

        assert_eq!(broken.len(), 1, "{broken:?}");
        assert!(
            broken[0].starts_with("forged_scope_rewritten"),
            "{broken:?}"
        );
    }

    #[test]
    fn a_forged_probe_that_failed_is_flagged() {
        let stamped = json!([call("shell::exec", json!(FORGED_COMMAND), "/w")]);

        let broken = gates_with(
            stamped,
            json!([1, 1, 0]),
            json!({"is_error": true, "details": {"error": "invalid_fs_scope"}}),
        );

        assert_eq!(broken.len(), 1, "{broken:?}");
        assert!(
            broken[0].starts_with("forged_scope_rewritten"),
            "{broken:?}"
        );
    }

    #[test]
    fn an_observer_that_saw_nothing_is_flagged() {
        assert_eq!(
            gates_of(json!([]), json!([1, 1, 0])),
            [
                "fs_scope_stamped: the pre_trigger observer saw no scoped call",
                "forged_scope_rewritten: the probe's forged /etc scope was not replaced by the \
                 workspace (rewritten=false, reply {\"is_error\":false})",
            ]
        );
    }
}
