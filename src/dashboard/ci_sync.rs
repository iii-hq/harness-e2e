//! CI sync: once turned on for this worker, it imports the exact-stack
//! workflow's runs in the background through the path **Import** takes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::Notify;

use super::controller::Controller;
use crate::plans::store::{
    github_started, import_id, native_runs, rate_limited, ExecutionSource, PlanExecution,
    NOT_SIGNED_IN,
};

/// Days GitHub keeps a run's artifacts.
const WINDOW_DAYS: i64 = 90;
// shortcut: KeepingUp lists runs created in the last 7 days, so a re-run of an older run comes only by Import; widen KEEP_UP_DAYS if people re-run older runs
const KEEP_UP_DAYS: i64 = 7;
const PER_PAGE: u32 = 100;
/// Failed imports of a run as GitHub last updated it, rate limits aside.
const MAX_ATTEMPTS: u32 = 3;
// shortcut: pauses the limit's whole hour instead of reading its reset from `gh api rate_limit`; read it if backfills stall
const RATE_LIMIT_PAUSE: Duration = Duration::from_secs(3600);

/// The latest releases first (`quick`), then every run GitHub keeps
/// (`backfill`), then the recent runs at each check (`keeping_up`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum CiSyncPhase {
    #[default]
    Off,
    Quick,
    Backfill,
    KeepingUp,
}

/// What CI sync did and does next; times are RFC 3339, counters count since
/// the worker started.
#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema)]
pub(super) struct CiSyncStatus {
    pub enabled: bool,
    pub phase: CiSyncPhase,
    /// Whether `gh` was signed in at the last check; null before one, and
    /// when `gh` could not tell (missing, or not answering).
    pub signed_in: Option<bool>,
    /// What stopped the last check short: `gh` not ready, a listing that
    /// failed. A run whose import failed only counts in `failed`.
    pub message: Option<String>,
    pub checked_at: Option<String>,
    pub next_check_at: Option<String>,
    pub imported: usize,
    pub failed: usize,
    /// Runs that left no results (no e2e-contract artifact), skipped until
    /// GitHub runs them again; their executions are deleted.
    pub skipped: usize,
    /// Runs the current pass has yet to import.
    pub remaining: usize,
    /// Until when a GitHub rate limit holds every check.
    pub paused_until: Option<String>,
}

/// The switch, kept in `ci-sync.json` in the data directory, and the status.
pub(super) struct CiSync {
    path: PathBuf,
    status: Mutex<CiSyncStatus>,
    wake: Arc<Notify>,
}

impl CiSync {
    /// Off when the file is missing or unreadable.
    pub(super) fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("ci-sync.json");
        let enabled = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|switch| switch["enabled"].as_bool())
            .unwrap_or(false);
        let phase = if enabled {
            CiSyncPhase::Quick
        } else {
            CiSyncPhase::Off
        };
        Self {
            path,
            status: Mutex::new(CiSyncStatus {
                enabled,
                phase,
                ..CiSyncStatus::default()
            }),
            wake: Arc::new(Notify::new()),
        }
    }

    pub(super) fn status(&self) -> CiSyncStatus {
        self.lock().clone()
    }

    fn lock(&self) -> MutexGuard<'_, CiSyncStatus> {
        self.status
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// The next check comes after `wait`.
    fn schedule(&self, wait: Duration) -> Duration {
        let mut status = self.lock();
        if status.enabled {
            status.next_check_at = Some(rfc3339(Utc::now() + wait));
        }
        wait
    }

    /// No `gh` call until the rate limit's hour is over; the run that hit it
    /// is neither failed nor tried.
    fn pause(&self) -> Duration {
        self.lock().paused_until = Some(rfc3339(Utc::now() + RATE_LIMIT_PAUSE));
        self.schedule(RATE_LIMIT_PAUSE)
    }
}

impl Controller {
    pub(super) fn ci_sync_status(&self) -> CiSyncStatus {
        self.ci_sync.status()
    }

    /// Turn CI sync on or off and keep the switch; turned on, it starts with
    /// the latest releases at once.
    pub(super) fn ci_sync_set(&self, enabled: bool) -> Result<CiSyncStatus> {
        let mut status = self.ci_sync.lock();
        crate::artifact::write_atomic(
            &self.ci_sync.path,
            &serde_json::to_vec(&json!({"enabled": enabled}))?,
        )?;
        if !enabled {
            (status.phase, status.next_check_at) = (CiSyncPhase::Off, None);
        } else if !status.enabled {
            status.phase = CiSyncPhase::Quick;
            self.ci_sync.wake.notify_one();
        }
        status.enabled = enabled;
        Ok(status.clone())
    }

    /// Check GitHub now; off, the check does nothing.
    pub(super) fn ci_sync_check(&self) -> CiSyncStatus {
        self.ci_sync.wake.notify_one();
        self.ci_sync.status()
    }
}

/// Check, then wait, for as long as the controller lives; turning CI sync on
/// or `ci-sync-check` cuts the wait short.
pub(super) fn spawn(controller: &Arc<Controller>) {
    let weak = Arc::downgrade(controller);
    let wake = controller.ci_sync.wake.clone();
    tokio::spawn(async move {
        loop {
            let Some(controller) = weak.upgrade() else {
                return;
            };
            let wait = tick(&controller).await;
            drop(controller);
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = wake.notified() => {}
            }
        }
    });
}

/// One check; answers the wait before the next.
pub(super) async fn tick(controller: &Arc<Controller>) -> Duration {
    let sync = &controller.ci_sync;
    let interval = controller.plan_store.sync_interval();
    let status = sync.status();
    if !status.enabled {
        let mut status = sync.lock();
        (status.phase, status.next_check_at) = (CiSyncPhase::Off, None);
        return interval;
    }
    if let Some(left) = status.paused_until.as_deref().and_then(left_until) {
        return left;
    }
    let github = controller.github_status().await;
    let ready = github["ready"] == json!(true);
    let message = github["message"].as_str().unwrap_or_default();
    {
        let mut status = sync.lock();
        // Only a signed-out `gh` asks for `gh auth login`; a missing or
        // stuck one says so in its message.
        status.signed_in = if ready {
            Some(true)
        } else {
            (message == NOT_SIGNED_IN).then_some(false)
        };
        status.paused_until = None;
        status.message = (!ready).then(|| message.replace(", then reopen this dialog", ""));
    }
    if !ready {
        return sync.schedule(interval);
    }
    match pass(controller, status.phase, interval).await {
        Ok(wait) => wait,
        Err(error) => {
            let error = format!("{error:#}");
            if rate_limited(&error) {
                return sync.pause();
            }
            sync.lock().message = Some(error);
            sync.schedule(interval)
        }
    }
}

/// A pass of `phase` over the runs GitHub lists; answers the wait.
async fn pass(
    controller: &Arc<Controller>,
    phase: CiSyncPhase,
    interval: Duration,
) -> Result<Duration> {
    let mut seen = HashSet::new();
    let (rows, next, wait) = match phase {
        CiSyncPhase::Off => return Ok(interval),
        // Release Control runs a day at a time, until the two newest Harness
        // releases each have one; then the newest other runs listed, so the
        // strip outside Release Control has a count.
        CiSyncPhase::Quick => {
            let mut others = Vec::new();
            for n in 0..=WINDOW_DAYS {
                if controller.release_overview().await?.verdict.is_some() {
                    break;
                }
                let (rows, other): (Vec<_>, Vec<_>) = window(controller, n, n, &mut seen)
                    .await?
                    .into_iter()
                    .partition(release_control);
                others.extend(other);
                if let Some(wait) = import(controller, rows).await? {
                    return Ok(wait);
                }
            }
            others.truncate(PER_PAGE as usize);
            (others, CiSyncPhase::Backfill, Duration::ZERO)
        }
        CiSyncPhase::Backfill => {
            let mut rows = Vec::new();
            for n in 0..=WINDOW_DAYS {
                rows.extend(window(controller, n, n, &mut seen).await?);
            }
            (rows, CiSyncPhase::KeepingUp, interval)
        }
        CiSyncPhase::KeepingUp => (
            window(controller, KEEP_UP_DAYS, 0, &mut seen).await?,
            CiSyncPhase::KeepingUp,
            interval,
        ),
    };
    if let Some(wait) = import(controller, rows).await? {
        return Ok(wait);
    }
    {
        let mut status = controller.ci_sync.lock();
        status.checked_at = Some(rfc3339(Utc::now()));
        if status.enabled {
            status.phase = next;
        }
    }
    Ok(controller.ci_sync.schedule(wait))
}

/// The runs created from `from` to `to` days ago, newest first, but those
/// this pass listed already.
async fn window(
    controller: &Controller,
    from: i64,
    to: i64,
    seen: &mut HashSet<u64>,
) -> Result<Vec<Value>> {
    let days = format!("{}..{}", day(from), day(to));
    let mut rows = Vec::new();
    for page in 1.. {
        let listed = controller
            .plan_store
            .github_runs_window(&controller.github_repository, &days, page, PER_PAGE)
            .await?;
        rows.extend(
            listed["runs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|row| row["run_id"].as_u64().is_some_and(|id| seen.insert(id)))
                .cloned(),
        );
        if listed["next_page"].is_null() {
            break;
        }
    }
    Ok(rows)
}

/// Import the rows that need it, Release Control's first. `Some` ends the
/// pass early with the wait: turned off, or a rate limit.
// shortcut: one sync import at a time (manual imports take the other turn); run two through for_each_concurrent if the backfill is too slow
async fn import(controller: &Arc<Controller>, mut rows: Vec<Value>) -> Result<Option<Duration>> {
    let sync = &controller.ci_sync;
    rows.sort_by_key(|row| !release_control(row));
    let mut queue = Vec::new();
    for row in rows {
        if decide(controller, &row).await? {
            queue.push(row);
        }
    }
    sync.lock().remaining = queue.len();
    for row in queue {
        if !sync.status().enabled {
            sync.lock().remaining = 0;
            return Ok(Some(controller.plan_store.sync_interval()));
        }
        // A manual import may have taken it since.
        if decide(controller, &row).await? {
            let run_id = row["run_id"].as_u64().unwrap_or_default();
            match controller
                .sync_import(&controller.github_repository, run_id)
                .await
            {
                Ok(None) => {}
                Ok(Some(execution)) if execution.state != "failed" => sync.lock().imported += 1,
                Ok(Some(execution))
                    if rate_limited(execution.error.as_deref().unwrap_or_default()) =>
                {
                    return Ok(Some(sync.pause()))
                }
                Ok(Some(execution)) if left_no_results(&execution) => {
                    skip(
                        controller,
                        &execution,
                        row["updated_at"].as_str().unwrap_or_default(),
                    )
                    .await?
                }
                Ok(Some(_)) => sync.lock().failed += 1,
                Err(error) if rate_limited(&format!("{error:#}")) => return Ok(Some(sync.pause())),
                // One run's failure only counts: the progress line stays.
                Err(error) => {
                    tracing::warn!(run_id, error = %format!("{error:#}"), "CI sync could not import a run");
                    sync.lock().failed += 1;
                }
            }
        }
        let mut status = sync.lock();
        status.remaining = status.remaining.saturating_sub(1);
    }
    Ok(None)
}

/// Whether CI sync imports this listed run now.
async fn decide(controller: &Controller, row: &Value) -> Result<bool> {
    let id = import_id(
        &controller.github_repository,
        row["run_id"].as_u64().unwrap_or_default(),
    );
    let mut execution = controller.plan_store.read_execution(&id).await.ok();
    let mut skipped = controller.plan_store.import_skipped(&id).await?;
    // An import that left no results before CI sync skipped such runs is
    // skipped as GitHub had updated the run then; one made by hand after a
    // skip keeps its error.
    if skipped.is_none() {
        if let Some(failed) = execution.take_if(|execution| left_no_results(execution)) {
            let at = match &failed.source {
                ExecutionSource::Github { run_updated_at, .. } => {
                    run_updated_at.clone().unwrap_or_default()
                }
                _ => String::new(),
            };
            skip(controller, &failed, &at).await?;
            skipped = Some(at);
        }
    }
    let deleted = controller.plan_store.import_deleted(&id).await?;
    Ok(needs_import(
        row,
        execution.as_ref(),
        deleted,
        skipped.as_deref(),
    ))
}

/// Skip a run that left no results, as GitHub updated it at `run_updated_at`:
/// its execution goes, and the Console hears so.
async fn skip(controller: &Controller, failed: &PlanExecution, run_updated_at: &str) -> Result<()> {
    controller
        .plan_store
        .skip_import(
            &failed.id,
            run_updated_at,
            failed.error.as_deref().unwrap_or_default(),
        )
        .await?;
    controller.ci_sync.lock().skipped += 1;
    controller.emit_change("deleted", &failed.id).await;
    Ok(())
}

/// A listed run needs an import unless it was deleted here, skipped as GitHub
/// last updated it (`skipped`), is being imported, or was started here; it
/// does when GitHub updated it since its import, and when its import failed,
/// `MAX_ATTEMPTS` times, unless its artifacts are gone or it left no bundle.
fn needs_import(
    row: &Value,
    execution: Option<&PlanExecution>,
    deleted: bool,
    skipped: Option<&str>,
) -> bool {
    if deleted || skipped.is_some_and(|at| row["updated_at"].as_str() == Some(at)) {
        return false;
    }
    let Some(execution) = execution else {
        return true;
    };
    if execution.state == "importing" || github_started(execution) {
        return false;
    }
    let (updated_at, attempts) = match &execution.source {
        ExecutionSource::Github {
            run_updated_at,
            sync_attempts,
            ..
        } => (run_updated_at.as_deref(), *sync_attempts),
        _ => (None, 0),
    };
    if updated_at.is_some_and(|updated_at| row["updated_at"].as_str() != Some(updated_at)) {
        return true;
    }
    execution.state == "failed"
        && !gone(execution.error.as_deref().unwrap_or_default())
        && attempts < MAX_ATTEMPTS
}

/// An import that ended failed because the run left no results, and holds
/// no runs of an earlier import: CI sync skips the run instead of keeping it.
fn left_no_results(execution: &PlanExecution) -> bool {
    execution.state == "failed"
        && !github_started(execution)
        && execution
            .error
            .as_deref()
            .is_some_and(|error| error.contains("keeps no e2e-contract artifact"))
        && native_runs(execution).all(str::is_empty)
}

/// A failure no retry fixes.
fn gone(error: &str) -> bool {
    error.contains("GitHub keeps artifacts for 90 days")
        || error.contains("The run produced no bundle")
}

fn release_control(row: &Value) -> bool {
    row["release_control_execution_id"].is_string()
}

/// The UTC day `n` days ago, as GitHub's `created` filter takes it.
fn day(n: i64) -> String {
    (Utc::now() - TimeDelta::days(n))
        .format("%Y-%m-%d")
        .to_string()
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// What is left until `at`, when it is still ahead.
fn left_until(at: &str) -> Option<Duration> {
    let at = DateTime::parse_from_rfc3339(at).ok()?.with_timezone(&Utc);
    (at - Utc::now())
        .to_std()
        .ok()
        .filter(|left| !left.is_zero())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::dashboard::controller::tests::without_control_plane;
    use crate::plans::store::tests::{
        exact_stack_bundle, fake_gh, import_stub, manager_with_gh, FakeRunner,
    };
    use crate::plans::store::{import_id, ExecutionSource, PlanExecution};

    /// Heads each `gh` stand-in: it logs every call, `run <id> <title>
    /// [<updated_at>]` prints a run, and the tests add their cases and `esac`.
    const GH: &str = r#"here="$(dirname "$0")"; echo "$*" >> "$here/calls"
run() { printf '{"id":%s,"run_attempt":1,"display_title":"%s","html_url":"https://github.com/o/r/actions/runs/%s","created_at":"2026-10-20T10:00:00Z","updated_at":"%s"}' "$1" "$2" "$1" "${3:-2026-10-20T11:00:00Z}"; }
case "$*" in
  "auth status"*) echo "Logged in to github.com account octo" ;;"#;

    fn controller_on(root: &Path, script: &str) -> Arc<Controller> {
        let data = root.join("data");
        let mut gh = fake_gh(root, script);
        gh.sync_interval = Duration::from_secs(42);
        gh.api_timeout = Duration::from_secs(5);
        without_control_plane(
            manager_with_gh(&data, Arc::new(FakeRunner::new(data.clone())), gh),
            &data,
        )
    }

    /// The runs whose artifacts `gh` listed, in the order it listed them.
    fn artifacts_listed(root: &Path) -> Vec<String> {
        fs::read_to_string(root.join("calls"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| Some(line.split("runs/").nth(1)?.split_once("/artifacts")?.0))
            .map(str::to_owned)
            .collect()
    }

    /// The `gh` calls holding `needle`.
    fn calls(root: &Path, needle: &str) -> usize {
        fs::read_to_string(root.join("calls"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains(needle))
            .count()
    }

    async fn attempts(controller: &Controller, run: u64) -> u32 {
        let execution = controller
            .plan_store
            .read_execution(&import_id("o/r", run))
            .await
            .unwrap();
        match execution.source {
            ExecutionSource::Github { sync_attempts, .. } => sync_attempts,
            source => panic!("not a GitHub import: {source:?}"),
        }
    }

    #[test]
    fn a_listed_run_is_imported_only_when_sync_needs_it() {
        let row = json!({"run_id": 1, "run_attempt": 1, "updated_at": "2026-10-20T11:00:00Z"});
        let imported =
            |state: &str, error: Option<&str>, updated: Option<&str>, tries: u32| PlanExecution {
                state: state.into(),
                error: error.map(str::to_owned),
                source: ExecutionSource::Github {
                    repository: "o/r".into(),
                    run_id: 1,
                    run_attempt: 1,
                    url: "https://github.com/o/r/actions/runs/1".into(),
                    release_control_execution_id: None,
                    stack: None,
                    run_updated_at: updated.map(str::to_owned),
                    sync_attempts: tries,
                    conclusion: None,
                    status: None,
                    follow: Default::default(),
                },
                ..import_stub()
            };
        let (same, earlier) = (Some("2026-10-20T11:00:00Z"), Some("2026-10-20T09:00:00Z"));
        let reset = Some("`gh api` failed: connection reset");
        let limited = Some("`gh api` failed: API rate limit exceeded for user ID 1. (HTTP 403)");
        let gone = [
            "This run keeps no e2e-contract artifact; GitHub keeps artifacts for 90 days",
            "The run produced no bundle: Root artifact validation failed",
        ];

        assert!(needs_import(&row, None, false, None));
        // Skipped as GitHub last updated it, a run waits for a re-run.
        assert!(!needs_import(&row, None, false, same));
        assert!(needs_import(&row, None, false, earlier));
        let mut needed = vec![
            imported("completed", None, earlier, 0),
            imported("failed", reset, same, 2),
            imported("failed", limited, same, 0),
        ];
        let mut not_needed = vec![
            imported("importing", None, same, 0),
            PlanExecution {
                idempotency_key: "github-start:o/r#1".into(),
                ..imported("completed", None, earlier, 0)
            },
            imported("completed", None, same, 0),
            imported("interrupted", None, None, 0),
            imported("failed", reset, same, 3),
        ];
        for gone in gone {
            needed.push(imported("failed", Some(gone), earlier, 3));
            not_needed.push(imported("failed", Some(gone), same, 0));
        }
        for execution in &needed {
            assert!(
                needs_import(&row, Some(execution), false, None),
                "{execution:?}"
            );
        }
        assert!(!needs_import(&row, None, true, None));
        for execution in &not_needed {
            assert!(
                !needs_import(&row, Some(execution), false, None),
                "{execution:?}"
            );
        }
    }

    #[test]
    fn only_a_failed_import_that_left_nothing_is_skipped() {
        let failed = |error: &str| PlanExecution {
            state: "failed".into(),
            error: Some(error.into()),
            source: ExecutionSource::Github {
                repository: "o/r".into(),
                run_id: 1,
                run_attempt: 1,
                url: "https://github.com/o/r/actions/runs/1".into(),
                release_control_execution_id: None,
                stack: None,
                run_updated_at: None,
                sync_attempts: 1,
                conclusion: None,
                status: None,
                follow: Default::default(),
            },
            ..import_stub()
        };
        let none = "This run keeps no e2e-contract artifact; GitHub keeps artifacts for 90 days";
        let mut kept_runs = failed(none);
        kept_runs.slots.push(
            serde_json::from_value(json!({"round": 1, "group_id": "g", "scenario_id": "s",
                "execution_id": "0123456789abcdef0123456789abcdef", "request": {}, "state": "completed",
                "result_path": null, "error": null, "observed": 1, "completed": 1, "passed": 1,
                "technical_valid": 1, "eligible": true}))
            .unwrap(),
        );

        assert!(left_no_results(&failed(none)));
        for kept in [
            PlanExecution {
                state: "completed".into(),
                ..failed(none)
            },
            failed("The run produced no bundle: Root artifact validation failed"),
            PlanExecution {
                idempotency_key: "github-start:o/r#1".into(),
                ..failed(none)
            },
            kept_runs,
        ] {
            assert!(!left_no_results(&kept), "{kept:?}");
        }
    }

    #[tokio::test]
    async fn ci_sync_is_off_until_turned_on_and_says_how_to_sign_in() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let data = root.join("data");
        let controller = controller_on(root, r#"echo "$*" >> "$(dirname "$0")/calls"; exit 1"#);

        assert_eq!(
            serde_json::to_string(&controller.ci_sync_status()).unwrap(),
            r#"{"enabled":false,"phase":"off","signed_in":null,"message":null,"checked_at":null,"next_check_at":null,"imported":0,"failed":0,"skipped":0,"remaining":0,"paused_until":null}"#
        );
        assert_eq!(tick(&controller).await, Duration::from_secs(42));
        assert_eq!(calls(root, ""), 0);

        let on = controller.ci_sync_set(true).unwrap();
        assert_eq!((on.enabled, on.phase), (true, CiSyncPhase::Quick));
        assert_eq!(
            fs::read_to_string(data.join("ci-sync.json")).unwrap(),
            r#"{"enabled":true}"#
        );
        assert_eq!(CiSync::load(&data).status().phase, CiSyncPhase::Quick);

        tick(&controller).await;
        let status = controller.ci_sync_status();
        assert_eq!(status.signed_in, Some(false));
        assert_eq!(
            status.message.as_deref(),
            Some("`gh` is not signed in on the worker's machine. Run `gh auth login` there.")
        );
        assert_eq!(calls(root, ""), 1);

        let off = controller.ci_sync_set(false).unwrap();
        assert_eq!((off.enabled, off.phase), (false, CiSyncPhase::Off));
        assert_eq!(
            fs::read_to_string(data.join("ci-sync.json")).unwrap(),
            r#"{"enabled":false}"#
        );
    }

    #[tokio::test]
    async fn ci_sync_without_gh_says_so_instead_of_asking_to_sign_in() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let controller = controller_on(root, "exit 0");
        fs::remove_file(root.join("gh")).unwrap();
        controller.ci_sync_set(true).unwrap();

        tick(&controller).await;
        let status = controller.ci_sync_status();
        assert_eq!(status.signed_in, None);
        assert_eq!(
            status.message.as_deref(),
            Some("The GitHub CLI (`gh`) is not installed for the Harness E2E worker. Install it, then run `gh auth login`.")
        );
    }

    #[tokio::test]
    async fn quick_imports_release_control_runs_day_by_day_before_the_newest_other_runs() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let controller = controller_on(
            root,
            &[
                &format!("D0={} D1={}\n", day(0), day(1)),
                GH,
                r#"
  *"created=$D0..$D0"*) printf '{"total_count":2,"workflow_runs":['; run 1 'E2E · rc-1'; printf ,; run 2 'E2E run · pr · flash'; printf ']}' ;;
  *"created=$D1..$D1"*) printf '{"total_count":2,"workflow_runs":['; run 3 'E2E · rc-3'; printf ,; run 4 'E2E run · pr · flash'; printf ']}' ;;
  *workflows/*) printf '{"total_count":0,"workflow_runs":[]}' ;;
  *artifacts*) ;;
  *runs/1) run 1 'E2E · rc-1' ;;
  *runs/2) run 2 'E2E run · pr · flash' ;;
  *runs/3) run 3 'E2E · rc-3' ;;
  *runs/4) run 4 'E2E run · pr · flash' ;;
esac"#,
            ]
            .concat(),
        );
        controller.ci_sync_set(true).unwrap();

        // No verdict ever comes: every day's Release Control run, then the
        // newest other runs, then the backfill.
        assert_eq!(tick(&controller).await, Duration::ZERO);
        assert_eq!(artifacts_listed(root), ["1", "3", "2", "4"]);
        assert_eq!(controller.ci_sync_status().phase, CiSyncPhase::Backfill);
    }

    #[tokio::test]
    async fn one_failed_import_counts_as_failed_and_leaves_the_progress_line_alone() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let (bundle, contract, _) =
            exact_stack_bundle(&root.join("fixture"), "rc:e2e:synced", "1.8.8");
        let controller = controller_on(
            root,
            &[
                GH,
                &format!(
                    r#"
  *workflows/*) printf '{{"total_count":2,"workflow_runs":['; run 1 'E2E · rc-1'; printf ,; run 2 'E2E · rc-2'; printf ']}}' ;;
  "api repos/o/r/actions/runs/1") echo 'HTTP 502: Bad Gateway' >&2; exit 1 ;;
  "api repos/o/r/actions/runs/2") run 2 'E2E · rc-2' ;;
  "api --paginate"*) printf 'e2e-contract-rc-2-gh-1
e2e-observation-rc-2-gh-1
' ;;
  "run download"*) case "$7" in
      e2e-contract-*) cp -R '{contract}' "$9" ;;
      *) cp -R '{bundle}' "$9" ;;
    esac ;;
esac"#,
                    contract = contract.display(),
                    bundle = bundle.display(),
                ),
            ]
            .concat(),
        );
        controller.ci_sync_set(true).unwrap();

        tick(&controller).await;
        let status = controller.ci_sync_status();
        assert_eq!(
            (status.message, status.failed, status.imported),
            (None, 1, 1)
        );
    }

    #[tokio::test]
    async fn a_run_without_a_contract_is_skipped_not_kept_as_a_failed_execution() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let controller = controller_on(
            root,
            &[
                GH,
                r#"
  *workflows/*) printf '{"total_count":1,"workflow_runs":['; run 7 'E2E · rc-7' "$(cat "$here/updated")"; printf ']}' ;;
  *runs/7/artifacts*) printf 'e2e-observation-rc-7-gh-1\n' ;;
  *runs/7) run 7 'E2E · rc-7' "$(cat "$here/updated")" ;;
esac"#,
            ]
            .concat(),
        );
        fs::write(root.join("updated"), "2026-10-20T11:00:00Z").unwrap();
        controller.ci_sync_set(true).unwrap();
        let id = import_id("o/r", 7);

        tick(&controller).await;
        assert!(controller.plan_store.read_execution(&id).await.is_err());
        assert_eq!(
            controller
                .plan_store
                .import_skipped(&id)
                .await
                .unwrap()
                .as_deref(),
            Some("2026-10-20T11:00:00Z")
        );
        let status = controller.ci_sync_status();
        assert_eq!((status.imported, status.failed, status.skipped), (0, 0, 1));

        // Listed again as GitHub left it, it is not tried again.
        tick(&controller).await;
        assert_eq!(calls(root, "runs/7/artifacts"), 1);

        // Run again on GitHub, it is.
        fs::write(root.join("updated"), "2026-10-21T09:00:00Z").unwrap();
        tick(&controller).await;
        assert_eq!(calls(root, "runs/7/artifacts"), 2);
        assert_eq!(
            controller
                .plan_store
                .import_skipped(&id)
                .await
                .unwrap()
                .as_deref(),
            Some("2026-10-21T09:00:00Z")
        );
        let status = controller.ci_sync_status();
        assert_eq!((status.failed, status.skipped), (0, 2));
    }

    #[tokio::test]
    async fn an_old_failed_execution_without_a_contract_turns_into_a_skip() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let controller = controller_on(
            root,
            &[
                GH,
                r#"
  *workflows/*) printf '{"total_count":2,"workflow_runs":['; run 8 'E2E · rc-8'; printf ,; run 9 'E2E · rc-9' "$(cat "$here/updated")"; printf ']}' ;;
  *artifacts*) ;;
  *runs/8) run 8 'E2E · rc-8' ;;
  *runs/9) run 9 'E2E · rc-9' "$(cat "$here/updated")" ;;
esac"#,
            ]
            .concat(),
        );
        // Imported before CI sync skipped such runs: each ended failed.
        fs::write(root.join("updated"), "2026-10-20T11:00:00Z").unwrap();
        for run in [8, 9] {
            let (begun, _) = controller
                .plan_store
                .begin_github_import("o/r", run)
                .await
                .unwrap();
            controller
                .plan_store
                .finish_github_import(&begun.id)
                .await
                .unwrap();
        }
        // GitHub ran 9 again since.
        fs::write(root.join("updated"), "2026-10-21T09:00:00Z").unwrap();
        controller.ci_sync_set(true).unwrap();

        tick(&controller).await;
        for run in [8, 9] {
            assert!(controller
                .plan_store
                .read_execution(&import_id("o/r", run))
                .await
                .is_err());
        }
        assert_eq!(
            (
                controller
                    .plan_store
                    .import_skipped(&import_id("o/r", 8))
                    .await
                    .unwrap(),
                controller
                    .plan_store
                    .import_skipped(&import_id("o/r", 9))
                    .await
                    .unwrap()
            ),
            (
                Some("2026-10-20T11:00:00Z".into()),
                Some("2026-10-21T09:00:00Z".into())
            )
        );
        // 8 is not tried again; 9, run again, is.
        assert_eq!(
            (
                calls(root, "runs/8/artifacts"),
                calls(root, "runs/9/artifacts")
            ),
            (1, 2)
        );
        let status = controller.ci_sync_status();
        assert_eq!((status.failed, status.skipped), (0, 3));
    }

    #[tokio::test]
    async fn ci_sync_tries_a_failed_import_three_times_and_never_one_whose_artifacts_are_gone() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let controller = controller_on(
            root,
            &[
                GH,
                r#"
  *workflows/*) printf '{"total_count":2,"workflow_runs":['; run 1 'E2E · rc-1'; printf ,; run 2 'E2E · rc-2'; printf ']}' ;;
  *runs/1/artifacts*) printf 'e2e-contract-rc-1-gh-1\n' ;;
  *runs/2/artifacts*) echo 'connection reset' >&2; exit 1 ;;
  *runs/1) run 1 'E2E · rc-1' ;;
  *runs/2) run 2 'E2E · rc-2' ;;
esac"#,
            ]
            .concat(),
        );
        controller.ci_sync_set(true).unwrap();

        assert_eq!(tick(&controller).await, Duration::ZERO);
        assert_eq!(controller.ci_sync_status().phase, CiSyncPhase::Backfill);
        for _ in 0..3 {
            assert_eq!(tick(&controller).await, Duration::from_secs(42));
        }

        assert_eq!(
            (
                calls(root, "runs/1/artifacts"),
                calls(root, "runs/2/artifacts")
            ),
            (1, 3)
        );
        assert_eq!(
            (
                attempts(&controller, 1).await,
                attempts(&controller, 2).await
            ),
            (1, 3)
        );
        let status = controller.ci_sync_status();
        assert_eq!(
            (
                status.phase,
                status.signed_in,
                status.imported,
                status.failed,
                status.remaining,
                status.message
            ),
            (CiSyncPhase::KeepingUp, Some(true), 0, 4, 0, None)
        );
        assert!(status.checked_at.is_some() && status.next_check_at.is_some());
    }

    #[tokio::test]
    async fn ci_sync_pauses_on_a_rate_limit_without_counting_an_attempt() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let controller = controller_on(
            root,
            &[
                GH,
                r#"
  *workflows/*) printf '{"total_count":1,"workflow_runs":['; run 3 'E2E run · pr · flash'; printf ']}' ;;
  *runs/3/artifacts*) echo 'gh: API rate limit exceeded for user ID 1. (HTTP 403)' >&2; exit 1 ;;
  *runs/3) run 3 'E2E run · pr · flash' ;;
esac"#,
            ]
            .concat(),
        );
        controller.ci_sync_set(true).unwrap();

        let wait = tick(&controller).await;
        assert!(
            wait > Duration::from_secs(59 * 60) && wait <= Duration::from_secs(60 * 60),
            "{wait:?}"
        );
        let status = controller.ci_sync_status();
        assert_eq!((status.phase, status.failed), (CiSyncPhase::Quick, 0));
        assert!(status.paused_until.is_some());
        assert_eq!(attempts(&controller, 3).await, 0);

        let before = calls(root, "");
        assert!(tick(&controller).await > Duration::from_secs(59 * 60));
        assert_eq!(calls(root, ""), before);
    }

    #[tokio::test]
    async fn keeping_up_sees_again_a_run_created_three_days_ago_that_github_ran_again() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let controller = controller_on(
            root,
            &[
                &format!("D0={} D3={} D7={}\n", day(0), day(3), day(7)),
                GH,
                r#"
  *"created=$D3..$D3"*|*"created=$D7..$D0"*) printf '{"total_count":1,"workflow_runs":['; run 5 'E2E · rc-5' "$(cat "$here/updated")"; printf ']}' ;;
  *workflows/*) printf '{"total_count":0,"workflow_runs":[]}' ;;
  *runs/5/artifacts*) ;;
  *runs/5) run 5 'E2E · rc-5' "$(cat "$here/updated")" ;;
esac"#,
            ]
            .concat(),
        );
        fs::write(root.join("updated"), "2026-10-20T11:00:00Z").unwrap();
        controller.ci_sync_set(true).unwrap();

        for _ in 0..3 {
            tick(&controller).await;
        }
        assert_eq!(controller.ci_sync_status().phase, CiSyncPhase::KeepingUp);
        assert_eq!(calls(root, "runs/5/artifacts"), 1);
        assert_eq!(calls(root, &format!("created={}..{}", day(7), day(0))), 1);

        fs::write(root.join("updated"), "2026-10-21T09:00:00Z").unwrap();
        tick(&controller).await;
        assert_eq!(calls(root, "runs/5/artifacts"), 2);
    }
}
