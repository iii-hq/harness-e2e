//! Executions run in Docker: the phases of the exact-stack workflow, each in
//! the executor image through `scripts/run_in_image.sh`, from a folder of the
//! data directory, then imported with the code that imports a GitHub run.
//!
//! `docker-executions/<execution id>/` holds
//!
//! - `inputs.json`: what `prepare` reads as `DISPATCH_*`: the suite (an id of
//!   the master plan, or the whole suite as JSON), the stack's YAML, the
//!   model and the agent profile. Credentials never go here.
//! - `checkout/`: what the wrapper mounts, at the same path: the Dockerfile
//!   this worker embeds (it names the image), the scripts it embeds or a copy
//!   of `scripts_dir`, both frozen for the execution, and `target/`, every
//!   phase's work as on a runner. `target/artifacts/` keeps the bundles under
//!   the workflow's artifact names: `e2e-contract-<id>-gh-1`,
//!   `e2e-observation-<id>-<campaign>-<group>-gh-<attempt>` and
//!   `e2e-observation-<id>-gh-<attempt>`.
//! - `logs/`: each phase's output.
//!
//! `prepare` (materialize, assemble, fixtures), one `group` container per
//! group, `docker_parallel_groups` at a time across executions, each packaged,
//! then `finalize`, whose root bundle is imported once every group ended.
//! Running a scenario again runs its groups as the next attempt, finalizes
//! again and imports again: the last attempt counts, as a re-run job's does
//! on GitHub.
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::sync::{watch, Semaphore};

use super::github::{directories, highest_attempt, read_json};
use super::{
    canonical, finish, now, sequential_groups, ExecutionParameters, ExecutionSource, PlanExecution,
    PlanStore, Rerun, Slot,
};
use crate::artifact;
use crate::plans::stacks;
use crate::test_plan::{self, MasterPlan};

/// The scripts every phase runs, extracted into each execution's checkout.
#[derive(rust_embed::Embed)]
#[folder = "scripts/"]
#[exclude = "*.pyc"]
#[exclude = "__pycache__/*"]
#[exclude = "*/__pycache__/*"]
struct Scripts;

/// The executor image's definition: its digest names the image.
const DOCKERFILE: &str = include_str!("../../../Dockerfile");

/// What the workflow gives a group job: ten minutes for polling, capture and
/// packaging past the suite's deadline.
const GROUP_DEADLINES: [(&str, &str); 2] = [
    ("HARNESS_E2E_SUITE_DEADLINE_SECONDS", "10200"),
    ("HARNESS_E2E_RUN_TIMEOUT_SECONDS", "10800"),
];

/// How this worker runs Docker executions (worker configuration).
#[derive(Debug, Clone)]
pub(crate) struct DockerSettings {
    /// Groups running at once, across executions.
    pub parallel_groups: usize,
    /// An env file with the provider credentials the GitHub groups receive,
    /// passed to `prepare assemble` and every group with `--env-file`.
    pub provider_env_file: Option<PathBuf>,
    /// A checkout's `scripts/` to run instead of the embedded scripts, copied
    /// into each new execution.
    pub scripts_dir: Option<PathBuf>,
}

impl Default for DockerSettings {
    fn default() -> Self {
        Self {
            parallel_groups: 2,
            provider_env_file: None,
            scripts_dir: None,
        }
    }
}

/// One group of a Docker execution and where it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct DockerGroup {
    pub round: u32,
    /// Empty until the stack's runner materialized the suite.
    #[serde(default)]
    pub campaign_id: String,
    pub group_id: String,
    pub scenarios: Vec<String>,
    /// `queued`, `running`, `done`, `failed`, `cancelled` or `interrupted`.
    pub state: String,
    /// The attempt its latest run carries.
    pub attempt: u32,
    #[serde(default)]
    pub error: Option<String>,
}

/// One phase to run: `scripts/run_in_image.sh [--env-file FILE] ARGS...`.
pub(super) struct Phase {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub env_file: Option<PathBuf>,
    pub log: PathBuf,
}

/// Runs phases in the executor image; tests stand in for Docker.
#[async_trait]
pub(super) trait Launcher: Send + Sync {
    /// Run one phase of `execution` from `checkout`; `true` when it
    /// succeeded. Once `cancel` holds `true`, the execution's containers are
    /// stopped and the phase ends as they do.
    async fn run(
        &self,
        checkout: &Path,
        execution: &str,
        phase: Phase,
        cancel: watch::Receiver<bool>,
    ) -> Result<bool>;
    /// Remove every container of `execution`, running or not.
    async fn remove(&self, execution: &str);
}

/// The host side: bash runs the checkout's wrapper, Docker the rest.
struct ImageLauncher;

/// What a phase keeps of the worker's environment: where Docker and a
/// temporary directory are, nothing else. Credentials come from the env
/// file, the rest from the phase.
const HOST_ENVIRONMENT: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "TMPDIR",
    "XDG_RUNTIME_DIR",
    "DOCKER_HOST",
    "DOCKER_CONFIG",
    "DOCKER_CONTEXT",
];

#[async_trait]
impl Launcher for ImageLauncher {
    async fn run(
        &self,
        checkout: &Path,
        execution: &str,
        phase: Phase,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<bool> {
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&phase.log)
            .with_context(|| format!("open {}", phase.log.display()))?;
        let mut command = Command::new("bash");
        command.arg(checkout.join("scripts/run_in_image.sh"));
        if let Some(file) = &phase.env_file {
            command.arg("--env-file").arg(file);
        }
        command
            .args(&phase.args)
            .current_dir(checkout)
            .env_clear()
            .envs(
                HOST_ENVIRONMENT
                    .iter()
                    .filter_map(|name| std::env::var_os(name).map(|value| (*name, value))),
            )
            .envs(phase.env.iter().map(|(name, value)| (name, value)))
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .context("start scripts/run_in_image.sh with bash")?;
        let mut watching = true;
        let mut stopping = *cancel.borrow();
        loop {
            if stopping {
                // Again until the phase ends: a container may still be
                // starting when the first stop looks for it.
                stop(execution).await;
            }
            tokio::select! {
                status = child.wait() => return Ok(status?.success()),
                changed = cancel.changed(), if watching && !stopping => match changed {
                    Ok(()) => stopping = *cancel.borrow(),
                    // No one can cancel it any more.
                    Err(_) => watching = false,
                },
                () = tokio::time::sleep(Duration::from_secs(5)), if stopping => {}
            }
        }
    }

    async fn remove(&self, execution: &str) {
        let containers = containers(execution, "-aq").await;
        if !containers.is_empty() {
            let _ = Command::new("docker")
                .arg("rm")
                .arg("-f")
                .args(&containers)
                .output()
                .await;
        }
    }
}

/// The containers `docker ps <flags>` lists for an execution, by label.
async fn containers(execution: &str, flags: &str) -> Vec<String> {
    match Command::new("docker")
        .args([
            "ps",
            flags,
            "--filter",
            &format!("label=harness-e2e.execution={execution}"),
        ])
        .output()
        .await
    {
        Ok(output) => String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
        Err(error) => {
            tracing::warn!(%execution, %error, "cannot list the execution's containers");
            Vec::new()
        }
    }
}

/// Stop an execution's running containers; each launcher inside takes its
/// stack down before it exits.
async fn stop(execution: &str) {
    let containers = containers(execution, "-q").await;
    if !containers.is_empty() {
        let _ = Command::new("docker")
            .args(["stop", "--time", "30"])
            .args(&containers)
            .output()
            .await;
    }
}

/// The Docker side of the plan store: settings, the launcher, the group
/// slots shared by every execution, and how to cancel a running one.
pub(super) struct Docker {
    pub(super) settings: DockerSettings,
    launcher: Arc<dyn Launcher>,
    groups: Arc<Semaphore>,
    cancels: std::sync::Mutex<HashMap<String, watch::Sender<bool>>>,
}

impl Docker {
    pub(super) fn new(settings: DockerSettings) -> Self {
        Self::with_launcher(settings, Arc::new(ImageLauncher))
    }

    pub(super) fn with_launcher(settings: DockerSettings, launcher: Arc<dyn Launcher>) -> Self {
        Self {
            groups: Arc::new(Semaphore::new(settings.parallel_groups.max(1))),
            settings,
            launcher,
            cancels: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn watch(&self, id: &str) -> watch::Receiver<bool> {
        self.cancels
            .lock()
            .unwrap()
            .entry(id.to_owned())
            .or_insert_with(|| watch::channel(false).0)
            .subscribe()
    }

    pub(super) fn cancel(&self, id: &str) {
        if let Some(sender) = self.cancels.lock().unwrap().get(id) {
            sender.send_replace(true);
        }
    }

    fn forget(&self, id: &str) {
        self.cancels.lock().unwrap().remove(id);
    }
}

/// A receiver no one cancels: what finishes an execution always runs.
fn uncancellable() -> watch::Receiver<bool> {
    watch::channel(false).1
}

async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    if cancel.wait_for(|cancelled| *cancelled).await.is_err() {
        std::future::pending::<()>().await;
    }
}

impl PlanStore {
    fn docker_folder(&self, id: &str) -> PathBuf {
        self.root.join("docker-executions").join(id)
    }

    /// Where a Docker execution keeps its bundles.
    pub(super) fn docker_artifacts(&self, id: &str) -> PathBuf {
        self.docker_folder(id).join("checkout/target/artifacts")
    }

    /// Start an execution, validated and materialized here, in Docker: its
    /// folder is written and it runs in the background. The stack is sent as
    /// YAML; only YAML that is not a stack is refused.
    pub(super) async fn start_docker(
        self: &Arc<Self>,
        mut execution: PlanExecution,
        master: &MasterPlan,
    ) -> Result<PlanExecution> {
        let parameters = execution
            .parameters
            .as_mut()
            .context("a Docker execution needs its parameters")?;
        let stack = parameters
            .stack
            .as_mut()
            .context("Pick the stack the execution runs on in Docker.")?;
        stack.sha256 = artifact::sha256_bytes(stack.yaml.as_bytes());
        let parameters = &*parameters;
        let stack = parameters.stack.as_ref().context("stack")?;
        let summary = stacks::summarize(&stack.yaml)?;
        let inputs = json!({
            "suite": dispatch_suite(parameters, master)?,
            "stack": stack.yaml,
            "model": format!("{}/{}", parameters.provider, parameters.model),
            "profile": parameters.agent,
        });
        let mut warnings = summary
            .warnings
            .iter()
            .map(|warning| format!("Stack {}: {warning}", stack.name))
            .collect::<Vec<_>>();
        match &self.docker.settings.provider_env_file {
            None => warnings.push(
                "No provider_env_file is configured: the providers start without credentials."
                    .into(),
            ),
            Some(file) if !file.is_file() => warnings.push(format!(
                "provider_env_file {} does not exist: the providers start without credentials.",
                file.display()
            )),
            Some(_) => {}
        }
        execution.warnings.extend(warnings);
        let groups = placeholder_groups(&execution.slots);
        execution.slots = group_slots(&groups);
        execution.source = ExecutionSource::Docker {
            attempt: 1,
            phase: "prepare".into(),
            image: None,
            groups,
        };
        let folder = self.docker_folder(&execution.id);
        self.write_checkout(&folder).await?;
        artifact::write_atomic(
            &folder.join("inputs.json"),
            &serde_json::to_vec_pretty(&inputs)?,
        )?;
        self.write_execution(&execution).await?;
        self.spawn_docker(&execution.id, None);
        Ok(execution)
    }

    /// The checkout the wrapper mounts: the embedded Dockerfile, and the
    /// embedded scripts or a copy of `scripts_dir` as it is now.
    async fn write_checkout(&self, folder: &Path) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let checkout = folder.join("checkout");
        fs::create_dir_all(checkout.join("target/artifacts"))
            .with_context(|| format!("create {}", checkout.display()))?;
        fs::create_dir_all(folder.join("logs"))?;
        fs::write(checkout.join("Dockerfile"), DOCKERFILE)?;
        if let Some(scripts) = &self.docker.settings.scripts_dir {
            return copy_tree(scripts, &checkout.join("scripts")).await;
        }
        for name in Scripts::iter() {
            let file = Scripts::get(&name).context("embedded script")?;
            let path = checkout.join("scripts").join(name.as_ref());
            fs::create_dir_all(path.parent().context("script path")?)?;
            fs::write(&path, file.data)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    }

    pub(super) async fn remove_docker_folder(&self, id: &str) {
        let folder = self.docker_folder(id);
        if let Ok(Err(error)) =
            tokio::task::spawn_blocking(move || fs::remove_dir_all(folder)).await
        {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(execution_id = %id, %error, "cannot remove the Docker execution's folder");
            }
        }
    }

    /// Drive an execution in the background; whatever stops it ends it with
    /// the reason, keeping what finished.
    fn spawn_docker(self: &Arc<Self>, id: &str, stopped: Option<String>) {
        let store = self.clone();
        let id = id.to_owned();
        // Subscribed now, so a cancel that arrives before the drive starts counts.
        let cancel = self.docker.watch(&id);
        tokio::spawn(async move {
            if let Err(error) = store.drive_docker(&id, cancel, stopped).await {
                let reason = format!("{error:#}");
                tracing::warn!(execution_id = %id, error = %reason, "Docker execution stopped");
                if let Err(error) = store.end_docker(&id, Some(reason)).await {
                    tracing::error!(execution_id = %id, error = %format!("{error:#}"), "cannot record the end of a Docker execution");
                }
            }
            store.docker.forget(&id);
        });
    }

    async fn drive_docker(
        &self,
        id: &str,
        cancel: watch::Receiver<bool>,
        stopped: Option<String>,
    ) -> Result<()> {
        let checkout = self
            .docker_folder(id)
            .join("checkout")
            .canonicalize()
            .context("the execution's checkout is gone")?;
        if docker_phase(&self.read_execution(id).await?) == "prepare"
            && !self.prepare_docker(id, &checkout, cancel.clone()).await?
        {
            return self.end_docker(id, None).await;
        }
        self.run_docker_groups(id, &checkout, &cancel).await?;
        let finished = match &self.read_execution(id).await?.source {
            ExecutionSource::Docker { groups, .. } => groups
                .iter()
                .any(|group| matches!(group.state.as_str(), "done" | "failed" | "cancelled")),
            _ => false,
        };
        // Nothing ran: nothing to finalize or import.
        if !finished {
            return self.end_docker(id, stopped).await;
        }
        self.finalize_docker(id, &checkout).await?;
        self.import_docker(id, stopped).await
    }

    /// Materialize the suite and assemble the stack, then check out the
    /// groups' fixtures. `false` when it was cancelled.
    async fn prepare_docker(
        &self,
        id: &str,
        checkout: &Path,
        cancel: watch::Receiver<bool>,
    ) -> Result<bool> {
        let folder = self.docker_folder(id);
        let inputs = read_json(&folder.join("inputs.json"))?;
        let key = || vec![("EXECUTION_KEY".to_owned(), id.to_owned())];
        let mut dispatch = key();
        for name in ["suite", "stack", "model", "profile"] {
            dispatch.push((
                format!("DISPATCH_{}", name.to_uppercase()),
                inputs[name].as_str().unwrap_or_default().to_owned(),
            ));
        }
        // The worker's own token, if it has one, spares the API's anonymous limit.
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            dispatch.push(("GITHUB_TOKEN".into(), token));
        }
        for (step, env, env_file) in [
            ("materialize", dispatch, None),
            (
                "assemble",
                key(),
                self.docker.settings.provider_env_file.clone(),
            ),
        ] {
            let log = folder.join(format!("logs/prepare-{step}.log"));
            let phase = Phase {
                args: vec!["prepare".into(), step.into()],
                env,
                env_file: env_file.filter(|file| file.is_file()),
                log: log.clone(),
            };
            if !self
                .docker
                .launcher
                .run(checkout, id, phase, cancel.clone())
                .await?
            {
                if *cancel.borrow() {
                    return Ok(false);
                }
                bail!(
                    "Preparing the execution failed at `prepare {step}`; see {}",
                    log.display()
                );
            }
        }
        let (groups, image) = prepared_groups(checkout)?;
        copy_tree(
            &checkout.join("target/harness-e2e-contract"),
            &checkout.join(format!("target/artifacts/e2e-contract-{id}-gh-1")),
        )
        .await?;
        let mut env = key();
        if groups.iter().any(|group| private_fixtures(&group.group_id)) {
            if let Some(token) = self.github_token().await {
                env.push(("GITHUB_TOKEN".into(), token));
            }
        }
        let log = folder.join("logs/prepare-fixtures.log");
        let fixtures = Phase {
            args: vec!["prepare".into(), "fixtures".into()],
            env,
            env_file: None,
            log: log.clone(),
        };
        let fetched = self
            .docker
            .launcher
            .run(checkout, id, fixtures, cancel.clone())
            .await?;
        if !fetched && *cancel.borrow() {
            return Ok(false);
        }
        let _guard = self.lock.lock().await;
        let mut execution = self.read_execution(id).await?;
        if !fetched {
            execution.warnings.push(format!(
                "Checking out the groups' fixtures failed (see {}); a group that needs one says so.",
                log.display()
            ));
        }
        execution.slots = group_slots(&groups);
        if let ExecutionSource::Docker {
            phase,
            image: prepared,
            groups: current,
            ..
        } = &mut execution.source
        {
            *phase = "groups".into();
            *prepared = image;
            *current = groups;
        }
        execution.updated_at = now();
        self.write_execution(&execution).await?;
        Ok(true)
    }

    /// The worker's GitHub token, for the private fixture sources: its
    /// environment's, else the signed-in `gh`'s.
    async fn github_token(&self) -> Option<String> {
        if let Some(token) = std::env::var("GITHUB_TOKEN")
            .ok()
            .filter(|token| !token.is_empty())
        {
            return Some(token);
        }
        self.gh(self.github.api_timeout, &["auth", "token"])
            .await
            .ok()
            .map(|token| String::from_utf8_lossy(&token).trim().to_owned())
            .filter(|token| !token.is_empty())
    }

    /// Run every queued group, `parallel_groups` at a time across executions.
    async fn run_docker_groups(
        &self,
        id: &str,
        checkout: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<()> {
        let queued = match &self.read_execution(id).await?.source {
            ExecutionSource::Docker { groups, .. } => groups
                .iter()
                .enumerate()
                .filter(|(_, group)| group.state == "queued")
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        self.update_docker(id, |_, phase, _| *phase = "groups".into())
            .await?;
        // ponytail: Kanban groups share the fixture's node_modules install,
        // so they run one at a time; a checkout per group would lift this.
        let kanban = tokio::sync::Mutex::new(());
        let runs = queued
            .into_iter()
            .map(|index| self.run_docker_group(id, checkout, index, cancel.clone(), &kanban));
        futures_util::future::join_all(runs)
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        Ok(())
    }

    async fn run_docker_group(
        &self,
        id: &str,
        checkout: &Path,
        index: usize,
        mut cancel: watch::Receiver<bool>,
        kanban: &tokio::sync::Mutex<()>,
    ) -> Result<()> {
        let permit = tokio::select! {
            permit = self.docker.groups.clone().acquire_owned() => Some(permit?),
            () = cancelled(&mut cancel) => None,
        };
        let group = match &self.read_execution(id).await?.source {
            ExecutionSource::Docker { groups, .. } => groups[index].clone(),
            _ => bail!("execution {id} is not a Docker execution"),
        };
        let _kanban = if group.group_id.starts_with("case-kanban-") {
            Some(kanban.lock().await)
        } else {
            None
        };
        if permit.is_none() || *cancel.borrow() {
            return self.set_group(id, index, "cancelled", None).await;
        }
        self.set_group(id, index, "running", None).await?;
        let name = format!(
            "e2e-observation-{id}-{}-{}-gh-{}",
            group.campaign_id, group.group_id, group.attempt
        );
        let artifacts = checkout.join("target/artifacts").join(&name);
        if artifacts.exists() {
            fs::remove_dir_all(&artifacts)?;
        }
        let mut env = vec![
            ("EXECUTION_KEY".to_owned(), id.to_owned()),
            (
                "HARNESS_E2E_CONTRACT".into(),
                format!(
                    "target/harness-e2e-contract/contracts/{}.json",
                    group.campaign_id
                ),
            ),
            (
                "HARNESS_E2E_CAMPAIGN_GROUP_ID".into(),
                group.group_id.clone(),
            ),
            (
                "HARNESS_E2E_ARTIFACTS_DIR".into(),
                artifacts.to_string_lossy().into_owned(),
            ),
        ];
        env.extend(
            GROUP_DEADLINES
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
        );
        if registry(&group.group_id) {
            // The Registry fixture publishes the application it screenshots
            // on the host's loopback; its engine takes a port of its own there.
            let port = std::net::TcpListener::bind("127.0.0.1:0")?
                .local_addr()?
                .port();
            env.push(("HARNESS_E2E_DOCKER_NETWORK".into(), "host".into()));
            env.push(("HARNESS_E2E_ENGINE_PORT".into(), port.to_string()));
        }
        let folder = self.docker_folder(id);
        let log = folder.join(format!(
            "logs/group-{}-{}-{}.log",
            group.campaign_id, group.group_id, group.attempt
        ));
        let succeeded = self
            .docker
            .launcher
            .run(
                checkout,
                id,
                Phase {
                    args: vec!["group".into()],
                    env,
                    env_file: self
                        .docker
                        .settings
                        .provider_env_file
                        .clone()
                        .filter(|file| file.is_file()),
                    log: log.clone(),
                },
                cancel.clone(),
            )
            .await?;
        let workflow = json!({"runner": "harness-e2e console", "execution_id": id,
            "group_id": group.group_id, "attempt": group.attempt, "job": "group"});
        self.package(id, checkout, &workflow, &[&artifacts], "Group", &name)
            .await?;
        let (state, error) = if succeeded {
            ("done", None)
        } else if *cancel.borrow() {
            ("cancelled", None)
        } else {
            let failure = read_json(&artifacts.join("failure.json")).unwrap_or(Value::Null);
            (
                "failed",
                Some(failure["error"].as_str().map_or_else(
                    || format!("The group failed; see {}", log.display()),
                    str::to_owned,
                )),
            )
        };
        self.set_group(id, index, state, error).await
    }

    /// Hash each root into its bundle-manifest.json, as the workflow does
    /// before it uploads one. A root that holds something unsafe is replaced
    /// by the diagnostic alone.
    async fn package(
        &self,
        id: &str,
        checkout: &Path,
        workflow: &Value,
        roots: &[&Path],
        kind: &str,
        name: &str,
    ) -> Result<()> {
        let mut args = vec!["package".to_owned(), workflow.to_string()];
        args.extend(roots.iter().map(|root| root.to_string_lossy().into_owned()));
        let log = self
            .docker_folder(id)
            .join(format!("logs/package-{name}.log"));
        let packaged = self
            .docker
            .launcher
            .run(
                checkout,
                id,
                Phase {
                    args,
                    env: vec![("EXECUTION_KEY".into(), id.into())],
                    env_file: None,
                    log: log.clone(),
                },
                uncancellable(),
            )
            .await?;
        if !packaged {
            for root in roots {
                if root.exists() {
                    fs::remove_dir_all(root)?;
                }
                fs::create_dir_all(root)?;
                fs::write(
                    root.join("failure.json"),
                    json!({"phase": "artifact_packaging", "outcome": "infra_failed",
                        "error": format!("{kind} artifact validation failed; the unsafe tree was not kept. See {}.", log.display())})
                    .to_string(),
                )?;
            }
        }
        Ok(())
    }

    /// Lay the groups' last attempts out and aggregate them, then package the
    /// root bundle under the execution's attempt.
    async fn finalize_docker(&self, id: &str, checkout: &Path) -> Result<()> {
        let (attempt, groups) = self
            .update_docker(id, |_, phase, _| *phase = "finalize".into())
            .await?;
        let artifacts = checkout.join("target/artifacts");
        let names = directories(&artifacts)?
            .iter()
            .map(|path| super::github::file_name(path))
            .collect::<Vec<_>>();
        // Each group's last attempt, as the workflow selects a re-run job's.
        let mut selected = serde_json::Map::new();
        for group in &groups {
            let stem = format!(
                "e2e-observation-{id}-{}-{}",
                group.campaign_id, group.group_id
            );
            if let Some((name, _)) =
                highest_attempt(names.iter().map(String::as_str), |found| found == stem)
            {
                selected.insert(
                    format!("{} · {}", group.campaign_id, group.group_id),
                    json!({"name": name}),
                );
            }
        }
        fs::write(
            checkout.join("target/selected-group-artifacts.json"),
            serde_json::to_vec_pretty(&selected)?,
        )?;
        let log = self
            .docker_folder(id)
            .join(format!("logs/finalize-{attempt}.log"));
        let aggregated = self
            .docker
            .launcher
            .run(
                checkout,
                id,
                Phase {
                    args: vec!["finalize".into()],
                    env: vec![
                        ("EXECUTION_KEY".into(), id.into()),
                        (
                            "HARNESS_E2E_GROUP_ARTIFACTS".into(),
                            "target/artifacts".into(),
                        ),
                    ],
                    env_file: None,
                    log: log.clone(),
                },
                uncancellable(),
            )
            .await?;
        let campaigns = checkout.join("target/harness-e2e-campaign");
        let root = artifacts.join(format!("e2e-observation-{id}-gh-{attempt}"));
        if root.exists() {
            fs::remove_dir_all(&root)?;
        }
        let roots = directories(&campaigns).unwrap_or_default();
        if roots.is_empty() {
            fs::create_dir_all(&root)?;
            fs::write(
                root.join("failure.json"),
                json!({"phase": "finalize", "outcome": "infra_failed",
                    "error": format!("The finalizer laid out no campaign; see {}.", log.display())})
                .to_string(),
            )?;
            return Ok(());
        }
        let workflow = json!({"runner": "harness-e2e console", "execution_id": id,
            "attempt": attempt, "job": "finalize"});
        let roots_ref = roots.iter().map(PathBuf::as_path).collect::<Vec<_>>();
        self.package(
            id,
            checkout,
            &workflow,
            &roots_ref,
            "Root",
            &format!("root-{attempt}"),
        )
        .await?;
        fs::rename(&campaigns, &root).context("keep the root bundle")?;
        if !aggregated {
            let _guard = self.lock.lock().await;
            let mut execution = self.read_execution(id).await?;
            let note = format!(
                "The finalizer did not aggregate attempt {attempt} (see {}); its groups were imported without the campaign summary.",
                log.display()
            );
            if !execution.warnings.contains(&note) {
                execution.warnings.push(note);
            }
            self.write_execution(&execution).await?;
        }
        Ok(())
    }

    /// Import the folder as a GitHub run is imported; `stopped` says why the
    /// execution did not run to its end.
    async fn import_docker(&self, id: &str, stopped: Option<String>) -> Result<()> {
        self.update_docker(id, |_, phase, _| *phase = "import".into())
            .await?;
        match self.download_and_install(id, stopped).await {
            Ok(()) => Ok(()),
            Err(error) => {
                self.end_docker(id, Some(format!("Importing its results failed: {error:#}")))
                    .await
            }
        }
    }

    /// End an execution that will import nothing more: what never ran is
    /// said so, and `reason` is why it stopped.
    async fn end_docker(&self, id: &str, reason: Option<String>) -> Result<()> {
        let _guard = self.lock.lock().await;
        let mut execution = self.read_execution(id).await?;
        let cancelled = execution.cancel_requested;
        if let ExecutionSource::Docker { phase, groups, .. } = &mut execution.source {
            for group in groups
                .iter_mut()
                .filter(|group| matches!(group.state.as_str(), "queued" | "running"))
            {
                group.state = if cancelled {
                    "cancelled"
                } else {
                    "interrupted"
                }
                .into();
            }
            *phase = "done".into();
            if placeholders(&execution.slots) {
                execution.slots = group_slots(groups);
            }
        }
        finish(&mut execution, reason, &self.root)?;
        self.write_execution(&execution).await
    }

    /// Change a Docker execution's attempt, phase or groups under the lock;
    /// answers with its attempt and groups.
    async fn update_docker(
        &self,
        id: &str,
        change: impl FnOnce(&mut u32, &mut String, &mut Vec<DockerGroup>),
    ) -> Result<(u32, Vec<DockerGroup>)> {
        let _guard = self.lock.lock().await;
        let mut execution = self.read_execution(id).await?;
        let ExecutionSource::Docker {
            attempt,
            phase,
            groups,
            ..
        } = &mut execution.source
        else {
            bail!("execution {id} is not a Docker execution");
        };
        change(attempt, phase, groups);
        let answer = (*attempt, groups.clone());
        if placeholders(&execution.slots) {
            execution.slots = group_slots(&answer.1);
        }
        execution.updated_at = now();
        self.write_execution(&execution).await?;
        Ok(answer)
    }

    async fn set_group(
        &self,
        id: &str,
        index: usize,
        state: &str,
        error: Option<String>,
    ) -> Result<()> {
        self.update_docker(id, |_, _, groups| {
            groups[index].state = state.into();
            groups[index].error = error;
        })
        .await
        .map(drop)
    }

    /// Run the groups of a scenario of a finished Docker execution again, in
    /// every round, as the execution's next attempt: then finalize and import
    /// again. The last attempt counts.
    pub(super) async fn rerun_docker(
        self: &Arc<Self>,
        id: &str,
        scenario_id: &str,
    ) -> Result<PlanExecution> {
        let _guard = self.lock.lock().await;
        let mut execution = self.read_execution(id).await?;
        ensure!(
            !execution.active() && execution.state != "importing",
            "Only a finished execution can run a scenario again."
        );
        let ExecutionSource::Docker {
            attempt,
            phase,
            groups,
            ..
        } = &mut execution.source
        else {
            bail!("execution {id} is not a Docker execution");
        };
        ensure!(
            groups.iter().all(|group| !group.campaign_id.is_empty()),
            "This execution stopped before its stack was prepared; Run again starts it anew."
        );
        let matched = groups
            .iter()
            .enumerate()
            .filter(|(_, group)| group.scenarios.iter().any(|id| id == scenario_id))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        ensure!(
            !matched.is_empty(),
            "This execution did not run the scenario '{scenario_id}'."
        );
        *attempt += 1;
        let mut scenarios = Vec::<String>::new();
        for index in matched {
            let group = &mut groups[index];
            (group.state, group.attempt, group.error) = ("queued".into(), *attempt, None);
            for scenario in &group.scenarios {
                if !scenarios.contains(scenario) {
                    scenarios.push(scenario.clone());
                }
            }
        }
        *phase = "groups".into();
        if scenarios.len() > 1 {
            let note = format!(
                "{} run only together, in this order; running one again runs the whole group.",
                scenarios.join(" then ")
            );
            if !execution.warnings.contains(&note) {
                execution.warnings.push(note);
            }
        }
        execution.rerun = Some(Rerun {
            scenarios,
            runs: Vec::new(),
            started_at: now(),
            state: std::mem::replace(&mut execution.state, "running".into()),
            error: execution.error.take(),
            finished_at: execution.finished_at.take(),
        });
        execution.cancel_requested = false;
        execution.updated_at = now();
        self.write_execution(&execution).await?;
        self.spawn_docker(id, None);
        Ok(execution)
    }

    /// The worker restarted during this execution: its containers go, and
    /// what finished is finalized and imported; the rest is interrupted.
    pub(super) async fn restart_docker(
        self: &Arc<Self>,
        mut execution: PlanExecution,
    ) -> Result<()> {
        self.docker.launcher.remove(&execution.id).await;
        let reason = "The worker restarted during this execution; its containers were removed and what did not finish was interrupted. Run a scenario again to run it.".to_owned();
        let ExecutionSource::Docker { phase, groups, .. } = &mut execution.source else {
            return Ok(());
        };
        for group in groups
            .iter_mut()
            .filter(|group| matches!(group.state.as_str(), "queued" | "running"))
        {
            group.state = "interrupted".into();
        }
        let finished = groups.iter().any(|group| {
            !group.campaign_id.is_empty()
                && matches!(group.state.as_str(), "done" | "failed" | "cancelled")
        });
        if !finished {
            self.write_execution(&execution).await?;
            return self.end_docker(&execution.id, Some(reason)).await;
        }
        *phase = "finalize".into();
        self.write_execution(&execution).await?;
        self.spawn_docker(&execution.id, Some(reason));
        Ok(())
    }
}

fn docker_phase(execution: &PlanExecution) -> &str {
    match &execution.source {
        ExecutionSource::Docker { phase, .. } => phase,
        _ => "",
    }
}

/// What `prepare` sends as `DISPATCH_SUITE`: a suite of the master plan by
/// its id when it runs as reviewed, so its digest is the workflow's for that
/// suite; any other suite whole, as JSON, with every scenario given.
fn dispatch_suite(parameters: &ExecutionParameters, master: &MasterPlan) -> Result<String> {
    let suite = parameters.suite.as_ref();
    let scenarios = &parameters.scenarios;
    if let Some(id) = suite.and_then(|suite| suite.id.as_ref()) {
        if master.suites.iter().any(|reviewed| &reviewed.id == id) {
            let snapshot = master.materialize(id)?;
            if canonical(&snapshot.scenario_ids) == canonical(scenarios)
                && snapshot.profile.repetitions == parameters.runs
                && snapshot.profile.technical_retries == parameters.technical_retries
            {
                return Ok(id.clone());
            }
        }
    }
    Ok(serde_json::to_string(&test_plan::Suite {
        id: suite
            .and_then(|suite| suite.id.clone())
            .unwrap_or_else(|| "unnamed".into()),
        label: suite.map(|suite| suite.label.clone()).unwrap_or_default(),
        purpose: String::new(),
        metrics: Vec::new(),
        modules: Vec::new(),
        scenarios: scenarios.clone(),
        scenario_groups: sequential_groups(master)
            .into_iter()
            .filter(|group| group.iter().all(|id| scenarios.contains(id)))
            .collect(),
        repetitions: parameters.runs,
        technical_retries: parameters.technical_retries,
        lane: String::new(),
    })?)
}

/// The groups as this worker materialized the suite, until the stack's
/// runner does.
fn placeholder_groups(slots: &[Slot]) -> Vec<DockerGroup> {
    let mut groups: Vec<DockerGroup> = Vec::new();
    for slot in slots {
        match groups
            .iter_mut()
            .find(|group| group.round == slot.round && group.group_id == slot.group_id)
        {
            Some(group) => group.scenarios.push(slot.scenario_id.clone()),
            None => groups.push(DockerGroup {
                round: slot.round,
                campaign_id: String::new(),
                group_id: slot.group_id.clone(),
                scenarios: vec![slot.scenario_id.clone()],
                state: "queued".into(),
                attempt: 1,
                error: None,
            }),
        }
    }
    groups
}

/// The groups the stack's runner materialized, from the contracts `prepare`
/// wrote, with the executor image they were prepared in.
fn prepared_groups(checkout: &Path) -> Result<(Vec<DockerGroup>, Option<String>)> {
    let contracts = checkout.join("target/harness-e2e-contract/contracts");
    let resolution = read_json(&contracts.join("resolution.json"))?;
    let mut campaigns = resolution["campaign_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|id| id.as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    // Rounds in the order an import reads the campaigns.
    campaigns.sort();
    let mut groups = Vec::new();
    for entry in resolution["matrix"]["include"]
        .as_array()
        .context("resolution.json has no group matrix")?
    {
        let campaign = entry["campaign_id"]
            .as_str()
            .context("a matrix entry names no campaign")?;
        let group_id = entry["group_id"]
            .as_str()
            .context("a matrix entry names no group")?;
        let contract = read_json(&contracts.join(format!("{campaign}.json")))?;
        let scenarios = contract["suite"]["groups"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|group| group["id"] == group_id)
            .and_then(|group| group["scenarios"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect();
        groups.push(DockerGroup {
            round: campaigns.iter().position(|id| id == campaign).unwrap_or(0) as u32 + 1,
            campaign_id: campaign.to_owned(),
            group_id: group_id.to_owned(),
            scenarios,
            state: "queued".into(),
            attempt: 1,
            error: None,
        });
    }
    let image = resolution["executor_image"].as_str().map(str::to_owned);
    Ok((groups, image))
}

/// Slots before anything is imported: one per scenario of each group, in
/// its group's state, without a native run.
fn group_slots(groups: &[DockerGroup]) -> Vec<Slot> {
    groups
        .iter()
        .flat_map(|group| {
            group.scenarios.iter().map(|scenario| {
                let mut slot = super::github::slot(group.round, &group.group_id, scenario);
                slot.state = match group.state.as_str() {
                    "queued" => "pending",
                    "running" => "running",
                    "done" | "failed" => "finished",
                    _ => "not_run",
                }
                .into();
                slot.error.clone_from(&group.error);
                slot
            })
        })
        .collect()
}

/// Nothing was imported yet: the slots stand for the groups.
fn placeholders(slots: &[Slot]) -> bool {
    slots.iter().all(|slot| slot.execution_id.is_empty())
}

fn registry(group_id: &str) -> bool {
    group_id.starts_with("case-registry-")
}

/// Groups whose fixtures are private repositories.
fn private_fixtures(group_id: &str) -> bool {
    registry(group_id) || group_id == "case-trending-topics-build"
}

/// Copy a directory tree, as `cp -a` does, to `destination`, which must not
/// exist yet.
pub(super) async fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let output = Command::new("cp")
        .arg("-a")
        .arg(source)
        .arg(destination)
        .output()
        .await
        .context("run cp")?;
    ensure!(
        output.status.success(),
        "copy {} to {}: {}",
        source.display(),
        destination.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use tokio::sync::Mutex;

    use super::super::tests::{exact_stack_bundle, suite_parameters, FakeRunner};
    use super::super::{github, ExecutionStack, Runner, Where};
    use super::*;
    use crate::control::{execution_id_for_key, RunRequest};

    /// One phase as the fake Docker saw it.
    #[derive(Debug, Clone)]
    struct Call {
        args: Vec<String>,
        env: HashMap<String, String>,
        env_file: Option<PathBuf>,
    }

    /// Stands in for Docker: each phase leaves what the real one leaves.
    #[derive(Default)]
    struct FakeLauncher {
        calls: std::sync::Mutex<Vec<Call>>,
        running: AtomicUsize,
        most: AtomicUsize,
        /// Groups that run until they are cancelled.
        hold: std::sync::Mutex<BTreeSet<String>>,
        removed: std::sync::Mutex<Vec<String>>,
    }

    impl FakeLauncher {
        fn calls(&self, phase: &str) -> Vec<Call> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| call.args[0] == phase)
                .cloned()
                .collect()
        }
    }

    #[async_trait]
    impl Launcher for FakeLauncher {
        async fn run(
            &self,
            checkout: &Path,
            execution: &str,
            phase: Phase,
            mut cancel: watch::Receiver<bool>,
        ) -> Result<bool> {
            let env = phase.env.iter().cloned().collect::<HashMap<_, _>>();
            self.calls.lock().unwrap().push(Call {
                args: phase.args.clone(),
                env: env.clone(),
                env_file: phase.env_file.clone(),
            });
            fs::write(&phase.log, phase.args.join(" "))?;
            let contracts = checkout.join("target/harness-e2e-contract");
            match phase.args[1..]
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .as_slice()
            {
                ["materialize"] => {
                    fs::create_dir_all(&contracts)?;
                    let master = test_plan::embedded()?;
                    let suite = &env["DISPATCH_SUITE"];
                    let snapshot = if suite.starts_with('{') {
                        master.materialize_suite(serde_json::from_str(suite)?)?
                    } else {
                        master.materialize(suite)?
                    };
                    fs::write(contracts.join("suite.json"), serde_json::to_vec(&snapshot)?)?;
                    fs::write(
                        contracts.join("execution.json"),
                        json!({"suite": suite, "stack": "inline", "model": env["DISPATCH_MODEL"]})
                            .to_string(),
                    )?;
                    fs::write(contracts.join("stack.yaml"), &env["DISPATCH_STACK"])?;
                }
                ["assemble"] => {
                    let snapshot = read_json(&contracts.join("suite.json"))?;
                    write_contracts(&contracts, &snapshot)?;
                    let requested = fs::read_to_string(contracts.join("stack.yaml"))?;
                    fs::write(
                        contracts.join("stack.yaml"),
                        format!("# assembled\n{requested}"),
                    )?;
                }
                ["fixtures"] => {}
                [] if phase.args[0] == "group" => {
                    let group = env["HARNESS_E2E_CAMPAIGN_GROUP_ID"].clone();
                    let running = self.running.fetch_add(1, Ordering::SeqCst) + 1;
                    self.most.fetch_max(running, Ordering::SeqCst);
                    let artifacts = PathBuf::from(&env["HARNESS_E2E_ARTIFACTS_DIR"]);
                    let held = self.hold.lock().unwrap().contains(&group);
                    let succeeded = if held {
                        cancelled(&mut cancel).await;
                        fs::create_dir_all(&artifacts)?;
                        fs::write(
                            artifacts.join("failure.json"),
                            json!({"phase": "execution", "error": "stopped"}).to_string(),
                        )?;
                        false
                    } else {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        let contract = checkout.join(&env["HARNESS_E2E_CONTRACT"]);
                        write_group_bundle(&artifacts, &contract, execution, &group)?;
                        true
                    };
                    self.running.fetch_sub(1, Ordering::SeqCst);
                    return Ok(succeeded);
                }
                [_, roots @ ..] if phase.args[0] == "package" => {
                    for root in roots {
                        fs::write(Path::new(root).join("bundle-manifest.json"), "{}")?;
                    }
                }
                [] if phase.args[0] == "finalize" => {
                    restore(checkout, &env["HARNESS_E2E_GROUP_ARTIFACTS"]).await?;
                }
                other => bail!("unexpected phase {other:?}"),
            }
            Ok(true)
        }

        async fn remove(&self, execution: &str) {
            self.removed.lock().unwrap().push(execution.into());
        }
    }

    /// The contracts `prepare assemble` writes for a materialized suite.
    fn write_contracts(contracts: &Path, snapshot: &Value) -> Result<()> {
        let campaigns = snapshot["campaigns"].as_array().context("campaigns")?;
        let mut include = Vec::new();
        for campaign in campaigns {
            let id = campaign["campaign_id"].as_str().context("campaign id")?;
            fs::create_dir_all(contracts.join("contracts"))?;
            fs::write(
                contracts.join(format!("contracts/{id}.json")),
                json!({"suite": {"groups": campaign["groups"]}}).to_string(),
            )?;
            for group in campaign["groups"].as_array().context("groups")? {
                include.push(json!({"campaign_id": id, "group_id": group["id"]}));
            }
        }
        fs::write(
            contracts.join("contracts/resolution.json"),
            json!({"campaign_ids": campaigns.iter().map(|c| &c["campaign_id"]).collect::<Vec<_>>(),
                "matrix": {"include": include},
                "executor_image": "ghcr.io/iii-hq/harness-e2e@sha256:dd"})
            .to_string(),
        )?;
        fs::write(contracts.join("worker-compose.lock"), "containers: {}\n")?;
        Ok(())
    }

    /// The native run one group leaves, keyed by its execution, group and
    /// attempt, beside the contract it ran.
    fn write_group_bundle(
        artifacts: &Path,
        contract: &Path,
        execution: &str,
        group: &str,
    ) -> Result<String> {
        let attempt = artifacts
            .to_string_lossy()
            .rsplit_once("-gh-")
            .map(|(_, attempt)| attempt.to_owned())
            .context("attempt")?;
        let scenarios = read_json(contract)?["suite"]["groups"]
            .as_array()
            .context("groups")?
            .iter()
            .find(|entry| entry["id"] == group)
            .context("group")?["scenarios"]
            .clone();
        let request: RunRequest = serde_json::from_value(json!({
            "idempotency_key": format!("{execution}:{group}:{attempt}"), "label": group,
            "lane": "local", "model": "model", "provider": "provider",
            "scenarios": scenarios, "runs": 1, "technical_retries": 0,
        }))?;
        fs::create_dir_all(artifacts.join("stack"))?;
        let native = FakeRunner::new(artifacts.join("native")).native_record(request.clone())?;
        fs::write(
            artifacts.join("run-request.json"),
            serde_json::to_vec(&request)?,
        )?;
        fs::copy(contract, artifacts.join("stack-lock.json"))?;
        Ok(native.execution_id)
    }

    /// What `finalize restore` lays out: each campaign's selected groups.
    async fn restore(checkout: &Path, bundles: &str) -> Result<()> {
        let selected = read_json(&checkout.join("target/selected-group-artifacts.json"))?;
        let contracts = checkout.join("target/harness-e2e-contract/contracts");
        let campaigns = checkout.join("target/harness-e2e-campaign");
        for contract in directories(&contracts)
            .unwrap_or_default()
            .into_iter()
            .chain(
                fs::read_dir(&contracts)?
                    .map(|entry| entry.map(|entry| entry.path()))
                    .collect::<std::io::Result<Vec<_>>>()?,
            )
        {
            let name = github::file_name(&contract);
            let Some(campaign) = name.strip_suffix(".json").filter(|n| *n != "resolution") else {
                continue;
            };
            let root = campaigns.join(campaign);
            fs::create_dir_all(root.join("groups"))?;
            fs::copy(&contract, root.join("stack-lock.json"))?;
            for group in read_json(&contract)?["suite"]["groups"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let group = group["id"].as_str().context("group id")?;
                let destination = root.join("groups").join(group);
                match selected[format!("{campaign} · {group}")]["name"].as_str() {
                    Some(name) => {
                        copy_tree(&checkout.join(bundles).join(name), &destination).await?
                    }
                    None => {
                        fs::create_dir_all(&destination)?;
                        fs::write(
                            destination.join("failure.json"),
                            json!({"error": "group observation artifact was not available"})
                                .to_string(),
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    fn docker_store(
        root: &Path,
        runner: Arc<FakeRunner>,
        launcher: Arc<FakeLauncher>,
        settings: DockerSettings,
    ) -> Arc<PlanStore> {
        for directory in ["suites", "stacks", "executions"] {
            fs::create_dir_all(root.join("plan-store").join(directory)).unwrap();
        }
        Arc::new(PlanStore {
            root: root.into(),
            persistence: None,
            runner: Some(runner),
            github: github::GithubCli::default(),
            docker: Docker::with_launcher(settings, launcher),
            changes: tokio::sync::broadcast::channel(64).0,
            lock: Mutex::new(()),
            attempts: AtomicU64::new(0),
        })
    }

    /// The repository's `default` stack, to run `pr` on in Docker.
    fn docker_parameters(suite: &str) -> ExecutionParameters {
        ExecutionParameters {
            r#where: Where::Docker,
            stack: Some(ExecutionStack {
                name: "default".into(),
                yaml: stacks::REPOSITORY[0].1.into(),
                sha256: "sha256:ignored".into(),
            }),
            ..suite_parameters(suite)
        }
    }

    /// Wait until `done` holds for the execution.
    async fn until(
        store: &PlanStore,
        id: &str,
        done: impl Fn(&PlanExecution) -> bool,
    ) -> PlanExecution {
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let execution = store.read_execution(id).await.unwrap();
                if done(&execution) {
                    return execution;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the Docker execution did not get there")
    }

    fn settled(execution: &PlanExecution) -> bool {
        !execution.active() && docker_phase(execution) == "done"
    }

    fn groups(execution: &PlanExecution) -> Vec<(String, String, u32)> {
        match &execution.source {
            ExecutionSource::Docker { groups, .. } => groups
                .iter()
                .map(|group| (group.group_id.clone(), group.state.clone(), group.attempt))
                .collect(),
            other => panic!("not a Docker execution: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_docker_execution_prepares_runs_its_groups_two_at_a_time_and_imports_its_folder() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let runner = Arc::new(FakeRunner::new(data.clone()));
        let launcher = Arc::new(FakeLauncher::default());
        let providers = root.path().join("providers.env");
        fs::write(&providers, "DEEPSEEK_API_KEY=secret\n").unwrap();
        let store = docker_store(
            &data,
            runner.clone(),
            launcher.clone(),
            DockerSettings {
                provider_env_file: Some(providers.clone()),
                ..DockerSettings::default()
            },
        );
        let started = store
            .start_execution(docker_parameters("pr"), "In Docker")
            .await
            .unwrap();
        let id = started.id.clone();
        // At once: its groups as this worker materializes the suite, queued.
        let placeholders = groups(&started);
        assert_eq!(placeholders.len(), 4);
        assert!(placeholders.iter().all(|(_, state, _)| state == "queued"));
        assert!(started
            .slots
            .iter()
            .all(|slot| slot.state == "pending" && slot.execution_id.is_empty()));
        let folder = data.join("docker-executions").join(&id);
        let inputs = read_json(&folder.join("inputs.json")).unwrap();
        assert_eq!(inputs["suite"], "pr");
        assert_eq!(inputs["model"], "provider/model");
        assert_eq!(inputs["stack"], stacks::REPOSITORY[0].1);
        assert!(!fs::read_to_string(folder.join("inputs.json"))
            .unwrap()
            .contains("secret"));
        assert_eq!(
            fs::read_to_string(folder.join("checkout/Dockerfile")).unwrap(),
            DOCKERFILE
        );
        for script in ["executor.sh", "run_in_image.sh", "kanban_eval/bootstrap.py"] {
            let mode = fs::metadata(folder.join("checkout/scripts").join(script))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "{script}");
        }

        let done = until(&store, &id, settled).await;
        assert_eq!(done.state, "completed", "{:?}", done.error);
        let ExecutionSource::Docker {
            attempt,
            image,
            groups: ran,
            ..
        } = &done.source
        else {
            panic!("{:?}", done.source);
        };
        assert_eq!(*attempt, 1);
        assert_eq!(
            image.as_deref(),
            Some("ghcr.io/iii-hq/harness-e2e@sha256:dd")
        );
        assert!(ran
            .iter()
            .all(|group| group.state == "done" && group.campaign_id == "pr-r01"));
        // Imported as a GitHub run is: every slot an ordinary retained run.
        assert_eq!(done.slots.len(), 4);
        for slot in &done.slots {
            assert_eq!(slot.state, "finished");
            assert!(runner.record(&slot.execution_id).await.is_some());
        }
        assert!(done.measurements.is_some());
        let parameters = done.parameters.as_ref().unwrap();
        assert_eq!(parameters.r#where, Where::Docker);
        assert_eq!(parameters.suite.as_ref().unwrap().id.as_deref(), Some("pr"));
        assert_eq!(
            parameters.suite.as_ref().unwrap().sha256,
            test_plan::embedded()
                .unwrap()
                .materialize("pr")
                .unwrap()
                .profile_sha256
        );
        // The stack as its contract recorded it, named as it was picked.
        let stack = parameters.stack.as_ref().unwrap();
        let recorded = format!("# assembled\n{}", stacks::REPOSITORY[0].1);
        assert_eq!(
            (stack.name.as_str(), stack.yaml.as_str()),
            ("default", recorded.as_str())
        );
        assert_eq!(stack.sha256, artifact::sha256_bytes(recorded.as_bytes()));
        assert_eq!(done.label.as_deref(), Some("In Docker"));

        // Two groups at a time, each with the provider credentials by file.
        assert_eq!(launcher.most.load(Ordering::SeqCst), 2);
        let phases = launcher
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| match call.args[0].as_str() {
                "prepare" => call.args.join(" "),
                phase => phase.to_owned(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            &phases[..3],
            [
                "prepare materialize",
                "prepare assemble",
                "prepare fixtures"
            ]
        );
        // Each group packaged after it ran, then the root after the finalizer.
        assert_eq!(phases[phases.len() - 2..], ["finalize", "package"]);
        assert_eq!(phases.iter().filter(|phase| *phase == "package").count(), 5);
        let root_package = launcher.calls("package").pop().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&root_package.args[1]).unwrap(),
            json!({"runner": "harness-e2e console", "execution_id": id, "attempt": 1, "job": "finalize"})
        );
        for call in launcher.calls.lock().unwrap().iter() {
            let credentials = matches!(call.args[0].as_str(), "group")
                || call.args[..] == ["prepare", "assemble"];
            assert_eq!(call.env_file.is_some(), credentials, "{:?}", call.args);
            assert_eq!(call.env["EXECUTION_KEY"], id);
        }
        let group = &launcher.calls("group")[0];
        assert_eq!(
            group.env["HARNESS_E2E_CONTRACT"],
            "target/harness-e2e-contract/contracts/pr-r01.json"
        );
        assert_eq!(group.env["HARNESS_E2E_SUITE_DEADLINE_SECONDS"], "10200");
        assert!(!group.env.contains_key("HARNESS_E2E_DOCKER_NETWORK"));
        // Every artifact under the name the workflow gives it.
        let mut names = directories(&store.docker_artifacts(&id))
            .unwrap()
            .iter()
            .map(|path| github::file_name(path))
            .collect::<Vec<_>>();
        names.sort();
        let mut expected = vec![
            format!("e2e-contract-{id}-gh-1"),
            format!("e2e-observation-{id}-gh-1"),
        ];
        for group in [
            "case-minimal-path",
            "case-persistent-state",
            "case-shell-coder-sandbox",
            "case-tool-contract-recovery",
        ] {
            expected.push(format!("e2e-observation-{id}-pr-r01-{group}-gh-1"));
        }
        expected.sort();
        assert_eq!(names, expected);

        // Running a scenario again runs its group as attempt 2, finalizes
        // and imports again: the last attempt counts.
        let before = done.slots.clone();
        let rerun = store.rerun_scenario(&id, "minimal_path").await.unwrap();
        assert_eq!(rerun.state, "running");
        assert_eq!(
            rerun.rerun.as_ref().unwrap().scenarios,
            vec!["minimal_path".to_owned()]
        );
        let again = until(&store, &id, |execution| {
            settled(execution) && execution.rerun.is_none()
        })
        .await;
        assert_eq!(again.state, "completed", "{:?}", again.error);
        assert!(matches!(
            again.source,
            ExecutionSource::Docker { attempt: 2, .. }
        ));
        let reran = launcher.calls("group");
        assert_eq!(reran.len(), 5);
        assert!(
            reran[4].env["HARNESS_E2E_ARTIFACTS_DIR"].ends_with("pr-r01-case-minimal-path-gh-2")
        );
        assert!(store
            .docker_artifacts(&id)
            .join(format!("e2e-observation-{id}-gh-2"))
            .is_dir());
        for (old, new) in before.iter().zip(&again.slots) {
            if new.scenario_id == "minimal_path" {
                assert_eq!(
                    new.execution_id,
                    execution_id_for_key(&format!("{id}:case-minimal-path:2"))
                );
                assert!(runner.record(&old.execution_id).await.is_none());
            } else {
                assert_eq!(new.execution_id, old.execution_id);
            }
        }
        assert_eq!(
            groups(&again)
                .iter()
                .map(|(group, _, attempt)| (group.as_str(), *attempt))
                .collect::<Vec<_>>(),
            vec![
                ("case-minimal-path", 2),
                ("case-persistent-state", 1),
                ("case-tool-contract-recovery", 1),
                ("case-shell-coder-sandbox", 1),
            ]
        );
    }

    #[tokio::test]
    async fn cancelling_stops_the_running_group_and_keeps_what_finished() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let launcher = Arc::new(FakeLauncher::default());
        launcher
            .hold
            .lock()
            .unwrap()
            .insert("case-persistent-state".into());
        let store = docker_store(
            &data,
            Arc::new(FakeRunner::new(data.clone())),
            launcher.clone(),
            DockerSettings {
                parallel_groups: 1,
                ..DockerSettings::default()
            },
        );
        let id = store
            .start_execution(docker_parameters("pr"), "")
            .await
            .unwrap()
            .id;
        let running = until(&store, &id, |execution| {
            groups(execution)
                .iter()
                .any(|(group, state, _)| group == "case-persistent-state" && state == "running")
        })
        .await;
        // While it runs, its slots follow its groups.
        assert_eq!(running.slots[0].state, "finished");
        assert_eq!(running.slots[1].state, "running");
        store.cancel(&id).await.unwrap();
        let cancelled = until(&store, &id, settled).await;
        assert_eq!(cancelled.state, "cancelled");
        assert_eq!(
            groups(&cancelled)
                .into_iter()
                .map(|(_, state, _)| state)
                .collect::<Vec<_>>(),
            ["done", "cancelled", "cancelled", "cancelled"]
        );
        // The two queued groups never started.
        assert_eq!(launcher.calls("group").len(), 2);
        let [finished, stopped, never, _] = cancelled.slots.as_slice() else {
            panic!("{:?}", cancelled.slots);
        };
        assert_eq!(finished.state, "finished");
        assert!(!finished.execution_id.is_empty());
        assert_eq!(stopped.error.as_deref(), Some("stopped"));
        assert_eq!(
            never.error.as_deref(),
            Some("group observation artifact was not available")
        );
    }

    #[tokio::test]
    async fn a_worker_restart_removes_the_containers_and_imports_what_finished() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let runner = Arc::new(FakeRunner::new(data.clone()));
        let launcher = Arc::new(FakeLauncher::default());
        let store = docker_store(
            &data,
            runner.clone(),
            launcher.clone(),
            DockerSettings::default(),
        );
        // As the stopped worker left it: prepared, one group done, one running.
        let id = "plan-restarted".to_owned();
        let folder = store.docker_folder(&id);
        let contracts = folder.join("checkout/target/harness-e2e-contract");
        let snapshot =
            serde_json::to_value(test_plan::embedded().unwrap().materialize("pr").unwrap())
                .unwrap();
        write_contracts(&contracts, &snapshot).unwrap();
        fs::write(
            contracts.join("stack.yaml"),
            "iii: 0.24.2\ncontainers: {}\n",
        )
        .unwrap();
        copy_tree(
            &contracts,
            &store
                .docker_artifacts(&id)
                .join(format!("e2e-contract-{id}-gh-1")),
        )
        .await
        .unwrap();
        fs::create_dir_all(folder.join("logs")).unwrap();
        let native = write_group_bundle(
            &store.docker_artifacts(&id).join(format!(
                "e2e-observation-{id}-pr-r01-case-minimal-path-gh-1"
            )),
            &contracts.join("contracts/pr-r01.json"),
            &id,
            "case-minimal-path",
        )
        .unwrap();
        let (mut prepared, _) = prepared_groups(&folder.join("checkout")).unwrap();
        prepared[0].state = "done".into();
        prepared[1].state = "running".into();
        let mut execution = PlanExecution {
            id: id.clone(),
            idempotency_key: "execution:restarted".into(),
            label: None,
            parameters: Some(docker_parameters("pr")),
            slots: group_slots(&prepared),
            source: ExecutionSource::Docker {
                attempt: 1,
                phase: "groups".into(),
                image: None,
                groups: prepared,
            },
            stack: Vec::new(),
            warnings: Vec::new(),
            state: "running".into(),
            started_at: now(),
            updated_at: now(),
            finished_at: None,
            cancel_requested: false,
            error: None,
            measurements: None,
            system_under_test: None,
            rerun: None,
        };
        store.write_execution(&execution).await.unwrap();
        // One that stopped while it prepared its stack.
        execution.id = "plan-preparing".into();
        execution.idempotency_key = "execution:preparing".into();
        execution.source = ExecutionSource::Docker {
            attempt: 1,
            phase: "prepare".into(),
            image: None,
            groups: placeholder_groups(&execution.slots),
        };
        store.write_execution(&execution).await.unwrap();

        store.reconcile().await.unwrap();
        let preparing = store.read_execution("plan-preparing").await.unwrap();
        assert_eq!(preparing.state, "interrupted");
        assert!(preparing.error.unwrap().contains("worker restarted"));
        let restarted = until(&store, &id, |execution| {
            settled(execution) && execution.error.is_some()
        })
        .await;
        assert_eq!(
            *launcher.removed.lock().unwrap(),
            vec![id.clone(), "plan-preparing".into()]
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        );
        assert_eq!(restarted.state, "interrupted");
        assert!(restarted
            .error
            .as_deref()
            .unwrap()
            .contains("worker restarted"));
        assert_eq!(
            groups(&restarted)
                .into_iter()
                .map(|(_, state, _)| state)
                .take(2)
                .collect::<Vec<_>>(),
            ["done", "interrupted"]
        );
        // No group ran again; the finished one was imported.
        assert!(launcher.calls("group").is_empty());
        assert_eq!(restarted.slots[0].execution_id, native);
        assert!(runner.record(&native).await.is_some());
        assert_eq!(restarted.slots[1].state, "not_run");
    }

    #[tokio::test]
    async fn the_import_reads_a_folder_as_it_reads_a_github_run() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let runner = Arc::new(FakeRunner::new(data.clone()));
        let store = docker_store(
            &data,
            runner.clone(),
            Arc::new(FakeLauncher::default()),
            DockerSettings::default(),
        );
        let id = "plan-folder".to_owned();
        let artifacts = store.docker_artifacts(&id);
        let mut natives = Vec::new();
        for attempt in [1, 2] {
            let (bundle, contract, native) = exact_stack_bundle(
                &root.path().join(format!("attempt-{attempt}")),
                &format!("rc:e2e:{attempt}"),
                "1.8.8",
            );
            fs::write(
                contract.join("stack.yaml"),
                format!("iii: 0.24.{attempt}\n"),
            )
            .unwrap();
            copy_tree(
                &bundle,
                &artifacts.join(format!("e2e-observation-{id}-gh-{attempt}")),
            )
            .await
            .unwrap();
            if attempt == 1 {
                copy_tree(
                    &contract,
                    &artifacts.join(format!("e2e-contract-{id}-gh-1")),
                )
                .await
                .unwrap();
            }
            natives.push(native);
        }
        let mut execution = PlanExecution {
            id: id.clone(),
            idempotency_key: "execution:folder".into(),
            label: None,
            parameters: Some(docker_parameters("pr")),
            source: ExecutionSource::Docker {
                attempt: 1,
                phase: "import".into(),
                image: None,
                groups: Vec::new(),
            },
            stack: Vec::new(),
            warnings: Vec::new(),
            state: "running".into(),
            started_at: now(),
            updated_at: now(),
            finished_at: None,
            cancel_requested: false,
            error: None,
            slots: Vec::new(),
            measurements: None,
            system_under_test: None,
            rerun: None,
        };
        store.write_execution(&execution).await.unwrap();
        store.download_and_install(&id, None).await.unwrap();
        execution = store.read_execution(&id).await.unwrap();
        // The highest attempt, as from GitHub; the folder keeps its copy.
        assert_eq!(execution.slots[0].execution_id, natives[1]);
        assert!(runner.record(&natives[1]).await.is_some());
        assert!(matches!(
            execution.source,
            ExecutionSource::Docker { attempt: 2, ref phase, .. } if phase == "done"
        ));
        assert!(artifacts
            .join(format!(
                "e2e-observation-{id}-gh-2/smoke-r01/groups/case-context/native"
            ))
            .join(&natives[1])
            .is_dir());
        let stack = execution.parameters.unwrap().stack.unwrap();
        assert_eq!(
            (stack.name.as_str(), stack.yaml.as_str()),
            ("default", "iii: 0.24.1\n")
        );
    }

    #[test]
    fn a_reviewed_suite_is_sent_by_id_and_any_other_whole() {
        let master = test_plan::embedded().unwrap();
        let mut parameters = suite_parameters("software-engineering");
        assert_eq!(
            dispatch_suite(&parameters, &master).unwrap(),
            "software-engineering"
        );
        // Changed, it runs whole under its name, sequential groups kept.
        parameters.runs = 2;
        let sent: Value =
            serde_json::from_str(&dispatch_suite(&parameters, &master).unwrap()).unwrap();
        assert_eq!(sent["id"], "software-engineering");
        assert_eq!(sent["repetitions"], 2);
        assert_eq!(
            sent["scenario_groups"],
            json!([["registry_implementation", "registry_verification"]])
        );
        master
            .materialize_suite(serde_json::from_value(sent).unwrap())
            .unwrap();
        // Ticked by hand: unnamed.
        parameters.suite = None;
        parameters.scenarios = vec!["minimal_path".into(), "a_scenario_it_may_know".into()];
        let sent: Value =
            serde_json::from_str(&dispatch_suite(&parameters, &master).unwrap()).unwrap();
        assert_eq!(sent["id"], "unnamed");
        assert_eq!(
            sent["scenarios"],
            json!(["minimal_path", "a_scenario_it_may_know"])
        );
    }

    #[tokio::test]
    async fn a_scripts_dir_is_copied_into_each_new_execution() {
        let root = tempfile::tempdir().unwrap();
        let scripts = root.path().join("scripts");
        fs::create_dir_all(&scripts).unwrap();
        fs::write(scripts.join("executor.sh"), "echo edited\n").unwrap();
        let store = docker_store(
            &root.path().join("data"),
            Arc::new(FakeRunner::new(root.path().join("data"))),
            Arc::new(FakeLauncher::default()),
            DockerSettings {
                scripts_dir: Some(scripts.clone()),
                ..DockerSettings::default()
            },
        );
        let folder = root.path().join("execution");
        store.write_checkout(&folder).await.unwrap();
        assert_eq!(
            fs::read_to_string(folder.join("checkout/scripts/executor.sh")).unwrap(),
            "echo edited\n"
        );
        assert!(!folder.join("checkout/scripts/run_in_image.sh").exists());
        // The next execution reads it again.
        fs::write(scripts.join("executor.sh"), "echo again\n").unwrap();
        let next = root.path().join("next");
        store.write_checkout(&next).await.unwrap();
        assert_eq!(
            fs::read_to_string(next.join("checkout/scripts/executor.sh")).unwrap(),
            "echo again\n"
        );
    }

    #[tokio::test]
    async fn the_wrapper_gets_the_phase_its_env_file_and_none_of_the_workers_environment() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("checkout");
        fs::create_dir_all(checkout.join("scripts")).unwrap();
        let wrapper = checkout.join("scripts/run_in_image.sh");
        fs::write(
            &wrapper,
            "printf '%s\\n' \"$@\" \"KEY=${EXECUTION_KEY:-}\" \"CARGO=${CARGO_MANIFEST_DIR:-unset}\"\n[ \"${!#}\" != fail ]\n",
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        // Set for this test process, never passed on.
        assert!(std::env::var_os("CARGO_MANIFEST_DIR").is_some());
        let log = root.path().join("phase.log");
        let phase = |args: &[&str]| Phase {
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: vec![("EXECUTION_KEY".into(), "plan-1".into())],
            env_file: Some(root.path().join("providers.env")),
            log: log.clone(),
        };
        assert!(ImageLauncher
            .run(&checkout, "plan-1", phase(&["group"]), uncancellable())
            .await
            .unwrap());
        assert!(!ImageLauncher
            .run(
                &checkout,
                "plan-1",
                phase(&["prepare", "fail"]),
                uncancellable()
            )
            .await
            .unwrap());
        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            format!(
                "--env-file\n{0}\ngroup\nKEY=plan-1\nCARGO=unset\n--env-file\n{0}\nprepare\nfail\nKEY=plan-1\nCARGO=unset\n",
                root.path().join("providers.env").display()
            )
        );
    }
}
