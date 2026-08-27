use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Args;
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::runtime::WorkerMetadata;
use iii_sdk::{register_worker, InitOptions};
use serde::{Deserialize, Serialize};

use crate::control::ControlPlane;
use crate::manifest::WORKER_NAME;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerConfig {
    /// Directory where E2E execution records, plans, reports and dashboard state live.
    pub data_dir: String,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            data_dir: "~/.iii/data/harness-e2e".into(),
        }
    }
}

impl WorkerConfig {
    fn validate(self) -> Result<Self, String> {
        if self.data_dir.trim().is_empty() {
            return Err("data_dir cannot be empty".into());
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Default, Args)]
pub struct WorkerArgs {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkerEnvironment {
    url: String,
    namespace: String,
    worker_name: String,
    config: PathBuf,
}

impl WorkerEnvironment {
    fn read() -> Result<Self> {
        Self::from_environment(|name| std::env::var(name).ok())
    }

    fn from_environment(mut environment: impl FnMut(&str) -> Option<String>) -> Result<Self> {
        let required = |name: &str, value: Option<String>| -> Result<String> {
            value
                .filter(|value| !value.trim().is_empty())
                .with_context(|| format!("{name} is required; start harness-e2e through iii compose"))
        };
        let worker_name = required("III_WORKER_NAME", environment("III_WORKER_NAME"))?;
        if worker_name != WORKER_NAME {
            bail!("III_WORKER_NAME must be '{WORKER_NAME}', got '{worker_name}'");
        }
        Ok(Self {
            url: required("III_URL", environment("III_URL"))?,
            namespace: required("III_NAMESPACE", environment("III_NAMESPACE"))?,
            worker_name,
            config: PathBuf::from(required("III_CONFIG", environment("III_CONFIG"))?),
        })
    }
}

pub async fn serve(_args: WorkerArgs) -> Result<()> {
    let environment = WorkerEnvironment::read()?;
    let config = load_config(&environment.config)?;

    let iii = register_worker(
        &environment.url,
        InitOptions {
            metadata: Some(WorkerMetadata {
                runtime: "rust".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                name: environment.worker_name.clone(),
                os: std::env::consts::OS.into(),
                pid: Some(std::process::id()),
                namespace: Some(environment.namespace.clone()),
                ..WorkerMetadata::default()
            }),
            namespace: Some(environment.namespace.clone()),
            ..InitOptions::default()
        },
    );
    wait_for_state(&iii).await?;

    let data_dir = resolve_data_dir(&config.data_dir)?;
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("create worker data directory {}", data_dir.display()))?;
    tracing::info!(
        data_dir = %data_dir.display(),
        namespace = %environment.namespace,
        config = %environment.config.display(),
        "Harness E2E storage directory selected"
    );
    let control = ControlPlane::new(iii.clone(), environment.url, data_dir)
        .await
        .context("restore the E2E control plane")?;
    control.register();
    crate::console_ui::register(&iii);
    crate::dashboard::register_worker_functions(&iii, control.clone())
        .await
        .context("register dashboard functions")?;
    tracing::info!(worker = WORKER_NAME, "e2e control plane ready");
    shutdown_signal().await?;
    iii.shutdown_async().await;
    Ok(())
}

fn load_config(path: &Path) -> Result<WorkerConfig> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("read III_CONFIG {}", path.display()))?;
    let config: WorkerConfig = serde_yaml::from_str(&source)
        .with_context(|| format!("decode III_CONFIG {}", path.display()))?;
    config.validate().map_err(anyhow::Error::msg)
}

fn resolve_data_dir(value: &str) -> Result<PathBuf> {
    if value.trim().is_empty() {
        bail!("worker config data_dir cannot be empty");
    }
    expand_home(value)
}

fn expand_home(value: &str) -> Result<PathBuf> {
    if value == "~" || value.starts_with("~/") || value.starts_with("~\\") {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .context("data_dir uses '~' but no home directory is available")?;
        let suffix = value
            .strip_prefix("~/")
            .or_else(|| value.strip_prefix("~\\"))
            .unwrap_or_default();
        return Ok(PathBuf::from(home).join(suffix));
    }
    Ok(PathBuf::from(value))
}

async fn wait_for_state(iii: &iii_sdk::IIIClient) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let ready = iii
            .trigger(TriggerRequest {
                function_id: "state::list".into(),
                payload: serde_json::json!({ "scope": "harness_e2e_execution" }),
                action: None,
                timeout_ms: Some(1_000),
            })
            .await
            .is_ok();
        if ready {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("state::list was not ready within 30 seconds");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(unix)]
async fn shutdown_signal() -> Result<()> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut terminate = signal(SignalKind::terminate()).context("bind SIGTERM")?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result.context("bind Ctrl+C")?,
        _ = terminate.recv() => {},
    }
    Ok(())
}

#[cfg(not(unix))]
async fn shutdown_signal() -> Result<()> {
    tokio::signal::ctrl_c().await.context("bind Ctrl+C")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn config_must_be_the_compose_materialized_file() {
        let missing = PathBuf::from("definitely-missing-harness-e2e-config.yaml");
        assert!(load_config(&missing).unwrap_err().to_string().contains("read III_CONFIG"));
    }

    #[test]
    fn config_rejects_an_empty_data_directory() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yaml");
        std::fs::write(&path, "data_dir: ''\n").unwrap();
        assert!(load_config(&path)
            .unwrap_err()
            .to_string()
            .contains("data_dir cannot be empty"));
    }

    #[test]
    fn compose_environment_is_required_and_preserved() {
        let values = BTreeMap::from([
            ("III_WORKER_NAME", WORKER_NAME.to_string()),
            ("III_URL", "ws://127.0.0.1:49259".to_string()),
            ("III_NAMESPACE", "campaign-123".to_string()),
            ("III_CONFIG", "/tmp/compose/harness-e2e.yaml".to_string()),
        ]);
        let environment =
            WorkerEnvironment::from_environment(|name| values.get(name).cloned()).unwrap();

        assert_eq!(environment.url, "ws://127.0.0.1:49259");
        assert_eq!(environment.namespace, "campaign-123");
        assert_eq!(environment.worker_name, WORKER_NAME);
        assert_eq!(environment.config, PathBuf::from("/tmp/compose/harness-e2e.yaml"));
    }

    #[test]
    fn standalone_start_is_rejected() {
        let error = WorkerEnvironment::from_environment(|_| None).unwrap_err();
        assert!(error.to_string().contains("III_WORKER_NAME is required"));
    }
}
