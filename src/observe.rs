use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{watch, Notify};

use crate::wire::{
    SessionMetricsResponse, StatusReport, TurnCompletedEvent, TurnCompletedPayload, TurnStatus,
};

pub const SINK_FUNCTION_ID: &str = "e2e::on-turn-completed";
pub const TURN_COMPLETED_TRIGGER: &str = "harness::turn-completed";

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SinkAck {
    pub accepted: bool,
}

#[derive(Clone)]
pub struct ObserveHub {
    inner: Arc<ObserveHubInner>,
}

struct ObserveHubInner {
    events: Mutex<VecDeque<(u64, TurnCompletedEvent)>>,
    next_sequence: Mutex<u64>,
    notify: Notify,
}

const MAX_RETAINED_EVENTS: usize = 4_096;

pub struct ObserveSubscription {
    inner: Arc<ObserveHubInner>,
    cursor: Mutex<u64>,
}

impl ObserveHub {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ObserveHubInner {
                events: Mutex::new(VecDeque::new()),
                next_sequence: Mutex::new(0),
                notify: Notify::new(),
            }),
        }
    }

    pub fn push(&self, event: TurnCompletedEvent) {
        let sequence = {
            let mut next = self
                .inner
                .next_sequence
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let sequence = *next;
            *next = next.saturating_add(1);
            sequence
        };
        let mut events = self.lock_events();
        events.push_back((sequence, event));
        while events.len() > MAX_RETAINED_EVENTS {
            events.pop_front();
        }
        drop(events);
        self.inner.notify.notify_waiters();
    }

    pub fn drain(&self) {
        self.lock_events().clear();
    }

    pub fn subscribe(&self) -> ObserveSubscription {
        let cursor = self
            .lock_events()
            .front()
            .map(|(sequence, _)| *sequence)
            .unwrap_or_else(|| {
                *self
                    .inner
                    .next_sequence
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
            });
        ObserveSubscription {
            inner: self.inner.clone(),
            cursor: Mutex::new(cursor),
        }
    }

    fn lock_events(&self) -> std::sync::MutexGuard<'_, VecDeque<(u64, TurnCompletedEvent)>> {
        self.inner
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl ObserveSubscription {
    pub async fn wait_event(&self, timeout: Duration) -> Option<TurnCompletedEvent> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.inner.notify.notified();
            if let Some(event) = self.next_retained() {
                return Some(event);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() || tokio::time::timeout(remaining, notified).await.is_err() {
                return None;
            }
        }
    }

    fn next_retained(&self) -> Option<TurnCompletedEvent> {
        let events = self
            .inner
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut cursor = self
            .cursor
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((first, _)) = events.front() {
            *cursor = (*cursor).max(*first);
        }
        let (sequence, event) = events.iter().find(|(sequence, _)| *sequence >= *cursor)?;
        *cursor = sequence.saturating_add(1);
        Some(event.clone())
    }
}

pub fn trigger_type_ids(listed: &Value) -> impl Iterator<Item = &str> {
    listed
        .as_array()
        .or_else(|| listed.as_object()?.values().find_map(Value::as_array))
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.as_str()
                .or_else(|| item.get("id").and_then(Value::as_str))
                .or_else(|| item.get("trigger_type").and_then(Value::as_str))
        })
}

pub fn turn_completed_available(listed: &Value) -> bool {
    trigger_type_ids(listed).any(|id| id == TURN_COMPLETED_TRIGGER)
}

pub fn missing_turn_completed_trigger() -> anyhow::Error {
    anyhow!(
        "unsupported: required trigger type {TURN_COMPLETED_TRIGGER} is not registered; \
         event-driven observation cannot proceed"
    )
}

pub fn binding_id(response: &Value) -> Result<String> {
    response
        .get("id")
        .or_else(|| response.get("subscription_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("engine::register_trigger response is missing id"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MetricsProgress {
    sessions: u64,
    function_calls: u64,
    function_call_errors: u64,
}

impl From<&SessionMetricsResponse> for MetricsProgress {
    fn from(metrics: &SessionMetricsResponse) -> Self {
        Self {
            sessions: metrics.totals.sessions,
            function_calls: metrics.totals.function_calls,
            function_call_errors: metrics.totals.function_call_errors,
        }
    }
}

struct ObserveMachine {
    root_session_id: String,
    seen: HashSet<(String, String)>,
}

enum EventKind {
    Duplicate,
    Unrelated,
    Progress,
    RootFailed { message: String },
}

impl ObserveMachine {
    fn new(root_session_id: impl Into<String>) -> Self {
        Self {
            root_session_id: root_session_id.into(),
            seen: HashSet::new(),
        }
    }

    fn classify(&mut self, event: &TurnCompletedPayload) -> EventKind {
        let key = (event.session_id.clone(), event.turn_id.clone());
        if !self.seen.insert(key) {
            return EventKind::Duplicate;
        }
        if event.session_id != self.root_session_id {
            return EventKind::Unrelated;
        }
        if event.session_id == self.root_session_id
            && matches!(event.status, TurnStatus::Failed | TurnStatus::Cancelled)
        {
            return EventKind::RootFailed {
                message: format!(
                    "turn ended as {:?}: {}",
                    event.status,
                    event
                        .result_error
                        .as_deref()
                        .unwrap_or("no error was reported")
                ),
            };
        }
        EventKind::Progress
    }
}

fn status_is_fatal(status: &StatusReport) -> Result<()> {
    match status.status {
        TurnStatus::Failed | TurnStatus::Cancelled => {
            bail!(
                "turn ended as {:?}: {}",
                status.status,
                status
                    .result_error
                    .as_deref()
                    .unwrap_or("no error was reported")
            );
        }
        TurnStatus::Completed | TurnStatus::Running | TurnStatus::AwaitingFunctions => Ok(()),
    }
}

fn ensure_not_cancelled(cancellation: Option<&watch::Receiver<bool>>) -> Result<()> {
    if cancellation.is_some_and(|receiver| *receiver.borrow()) {
        bail!("E2E execution was cancelled");
    }
    Ok(())
}

fn totals_changed(
    previous: &mut Option<MetricsProgress>,
    metrics: &SessionMetricsResponse,
) -> bool {
    let observed = MetricsProgress::from(metrics);
    if previous.as_ref() == Some(&observed) {
        return false;
    }
    let changed = previous.is_some();
    *previous = Some(observed);
    changed
}

#[async_trait]
pub(crate) trait TreeObserver: Send + Sync {
    async fn next_turn_completed(&self, timeout: Duration) -> Option<TurnCompletedEvent>;
    async fn pull_metrics(&self, root_session_id: &str) -> Result<SessionMetricsResponse>;
    async fn pull_root_status(&self, root_session_id: &str) -> Result<Option<StatusReport>>;
    async fn stop_tree(&self, root_session_id: &str);
}

pub(crate) async fn wait_until_complete<O: TreeObserver>(
    observer: &O,
    scenario_id: &str,
    root_session_id: &str,
    stuck_timeout: Duration,
    sample_interval: Duration,
    log_heartbeat: bool,
    cancellation: Option<&watch::Receiver<bool>>,
) -> Result<SessionMetricsResponse> {
    let started = tokio::time::Instant::now();
    let mut last_progress = started;
    let mut previous_metrics = None;
    let mut machine = ObserveMachine::new(root_session_id);
    let mut cancellation = cancellation.cloned();

    loop {
        ensure_not_cancelled(cancellation.as_ref())?;
        let remaining = stuck_timeout.saturating_sub(last_progress.elapsed());
        if remaining.is_zero() {
            observer.stop_tree(root_session_id).await;
            bail!(
                "scenario {scenario_id} made no observable progress for {}s while waiting for \
                 the complete session tree {root_session_id}",
                stuck_timeout.as_secs()
            );
        }
        let wait = remaining.min(sample_interval);
        let event = match cancellation.as_mut() {
            Some(receiver) => {
                tokio::select! {
                    _ = receiver.changed() => {
                        ensure_not_cancelled(Some(receiver))?;
                        continue;
                    }
                    event = observer.next_turn_completed(wait) => event,
                }
            }
            None => observer.next_turn_completed(wait).await,
        };

        if let Some(event) = event {
            match machine.classify(&event) {
                EventKind::Duplicate | EventKind::Unrelated => continue,
                EventKind::RootFailed { message } => bail!("{message}"),
                EventKind::Progress => {
                    last_progress = tokio::time::Instant::now();
                    match observer.pull_metrics(root_session_id).await {
                        Ok(metrics) => {
                            let _ = totals_changed(&mut previous_metrics, &metrics);
                            if metrics.complete {
                                return Ok(metrics);
                            }
                        }
                        Err(error) => tracing::debug!(
                            scenario = scenario_id,
                            session_id = root_session_id,
                            %error,
                            "could not sample E2E metrics after turn-completed"
                        ),
                    }
                }
            }
            continue;
        }

        let metrics = match observer.pull_metrics(root_session_id).await {
            Ok(metrics) => metrics,
            Err(error) => {
                tracing::debug!(
                    scenario = scenario_id,
                    session_id = root_session_id,
                    %error,
                    "could not sample E2E progress metrics"
                );
                continue;
            }
        };
        match observer.pull_root_status(root_session_id).await {
            Ok(Some(status)) => {
                status_is_fatal(&status)?;
                if log_heartbeat {
                    tracing::info!(
                        scenario = scenario_id,
                        session_id = root_session_id,
                        elapsed_seconds = started.elapsed().as_secs(),
                        inactive_seconds = last_progress.elapsed().as_secs(),
                        status = ?status.status,
                        step = status.step,
                        turns = status.turn_count,
                        max_turns = status.max_turns,
                        pending_functions = status.pending_function_calls.len(),
                        children = status.children.len(),
                        queued_messages = status.queued.len(),
                        expects_wake = status.expects_wake,
                        sessions = metrics.totals.sessions,
                        tree_complete = metrics.complete,
                        "E2E scenario progress"
                    );
                }
            }
            Ok(None) => {}
            Err(error) => tracing::debug!(
                scenario = scenario_id,
                session_id = root_session_id,
                %error,
                "could not sample E2E root status"
            ),
        }
        if metrics.complete {
            return Ok(metrics);
        }
        if totals_changed(&mut previous_metrics, &metrics) {
            last_progress = tokio::time::Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    use serde_json::json;
    use tokio::sync::watch;

    use super::*;
    use crate::wire::{SessionMetricsPayload, SessionUsageTotals, StatusReportPayload};

    struct Injected {
        hub: ObserveHub,
        subscription: ObserveSubscription,
        metrics: Mutex<SessionMetricsResponse>,
        status: Mutex<StatusReport>,
        stopped: AtomicBool,
        metrics_error: Mutex<Option<String>>,
    }

    impl Injected {
        fn new(metrics: SessionMetricsResponse, status: StatusReport) -> Self {
            let hub = ObserveHub::new();
            Self {
                subscription: hub.subscribe(),
                hub,
                metrics: Mutex::new(metrics),
                status: Mutex::new(status),
                stopped: AtomicBool::new(false),
                metrics_error: Mutex::new(None),
            }
        }

        fn push(&self, event: TurnCompletedEvent) {
            self.hub.push(event);
        }

        fn set_metrics(&self, metrics: SessionMetricsResponse) {
            *self.metrics.lock().unwrap() = metrics;
        }
    }

    #[async_trait]
    impl TreeObserver for Injected {
        async fn next_turn_completed(&self, timeout: Duration) -> Option<TurnCompletedEvent> {
            self.subscription.wait_event(timeout).await
        }

        async fn pull_metrics(&self, _root_session_id: &str) -> Result<SessionMetricsResponse> {
            if let Some(error) = self.metrics_error.lock().unwrap().clone() {
                bail!("{error}");
            }
            Ok(self.metrics.lock().unwrap().clone())
        }

        async fn pull_root_status(&self, _root_session_id: &str) -> Result<Option<StatusReport>> {
            Ok(Some(self.status.lock().unwrap().clone()))
        }

        async fn stop_tree(&self, _root_session_id: &str) {
            self.stopped.store(true, Ordering::SeqCst);
        }
    }

    fn event(
        session_id: &str,
        turn_id: &str,
        status: TurnStatus,
        terminal: bool,
        result_error: Option<&str>,
    ) -> TurnCompletedEvent {
        TurnCompletedEvent::from_normalized(TurnCompletedPayload {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            status,
            terminal,
            result_error: result_error.map(str::to_string),
        })
    }

    fn metrics(complete: bool, sessions: u64, function_calls: u64) -> SessionMetricsResponse {
        SessionMetricsResponse::from_normalized(SessionMetricsPayload {
            root_session_id: "root".to_string(),
            complete,
            totals: SessionUsageTotals {
                sessions,
                turns: 1,
                function_calls,
                function_call_errors: 0,
                ..Default::default()
            },
            by_session: Vec::new(),
            traces: None,
        })
    }

    fn status(turn_status: TurnStatus, expects_wake: bool) -> StatusReport {
        StatusReport::from_normalized(StatusReportPayload {
            session_id: "root".to_string(),
            turn_id: Some("turn-1".to_string()),
            status: turn_status,
            step: 1,
            turn_count: 1,
            max_turns: 100,
            pending_function_calls: Vec::new(),
            children: Vec::new(),
            queued: Vec::new(),
            expects_wake,
            result_error: None,
            validation_retries: 0,
            transient_resumes: 0,
        })
    }

    async fn wait(
        observer: &Injected,
        stuck: Duration,
        sample: Duration,
        cancellation: Option<&watch::Receiver<bool>>,
    ) -> Result<SessionMetricsResponse> {
        wait_until_complete(
            observer,
            "direct_answer",
            "root",
            stuck,
            sample,
            false,
            cancellation,
        )
        .await
    }

    #[test]
    fn extras_on_the_event_payload_do_not_break_parse() {
        let raw = json!({
            "session_id": "root",
            "turn_id": "t1",
            "status": "completed",
            "terminal": true,
            "timestamp": 99,
            "reason": "ok",
            "context": { "budget": 1 },
            "unexpected": [1, 2, 3]
        });
        let event: TurnCompletedEvent = serde_json::from_value(raw).unwrap();
        assert_eq!(event.session_id, "root");
        assert_eq!(event.turn_id, "t1");
        assert!(event.terminal);
    }

    #[test]
    fn preflight_without_the_trigger_type_is_unsupported() {
        assert!(!turn_completed_available(&json!({ "triggers": [] })));
        assert!(!turn_completed_available(&json!({})));
        let listed = json!({
            "triggers": [
                { "id": "harness::turn-started" },
                { "id": "harness::turn-completed", "description": "done" }
            ]
        });
        assert!(turn_completed_available(&listed));
        let error = missing_turn_completed_trigger().to_string();
        assert!(error.contains("unsupported"), "{error}");
        assert!(error.contains(TURN_COMPLETED_TRIGGER), "{error}");
    }

    #[test]
    fn binding_id_accepts_id_or_subscription_id() {
        assert_eq!(binding_id(&json!({ "id": "sub-1" })).unwrap(), "sub-1");
        assert_eq!(
            binding_id(&json!({ "subscription_id": "sub-2" })).unwrap(),
            "sub-2"
        );
        assert!(binding_id(&json!({})).is_err());
    }

    #[tokio::test]
    async fn concurrent_session_subscribers_receive_the_same_broadcast_without_cross_attribution() {
        let hub = ObserveHub::new();
        let first = hub.subscribe();
        let second = hub.subscribe();
        hub.push(event(
            "session-a",
            "turn-a",
            TurnStatus::Completed,
            true,
            None,
        ));
        hub.push(event(
            "session-b",
            "turn-b",
            TurnStatus::Completed,
            true,
            None,
        ));

        let first_events = [
            first.wait_event(Duration::from_millis(20)).await.unwrap(),
            first.wait_event(Duration::from_millis(20)).await.unwrap(),
        ];
        let second_events = [
            second.wait_event(Duration::from_millis(20)).await.unwrap(),
            second.wait_event(Duration::from_millis(20)).await.unwrap(),
        ];
        assert_eq!(
            first_events
                .iter()
                .map(|event| (event.session_id.as_str(), event.turn_id.as_str()))
                .collect::<Vec<_>>(),
            second_events
                .iter()
                .map(|event| (event.session_id.as_str(), event.turn_id.as_str()))
                .collect::<Vec<_>>()
        );

        let mut session_a = ObserveMachine::new("session-a");
        let mut session_b = ObserveMachine::new("session-b");
        assert!(matches!(
            session_a.classify(&first_events[0]),
            EventKind::Progress
        ));
        assert!(matches!(
            session_a.classify(&first_events[1]),
            EventKind::Unrelated
        ));
        assert!(matches!(
            session_b.classify(&second_events[0]),
            EventKind::Unrelated
        ));
        assert!(matches!(
            session_b.classify(&second_events[1]),
            EventKind::Progress
        ));
    }

    #[tokio::test]
    async fn completed_terminal_root_with_complete_metrics_returns() {
        let observer = Injected::new(metrics(true, 1, 0), status(TurnStatus::Completed, false));
        observer.push(event("root", "turn-1", TurnStatus::Completed, true, None));

        let observed = wait(
            &observer,
            Duration::from_secs(2),
            Duration::from_millis(200),
            None,
        )
        .await
        .unwrap();
        assert!(observed.complete);
        assert!(!observer.stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn parked_root_completion_keeps_waiting() {
        let observer = Injected::new(metrics(false, 1, 0), status(TurnStatus::Completed, true));
        observer.push(event("root", "turn-1", TurnStatus::Completed, false, None));
        let wait_fut = wait(
            &observer,
            Duration::from_secs(2),
            Duration::from_millis(20),
            None,
        );
        let drive = async {
            tokio::time::sleep(Duration::from_millis(40)).await;
            observer.set_metrics(metrics(true, 1, 1));
            observer.push(event("root", "turn-2", TurnStatus::Completed, true, None));
        };
        let (result, _) = tokio::join!(wait_fut, drive);
        assert!(result.unwrap().complete);
    }

    #[tokio::test]
    async fn failed_root_event_errors_with_result_error() {
        let observer = Injected::new(metrics(false, 1, 0), status(TurnStatus::Failed, false));
        observer.push(event(
            "root",
            "turn-1",
            TurnStatus::Failed,
            true,
            Some("provider stopped"),
        ));

        let error = wait(
            &observer,
            Duration::from_secs(2),
            Duration::from_millis(200),
            None,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("provider stopped"), "{error}");
        assert!(error.contains("Failed"), "{error}");
    }

    #[tokio::test]
    async fn child_failure_does_not_abort_the_wait() {
        let observer = Injected::new(metrics(false, 2, 1), status(TurnStatus::Running, false));
        observer.push(event(
            "child",
            "turn-c",
            TurnStatus::Failed,
            true,
            Some("child crashed"),
        ));
        observer.set_metrics(metrics(true, 2, 1));
        observer.push(event("root", "turn-1", TurnStatus::Completed, true, None));

        let observed = wait(
            &observer,
            Duration::from_secs(2),
            Duration::from_millis(50),
            None,
        )
        .await
        .unwrap();
        assert!(observed.complete);
    }

    #[tokio::test]
    async fn duplicate_event_does_not_reset_stuck() {
        let observer = Injected::new(metrics(false, 1, 0), status(TurnStatus::Running, false));
        let duplicate = event("root", "turn-1", TurnStatus::Completed, false, None);
        observer.push(duplicate.clone());
        observer.push(duplicate);

        let error = wait(
            &observer,
            Duration::from_millis(90),
            Duration::from_millis(20),
            None,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("no observable progress"), "{error}");
        assert!(observer.stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn unchanged_totals_on_tick_do_not_reset_stuck() {
        let observer = Injected::new(metrics(false, 1, 0), status(TurnStatus::Running, false));
        let error = wait(
            &observer,
            Duration::from_millis(90),
            Duration::from_millis(20),
            None,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("no observable progress"), "{error}");
        assert!(observer.stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn changing_totals_on_tick_reset_stuck() {
        let observer = Injected::new(metrics(false, 1, 0), status(TurnStatus::Running, false));
        let wait_fut = wait(
            &observer,
            Duration::from_millis(120),
            Duration::from_millis(25),
            None,
        );
        let drive = async {
            tokio::time::sleep(Duration::from_millis(40)).await;
            observer.set_metrics(metrics(false, 2, 1));
            tokio::time::sleep(Duration::from_millis(50)).await;
            observer.set_metrics(metrics(true, 2, 1));
        };
        let (result, _) = tokio::join!(wait_fut, drive);
        assert!(result.unwrap().complete);
        assert!(!observer.stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn complete_metrics_on_tick_without_an_event_still_finishes() {
        let observer = Injected::new(metrics(true, 1, 0), status(TurnStatus::Completed, false));
        let observed = wait(
            &observer,
            Duration::from_secs(2),
            Duration::from_millis(20),
            None,
        )
        .await
        .unwrap();
        assert!(observed.complete);
    }

    #[tokio::test]
    async fn cancellation_aborts_the_wait() {
        let observer = Injected::new(metrics(false, 1, 0), status(TurnStatus::Running, false));
        let (tx, rx) = watch::channel(false);
        let wait_fut = wait(
            &observer,
            Duration::from_secs(5),
            Duration::from_secs(5),
            Some(&rx),
        );
        let cancel = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            tx.send(true).unwrap();
        };
        let (result, _) = tokio::join!(wait_fut, cancel);
        let error = result.unwrap_err().to_string();
        assert!(error.contains("cancelled"), "{error}");
    }
}
