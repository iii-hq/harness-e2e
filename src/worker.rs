use std::collections::BTreeMap;
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
use crate::persistence::Persistence;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerConfig {
    /// Directory for native evidence bundles, journals, logs and exports.
    pub data_dir: String,
    /// Database name in the dedicated control-plane namespace.
    #[serde(default = "default_control_database")]
    pub control_database: String,
    /// Namespace that owns the control-plane database worker.
    #[serde(default = "default_control_namespace")]
    pub control_namespace: String,
    /// GitHub repository whose exact-stack workflow runs can be imported.
    #[serde(default = "default_github_repository")]
    pub github_repository: String,
    /// Docker executions: groups running at once, across executions.
    #[serde(default = "default_docker_parallel_groups")]
    pub docker_parallel_groups: usize,
    /// Docker executions: an env file with the provider credentials the
    /// GitHub groups receive (DEEPSEEK_API_KEY, ZAI_API_KEY,
    /// TYPESAFE_API_KEY), passed to the executor with `--env-file`. Resolved
    /// as `data_dir` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_env_file: Option<String>,
    /// Docker executions: a checkout's `scripts/` to run instead of the
    /// scripts this worker embeds, copied into each new execution, so an
    /// edited script takes effect on the next one. Resolved as `data_dir` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scripts_dir: Option<String>,
    /// Docker executions: a pull-through cache per registry (`docker.io`,
    /// `mcr.microsoft.com`, ...) for the Docker daemon each group runs, which
    /// starts with no image: it pulls from the mirror first, from the
    /// registry itself when the mirror fails.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub docker_registry_mirrors: BTreeMap<String, String>,
}

fn default_control_database() -> String {
    "harness_e2e".into()
}
fn default_control_namespace() -> String {
    "harness-e2e-control".into()
}
fn default_github_repository() -> String {
    "iii-hq/harness-e2e".into()
}
fn default_docker_parallel_groups() -> usize {
    2
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            data_dir: "~/.iii/data/harness-e2e".into(),
            control_database: default_control_database(),
            control_namespace: default_control_namespace(),
            github_repository: default_github_repository(),
            docker_parallel_groups: default_docker_parallel_groups(),
            provider_env_file: None,
            scripts_dir: None,
            docker_registry_mirrors: BTreeMap::new(),
        }
    }
}

impl WorkerConfig {
    fn validate(self) -> Result<Self, String> {
        if self.data_dir.trim().is_empty() {
            return Err("data_dir cannot be empty".into());
        }
        if self.control_database.trim().is_empty() || self.control_namespace.trim().is_empty() {
            return Err("control database and namespace cannot be empty".into());
        }
        if self.docker_parallel_groups == 0 {
            return Err("docker_parallel_groups must be at least 1".into());
        }
        // Each becomes a line of the daemon's hosts.toml.
        let plain = |value: &str| {
            !value.is_empty()
                && !value.contains(|c: char| c.is_whitespace() || "\"'=\\".contains(c))
        };
        for (registry, mirror) in &self.docker_registry_mirrors {
            if !plain(registry)
                || registry.contains('/')
                || !plain(mirror)
                || !(mirror.starts_with("http://") || mirror.starts_with("https://"))
            {
                return Err(format!(
                    "docker_registry_mirrors: {registry:?} needs a registry host and an http(s) URL"
                ));
            }
        }
        Ok(self)
    }

    /// How Docker executions run, with paths resolved as `data_dir` is.
    pub(crate) fn docker(&self, config_path: &Path) -> Result<crate::plans::store::DockerSettings> {
        let path = |value: &Option<String>| {
            value
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(|value| resolve_data_dir(value, config_path))
                .transpose()
        };
        Ok(crate::plans::store::DockerSettings {
            parallel_groups: self.docker_parallel_groups,
            provider_env_file: path(&self.provider_env_file)?,
            scripts_dir: path(&self.scripts_dir)?,
            registry_mirrors: self.docker_registry_mirrors.clone(),
        })
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
                .with_context(|| {
                    format!("{name} is required; start harness-e2e through iii compose")
                })
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
    wait_for_persistence(&iii, &config.control_namespace, &config.control_database).await?;

    let data_dir = resolve_data_dir(&config.data_dir, &environment.config)?;
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("create worker data directory {}", data_dir.display()))?;
    tracing::info!(
        data_dir = %data_dir.display(),
        namespace = %environment.namespace,
        config = %environment.config.display(),
        "Harness E2E storage directory selected"
    );
    let control = ControlPlane::new_with_persistence(
        iii.clone(),
        environment.url,
        data_dir,
        Persistence::new(
            iii.clone(),
            config.control_database.clone(),
            config.control_namespace.clone(),
        ),
    )
    .await
    .context("restore the E2E control plane")?;
    control.register();
    crate::console_ui::register(&iii);
    crate::dashboard::register_worker_functions(
        &iii,
        control.clone(),
        config.github_repository.clone(),
        config.docker(&environment.config)?,
    )
    .await
    .context("register dashboard functions")?;
    tracing::info!(worker = WORKER_NAME, "e2e control plane ready");
    shutdown_signal().await?;
    iii.shutdown_async().await;
    Ok(())
}

pub fn load_config(path: &Path) -> Result<WorkerConfig> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("read worker config {}", path.display()))?;
    let config: WorkerConfig = serde_yaml::from_str(&source)
        .with_context(|| format!("decode worker config {}", path.display()))?;
    config.validate().map_err(anyhow::Error::msg)
}

pub fn resolve_data_dir(value: &str, config_path: &Path) -> Result<PathBuf> {
    if value.trim().is_empty() {
        bail!("worker config data_dir cannot be empty");
    }
    let path = expand_home(value)?;
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(config_path
        .canonicalize()
        .with_context(|| format!("resolve worker config {}", config_path.display()))?
        .parent()
        .context("worker config has no parent directory")?
        .join(path))
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

async fn wait_for_persistence(
    iii: &iii_sdk::IIIClient,
    namespace: &str,
    database: &str,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let ready = iii
            .trigger(
                TriggerRequest {
                    function_id: "database::query".into(),
                    payload: serde_json::json!({
                        "db": database,
                        "sql": "SELECT 1",
                        "params": [],
                    }),
                    action: None,
                    timeout_ms: Some(15_000),
                }
                .namespace(namespace),
            )
            .await
            .is_ok();
        if ready {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("control-plane database was not ready before the startup deadline");
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
        assert!(load_config(&missing)
            .unwrap_err()
            .to_string()
            .contains("read worker config"));
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
    fn config_preserves_custom_persistence_destination() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yaml");
        std::fs::write(
            &path,
            "data_dir: /tmp/e2e-evidence\ncontrol_database: custom_db\ncontrol_namespace: campaign-123\n",
        )
        .unwrap();
        let config = load_config(&path).unwrap();
        assert_eq!(config.control_database, "custom_db");
        assert_eq!(config.control_namespace, "campaign-123");
        assert_eq!(
            resolve_data_dir(&config.data_dir, &path).unwrap(),
            PathBuf::from("/tmp/e2e-evidence")
        );
    }

    #[test]
    fn relative_data_directory_is_resolved_from_config_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yaml");
        std::fs::write(&path, "data_dir: evidence\n").unwrap();
        let config = load_config(&path).unwrap();
        assert_eq!(
            resolve_data_dir(&config.data_dir, &path).unwrap(),
            directory.path().join("evidence")
        );
    }

    #[test]
    fn docker_executions_run_two_groups_at_once_unless_configured() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yaml");
        std::fs::write(&path, "data_dir: evidence\n").unwrap();
        let docker = load_config(&path).unwrap().docker(&path).unwrap();
        assert_eq!(
            (
                docker.parallel_groups,
                docker.provider_env_file,
                docker.scripts_dir
            ),
            (2, None, None)
        );
        // Paths resolve as data_dir does.
        std::fs::write(
            &path,
            "data_dir: evidence\ndocker_parallel_groups: 4\nprovider_env_file: providers.env\nscripts_dir: /src/harness-e2e/scripts\n",
        )
        .unwrap();
        let docker = load_config(&path).unwrap().docker(&path).unwrap();
        assert_eq!(docker.parallel_groups, 4);
        assert_eq!(
            docker.provider_env_file,
            Some(directory.path().join("providers.env"))
        );
        assert_eq!(
            docker.scripts_dir,
            Some(PathBuf::from("/src/harness-e2e/scripts"))
        );
        std::fs::write(&path, "data_dir: evidence\ndocker_parallel_groups: 0\n").unwrap();
        assert!(load_config(&path)
            .unwrap_err()
            .to_string()
            .contains("docker_parallel_groups must be at least 1"));
    }

    #[test]
    fn docker_registry_mirrors_name_a_registry_and_an_http_url() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yaml");
        std::fs::write(
            &path,
            "data_dir: evidence\ndocker_registry_mirrors:\n  mcr.microsoft.com: http://172.17.0.1:5001\n",
        )
        .unwrap();
        let docker = load_config(&path).unwrap().docker(&path).unwrap();
        assert_eq!(
            docker.registry_mirrors,
            BTreeMap::from([(
                "mcr.microsoft.com".to_owned(),
                "http://172.17.0.1:5001".to_owned()
            )])
        );
        for bad in [
            "mcr.microsoft.com: 172.17.0.1:5001",
            "mcr.microsoft.com: 'http://x\" y'",
            "mcr.microsoft.com/playwright: http://x",
        ] {
            std::fs::write(
                &path,
                format!("data_dir: evidence\ndocker_registry_mirrors:\n  {bad}\n"),
            )
            .unwrap();
            assert!(
                load_config(&path)
                    .unwrap_err()
                    .to_string()
                    .contains("docker_registry_mirrors"),
                "{bad}"
            );
        }
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
        assert_eq!(
            environment.config,
            PathBuf::from("/tmp/compose/harness-e2e.yaml")
        );
    }

    #[test]
    fn standalone_start_is_rejected() {
        let error = WorkerEnvironment::from_environment(|_| None).unwrap_err();
        assert!(error.to_string().contains("III_WORKER_NAME is required"));
    }
}
