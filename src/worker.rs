use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Args;
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::runtime::WorkerMetadata;
use iii_sdk::{register_worker, InitOptions};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::control::ControlPlane;
use crate::manifest::WORKER_NAME;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerConfig {
    pub data_dir: String,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            data_dir: "~/.iii/data/harness-e2e".into(),
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct WorkerArgs {
    #[arg(long, env = "III_URL", default_value = "ws://127.0.0.1:49134")]
    pub url: String,

    /// Static worker configuration written by `iii worker add`.
    #[arg(long, default_value = "config.yaml")]
    pub config: PathBuf,

    /// Override the configured data directory for local development.
    #[arg(long, alias = "output-root", env = "HARNESS_E2E_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
}

impl Default for WorkerArgs {
    fn default() -> Self {
        Self {
            url: "ws://127.0.0.1:49134".into(),
            config: PathBuf::from("config.yaml"),
            data_dir: None,
        }
    }
}

pub async fn serve(args: WorkerArgs) -> Result<()> {
    let config = load_config(&args.config, args.data_dir.as_deref())?;
    let data_dir = expand_home(&config.data_dir)?;
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("create worker data directory {}", data_dir.display()))?;

    let iii = register_worker(
        &args.url,
        InitOptions {
            metadata: Some(WorkerMetadata {
                runtime: "rust".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                name: WORKER_NAME.into(),
                os: std::env::consts::OS.into(),
                pid: Some(std::process::id()),
                ..WorkerMetadata::default()
            }),
            ..InitOptions::default()
        },
    );
    wait_for_state(&iii).await?;
    let control = ControlPlane::new(iii.clone(), args.url, data_dir)
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

fn load_config(path: &Path, override_dir: Option<&Path>) -> Result<WorkerConfig> {
    let mut config = if path.is_file() {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("read worker config {}", path.display()))?;
        serde_yaml::from_str(&source)
            .with_context(|| format!("decode worker config {}", path.display()))?
    } else {
        WorkerConfig::default()
    };
    if let Some(data_dir) = override_dir {
        config.data_dir = data_dir.to_string_lossy().into_owned();
    }
    if config.data_dir.trim().is_empty() {
        bail!("worker config data_dir cannot be empty");
    }
    Ok(config)
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

    #[test]
    fn config_defaults_without_a_file_and_accepts_a_cli_override() {
        let missing = PathBuf::from("definitely-missing-harness-e2e-config.yaml");
        assert_eq!(
            load_config(&missing, None).unwrap(),
            WorkerConfig::default()
        );
        assert_eq!(
            load_config(&missing, Some(Path::new("target/custom")))
                .unwrap()
                .data_dir,
            "target/custom"
        );
    }

    #[test]
    fn config_rejects_an_empty_data_directory() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yaml");
        std::fs::write(&path, "data_dir: ''\n").unwrap();
        assert!(load_config(&path, None)
            .unwrap_err()
            .to_string()
            .contains("data_dir cannot be empty"));
    }
}
