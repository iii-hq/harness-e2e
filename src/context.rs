use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::runtime::WorkerMetadata;
use iii_sdk::{register_worker, IIIClient, InitOptions, RegisterFunction};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::observe::{self, ObserveHub, ObserveSubscription, TreeObserver};
use crate::wire::{
    self, ControlPlaneEvidence, SessionMetricsResponse, SessionTreeResponse, StatusReport,
    StopResponse, TeardownResponse, TurnCompletedEvent, TurnStatus,
};

pub const INVOCATION_TIMEOUT: Duration = Duration::from_secs(120);
const PROGRESS_SAMPLE_INTERVAL: Duration = Duration::from_secs(15);

pub struct E2eContext {
    client: IIIClient,
    hub: ObserveHub,
    binding_id: Mutex<Option<String>>,
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
            binding_id: Mutex::new(None),
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
                    name: "harness-e2e".to_string(),
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
            binding_id: Mutex::new(None),
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
        let response = self
            .trigger_value(
                "engine::register_trigger",
                json!({
                    "trigger_type": observe::TURN_COMPLETED_TRIGGER,
                    "function_id": observe::SINK_FUNCTION_ID,
                    "config": {},
                }),
            )
            .await
            .context("bind harness::turn-completed")?;
        let id = observe::binding_id(&response)?;
        *self.lock_binding() = Some(id);
        Ok(())
    }

    pub async fn unbind_turn_completed(&self) -> Result<()> {
        let id = self.lock_binding().clone();
        let Some(id) = id else {
            return Ok(());
        };
        self.trigger_value(
            "engine::unregister_trigger",
            json!({
                "id": id,
                "trigger_type": observe::TURN_COMPLETED_TRIGGER,
            }),
        )
        .await
        .context("unbind harness::turn-completed")?;
        self.lock_binding().take();
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
            stuck_timeout,
            PROGRESS_SAMPLE_INTERVAL,
            log_heartbeat,
            cancellation,
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
        match tokio::time::timeout(
            outer,
            self.client.trigger(TriggerRequest {
                function_id: function_id.to_string(),
                payload,
                action: None,
                timeout_ms: Some(timeout_ms),
            }),
        )
        .await
        {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(anyhow!("{function_id}: {error}")),
            Err(_) => bail!("{function_id}: no response within {}ms", outer.as_millis()),
        }
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

    fn lock_binding(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.binding_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
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
            .trigger("harness::status", json!({ "session_id": root_session_id }))
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

#[allow(dead_code)]
fn session_is_terminal(status: &StatusReport) -> Result<bool> {
    match status.status {
        TurnStatus::Completed => Ok(!status.expects_wake),
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
        TurnStatus::Running | TurnStatus::AwaitingFunctions => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(
        turn_id: &str,
        status: TurnStatus,
        expects_wake: bool,
        result_error: Option<&str>,
    ) -> StatusReport {
        StatusReport::from_normalized(crate::wire::StatusReportPayload {
            session_id: "session".to_string(),
            turn_id: Some(turn_id.to_string()),
            status,
            step: 0,
            turn_count: 1,
            max_turns: 100,
            pending_function_calls: Vec::new(),
            children: Vec::new(),
            expects_wake,
            queued: Vec::new(),
            result_error: result_error.map(str::to_string),
            validation_retries: 0,
            transient_resumes: 0,
        })
    }

    #[test]
    fn a_wake_can_advance_the_session_to_a_new_turn() {
        let parked = status("turn-initial", TurnStatus::Completed, true, None);
        let resumed = status("turn-after-wake", TurnStatus::Running, false, None);
        let completed = status("turn-after-wake", TurnStatus::Completed, false, None);

        assert!(!session_is_terminal(&parked).unwrap());
        assert!(!session_is_terminal(&resumed).unwrap());
        assert!(session_is_terminal(&completed).unwrap());
    }

    #[test]
    fn failed_and_cancelled_turns_are_errors() {
        for turn_status in [TurnStatus::Failed, TurnStatus::Cancelled] {
            let report = status("turn", turn_status, false, Some("provider stopped"));

            let error = session_is_terminal(&report).unwrap_err();
            assert!(error.to_string().contains("provider stopped"));
        }
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
}
