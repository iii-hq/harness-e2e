use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use iii_sdk::errors::Error as SdkError;
use iii_sdk::protocol::{RegisterTriggerInput, TriggerRequest};
use iii_sdk::runtime::{IIIConnectionState, WorkerMetadata};
use iii_sdk::trigger::Trigger;
use iii_sdk::{register_worker, IIIClient, InitOptions, RegisterFunction};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::observe::{self, ObserveHub, ObserveSubscription, TreeObserver};
use crate::report::ObservedWorkerContract;
use crate::wire::{
    self, ControlPlaneEvidence, SessionMetricsResponse, SessionTreeResponse, StatusReport,
    StopResponse, TeardownResponse, TurnCompletedEvent,
};

pub const INVOCATION_TIMEOUT: Duration = Duration::from_secs(120);
const PROGRESS_SAMPLE_INTERVAL: Duration = Duration::from_secs(15);
const READ_TRANSPORT_RETRIES: u32 = 2;
const READ_TRANSPORT_BACKOFF: Duration = Duration::from_millis(100);

pub struct E2eContext {
    client: IIIClient,
    hub: ObserveHub,
    binding: Mutex<Option<Trigger>>,
}

pub struct RuntimeVersions {
    pub engine: String,
    pub harness: String,
}

impl E2eContext {
    /// The raw worker connection. Scenarios with a `setup` hook use it to
    /// register TEMPORARY functions (e.g. a custom post-turn validator) —
    /// they live exactly as long as this process's engine connection.
    pub fn client(&self) -> &IIIClient {
        &self.client
    }

    pub(crate) fn from_client(client: IIIClient) -> Self {
        Self {
            client,
            hub: ObserveHub::new(),
            binding: Mutex::new(None),
        }
    }
}

impl E2eContext {
    pub async fn connect(url: &str) -> Result<Self> {
        let client = register_worker(
            url,
            InitOptions {
                metadata: Some(WorkerMetadata {
                    runtime: "rust".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    name: ephemeral_worker_name(),
                    os: std::env::consts::OS.to_string(),
                    pid: Some(std::process::id()),
                    ..WorkerMetadata::default()
                }),
                ..InitOptions::default()
            },
        );
        let context = Self {
            client,
            hub: ObserveHub::new(),
            binding: Mutex::new(None),
        };
        context.wait_until_ready().await?;
        context.register_observation_sink();
        Ok(context)
    }

    pub async fn trigger<I, O>(&self, function_id: &str, payload: I) -> Result<O>
    where
        I: Serialize + Send,
        O: DeserializeOwned,
    {
        let payload = serde_json::to_value(payload)
            .with_context(|| format!("serialize request for {function_id}"))?;
        let value = self
            .trigger_value_with_timeout(function_id, payload, INVOCATION_TIMEOUT)
            .await?;
        serde_json::from_value(value).with_context(|| format!("decode response from {function_id}"))
    }

    pub async fn trigger_value(&self, function_id: &str, payload: Value) -> Result<Value> {
        self.trigger_value_with_timeout(function_id, payload, INVOCATION_TIMEOUT)
            .await
    }

    pub async fn function_exists(&self, function_id: &str) -> Result<bool> {
        let listed = self
            .trigger_value(
                "engine::functions::list",
                json!({ "include_internal": true }),
            )
            .await?;
        let exists = function_ids(&listed).any(|id| id == function_id);
        Ok(exists)
    }

    pub async fn preflight_control_plane(&self) -> Result<ControlPlaneEvidence> {
        let function_ids = wire::control_plane_function_ids().collect::<Vec<_>>();
        let info = self
            .trigger_value(
                "engine::functions::info",
                json!({ "function_ids": function_ids }),
            )
            .await
            .context("discover Harness control-plane contracts")?;
        wire::validate_control_plane(&info)
    }

    pub async fn observe_function_contracts(
        &self,
        function_ids: &[String],
    ) -> Result<Vec<ObservedWorkerContract>> {
        if function_ids.is_empty() {
            return Ok(Vec::new());
        }
        let response = self
            .trigger_value(
                "engine::functions::info",
                json!({ "function_ids": function_ids }),
            )
            .await
            .context("observe scenario function contracts")?;
        let functions = response
            .get("functions")
            .and_then(Value::as_array)
            .context("engine::functions::info response is missing functions[]")?;
        function_ids
            .iter()
            .map(|function_id| {
                let function = functions
                    .iter()
                    .find(|function| {
                        function.get("function_id").and_then(Value::as_str)
                            == Some(function_id.as_str())
                    })
                    .with_context(|| {
                        format!("engine::functions::info omitted required function {function_id}")
                    })?;
                if let Some(error) = function.get("error").and_then(Value::as_str) {
                    bail!("required function {function_id} is unavailable: {error}");
                }
                let request_schema = function
                    .get("request_schema")
                    .filter(|schema| schema.is_object())
                    .with_context(|| format!("{function_id} has no JSON request schema"))?;
                let response_schema = function
                    .get("response_schema")
                    .filter(|schema| schema.is_object())
                    .with_context(|| format!("{function_id} has no JSON response schema"))?;
                Ok(ObservedWorkerContract {
                    function_id: function_id.clone(),
                    request_schema_sha256: crate::artifact::sha256_value(request_schema)?,
                    response_schema_sha256: crate::artifact::sha256_value(response_schema)?,
                })
            })
            .collect()
    }

    pub(crate) fn drain_turn_completed_events(&self) {
        self.hub.drain();
    }

    pub async fn runtime_versions(&self) -> Result<RuntimeVersions> {
        let health = self
            .trigger_value("engine::health::check", json!({}))
            .await
            .context("query iii engine version")?;
        let engine = health
            .get("version")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .context("engine::health::check response is missing version")?
            .to_string();
        let workers = self
            .trigger_value("engine::workers::list", json!({}))
            .await
            .context("query Harness worker version")?;
        let harness = workers
            .get("workers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|worker| worker.get("name").and_then(Value::as_str) == Some("harness"))
            .and_then(|worker| worker.get("version"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .context("engine::workers::list has no versioned Harness worker")?
            .to_string();
        Ok(RuntimeVersions { engine, harness })
    }

    pub async fn bind_turn_completed(&self) -> Result<()> {
        self.unbind_turn_completed().await.ok();
        self.hub.drain();
        let listed = self
            .trigger_value(
                "engine::triggers::list",
                json!({ "include_internal": true }),
            )
            .await
            .context("discover harness::turn-completed trigger type")?;
        if !observe::turn_completed_available(&listed) {
            return Err(observe::missing_turn_completed_trigger());
        }
        // Observation is a worker-owned subscription in the stack namespace.
        // engine::register_trigger creates a durable binding in default instead.
        let binding = self.client.register_trigger(RegisterTriggerInput::new(
            observe::TURN_COMPLETED_TRIGGER,
            observe::SINK_FUNCTION_ID,
            json!({}),
        ))?;
        *self.lock_binding() = Some(binding);
        Ok(())
    }

    pub async fn unbind_turn_completed(&self) -> Result<()> {
        if let Some(binding) = self.lock_binding().take() {
            binding.unregister();
        }
        self.hub.drain();
        Ok(())
    }

    pub async fn wait_for_tree(
        &self,
        scenario_id: &str,
        session_id: &str,
        stuck_timeout: Duration,
        log_heartbeat: bool,
        cancellation: Option<&watch::Receiver<bool>>,
    ) -> Result<SessionMetricsResponse> {
        let observer = ContextTreeObserver {
            context: self,
            subscription: self.hub.subscribe(),
        };
        observe::wait_until_complete(
            &observer,
            scenario_id,
            session_id,
            observe::WaitOptions {
                stuck_timeout,
                sample_interval: PROGRESS_SAMPLE_INTERVAL,
                log_heartbeat,
                cancellation,
                expected_turn_id: None,
            },
        )
        .await
    }

    pub async fn wait_for_turn(
        &self,
        scenario_id: &str,
        session_id: &str,
        turn_id: &str,
        stuck_timeout: Duration,
        log_heartbeat: bool,
        cancellation: Option<&watch::Receiver<bool>>,
    ) -> Result<SessionMetricsResponse> {
        let observer = ContextTreeObserver {
            context: self,
            subscription: self.hub.subscribe(),
        };
        observe::wait_until_complete(
            &observer,
            scenario_id,
            session_id,
            observe::WaitOptions {
                stuck_timeout,
                sample_interval: PROGRESS_SAMPLE_INTERVAL,
                log_heartbeat,
                cancellation,
                expected_turn_id: Some(turn_id),
            },
        )
        .await
    }

    pub async fn metrics(&self, session_id: &str) -> Result<SessionMetricsResponse> {
        self.trigger("harness::metrics", json!({ "root_session_id": session_id }))
            .await
    }

    pub async fn transcript(&self, session_id: &str) -> Result<Value> {
        let mut cursor: Option<String> = None;
        let mut messages = Vec::new();
        loop {
            let response: Value = self
                .trigger(
                    "session::messages",
                    json!({
                        "session_id": session_id,
                        "limit": 500,
                        "cursor": cursor,
                        "include_custom": true,
                    }),
                )
                .await?;
            let page = response
                .get("messages")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("session::messages returned a malformed page"))?;
            messages.extend(page.iter().cloned());
            let next = response
                .get("next_cursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next.is_none() {
                break;
            }
            if next == cursor {
                bail!("session::messages repeated transcript cursor for {session_id}");
            }
            cursor = next;
        }
        Ok(json!({ "messages": messages }))
    }

    pub async fn stop_session(&self, session_id: &str, turn_id: Option<&str>) -> Result<()> {
        let _: StopResponse = self
            .trigger(
                "harness::stop",
                json!({ "session_id": session_id, "turn_id": turn_id }),
            )
            .await?;
        Ok(())
    }

    async fn stop_session_tree(&self, root_session_id: &str) {
        let tree = tokio::time::timeout(
            Duration::from_secs(5),
            self.trigger::<_, SessionTreeResponse>(
                "harness::session-tree",
                json!({ "root_session_id": root_session_id }),
            ),
        )
        .await;
        if let Ok(Ok(tree)) = tree {
            for session in tree.sessions.iter().rev() {
                let _ = self.stop_session(&session.session_id, None).await;
            }
        } else {
            let _ = self.stop_session(root_session_id, None).await;
        }
    }

    pub async fn teardown(&self, root_session_id: &str) -> Result<u64> {
        let response: TeardownResponse = self
            .trigger(
                "harness::teardown",
                json!({ "root_session_id": root_session_id }),
            )
            .await?;
        Ok(response.removed)
    }

    pub async fn shutdown(&self) {
        self.client.shutdown_async().await;
    }

    async fn wait_until_ready(&self) -> Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            if self
                .trigger_value_with_timeout(
                    "engine::functions::list",
                    json!({ "include_internal": true }),
                    Duration::from_secs(1),
                )
                .await
                .is_ok()
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("timed out connecting the E2E runner to the iii engine");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn trigger_value_with_timeout(
        &self,
        function_id: &str,
        payload: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let timeout_ms = timeout.as_millis().min(u64::MAX as u128) as u64;
        let outer = timeout + Duration::from_secs(5);
        invoke_with_transport_retries(function_id, outer, || async {
            ensure_connected_before_dispatch(self.client.get_connection_state())?;
            self.client
                .trigger(TriggerRequest {
                    function_id: function_id.to_string(),
                    payload: payload.clone(),
                    action: None,
                    timeout_ms: Some(timeout_ms),
                })
                .await
                .with_context(|| format!("invoke {function_id}"))
        })
        .await
    }

    fn register_observation_sink(&self) {
        let hub = self.hub.clone();
        self.client.register_function(
            observe::SINK_FUNCTION_ID,
            RegisterFunction::new_async(move |payload: Value| {
                let hub = hub.clone();
                async move {
                    match serde_json::from_value::<TurnCompletedEvent>(payload) {
                        Ok(event) => hub.push(event),
                        Err(error) => tracing::warn!(
                            %error,
                            "ignored malformed harness::turn-completed payload"
                        ),
                    }
                    Ok::<observe::SinkAck, iii_sdk::errors::Error>(observe::SinkAck {
                        accepted: true,
                    })
                }
            })
            .description("Internal harness::turn-completed sink for E2E session-tree observation.")
            .metadata(json!({ "internal": true })),
        );
    }

    fn lock_binding(&self) -> std::sync::MutexGuard<'_, Option<Trigger>> {
        self.binding
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Classify SDK transport failures without interpreting remote/provider text.
pub(crate) fn transport_failure_code(error: &anyhow::Error) -> Option<&'static str> {
    match error.downcast_ref::<SdkError>()? {
        SdkError::NotConnected => Some("transport_not_connected"),
        SdkError::Timeout => Some("transport_timeout"),
        SdkError::WebSocket(_) => Some("transport_websocket"),
        _ => None,
    }
}

#[derive(Debug)]
struct InvocationNotDispatched;

impl std::fmt::Display for InvocationNotDispatched {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invocation was not dispatched because the connection was not ready")
    }
}

#[derive(Debug)]
struct EarlierInvocationAttempts(u32);

impl std::fmt::Display for EarlierInvocationAttempts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invocation failed after {} attempts", self.0)
    }
}

/// Raw SDK NotConnected is ambiguous: it can also mean a response channel
/// closed after dispatch. Only our own pre-dispatch check proves no invocation.
pub(crate) fn request_not_dispatched(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<SdkError>(),
        Some(SdkError::NotConnected)
    ) && error.downcast_ref::<InvocationNotDispatched>().is_some()
        && error.downcast_ref::<EarlierInvocationAttempts>().is_none()
}

fn ensure_connected_before_dispatch(state: IIIConnectionState) -> Result<()> {
    if state != IIIConnectionState::Connected {
        return Err(anyhow::Error::new(SdkError::NotConnected).context(InvocationNotDispatched));
    }
    Ok(())
}

fn is_retryable_read(function_id: &str) -> bool {
    matches!(
        function_id,
        "harness::status"
            | "harness::metrics"
            | "harness::session-tree"
            | "session::messages"
            | "engine::functions::list"
            | "engine::functions::info"
            | "router::models::get"
            | "router::models::list"
    )
}

async fn invoke_with_transport_retries<F, Fut>(
    function_id: &str,
    budget: Duration,
    mut invoke: F,
) -> Result<Value>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    let retry_limit = if is_retryable_read(function_id) {
        READ_TRANSPORT_RETRIES
    } else {
        0
    };
    // The budget covers all attempts and backoff, so transient errors cannot
    // multiply an invocation's deadline. Dropping this future cancels both.
    let result = tokio::time::timeout(budget, async {
        for attempt in 0..=retry_limit {
            match invoke().await {
                Ok(value) => return Ok(value),
                Err(error) => {
                    let code = transport_failure_code(&error);
                    if attempt == retry_limit || code.is_none() {
                        return Err(if attempt == 0 {
                            error
                        } else {
                            error.context(EarlierInvocationAttempts(attempt + 1))
                        });
                    }
                    tracing::warn!(
                        function_id,
                        code,
                        retry = attempt + 1,
                        "retrying read-only invocation after a typed transport failure"
                    );
                    tokio::time::sleep(READ_TRANSPORT_BACKOFF * (attempt + 1)).await;
                }
            }
        }
        unreachable!("bounded invocation loop always returns on its last attempt")
    })
    .await;
    match result {
        Ok(result) => result.with_context(|| format!("invoke {function_id}")),
        Err(_) => Err(anyhow::Error::new(SdkError::Timeout)).with_context(|| {
            format!(
                "{function_id}: no response within {}ms including read retries",
                budget.as_millis()
            )
        }),
    }
}

fn ephemeral_worker_name() -> String {
    format!(
        "harness-e2e-{}-{}",
        std::process::id(),
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

struct ContextTreeObserver<'a> {
    context: &'a E2eContext,
    subscription: ObserveSubscription,
}

#[async_trait]
impl TreeObserver for ContextTreeObserver<'_> {
    async fn next_turn_completed(&self, timeout: Duration) -> Option<TurnCompletedEvent> {
        self.subscription.wait_event(timeout).await
    }

    async fn pull_metrics(&self, root_session_id: &str) -> Result<SessionMetricsResponse> {
        self.context.metrics(root_session_id).await
    }

    async fn pull_root_status(&self, root_session_id: &str) -> Result<Option<StatusReport>> {
        self.context
            .trigger(
                "harness::status",
                json!({ "session_id": root_session_id, "verbose": true }),
            )
            .await
    }

    async fn stop_tree(&self, root_session_id: &str) {
        self.context.stop_session_tree(root_session_id).await;
    }
}

fn function_ids(listed: &Value) -> impl Iterator<Item = &str> {
    listed
        .as_array()
        .or_else(|| listed.as_object()?.values().find_map(Value::as_array))
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.as_str()
                .or_else(|| item.get("function_id").and_then(Value::as_str))
                .or_else(|| item.get("id").and_then(Value::as_str))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_transport_classification_preserves_sources_and_rejects_provider_text() {
        for (source, expected) in [
            (SdkError::NotConnected, Some("transport_not_connected")),
            (SdkError::Timeout, Some("transport_timeout")),
            (
                SdkError::WebSocket("connection closed".into()),
                Some("transport_websocket"),
            ),
            (
                SdkError::Handler("provider returned 503: websocket timeout".into()),
                None,
            ),
            (
                SdkError::Remote {
                    code: "503".into(),
                    message: "temporarily unavailable".into(),
                    stacktrace: None,
                },
                None,
            ),
            (SdkError::Serde("timed out".into()), None),
        ] {
            let error = anyhow::Error::new(source).context("capture session evidence");
            assert_eq!(transport_failure_code(&error), expected);
            assert!(!request_not_dispatched(&error));
        }
        assert_eq!(
            transport_failure_code(&anyhow!("iii is not connected: timeout 503")),
            None
        );
    }

    #[test]
    fn no_dispatch_requires_the_local_pre_dispatch_check_and_no_prior_attempts() {
        ensure_connected_before_dispatch(IIIConnectionState::Connected).unwrap();
        for state in [
            IIIConnectionState::Disconnected,
            IIIConnectionState::Connecting,
            IIIConnectionState::Reconnecting,
            IIIConnectionState::Failed,
        ] {
            let error = ensure_connected_before_dispatch(state)
                .unwrap_err()
                .context("invoke harness::send");
            assert_eq!(
                transport_failure_code(&error),
                Some("transport_not_connected")
            );
            assert!(request_not_dispatched(&error));
            assert!(!request_not_dispatched(
                &error.context(EarlierInvocationAttempts(2))
            ));
        }
    }

    #[test]
    fn transport_retries_are_allowlisted_reads_only() {
        for function_id in [
            "harness::status",
            "harness::metrics",
            "harness::session-tree",
            "session::messages",
            "engine::functions::list",
            "engine::functions::info",
            "router::models::get",
            "router::models::list",
        ] {
            assert!(is_retryable_read(function_id), "{function_id}");
        }
        for function_id in [
            "harness::send",
            "harness::spawn",
            "harness::stop",
            "harness::teardown",
            "session::append",
            "router::models::reconcile",
            "router::chat",
            "judge::evaluate",
            "engine::register_trigger",
            "unknown::read",
        ] {
            assert!(!is_retryable_read(function_id), "{function_id}");
        }
    }

    #[tokio::test]
    async fn read_transport_retries_recover_without_repeating_the_subject() {
        let expected = json!({"status": "completed"});
        let mut outcomes = std::collections::VecDeque::from([
            Err(anyhow::Error::new(SdkError::Timeout).context("read status")),
            Err(anyhow::Error::new(SdkError::WebSocket("closed".into()))),
            Ok(expected.clone()),
        ]);
        let mut calls = 0;
        let observed =
            invoke_with_transport_retries("harness::status", Duration::from_secs(2), || {
                calls += 1;
                std::future::ready(outcomes.pop_front().unwrap())
            })
            .await
            .unwrap();
        assert_eq!(observed, expected);
        assert_eq!(calls, 3);
        assert!(outcomes.is_empty());
    }

    #[tokio::test]
    async fn read_transport_retries_stop_after_two_additional_attempts() {
        let mut calls = 0;
        let error =
            invoke_with_transport_retries("session::messages", Duration::from_secs(2), || {
                calls += 1;
                std::future::ready(Err(ensure_connected_before_dispatch(
                    IIIConnectionState::Disconnected,
                )
                .unwrap_err()))
            })
            .await
            .unwrap_err();
        assert_eq!(calls, 3);
        assert_eq!(
            transport_failure_code(&error),
            Some("transport_not_connected")
        );
        assert!(!request_not_dispatched(&error));
    }

    #[tokio::test]
    async fn mutating_calls_never_retry_even_on_typed_transport_failures() {
        for function_id in ["harness::send", "harness::spawn", "judge::evaluate"] {
            for source in [
                SdkError::NotConnected,
                SdkError::Timeout,
                SdkError::WebSocket("closed".into()),
            ] {
                let mut calls = 0;
                let error =
                    invoke_with_transport_retries(function_id, Duration::from_secs(1), || {
                        calls += 1;
                        std::future::ready(Err(anyhow::Error::new(source.clone())))
                    })
                    .await
                    .unwrap_err();
                assert_eq!(calls, 1, "{function_id}");
                assert!(transport_failure_code(&error).is_some());
                assert!(!request_not_dispatched(&error));
            }
        }
    }

    #[tokio::test]
    async fn provider_failures_are_not_retried_or_reclassified_as_transport() {
        let mut calls = 0;
        let error =
            invoke_with_transport_retries("harness::status", Duration::from_secs(1), || {
                calls += 1;
                std::future::ready(Err(anyhow::Error::new(SdkError::Remote {
                    code: "provider_error".into(),
                    message: "503 temporarily unavailable, timeout, NotConnected".into(),
                    stacktrace: None,
                })))
            })
            .await
            .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(transport_failure_code(&error), None);

        let failed_status = json!({"result_error": "503 provider timeout"});
        let result =
            invoke_with_transport_retries("harness::status", Duration::from_secs(1), || {
                calls += 1;
                std::future::ready(Ok(failed_status.clone()))
            })
            .await
            .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(result, failed_status);
    }

    #[tokio::test]
    async fn the_original_invocation_budget_includes_retry_backoff() {
        let mut calls = 0;
        let error =
            invoke_with_transport_retries("harness::metrics", Duration::from_millis(10), || {
                calls += 1;
                std::future::ready(Err(anyhow::Error::new(SdkError::NotConnected)))
            })
            .await
            .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(transport_failure_code(&error), Some("transport_timeout"));
        assert!(!request_not_dispatched(&error));
    }

    #[tokio::test]
    async fn cancelling_an_invocation_drops_the_pending_transport_future() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountDrop<'a>(&'a AtomicUsize);
        impl Drop for CountDrop<'_> {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let dropped = AtomicUsize::new(0);
        let calls = AtomicUsize::new(0);
        let cancelled = tokio::time::timeout(
            Duration::from_millis(10),
            invoke_with_transport_retries("harness::metrics", Duration::from_secs(1), || async {
                calls.fetch_add(1, Ordering::SeqCst);
                let _guard = CountDrop(&dropped);
                std::future::pending::<Result<Value>>().await
            }),
        )
        .await;
        assert!(cancelled.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn function_discovery_accepts_engine_response_shapes() {
        let listed = json!({
            "functions": [
                { "function_id": "provider::zai::stream" },
                { "id": "database::query" },
                "state::get"
            ]
        });
        assert_eq!(
            function_ids(&listed).collect::<Vec<_>>(),
            ["provider::zai::stream", "database::query", "state::get"]
        );
    }

    #[test]
    fn each_connection_uses_a_distinct_ephemeral_worker_name() {
        let first = ephemeral_worker_name();
        let second = ephemeral_worker_name();
        let prefix = format!("harness-e2e-{}-", std::process::id());

        assert!(first.starts_with(&prefix));
        assert!(second.starts_with(&prefix));
        assert_ne!(first, second);
    }
}
