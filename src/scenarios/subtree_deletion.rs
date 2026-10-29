//! `subtree_deletion` — deleting a sub-agent's subtree while its parent
//! session lives on. Between the parent's two exchanges, with P idle (its
//! first turn has ended), the runner builds parent P → child C → grandchild
//! G with direct `harness::spawn` calls, each child answering one line, then:
//!
//! - `harness::delete-session-tree(C)` returns the `delete_<sha256(C)>`
//!   operation, which completes having deleted exactly C and G, both gone
//!   from session-manager and `harness::status`;
//! - the tombstone refuses a send to C and a spawn under it;
//! - P is untouched and gets exactly one `e_<op>_parent` notice;
//! - a third child D deleted directly with `session::delete` loses its
//!   `harness::status` record (`harness::on-session-deleted`).
//!
//! P's second exchange then has to complete as usual. A delete while P's
//! own turn runs or waits on C is not exercised here.

use std::time::Duration;

use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::mechanism::{self, call, record};
use super::{
    async_trait, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation, Scenario,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "subtree_deletion";
const DELIVERABLE_ID: &str = "subtree_deletion_checks";
const WAIT: Duration = Duration::from_secs(150);
const CHILD_TASK: &str =
    "Reply with exactly `CHILD-OK` and nothing else. Do not call any function.";

const DELETED: AssessmentSpec = AssessmentSpec::gate(
    "subtree_deleted",
    35,
    "Did `harness::delete-session-tree` on the child return the `delete_<sha256>` operation and complete with exactly the child and grandchild deleted from session-manager and `harness::status`?",
)
.labeled("Deleting a child removes it and its grandchild");
const TOMBSTONE: AssessmentSpec = AssessmentSpec::gate(
    "tombstone",
    20,
    "Did the deleted child's tombstone refuse both a send to it and a spawn under it?",
)
.labeled("A deleted session refuses sends and spawns");
const PARENT: AssessmentSpec = AssessmentSpec::gate(
    "parent_kept",
    30,
    "Did the parent survive with one `e_<op>_parent` notice, no deleted child left in its tree, and its next exchange completed?",
)
.labeled("The parent keeps going with one notice");
const DIRECT_DELETE: AssessmentSpec = AssessmentSpec::gate(
    "direct_delete",
    15,
    "Did a child deleted directly with `session::delete` lose its `harness::status` record?",
)
.labeled("A directly deleted session leaves no Harness record");
const ASSESSMENTS: &[AssessmentSpec] = &[DELETED, TOMBSTONE, PARENT, DIRECT_DELETE];

fn code(run_id: &str, exchange: usize) -> String {
    let hex = super::stable_seed(&format!("{ID}:{run_id}")) & 0xff_ffff;
    format!("SUB-{hex:06x}-{exchange}")
}

fn prompts(run_id: &str) -> [String; 2] {
    [0, 1].map(|exchange| {
        format!(
            "Reply with exactly `ACK {}` and nothing else. Do not call any function.",
            code(run_id, exchange)
        )
    })
}

fn child(run_id: &str, name: &str) -> String {
    format!("{}_{name}", mechanism::session_id(run_id))
}

fn operation_id(session_id: &str) -> String {
    let digest = crate::artifact::sha256_bytes(session_id.as_bytes());
    format!("delete_{}", digest.trim_start_matches("sha256:"))
}

pub struct SubtreeDeletion;

#[async_trait]
impl Scenario for SubtreeDeletion {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Delete a sub-agent and its children")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Two one-line replies; between them the runner spawns a child and a grandchild, deletes that subtree and checks what is left.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("Deleting a sub-agent stops and removes exactly its subtree, keeps it from coming back, and tells the parent once while it keeps working. If it fails, deleted agents keep running or reappear, data survives a delete, or the parent loses its own history.")
    }

    /// One retained cohort: the probes do not vary with a seed.
    fn case(&self, _seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            super::stable_seed(ID),
            json!({ "subtree": ["child", "grandchild"] }),
            vec![Capability::E2eControlPlaneV1, Capability::E2eSubagents],
            mechanism::contract(DELIVERABLE_ID),
        )
    }

    fn canonical_seed_only(&self) -> bool {
        true
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let [prompt, _] = prompts(run_id);
        ScenarioSpec {
            id: ID,
            prompt,
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(3),
                max_output_tokens: Some(1_024),
                max_total_tokens: Some(80_000),
                stuck_timeout_seconds: 180,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    fn dialogue_followups(&self, run_id: &str) -> Vec<String> {
        prompts(run_id).into_iter().skip(1).collect()
    }

    fn allowed_functions(&self, _run_id: &str) -> Option<Vec<String>> {
        Some(Vec::new())
    }

    /// What the runner tells the children it spawns is part of the task.
    fn planted_state(&self, _run_id: &str) -> Option<Value> {
        Some(json!({ "runner_messages": { "child_task": CHILD_TASK } }))
    }

    async fn setup(&self, _context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        mechanism::reset(run_id);
        for name in ["c", "g", "d"] {
            mechanism::keep_session(run_id, &child(run_id, name));
        }
        Ok(())
    }

    async fn before_followup(
        &self,
        context: &E2eContext,
        run_id: &str,
        session_id: &str,
        _exchange: usize,
    ) {
        let route = match context.transcript(session_id).await {
            Ok(transcript) => mechanism::subject_route(&transcript),
            Err(_) => None,
        };
        let Some(route) = route else {
            record(
                run_id,
                "tree_built",
                false,
                "no assistant reply to take the route from",
            );
            return;
        };
        delete_subtree(context, run_id, session_id, &route).await;
    }

    async fn capture(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let reply = observation.response.trim();
        let code = code(run_id, 1);
        record(
            run_id,
            "parent_continues",
            reply.contains(&code),
            format!("reply={reply:?} expected {code}"),
        );
        Ok(vec![mechanism::deliverable(DELIVERABLE_ID, run_id)])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        if !observation.metrics.complete {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "metrics_complete",
                "subject metrics are incomplete",
            ));
        }
        let checks = mechanism::observed_checks(observation, run_id);
        // A child whose provider dropped its one line says nothing about
        // deletion; any other fault building the tree is the Harness's.
        if let Some(check) = checks
            .iter()
            .find(|check| check.id == "children_answered" && !check.passed)
        {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "children_answered",
                format!("a child's provider dropped its turn: {}", check.detail),
            ));
        }
        Ok(assessment::build_evaluation([
            mechanism::grade(
                DELETED,
                &checks,
                &[
                    "tree_built",
                    "operation_id",
                    "deletion_completed",
                    "subtree_gone",
                ],
            ),
            mechanism::grade(
                TOMBSTONE,
                &checks,
                &["tombstone_refuses_send", "tombstone_refuses_spawn"],
            ),
            mechanism::grade(
                PARENT,
                &checks,
                &["parent_notice_once", "parent_tree", "parent_continues"],
            ),
            mechanism::grade(DIRECT_DELETE, &checks, &["direct_delete_purges"]),
        ]))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        mechanism::release(context, run_id).await
    }
}

/// A direct spawn of `session_id` under `parent`, answering one line.
async fn spawn(
    context: &E2eContext,
    parent: &str,
    session_id: &str,
    route: &(String, String),
) -> anyhow::Result<Value> {
    context
        .trigger_value(
            "harness::spawn",
            json!({
                "task": CHILD_TASK,
                "session_id": session_id,
                "parent_session_id": parent,
                "model": route.0,
                "provider": route.1,
                "display": { "name": "E2E child" },
                "options": { "max_turns": 2, "functions": { "allow": [], "deny": [] } },
            }),
        )
        .await
}

/// `harness::status` of `session_id` once its turn is terminal; `Some(Null)`
/// when the Harness holds no record.
async fn settled(context: &E2eContext, session_id: &str) -> Option<Value> {
    mechanism::poll(WAIT, || async {
        let status = context
            .trigger_value("harness::status", json!({ "session_id": session_id }))
            .await
            .ok()?;
        (status.is_null()
            || matches!(
                status["status"].as_str(),
                Some("completed" | "failed" | "cancelled")
            ))
        .then_some(status)
    })
    .await
}

async fn delete_subtree(
    context: &E2eContext,
    run_id: &str,
    parent: &str,
    route: &(String, String),
) {
    let (c, g, d) = (child(run_id, "c"), child(run_id, "g"), child(run_id, "d"));
    let mut built = Vec::new();
    for (session, under) in [(&c, parent), (&g, c.as_str()), (&d, parent)] {
        let spawned = spawn(context, under, session, route).await;
        let status = match &spawned {
            Ok(_) => settled(context, session).await,
            Err(_) => None,
        };
        built.push(format!(
            "{session}: spawn={} status={:?}",
            spawned
                .as_ref()
                .map_or_else(|e| format!("{e:#}"), |_| "ok".into()),
            status.as_ref().map(|status| status["status"].clone())
        ));
        match child_outcome(&status) {
            ChildOutcome::Answered => {}
            ChildOutcome::ModelSilent => {
                record(run_id, "children_answered", false, built.join("; "));
                return;
            }
            ChildOutcome::HarnessFault => {
                record(run_id, "tree_built", false, built.join("; "));
                return;
            }
        }
    }
    let tree = context
        .trigger_value(
            "harness::session-tree",
            json!({ "root_session_id": parent }),
        )
        .await
        .unwrap_or(Value::Null);
    let members = tree_members(&tree);
    record(
        run_id,
        "tree_built",
        [&c, &g, &d].iter().all(|id| members.contains(id)),
        format!("{}; tree={members:?}", built.join("; ")),
    );

    let expected_op = operation_id(&c);
    let started = context
        .trigger_value("harness::delete-session-tree", json!({ "session_id": c }))
        .await;
    record(
        run_id,
        "operation_id",
        matches!(&started, Ok(snapshot) if snapshot["operation_id"] == expected_op.as_str()
            && snapshot["session_id"] == c.as_str()),
        format!("expected {expected_op}: {started:?}"),
    );
    let finished = mechanism::poll(WAIT, || async {
        let snapshot = context
            .trigger_value(
                "harness::delete-session-tree-status",
                json!({ "operation_id": expected_op }),
            )
            .await
            .ok()?;
        (snapshot["status"] != "deleting" && !snapshot.is_null()).then_some(snapshot)
    })
    .await;
    let mut deleted: Vec<String> = finished
        .as_ref()
        .and_then(|snapshot| serde_json::from_value(snapshot["deleted_session_ids"].clone()).ok())
        .unwrap_or_default();
    deleted.sort();
    let mut wanted = vec![c.clone(), g.clone()];
    wanted.sort();
    record(
        run_id,
        "deletion_completed",
        finished
            .as_ref()
            .is_some_and(|snapshot| snapshot["status"] == "completed")
            && deleted == wanted,
        format!("{finished:?}"),
    );
    let mut gone = Vec::new();
    for session in [&c, &g] {
        let meta = context
            .trigger_value("session::get", json!({ "session_id": session }))
            .await;
        let status = context
            .trigger_value("harness::status", json!({ "session_id": session }))
            .await;
        // `session::get` answers `null` for an unknown session.
        let absent = matches!(&meta, Ok(Value::Null));
        gone.push((
            session.clone(),
            absent,
            matches!(&status, Ok(Value::Null)),
            format!("{meta:?}"),
        ));
    }
    record(
        run_id,
        "subtree_gone",
        gone.iter()
            .all(|(_, absent, no_status, _)| *absent && *no_status),
        format!("{gone:?}"),
    );

    let send = context
        .trigger_value(
            "harness::send",
            json!({ "session_id": c, "message": "Are you there?", "model": route.0, "provider": route.1 }),
        )
        .await;
    record(
        run_id,
        "tombstone_refuses_send",
        matches!(&send, Err(error) if format!("{error:#}").contains("tombstoned")),
        format!("{send:?}"),
    );
    let respawn = spawn(context, &c, &format!("{c}_again"), route).await;
    record(
        run_id,
        "tombstone_refuses_spawn",
        matches!(&respawn, Err(error) if format!("{error:#}").contains("tombstoned")),
        format!("{respawn:?}"),
    );

    let notice = format!("e_{expected_op}_parent");
    let notices = mechanism::poll(WAIT, || async {
        let status = context
            .trigger_value("harness::status", json!({ "session_id": parent }))
            .await
            .ok()?;
        if !matches!(
            status["status"].as_str(),
            Some("completed" | "failed" | "cancelled")
        ) {
            return None;
        }
        let transcript = context.transcript(parent).await.ok()?;
        let rows: Vec<Value> = mechanism::entries(&transcript)
            .filter(|entry| entry["entry_id"] == notice.as_str())
            .cloned()
            .collect();
        (!rows.is_empty()).then_some(rows)
    })
    .await
    .unwrap_or_default();
    record(
        run_id,
        "parent_notice_once",
        notices.len() == 1
            && notices[0]["message"]["role"] == "user"
            && mechanism::text_of(&notices[0]["message"]).contains(&expected_op),
        format!("{} `{notice}` entr(y/ies)", notices.len()),
    );
    let tree = context
        .trigger_value(
            "harness::session-tree",
            json!({ "root_session_id": parent }),
        )
        .await
        .unwrap_or(Value::Null);
    let members = tree_members(&tree);
    record(
        run_id,
        "parent_tree",
        members.contains(&parent.to_string()) && !members.contains(&c) && !members.contains(&g),
        format!("tree={members:?}"),
    );

    let removed = call(
        context.client(),
        "session::delete",
        json!({ "session_id": d }),
    )
    .await;
    let purged = mechanism::poll(Duration::from_secs(30), || async {
        let status = context
            .trigger_value("harness::status", json!({ "session_id": d }))
            .await
            .ok()?;
        status.is_null().then_some(())
    })
    .await;
    record(
        run_id,
        "direct_delete_purges",
        removed.is_ok() && purged.is_some(),
        format!(
            "session::delete={removed:?} status_null={}",
            purged.is_some()
        ),
    );
}

#[derive(Debug, PartialEq, Eq)]
enum ChildOutcome {
    Answered,
    /// The provider dropped the child's turn: says nothing about the Harness.
    ModelSilent,
    /// A spawn error, a turn that failed for another reason or never ended.
    HarnessFault,
}

fn child_outcome(status: &Option<Value>) -> ChildOutcome {
    let Some(status) = status else {
        return ChildOutcome::HarnessFault;
    };
    let transport = status["result_error"]
        .as_str()
        .is_some_and(|reason| crate::suite::PROVIDER_TRANSPORT_FAILURES.contains(&reason));
    match status["status"].as_str() {
        Some("completed") => ChildOutcome::Answered,
        Some("failed") if transport => ChildOutcome::ModelSilent,
        _ => ChildOutcome::HarnessFault,
    }
}

fn tree_members(tree: &Value) -> Vec<String> {
    tree["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| node["session_id"].as_str().map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_operation_id_is_the_sha256_of_the_selected_session() {
        assert_eq!(
            operation_id("abc"),
            "delete_ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    async fn evaluate_with(run_id: &str, check: &str) -> ObjectiveEvaluation {
        let observation = ScenarioObservation::for_test(
            crate::scenarios::ScenarioId::SubtreeDeletion,
            "control",
            json!({ "messages": [] }),
            "ACK",
            Vec::new(),
        );
        record(run_id, check, false, "child failed");
        let context = E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let evaluation = SubtreeDeletion
            .evaluate(&context, &observation, run_id)
            .await
            .unwrap();
        mechanism::reset(run_id);
        evaluation
    }

    #[tokio::test]
    async fn a_child_whose_model_never_answered_leaves_the_run_undetermined() {
        let evaluation = evaluate_with("never-answered", "children_answered").await;
        assert_eq!(
            evaluation.completion,
            Some(crate::report::CompletionState::Undetermined)
        );
    }

    /// The runner builds the tree itself, so a spawn or session-tree fault is
    /// the Harness's and fails the deletion criterion.
    #[tokio::test]
    async fn a_tree_the_harness_could_not_build_fails_the_deletion() {
        let evaluation = evaluate_with("never-built", "tree_built").await;
        assert_ne!(
            evaluation.completion,
            Some(crate::report::CompletionState::Undetermined)
        );
        let deleted = evaluation
            .awards
            .iter()
            .find(|award| award.id == "subtree_deleted")
            .unwrap();
        assert_eq!(deleted.awarded, Some(0));
    }

    #[test]
    fn a_failed_child_turn_is_the_model_s_only_when_the_provider_dropped_it() {
        let failed = |error: &str| json!({ "status": "failed", "result_error": error });
        assert_eq!(
            child_outcome(&Some(json!({ "status": "completed" }))),
            ChildOutcome::Answered
        );
        assert_eq!(
            child_outcome(&Some(failed(crate::suite::PROVIDER_TRANSPORT_FAILURES[0]))),
            ChildOutcome::ModelSilent
        );
        assert_eq!(
            child_outcome(&Some(failed("harness.step_failed"))),
            ChildOutcome::HarnessFault
        );
        assert_eq!(child_outcome(&None), ChildOutcome::HarnessFault);
    }

    #[test]
    fn the_child_task_is_part_of_the_definition() {
        let planted = SubtreeDeletion.planted_state("run").unwrap();
        assert_eq!(planted["runner_messages"]["child_task"], CHILD_TASK);
    }

    #[test]
    fn criteria_weights_total_one_hundred() {
        SubtreeDeletion.spec("run").validate().unwrap();
    }
}
