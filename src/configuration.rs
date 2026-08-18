//! Operator-facing configuration for the Harness E2E worker.
//!
//! The YAML file written by `iii worker add` is a seed.  Once the worker is
//! running, the `configuration` worker is the source of truth and exposes the
//! value in Console → Workers → configure harness-e2e.  The storage directory is captured
//! while the worker boots because the control plane and its dashboard read
//! model are rooted there; changing it in Console therefore takes effect on
//! the next worker restart.

use std::time::Duration;

use iii_sdk::errors::Error;
use iii_sdk::protocol::{RegisterTriggerInput, TriggerRequest};
use iii_sdk::{IIIClient, RegisterFunction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::worker::WorkerConfig;

pub const CONFIG_ID: &str = "harness-e2e";
const CONFIG_FN_ID: &str = "harness-e2e::on-config-change";
const CONFIG_TIMEOUT_MS: u64 = 5_000;
const CONFIG_RETRIES: u32 = 3;

/// Register the E2E storage schema and seed it only when the configuration
/// worker has no value yet. Re-registering on every boot is safe and preserves
/// an operator's Console edit.
pub async fn register_config(iii: &IIIClient, seed: &WorkerConfig) -> Result<(), String> {
    let mut payload = json!({
        "id": CONFIG_ID,
        "name": "Harness E2E",
        "description": "Directory used to persist and reload Harness E2E executions, plans, reports and dashboard data. Changes are applied after restarting the harness-e2e worker.",
        "schema": WorkerConfig::json_schema(),
    });
    if should_seed(iii).await? {
        payload["initial_value"] = seed.to_json();
    }
    trigger_with_retry(iii, "configuration::register", payload).await?;
    Ok(())
}

/// Fetch the authoritative value. A missing entry is a normal first boot and
/// falls back to the built-in default; malformed values are rejected loudly.
pub async fn fetch_config(iii: &IIIClient) -> Result<WorkerConfig, String> {
    match try_get_value(iii).await? {
        Some(value) if !value.is_null() => WorkerConfig::from_json(&value),
        _ => Ok(WorkerConfig::default()),
    }
}

/// Bind a small internal handler to configuration updates. The handler does
/// not mutate the live control plane (its filesystem root is boot-captured),
/// but makes the restart requirement visible in the worker log and in traces.
pub fn register_config_trigger(iii: &IIIClient) -> Result<(), Error> {
    iii.register_function(
        CONFIG_FN_ID,
        RegisterFunction::new_async(|event: ConfigChangeEvent| async move {
            tracing::info!(
                config_id = CONFIG_ID,
                changed_id = ?event.id,
                "Harness E2E storage configuration changed; restart the worker to apply the new data directory"
            );
            Ok::<ConfigChangeResponse, Error>(ConfigChangeResponse { ok: true })
        })
        .description(
            "Internal: record that Harness E2E storage changed; the new directory applies on worker restart.",
        )
        .metadata(json!({ "internal": true })),
    );
    iii.register_trigger(RegisterTriggerInput {
        trigger_type: "configuration".into(),
        function_id: CONFIG_FN_ID.into(),
        config: json!({
            "configuration_id": CONFIG_ID,
            "event_types": ["configuration:updated"]
        }),
        metadata: None,
    })?;
    Ok(())
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
struct ConfigChangeEvent {
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct ConfigChangeResponse {
    ok: bool,
}

async fn should_seed(iii: &IIIClient) -> Result<bool, String> {
    match try_get_value(iii).await? {
        None => Ok(true),
        Some(value) if value.is_null() => Ok(true),
        Some(_) => Ok(false),
    }
}

async fn try_get_value(iii: &IIIClient) -> Result<Option<Value>, String> {
    match trigger_with_retry(iii, "configuration::get", json!({ "id": CONFIG_ID })).await {
        Ok(response) => Ok(response.get("value").cloned()),
        Err(error) if error.to_ascii_uppercase().contains("NOT_FOUND") => Ok(None),
        Err(error) => Err(error),
    }
}

async fn trigger_with_retry(
    iii: &IIIClient,
    function_id: &str,
    payload: Value,
) -> Result<Value, String> {
    let mut last_error = String::new();
    for attempt in 1..=CONFIG_RETRIES {
        match iii
            .trigger(TriggerRequest {
                function_id: function_id.into(),
                payload: payload.clone(),
                action: None,
                timeout_ms: Some(CONFIG_TIMEOUT_MS),
            })
            .await
        {
            Ok(value) => return Ok(value),
            Err(error) => {
                last_error = error.to_string();
                if is_not_found(&last_error) {
                    return Err(last_error);
                }
                if attempt < CONFIG_RETRIES {
                    tracing::warn!(
                        function_id,
                        attempt,
                        error = %last_error,
                        "configuration RPC failed; retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(250 * u64::from(attempt))).await;
                }
            }
        }
    }
    Err(format!(
        "{function_id} failed after {CONFIG_RETRIES} attempts: {last_error}"
    ))
}

fn is_not_found(error: &str) -> bool {
    error.to_ascii_uppercase().contains("NOT_FOUND")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_describes_the_default_storage_directory() {
        let schema = WorkerConfig::json_schema();
        assert_eq!(schema["example"]["data_dir"], "~/.iii/data/harness-e2e");
        assert_eq!(schema["properties"]["data_dir"]["type"], "string");
        assert_eq!(schema["properties"]["data_dir"]["minLength"], 1);
    }

    #[test]
    fn json_config_round_trips() {
        let config = WorkerConfig {
            data_dir: "/var/lib/harness-e2e".into(),
        };
        assert_eq!(WorkerConfig::from_json(&config.to_json()).unwrap(), config);
    }

    #[test]
    fn missing_configuration_is_not_retried() {
        assert!(is_not_found("remote error (NOT_FOUND): missing"));
        assert!(!is_not_found("connection reset by peer"));
    }
}
