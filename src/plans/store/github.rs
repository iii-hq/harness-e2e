//! Executions imported from the exact-stack GitHub workflow, and from a
//! Docker execution's folder, which holds the same artifacts.
//!
//! `gh` only lists runs and downloads artifacts. Installing reads an
//! extracted bundle: every native run in it becomes an ordinary retained run
//! and the execution records how the groups map onto them.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use futures_util::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::process::Command;

use super::{
    finish, now, update_slot, ExecutionParameters, ExecutionSource, ExecutionStack, ExecutionSuite,
    PlanExecution, PlanStore, Runner, Slot, StackWorker, Where,
};
use crate::artifact;
use crate::control::{ExecutionPhase, ExecutionRecord, LaneBudget, RunRequest};
use crate::report::{E2eManifest, E2eReport};

const WORKFLOW: &str = "exact-stack-e2e.yml";
const PAGE_SIZE: usize = 20;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub(crate) struct GithubRunsListRequest {
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct GithubRunContractsRequest {
    #[serde(default)]
    pub repository: Option<String>,
    /// The runs of one listed page whose contract is not read yet.
    pub runs: Vec<GithubRunAttempt>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct GithubRunAttempt {
    pub run_id: u64,
    pub run_attempt: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct GithubRunImportRequest {
    #[serde(default)]
    pub repository: Option<String>,
    pub run_id: u64,
}

/// How the worker calls the GitHub CLI; tests point it at a stand-in.
#[derive(Debug, Clone)]
pub(crate) struct GithubCli {
    pub program: PathBuf,
    /// For `gh api` calls and the small contract artifact.
    pub api_timeout: Duration,
    /// For the run's evidence bundle (hundreds of MB).
    pub download_timeout: Duration,
}

impl Default for GithubCli {
    fn default() -> Self {
        Self {
            program: "gh".into(),
            api_timeout: Duration::from_secs(60),
            download_timeout: Duration::from_secs(30 * 60),
        }
    }
}

/// `gh run download` when nothing matches, expired artifacts included.
const NO_ARTIFACT: &str = "no valid artifacts found to download";

/// Where an import reads an execution's artifacts, each named as the
/// workflow uploads it (`<stem>-gh-<attempt>`): a GitHub run through `gh`,
/// or the folder a Docker execution wrote them to.
pub(super) enum Bundles {
    Github { repository: String, run_id: u64 },
    Folder(PathBuf),
}

impl Bundles {
    fn of(store: &PlanStore, execution: &PlanExecution) -> Result<Self> {
        Ok(match &execution.source {
            ExecutionSource::Github {
                repository, run_id, ..
            } => Self::Github {
                repository: repository.clone(),
                run_id: *run_id,
            },
            ExecutionSource::Docker { .. } => Self::Folder(store.docker_artifacts(&execution.id)),
            ExecutionSource::Local => {
                bail!("execution {} has no artifacts to import", execution.id)
            }
        })
    }

    /// The names of the artifacts it keeps; expired ones are not kept.
    async fn names(&self, store: &PlanStore) -> Result<Vec<String>> {
        match self {
            Self::Github { repository, run_id } => {
                let listing = store
                    .gh(
                        store.github.api_timeout,
                        &[
                            "api",
                            "--paginate",
                            &format!(
                                "repos/{repository}/actions/runs/{run_id}/artifacts?per_page=100"
                            ),
                            "--jq",
                            ".artifacts[] | select(.expired | not) | .name",
                        ],
                    )
                    .await?;
                Ok(String::from_utf8_lossy(&listing)
                    .lines()
                    .map(str::to_owned)
                    .collect())
            }
            Self::Folder(folder) => Ok(directories(folder)
                .unwrap_or_default()
                .iter()
                .map(|path| file_name(path))
                .collect()),
        }
    }

    /// Put the artifact `name`'s files in `destination`, which it creates.
    async fn fetch(
        &self,
        store: &PlanStore,
        name: &str,
        destination: &Path,
        timeout: Duration,
    ) -> Result<()> {
        match self {
            Self::Github { repository, run_id } => {
                store
                    .gh(
                        timeout,
                        &[
                            "run",
                            "download",
                            &run_id.to_string(),
                            "-R",
                            repository,
                            "-n",
                            name,
                            "-D",
                            &destination.to_string_lossy(),
                        ],
                    )
                    .await?;
                Ok(())
            }
            // Copied: installing moves its native runs out, and a later
            // import reads the folder again.
            Self::Folder(folder) => super::docker::copy_tree(&folder.join(name), destination).await,
        }
    }
}
/// Contract downloads a listing runs at once.
const CONTRACT_DOWNLOADS: usize = 4;

impl PlanStore {
    /// Completed runs of the exact-stack workflow, newest first, with the
    /// local execution that already imported them. Only one `gh api` call:
    /// a run whose contract was read before carries its suite and subject,
    /// the others are `contract_pending` until `github_run_contracts`.
    pub(crate) async fn github_runs(&self, repository: &str, page: u32) -> Result<Value> {
        validate_repository(repository)?;
        ensure!(page >= 1, "page starts at 1");
        let response: Value = serde_json::from_slice(
            &self
                .gh(
                    self.github.api_timeout,
                    &[
                        "api",
                        &format!(
                            "repos/{repository}/actions/workflows/{WORKFLOW}/runs?status=completed&per_page={PAGE_SIZE}&page={page}"
                        ),
                    ],
                )
                .await?,
        )
        .context("decode GitHub workflow runs")?;
        let runs = response["workflow_runs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut rows = Vec::new();
        for run in &runs {
            rows.push(self.run_row(repository, run).await);
        }
        let more = response["total_count"]
            .as_u64()
            .is_some_and(|total| total > u64::from(page) * PAGE_SIZE as u64);
        Ok(json!({
            "repository": repository,
            "page": page,
            "runs": rows,
            "next_page": more.then_some(page + 1),
        }))
    }

    /// A run as listed: dated by its creation, with the start of its latest
    /// attempt apart.
    async fn run_row(&self, repository: &str, run: &Value) -> Value {
        let run_id = run["id"].as_u64().unwrap_or_default();
        let attempt = run["run_attempt"].as_u64().unwrap_or(1);
        let title = run["display_title"].as_str().unwrap_or_default();
        let mut row = json!({
            "run_id": run_id,
            "run_attempt": attempt,
            "title": title,
            "created_at": run["created_at"].as_str().or(run["run_started_at"].as_str()),
            "attempt_started_at": run["run_started_at"],
            "conclusion": run["conclusion"],
            "url": run["html_url"],
            "release_control_execution_id": title.strip_prefix("E2E · "),
            "execution_id": null,
            "execution_state": null,
        });
        match self.cached_contract(repository, run_id, attempt) {
            Some(summary) => merge(&mut row, &summary),
            None => row["contract_pending"] = json!(true),
        }
        if let Ok(execution) = self.read_execution(&import_id(repository, run_id)).await {
            row["execution_id"] = json!(execution.id);
            row["execution_state"] = json!(execution.state);
        }
        row
    }

    /// Suite, subject, profile and runner of each run, read from its contract
    /// artifact (a few at a time) and cached. A failure is that run's
    /// `contract_error`.
    pub(crate) async fn github_run_contracts(
        &self,
        repository: &str,
        runs: &[GithubRunAttempt],
    ) -> Result<Value> {
        validate_repository(repository)?;
        ensure!(
            runs.len() <= PAGE_SIZE,
            "read at most {PAGE_SIZE} contracts at once"
        );
        let runs = runs
            .iter()
            .map(|run| (run.run_id, run.run_attempt))
            .collect::<Vec<_>>();
        let rows = futures_util::stream::iter(runs)
            .map(|(run_id, attempt)| async move {
                let mut row = json!({"run_id": run_id, "run_attempt": attempt});
                match self.contract_summary(repository, run_id, attempt).await {
                    Ok(summary) => merge(&mut row, &summary),
                    Err(error) => row["contract_error"] = json!(format!("{error:#}")),
                }
                row
            })
            .buffered(CONTRACT_DOWNLOADS)
            .collect::<Vec<_>>()
            .await;
        Ok(json!({"runs": rows}))
    }

    fn contract_cache(&self, repository: &str, run_id: u64, attempt: u64) -> PathBuf {
        self.root
            .join(".github-runs")
            .join(repository.replace('/', "__"))
            .join(format!("{run_id}-{attempt}.json"))
    }

    fn cached_contract(&self, repository: &str, run_id: u64, attempt: u64) -> Option<Value> {
        fs::read(self.contract_cache(repository, run_id, attempt))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    }

    /// Suite and subject of a run, read from its contract artifact once and
    /// cached under the data directory. A run whose contract is gone
    /// (expired or never uploaded) is cached as such too.
    async fn contract_summary(&self, repository: &str, run_id: u64, attempt: u64) -> Result<Value> {
        let cache = self.contract_cache(repository, run_id, attempt);
        if let Some(summary) = self.cached_contract(repository, run_id, attempt) {
            return Ok(summary);
        }
        let scratch = self.scratch()?;
        let downloaded = self
            .gh(
                self.github.api_timeout,
                &[
                    "run",
                    "download",
                    &run_id.to_string(),
                    "-R",
                    repository,
                    "-p",
                    "e2e-contract-*",
                    "-D",
                    &scratch.path().to_string_lossy(),
                ],
            )
            .await;
        let summary = match downloaded {
            Ok(_) => {
                let mut names = Vec::new();
                for entry in fs::read_dir(scratch.path())? {
                    names.push(entry?.file_name().to_string_lossy().into_owned());
                }
                match highest_attempt(names.iter().map(String::as_str), |stem| {
                    stem.starts_with("e2e-contract-")
                }) {
                    Some((contract, _)) => contract_fields(&scratch.path().join(contract)),
                    None => json!({"contract_error": "This run kept no e2e-contract artifact"}),
                }
            }
            Err(error) if format!("{error:#}").contains(NO_ARTIFACT) => json!({
                "contract_error": "This run keeps no e2e-contract artifact (GitHub keeps artifacts for 90 days)"
            }),
            Err(error) => return Err(error),
        };
        fs::create_dir_all(cache.parent().context("contract cache has no parent")?)?;
        artifact::write_atomic(&cache, &serde_json::to_vec(&summary)?)?;
        Ok(summary)
    }

    /// Create (or reset) the execution for one run in the `importing` state.
    /// A run already being imported is not started twice: the second call
    /// gets that execution back and `false`.
    pub(crate) async fn begin_github_import(
        &self,
        repository: &str,
        run_id: u64,
    ) -> Result<(PlanExecution, bool)> {
        validate_repository(repository)?;
        ensure!(run_id > 0, "run_id must be a GitHub Actions run id");
        let run: Value = serde_json::from_slice(
            &self
                .gh(
                    self.github.api_timeout,
                    &["api", &format!("repos/{repository}/actions/runs/{run_id}")],
                )
                .await?,
        )
        .context("decode GitHub workflow run")?;
        let id = import_id(repository, run_id);
        let _guard = self.lock.lock().await;
        let previous = self.read_execution(&id).await.ok();
        if let Some(previous) = previous.as_ref().filter(|e| e.state == "importing") {
            return Ok((previous.clone(), false));
        }
        let title = run["display_title"].as_str().unwrap_or_default();
        // A new import keeps the name and, until they are replaced, the runs
        // the previous import installed.
        let mut execution = previous.unwrap_or_else(|| PlanExecution {
            id: id.clone(),
            idempotency_key: format!("github:{repository}#{run_id}"),
            label: None,
            parameters: None,
            source: ExecutionSource::Local,
            stack: Vec::new(),
            warnings: Vec::new(),
            state: String::new(),
            started_at: now(),
            updated_at: now(),
            finished_at: None,
            cancel_requested: false,
            error: None,
            slots: Vec::new(),
            measurements: None,
            system_under_test: None,
            rerun: None,
        });
        execution.source = ExecutionSource::Github {
            repository: repository.to_owned(),
            run_id,
            run_attempt: run["run_attempt"].as_u64().unwrap_or(1) as u32,
            url: run["html_url"].as_str().unwrap_or_default().to_owned(),
            release_control_execution_id: title.strip_prefix("E2E · ").map(str::to_owned),
            stack: None,
        };
        execution.state = "importing".into();
        execution.error = None;
        // Dated by the run's creation, as the import list shows it.
        execution.started_at = run["created_at"]
            .as_str()
            .or(run["run_started_at"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(now);
        execution.finished_at = run["updated_at"].as_str().map(str::to_owned);
        execution.updated_at = now();
        self.write_execution(&execution).await?;
        Ok((execution, true))
    }

    /// Download and install the highest attempt of an import begun with
    /// `begin_github_import`. The execution always ends terminal: `completed`,
    /// or `failed` with the reason and whatever an earlier import installed.
    pub(crate) async fn finish_github_import(&self, id: &str) -> Result<()> {
        if let Err(error) = self.download_and_install(id, None).await {
            tracing::warn!(execution_id = %id, error = %format!("{error:#}"), "GitHub import failed");
            let _guard = self.lock.lock().await;
            let mut execution = self.read_execution(id).await?;
            execution.state = "failed".into();
            execution.error = Some(format!("{error:#}"));
            execution.updated_at = now();
            self.write_execution(&execution).await?;
        }
        Ok(())
    }

    /// Install the highest attempt of an execution's root bundle, from where
    /// its source keeps its artifacts; `stopped` says why it did not run to
    /// its end.
    pub(super) async fn download_and_install(
        &self,
        id: &str,
        stopped: Option<String>,
    ) -> Result<()> {
        let mut execution = self.read_execution(id).await?;
        let bundles = Bundles::of(self, &execution)?;
        let names = bundles.names(self).await?;
        let (contract, _) = highest_attempt(names.iter().map(String::as_str), |stem| {
            stem.starts_with("e2e-contract-")
        })
        .context("This run keeps no e2e-contract artifact; GitHub keeps artifacts for 90 days")?;
        let release_control_id = contract
            .rsplit_once("-gh-")
            .and_then(|(stem, _)| stem.strip_prefix("e2e-contract-"))
            .unwrap_or_default()
            .to_owned();
        let root_stem = format!("e2e-observation-{release_control_id}");
        let (bundle, attempt) =
            highest_attempt(names.iter().map(String::as_str), |stem| stem == root_stem)
                .with_context(|| {
                    format!(
                        "This run keeps no {root_stem} bundle; GitHub keeps artifacts for 90 days"
                    )
                })?;
        let scratch = self.scratch()?;
        let installed = async {
            for (name, directory, timeout) in [
                (contract, "contract", self.github.api_timeout),
                (bundle, "bundle", self.github.download_timeout),
            ] {
                bundles
                    .fetch(self, name, &scratch.path().join(directory), timeout)
                    .await?;
            }
            match &mut execution.source {
                ExecutionSource::Github {
                    run_attempt,
                    release_control_execution_id,
                    ..
                } => {
                    *run_attempt = attempt;
                    // A contract that states its execution says whether Release
                    // Control dispatched it; an older one is always Release Control's.
                    *release_control_execution_id =
                        match read_json(&scratch.path().join("contract/execution.json")) {
                            Ok(stated) => stated["execution_id"].as_str().map(str::to_owned),
                            Err(_) => Some(release_control_id),
                        };
                }
                ExecutionSource::Docker {
                    attempt: current,
                    phase,
                    ..
                } => (*current, *phase) = (attempt, "done".into()),
                ExecutionSource::Local => {}
            }
            self.install_bundle(
                &mut execution,
                &scratch.path().join("bundle"),
                &scratch.path().join("contract"),
                stopped,
            )
            .await
        }
        .await;
        // Deleting a bundle of hundreds of MB blocks.
        let _ = tokio::task::spawn_blocking(move || drop(scratch)).await;
        installed
    }

    /// A temporary directory inside the data directory: bundles are large and
    /// never go through memory or the system temporary directory.
    fn scratch(&self) -> Result<tempfile::TempDir> {
        tempfile::Builder::new()
            .prefix("github-")
            .tempdir_in(self.imports_dir()?)
            .context("create an import directory in the data directory")
    }

    fn imports_dir(&self) -> Result<PathBuf> {
        let path = self.root.join(".imports");
        fs::create_dir_all(&path).with_context(|| format!("create {}", path.display()))?;
        Ok(path)
    }

    /// Install an extracted exact-stack bundle: each group's native run is
    /// moved into the data directory and retained like a finished local run.
    /// A group without a readable native run becomes a slot with its error;
    /// a bundle with no readable group fails and leaves the execution as it
    /// was. Runs of the previous import that the new one does not carry are
    /// deleted only once the new execution is written. An execution that
    /// `stopped` before it ran everything ends interrupted, with the reason.
    pub(super) async fn install_bundle(
        &self,
        execution: &mut PlanExecution,
        bundle: &Path,
        contract: &Path,
        stopped: Option<String>,
    ) -> Result<()> {
        let runner = self.runner()?;
        if let Ok(failure) = read_json(&bundle.join("failure.json")) {
            bail!(
                "The run produced no bundle: {}",
                failure["error"].as_str().unwrap_or("no detail")
            );
        }
        let fields = contract_fields(contract);
        let campaigns = directories(bundle)?
            .into_iter()
            .filter(|path| path.join("groups").is_dir())
            .collect::<Vec<_>>();
        ensure!(!campaigns.is_empty(), "The bundle holds no campaign groups");
        let previous = execution
            .slots
            .iter()
            .map(|slot| slot.execution_id.clone())
            .filter(|id| !id.is_empty())
            .collect::<BTreeSet<_>>();

        let mut slots = Vec::new();
        let mut scenarios = Vec::new();
        let mut requests = Vec::new();
        let mut stacks = Vec::new();
        let mut errors = Vec::new();
        for (index, campaign) in campaigns.iter().enumerate() {
            let round = index as u32 + 1;
            // Groups in the order the campaign declared them, then any other.
            let mut groups = read_json(&campaign.join("stack-lock.json"))
                .ok()
                .and_then(|contract| contract["suite"]["groups"].as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|group| {
                    let id = group["id"].as_str()?.to_owned();
                    let scenarios = group["scenarios"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|id| id.as_str().map(str::to_owned))
                        .collect::<Vec<_>>();
                    Some((id, scenarios))
                })
                .collect::<Vec<_>>();
            for path in directories(&campaign.join("groups"))? {
                let name = file_name(&path);
                if !groups.iter().any(|(id, _)| *id == name) {
                    groups.push((name, Vec::new()));
                }
            }
            for (group_id, declared) in groups {
                let directory = campaign.join("groups").join(&group_id);
                match self
                    .install_group(runner, &directory, round, &group_id, &previous)
                    .await
                {
                    Ok((group_slots, request, stack)) => {
                        for slot in &group_slots {
                            push_unique(&mut scenarios, &slot.scenario_id);
                        }
                        slots.extend(group_slots);
                        stacks.push((group_id.clone(), stack));
                        requests.push(request);
                    }
                    Err(error) => {
                        let error = format!("{error:#}");
                        errors.push(format!("{group_id}: {error}"));
                        // Without the campaign contract the group's scenarios
                        // are unknown: one slot stands for the group.
                        for scenario in &declared {
                            push_unique(&mut scenarios, scenario);
                        }
                        let declared = if declared.is_empty() {
                            vec![group_id.clone()]
                        } else {
                            declared
                        };
                        for scenario in declared {
                            let mut slot = slot(round, &group_id, &scenario);
                            slot.state = "not_run".into();
                            slot.error = Some(error.clone());
                            slots.push(slot);
                        }
                    }
                }
            }
        }
        ensure!(
            !requests.is_empty(),
            "No group of this run left a native run this runner can read: {}",
            errors.join("; ")
        );

        let first = requests.first();
        let text = |value: &Value| value.as_str().map(str::to_owned);
        let asked = execution.parameters.as_ref();
        // The stack as its contract recorded it once assembled, to run it
        // again as recorded; named as it was asked for.
        let stack = fs::read_to_string(contract.join("stack.yaml"))
            .ok()
            .map(|yaml| ExecutionStack {
                name: asked
                    .and_then(|parameters| parameters.stack.as_ref())
                    .map(|stack| stack.name.clone())
                    .or_else(|| text(&fields["stack"]))
                    .unwrap_or_else(|| "inline".into()),
                sha256: artifact::sha256_bytes(yaml.as_bytes()),
                yaml,
            })
            .or_else(|| asked.and_then(|parameters| parameters.stack.clone()));
        let r#where = match &execution.source {
            ExecutionSource::Github { .. } => Where::Github,
            ExecutionSource::Docker { .. } => Where::Docker,
            ExecutionSource::Local => asked.map_or(Where::Harness, |parameters| parameters.r#where),
        };
        let parameters = ExecutionParameters {
            scenarios,
            runs: campaigns.len() as u32,
            technical_retries: requests
                .iter()
                .map(|request| request.technical_retries)
                .max()
                .unwrap_or_default(),
            model: text(&fields["model"])
                .or_else(|| first.map(|request| request.model.clone()))
                .unwrap_or_default(),
            provider: text(&fields["provider"])
                .or_else(|| first.map(|request| request.provider.clone()))
                .unwrap_or_default(),
            agent: text(&fields["agent"])
                .or_else(|| first.and_then(|request| request.agent.clone())),
            suite: text(&fields["suite"]).map(|id| ExecutionSuite {
                label: text(&fields["suite_label"]).unwrap_or_else(|| id.clone()),
                id: Some(id),
                sha256: text(&fields["suite_sha256"]).unwrap_or_default(),
            }),
            r#where,
            stack,
        };
        let mut next = execution.clone();
        next.parameters = Some(parameters);
        if let ExecutionSource::Github { stack, .. } = &mut next.source {
            *stack = text(&fields["stack"]);
        }
        next.stack = merge_stacks(stacks);
        next.slots = slots;
        next.error = None;
        next.measurements = None;
        let finished_at = next.finished_at.take();
        let root = self.root.clone();
        let mut next = tokio::task::spawn_blocking(move || {
            finish(&mut next, stopped, &root)?;
            Ok::<_, anyhow::Error>(next)
        })
        .await
        .context("consolidate the imported runs")??;
        next.finished_at = finished_at.or(next.finished_at.take());
        {
            let _guard = self.lock.lock().await;
            // A rename made while the import ran wins over the suite name.
            next.label = self
                .read_execution(&next.id)
                .await?
                .label
                .or_else(|| text(&fields["suite_label"]));
            self.write_execution(&next).await?;
        }
        let kept = next
            .slots
            .iter()
            .map(|slot| slot.execution_id.clone())
            .collect::<BTreeSet<_>>();
        for id in previous.difference(&kept) {
            if let Err(error) = runner.remove(id).await {
                tracing::warn!(execution_id = %next.id, native_id = %id, error = %format!("{error:#}"), "cannot remove a run the import replaced");
            }
        }
        *execution = next;
        Ok(())
    }

    /// Move one group's native run into the data directory, retain it, and
    /// return the group's slots. Evidence another execution retains under the
    /// same id is never replaced.
    async fn install_group(
        &self,
        runner: &std::sync::Arc<dyn Runner>,
        directory: &Path,
        round: u32,
        group_id: &str,
        previous: &BTreeSet<String>,
    ) -> Result<(Vec<Slot>, RunRequest, Vec<StackWorker>)> {
        let root = self.root.clone();
        let directory = directory.to_owned();
        let group_id = group_id.to_owned();
        let replaceable = previous.clone();
        let (record, slots, request, stack) = tokio::task::spawn_blocking(move || {
            let natives = directories(&directory.join("native")).unwrap_or_default();
            if natives.is_empty() {
                let failure = read_json(&directory.join("failure.json")).unwrap_or(Value::Null);
                bail!(
                    "{}",
                    failure["error"]
                        .as_str()
                        .unwrap_or("The group retained no native run")
                );
            }
            ensure!(
                natives.len() == 1,
                "The group retained {} native runs; expected one",
                natives.len()
            );
            let source = &natives[0];
            let id = file_name(source);
            ensure!(
                id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "The group's native run '{id}' is not an E2E execution id"
            );
            let target = root.join(&id);
            ensure!(
                !target.exists() || replaceable.contains(&id),
                "Native run {id} is already retained by another execution; its evidence was left untouched"
            );
            let request_value = read_json(&directory.join("run-request.json"))?;
            let request: RunRequest = serde_json::from_value(request_value.clone())
                .context("This runner cannot read the group's run request")?;
            let (report, _) = E2eReport::read_from(source)
                .context("This runner cannot read the group's native report")?;
            ensure!(
                report.execution.execution_id == id,
                "The native report in {id} belongs to execution {}",
                report.execution.execution_id
            );
            let manifest: E2eManifest =
                serde_json::from_value(read_json(&source.join("manifest.json"))?)
                    .context("This runner cannot read the group's native manifest")?;
            let status = read_json(&directory.join("status.json")).unwrap_or(Value::Null);
            let phase = serde_json::from_value::<ExecutionPhase>(status["phase"].clone())
                .ok()
                .filter(|phase| phase.terminal())
                .unwrap_or(ExecutionPhase::Completed);
            if target.exists() {
                fs::remove_dir_all(&target)
                    .with_context(|| format!("replace retained evidence for {id}"))?;
            }
            fs::rename(source, &target).with_context(|| format!("move native run {id}"))?;
            let record = ExecutionRecord {
                execution_id: id.clone(),
                idempotency_key: request.idempotency_key.clone(),
                phase,
                requested_at: report.execution.started_at.clone(),
                updated_at: report.execution.completed_at.clone(),
                request_sha256: artifact::sha256_value(&request)?,
                run_contract_sha256: request
                    .run_contract
                    .as_ref()
                    .map(artifact::sha256_value)
                    .transpose()?,
                lane_budget: crate::control::validate_run_request(&request).unwrap_or(
                    LaneBudget {
                        max_cases: request.scenarios.len() as u16,
                        max_runs_per_case: request.runs,
                        max_technical_retries: request.technical_retries,
                        max_declared_turns: 0,
                    },
                ),
                request: request.clone(),
                transitions: Vec::new(),
                journal_progress: Default::default(),
                active_attempt: None,
                resume_state_path: None,
                resume_state_sha256: None,
                cancel_requested: false,
                error: status["error"].as_str().unwrap_or_default().to_owned(),
                result_path: Some(format!("{id}/results.json")),
                report: Some(report),
                dashboard_projection: None,
                manifest: Some(manifest),
                observation: None,
                observation_artifact: None,
                archive: None,
            };
            let slots = request
                .scenarios
                .iter()
                .map(|scenario| {
                    let mut slot = slot(round, &group_id, scenario.as_str());
                    slot.execution_id = id.clone();
                    slot.request = request_value.clone();
                    if let Err(error) = update_slot(&mut slot, &record, &root) {
                        slot.error = Some(format!("{error:#}"));
                    }
                    slot
                })
                .collect::<Vec<_>>();
            Ok((record, slots, request, group_stack(&directory)))
        })
        .await
        .context("install the group's native run")??;
        let id = record.execution_id.clone();
        if let Err(error) = runner.install(record).await {
            // Evidence this import just brought in goes with the failed
            // install, so importing again does not mistake it for another
            // execution's.
            if !previous.contains(&id) {
                let target = self.root.join(&id);
                let _ = tokio::task::spawn_blocking(move || fs::remove_dir_all(target)).await;
            }
            return Err(error);
        }
        Ok((slots, request, stack))
    }

    /// Run `gh` with its temporary files in the data directory (it stages
    /// downloads there) and a deadline; a missing binary, a failed call or a
    /// call past its deadline becomes the step the user has to take.
    pub(super) async fn gh(&self, timeout: Duration, args: &[&str]) -> Result<Vec<u8>> {
        let command = Command::new(&self.github.program)
            .args(args)
            .env("GH_PROMPT_DISABLED", "1")
            .env("TMPDIR", self.imports_dir()?)
            .kill_on_drop(true)
            .output();
        let output = match tokio::time::timeout(timeout, command).await {
            Err(_) => bail!(
                "`gh {}` did not finish within {timeout:?} and was stopped; try again.",
                args.join(" ")
            ),
            Ok(Ok(output)) => output,
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => bail!(
                "The GitHub CLI (`gh`) is not installed for the Harness E2E worker. Install it, then run `gh auth login`."
            ),
            Ok(Err(error)) => return Err(error).context("run the GitHub CLI"),
        };
        ensure!(
            output.status.success(),
            "`gh {}` failed: {}. Check access with `gh auth status`; sign in with `gh auth login`.",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(output.stdout)
    }
}

/// One execution per repository and run: importing again replaces it.
pub(super) fn import_id(repository: &str, run_id: u64) -> String {
    let digest = artifact::sha256_bytes(format!("github:{repository}#{run_id}").as_bytes());
    format!("plan-{}", &digest[7..39])
}

/// The artifact named `<stem>-gh-<attempt>` with the highest attempt among
/// those whose stem matches.
pub(super) fn highest_attempt<'a>(
    names: impl IntoIterator<Item = &'a str>,
    matches: impl Fn(&str) -> bool,
) -> Option<(&'a str, u32)> {
    names
        .into_iter()
        .filter_map(|name| {
            let (stem, attempt) = name.rsplit_once("-gh-")?;
            matches(stem).then_some((name, attempt.parse().ok()?))
        })
        .max_by_key(|(_, attempt)| *attempt)
}

/// What a contract artifact says its execution ran. A contract states the
/// execution itself (`execution.json`: suite, stack, `provider/model`, agent
/// profile, iii) with the suite's snapshot and the stack's lock; one from
/// before names only the plan Release Control dispatched (`plan.json`,
/// `profile.json`). Either is read, the stated execution first.
fn contract_fields(contract: &Path) -> Value {
    let read = |name: &str| read_json(&contract.join(name)).unwrap_or(Value::Null);
    let first =
        |stated: &Value, older: &Value| if stated.is_null() { older } else { stated }.clone();
    let (execution, plan) = (read("execution.json"), read("plan.json"));
    let snapshot = first(&read("suite.json"), &read("profile.json"));
    let (provider, model) = match execution["model"].as_str().and_then(|m| m.split_once('/')) {
        Some((provider, model)) => (json!(provider), json!(model)),
        None => (
            plan["subject"]["provider"].clone(),
            plan["subject"]["model"].clone(),
        ),
    };
    let lock = fs::read_to_string(contract.join("worker-compose.lock"))
        .ok()
        .and_then(|source| serde_yaml::from_str::<Value>(&source).ok())
        .unwrap_or(Value::Null);
    json!({
        "suite": first(&snapshot["profile"]["id"], &plan["profile"]["id"]),
        "suite_label": snapshot["profile"]["label"],
        "suite_sha256": snapshot["profile_sha256"],
        "model": model,
        "provider": provider,
        "agent": first(&execution["profile"], &plan["agent_profile"]),
        "runner_version": first(
            &lock["containers"]["harness-e2e"]["resolved"]["version"],
            &plan["runner"]["version"],
        ),
        "stack": execution["stack"],
        "iii": read("contracts/resolution.json")["cli_version"],
    })
}

/// Workers one group ran on: what its compose lock resolved and what the
/// engine reported running. Engine built-ins are left out.
fn group_stack(directory: &Path) -> Vec<StackWorker> {
    let lock = fs::read_to_string(directory.join("stack/worker-compose.lock"))
        .ok()
        .and_then(|source| serde_yaml::from_str::<Value>(&source).ok())
        .unwrap_or(Value::Null);
    let workers = read_json(&directory.join("stack/workers.json")).unwrap_or(Value::Null);
    super::stack::rows(
        &lock["containers"],
        |container| {
            container["resolved"]["version"]
                .as_str()
                .or(container["requested"].as_str())
                .map(str::to_owned)
        },
        super::stack::observed_versions(&workers, None),
    )
}

/// One row per distinct worker version; groups are listed only for a worker
/// whose version differs between groups.
fn merge_stacks(groups: Vec<(String, Vec<StackWorker>)>) -> Vec<StackWorker> {
    let mut rows: Vec<StackWorker> = Vec::new();
    for (group, workers) in groups {
        for worker in workers {
            match rows.iter_mut().find(|row| {
                StackWorker {
                    groups: Vec::new(),
                    ..(*row).clone()
                } == worker
            }) {
                Some(row) => row.groups.push(group.clone()),
                None => rows.push(StackWorker {
                    groups: vec![group.clone()],
                    ..worker
                }),
            }
        }
    }
    let mut versions = BTreeMap::<String, usize>::new();
    for row in &rows {
        *versions.entry(row.name.clone()).or_default() += 1;
    }
    for row in &mut rows {
        if versions[&row.name] == 1 {
            row.groups.clear();
        }
    }
    rows.sort_by(|left, right| left.name.cmp(&right.name));
    rows
}

pub(super) fn slot(round: u32, group_id: &str, scenario_id: &str) -> Slot {
    Slot {
        round,
        group_id: group_id.into(),
        scenario_id: scenario_id.into(),
        execution_id: String::new(),
        request: Value::Null,
        state: "pending".into(),
        result_path: None,
        error: None,
        observed: 0,
        completed: 0,
        passed: 0,
        technical_valid: 0,
        eligible: false,
        previous_attempts: Vec::new(),
    }
}

fn merge(row: &mut Value, fields: &Value) {
    for (key, value) in fields.as_object().into_iter().flatten() {
        row[key] = value.clone();
    }
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_owned());
    }
}

fn validate_repository(repository: &str) -> Result<()> {
    let parts = repository.split('/').collect::<Vec<_>>();
    ensure!(
        parts.len() == 2
            && parts.iter().all(|part| {
                !part.is_empty()
                    && *part != "."
                    && *part != ".."
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            }),
        "repository must be a GitHub owner/name"
    );
    Ok(())
}

pub(super) fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("read {}", path.display()))?)
        .with_context(|| format!("decode {}", path.display()))
}

pub(super) fn directories(path: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            entries.push(entry.path());
        }
    }
    entries.sort();
    Ok(entries)
}

pub(super) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::WorkerSource;
    use super::*;

    #[test]
    fn repositories_are_owner_and_name_only() {
        for valid in ["iii-hq/harness-e2e", "owner/repo.name", "a_b/c-d"] {
            assert!(validate_repository(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "",
            "owner",
            "owner/",
            "/repo",
            "a/b/c",
            "../repo",
            "owner/..",
            "owner/re po",
            "owner/repo;rm",
            "https://github.com/o/r",
        ] {
            assert!(validate_repository(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn the_highest_attempt_of_the_matching_artifact_wins() {
        let names = [
            "e2e-observation-exec-gh-1",
            "e2e-observation-exec-case-a-gh-3",
            "e2e-observation-exec-gh-2",
            "e2e-contract-exec-gh-1",
        ];
        assert_eq!(
            highest_attempt(names, |stem| stem == "e2e-observation-exec"),
            Some(("e2e-observation-exec-gh-2", 2))
        );
        assert_eq!(
            highest_attempt(names, |stem| stem.starts_with("e2e-contract-")),
            Some(("e2e-contract-exec-gh-1", 1))
        );
        assert_eq!(highest_attempt(names, |stem| stem == "missing"), None);
    }

    #[test]
    fn contracts_are_read_whether_they_state_the_execution_or_only_the_plan() {
        let root = tempfile::tempdir().unwrap();
        let write = |dir: &Path, name: &str, value: &str| {
            let path = dir.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, value).unwrap();
        };
        let older = root.path().join("older");
        write(
            &older,
            "plan.json",
            &json!({"profile": {"id": "regression"},
            "subject": {"provider": "deepseek", "model": "deepseek-v4-flash"},
            "agent_profile": "tech-lead", "runner": {"version": "0.11.28"}})
            .to_string(),
        );
        write(
            &older,
            "profile.json",
            &json!({"profile": {"id": "regression", "label": "Regression"}, "profile_sha256": "sha256:regression"}).to_string(),
        );
        write(
            &older,
            "contracts/resolution.json",
            r#"{"cli_version": "0.24.1"}"#,
        );
        assert_eq!(
            contract_fields(&older),
            json!({"suite": "regression", "suite_label": "Regression", "suite_sha256": "sha256:regression", "model": "deepseek-v4-flash",
                "provider": "deepseek", "agent": "tech-lead", "runner_version": "0.11.28",
                "stack": null, "iii": "0.24.1"})
        );

        let stated = root.path().join("stated");
        write(
            &stated,
            "execution.json",
            &json!({"execution_id": null, "suite": {"id": "smoke"},
            "stack": "default", "model": "zai/glm-5.1", "profile": null, "iii": "0.24.2"})
            .to_string(),
        );
        write(
            &stated,
            "suite.json",
            &json!({"profile": {"id": "smoke", "label": "smoke"}}).to_string(),
        );
        write(&stated, "worker-compose.lock", "version: 1\ncontainers:\n  harness-e2e:\n    worker: package://harness-e2e\n    requested: latest\n    resolved:\n      version: 0.12.2\n");
        write(
            &stated,
            "contracts/resolution.json",
            r#"{"cli_version": "0.24.2"}"#,
        );
        assert_eq!(
            contract_fields(&stated),
            json!({"suite": "smoke", "suite_label": "smoke", "suite_sha256": null, "model": "glm-5.1", "provider": "zai",
                "agent": null, "runner_version": "0.12.2", "stack": "default", "iii": "0.24.2"})
        );
    }

    #[test]
    fn stack_rows_list_groups_only_where_a_worker_differs() {
        let worker = |name: &str, observed: &str| StackWorker {
            name: name.into(),
            source: WorkerSource::Package,
            requested: Some("1.0.0".into()),
            observed: Some(observed.into()),
            commit: None,
            dirty: None,
            groups: Vec::new(),
        };
        let rows = merge_stacks(vec![
            (
                "a".into(),
                vec![worker("state", "1.0.0"), worker("harness", "1.8.8")],
            ),
            (
                "b".into(),
                vec![worker("state", "1.0.0"), worker("harness", "1.8.9")],
            ),
            ("c".into(), vec![worker("harness", "1.8.8")]),
        ]);
        assert_eq!(
            rows.iter()
                .map(|row| (
                    row.name.as_str(),
                    row.observed.as_deref().unwrap(),
                    row.groups.join(",")
                ))
                .collect::<Vec<_>>(),
            vec![
                ("harness", "1.8.8", "a,c".to_owned()),
                ("harness", "1.8.9", "b".to_owned()),
                ("state", "1.0.0", String::new()),
            ]
        );
    }
}
