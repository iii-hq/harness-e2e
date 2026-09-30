//! Add the built-in E2E pool to the database worker's runtime configuration.
//! Compose's Registry graph carries dependency versions, not per-consumer
//! configuration. Discover the actual database instance/configuration id;
//! never assume its container is called `database` or save over its settings.
//!
//! The pool lives only in the active (runtime) value, so it has to be kept
//! there: a saved edit of the database configuration replaces the runtime
//! value, and the database worker builds every pool or none, so a pool it
//! cannot open must not stay behind.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use iii_sdk::protocol::{RegisterTriggerInput, TriggerRequest};
use iii_sdk::{IIIClient, RegisterFunction};
use serde_json::{json, Value};

const CALL_TIMEOUT_MS: u64 = 15_000;
/// How long one out-of-startup write (restore, removal) may keep rebasing.
const WRITE_WINDOW: Duration = Duration::from_secs(15);
const RESTORE_FUNCTION_ID: &str = "e2e::control-pool-restore";

/// The built-in control pool: one SQLite connection under `data_dir`.
pub(super) struct ControlPool {
    namespace: String,
    database: String,
    url: String,
}

impl ControlPool {
    pub(super) fn new(namespace: &str, database: &str, data_dir: &Path) -> Self {
        Self {
            namespace: namespace.to_owned(),
            database: database.to_owned(),
            url: format!("sqlite:{}", data_dir.join("control.sqlite").display()),
        }
    }

    /// Add the pool to the database worker's active configuration unless a
    /// pool of that name is already there.
    pub(super) async fn provision(
        &self,
        iii: &IIIClient,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        let id = self.configuration_id(iii).await?;
        self.ensure(iii, &id, deadline).await
    }

    /// Take back the pool this worker added, when it is still exactly ours.
    pub(super) async fn withdraw(&self, iii: &IIIClient) -> Result<()> {
        let id = self.configuration_id(iii).await?;
        let current = active_value(iii, &id).await?;
        let Some(value) = self.without_pool(current.clone()) else {
            return Ok(());
        };
        let deadline = tokio::time::Instant::now() + WRITE_WINDOW;
        rebase(
            current,
            value,
            |previous| Ok(self.without_pool(previous)),
            deadline,
            |value| set_runtime(iii, &id, value),
        )
        .await
        .context("remove the E2E pool from the database worker's runtime configuration")?;
        tracing::warn!(
            namespace = %self.namespace,
            database = %self.database,
            "removed the E2E pool the database worker could not serve"
        );
        Ok(())
    }

    /// Restore the pool whenever the database configuration is replaced. A
    /// saved edit drops runtime values, the pool with them.
    pub(super) async fn keep(self, iii: &IIIClient) -> Result<()> {
        let id = self.configuration_id(iii).await?;
        let pool = Arc::new(self);
        let (handler_pool, handler_iii, handler_id) = (pool.clone(), iii.clone(), id.clone());
        iii.register_function(
            RESTORE_FUNCTION_ID,
            RegisterFunction::new_async(move |_event: Value| {
                // The event is advisory; the active value is re-read.
                let (pool, iii, id) = (
                    handler_pool.clone(),
                    handler_iii.clone(),
                    handler_id.clone(),
                );
                async move {
                    let deadline = tokio::time::Instant::now() + WRITE_WINDOW;
                    if let Err(error) = pool.ensure(&iii, &id, deadline).await {
                        tracing::error!(
                            error = format!("{error:#}"),
                            "could not restore the E2E pool after a database configuration change"
                        );
                    }
                    Ok::<Value, iii_sdk::errors::Error>(json!({"ok": true}))
                }
            })
            .description("Internal: restore the E2E pool after the database configuration changes.")
            .metadata(json!({"internal": true})),
        );
        iii.register_trigger(RegisterTriggerInput::new(
            "configuration",
            RESTORE_FUNCTION_ID,
            json!({"configuration_id": id, "event_types": ["configuration:updated"]}),
        ))
        .context("subscribe to the database worker's configuration")?;
        // A save between provisioning and this subscription went unobserved.
        pool.ensure(iii, &id, tokio::time::Instant::now() + WRITE_WINDOW)
            .await
    }

    async fn ensure(
        &self,
        iii: &IIIClient,
        id: &str,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        let current = active_value(iii, id).await?;
        let Some(value) = self.with_pool(current.clone())? else {
            return Ok(());
        };
        require_runtime_writes(iii).await?;
        rebase(
            current,
            value,
            |previous| self.with_pool(previous),
            deadline,
            |value| set_runtime(iii, id, value),
        )
        .await
        .context("add the E2E pool to the database worker's runtime configuration")?;
        tracing::info!(
            namespace = %self.namespace,
            database = %self.database,
            "provisioned the dedicated E2E SQLite pool"
        );
        Ok(())
    }

    async fn configuration_id(&self, iii: &IIIClient) -> Result<String> {
        let namespace = self.namespace.as_str();
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
        subscribed_configuration(&worker, namespace)
    }

    fn entry(&self) -> Value {
        json!({"url": self.url, "pool": {"max": 1}})
    }

    fn with_pool(&self, mut config: Value) -> Result<Option<Value>> {
        let pools = config
            .get_mut("databases")
            .and_then(Value::as_object_mut)
            .context("database configuration must declare its existing pools")?;
        // Preserve every explicit destination, including an unavailable one.
        // A connection failure must never silently redirect stored results.
        if pools.contains_key(&self.database) {
            return Ok(None);
        }
        pools.insert(self.database.clone(), self.entry());
        Ok(Some(config))
    }

    fn without_pool(&self, mut config: Value) -> Option<Value> {
        let pools = config.get_mut("databases")?.as_object_mut()?;
        if pools.get(&self.database) != Some(&self.entry()) {
            return None;
        }
        pools.remove(&self.database);
        Some(config)
    }
}

/// Replace the active value with `value`, rebasing on an edit observed in
/// `old_value`. The service replaces whole values; saved values are never
/// changed here. `edit` re-applies the change to a newer value (`None` when
/// that value already has it, which is then restored as is).
async fn rebase<E, F, Fut>(
    snapshot: Value,
    value: Value,
    edit: E,
    deadline: tokio::time::Instant,
    mut replace: F,
) -> Result<()>
where
    E: Fn(Value) -> Result<Option<Value>>,
    F: FnMut(Value) -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    let (mut expected, mut value) = (snapshot, value);
    loop {
        if tokio::time::Instant::now() >= deadline {
            bail!("concurrent database configuration changes did not settle in time");
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
        value = edit(previous.clone())?.unwrap_or(previous);
    }
}

fn subscribed_configuration(worker: &Value, namespace: &str) -> Result<String> {
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
    match ids.len() {
        0 => bail!("database worker has not registered its configuration subscription yet"),
        1 => Ok(ids.into_iter().next().unwrap().to_owned()),
        _ => bail!("database worker must identify one configuration subscription"),
    }
}

async fn active_value(iii: &IIIClient, id: &str) -> Result<Value> {
    let current = call(iii, "configuration::get", json!({"id": id, "raw": true}))
        .await
        .context("read the database worker's runtime configuration")?;
    Ok(current["value"].clone())
}

/// A saved write would change the operator's configuration; refuse instead.
async fn require_runtime_writes(iii: &IIIClient) -> Result<()> {
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
    Ok(())
}

async fn set_runtime(iii: &IIIClient, id: &str, value: Value) -> Result<Value> {
    call(
        iii,
        "configuration::set",
        json!({"id": id, "value": value, "flush": false}),
    )
    .await
}

async fn call(iii: &IIIClient, function: &str, payload: Value) -> Result<Value> {
    iii.trigger(
        TriggerRequest {
            function_id: function.into(),
            payload,
            action: None,
            timeout_ms: Some(CALL_TIMEOUT_MS),
        }
        .namespace("default"),
    )
    .await
    .map_err(anyhow::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> ControlPool {
        ControlPool::new("project-one", "harness_e2e", Path::new("/evidence"))
    }

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
        let updated = pool().with_pool(config.clone()).unwrap().unwrap();
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
            assert!(pool().with_pool(config).unwrap().is_none());
        }
        for config in [Value::Null, json!({}), json!({"databases": []})] {
            assert!(pool().with_pool(config).is_err());
        }
    }

    #[test]
    fn withdrawing_removes_only_the_pool_this_worker_added() {
        let added = pool()
            .with_pool(json!({"databases": {"primary": {"url": "sqlite:app.db"}}}))
            .unwrap()
            .unwrap();
        assert_eq!(
            pool().without_pool(added),
            Some(json!({"databases": {"primary": {"url": "sqlite:app.db"}}}))
        );
        // An operator's pool of the same name, or another data_dir's, stays.
        let operator = json!({"databases": {"harness_e2e": {"url": "postgres://example/e2e"}}});
        assert_eq!(pool().without_pool(operator), None);
        let elsewhere = ControlPool::new("project-one", "harness_e2e", Path::new("/other"));
        let added = elsewhere.with_pool(json!({"databases": {}})).unwrap();
        assert_eq!(pool().without_pool(added.unwrap()), None);
        for config in [Value::Null, json!({}), json!({"databases": {}})] {
            assert_eq!(pool().without_pool(config), None);
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
            subscribed_configuration(&worker, "custom-project").unwrap(),
            "operator-sql-config"
        );
        assert!(subscribed_configuration(&worker, "another-project").is_err());
        worker["registered_triggers"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "trigger_type": "configuration", "function_id": "database::on-config-change",
                "config": {"configuration_id": "ambiguous"},
            }));
        assert!(subscribed_configuration(&worker, "custom-project").is_err());
        let registering =
            json!({"worker": {"namespace": "project-one"}, "registered_triggers": []});
        let error = subscribed_configuration(&registering, "project-one").unwrap_err();
        assert!(error.to_string().contains("not registered"), "{error}");
    }

    /// Replays `rebase` against an active value an operator edits once,
    /// between the snapshot and the first write.
    async fn race(
        snapshot: Value,
        operator: Value,
        edit: impl Fn(Value) -> Result<Option<Value>>,
    ) -> Value {
        let active = std::cell::RefCell::new(operator);
        let value = edit(snapshot.clone()).unwrap().unwrap();
        rebase(
            snapshot,
            value,
            edit,
            tokio::time::Instant::now() + Duration::from_secs(1),
            |value| {
                let old = active.replace(value);
                std::future::ready(Ok(json!({"old_value": old})))
            },
        )
        .await
        .unwrap();
        active.into_inner()
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
            let pool = pool();
            let final_value = race(snapshot, operator.clone(), |c| pool.with_pool(c)).await;
            assert_eq!(
                final_value["databases"]["analytics"],
                operator["databases"]["analytics"]
            );
            assert_eq!(final_value["history_max_entries"], 7);
            if custom_destination {
                assert_eq!(final_value, operator);
            } else {
                assert_eq!(final_value["databases"]["harness_e2e"], pool.entry());
            }
        }
    }

    #[tokio::test]
    async fn withdrawing_keeps_an_intervening_operator_edit() {
        let pool = pool();
        let snapshot = pool
            .with_pool(json!({"databases": {"primary": {"url": "sqlite:app.db"}}}))
            .unwrap()
            .unwrap();
        let mut operator = snapshot.clone();
        operator["history_max_entries"] = json!(7);
        let final_value = race(snapshot, operator, |c| Ok(pool.without_pool(c))).await;
        assert_eq!(
            final_value,
            json!({"databases": {"primary": {"url": "sqlite:app.db"}}, "history_max_entries": 7})
        );
    }
}
