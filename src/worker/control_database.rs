//! Add the built-in E2E pool to the database worker's runtime configuration.
//! Compose's Registry graph carries dependency versions, not per-consumer
//! configuration. Discover the actual database instance/configuration id;
//! never assume its container is called `database` or save over its settings.

use std::future::Future;
use std::path::Path;

use anyhow::{bail, Context, Result};
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::IIIClient;
use serde_json::{json, Value};

pub(super) async fn provision(
    iii: &IIIClient,
    namespace: &str,
    database: &str,
    data_dir: &Path,
    deadline: tokio::time::Instant,
) -> Result<()> {
    let function = call(
        iii,
        "engine::functions::info",
        json!({"function_id": "database::query", "namespace": namespace}),
    )
    .await
    .context("find the control database worker")?;
    let worker_name = function["worker_name"]
        .as_str()
        .context("database::query has no worker owner")?;
    let worker = call(
        iii,
        "engine::workers::info",
        json!({"name": worker_name, "namespace": namespace}),
    )
    .await
    .context("inspect the control database worker")?;
    let id = configuration_id(&worker, namespace)?;
    let current = call(iii, "configuration::get", json!({"id": id, "raw": true}))
        .await
        .context("read the database worker's runtime configuration")?;
    let url = format!("sqlite:{}", data_dir.join("control.sqlite").display());
    let config = current["value"].clone();
    if with_pool(config.clone(), database, &url)?.is_some() {
        let setter = call(
            iii,
            "engine::functions::info",
            json!({"function_id": "configuration::set", "namespace": "default"}),
        )
        .await
        .context("inspect runtime configuration support")?;
        if setter["request_schema"]["properties"]
            .get("flush")
            .is_none()
        {
            bail!(
                "automatic E2E pool setup requires runtime-only configuration writes; \
                   upgrade iii or provision the control database explicitly"
            );
        }
        apply_pool(config, database, &url, deadline, |value| {
            call(
                iii,
                "configuration::set",
                json!({"id": id, "value": value, "flush": false}),
            )
        })
        .await
        .context("add the E2E pool to the database worker's runtime configuration")?;
        tracing::info!(
            namespace,
            database,
            "provisioned the dedicated E2E SQLite pool"
        );
    }
    Ok(())
}

pub(super) fn retryable(error: &anyhow::Error) -> bool {
    error.is::<DatabaseNotReady>()
        || matches!(error.downcast_ref::<iii_sdk::errors::Error>(),
        Some(iii_sdk::errors::Error::Remote { code, .. })
        if code == "NOT_FOUND" || code == "function_not_found")
}

#[derive(Debug)]
struct DatabaseNotReady;

impl std::fmt::Display for DatabaseNotReady {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("database worker has not registered its configuration subscription yet")
    }
}

impl std::error::Error for DatabaseNotReady {}

/// Rebase the runtime overlay on an operator edit observed in `old_value`.
/// The service replaces whole values; saved values are never changed here.
async fn apply_pool<F, Fut>(
    config: Value,
    database: &str,
    url: &str,
    deadline: tokio::time::Instant,
    mut replace: F,
) -> Result<()>
where
    F: FnMut(Value) -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    let mut expected = config;
    let Some(mut value) = with_pool(expected.clone(), database, url)? else {
        return Ok(());
    };
    loop {
        if tokio::time::Instant::now() >= deadline {
            bail!("concurrent database configuration changes did not settle before startup");
        }
        let response = replace(value.clone()).await?;
        let previous = response
            .get("old_value")
            .context("runtime configuration response must identify the replaced value")?
            .clone();
        if previous == expected {
            return Ok(());
        }
        expected = value;
        value = with_pool(previous.clone(), database, url)?.unwrap_or(previous);
    }
}

fn configuration_id(worker: &Value, namespace: &str) -> Result<String> {
    // Older engines omit namespace metadata; their default namespace is the
    // only destination that can be established without that metadata.
    let owner_namespace = worker["worker"]["namespace"].as_str().unwrap_or("default");
    if owner_namespace != namespace {
        bail!("database worker does not belong to the control namespace {namespace}");
    }
    let ids: std::collections::BTreeSet<&str> = worker["registered_triggers"]
        .as_array()
        .context("database worker has no configuration subscriptions")?
        .iter()
        .filter(|row| {
            row["trigger_type"] == "configuration"
                && row["function_id"] == "database::on-config-change"
        })
        .filter_map(|row| row["config"]["configuration_id"].as_str())
        .filter(|id| !id.trim().is_empty())
        .collect();
    if ids.is_empty() {
        return Err(DatabaseNotReady.into());
    }
    if ids.len() != 1 {
        bail!("database worker must identify one configuration subscription");
    }
    Ok(ids.into_iter().next().unwrap().to_owned())
}

fn with_pool(mut config: Value, database: &str, url: &str) -> Result<Option<Value>> {
    let pools = config
        .get_mut("databases")
        .and_then(Value::as_object_mut)
        .context("database configuration must declare its existing pools")?;
    // Preserve every explicit destination, including an unavailable one.
    // A connection failure must never silently redirect stored results.
    if pools.contains_key(database) {
        return Ok(None);
    }
    pools.insert(database.into(), json!({"url": url, "pool": {"max": 1}}));
    Ok(Some(config))
}

async fn call(iii: &IIIClient, function: &str, payload: Value) -> Result<Value> {
    iii.trigger(
        TriggerRequest {
            function_id: function.into(),
            payload,
            action: None,
            timeout_ms: Some(15_000),
        }
        .namespace("default"),
    )
    .await
    .map_err(anyhow::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_the_control_pool_preserves_other_pools_and_unexpanded_credentials() {
        let config = json!({
            "databases": {
                "primary": {"url": "${DATABASE_URL}", "pool": {"max": 12}},
                "analytics": {"url": "postgres://example/analytics"},
            },
            "history_max_entries": 42,
            "history_max_bytes": 4096,
        });
        let updated = with_pool(
            config.clone(),
            "harness_e2e",
            "sqlite:/evidence/control.sqlite",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            updated["databases"]["primary"],
            config["databases"]["primary"]
        );
        assert_eq!(
            updated["databases"]["analytics"],
            config["databases"]["analytics"]
        );
        assert_eq!(updated["history_max_entries"], 42);
        assert_eq!(updated["history_max_bytes"], 4096);
        assert_eq!(
            updated["databases"]["harness_e2e"],
            json!({"url": "sqlite:/evidence/control.sqlite", "pool": {"max": 1}})
        );
    }

    #[test]
    fn an_existing_control_pool_is_never_replaced() {
        for existing in [json!({"url": "postgres://example/e2e"}), Value::Null] {
            let config = json!({"databases": {"harness_e2e": existing}});
            assert!(
                with_pool(config, "harness_e2e", "sqlite:/new/control.sqlite")
                    .unwrap()
                    .is_none()
            );
        }
        for config in [Value::Null, json!({}), json!({"databases": []})] {
            assert!(with_pool(config, "harness_e2e", "sqlite:/new/control.sqlite").is_err());
        }
    }

    #[test]
    fn the_database_subscription_identifies_a_custom_config_and_namespace() {
        let mut worker = json!({
            "worker": {"name": "sql-service", "namespace": "custom-project"},
            "registered_triggers": [
                {"trigger_type": "configuration", "function_id": "database::on-config-change",
                 "config": {"configuration_id": "operator-sql-config"}},
                {"trigger_type": "configuration", "function_id": "other::reload",
                 "config": {"configuration_id": "unrelated"}},
            ],
        });
        assert_eq!(
            configuration_id(&worker, "custom-project").unwrap(),
            "operator-sql-config"
        );
        assert!(configuration_id(&worker, "another-project").is_err());
        worker["registered_triggers"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "trigger_type": "configuration", "function_id": "database::on-config-change",
                "config": {"configuration_id": "ambiguous"},
            }));
        assert!(configuration_id(&worker, "custom-project").is_err());
    }

    #[test]
    fn a_database_still_registering_its_subscription_is_retryable() {
        let worker = json!({"worker": {"namespace": "project-one"}, "registered_triggers": []});
        let error = configuration_id(&worker, "project-one").unwrap_err();
        assert!(retryable(&error));
        let other_namespace = configuration_id(&worker, "project-two").unwrap_err();
        assert!(!retryable(&other_namespace));
    }

    #[tokio::test]
    async fn intervening_operator_edits_are_rebased_and_custom_destinations_restored() {
        for custom_destination in [false, true] {
            let snapshot = json!({"databases": {"primary": {"url": "sqlite:original.db"}}});
            let mut operator = snapshot.clone();
            operator["databases"]["analytics"] = json!({"url": "postgres://example/analytics"});
            operator["history_max_entries"] = json!(7);
            if custom_destination {
                operator["databases"]["harness_e2e"] = json!({"url": "postgres://example/e2e"});
            }
            let active = std::cell::RefCell::new(operator.clone());
            apply_pool(
                snapshot,
                "harness_e2e",
                "sqlite:control.sqlite",
                tokio::time::Instant::now() + std::time::Duration::from_secs(1),
                |value| {
                    let old = active.replace(value);
                    std::future::ready(Ok(json!({"old_value": old})))
                },
            )
            .await
            .unwrap();
            let final_value = active.into_inner();
            assert_eq!(
                final_value["databases"]["analytics"],
                operator["databases"]["analytics"]
            );
            assert_eq!(final_value["history_max_entries"], 7);
            if custom_destination {
                assert_eq!(final_value, operator);
            } else {
                assert_eq!(
                    final_value["databases"]["harness_e2e"],
                    json!({"url": "sqlite:control.sqlite", "pool": {"max": 1}})
                );
            }
        }
    }
}
