//! Executions this worker starts on GitHub: it dispatches the exact-stack
//! workflow with the execution's parameters, follows the run while GitHub
//! runs it, and imports its artifacts when it ends, through the same path as
//! an import picked by hand.
//!
//! GitHub offers no push to a local worker, so following is the one poll:
//! `gh api` on the run every `follow_interval`.
use std::sync::Arc;

use anyhow::{ensure, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::Command;

use super::docker::dispatch_suite;
use super::github::import_id;
use super::{now, sequential_groups, ExecutionSource, PlanExecution, PlanStore, Rerun};
use crate::plans::stacks;
use crate::test_plan::MasterPlan;

const WORKFLOW: &str = "exact-stack-e2e.yml";
/// Marks an execution this worker started on GitHub, apart from imports.
const KEY_PREFIX: &str = "github-start:";

const NOT_INSTALLED: &str = "The GitHub CLI (`gh`) is not installed for the Harness E2E worker. Install it, then run `gh auth login`.";
const NOT_SIGNED_IN: &str =
    "`gh` is not signed in on the worker's machine. Run `gh auth login` there, then reopen this dialog.";

/// One job of the run's latest attempt, as the follow loop last saw it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct GithubJob {
    pub id: u64,
    pub name: String,
    /// `queued`, `in_progress`, `completed` (GitHub's words).
    pub status: String,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
    #[serde(default)]
    pub url: String,
}

/// What following a GitHub run adds to its execution.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct GithubFollow {
    /// This worker follows the run: started here, or a job re-run of an
    /// imported run.
    #[serde(default)]
    pub followed: bool,
    #[serde(default)]
    pub head_branch: Option<String>,
    #[serde(default)]
    pub head_sha: Option<String>,
    #[serde(default)]
    pub jobs: Vec<GithubJob>,
}

impl GithubFollow {
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Followed on GitHub by this worker (started here, or a job re-run): its
/// page follows the run until it ends and the worker imports it.
pub(super) fn github_started(execution: &PlanExecution) -> bool {
    match &execution.source {
        ExecutionSource::Github { follow, .. } => {
            follow.followed || execution.idempotency_key.starts_with(KEY_PREFIX)
        }
        _ => false,
    }
}

fn jobs_of(value: &Value) -> Vec<GithubJob> {
    value["jobs"]
        .as_array()
        .map(|jobs| {
            jobs.iter()
                .map(|job| GithubJob {
                    id: job["id"].as_u64().unwrap_or(0),
                    name: job["name"].as_str().unwrap_or_default().to_owned(),
                    status: job["status"].as_str().unwrap_or("queued").to_owned(),
                    conclusion: job["conclusion"].as_str().map(str::to_owned),
                    started_at: job["started_at"].as_str().map(str::to_owned),
                    completed_at: job["completed_at"].as_str().map(str::to_owned),
                    url: job["html_url"].as_str().unwrap_or_default().to_owned(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What the workflow's `stack` input gets: a repository stack by name when it
/// is that stack as written (the workflow reads `stacks/<name>.yaml` from the
/// branch), any other stack as its YAML.
pub(super) fn dispatch_stack(name: &str, yaml: &str) -> String {
    if stacks::REPOSITORY
        .iter()
        .any(|(id, listed)| *id == name && listed.trim() == yaml.trim())
    {
        name.to_owned()
    } else {
        yaml.to_owned()
    }
}

impl PlanStore {
    /// Whether `gh` can dispatch to `repository`: installed and signed in.
    /// A missing or signed-out `gh` is `ready: false` with how to fix it.
    pub(crate) fn docker_parallel_groups(&self) -> usize {
        self.docker.settings.parallel_groups.max(1)
    }

    pub(crate) async fn github_status(&self, repository: &str) -> Value {
        let output = tokio::time::timeout(
            self.github.api_timeout,
            Command::new(&self.github.program)
                .args(["auth", "status", "-h", "github.com"])
                .env("GH_PROMPT_DISABLED", "1")
                .kill_on_drop(true)
                .output(),
        )
        .await;
        let (ready, account, message) = match output {
            Err(_) => (false, None, Some("`gh auth status` did not answer in time; try again.".to_owned())),
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                (false, None, Some(NOT_INSTALLED.to_owned()))
            }
            Ok(Err(error)) => (false, None, Some(format!("Could not run `gh`: {error}"))),
            Ok(Ok(output)) if !output.status.success() => (false, None, Some(NOT_SIGNED_IN.to_owned())),
            Ok(Ok(output)) => {
                let text = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                (true, signed_in_account(&text), None)
            }
        };
        json!({
            "ready": ready,
            "repository": repository,
            "account": account,
            "message": message,
        })
    }

    /// Dispatch the workflow for a validated execution and keep it as the
    /// execution of that run, `running` until the run ends.
    pub(super) async fn start_github(
        self: &Arc<Self>,
        mut execution: PlanExecution,
        master: &MasterPlan,
        repository: &str,
    ) -> Result<PlanExecution> {
        let status = self.github_status(repository).await;
        if status["ready"] != json!(true) {
            anyhow::bail!("{}", status["message"].as_str().unwrap_or(NOT_SIGNED_IN));
        }
        let parameters = execution
            .parameters
            .clone()
            .context("a GitHub execution has parameters")?;
        let stack = parameters
            .stack
            .as_ref()
            .context("Pick the stack the execution runs on on GitHub.")?;
        let branch = String::from_utf8_lossy(
            &self
                .gh(
                    self.github.api_timeout,
                    &["api", &format!("repos/{repository}"), "--jq", ".default_branch"],
                )
                .await?,
        )
        .trim()
        .to_owned();
        let branch = if branch.is_empty() { "main".to_owned() } else { branch };
        let mut fields = vec![
            format!("ref={branch}"),
            format!("inputs[suite]={}", dispatch_suite(&parameters, master)?),
            format!("inputs[stack]={}", dispatch_stack(&stack.name, &stack.yaml)),
            format!("inputs[model]={}/{}", parameters.provider, parameters.model),
        ];
        if let Some(agent) = &parameters.agent {
            fields.push(format!("inputs[profile]={agent}"));
        }
        let endpoint = format!("repos/{repository}/actions/workflows/{WORKFLOW}/dispatches");
        let mut args = vec!["api", "-X", "POST", endpoint.as_str(), "-F", "return_run_details=true"];
        for field in &fields {
            args.push("-f");
            args.push(field);
        }
        let response = self.gh(self.github.api_timeout, &args).await?;
        let run: Value = serde_json::from_slice(&response).unwrap_or(Value::Null);
        let run_id = run["workflow_run_id"]
            .as_u64()
            .or_else(|| run["id"].as_u64())
            .context("GitHub accepted the dispatch but returned no run; update `gh` on the worker's machine.")?;
        let url = run["html_url"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("https://github.com/{repository}/actions/runs/{run_id}"));

        execution.id = import_id(repository, run_id);
        execution.idempotency_key = format!("{KEY_PREFIX}{repository}#{run_id}");
        // The import installs the runs GitHub produced.
        execution.slots = Vec::new();
        execution.source = ExecutionSource::Github {
            repository: repository.to_owned(),
            run_id,
            run_attempt: 1,
            url,
            release_control_execution_id: None,
            stack: None,
            status: Some("queued".into()),
            follow: GithubFollow {
                followed: true,
                ..GithubFollow::default()
            },
        };
        execution.state = "running".into();
        execution.updated_at = now();
        {
            let _guard = self.lock.lock().await;
            self.write_execution(&execution).await?;
        }
        self.spawn_follow_github(&execution.id);
        Ok(execution)
    }

    pub(super) fn spawn_follow_github(self: &Arc<Self>, id: &str) {
        let store = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            if let Err(error) = store.follow_github(&id).await {
                tracing::error!(execution_id = %id, error = %format!("{error:#}"), "follow the GitHub run");
            }
        });
    }

    /// Follow the run until it ends, then import it; the execution always
    /// ends `completed` or `failed`.
    async fn follow_github(self: &Arc<Self>, id: &str) -> Result<()> {
        let conclusion = loop {
            let execution = self.read_execution(id).await?;
            let ExecutionSource::Github {
                repository, run_id, ..
            } = &execution.source
            else {
                return Ok(());
            };
            if execution.state == "importing" {
                break None;
            }
            if !execution.active() {
                return Ok(());
            }
            let run = self
                .gh(
                    self.github.api_timeout,
                    &["api", &format!("repos/{repository}/actions/runs/{run_id}")],
                )
                .await
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).context("decode the GitHub run"));
            match run {
                Err(error) => {
                    // A passing network or API failure: the next look retries.
                    tracing::warn!(execution_id = %id, error = %format!("{error:#}"), "read the GitHub run");
                }
                Ok(run) => {
                    let attempt = run["run_attempt"].as_u64().unwrap_or(1) as u32;
                    // After a job re-run GitHub may still answer with the
                    // attempt that ended: wait for the new one.
                    let expected = match &execution.source {
                        ExecutionSource::Github { run_attempt, .. } => *run_attempt,
                        _ => 1,
                    };
                    if attempt >= expected {
                        let status = run["status"].as_str().unwrap_or("queued").to_owned();
                        let jobs = self
                            .gh(
                                self.github.api_timeout,
                                &[
                                    "api",
                                    &format!(
                                        "repos/{repository}/actions/runs/{run_id}/attempts/{attempt}/jobs?per_page=100"
                                    ),
                                ],
                            )
                            .await
                            .ok()
                            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                            .map(|value| jobs_of(&value));
                        let done = status == "completed";
                        let _guard = self.lock.lock().await;
                        let mut execution = self.read_execution(id).await?;
                        let mut changed = done;
                        if let ExecutionSource::Github {
                            status: current,
                            run_attempt,
                            follow,
                            ..
                        } = &mut execution.source
                        {
                            let mut next = follow.clone();
                            next.head_branch = run["head_branch"].as_str().map(str::to_owned);
                            next.head_sha = run["head_sha"].as_str().map(str::to_owned);
                            if let Some(jobs) = jobs {
                                next.jobs = jobs;
                            }
                            if current.as_deref() != Some(status.as_str())
                                || *run_attempt != attempt
                                || *follow != next
                            {
                                *current = Some(status.clone());
                                *run_attempt = attempt;
                                *follow = next;
                                changed = true;
                            }
                        }
                        if done {
                            execution.state = "importing".into();
                        }
                        if changed {
                            execution.updated_at = now();
                            self.write_execution(&execution).await?;
                        }
                        if done {
                            break run["conclusion"].as_str().map(str::to_owned);
                        }
                    }
                }
            }
            tokio::time::sleep(self.github.follow_interval).await;
        };
        let stopped = conclusion
            .filter(|conclusion| conclusion != "success")
            .map(|conclusion| format!("The GitHub run ended {conclusion}."));
        if let Err(error) = self.download_and_install(id, stopped).await {
            tracing::warn!(execution_id = %id, error = %format!("{error:#}"), "GitHub import failed");
            let _guard = self.lock.lock().await;
            let mut execution = self.read_execution(id).await?;
            execution.state = "failed".into();
            execution.error = Some(format!("{error:#}"));
            execution.updated_at = now();
            execution.finished_at = Some(now());
            execution.rerun = None;
            self.write_execution(&execution).await?;
        } else {
            // The import replaced the runs the re-run was for.
            let _guard = self.lock.lock().await;
            let mut execution = self.read_execution(id).await?;
            if execution.rerun.take().is_some() {
                self.write_execution(&execution).await?;
            }
        }
        // Cancelled from the Console: it ends cancelled, with what finished.
        let _guard = self.lock.lock().await;
        let mut execution = self.read_execution(id).await?;
        if execution.cancel_requested && execution.state == "failed" {
            execution.state = "cancelled".into();
            execution.updated_at = now();
            self.write_execution(&execution).await?;
        }
        Ok(())
    }

    /// Run one test of a finished GitHub execution again: re-run its group's
    /// job on GitHub (which redoes the aggregate job), follow the run and
    /// import it again when it ends; the highest attempt counts.
    pub(super) async fn rerun_github(
        self: &Arc<Self>,
        id: &str,
        scenario_id: &str,
    ) -> Result<PlanExecution> {
        let execution = self.read_execution(id).await?;
        let ExecutionSource::Github {
            repository, run_id, ..
        } = execution.source.clone()
        else {
            anyhow::bail!("{id} did not run on GitHub");
        };
        ensure!(
            !execution.active() && execution.state != "importing",
            "This execution is still running on GitHub; run a test again once it ends."
        );
        let group = sequential_groups(&crate::test_plan::embedded()?)
            .into_iter()
            .find(|group| group.iter().any(|scenario| scenario == scenario_id))
            .unwrap_or_else(|| vec![scenario_id.to_owned()]);
        let needle = format!("case-{}", group[0].replace('_', "-"));
        let jobs = self
            .gh(
                self.github.api_timeout,
                &["api", &format!("repos/{repository}/actions/runs/{run_id}/jobs?per_page=100")],
            )
            .await?;
        let jobs: Value = serde_json::from_slice(&jobs).context("decode the run's jobs")?;
        let job_id = jobs_of(&jobs)
            .into_iter()
            .find(|job| job.name.contains(&needle))
            .map(|job| job.id)
            .with_context(|| format!("No job of run #{run_id} runs {scenario_id}."))?;
        self.gh(
            self.github.api_timeout,
            &[
                "api",
                "-X",
                "POST",
                &format!("repos/{repository}/actions/jobs/{job_id}/rerun"),
            ],
        )
        .await
        .context("re-run the job on GitHub")?;

        let _guard = self.lock.lock().await;
        let mut execution = self.read_execution(id).await?;
        if let ExecutionSource::Github {
            run_attempt,
            status,
            follow,
            ..
        } = &mut execution.source
        {
            *run_attempt += 1;
            *status = Some("queued".into());
            follow.followed = true;
            follow.jobs.clear();
        }
        execution.rerun = Some(Rerun {
            scenarios: group,
            runs: Vec::new(),
            started_at: now(),
            state: std::mem::replace(&mut execution.state, "running".into()),
            error: execution.error.take(),
            finished_at: execution.finished_at.take(),
        });
        execution.cancel_requested = false;
        execution.updated_at = now();
        self.write_execution(&execution).await?;
        drop(_guard);
        self.spawn_follow_github(id);
        Ok(execution)
    }
}

/// The account `gh auth status` names ("Logged in to github.com account X").
fn signed_in_account(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (_, rest) = line.split_once("account ")?;
        rest.split_whitespace().next().map(str::to_owned)
    })
}

