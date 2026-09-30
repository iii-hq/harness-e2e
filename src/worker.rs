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

mod control_database;

use control_database::ControlPool;

const STARTUP_DEADLINE: Duration = Duration::from_secs(60);
const RETRY_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerConfig {
    /// Directory for native evidence bundles, journals, logs and exports.
    pub data_dir: String,
    /// Pool name for durable control-plane records.
    #[serde(default = "default_control_database")]
    pub control_database: String,
    /// Namespace that owns the control-plane database worker; omitted uses
    /// this worker's Compose namespace and provisions its dedicated pool.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub control_namespace: String,
    /// GitHub repository whose exact-stack workflow runs can be imported.
    #[serde(default = "default_github_repository")]
    pub github_repository: String,
    /// Docker executions: groups running at once, across executions.
    #[serde(default = "default_docker_parallel_groups")]
    pub docker_parallel_groups: usize,
    /// Docker executions: an env file of provider credentials under the
    /// Console's own (`credentials.env` in `data_dir`, set from the Stacks
    /// page): where both name one, the Console's wins. Resolved as
    /// `data_dir` is.
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
            control_namespace: String::new(),
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
        if self.control_database.trim().is_empty() {
            return Err("control database cannot be empty".into());
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

    /// The namespace holding the control database, and the built-in pool to
    /// provision there. Explicit destinations remain operator-owned; only the
    /// default pool in the installation namespace is added automatically.
    fn control_destination<'a>(
        &'a self,
        worker_namespace: &'a str,
        data_dir: &Path,
    ) -> (&'a str, Option<ControlPool>) {
        if !self.control_namespace.trim().is_empty() {
            return (&self.control_namespace, None);
        }
        let pool = (self.control_database == default_control_database())
            .then(|| ControlPool::new(worker_namespace, &self.control_database, data_dir));
        (worker_namespace, pool)
    }

    /// How Docker executions run, with paths resolved as `data_dir` is.
    pub(crate) fn docker(&self, base: &Path) -> Result<crate::plans::store::DockerSettings> {
        let path = |value: &Option<String>| {
            value
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(|value| resolve_data_dir(value, base))
                .transpose()
        };
        Ok(crate::plans::store::DockerSettings {
            parallel_groups: self.docker_parallel_groups,
            provider_env_file: path(&self.provider_env_file)?,
            scripts_dir: path(&self.scripts_dir)?,
            registry_mirrors: self.docker_registry_mirrors.clone(),
            logins: None,
        })
    }
}

#[derive(Debug, Clone, Default, Args)]
pub struct WorkerArgs {}

/// Where the worker's configuration comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfigSource {
    /// `III_CONFIG`: a file, as iii compose before 0.24.3 (and the release
    /// job) delivers it.
    File(PathBuf),
    /// `III_CONFIG_NAME`: the configuration-service entry iii compose 0.24.3+
    /// injects the merged value into, read with `configuration::get`.
    Service(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkerEnvironment {
    url: String,
    namespace: String,
    worker_name: String,
    config: ConfigSource,
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
        let present = |value: Option<String>| value.filter(|value| !value.trim().is_empty());
        let config = match (
            present(environment("III_CONFIG")),
            present(environment("III_CONFIG_NAME")),
        ) {
            (Some(path), _) => ConfigSource::File(PathBuf::from(path)),
            (None, Some(name)) => ConfigSource::Service(name.trim().to_string()),
            (None, None) => {
                bail!("III_CONFIG or III_CONFIG_NAME is required; start harness-e2e through iii compose")
            }
        };
        Ok(Self {
            url: required("III_URL", environment("III_URL"))?,
            namespace: required("III_NAMESPACE", environment("III_NAMESPACE"))?,
            worker_name,
            config,
        })
    }
}

pub async fn serve(_args: WorkerArgs) -> Result<()> {
    let environment = WorkerEnvironment::read()?;

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
    // Relative paths resolve beside the config file, or from the working
    // directory when the configuration service delivers the value.
    let (config, base) = match &environment.config {
        ConfigSource::File(path) => (load_config(path)?, config_file_dir(path)?),
        ConfigSource::Service(name) => (
            fetch_config(&iii, name).await?,
            std::env::current_dir().context("resolve the working directory")?,
        ),
    };
    let data_dir = resolve_data_dir(&config.data_dir, &base)?;
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("create worker data directory {}", data_dir.display()))?;
    let (control_namespace, pool) = config.control_destination(&environment.namespace, &data_dir);
    wait_for_persistence(
        &iii,
        control_namespace,
        &config.control_database,
        pool.as_ref(),
    )
    .await?;
    if let Some(pool) = pool {
        pool.keep(&iii)
            .await
            .context("keep the E2E pool in the database worker's runtime configuration")?;
    }
    tracing::info!(
        data_dir = %data_dir.display(),
        namespace = %environment.namespace,
        config = ?environment.config,
        "Harness E2E storage directory selected"
    );
    let control = ControlPlane::new_with_persistence(
        iii.clone(),
        environment.url,
        data_dir,
        Persistence::new(
            iii.clone(),
            config.control_database.clone(),
            control_namespace.to_owned(),
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
        config.docker(&base)?,
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

/// The configuration `name` holds in the configuration service, read as
/// the ecosystem workers read theirs: `configuration::get` in the default
/// namespace, placeholders expanded. No entry means the built-in defaults.
async fn fetch_config(iii: &iii_sdk::IIIClient, name: &str) -> Result<WorkerConfig> {
    let response = iii
        .trigger(
            TriggerRequest {
                function_id: "configuration::get".into(),
                payload: serde_json::json!({ "id": name }),
                action: None,
                timeout_ms: Some(15_000),
            }
            .namespace("default"),
        )
        .await;
    let value = match response {
        Ok(response) => response.get("value").cloned().unwrap_or_default(),
        Err(iii_sdk::errors::Error::Remote { code, .. }) if code == "NOT_FOUND" => {
            serde_json::Value::Null
        }
        Err(error) => bail!("read configuration {name}: {error}"),
    };
    config_from_value(value).with_context(|| format!("decode configuration {name}"))
}

fn config_from_value(value: serde_json::Value) -> Result<WorkerConfig> {
    if value.is_null() {
        tracing::info!("no configuration value found; using built-in default configuration");
        return Ok(WorkerConfig::default());
    }
    let config: WorkerConfig = serde_json::from_value(value)?;
    config.validate().map_err(anyhow::Error::msg)
}

fn config_file_dir(path: &Path) -> Result<PathBuf> {
    Ok(path
        .canonicalize()
        .with_context(|| format!("resolve worker config {}", path.display()))?
        .parent()
        .context("worker config has no parent directory")?
        .to_path_buf())
}

/// `value` with `~` expanded, relative to `base` when relative.
pub fn resolve_data_dir(value: &str, base: &Path) -> Result<PathBuf> {
    if value.trim().is_empty() {
        bail!("worker config data_dir cannot be empty");
    }
    let path = expand_home(value)?;
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(base.join(path))
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
    pool: Option<&ControlPool>,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + STARTUP_DEADLINE;
    let Err(error) = poll_ready(
        deadline,
        || query_control_database(iii, namespace, database),
        pool.map(|pool| move || pool.provision(iii, deadline)),
    )
    .await
    else {
        return Ok(());
    };
    // The database worker builds every pool or none: an E2E pool it cannot
    // open would make its next restart fail for every other database too.
    if let Some(pool) = pool {
        if let Err(cleanup) = pool.withdraw(iii).await {
            tracing::warn!(
                error = format!("{cleanup:#}"),
                "could not check the database configuration for an E2E pool to remove"
            );
        }
    }
    bail!(
        "control-plane database was not ready before the startup deadline \
         (namespace {namespace}, database {database}): {error:#}"
    )
}

/// Poll `ready` until `deadline`, running `setup` between polls until it
/// first succeeds. Every setup failure is retried; the deadline error keeps
/// the last one that has not since been resolved.
async fn poll_ready<R, RF, S, SF>(
    deadline: tokio::time::Instant,
    mut ready: R,
    mut setup: Option<S>,
) -> Result<()>
where
    R: FnMut() -> RF,
    RF: std::future::Future<Output = Result<()>>,
    S: FnMut() -> SF,
    SF: std::future::Future<Output = Result<()>>,
{
    let mut setup_error = None;
    loop {
        let error = match ready().await {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        if tokio::time::Instant::now() >= deadline {
            return Err(match setup_error {
                Some(setup) => anyhow::anyhow!("{error:#}; pool setup: {setup}"),
                None => error,
            });
        }
        if let Some(run) = setup.as_mut() {
            match run().await {
                Ok(()) => (setup, setup_error) = (None, None),
                Err(error) => {
                    tracing::debug!(
                        error = format!("{error:#}"),
                        "control database setup pending"
                    );
                    setup_error = Some(format!("{error:#}"));
                }
            }
        }
        tokio::time::sleep(RETRY_INTERVAL).await;
    }
}

async fn query_control_database(
    iii: &iii_sdk::IIIClient,
    namespace: &str,
    database: &str,
) -> Result<()> {
    iii.trigger(
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
    .await?;
    Ok(())
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
        let (namespace, pool) = config.control_destination("another-installation", Path::new("/e"));
        assert_eq!(namespace, "campaign-123");
        assert!(
            pool.is_none(),
            "explicit destinations are operator-provisioned"
        );
        assert_eq!(
            resolve_data_dir(&config.data_dir, &config_file_dir(&path).unwrap()).unwrap(),
            PathBuf::from("/tmp/e2e-evidence")
        );
    }

    #[test]
    fn omitted_control_namespace_routes_to_the_worker_installation() {
        let config = config_from_value(serde_json::json!({"data_dir": "evidence"})).unwrap();
        for installation in ["project-one", "project-two"] {
            let (namespace, pool) = config.control_destination(installation, Path::new("/e"));
            assert_eq!(namespace, installation);
            assert!(pool.is_some(), "the built-in pool is provisioned");
        }
        assert_eq!(config.control_database, "harness_e2e");
        let renamed =
            config_from_value(serde_json::json!({"data_dir": "e", "control_database": "other"}))
                .unwrap();
        let (namespace, pool) = renamed.control_destination("project-one", Path::new("/e"));
        assert_eq!(namespace, "project-one");
        assert!(pool.is_none(), "a renamed pool is operator-provisioned");
    }

    fn outcomes(
        results: Vec<Result<(), &'static str>>,
    ) -> (
        std::rc::Rc<std::cell::Cell<usize>>,
        impl FnMut() -> std::future::Ready<Result<()>>,
    ) {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = calls.clone();
        let mut results = results.into_iter();
        let next = move || {
            counter.set(counter.get() + 1);
            let result = results.next().unwrap_or(Err("exhausted"));
            std::future::ready(result.map_err(anyhow::Error::msg))
        };
        (calls, next)
    }

    #[tokio::test]
    async fn setup_failures_are_retried_until_setup_succeeds_once() {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let (queries, ready) =
            outcomes(vec![Err("no pool"), Err("no pool"), Err("no pool"), Ok(())]);
        let (setups, setup) = outcomes(vec![Err("invocation timed out"), Ok(())]);
        poll_ready(deadline, ready, Some(setup)).await.unwrap();
        assert_eq!(queries.get(), 4);
        assert_eq!(setups.get(), 2, "setup stops after its first success");
    }

    #[tokio::test]
    async fn the_deadline_reports_the_query_and_only_an_unresolved_setup_error() {
        let deadline = tokio::time::Instant::now();
        let (_, ready) = outcomes(vec![Err("unknown database")]);
        let (_, setup) = outcomes(vec![]);
        let error = poll_ready(deadline, ready, Some(setup)).await.unwrap_err();
        assert_eq!(error.to_string(), "unknown database");

        let deadline = tokio::time::Instant::now() + RETRY_INTERVAL;
        let (_, ready) = outcomes(vec![Err("unknown database"); 3]);
        let (_, setup) = outcomes(vec![Err("upgrade iii")]);
        let error = poll_ready(deadline, ready, Some(setup)).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "unknown database; pool setup: upgrade iii"
        );

        let deadline = tokio::time::Instant::now() + RETRY_INTERVAL * 2;
        let (_, ready) = outcomes(vec![Err("unknown database"); 4]);
        let (_, setup) = outcomes(vec![Err("not registered yet"), Ok(())]);
        let error = poll_ready(deadline, ready, Some(setup)).await.unwrap_err();
        assert_eq!(error.to_string(), "unknown database");
    }

    #[test]
    fn relative_data_directory_is_resolved_from_config_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yaml");
        std::fs::write(&path, "data_dir: evidence\n").unwrap();
        let config = load_config(&path).unwrap();
        assert_eq!(
            resolve_data_dir(&config.data_dir, &config_file_dir(&path).unwrap()).unwrap(),
            directory.path().join("evidence")
        );
    }

    #[test]
    fn docker_executions_run_two_groups_at_once_unless_configured() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yaml");
        std::fs::write(&path, "data_dir: evidence\n").unwrap();
        let docker = load_config(&path)
            .unwrap()
            .docker(&config_file_dir(&path).unwrap())
            .unwrap();
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
        let docker = load_config(&path)
            .unwrap()
            .docker(&config_file_dir(&path).unwrap())
            .unwrap();
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
        let docker = load_config(&path)
            .unwrap()
            .docker(&config_file_dir(&path).unwrap())
            .unwrap();
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
            ConfigSource::File(PathBuf::from("/tmp/compose/harness-e2e.yaml"))
        );
    }

    #[test]
    fn config_comes_from_the_file_first_then_the_configuration_service() {
        let read = |pairs: &[(&str, &str)]| {
            let mut values = BTreeMap::from([
                ("III_WORKER_NAME", WORKER_NAME.to_string()),
                ("III_URL", "ws://127.0.0.1:49259".to_string()),
                ("III_NAMESPACE", "campaign-123".to_string()),
            ]);
            values.extend(pairs.iter().map(|(name, value)| (*name, value.to_string())));
            WorkerEnvironment::from_environment(|name| values.get(name).cloned())
        };
        // iii compose before 0.24.3 sets both: the file wins.
        assert_eq!(
            read(&[
                ("III_CONFIG", "/tmp/c.yaml"),
                ("III_CONFIG_NAME", "e2e-x-harness-e2e")
            ])
            .unwrap()
            .config,
            ConfigSource::File(PathBuf::from("/tmp/c.yaml"))
        );
        // iii compose 0.24.3+ sets only the entry name.
        assert_eq!(
            read(&[
                ("III_CONFIG", " "),
                ("III_CONFIG_NAME", "e2e-x-harness-e2e")
            ])
            .unwrap()
            .config,
            ConfigSource::Service("e2e-x-harness-e2e".into())
        );
        assert!(read(&[])
            .unwrap_err()
            .to_string()
            .contains("III_CONFIG or III_CONFIG_NAME is required"));
    }

    #[test]
    fn configuration_service_value_decodes_like_the_file() {
        let config = config_from_value(serde_json::json!({
            "data_dir": "/tmp/e2e-evidence",
            "control_database": "primary",
            "control_namespace": "campaign-123",
        }))
        .unwrap();
        assert_eq!(
            (config.data_dir.as_str(), config.control_database.as_str()),
            ("/tmp/e2e-evidence", "primary")
        );
        assert_eq!(
            config_from_value(serde_json::Value::Null).unwrap(),
            WorkerConfig::default()
        );
        assert!(config_from_value(serde_json::json!({ "data_dir": "" }))
            .unwrap_err()
            .to_string()
            .contains("data_dir cannot be empty"));
        assert_eq!(
            resolve_data_dir("evidence", Path::new("/srv/worker")).unwrap(),
            PathBuf::from("/srv/worker/evidence")
        );
    }

    #[test]
    fn standalone_start_is_rejected() {
        let error = WorkerEnvironment::from_environment(|_| None).unwrap_err();
        assert!(error.to_string().contains("III_WORKER_NAME is required"));
    }
}
