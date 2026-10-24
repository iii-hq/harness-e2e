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
    github_started, import_id, rate_limited, ExecutionSource, PlanExecution,
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
    /// Whether `gh` was signed in at the last check; null before one.
    pub signed_in: Option<bool>,
    /// What stopped the last check short.
    pub message: Option<String>,
    pub checked_at: Option<String>,
    pub next_check_at: Option<String>,
    pub imported: usize,
    pub failed: usize,
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
    {
        let mut status = sync.lock();
        status.signed_in = Some(ready);
        status.paused_until = None;
        status.message = (!ready).then(|| {
            github["message"]
                .as_str()
                .unwrap_or_default()
                .replace(", then reopen this dialog", "")
        });
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
        // A day at a time, until the two newest Harness releases each have
        // a Release Control run.
        CiSyncPhase::Quick => {
            let mut others = 0;
            for n in 0..=WINDOW_DAYS {
                if controller.release_overview().await?.verdict.is_some() {
                    break;
                }
                let mut rows = window(controller, n, n, &mut seen).await?;
                rows.retain(|row| {
                    if release_control(row) {
                        return true;
                    }
                    others += 1;
                    others <= PER_PAGE as usize
                });
                if let Some(wait) = import(controller, rows).await? {
                    return Ok(wait);
                }
            }
            (Vec::new(), CiSyncPhase::Backfill, Duration::ZERO)
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
                Ok(Some(_)) => sync.lock().failed += 1,
                Err(error) if rate_limited(&format!("{error:#}")) => return Ok(Some(sync.pause())),
                Err(error) => {
                    let mut status = sync.lock();
                    status.failed += 1;
                    status.message = Some(format!("{error:#}"));
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
    let execution = controller
        .plan_store
        .github_execution(
            &controller.github_repository,
            row["run_id"].as_u64().unwrap_or_default(),
        )
        .await;
    let deleted = controller.plan_store.import_deleted(&id).await?;
    Ok(needs_import(row, execution.as_ref(), deleted))
}

/// A listed run needs an import unless it was deleted here, is being
/// imported, or was started here; it does when GitHub updated it since its
/// import, and when its import failed, `MAX_ATTEMPTS` times, unless its
/// artifacts are gone or it left no bundle.
fn needs_import(row: &Value, execution: Option<&PlanExecution>, deleted: bool) -> bool {
    if deleted {
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
    use crate::plans::store::tests::{fake_gh, import_stub, manager_with_gh, FakeRunner};
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

        assert!(needs_import(&row, None, false));
        let mut needed = vec![
            imported("completed", None, earlier, 0),
            imported("failed", reset, same, 2),
            imported("failed", limited, same, 0),
        ];
        let mut not_needed = vec![
            imported("importing", None, same, 0),
            {
                let mut execution = imported("completed", None, earlier, 0);
                let ExecutionSource::Github { follow, .. } = &mut execution.source else {
                    unreachable!()
                };
                follow.followed = true;
                execution
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
            assert!(needs_import(&row, Some(execution), false), "{execution:?}");
        }
        assert!(!needs_import(&row, None, true));
        for execution in &not_needed {
            assert!(!needs_import(&row, Some(execution), false), "{execution:?}");
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
            r#"{"enabled":false,"phase":"off","signed_in":null,"message":null,"checked_at":null,"next_check_at":null,"imported":0,"failed":0,"remaining":0,"paused_until":null}"#
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
    async fn ci_sync_tries_a_failed_import_three_times_and_never_one_whose_artifacts_are_gone() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let controller = controller_on(
            root,
            &[
                GH,
                r#"
  *workflows/*) printf '{"total_count":2,"workflow_runs":['; run 1 'E2E · rc-1'; printf ,; run 2 'E2E · rc-2'; printf ']}' ;;
  *runs/1/artifacts*) ;;
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
