//! The Trends first screen, over every execution the Console lists. Only the
//! Release Control executions that counted make releases: a point per
//! Harness release and combination, its first completed execution, each
//! release compared with the one before it over the tests both counted
//! alike, the latest pair being the verdict. Local and Docker executions sit
//! beside a release; other CI runs are only counted.
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use super::trends::{
    execution, identity, instant, measures, natives, point, semver_key, source, suite_label,
    Execution, TrendMeasures, TrendPoint, TrendRole, TrendRun, TrendSeriesKey, TrendStack, HARNESS,
    NOT_RECORDED, RUNNER,
};

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct ReleaseOverview {
    /// In semver order, the latest last.
    pub releases: Vec<ReleasePoint>,
    /// The latest release against the previous one; null with fewer than two.
    pub verdict: Option<Verdict>,
    /// Oldest first.
    pub locals: Vec<LocalMark>,
    pub outside_release_control: OutsideCount,
    /// Counted Release Control executions whose Harness is not one plain
    /// release: a commit, joined builds, or none recorded.
    pub unversioned: usize,
}

/// A release on the line: its counted Release Control executions.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct ReleasePoint {
    /// One plain semver, without a leading `v`.
    pub release: String,
    /// Oldest first; a Release Control id run again counts its latest attempt.
    pub executions: Vec<String>,
    pub combinations: Vec<Combination>,
    /// Each combination's first completed execution, in combination order; a
    /// combination with only interrupted executions has none.
    pub firsts: Vec<CombinationPoint>,
    /// Against the release before it; null for the first.
    pub step: Option<ReleaseStep>,
}

/// A combination's first completed execution of a release: what its chart
/// plots and its step compares.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct CombinationPoint {
    pub combination: Combination,
    pub point: TrendPoint,
    /// Runs that did not count and that the subject did not fail: the E2E
    /// infrastructure's.
    pub infra_failed_runs: usize,
    /// Runs the subject failed: the Harness, its provider request or a
    /// budget.
    pub harness_failed_runs: usize,
    /// Planned runs that never ran.
    pub not_run: usize,
}

/// A release against the one before it, combination by combination.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct ReleaseStep {
    pub previous: String,
    /// The combinations both releases first completed, over the tests both
    /// counted alike; in combination order.
    pub compared: Vec<CombinationDelta>,
    /// Every other combination either release ran, with why.
    pub left_out: Vec<LeftOutCombination>,
}

/// The latest release against the previous one: the latest release's step.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct Verdict {
    pub latest: String,
    pub previous: String,
    /// When the latest release's newest execution started.
    pub latest_started_at: String,
}

/// One side of a combination's step: its first completed execution, over
/// the tests compared.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct Side {
    pub execution: String,
    /// Over the compared tests' runs; `planned` is their planned runs.
    pub measures: TrendMeasures,
    /// The compared tests' counted runs: what Tests completed divides by.
    pub counted_runs: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct CombinationDelta {
    pub combination: Combination,
    pub latest: Side,
    pub previous: Side,
    /// Tests both sides counted under one definition.
    pub compared_tests: usize,
    /// The change in mean score, in points, rounded to 0.1; null when a side
    /// scored nothing.
    pub score_delta: Option<f64>,
    /// The change in completed runs over counted runs, in percentage points,
    /// rounded to 0.1.
    pub completed_delta: f64,
    /// The iii engine and the runner, when they changed between the two
    /// executions.
    pub also_changed: Vec<VersionChange>,
    pub tests_left_out: Vec<LeftOutTest>,
    /// The latest against the previous over the tests compared, as
    /// `harness-e2e compare` reads them; without a verdict when a side ran
    /// one run per test. Only the latest release's step has one.
    pub calibrated: Option<crate::verdict::Summary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct VersionChange {
    /// `iii` or `harness-e2e`.
    pub name: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct LeftOutTest {
    pub test: String,
    pub reason: TestLeftOut,
}

/// Why a test does not compare; the first that applies.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum TestLeftOut {
    /// Planned or run on the latest side only.
    OnlyLatest,
    OnlyPrevious,
    /// Both planned or ran it, and no run of it counted on the latest side
    /// (or on neither): it never ran, or the E2E infrastructure failed it.
    NotCountedLatest,
    NotCountedPrevious,
    /// Its definition differs between the sides, or is unknown or mixed.
    Redefined,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct LeftOutCombination {
    pub combination: Combination,
    pub reason: CombinationLeftOut,
    /// The only release of the two that counted a run of it.
    pub only_in: Option<String>,
    /// With `all_runs_infra_failed`: the other release's runs of it, none of
    /// which counted.
    pub infra_failed_runs: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum CombinationLeftOut {
    /// Only one release ran it.
    OnlyInOneRelease,
    /// Both first completed it, and no test compares.
    NothingAlike,
    /// Both counted it, and one has only interrupted executions of it.
    NoCompletedExecution,
    /// The other release ran it, and every run failed in the E2E
    /// infrastructure.
    AllRunsInfraFailed,
}

/// What the verdict compares like with like: a series and its stack.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
pub(super) struct Combination {
    /// A suite id, or the sha256 of an unsaved suite.
    pub suite: String,
    /// From the release's newest execution of it.
    pub suite_label: String,
    pub provider: String,
    pub model: String,
    pub profile: Option<String>,
    /// A stack name, or `not_recorded`.
    pub stack: String,
}

/// A local or Docker execution, beside a release it never counts in.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct LocalMark {
    pub execution_id: String,
    pub role: TrendRole,
    pub started_at: String,
    /// Its own release when that is on the line, else the newest release
    /// whose first execution started before it.
    pub release: String,
    /// The Harness it recorded (`@sha7`, a version); null: not recorded.
    pub ran: Option<String>,
    pub suite_label: String,
    /// The combination whose chart it sits on.
    pub combination: Combination,
    /// `measures` is null when nothing counted.
    pub point: TrendPoint,
}

/// GitHub executions without a Release Control id, whatever their state.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct OutsideCount {
    pub executions: usize,
    pub latest_at: Option<String>,
}

/// An execution of the Console, its point, and the runs of its native runs.
struct Ran<'a> {
    execution: Execution<'a>,
    point: TrendPoint,
    runs: Vec<&'a TrendRun>,
}

/// A series and its stack name.
type Key = (TrendSeriesKey, String);

/// A release's kept executions, oldest first, and by combination its first
/// completed one.
struct Line<'a> {
    release: &'a str,
    kept: Vec<&'a Ran<'a>>,
    combinations: BTreeMap<Key, (Combination, Option<&'a Ran<'a>>)>,
}

pub(super) fn overview(
    summaries: &[Value],
    runs: &BTreeMap<String, Vec<TrendRun>>,
) -> ReleaseOverview {
    let natives = natives(summaries);
    let mut executions = summaries
        .iter()
        .filter_map(execution)
        .map(|execution| {
            let natives = natives
                .get(execution.id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let runs = natives
                .iter()
                .filter_map(|native| runs.get(native["id"].as_str()?))
                .flatten()
                .collect::<Vec<_>>();
            let stack = TrendStack {
                name: execution.parameters.stack.as_ref().map(|s| s.name.clone()),
                matched_by_workers: false,
            };
            Ran {
                point: point(&execution, stack, natives, &runs),
                execution,
                runs,
            }
        })
        .collect::<Vec<_>>();
    executions.sort_by(|left, right| {
        instant(left.execution.started_at).cmp(&instant(right.execution.started_at))
    });

    let mut unversioned = 0;
    let mut releases = BTreeMap::<&str, BTreeMap<(&str, Key), &Ran>>::new();
    // A combination that ran and failed is not one that did not run: the
    // runs of executions where nothing counted, by release and combination.
    let mut invalid_only = BTreeMap::<(&str, Key), usize>::new();
    for ran in &executions {
        let finished = matches!(
            ran.execution.summary["state"].as_str(),
            Some("completed" | "interrupted")
        );
        if ran.point.source.role != TrendRole::ReleaseControl || !finished {
            continue;
        }
        if ran.point.counted == 0 {
            if let Some(release) = ran.point.release.as_deref().filter(|_| ran.point.runs > 0) {
                *invalid_only
                    .entry((release, combination_key(ran)))
                    .or_default() += ran.point.runs;
            }
            continue;
        }
        let Some(release) = &ran.point.release else {
            unversioned += 1;
            continue;
        };
        let id = ran.point.source.release_control_execution_id.as_deref();
        // Oldest first: a later attempt of the same id and combination
        // replaces the earlier; another combination under the id stays.
        releases
            .entry(release)
            .or_default()
            .insert((id.unwrap_or_default(), combination_key(ran)), ran);
    }
    let mut lines = releases
        .into_iter()
        .map(|(release, kept)| line(release, kept.into_values().collect()))
        .collect::<Vec<_>>();
    lines.sort_by_key(|line| semver_key(line.release));

    let last = lines.len().saturating_sub(1);
    let points = lines
        .iter()
        .enumerate()
        .map(|(index, line)| ReleasePoint {
            release: line.release.to_owned(),
            executions: line
                .kept
                .iter()
                .map(|ran| ran.execution.id.to_owned())
                .collect(),
            combinations: line
                .combinations
                .values()
                .map(|(combination, _)| combination.clone())
                .collect(),
            firsts: line
                .combinations
                .values()
                .filter_map(|(combination, first)| Some(first_point(combination, (*first)?)))
                .collect(),
            // Only the verdict's step reads the calibrated verdict, which
            // permutes every run.
            step: index
                .checked_sub(1)
                .map(|before| compare(line, &lines[before], &invalid_only, index == last)),
        })
        .collect();
    let verdict = match lines.as_slice() {
        [.., previous, latest] => Some(Verdict {
            latest: latest.release.to_owned(),
            previous: previous.release.to_owned(),
            latest_started_at: latest.kept[latest.kept.len() - 1]
                .execution
                .started_at
                .to_owned(),
        }),
        _ => None,
    };

    let locals = executions
        .iter()
        .filter(|ran| matches!(ran.point.source.role, TrendRole::Local | TrendRole::Docker))
        .filter_map(|ran| {
            let started = instant(ran.execution.started_at).0;
            let release = ran
                .point
                .release
                .as_deref()
                .filter(|release| lines.iter().any(|line| line.release == *release))
                .or_else(|| {
                    let current = lines
                        .iter()
                        .rev()
                        .find(|line| instant(line.kept[0].execution.started_at).0 <= started)?;
                    Some(current.release)
                })?;
            Some(LocalMark {
                execution_id: ran.execution.id.to_owned(),
                role: ran.point.source.role,
                started_at: ran.execution.started_at.to_owned(),
                release: release.to_owned(),
                ran: ran
                    .point
                    .workers
                    .as_ref()
                    .and_then(|w| w.get(HARNESS))
                    .cloned(),
                suite_label: suite_label(&ran.execution),
                combination: combination(&combination_key(ran), ran),
                point: ran.point.clone(),
            })
        })
        .collect();

    let outside = summaries
        .iter()
        .filter(|summary| {
            summary.get("parent_plan_execution_id").is_none()
                && source(summary).role == TrendRole::CiOther
        })
        .collect::<Vec<_>>();
    ReleaseOverview {
        releases: points,
        verdict,
        locals,
        outside_release_control: OutsideCount {
            executions: outside.len(),
            latest_at: outside
                .iter()
                .filter_map(|summary| summary["started_at"].as_str())
                .max_by(|left, right| instant(left).cmp(&instant(right)))
                .map(str::to_owned),
        },
        unversioned,
    }
}

/// The combination an execution ran: its series and its stack name.
fn combination_key(ran: &Ran) -> Key {
    (
        ran.execution.key.clone(),
        ran.point
            .stack
            .name
            .clone()
            .unwrap_or_else(|| NOT_RECORDED.into()),
    )
}

/// The combination `key` names, its suite labelled as `ran` labels it.
fn combination((series, stack): &Key, ran: &Ran) -> Combination {
    Combination {
        suite: series.suite.clone(),
        suite_label: suite_label(&ran.execution),
        provider: series.provider.clone(),
        model: series.model.clone(),
        profile: series.profile.clone(),
        stack: stack.clone(),
    }
}

/// A release's kept executions, oldest first, grouped by combination.
fn line<'a>(release: &'a str, mut kept: Vec<&'a Ran<'a>>) -> Line<'a> {
    kept.sort_by(|left, right| {
        instant(left.execution.started_at).cmp(&instant(right.execution.started_at))
    });
    let mut groups = BTreeMap::<Key, Vec<&Ran>>::new();
    for ran in &kept {
        groups.entry(combination_key(ran)).or_default().push(ran);
    }
    let combinations = groups
        .into_iter()
        .map(|(key, group)| {
            let combination = combination(&key, group[group.len() - 1]);
            let first = group
                .into_iter()
                .find(|ran| ran.execution.summary["state"].as_str() == Some("completed"));
            (key, (combination, first))
        })
        .collect();
    Line {
        release,
        kept,
        combinations,
    }
}

/// A first completed execution as the chart plots it, with what it did not
/// measure.
fn first_point(combination: &Combination, ran: &Ran) -> CombinationPoint {
    let parameters = &ran.execution.parameters;
    let not_run = parameters
        .scenarios
        .iter()
        .map(|test| {
            let ran = ran.runs.iter().filter(|run| run.test == *test).count();
            (parameters.runs.max(1) as usize).saturating_sub(ran)
        })
        .sum();
    CombinationPoint {
        combination: combination.clone(),
        point: ran.point.clone(),
        infra_failed_runs: ran
            .runs
            .iter()
            .filter(|run| !run.counted && !run.harness_failed)
            .count(),
        harness_failed_runs: ran.runs.iter().filter(|run| run.harness_failed).count(),
        not_run,
    }
}

/// A release against an earlier one: each combination both first completed,
/// over the tests both counted alike; every other combination either ran is
/// left out with why. `calibrate` also reads each compared combination's
/// calibrated verdict.
fn compare(
    latest: &Line,
    previous: &Line,
    invalid_only: &BTreeMap<(&str, Key), usize>,
    calibrate: bool,
) -> ReleaseStep {
    let (mut compared, mut left_out) = (Vec::new(), Vec::new());
    let keys = latest
        .combinations
        .keys()
        .chain(previous.combinations.keys())
        .collect::<BTreeSet<_>>();
    let left = |combination: &Combination,
                reason: CombinationLeftOut,
                only_in: Option<&str>,
                infra_failed_runs: Option<usize>| LeftOutCombination {
        combination: combination.clone(),
        reason,
        only_in: only_in.map(str::to_owned),
        infra_failed_runs,
    };
    // Only `line` counted it: whether `other` ran it and every run failed.
    let one_side =
        |combination: &Combination, line: &Line, other: &Line, key: &Key| match invalid_only
            .get(&(other.release, key.clone()))
        {
            Some(&runs) => left(
                combination,
                CombinationLeftOut::AllRunsInfraFailed,
                Some(line.release),
                Some(runs),
            ),
            None => left(
                combination,
                CombinationLeftOut::OnlyInOneRelease,
                Some(line.release),
                None,
            ),
        };
    for key in keys {
        match (latest.combinations.get(key), previous.combinations.get(key)) {
            (Some((combination, Some(first))), Some((_, Some(before)))) => {
                match delta(combination, first, before, calibrate) {
                    Some(delta) => compared.push(delta),
                    None => left_out.push(left(
                        combination,
                        CombinationLeftOut::NothingAlike,
                        None,
                        None,
                    )),
                }
            }
            (Some((combination, _)), Some(_)) => left_out.push(left(
                combination,
                CombinationLeftOut::NoCompletedExecution,
                None,
                None,
            )),
            (Some((combination, _)), None) => {
                left_out.push(one_side(combination, latest, previous, key))
            }
            (None, Some((combination, _))) => {
                left_out.push(one_side(combination, previous, latest, key))
            }
            (None, None) => {}
        }
    }
    ReleaseStep {
        previous: previous.release.to_owned(),
        compared,
        left_out,
    }
}

/// A combination both releases first completed, over the tests both counted
/// under one same definition; none when no test was. Every other test is
/// left out with why.
fn delta(
    combination: &Combination,
    latest: &Ran,
    previous: &Ran,
    calibrate: bool,
) -> Option<CombinationDelta> {
    let (latest_tests, previous_tests) = (tests(latest), tests(previous));
    let (latest_counted, previous_counted) = (counted(latest), counted(previous));
    let (mut alike, mut tests_left_out) = (BTreeSet::new(), Vec::new());
    for test in latest_tests.union(&previous_tests).copied() {
        let reason = if !previous_tests.contains(test) {
            Some(TestLeftOut::OnlyLatest)
        } else if !latest_tests.contains(test) {
            Some(TestLeftOut::OnlyPrevious)
        } else {
            match (
                definition(&latest_counted, test),
                definition(&previous_counted, test),
            ) {
                (None, _) => Some(TestLeftOut::NotCountedLatest),
                (_, None) => Some(TestLeftOut::NotCountedPrevious),
                (Some(Some(latest)), Some(Some(previous))) if latest == previous => None,
                _ => Some(TestLeftOut::Redefined),
            }
        };
        match reason {
            Some(reason) => tests_left_out.push(LeftOutTest {
                test: test.to_owned(),
                reason,
            }),
            None => {
                alike.insert(test);
            }
        }
    }
    let keep = |test: &str| alike.contains(test);
    let (latest_side, previous_side) = (side(latest, keep)?, side(previous, keep)?);
    let share = |side: &Side| 100.0 * side.measures.completed as f64 / side.counted_runs as f64;
    let runs = |ran: &Ran| {
        ran.runs
            .iter()
            .copied()
            .filter(|run| keep(&run.test))
            .map(verdict_run)
            .collect::<Vec<_>>()
    };
    Some(CombinationDelta {
        combination: combination.clone(),
        compared_tests: alike.len(),
        score_delta: latest_side
            .measures
            .score_mean
            .zip(previous_side.measures.score_mean)
            .map(|(latest, previous)| round1(latest - previous)),
        completed_delta: round1(share(&latest_side) - share(&previous_side)),
        also_changed: also_changed(latest, previous),
        tests_left_out,
        calibrated: calibrate.then(|| {
            crate::verdict::compare(
                &runs(previous),
                &runs(latest),
                &crate::verdict::Rule::default(),
            )
            .summary()
        }),
        latest: latest_side,
        previous: previous_side,
    })
}

/// An execution over the tests kept; none when none of their runs counted.
fn side(ran: &Ran, keep: impl Fn(&str) -> bool) -> Option<Side> {
    let runs = ran
        .runs
        .iter()
        .copied()
        .filter(|run| keep(&run.test))
        .collect::<Vec<_>>();
    Some(Side {
        execution: ran.execution.id.to_owned(),
        measures: measures(&runs, planned(&ran.execution, &ran.runs, &keep))?,
        counted_runs: runs.iter().filter(|run| run.counted).count(),
    })
}

/// What else changed between two executions: the iii engine and the runner.
fn also_changed(latest: &Ran, previous: &Ran) -> Vec<VersionChange> {
    let runner = |ran: &Ran| {
        let row = ran
            .execution
            .workers
            .iter()
            .find(|row| row.name == RUNNER)?;
        identity(row, false)
    };
    [
        (
            "iii",
            previous.point.engine.clone(),
            latest.point.engine.clone(),
        ),
        (RUNNER, runner(previous), runner(latest)),
    ]
    .into_iter()
    .filter_map(|(name, from, to)| match (from, to) {
        (Some(from), Some(to)) if from != to => Some(VersionChange {
            name: name.to_owned(),
            from,
            to,
        }),
        _ => None,
    })
    .collect()
}

/// Rounded to 0.1, halves away from zero.
fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// A run as the verdict reads it; the read model keeps whether it counted,
/// not its cause or its criteria.
fn verdict_run(run: &TrendRun) -> crate::verdict::Run {
    crate::verdict::Run {
        attempt_id: run.attempt_id.clone(),
        scenario: run.test.clone(),
        behavior: run.behavior_sha256.clone(),
        counted: run.counted,
        score: run.score,
        completed: run.completed,
        cost_usd: run.cost_usd,
        wall_time_ms: run.duration_ms,
        turns: run.turns,
        ..crate::verdict::Run::default()
    }
}

/// The tests an execution planned or ran.
fn tests<'a>(ran: &'a Ran) -> BTreeSet<&'a str> {
    let planned = ran.execution.parameters.scenarios.iter();
    planned
        .map(String::as_str)
        .chain(ran.runs.iter().map(|run| run.test.as_str()))
        .collect()
}

fn counted<'a>(ran: &Ran<'a>) -> Vec<&'a TrendRun> {
    ran.runs.iter().copied().filter(|run| run.counted).collect()
}

/// The one `behavior_sha256` all of a test's counted runs carry:
/// `Some(None)` when they differ or one is unknown, none when none counted.
fn definition<'a>(counted: &[&'a TrendRun], test: &str) -> Option<Option<&'a str>> {
    let mut definitions = counted
        .iter()
        .copied()
        .filter(|run| run.test == test)
        .map(|run| run.behavior_sha256.as_deref());
    let first = definitions.next()?;
    Some(first.filter(|_| definitions.all(|definition| definition == first)))
}

/// The runs planned for the tests kept (tests × runs per test), else the
/// kept tests' runs observed; keeping every test, it is `planned_runs`.
fn planned(execution: &Execution, runs: &[&TrendRun], keep: impl Fn(&str) -> bool) -> usize {
    let parameters = &execution.parameters;
    if parameters.scenarios.is_empty() {
        runs.iter().filter(|run| keep(&run.test)).count()
    } else {
        let tests = parameters
            .scenarios
            .iter()
            .filter(|test| keep(test))
            .count();
        tests * parameters.runs.max(1) as usize
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::dashboard::trends::tests::{listed, native, run, worker};
    use serde_json::json;

    pub(in crate::dashboard) fn at(day: u32) -> String {
        format!("2026-10-{day:02}T10:00:00Z")
    }

    /// A completed Release Control execution of Regression on deepseek/flash
    /// and the `default` stack, with its native run `n-<id>`.
    pub(in crate::dashboard) fn rc(id: &str, day: u32, harness: &str) -> Vec<Value> {
        let mut summary = listed(id, &at(day), Some("default"), &[HARNESS]);
        summary["state"] = json!("completed");
        summary["source"] = json!({
            "kind": "github", "repository": "o/r", "run_id": day, "run_attempt": 1, "url": "",
            "release_control_execution_id": id,
        });
        summary["stack"][0]["resolved"] = json!(harness);
        vec![summary, native(&format!("n-{id}"), id)]
    }

    /// The execution with its summary edited.
    pub(in crate::dashboard) fn with(
        mut execution: Vec<Value>,
        edit: impl FnOnce(&mut Value),
    ) -> Vec<Value> {
        edit(&mut execution[0]);
        execution
    }

    /// The runs of each execution's native run.
    pub(in crate::dashboard) fn runs(
        of: Vec<(&str, Vec<TrendRun>)>,
    ) -> BTreeMap<String, Vec<TrendRun>> {
        of.into_iter()
            .map(|(id, runs)| (format!("n-{id}"), runs))
            .collect()
    }

    /// A counted, completed run: 10 calls, 1 error, 1,000 ms.
    pub(in crate::dashboard) fn scored(test: &str, score: f64) -> TrendRun {
        run(test, true, true, Some(score))
    }

    /// A run the E2E infrastructure failed: it does not count.
    fn infra_failed(test: &str) -> TrendRun {
        run(test, false, false, None)
    }

    /// A run the subject failed: it counts, not completed.
    fn harness_failed(test: &str) -> TrendRun {
        TrendRun {
            harness_failed: true,
            ..run(test, true, false, None)
        }
    }

    pub(in crate::dashboard) fn suite(id: &str, label: &str) -> impl FnOnce(&mut Value) {
        let suite = json!({"id": id, "label": label, "sha256": format!("sha256:{id}")});
        move |summary| summary["parameters"]["suite"] = suite
    }

    /// The tests planned, each `runs` times.
    fn planning(tests: &[&str], runs: u32) -> impl FnOnce(&mut Value) {
        let tests = json!(tests);
        move |summary| {
            summary["parameters"]["scenarios"] = tests;
            summary["parameters"]["runs"] = json!(runs);
        }
    }

    /// The same test, its definition changed.
    fn redefined(run: TrendRun) -> TrendRun {
        TrendRun {
            behavior_sha256: Some("sha256:changed".into()),
            ..run
        }
    }

    fn local(summary: &mut Value) {
        summary["source"] = json!({"kind": "local"});
    }

    /// The Harness built from a commit.
    fn built(summary: &mut Value) {
        summary["stack"][0]["commit"] = json!(format!("3f2a9c1{}", "d".repeat(33)));
    }

    fn combination(suite: &str, label: &str, model: &str) -> Combination {
        Combination {
            suite: suite.into(),
            suite_label: label.into(),
            provider: "deepseek".into(),
            model: model.into(),
            profile: None,
            stack: "default".into(),
        }
    }

    fn left(
        combination: Combination,
        reason: CombinationLeftOut,
        only_in: Option<&str>,
        infra_failed_runs: Option<usize>,
    ) -> LeftOutCombination {
        LeftOutCombination {
            combination,
            reason,
            only_in: only_in.map(str::to_owned),
            infra_failed_runs,
        }
    }

    fn left_test(test: &str, reason: TestLeftOut) -> LeftOutTest {
        LeftOutTest {
            test: test.into(),
            reason,
        }
    }

    /// The latest release's step.
    fn latest_step(overview: &ReleaseOverview) -> &ReleaseStep {
        overview.releases.last().unwrap().step.as_ref().unwrap()
    }

    fn names(overview: &ReleaseOverview) -> Vec<Vec<&str>> {
        overview
            .releases
            .iter()
            .map(|release| release.executions.iter().map(String::as_str).collect())
            .collect()
    }

    #[test]
    fn a_combination_has_a_calibrated_verdict_only_with_repeated_runs() {
        let summaries = [rc("p", 1, "1.9.2"), rc("l", 3, "1.9.3")].concat();
        let calibrated = |previous: Vec<TrendRun>, latest: Vec<TrendRun>| {
            let overview = overview(&summaries, &runs(vec![("p", previous), ("l", latest)]));
            latest_step(&overview).compared[0]
                .calibrated
                .clone()
                .unwrap()
        };
        // One run per test: neither better nor worse, and it says why.
        let once = calibrated(vec![scored("t1", 80.0)], vec![scored("t1", 95.0)]);
        assert_eq!(once.verdict, None);
        assert!(once.unavailable.unwrap().starts_with("One run per test"));
        let repeated = |score: f64| {
            (0..4)
                .flat_map(|test| (0..8).map(move |i| scored(&format!("t{test}"), score + i as f64)))
                .collect::<Vec<_>>()
        };
        let dropped = calibrated(repeated(90.0), repeated(60.0));
        assert_eq!(dropped.verdict, Some(crate::verdict::Verdict::Worse));
        // The read model keeps no cause, and the summary carries the warnings.
        assert!(dropped.warnings[0].contains("carry no cause"));
        assert!(dropped.warnings.iter().any(|warning| warning
            .starts_with("t0 fell 30.0 points, beyond the 5-point margin on its own")));
        let quality = dropped.quality.unwrap();
        assert_eq!((quality.a, quality.b, quality.tests), (93.5, 63.5, 4));
        // Runs are read in attempt order, whatever order the read model holds.
        let noisy = |prefix: &str, scores: [f64; 4]| {
            (0..3)
                .flat_map(|test| {
                    scores.iter().enumerate().map(move |(i, score)| TrendRun {
                        attempt_id: format!("{prefix}{i}"),
                        ..scored(&format!("t{test}"), *score)
                    })
                })
                .collect::<Vec<_>>()
        };
        let (before, latest) = (
            noisy("p", [70.0, 95.0, 80.0, 90.0]),
            noisy("l", [60.0, 100.0, 85.0, 75.0]),
        );
        let reversed = latest.iter().rev().cloned().collect::<Vec<_>>();
        assert_eq!(
            calibrated(before.clone(), latest),
            calibrated(before, reversed)
        );
    }

    #[test]
    fn the_verdict_compares_the_combinations_and_tests_both_releases_ran_alike() {
        let summaries = [
            rc("p", 1, "1.9.2"),
            rc("l", 3, "1.9.3"),
            rc("l2", 4, "1.9.3"),
            with(rc("o", 3, "1.9.3"), suite("onboarding", "Onboarding")),
            with(rc("ps", 1, "1.9.2"), suite("se", "SE")),
            with(rc("ls", 3, "1.9.3"), suite("se", "SE")),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                ("p", vec![scored("t1", 80.0), scored("t2", 10.0)]),
                ("l", vec![scored("t1", 90.0), redefined(scored("t2", 70.0))]),
                ("l2", vec![scored("t1", 30.0)]),
                ("o", vec![scored("t9", 50.0)]),
                ("ps", vec![scored("s1", 10.0)]),
                ("ls", vec![redefined(scored("s1", 20.0))]),
            ]),
        );
        let verdict = overview.verdict.as_ref().unwrap();
        assert_eq!(
            (verdict.latest.as_str(), verdict.previous.as_str()),
            ("1.9.3", "1.9.2")
        );
        // The newest execution of the release, not its first.
        assert_eq!(verdict.latest_started_at, at(4));
        let step = latest_step(&overview);
        assert_eq!(step.previous, "1.9.2");
        let [delta] = step.compared.as_slice() else {
            panic!("one delta: {:?}", step.compared);
        };
        assert_eq!(
            delta.combination,
            combination("regression", "Regression", "flash")
        );
        // t2 changed its definition: only t1 compares, run by l, the first
        // completed execution; l2 ran later.
        assert_eq!(
            delta.tests_left_out,
            [left_test("t2", TestLeftOut::Redefined)]
        );
        assert_eq!(
            (
                delta.latest.execution.as_str(),
                delta.previous.execution.as_str()
            ),
            ("l", "p")
        );
        assert_eq!(
            (
                delta.latest.measures.score_mean,
                delta.previous.measures.score_mean
            ),
            (Some(90.0), Some(80.0))
        );
        assert_eq!(
            (
                delta.latest.measures.completed,
                delta.latest.measures.planned,
                delta.latest.counted_runs
            ),
            (1, 1, 1)
        );
        assert_eq!(
            (
                delta.compared_tests,
                delta.score_delta,
                delta.completed_delta
            ),
            (1, Some(10.0), 0.0)
        );
        assert_eq!(
            step.left_out,
            [
                left(
                    combination("onboarding", "Onboarding", "flash"),
                    CombinationLeftOut::OnlyInOneRelease,
                    Some("1.9.3"),
                    None
                ),
                left(
                    combination("se", "SE", "flash"),
                    CombinationLeftOut::NothingAlike,
                    None,
                    None
                ),
            ]
        );
        assert_eq!(overview.releases[0].step, None);
    }

    #[test]
    fn local_and_other_ci_executions_never_count() {
        let summaries = [
            rc("p", 1, "1.9.2"),
            rc("l", 3, "1.9.3"),
            with(rc("ci", 4, "1.9.3"), |summary| {
                summary["source"]["release_control_execution_id"] = json!(null)
            }),
            with(rc("local", 5, "1.9.3"), |summary| {
                local(summary);
                built(summary);
            }),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                ("p", vec![scored("t1", 80.0)]),
                ("l", vec![scored("t1", 90.0)]),
                ("ci", vec![scored("t1", 0.0)]),
                ("local", vec![scored("t1", 0.0)]),
            ]),
        );
        assert_eq!(names(&overview), [["p"], ["l"]]);
        assert_eq!(
            latest_step(&overview).compared[0]
                .latest
                .measures
                .score_mean,
            Some(90.0)
        );
        assert_eq!(
            overview.outside_release_control,
            OutsideCount {
                executions: 1,
                latest_at: Some(at(4)),
            }
        );
        let [mark] = overview.locals.as_slice() else {
            panic!("one mark: {:?}", overview.locals);
        };
        assert_eq!(
            (
                mark.execution_id.as_str(),
                mark.role,
                mark.release.as_str(),
                mark.ran.as_deref()
            ),
            ("local", TrendRole::Local, "1.9.3", Some("@3f2a9c1"))
        );
        assert_eq!(
            (mark.suite_label.as_str(), mark.started_at.clone()),
            ("Regression", at(5))
        );
        // On the chart of the combination it ran.
        assert_eq!(
            mark.combination,
            combination("regression", "Regression", "flash")
        );
        assert_eq!(mark.point.measures.as_ref().unwrap().score_mean, Some(0.0));
        assert_eq!(overview.unversioned, 0);
    }

    #[test]
    fn a_local_ring_sits_at_its_own_release_else_at_the_release_current_when_it_started() {
        let unrecorded = |summary: &mut Value| {
            local(summary);
            summary["stack"][0] = json!(worker(HARNESS, None, None));
        };
        let summaries = [
            rc("p", 2, "1.9.2"),
            rc("l", 4, "1.9.3"),
            with(rc("early", 1, "1.9.2"), unrecorded),
            with(rc("bare", 3, "1.9.2"), unrecorded),
            with(rc("docker", 5, "1.9.2"), |summary| {
                summary["source"] =
                    json!({"kind": "docker", "attempt": 1, "phase": "done", "groups": []});
            }),
            with(rc("pinned", 6, "1.9.9"), local),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                ("p", vec![scored("t1", 80.0)]),
                ("l", vec![scored("t1", 90.0)]),
            ]),
        );
        let marks = overview
            .locals
            .iter()
            .map(|mark| {
                (
                    mark.execution_id.as_str(),
                    mark.role,
                    mark.release.as_str(),
                    mark.ran.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        // `early` started before any release ran.
        assert_eq!(
            marks,
            [
                ("bare", TrendRole::Local, "1.9.2", None),
                ("docker", TrendRole::Docker, "1.9.2", Some("1.9.2")),
                ("pinned", TrendRole::Local, "1.9.3", Some("1.9.9")),
            ]
        );
        assert_eq!(overview.locals[0].point.measures, None);
    }

    #[test]
    fn an_interrupted_execution_with_counted_runs_counts_and_a_failed_import_or_one_in_flight_does_not(
    ) {
        let summaries = [
            rc("p", 1, "1.9.2"),
            with(rc("l", 2, "1.9.3"), |summary| {
                summary["state"] = json!("interrupted");
                summary["parameters"]["scenarios"] = json!(["t1", "t2"]);
            }),
            with(rc("failed", 3, "1.9.4"), |summary| {
                summary["state"] = json!("failed")
            }),
            with(rc("running", 4, "1.9.5"), |summary| {
                summary["state"] = json!("running")
            }),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                ("p", vec![scored("t1", 80.0)]),
                ("l", vec![scored("t1", 90.0)]),
                ("running", vec![scored("t1", 50.0)]),
            ]),
        );
        assert_eq!(
            overview
                .releases
                .iter()
                .map(|release| release.release.as_str())
                .collect::<Vec<_>>(),
            ["1.9.2", "1.9.3"]
        );
        let latest = &overview.releases[1];
        // The release exists, but an interrupted execution is never a side.
        assert_eq!(latest.executions, ["l"]);
        assert!(latest.firsts.is_empty());
        let step = latest_step(&overview);
        assert!(step.compared.is_empty());
        assert_eq!(
            step.left_out,
            [left(
                combination("regression", "Regression", "flash"),
                CombinationLeftOut::NoCompletedExecution,
                None,
                None
            )]
        );
        assert_eq!(overview.verdict.unwrap().latest, "1.9.3");
    }

    #[test]
    fn a_test_one_side_never_counted_is_left_out_not_failed() {
        let summaries = [
            with(
                rc("p", 1, "1.9.2"),
                planning(&["t1", "t2", "t4", "t5", "t6"], 1),
            ),
            with(
                rc("l", 2, "1.9.3"),
                planning(&["t1", "t2", "t3", "t4", "t6"], 1),
            ),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                (
                    "p",
                    vec![
                        scored("t1", 80.0),
                        scored("t2", 60.0),
                        infra_failed("t4"),
                        scored("t5", 70.0),
                    ],
                ),
                (
                    "l",
                    // t2 failed in the E2E infrastructure under a new
                    // definition: not counted comes first.
                    vec![
                        scored("t1", 90.0),
                        redefined(infra_failed("t2")),
                        scored("t4", 50.0),
                    ],
                ),
            ]),
        );
        let [delta] = latest_step(&overview).compared.as_slice() else {
            panic!("one delta: {:?}", latest_step(&overview).compared);
        };
        // t6 never ran on either side.
        assert_eq!(
            delta.tests_left_out,
            [
                left_test("t2", TestLeftOut::NotCountedLatest),
                left_test("t3", TestLeftOut::OnlyLatest),
                left_test("t4", TestLeftOut::NotCountedPrevious),
                left_test("t5", TestLeftOut::OnlyPrevious),
                left_test("t6", TestLeftOut::NotCountedLatest),
            ]
        );
        // Only t1 compares: what never counted lowers nothing.
        assert_eq!(
            (
                delta.compared_tests,
                delta.score_delta,
                delta.completed_delta
            ),
            (1, Some(10.0), 0.0)
        );
        assert_eq!(
            (delta.latest.measures.planned, delta.latest.counted_runs),
            (1, 1)
        );
    }

    #[test]
    fn tests_completed_reads_the_counted_runs_and_coverage_counts_what_did_not() {
        let summaries = [
            with(rc("p", 1, "1.9.2"), planning(&["t1", "t2"], 2)),
            with(rc("l", 2, "1.9.3"), planning(&["t1", "t2"], 2)),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                (
                    "p",
                    vec![
                        scored("t1", 100.0),
                        scored("t1", 100.0),
                        scored("t2", 100.0),
                        scored("t2", 100.0),
                    ],
                ),
                (
                    "l",
                    // t2's second run never ran.
                    vec![
                        scored("t1", 100.0),
                        infra_failed("t1"),
                        harness_failed("t2"),
                    ],
                ),
            ]),
        );
        let [delta] = latest_step(&overview).compared.as_slice() else {
            panic!("one delta: {:?}", latest_step(&overview).compared);
        };
        // The infrastructure's failure leaves the denominator; the Harness's
        // stays in it, not completed: 1 of 2 against 4 of 4.
        assert_eq!(
            (delta.latest.counted_runs, delta.latest.measures.completed),
            (2, 1)
        );
        assert_eq!(delta.latest.measures.planned, 4);
        assert_eq!(
            (delta.score_delta, delta.completed_delta),
            (Some(0.0), -50.0)
        );
        let first = &overview.releases[1].firsts[0];
        assert_eq!(first.point.execution_id, "l");
        assert_eq!(
            (
                first.infra_failed_runs,
                first.harness_failed_runs,
                first.not_run
            ),
            (1, 1, 1)
        );
        assert_eq!(first.point.counted, 2);
    }

    #[test]
    fn a_combination_whose_runs_all_failed_is_left_out_with_its_runs() {
        let onboarding = || suite("onboarding", "Onboarding");
        let summaries = [
            rc("p", 1, "1.9.2"),
            with(rc("po", 1, "1.9.2"), onboarding()),
            rc("l", 2, "1.9.3"),
            with(rc("lo", 2, "1.9.3"), onboarding()),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                ("p", vec![scored("t1", 80.0)]),
                ("po", vec![scored("t9", 50.0)]),
                ("l", vec![scored("t1", 90.0)]),
                ("lo", vec![infra_failed("t9"); 3]),
            ]),
        );
        // It makes no point of the latest release, and is not "not run".
        assert_eq!(overview.releases[1].combinations.len(), 1);
        assert_eq!(
            latest_step(&overview).left_out,
            [left(
                combination("onboarding", "Onboarding", "flash"),
                CombinationLeftOut::AllRunsInfraFailed,
                Some("1.9.2"),
                Some(3)
            )]
        );
    }

    #[test]
    fn a_step_names_the_engine_and_the_runner_when_they_moved() {
        let runner = |version: &'static str| {
            move |summary: &mut Value| {
                summary["stack"].as_array_mut().unwrap().push(json!(worker(
                    RUNNER,
                    Some(version),
                    None
                )))
            }
        };
        let mut latest = with(rc("l", 2, "1.9.3"), runner("0.22.1"));
        latest[1]["engine_version"] = json!("0.24.4");
        let summaries = [
            with(rc("p", 1, "1.9.2"), runner("0.22.0")),
            latest,
            // A runner recorded on one side only is no change.
            with(rc("n", 3, "1.9.4"), runner("0.22.1")),
            rc("m", 4, "1.9.5"),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(
                ["p", "l", "n", "m"]
                    .map(|id| (id, vec![scored("t1", 80.0)]))
                    .to_vec(),
            ),
        );
        let changes = |release: usize| {
            overview.releases[release].step.as_ref().unwrap().compared[0]
                .also_changed
                .clone()
        };
        let change = |name: &str, from: &str, to: &str| VersionChange {
            name: name.into(),
            from: from.into(),
            to: to.into(),
        };
        assert_eq!(
            changes(1),
            [
                change("iii", "0.24.3", "0.24.4"),
                change("harness-e2e", "0.22.0", "0.22.1"),
            ]
        );
        assert_eq!(changes(2), [change("iii", "0.24.4", "0.24.3")]);
        assert!(changes(3).is_empty());
    }

    #[test]
    fn releases_follow_semver_not_the_order_they_first_ran() {
        let summaries = [rc("new", 1, "1.10.0"), rc("old", 2, "1.9.3")].concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                ("new", vec![scored("t1", 90.0)]),
                ("old", vec![scored("t1", 80.0)]),
            ]),
        );
        assert_eq!(
            overview
                .releases
                .iter()
                .map(|release| (
                    release.release.as_str(),
                    release.firsts[0].point.started_at.clone()
                ))
                .collect::<Vec<_>>(),
            [("1.9.3", at(2)), ("1.10.0", at(1))]
        );
        let verdict = overview.verdict.unwrap();
        assert_eq!(
            (verdict.latest.as_str(), verdict.previous.as_str()),
            ("1.10.0", "1.9.3")
        );
    }

    #[test]
    fn with_no_shared_combination_the_verdict_compares_nothing_and_the_step_says_why() {
        let summaries = [
            with(rc("p", 1, "1.9.2"), |summary| {
                summary["parameters"]["model"] = json!("pro")
            }),
            rc("l", 2, "1.9.3"),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                ("p", vec![scored("t1", 80.0)]),
                ("l", vec![scored("t1", 90.0)]),
            ]),
        );
        // A verdict still, over nothing.
        assert!(overview.verdict.is_some());
        let step = latest_step(&overview);
        assert!(step.compared.is_empty());
        assert_eq!(
            step.left_out,
            [
                left(
                    combination("regression", "Regression", "flash"),
                    CombinationLeftOut::OnlyInOneRelease,
                    Some("1.9.3"),
                    None
                ),
                left(
                    combination("regression", "Regression", "pro"),
                    CombinationLeftOut::OnlyInOneRelease,
                    Some("1.9.2"),
                    None
                ),
            ]
        );
    }

    #[test]
    fn every_release_steps_from_its_first_completed_execution_against_the_release_before() {
        let state = |version: &'static str| {
            move |summary: &mut Value| {
                summary["stack"].as_array_mut().unwrap().push(json!(worker(
                    "state",
                    Some(version),
                    None
                )))
            }
        };
        let summaries = [
            rc("a", 1, "1.9.1"),
            rc("b", 2, "1.9.2"),
            with(rc("c1", 3, "1.9.3"), state("2.0.0")),
            with(rc("c2", 4, "1.9.3"), state("2.0.1")),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                ("a", vec![scored("t1", 50.0)]),
                ("b", vec![scored("t1", 70.0)]),
                ("c1", vec![scored("t1", 90.0)]),
                // A later night of the release does not move its step.
                ("c2", vec![scored("t1", 10.0)]),
            ]),
        );
        let steps = overview
            .releases
            .iter()
            .map(|release| {
                let step = release.step.as_ref()?;
                let delta = &step.compared[0];
                Some((
                    step.previous.as_str(),
                    delta.latest.measures.score_mean,
                    delta.previous.measures.score_mean,
                    delta.calibrated.is_some(),
                ))
            })
            .collect::<Vec<_>>();
        // Only the verdict's step reads the calibrated verdict.
        assert_eq!(
            steps,
            [
                None,
                Some(("1.9.1", Some(70.0), Some(50.0), false)),
                Some(("1.9.2", Some(90.0), Some(70.0), true)),
            ]
        );
        let latest = &overview.releases[2];
        assert_eq!(latest.executions, ["c1", "c2"]);
        // The first completed execution's versions.
        let first = &latest.firsts[0].point;
        assert_eq!(
            (first.engine.as_deref(), first.workers.clone()),
            (
                Some("0.24.3"),
                Some(BTreeMap::from([
                    ("harness".to_owned(), "1.9.3".to_owned()),
                    ("state".to_owned(), "2.0.0".to_owned()),
                ]))
            )
        );
    }

    #[test]
    fn a_release_control_id_counts_once_per_release_its_latest_attempt() {
        let redispatched = |summary: &mut Value| {
            summary["source"]["release_control_execution_id"] = json!("75b03d86")
        };
        let summaries = [
            rc("p", 1, "1.9.2"),
            with(rc("first", 2, "1.9.3"), redispatched),
            with(rc("second", 3, "1.9.3"), redispatched),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                ("p", vec![scored("t1", 80.0)]),
                ("first", vec![scored("t1", 10.0)]),
                ("second", vec![scored("t1", 90.0)]),
            ]),
        );
        let latest = &overview.releases[1];
        assert_eq!(latest.executions, ["second"]);
        let first = &latest.firsts[0].point;
        assert_eq!(
            (
                first.execution_id.as_str(),
                first.measures.as_ref().unwrap().score_mean
            ),
            ("second", Some(90.0))
        );
    }

    #[test]
    fn one_release_control_id_keeps_each_combination_it_ran() {
        let shared = |summary: &mut Value| {
            summary["source"]["release_control_execution_id"] = json!("75b03d86")
        };
        let summaries = [
            rc("p", 1, "1.9.2"),
            with(rc("regression", 2, "1.9.3"), shared),
            with(
                with(rc("onboarding", 3, "1.9.3"), shared),
                suite("onboarding", "Onboarding"),
            ),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                ("p", vec![scored("t1", 80.0)]),
                ("regression", vec![scored("t1", 90.0)]),
                ("onboarding", vec![scored("t9", 50.0)]),
            ]),
        );
        let latest = &overview.releases[1];
        assert_eq!(latest.executions, ["regression", "onboarding"]);
        assert_eq!(latest.combinations.len(), 2);
        assert_eq!(latest.firsts.len(), 2);
    }

    #[test]
    fn an_execution_with_no_single_semver_release_is_counted_as_unversioned() {
        let summaries = [
            with(rc("built", 1, "1.9.3"), built),
            with(rc("mixed", 2, "1.9.3"), |summary| {
                summary["stack"].as_array_mut().unwrap().push(json!(worker(
                    HARNESS,
                    Some("1.9.4"),
                    None
                )))
            }),
            with(rc("none", 3, "1.9.3"), |summary| {
                summary["stack"] = json!([])
            }),
            rc("ok", 4, "1.9.3"),
            with(rc("gone", 5, "1.9.3"), |summary| {
                built(summary);
                summary["state"] = json!("failed");
            }),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(
                ["built", "mixed", "none", "ok"]
                    .map(|id| (id, vec![scored("t1", 80.0)]))
                    .to_vec(),
            ),
        );
        assert_eq!(overview.unversioned, 3);
        assert_eq!(names(&overview), [["ok"]]);
    }

    #[test]
    fn fewer_than_two_releases_have_no_verdict() {
        let one = overview(
            &rc("l", 1, "1.9.3"),
            &runs(vec![("l", vec![scored("t1", 90.0)])]),
        );
        assert_eq!((one.releases.len(), one.verdict), (1, None));
        assert_eq!(one.releases[0].step, None);

        let empty = overview(&[], &BTreeMap::new());
        assert!(empty.releases.is_empty() && empty.locals.is_empty());
        assert_eq!((empty.verdict, empty.unversioned), (None, 0));
        assert_eq!(
            empty.outside_release_control,
            OutsideCount {
                executions: 0,
                latest_at: None,
            }
        );
    }

    const CORE: [&str; 9] = [
        "persistent_state",
        "tool_contract_recovery",
        "timer_wake",
        "shell_coder_sandbox",
        "database_migration_recovery",
        "contention_ledger",
        "validation_self_repair",
        "context_pressure",
        "prompt_injection_resilience",
    ];
    const APPLICATIONS: [&str; 7] = [
        "registry_implementation",
        "registry_verification",
        "trending_topics_build",
        "alertmanager_route_match",
        "chess_engine_build",
        "form_flow_build",
        "state_machine_canvas_build",
    ];
    const DEEPWORK: [&str; 3] = [
        "chess_engine_build",
        "form_flow_build",
        "state_machine_canvas_build",
    ];

    /// A Release Control execution of the dashboard's fixture on
    /// deepseek-flash and the `default` stack: Harness `harness`, the
    /// engine, runner and context manager given, each test of `suite`
    /// planned `runs` times.
    fn nightly(
        id: &str,
        started_at: &str,
        harness: &str,
        versions: [&str; 3],
        suite: (&str, &str, &[&str]),
        runs: u32,
    ) -> Vec<Value> {
        let [engine, runner, context_manager] = versions;
        let mut summary = listed(id, started_at, Some("default"), &[HARNESS]);
        summary["state"] = json!("completed");
        let run_id = id.split('-').nth(1).unwrap_or_default();
        summary["source"] = json!({
            "kind": "github", "repository": "iii-hq/harness-e2e",
            "run_id": run_id.parse::<u64>().unwrap_or_default(), "run_attempt": 1,
            "url": format!("https://github.com/iii-hq/harness-e2e/actions/runs/{run_id}"),
            "release_control_execution_id": format!("rc-{id}"),
        });
        summary["stack"] = json!([
            worker(HARNESS, Some(harness), None),
            worker("context-manager", Some(context_manager), None),
            worker(RUNNER, Some(runner), None),
        ]);
        let (suite_id, label, tests) = suite;
        summary["parameters"]["suite"] =
            json!({"id": suite_id, "label": label, "sha256": format!("sha256:{suite_id}")});
        summary["parameters"]["model"] = json!("deepseek-flash");
        summary["parameters"]["scenarios"] = json!(tests);
        summary["parameters"]["runs"] = json!(runs);
        let mut native = native(&format!("n-{id}"), id);
        native["engine_version"] = json!(engine);
        vec![summary, native]
    }

    /// Each test of `tests` `runs` times, as `run` makes it.
    fn every(tests: &[&str], runs: usize, run: impl Fn(&str) -> TrendRun) -> Vec<TrendRun> {
        tests
            .iter()
            .flat_map(|test| std::iter::repeat_with(|| run(test)).take(runs))
            .collect()
    }

    /// What the dashboard's tests read (`release-overview.json`): Core
    /// runtime regression on four releases, 1.8.36 dropping
    /// contention_ledger against 1.8.35; Application builds and changes
    /// with a test redefined and one failed in the E2E infrastructure;
    /// Application builds with Deepwork whose every run failed on 1.8.36;
    /// the onboarding tour on 1.8.35 only; three local runs of the Core
    /// setup; three other CI runs and four unversioned ones.
    fn dashboard_fixture() -> ReleaseOverview {
        let core = ("regression", "Core runtime regression", &CORE[..]);
        let applications = (
            "software-engineering",
            "Application builds and changes",
            &APPLICATIONS[..],
        );
        let deepwork = (
            "deepwork",
            "Application builds with Deepwork",
            &DEEPWORK[..],
        );
        let onboarding = (
            "onboarding",
            "Complete the onboarding tour",
            &["onboarding_tour"][..],
        );
        let before = ["0.24.2-rc.2", "0.18.0", "1.1.14"];
        let after = ["0.24.3-rc.1", "0.18.1", "1.1.15"];
        let local = |id: &str, started_at: &str| {
            let mut summary = listed(id, started_at, Some("default"), &[HARNESS]);
            summary["stack"][0]["commit"] = json!(format!("852b87e{}", "0".repeat(33)));
            summary["stack"][0]["dirty"] = json!(true);
            summary["parameters"]["suite"] = json!({"id": "regression", "label": "Core runtime regression", "sha256": "sha256:regression"});
            summary["parameters"]["model"] = json!("deepseek-flash");
            summary["parameters"]["scenarios"] = json!(["contention_ledger", "timer_wake"]);
            vec![summary, native(&format!("n-{id}"), id)]
        };
        let other_ci = |id: &str, started_at: &str| {
            with(
                nightly(id, started_at, "1.8.36", after, core, 3),
                |summary| summary["source"]["release_control_execution_id"] = json!(null),
            )
        };
        let unversioned = |id: &str, started_at: &str| {
            with(nightly(id, started_at, "1.8.34", before, core, 3), built)
        };
        let summaries = [
            nightly(
                "github-35742568444-1",
                "2026-09-20T02:17:00-03:00",
                "1.8.31",
                before,
                core,
                3,
            ),
            unversioned("github-35767230299-1", "2026-09-21T02:17:00-03:00"),
            unversioned("github-35821773226-2", "2026-09-22T02:17:00-03:00"),
            nightly(
                "github-36010839787-2",
                "2026-09-23T02:17:00-03:00",
                "1.8.34",
                before,
                core,
                3,
            ),
            unversioned("github-35959254741-1", "2026-09-23T09:10:00-03:00"),
            unversioned("github-35959254742-1", "2026-09-23T10:40:00-03:00"),
            local("local-local-1", "2026-09-24T15:11:00-03:00"),
            local("local-local-2", "2026-09-24T15:56:00-03:00"),
            local("local-local-3", "2026-09-24T17:58:00-03:00"),
            with(
                nightly(
                    "github-36097908501-1",
                    "2026-09-25T00:17:00-03:00",
                    "1.8.35",
                    before,
                    core,
                    3,
                ),
                |summary| summary["state"] = json!("interrupted"),
            ),
            nightly(
                "github-36097908502-1",
                "2026-09-25T02:17:00-03:00",
                "1.8.35",
                before,
                core,
                3,
            ),
            nightly(
                "github-36097908503-1",
                "2026-09-25T02:47:00-03:00",
                "1.8.35",
                before,
                applications,
                1,
            ),
            nightly(
                "github-36097908504-1",
                "2026-09-25T03:17:00-03:00",
                "1.8.35",
                before,
                deepwork,
                1,
            ),
            nightly(
                "github-36097908505-1",
                "2026-09-25T03:47:00-03:00",
                "1.8.35",
                before,
                onboarding,
                1,
            ),
            other_ci("github-36200000001-1", "2026-09-26T09:40:00-03:00"),
            nightly(
                "github-36285373498-1",
                "2026-09-27T02:17:00-03:00",
                "1.8.36",
                after,
                core,
                3,
            ),
            nightly(
                "github-36285373499-1",
                "2026-09-27T02:47:00-03:00",
                "1.8.36",
                after,
                applications,
                1,
            ),
            nightly(
                "github-36285373500-1",
                "2026-09-27T03:17:00-03:00",
                "1.8.36",
                after,
                deepwork,
                1,
            ),
            other_ci("github-36200000002-1", "2026-09-27T09:40:00-03:00"),
            nightly(
                "github-36381232467-1",
                "2026-09-28T02:17:00-03:00",
                "1.8.36",
                after,
                core,
                3,
            ),
            other_ci("github-36200000003-1", "2026-09-28T09:40:00-03:00"),
        ]
        .concat();
        let passing = |test: &str| scored(test, 100.0);
        // Counted, not completed, scoring 70.
        let dropping = |test: &str| {
            if test == "contention_ledger" {
                run(test, true, false, Some(70.0))
            } else {
                passing(test)
            }
        };
        let applications_scored = |test: &str| {
            let score = 60.0 + 5.0 * APPLICATIONS.iter().position(|t| *t == test).unwrap() as f64;
            scored(test, score)
        };
        overview(
            &summaries,
            &runs(vec![
                ("github-35742568444-1", every(&CORE, 3, dropping)),
                ("github-35767230299-1", every(&CORE, 3, passing)),
                ("github-35821773226-2", every(&CORE, 3, passing)),
                ("github-36010839787-2", every(&CORE, 3, passing)),
                ("github-35959254741-1", every(&CORE, 3, passing)),
                ("github-35959254742-1", every(&CORE, 3, passing)),
                (
                    "local-local-1",
                    vec![scored("contention_ledger", 70.0), passing("timer_wake")],
                ),
                (
                    "local-local-2",
                    vec![passing("contention_ledger"), passing("timer_wake")],
                ),
                (
                    "local-local-3",
                    vec![
                        infra_failed("contention_ledger"),
                        infra_failed("timer_wake"),
                    ],
                ),
                ("github-36097908501-1", every(&CORE[..4], 3, passing)),
                ("github-36097908502-1", every(&CORE, 3, passing)),
                (
                    "github-36097908503-1",
                    every(&APPLICATIONS, 1, applications_scored),
                ),
                (
                    "github-36097908504-1",
                    every(&DEEPWORK, 1, |test| scored(test, 85.0)),
                ),
                (
                    "github-36097908505-1",
                    vec![scored("onboarding_tour", 90.0)],
                ),
                ("github-36200000001-1", every(&CORE, 3, passing)),
                ("github-36285373498-1", every(&CORE, 3, dropping)),
                (
                    "github-36285373499-1",
                    every(&APPLICATIONS, 1, |test| match test {
                        "registry_implementation" => redefined(applications_scored(test)),
                        "registry_verification" => infra_failed(test),
                        _ => applications_scored(test),
                    }),
                ),
                ("github-36285373500-1", every(&DEEPWORK, 1, infra_failed)),
                ("github-36200000002-1", every(&CORE, 3, passing)),
                ("github-36381232467-1", every(&CORE, 3, passing)),
                ("github-36200000003-1", every(&CORE, 3, passing)),
            ]),
        )
    }

    #[test]
    fn the_dashboard_fixture_is_this_overview() {
        let fixture = dashboard_fixture();
        let step = latest_step(&fixture);
        let [core, applications] = step.compared.as_slice() else {
            panic!("two compared: {:?}", step.compared);
        };
        // The first completed execution of 1.8.36, not its newest.
        assert_eq!(core.latest.execution, "github-36285373498-1");
        assert_eq!(
            (core.score_delta, core.completed_delta),
            (Some(-3.3), -11.1)
        );
        assert_eq!(
            applications.tests_left_out,
            [
                left_test("registry_implementation", TestLeftOut::Redefined),
                left_test("registry_verification", TestLeftOut::NotCountedLatest),
            ]
        );
        assert_eq!(
            step.left_out
                .iter()
                .map(|left| (left.combination.suite.as_str(), left.reason))
                .collect::<Vec<_>>(),
            [
                ("deepwork", CombinationLeftOut::AllRunsInfraFailed),
                ("onboarding", CombinationLeftOut::OnlyInOneRelease),
            ]
        );

        let value = serde_json::to_value(&fixture).unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("dashboard/src/test-fixtures/release-overview.json");
        if std::env::var_os("UPDATE_DASHBOARD_FIXTURE").is_some() {
            std::fs::write(
                &path,
                format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
            )
            .unwrap();
        }
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap(),
            value
        );
    }
}
