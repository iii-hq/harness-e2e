//! The turn lifecycle a correct stack shows on every native run: the turn
//! events the runner's bindings received and the sessions' own state once the
//! tree completed. `execute` captures it before teardown removes the sessions.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::harness_invariants::{HarnessInvariant, HarnessViolation};
use crate::report::E2eRunReport;

pub const TURN_STARTED_TRIGGER: &str = "harness::turn-started";
pub const STARTED_SINK_FUNCTION_ID: &str = "e2e::on-turn-started";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnEventKind {
    Started,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnEvent {
    pub kind: TurnEventKind,
    pub session_id: String,
    pub turn_id: String,
    /// `turn-completed` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<bool>,
}

impl TurnEvent {
    /// A `harness::turn-started` payload, or `None` when it names no turn.
    pub fn started(payload: &Value) -> Option<Self> {
        Some(Self {
            kind: TurnEventKind::Started,
            session_id: text(payload, "/session_id")?.into(),
            turn_id: text(payload, "/turn_id")?.into(),
            terminal: None,
        })
    }
}

/// Every turn event the bindings delivered, in arrival order and before any
/// dedupe: the waits drain their own queue between exchanges, this one only
/// when a run binds.
#[derive(Clone, Default)]
pub struct TurnLog(Arc<Mutex<Vec<TurnEvent>>>);

impl TurnLog {
    pub fn push(&self, event: TurnEvent) {
        self.lock().push(event);
    }

    pub fn clear(&self) {
        self.lock().clear();
    }

    pub fn snapshot(&self) -> Vec<TurnEvent> {
        self.lock().clone()
    }

    pub fn completed(&self, session_id: &str, turn_id: &str) -> bool {
        self.lock().iter().any(|event| {
            event.kind == TurnEventKind::Completed
                && event.session_id == session_id
                && event.turn_id == turn_id
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<TurnEvent>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A turn the runner sent, and whether its `turn-completed` had arrived once
/// the tree settled: without it only the status poll saw the turn end. Read
/// after the settle window, so an event the poll merely beat still counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SentTurn {
    pub turn_id: String,
    pub completed_by_event: bool,
}

/// One session of the tree: where the metrics place it, its turns in transcript
/// order (`origin.turn_id` of its assistant entries) and its `session::get`
/// meta at the end of the polling window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionLifecycle {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    pub depth: u32,
    pub turn_ids: Vec<String>,
    pub meta: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TurnLifecycleEvidence {
    pub scenario: String,
    /// The tree's events; delivery is at-least-once, so repeats are counted
    /// in `duplicate_completions` and never fail the run.
    pub events: Vec<TurnEvent>,
    pub duplicate_completions: u32,
    pub sent_turns: Vec<SentTurn>,
    /// The root first.
    pub sessions: Vec<SessionLifecycle>,
}

impl TurnLifecycleEvidence {
    /// Keeps the events of this tree only: the binding sees the whole namespace.
    pub fn new(
        scenario: &str,
        events: Vec<TurnEvent>,
        sent_turns: Vec<SentTurn>,
        sessions: Vec<SessionLifecycle>,
    ) -> Self {
        let events = events
            .into_iter()
            .filter(|event| sessions.iter().any(|s| s.session_id == event.session_id))
            .collect::<Vec<_>>();
        let duplicate_completions = completions(&events)
            .values()
            .map(|terminal| terminal.len().saturating_sub(1) as u32)
            .sum();
        Self {
            scenario: scenario.into(),
            events,
            duplicate_completions,
            sent_turns,
            sessions,
        }
    }
}

/// The session title the runner sends and reads back.
pub fn session_title(scenario: &str) -> String {
    format!("Harness E2E: {scenario}")
}

/// The window `session::get` gets to project a terminal turn onto the session.
pub const SESSION_STATUS_WINDOW: std::time::Duration = std::time::Duration::from_secs(5);

/// Whether the session status is one a finished turn projects to.
pub fn settled(meta: Option<&Value>) -> bool {
    matches!(
        meta.and_then(|meta| meta.get("status"))
            .and_then(Value::as_str),
        Some("done" | "error")
    )
}

/// Reads the completed tree's sessions before the terminal `harness::status`,
/// which re-projects the session status and would hide a projection the
/// finalizer lost.
pub async fn capture(
    context: &crate::context::E2eContext,
    scenario: &str,
    root_session_id: &str,
    sent_turn_ids: &[String],
    metrics: &crate::wire::SessionMetricsResponse,
) -> anyhow::Result<TurnLifecycleEvidence> {
    let mut sessions = vec![SessionLifecycle {
        session_id: root_session_id.into(),
        parent_session_id: None,
        depth: 0,
        turn_ids: Vec::new(),
        meta: None,
    }];
    sessions.extend(
        metrics
            .by_session
            .iter()
            .filter(|usage| usage.session_id != root_session_id)
            .map(|usage| SessionLifecycle {
                session_id: usage.session_id.clone(),
                parent_session_id: usage.parent_session_id.clone(),
                depth: usage.depth,
                turn_ids: Vec::new(),
                meta: None,
            }),
    );
    read_sessions(&mut sessions, SESSION_STATUS_WINDOW, |session_id| {
        context.trigger(
            "session::get",
            serde_json::json!({ "session_id": session_id }),
        )
    })
    .await?;
    for session in &mut sessions {
        session.turn_ids = transcript_turns(&context.transcript(&session.session_id).await?);
    }
    let log = context.turn_log();
    let sent_turns = sent_turn_ids
        .iter()
        .map(|turn_id| SentTurn {
            turn_id: turn_id.clone(),
            completed_by_event: log.completed(root_session_id, turn_id),
        })
        .collect();
    Ok(TurnLifecycleEvidence::new(
        scenario,
        log.snapshot(),
        sent_turns,
        sessions,
    ))
}

/// Reads each session's `session::get` meta until all settled or the window
/// closed, rereading only the sessions still working.
async fn read_sessions<F, Fut>(
    sessions: &mut [SessionLifecycle],
    window: std::time::Duration,
    mut get: F,
) -> anyhow::Result<()>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Option<Value>>>,
{
    let deadline = tokio::time::Instant::now() + window;
    loop {
        for session in sessions.iter_mut().filter(|s| !settled(s.meta.as_ref())) {
            let response = get(session.session_id.clone()).await?;
            session.meta = response.and_then(|response| response.get("meta").cloned());
        }
        if sessions.iter().all(|s| settled(s.meta.as_ref()))
            || tokio::time::Instant::now() >= deadline
        {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

pub(crate) fn violations(report: &E2eRunReport) -> Vec<HarnessViolation> {
    let Some(evidence) = report.turn_lifecycle.as_ref() else {
        return Vec::new();
    };
    let mut violations = turn_events(evidence);
    violations.extend(session_state(evidence, &report.attempt_id));
    violations
}

/// The terminal flags of each `(session, turn)`'s completions, one per event.
fn completions(events: &[TurnEvent]) -> BTreeMap<(&str, &str), Vec<Option<bool>>> {
    let mut completions = BTreeMap::<_, Vec<_>>::new();
    for event in events.iter().filter(|e| e.kind == TurnEventKind::Completed) {
        completions
            .entry((event.session_id.as_str(), event.turn_id.as_str()))
            .or_default()
            .push(event.terminal);
    }
    completions
}

fn turn_events(evidence: &TurnLifecycleEvidence) -> Vec<HarnessViolation> {
    let violation = |detail: String| HarnessViolation {
        invariant: HarnessInvariant::TurnEvents,
        detail,
    };
    let completions = completions(&evidence.events);
    let mut violations = Vec::new();
    let Some(root) = evidence.sessions.first() else {
        return violations;
    };
    for turn_id in &root.turn_ids {
        let started = evidence
            .events
            .iter()
            .filter(|e| e.kind == TurnEventKind::Started)
            .filter(|e| e.session_id == root.session_id && &e.turn_id == turn_id)
            .count();
        if started != 1 {
            violations.push(violation(format!(
                "root turn {turn_id} got {started} turn-started events, not one"
            )));
        }
    }
    for session in &evidence.sessions {
        for turn_id in &session.turn_ids {
            if !completions.contains_key(&(session.session_id.as_str(), turn_id.as_str())) {
                violations.push(violation(format!(
                    "turn {turn_id} of session {} got no turn-completed",
                    session.session_id
                )));
            }
        }
    }
    for sent in evidence.sent_turns.iter().filter(|t| !t.completed_by_event) {
        violations.push(violation(format!(
            "turn {} completed through the status poll, not its turn-completed",
            sent.turn_id
        )));
    }
    // A completion is non-terminal only while the session owns an armed wake,
    // so a later root turn the runner did not send must follow it; the last
    // turn carries the run's outcome.
    let sent = |turn_id: &str| evidence.sent_turns.iter().any(|t| t.turn_id == turn_id);
    let mut turns = root.turn_ids.iter().peekable();
    while let Some(turn_id) = turns.next() {
        let flags = completions
            .get(&(root.session_id.as_str(), turn_id.as_str()))
            .map_or(&[][..], Vec::as_slice);
        let woken = turns.peek().is_some_and(|next| !sent(next));
        let last = turns.peek().is_none();
        if last && flags.iter().any(|flag| *flag != Some(true)) {
            violations.push(violation(format!(
                "the last root turn {turn_id} completed with terminal false"
            )));
        } else if !last && !woken && flags.contains(&Some(false)) {
            violations.push(violation(format!(
                "root turn {turn_id} completed with terminal false but no wake followed"
            )));
        }
    }
    violations
}

fn session_state(evidence: &TurnLifecycleEvidence, attempt_id: &str) -> Vec<HarnessViolation> {
    let mut violations = Vec::new();
    let Some(root) = evidence.sessions.first() else {
        return violations;
    };
    let expected_outcome = evidence
        .scenario
        .parse::<crate::scenarios::ScenarioId>()
        .map(|id| id.module().expected_root_outcome());
    let root_status = match expected_outcome {
        Ok(crate::wire::TurnStatus::Failed) => "error",
        _ => "done",
    };
    for (index, session) in evidence.sessions.iter().enumerate() {
        let id = &session.session_id;
        let Some(meta) = session.meta.as_ref() else {
            violations.push(HarnessViolation {
                invariant: HarnessInvariant::SessionReadBack,
                detail: format!("session::get returned no session {id}"),
            });
            continue;
        };
        let status = text(meta, "/status");
        // The root's turns all ended as the scenario expects, or the run would
        // not have passed; a child that failed on its own projects to `error`.
        let expected: &[&str] = if index == 0 {
            &[root_status]
        } else {
            &["done", "error"]
        };
        if !status.is_some_and(|status| expected.contains(&status)) {
            violations.push(HarnessViolation {
                invariant: HarnessInvariant::SessionStatus,
                detail: format!(
                    "session {id} is {} {}s after its tree completed, not {}",
                    status.unwrap_or("without a status"),
                    SESSION_STATUS_WINDOW.as_secs(),
                    expected.join(" or ")
                ),
            });
        }
        let mut expected = vec![("/kind", Value::from(crate::wire::E2E_SESSION_KIND))];
        if index == 0 {
            expected.extend([
                ("/title", Value::from(session_title(&evidence.scenario))),
                ("/metadata/e2e_run_id", Value::from(attempt_id)),
                (
                    "/metadata/e2e_scenario",
                    Value::from(evidence.scenario.as_str()),
                ),
            ]);
        } else {
            // shortcut: the metrics walk follows these same parent links, so a
            // child stored under the wrong parent is never listed; catching it
            // needs the spawned ids from the root transcript.
            let parent = session
                .parent_session_id
                .as_deref()
                .unwrap_or(&root.session_id);
            expected.extend([
                ("/metadata/parent_session_id", Value::from(parent)),
                ("/metadata/depth", Value::from(session.depth)),
            ]);
        }
        for (pointer, value) in expected {
            let found = meta.pointer(pointer);
            if found != Some(&value) {
                violations.push(HarnessViolation {
                    invariant: HarnessInvariant::SessionReadBack,
                    detail: format!(
                        "session {id} reads back {pointer} = {}, not {value}",
                        found.map_or_else(|| "nothing".into(), Value::to_string)
                    ),
                });
            }
        }
    }
    violations
}

/// The distinct `origin.turn_id`s of a transcript's assistant entries, in order.
pub fn transcript_turns(transcript: &Value) -> Vec<String> {
    let mut turns = Vec::<String>::new();
    let entries = transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| text(entry, "/message/role") == Some("assistant"));
    for turn_id in entries.filter_map(|entry| text(entry, "/origin/turn_id")) {
        if !turns.iter().any(|seen| seen == turn_id) {
            turns.push(turn_id.into());
        }
    }
    turns
}

fn text<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::report::RunStatus;

    fn started(session_id: &str, turn_id: &str) -> TurnEvent {
        TurnEvent {
            kind: TurnEventKind::Started,
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            terminal: None,
        }
    }

    fn completed(session_id: &str, turn_id: &str, terminal: bool) -> TurnEvent {
        TurnEvent {
            kind: TurnEventKind::Completed,
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            terminal: Some(terminal),
        }
    }

    fn sent(turn_id: &str) -> SentTurn {
        SentTurn {
            turn_id: turn_id.into(),
            completed_by_event: true,
        }
    }

    fn root_meta() -> Value {
        json!({
            "session_id": "root", "status": "done", "kind": "e2e",
            "title": "Harness E2E: timer_wake",
            "metadata": { "e2e_run_id": "attempt", "e2e_scenario": "timer_wake" },
        })
    }

    fn child_meta(status: &str) -> Value {
        json!({
            "session_id": "child", "status": status, "kind": "e2e",
            "metadata": { "parent_session_id": "root", "depth": 1, "spawned_by": "agent" },
        })
    }

    fn session(id: &str, turn_ids: &[&str], meta: Value) -> SessionLifecycle {
        SessionLifecycle {
            session_id: id.into(),
            parent_session_id: (id != "root").then(|| "root".into()),
            depth: u32::from(id != "root"),
            turn_ids: turn_ids.iter().map(|t| t.to_string()).collect(),
            meta: Some(meta),
        }
    }

    /// A timer wake: t1 arms a one-shot notify, the wake runs t2, and t1 spawned
    /// a child.
    fn woken() -> (Vec<TurnEvent>, Vec<SentTurn>, Vec<SessionLifecycle>) {
        (
            vec![
                started("root", "t1"),
                started("child", "c1"),
                completed("child", "c1", true),
                completed("root", "t1", false),
                started("root", "t2"),
                completed("root", "t2", true),
            ],
            vec![sent("t1")],
            vec![
                session("root", &["t1", "t2"], root_meta()),
                session("child", &["c1"], child_meta("done")),
            ],
        )
    }

    fn report(
        (events, sent_turns, sessions): (Vec<TurnEvent>, Vec<SentTurn>, Vec<SessionLifecycle>),
    ) -> E2eRunReport {
        let mut report =
            E2eRunReport::new("run".into(), "attempt".into(), 1, "root".into(), "p".into());
        report.status = RunStatus::Passed;
        report.turn_lifecycle = Some(TurnLifecycleEvidence::new(
            "timer_wake",
            events,
            sent_turns,
            sessions,
        ));
        report
    }

    fn details(report: &E2eRunReport) -> Vec<(HarnessInvariant, String)> {
        violations(report)
            .into_iter()
            .map(|v| (v.invariant, v.detail))
            .collect()
    }

    #[test]
    fn a_woken_tree_that_settled_keeps_every_invariant() {
        assert_eq!(details(&report(woken())), []);
    }

    #[test]
    fn duplicate_completions_are_counted_not_failed() {
        let (mut events, sent_turns, sessions) = woken();
        events.push(completed("root", "t2", true));
        events.push(completed("other", "x", true));

        let report = report((events, sent_turns, sessions));

        assert_eq!(details(&report), []);
        let evidence = report.turn_lifecycle.as_ref().unwrap();
        assert_eq!(evidence.duplicate_completions, 1);
        assert!(evidence.events.iter().all(|e| e.session_id != "other"));
    }

    #[test]
    fn each_root_turn_starts_exactly_once_and_every_turn_completes() {
        let (mut events, sent_turns, sessions) = woken();
        events.retain(|e| *e != started("root", "t2") && *e != completed("child", "c1", true));
        events.push(started("root", "t1"));

        let found = details(&report((events, sent_turns, sessions)));

        assert_eq!(
            found,
            [
                (
                    HarnessInvariant::TurnEvents,
                    "root turn t1 got 2 turn-started events, not one".into()
                ),
                (
                    HarnessInvariant::TurnEvents,
                    "root turn t2 got 0 turn-started events, not one".into()
                ),
                (
                    HarnessInvariant::TurnEvents,
                    "turn c1 of session child got no turn-completed".into()
                ),
            ]
        );
    }

    #[test]
    fn a_sent_turn_the_poll_completed_is_flagged() {
        let (events, mut sent_turns, sessions) = woken();
        sent_turns[0].completed_by_event = false;

        assert_eq!(
            details(&report((events, sent_turns, sessions))),
            [(
                HarnessInvariant::TurnEvents,
                "turn t1 completed through the status poll, not its turn-completed".into()
            )]
        );
    }

    #[test]
    fn terminal_is_false_only_before_a_wake_and_true_on_the_last_turn() {
        let flip = |events: &mut Vec<TurnEvent>, turn: &str, terminal: bool| {
            for event in events.iter_mut().filter(|e| e.session_id == "root") {
                if event.turn_id == turn && event.kind == TurnEventKind::Completed {
                    event.terminal = Some(terminal);
                }
            }
        };
        let (mut last_open, sent_turns, sessions) = woken();
        flip(&mut last_open, "t2", false);
        assert_eq!(
            details(&report((last_open, sent_turns.clone(), sessions.clone()))),
            [(
                HarnessInvariant::TurnEvents,
                "the last root turn t2 completed with terminal false".into()
            )]
        );

        // A dialogue: the runner sent t2, so nothing woke the session after t1.
        let (events, _, sessions) = woken();
        assert_eq!(
            details(&report((events, vec![sent("t1"), sent("t2")], sessions))),
            [(
                HarnessInvariant::TurnEvents,
                "root turn t1 completed with terminal false but no wake followed".into()
            )]
        );

        // Terminal before a wake is not this gate's to judge.
        let (mut early, sent_turns, sessions) = woken();
        flip(&mut early, "t1", true);
        assert_eq!(details(&report((early, sent_turns, sessions))), []);
    }

    #[test]
    fn sessions_settle_done_or_a_child_error_and_read_back_what_was_sent() {
        let (events, sent_turns, mut sessions) = woken();
        sessions[1].meta = Some(child_meta("error"));
        assert_eq!(
            details(&report((
                events.clone(),
                sent_turns.clone(),
                sessions.clone()
            ))),
            []
        );

        sessions[0].meta.as_mut().unwrap()["status"] = json!("idle");
        sessions[0].meta.as_mut().unwrap()["metadata"]["e2e_scenario"] = json!("other");
        sessions[1].meta = Some(json!({
            "session_id": "child", "status": "working", "kind": "user",
            "metadata": { "parent_session_id": "root", "depth": 2 },
        }));
        let found = details(&report((events, sent_turns, sessions)));

        assert_eq!(
            found,
            [
                (
                    HarnessInvariant::SessionStatus,
                    "session root is idle 5s after its tree completed, not done".into()
                ),
                (
                    HarnessInvariant::SessionReadBack,
                    "session root reads back /metadata/e2e_scenario = \"other\", not \
                     \"timer_wake\""
                        .into()
                ),
                (
                    HarnessInvariant::SessionStatus,
                    "session child is working 5s after its tree completed, not done or error"
                        .into()
                ),
                (
                    HarnessInvariant::SessionReadBack,
                    "session child reads back /kind = \"user\", not \"e2e\"".into()
                ),
                (
                    HarnessInvariant::SessionReadBack,
                    "session child reads back /metadata/depth = 2, not 1".into()
                ),
            ]
        );
    }

    #[test]
    fn a_root_expected_to_fail_settles_error() {
        let evidence = |status: &str| {
            let mut meta = root_meta();
            meta["status"] = json!(status);
            meta["title"] = json!(session_title("turn_failure_surface"));
            meta["metadata"]["e2e_scenario"] = json!("turn_failure_surface");
            TurnLifecycleEvidence::new(
                "turn_failure_surface",
                Vec::new(),
                Vec::new(),
                vec![session("root", &[], meta)],
            )
        };
        assert!(session_state(&evidence("error"), "attempt").is_empty());
        assert_eq!(
            session_state(&evidence("done"), "attempt")
                .into_iter()
                .map(|v| (v.invariant, v.detail))
                .collect::<Vec<_>>(),
            [(
                HarnessInvariant::SessionStatus,
                "session root is done 5s after its tree completed, not error".into()
            )]
        );
    }

    #[test]
    fn a_session_the_store_lost_is_flagged() {
        let (events, sent_turns, mut sessions) = woken();
        sessions[1].meta = None;

        assert_eq!(
            details(&report((events, sent_turns, sessions))),
            [(
                HarnessInvariant::SessionReadBack,
                "session::get returned no session child".into()
            )]
        );
    }

    #[test]
    fn a_run_without_lifecycle_evidence_is_not_judged() {
        let mut report = report(woken());
        report.turn_lifecycle = None;
        assert!(violations(&report).is_empty());
    }

    #[test]
    fn the_started_sink_keeps_the_session_and_turn_of_a_payload() {
        let payload = json!({ "session_id": "root", "turn_id": "t1", "depth": 0 });
        assert_eq!(TurnEvent::started(&payload), Some(started("root", "t1")));
        assert_eq!(TurnEvent::started(&json!({ "session_id": "root" })), None);
    }

    #[tokio::test]
    async fn session_reads_repeat_until_every_session_settles() {
        let (_, _, mut sessions) = woken();
        sessions.iter_mut().for_each(|s| s.meta = None);
        let mut answers = std::collections::VecDeque::from([
            ("root", "working"),
            ("child", "done"),
            ("root", "done"),
        ]);
        let mut asked = Vec::new();

        read_sessions(&mut sessions, SESSION_STATUS_WINDOW, |id| {
            asked.push(id.clone());
            let (expected, status) = answers.pop_front().unwrap();
            assert_eq!(id, expected);
            std::future::ready(Ok(Some(json!({ "meta": { "status": status } }))))
        })
        .await
        .unwrap();

        assert_eq!(asked, ["root", "child", "root"]);
        assert!(sessions.iter().all(|s| settled(s.meta.as_ref())));
    }

    #[tokio::test]
    async fn session_reads_give_up_at_the_window_and_surface_a_failed_read() {
        let (_, _, mut sessions) = woken();
        sessions[0].meta = None;
        let mut asked = 0;
        read_sessions(&mut sessions[..1], std::time::Duration::ZERO, |_| {
            asked += 1;
            std::future::ready(Ok(Some(json!({ "meta": { "status": "working" } }))))
        })
        .await
        .unwrap();
        assert_eq!(asked, 1);
        assert_eq!(
            text(sessions[0].meta.as_ref().unwrap(), "/status"),
            Some("working")
        );

        let error = read_sessions(&mut sessions, std::time::Duration::ZERO, |_| {
            std::future::ready(Err(anyhow::anyhow!("store down")))
        })
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("store down"), "{error:#}");
    }

    #[test]
    fn transcript_turns_are_the_assistant_origins_in_order() {
        let transcript = json!({ "messages": [
            { "message": { "role": "user" }, "origin": { "turn_id": "t0" } },
            { "message": { "role": "assistant" }, "origin": { "turn_id": "t1" } },
            { "message": { "role": "function_result" }, "origin": { "turn_id": "t1" } },
            { "message": { "role": "assistant" }, "origin": { "turn_id": "t1" } },
            { "custom": { "custom_type": "trigger_fired" } },
            { "message": { "role": "assistant" }, "origin": { "turn_id": "t2" } },
        ] });

        assert_eq!(transcript_turns(&transcript), ["t1", "t2"]);
    }
}
