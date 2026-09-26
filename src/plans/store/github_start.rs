//! Executions this worker starts on GitHub: it dispatches the exact-stack
//! workflow with the user's `gh`, follows the run every `poll_interval` and,
//! once it ended, imports it as an import picked by hand does. Cancelling
//! cancels the run; running a scenario again re-runs its group's job, which
//! GitHub follows with the finalizer, then imports the run again: the last
//! attempt counts. A worker that restarts follows its running ones again.
use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::{anyhow, bail, ensure, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::docker::dispatch_suite;
use super::github::{import_id, slot, WORKFLOW};
use super::{finish, now, ExecutionSource, PlanExecution, PlanStore, Rerun, Slot};
use crate::artifact;
use crate::plans::stacks;
use crate::test_plan::MasterPlan;

/// The branch the workflow runs from.
const REF: &str = "main";

/// One job of a run's latest attempt, as `gh run view` last showed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct GithubJob {
    #[serde(alias = "databaseId")]
    pub id: u64,
    /// `<campaign> · <group>` for a group's job.
    pub name: String,
    /// GitHub's: `queued`, `waiting`, `in_progress`, `completed`, …
    pub status: String,
    /// Empty until it completed; then `success`, `failure`, `cancelled`, …
    #[serde(default)]
    pub conclusion: String,
    #[serde(default)]
    pub url: String,
}

/// What following a run reads of it.
#[derive(Debug, Deserialize)]
struct RunView {
    status: String,
    #[serde(default)]
    conclusion: String,
    attempt: u32,
    #[serde(default)]
    jobs: Vec<GithubJob>,
}

impl PlanStore {
    /// Dispatch the workflow for an execution validated and materialized
    /// here, and keep it as the execution of the run GitHub created, running
    /// until the run ended and was imported. The inputs go on `gh`'s standard
    /// input; they name no Release Control execution, so nothing is reported
    /// to it.
    pub(super) async fn start_github(
        self: &Arc<Self>,
        mut execution: PlanExecution,
        master: &MasterPlan,
    ) -> Result<PlanExecution> {
        let parameters = execution
            .parameters
            .as_mut()
            .context("a GitHub execution needs its parameters")?;
        let stack = parameters
            .stack
            .as_mut()
            .context("Pick the stack to run on GitHub.")?;
        stack.sha256 = artifact::sha256_bytes(stack.yaml.as_bytes());
        let parameters = &*parameters;
        let stack = parameters.stack.as_ref().context("stack")?;
        let summary = stacks::summarize(&stack.yaml)?;
        let mut inputs = json!({
            "suite": dispatch_suite(parameters, master)?,
            "stack": stack.yaml,
            "model": format!("{}/{}", parameters.provider, parameters.model),
        });
        if let Some(agent) = &parameters.agent {
            inputs["profile"] = json!(agent);
        }
        let warnings = summary
            .warnings
            .iter()
            .map(|warning| format!("Stack {}: {warning}", stack.name))
            .collect::<Vec<_>>();
        let name = stack.name.clone();
        let repository = self.github.repository.clone();
        let output = self
            .gh_with(
                self.github.api_timeout,
                &[
                    "workflow",
                    "run",
                    WORKFLOW,
                    "-R",
                    &repository,
                    "--ref",
                    REF,
                    "--json",
                ],
                Some(inputs.to_string().as_bytes()),
            )
            .await
            .map_err(|error| anyhow!("GitHub did not start the run: {error:#}"))?;
        let run_id = created_run(&output.stdout)
            .or_else(|| created_run(&output.stderr))
            .ok_or_else(|| {
                anyhow!(
                    "GitHub started the run, but `gh` did not say which one; update `gh`, and import the run with Import from GitHub once it ends."
                )
            })?;
        execution.warnings.extend(warnings);
        execution.id = import_id(&repository, run_id);
        // As an import of the run is keyed: importing it later is this execution.
        execution.idempotency_key = format!("github:{repository}#{run_id}");
        // GitHub materializes the suite again; until the import, a slot per
        // scenario of each group follows its group's job.
        execution.slots = execution
            .slots
            .iter()
            .map(|placeholder| {
                slot(
                    placeholder.round,
                    &placeholder.group_id,
                    &placeholder.scenario_id,
                )
            })
            .collect();
        execution.source = ExecutionSource::Github {
            url: format!("https://github.com/{repository}/actions/runs/{run_id}"),
            repository,
            run_id,
            run_attempt: 1,
            release_control_execution_id: None,
            stack: Some(name),
            status: Some("queued".into()),
            jobs: Vec::new(),
        };
        self.write_execution(&execution).await?;
        self.spawn_github(&execution.id);
        Ok(execution)
    }

    /// Follow a run in the background until it ended and was imported;
    /// whatever stops that ends the execution with the reason.
    pub(super) fn spawn_github(self: &Arc<Self>, id: &str) {
        let store = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            if let Err(error) = store.follow_github(&id).await {
                let reason = format!("{error:#}");
                tracing::warn!(execution_id = %id, error = %reason, "GitHub execution stopped");
                if let Err(error) = store.end_github(&id, Some(reason)).await {
                    tracing::error!(execution_id = %id, error = %format!("{error:#}"), "cannot record the end of a GitHub execution");
                }
            }
        });
    }

    async fn follow_github(&self, id: &str) -> Result<()> {
        loop {
            let execution = self.read_execution(id).await?;
            let ExecutionSource::Github {
                repository,
                run_id,
                run_attempt,
                ..
            } = &execution.source
            else {
                return Ok(());
            };
            if !execution.active() {
                return Ok(());
            }
            match self.view_run(repository, *run_id).await {
                // Cancelled, it ends even when GitHub no longer shows the run.
                Err(error) if execution.cancel_requested => bail!(
                    "Cancelled here while GitHub could not show the run ({error:#}); import it with Import from GitHub once it ended."
                ),
                // A passing failure: the next look tries again; meanwhile it says so.
                Err(error) => {
                    let note = format!(
                        "Could not read the run on GitHub; trying again in {:?}. {error:#}",
                        self.github.poll_interval
                    );
                    self.update_github(id, |execution| execution.error = Some(note))
                        .await?;
                }
                // Right after a job re-run, GitHub may still show the attempt
                // that ended.
                Ok(view) if view.attempt < *run_attempt => {}
                Ok(view) => {
                    let done = view.status == "completed";
                    let conclusion = view.conclusion.clone();
                    self.update_github(id, |execution| {
                        // Until the import, the slots follow their group's job.
                        if execution
                            .slots
                            .iter()
                            .all(|slot| slot.execution_id.is_empty())
                        {
                            follow_jobs(&mut execution.slots, &view.jobs);
                        }
                        if let ExecutionSource::Github {
                            run_attempt,
                            status,
                            jobs,
                            ..
                        } = &mut execution.source
                        {
                            (*run_attempt, *status, *jobs) =
                                (view.attempt, Some(view.status), view.jobs);
                        }
                        execution.error = None;
                    })
                    .await?;
                    if done {
                        return self.import_github(id, &conclusion).await;
                    }
                }
            }
            tokio::time::sleep(self.github.poll_interval).await;
        }
    }

    /// Import the run that ended. A re-run cancelled before it ended imports
    /// nothing: the attempt before it still counts.
    async fn import_github(&self, id: &str, conclusion: &str) -> Result<()> {
        let execution = self.read_execution(id).await?;
        if execution.cancel_requested && execution.rerun.is_some() {
            return self.end_github(id, None).await;
        }
        // Cancelled from here, it ends cancelled with what finished.
        let stopped = (!execution.cancel_requested && !matches!(conclusion, "success" | "failure"))
            .then(|| format!("The run ended {conclusion} on GitHub."));
        if let Err(error) = self.download_and_install(id, stopped).await {
            return self
                .end_github(
                    id,
                    Some(format!(
                        "The run ended {conclusion} on GitHub; importing its results failed: {error:#}"
                    )),
                )
                .await;
        }
        Ok(())
    }

    /// End an execution that will import nothing more; `reason` says why.
    async fn end_github(&self, id: &str, reason: Option<String>) -> Result<()> {
        let _guard = self.lock.lock().await;
        let mut execution = self.read_execution(id).await?;
        finish(&mut execution, reason, &self.root)?;
        self.write_execution(&execution).await
    }

    /// Change an execution under the lock; written only when it changed.
    async fn update_github(&self, id: &str, change: impl FnOnce(&mut PlanExecution)) -> Result<()> {
        let _guard = self.lock.lock().await;
        let mut execution = self.read_execution(id).await?;
        let before = serde_json::to_value(&execution)?;
        change(&mut execution);
        if serde_json::to_value(&execution)? != before {
            execution.updated_at = now();
            self.write_execution(&execution).await?;
        }
        Ok(())
    }

    async fn view_run(&self, repository: &str, run_id: u64) -> Result<RunView> {
        let output = self
            .gh(
                self.github.api_timeout,
                &[
                    "run",
                    "view",
                    &run_id.to_string(),
                    "-R",
                    repository,
                    "--json",
                    "status,conclusion,attempt,jobs",
                ],
            )
            .await?;
        serde_json::from_slice(&output).context("decode the run `gh run view` showed")
    }

    /// Ask GitHub to cancel the run of a GitHub execution; its follower
    /// imports what finished once the run ended.
    pub(super) async fn cancel_github(&self, execution: &PlanExecution) -> Result<()> {
        if let ExecutionSource::Github {
            repository, run_id, ..
        } = &execution.source
        {
            self.gh(
                self.github.api_timeout,
                &["run", "cancel", &run_id.to_string(), "-R", repository],
            )
            .await
            .map_err(|error| anyhow!("GitHub did not cancel the run: {error:#}"))?;
        }
        Ok(())
    }

    /// Run a scenario of a finished GitHub execution again: GitHub re-runs
    /// its group's job, then the finalizer, as the run's next attempt, which
    /// this worker follows and imports. The last attempt counts. A run
    /// Release Control dispatched reports to it, so it is re-run from there.
    pub(super) async fn rerun_github(
        self: &Arc<Self>,
        id: &str,
        scenario_id: &str,
    ) -> Result<PlanExecution> {
        let execution = self.read_execution(id).await?;
        let ExecutionSource::Github {
            repository,
            run_id,
            url,
            release_control_execution_id,
            ..
        } = &execution.source
        else {
            bail!("execution {id} did not run on GitHub");
        };
        ensure!(
            release_control_execution_id.is_none(),
            "Release Control dispatched this run and reads its reports: run {scenario_id} again from Release Control, which re-runs its job on GitHub ({url}), then import the run again."
        );
        ensure!(
            !execution.active() && execution.state != "importing",
            "Only a finished execution can run a scenario again."
        );
        let groups = execution
            .slots
            .iter()
            .filter(|slot| slot.scenario_id == scenario_id)
            .map(|slot| (slot.round, slot.group_id.clone()))
            .collect::<BTreeSet<_>>();
        ensure!(
            !groups.is_empty(),
            "This execution did not run the scenario '{scenario_id}'."
        );
        // ponytail: one job per re-run, as GitHub re-runs one at a time; a
        // scenario of several rounds would need them re-run one after another.
        ensure!(
            groups.len() == 1,
            "{scenario_id} ran in {} rounds, a job each, and GitHub re-runs one job at a time: re-run them on GitHub ({url}) one after another, then import the run again.",
            groups.len()
        );
        let (round, group) = groups.into_iter().next().context("the scenario's group")?;
        let view = self.view_run(repository, *run_id).await?;
        let job = view
            .jobs
            .iter()
            .find(|job| group_job(job, round, &group))
            .with_context(|| {
                format!(
                    "The run's latest attempt has no job for {group}; re-run it on GitHub ({url})."
                )
            })?;
        self.gh(
            self.github.api_timeout,
            &[
                "run",
                "rerun",
                &run_id.to_string(),
                "-R",
                repository,
                "--job",
                &job.id.to_string(),
            ],
        )
        .await
        .map_err(|error| anyhow!("GitHub did not re-run the job: {error:#}"))?;

        let _guard = self.lock.lock().await;
        let mut execution = self.read_execution(id).await?;
        let mut scenarios = Vec::<String>::new();
        for slot in execution
            .slots
            .iter()
            .filter(|slot| slot.round == round && slot.group_id == group)
        {
            if !scenarios.contains(&slot.scenario_id) {
                scenarios.push(slot.scenario_id.clone());
            }
        }
        if scenarios.len() > 1 {
            let note = format!(
                "{} run only together, in this order; running one again runs the whole group.",
                scenarios.join(" then ")
            );
            if !execution.warnings.contains(&note) {
                execution.warnings.push(note);
            }
        }
        if let ExecutionSource::Github {
            run_attempt,
            status,
            ..
        } = &mut execution.source
        {
            // The attempt the follower waits for.
            (*run_attempt, *status) = (view.attempt + 1, Some("queued".into()));
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
        drop(_guard);
        self.spawn_github(id);
        Ok(execution)
    }
}

/// The run `gh workflow run` says it created: `…/actions/runs/<id>`.
fn created_run(output: &[u8]) -> Option<u64> {
    let text = String::from_utf8_lossy(output);
    let (_, rest) = text.split_once("/actions/runs/")?;
    rest.split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// Whether a job runs this round's group: `<suite>-r<NN> · <group>`.
fn group_job(job: &GithubJob, round: u32, group: &str) -> bool {
    job.name.ends_with(&format!("-r{round:02} · {group}"))
}

/// Slots before the import show their group's job.
fn follow_jobs(slots: &mut [Slot], jobs: &[GithubJob]) {
    for slot in slots {
        let Some(job) = jobs
            .iter()
            .find(|job| group_job(job, slot.round, &slot.group_id))
        else {
            continue;
        };
        slot.state = match job.status.as_str() {
            "completed" => "finished",
            "in_progress" => "running",
            _ => "pending",
        }
        .into();
        slot.error = (!matches!(job.conclusion.as_str(), "" | "success"))
            .then(|| format!("Its job ended {} on GitHub.", job.conclusion));
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    use serde_json::{json, Value};

    use super::super::tests::{
        exact_stack_bundle, fake_gh, manager_with_gh, suite_parameters, FakeRunner,
    };
    use super::super::{github, ExecutionParameters, ExecutionStack, Runner, Where};
    use super::*;

    /// Stands in for `gh`: it logs each call and answers from its folder:
    /// `refuse` makes the dispatch fail with its text, `view.json` is the run
    /// (no file: GitHub does not answer), `artifacts/<name>/` what the run
    /// uploaded.
    const GH: &str = r#"dir="$(dirname "$0")"
echo "$*" >>"$dir/calls"
case "$1 $2" in
  "workflow run")
    cat >"$dir/inputs.json"
    if [ -f "$dir/refuse" ]; then cat "$dir/refuse" >&2; exit 1; fi
    [ -f "$dir/silent" ] || echo "https://github.com/o/r/actions/runs/77" ;;
  "run view")
    [ -f "$dir/view.json" ] || { echo "HTTP 502: Bad Gateway" >&2; exit 1; }
    cat "$dir/view.json" ;;
  "run cancel"|"run rerun")
    if [ -f "$dir/refuse" ]; then cat "$dir/refuse" >&2; exit 1; fi ;;
  "api repos/o/r/actions/runs/77")
    echo '{"id":77,"run_attempt":1,"display_title":"E2E run · pr · provider/model"}' ;;
  "api --paginate") ls "$dir/artifacts" ;;
  "run download") cp -a "$dir/artifacts/$7" "$9" ;;
  *) echo "unexpected: $*" >&2; exit 1 ;;
esac"#;

    struct Github {
        root: tempfile::TempDir,
        data: PathBuf,
        runner: Arc<FakeRunner>,
        store: Arc<PlanStore>,
    }

    impl Github {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let data = root.path().join("data");
            let runner = Arc::new(FakeRunner::new(data.clone()));
            let gh = github::GithubCli {
                repository: "o/r".into(),
                api_timeout: Duration::from_secs(10),
                download_timeout: Duration::from_secs(10),
                poll_interval: Duration::from_millis(20),
                ..fake_gh(root.path(), GH)
            };
            let store = manager_with_gh(&data, runner.clone(), gh);
            Self {
                root,
                data,
                runner,
                store,
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.root.path().join(name)
        }

        fn calls(&self) -> Vec<String> {
            fs::read_to_string(self.path("calls"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        /// The run as GitHub shows it now.
        fn show(&self, attempt: u32, status: &str, conclusion: &str, jobs: &[(u64, &str, &str)]) {
            let jobs = jobs
                .iter()
                .map(|(id, name, conclusion)| {
                    json!({"databaseId": id, "name": name,
                        "status": if conclusion.is_empty() { "in_progress" } else { "completed" },
                        "conclusion": conclusion, "url": format!("https://github.com/o/r/actions/runs/77/job/{id}")})
                })
                .collect::<Vec<_>>();
            artifact::write_atomic(
                &self.path("view.json"),
                json!({"status": status, "conclusion": conclusion, "attempt": attempt, "jobs": jobs})
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();
        }

        /// What attempt `attempt` of run 77 uploads: its contract once, then
        /// its root bundle; its native run's id.
        fn upload(&self, attempt: u32, observed: &str) -> String {
            let (bundle, contract, native) = exact_stack_bundle(
                &self.path(&format!("attempt-{attempt}")),
                &format!("gh:77:{attempt}"),
                observed,
            );
            // As the workflow states an execution it dispatched without one.
            fs::write(
                contract.join("execution.json"),
                json!({"execution_id": null, "suite": {"id": "smoke"}, "stack": "inline",
                    "model": "provider/model", "profile": null})
                .to_string(),
            )
            .unwrap();
            let artifacts = self.path("artifacts");
            fs::create_dir_all(&artifacts).unwrap();
            let contract_name = artifacts.join("e2e-contract-77-gh-1");
            if !contract_name.exists() {
                fs::rename(contract, contract_name).unwrap();
            }
            fs::rename(
                bundle,
                artifacts.join(format!("e2e-observation-77-gh-{attempt}")),
            )
            .unwrap();
            native
        }

        async fn until(&self, id: &str, done: impl Fn(&PlanExecution) -> bool) -> PlanExecution {
            tokio::time::timeout(Duration::from_secs(60), async {
                loop {
                    let execution = self.store.read_execution(id).await.unwrap();
                    if done(&execution) {
                        return execution;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the GitHub execution did not get there")
        }

        /// Wait until the follower looked at the run `looks` more times.
        async fn looked(&self, looks: usize) {
            let count = || {
                self.calls()
                    .iter()
                    .filter(|c| c.starts_with("run view"))
                    .count()
            };
            let target = count() + looks;
            tokio::time::timeout(Duration::from_secs(60), async {
                while count() < target {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the follower stopped looking");
        }
    }

    /// `pr` on the repository's default stack, on GitHub.
    fn on_github() -> ExecutionParameters {
        ExecutionParameters {
            r#where: Where::Github,
            stack: Some(ExecutionStack {
                name: "default".into(),
                yaml: stacks::REPOSITORY[0].1.into(),
                sha256: String::new(),
            }),
            ..suite_parameters("pr")
        }
    }

    fn github_source(execution: &PlanExecution) -> (u32, Option<&str>, &[GithubJob]) {
        match &execution.source {
            ExecutionSource::Github {
                run_attempt,
                status,
                jobs,
                ..
            } => (*run_attempt, status.as_deref(), jobs),
            other => panic!("not a GitHub execution: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_github_execution_dispatches_follows_its_jobs_and_imports_the_run_once_it_ends() {
        let github = Github::new();
        let started = github
            .store
            .start_execution(on_github(), "On GitHub")
            .await
            .unwrap();
        // The run GitHub created is the execution, as its import would be.
        assert_eq!(started.id, github::import_id("o/r", 77));
        assert_eq!(started.idempotency_key, "github:o/r#77");
        assert_eq!(started.state, "running");
        let ExecutionSource::Github { url, stack, .. } = &started.source else {
            panic!("{:?}", started.source);
        };
        assert_eq!(url, "https://github.com/o/r/actions/runs/77");
        assert_eq!(stack.as_deref(), Some("default"));
        assert_eq!(
            github.calls()[0],
            "workflow run exact-stack-e2e.yml -R o/r --ref main --json"
        );
        // A reviewed suite by its id, the stack's YAML, no Release Control
        // execution and, without one, no profile.
        let inputs: Value =
            serde_json::from_slice(&fs::read(github.path("inputs.json")).unwrap()).unwrap();
        assert_eq!(
            inputs,
            json!({"suite": "pr", "stack": stacks::REPOSITORY[0].1, "model": "provider/model"})
        );
        // Until the import, one placeholder slot per scenario of each group.
        assert_eq!(started.slots.len(), 4);
        assert!(started
            .slots
            .iter()
            .all(|slot| slot.state == "pending" && slot.execution_id.is_empty()));

        // GitHub does not answer: the execution says so and keeps following.
        let failing = github
            .until(&started.id, |execution| execution.error.is_some())
            .await;
        assert_eq!(failing.state, "running");
        assert!(failing.error.as_deref().unwrap().contains("HTTP 502"));

        // Its slots follow their group's job.
        github.show(
            1,
            "in_progress",
            "",
            &[
                (1, "Materialize the suite and assemble the stack", "success"),
                (2, "pr-r01 · case-minimal-path", ""),
                (3, "pr-r01 · case-persistent-state", "failure"),
            ],
        );
        let running = github
            .until(&started.id, |execution| execution.error.is_none())
            .await;
        let states = running
            .slots
            .iter()
            .map(|slot| {
                (
                    slot.group_id.as_str(),
                    slot.state.as_str(),
                    slot.error.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        assert!(states.contains(&("case-minimal-path", "running", None)));
        assert!(states.contains(&(
            "case-persistent-state",
            "finished",
            Some("Its job ended failure on GitHub.")
        )));
        assert!(states.contains(&("case-shell-coder-sandbox", "pending", None)));
        let (_, status, jobs) = github_source(&running);
        assert_eq!(status, Some("in_progress"));
        assert_eq!(jobs.len(), 3);

        // Ended: imported with no button, as an import picked by hand is.
        let native = github.upload(1, "1.8.8");
        github.show(
            1,
            "completed",
            "success",
            &[(4, "smoke-r01 · case-context", "success")],
        );
        let done = github
            .until(&started.id, |execution| !execution.active())
            .await;
        assert_eq!(done.state, "completed", "{:?}", done.error);
        assert_eq!(done.slots[0].execution_id, native);
        assert!(github.runner.record(&native).await.is_some());
        assert!(done.measurements.is_some());
        assert_eq!(done.label.as_deref(), Some("On GitHub"));
        let parameters = done.parameters.as_ref().unwrap();
        assert_eq!(parameters.r#where, Where::Github);
        // Named as picked; the YAML its contract recorded, to run again as is.
        let stack = parameters.stack.as_ref().unwrap();
        assert_eq!(stack.name, "default");
        assert_eq!(
            stack.yaml,
            fs::read_to_string(github.path("artifacts/e2e-contract-77-gh-1/stack.yaml")).unwrap()
        );
        let ExecutionSource::Github {
            release_control_execution_id,
            ..
        } = &done.source
        else {
            panic!("{:?}", done.source);
        };
        assert_eq!(*release_control_execution_id, None);
        assert!(github.data.join(&native).join("results.json").is_file());
        // The follower stopped with it.
        let looks = github.calls().len();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(github.calls().len(), looks);
    }

    #[tokio::test]
    async fn a_scenario_runs_again_as_its_jobs_next_attempt_and_a_cancelled_one_keeps_the_last() {
        let github = Github::new();
        let id = github
            .store
            .start_execution(on_github(), "")
            .await
            .unwrap()
            .id;
        github.upload(1, "1.8.8");
        let jobs = [
            (
                11,
                "Materialize the suite and assemble the stack",
                "success",
            ),
            (12, "smoke-r01 · case-context", "success"),
            (13, "smoke-r01 · case-registry", "failure"),
        ];
        github.show(1, "completed", "failure", &jobs);
        let first = github.until(&id, |execution| !execution.active()).await;
        assert_eq!(first.state, "completed", "{:?}", first.error);

        let rerun = github
            .store
            .rerun_scenario(&id, "context_pressure")
            .await
            .unwrap();
        assert!(github
            .calls()
            .contains(&"run rerun 77 -R o/r --job 12".to_owned()));
        assert_eq!(rerun.state, "running");
        assert_eq!(
            rerun.rerun.as_ref().unwrap().scenarios,
            ["context_pressure"]
        );
        assert_eq!(github_source(&rerun).0, 2);
        // GitHub still shows the attempt that ended: nothing is imported.
        github.looked(2).await;
        let waiting = github.store.read_execution(&id).await.unwrap();
        assert_eq!(waiting.state, "running");
        assert_eq!(
            github
                .calls()
                .iter()
                .filter(|call| call.starts_with("run download"))
                .count(),
            2
        );
        // The next attempt ended: imported again, the last attempt counts.
        let native = github.upload(2, "1.8.9");
        github.show(2, "completed", "success", &jobs);
        let again = github
            .until(&id, |execution| {
                !execution.active() && execution.rerun.is_none()
            })
            .await;
        assert_eq!(again.state, "completed", "{:?}", again.error);
        assert_eq!(again.slots[0].execution_id, native);
        assert_eq!(again.stack[0].observed.as_deref(), Some("1.8.9"));
        assert_eq!(github_source(&again).0, 2);

        // Run again and cancelled before it ended: the run is cancelled on
        // GitHub and the attempt before it still counts.
        let downloads = |calls: Vec<String>| {
            calls
                .iter()
                .filter(|call| call.starts_with("run download"))
                .count()
        };
        github
            .store
            .rerun_scenario(&id, "context_pressure")
            .await
            .unwrap();
        github.show(3, "in_progress", "", &jobs[..2]);
        github.looked(1).await;
        let cancelling = github.store.cancel(&id).await.unwrap();
        assert_eq!(cancelling["state"], "cancelling");
        assert!(github.calls().contains(&"run cancel 77 -R o/r".to_owned()));
        let before = downloads(github.calls());
        github.upload(3, "2.0.0");
        github.show(3, "completed", "cancelled", &jobs);
        let kept = github
            .until(&id, |execution| {
                !execution.active() && execution.rerun.is_none()
            })
            .await;
        assert_eq!(kept.state, "completed");
        assert!(kept.warnings.contains(
            &"Running context_pressure again was cancelled; what it did not run keeps its previous attempt."
                .to_owned()
        ));
        assert_eq!(kept.slots[0].execution_id, native);
        assert_eq!(kept.stack[0].observed.as_deref(), Some("1.8.9"));
        assert_eq!(downloads(github.calls()), before);
    }

    #[tokio::test]
    async fn cancelling_cancels_the_run_and_imports_what_finished() {
        let github = Github::new();
        let id = github
            .store
            .start_execution(on_github(), "")
            .await
            .unwrap()
            .id;
        github.show(1, "in_progress", "", &[]);
        github.looked(1).await;
        // Importing the run by hand while it runs starts no second import.
        let (running, started) = github.store.begin_github_import("o/r", 77).await.unwrap();
        assert!(!started);
        assert_eq!(running.state, "running");
        github.store.cancel(&id).await.unwrap();
        assert!(github.calls().contains(&"run cancel 77 -R o/r".to_owned()));
        assert_eq!(
            github.store.read_execution(&id).await.unwrap().state,
            "cancelling"
        );
        // The finalizer still ran: what finished is imported.
        let native = github.upload(1, "1.8.8");
        github.show(1, "completed", "cancelled", &[]);
        let cancelled = github.until(&id, |execution| !execution.active()).await;
        assert_eq!(cancelled.state, "cancelled");
        assert_eq!(cancelled.slots[0].execution_id, native);
        assert!(github.runner.record(&native).await.is_some());
    }

    #[tokio::test]
    async fn a_cancel_github_cannot_carry_out_still_ends_the_execution() {
        let github = Github::new();
        let id = github
            .store
            .start_execution(on_github(), "")
            .await
            .unwrap()
            .id;
        // GitHub no longer shows the run, nor cancels it.
        github.looked(1).await;
        fs::write(github.path("refuse"), "HTTP 404: Not Found").unwrap();
        let error = github.store.cancel(&id).await.unwrap_err().to_string();
        assert!(
            error.starts_with("GitHub did not cancel the run: "),
            "{error}"
        );
        assert!(error.contains("HTTP 404"), "{error}");
        let ended = github.until(&id, |execution| !execution.active()).await;
        assert_eq!(ended.state, "cancelled");
        assert!(ended
            .error
            .as_deref()
            .unwrap()
            .contains("import it with Import from GitHub"));
    }

    #[tokio::test]
    async fn a_restarted_worker_follows_its_github_executions_again() {
        let github = Github::new();
        // As a worker that stopped while GitHub ran it left it; the run
        // ended since.
        let execution = PlanExecution {
            id: github::import_id("o/r", 77),
            idempotency_key: "github:o/r#77".into(),
            label: None,
            parameters: Some(on_github()),
            source: ExecutionSource::Github {
                repository: "o/r".into(),
                run_id: 77,
                run_attempt: 1,
                url: "https://github.com/o/r/actions/runs/77".into(),
                release_control_execution_id: None,
                stack: Some("default".into()),
                status: Some("in_progress".into()),
                jobs: Vec::new(),
            },
            stack: Vec::new(),
            warnings: Vec::new(),
            state: "running".into(),
            started_at: now(),
            updated_at: now(),
            finished_at: None,
            cancel_requested: false,
            error: None,
            slots: vec![slot(1, "case-context", "context_pressure")],
            measurements: None,
            system_under_test: None,
            rerun: None,
        };
        github.store.write_execution(&execution).await.unwrap();
        let native = github.upload(1, "1.8.8");
        github.show(1, "completed", "success", &[]);
        github.store.reconcile().await.unwrap();
        let done = github
            .until(&execution.id, |execution| !execution.active())
            .await;
        assert_eq!(done.state, "completed", "{:?}", done.error);
        assert_eq!(done.slots[0].execution_id, native);
    }

    #[tokio::test]
    async fn a_refused_or_unsigned_dispatch_says_why_and_keeps_nothing() {
        let github = Github::new();
        for (refusal, expected) in [
            (
                "could not create workflow dispatch event: HTTP 403: Resource not accessible by integration",
                "HTTP 403",
            ),
            (
                "To get started with GitHub CLI, please run:  gh auth login",
                "please run:  gh auth login",
            ),
        ] {
            fs::write(github.path("refuse"), refusal).unwrap();
            let error = github
                .store
                .start_execution(on_github(), "")
                .await
                .unwrap_err()
                .to_string();
            assert!(error.starts_with("GitHub did not start the run: "), "{error}");
            assert!(error.contains(expected), "{error}");
            assert!(error.contains("gh auth status"), "{error}");
        }
        fs::remove_file(github.path("refuse")).unwrap();
        fs::write(github.path("silent"), "").unwrap();
        let error = github
            .store
            .start_execution(on_github(), "")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("did not say which one"), "{error}");
        assert!(github.store.executions().await.unwrap().is_empty());
        // Without a stack there is nothing to dispatch.
        let error = github
            .store
            .start_execution(
                ExecutionParameters {
                    stack: None,
                    ..on_github()
                },
                "",
            )
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(error, "Pick the stack to run on GitHub.");
    }

    #[tokio::test]
    async fn a_scenario_of_several_rounds_is_rerun_on_github_by_hand() {
        let github = Github::new();
        let mut execution = github.store.start_execution(on_github(), "").await.unwrap();
        github.show(1, "completed", "success", &[]);
        github
            .until(&execution.id, |execution| !execution.active())
            .await;
        execution = github.store.read_execution(&execution.id).await.unwrap();
        execution.slots = vec![
            slot(1, "case-minimal-path", "minimal_path"),
            slot(2, "case-minimal-path", "minimal_path"),
        ];
        github.store.write_execution(&execution).await.unwrap();
        let error = github
            .store
            .rerun_scenario(&execution.id, "minimal_path")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("ran in 2 rounds"), "{error}");
        assert!(!github
            .calls()
            .iter()
            .any(|call| call.starts_with("run rerun")));
    }

    #[test]
    fn the_created_run_is_read_from_what_gh_printed() {
        assert_eq!(
            created_run(b"https://github.com/o/r/actions/runs/36143945941\n"),
            Some(36_143_945_941)
        );
        assert_eq!(
            created_run(
                "✓ Created workflow_dispatch event for exact-stack-e2e.yml at main\nhttps://github.com/o/r/actions/runs/7/\n".as_bytes()
            ),
            Some(7)
        );
        assert_eq!(
            created_run(b"Created workflow_dispatch event for exact-stack-e2e.yml at main\n"),
            None
        );
        let job = |name: &str| GithubJob {
            id: 1,
            name: name.into(),
            status: "queued".into(),
            conclusion: String::new(),
            url: String::new(),
        };
        assert!(group_job(
            &job("pr-r01 · case-minimal-path"),
            1,
            "case-minimal-path"
        ));
        assert!(!group_job(
            &job("pr-r02 · case-minimal-path"),
            1,
            "case-minimal-path"
        ));
        assert!(!group_job(
            &job("pr-r01 · case-minimal-path-2"),
            1,
            "case-minimal-path"
        ));
    }
}
