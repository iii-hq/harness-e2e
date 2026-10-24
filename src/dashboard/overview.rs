//! The Trends first screen, over every execution the Console lists. Only the
//! Release Control executions that counted make releases: a point per
//! Harness release, each compared with the one before it over what both ran
//! alike, the latest pair being the verdict. Local and Docker executions sit
//! beside a release; other CI runs are only counted.
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use super::trends::{
    execution, instant, mean, measures, natives, point, semver_key, source, suite_label, sum,
    Execution, TrendMeasures, TrendPoint, TrendRole, TrendRun, TrendSeriesKey, TrendStack, HARNESS,
    NOT_RECORDED,
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
    pub first_started_at: String,
    /// Oldest first; a Release Control id run again counts its latest attempt.
    pub executions: Vec<String>,
    pub combinations: Vec<Combination>,
    /// Over every combination it ran.
    pub aggregate: Option<Aggregate>,
    /// Combinations the release before it did not run.
    pub joined: Vec<Combination>,
    /// Combinations the release before it ran and this one did not.
    pub left: Vec<Combination>,
    /// Planned tests its executions never ran.
    pub tests_not_run: usize,
    /// Of its newest execution.
    pub engine: Option<String>,
    /// Of its newest execution.
    pub workers: Option<BTreeMap<String, String>>,
    /// Against the release before it; null for the first.
    pub step: Option<ReleaseStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct ReleaseStep {
    pub previous: String,
    /// Over what both releases ran alike; null when nothing was.
    pub latest_aggregate: Option<Aggregate>,
    pub previous_aggregate: Option<Aggregate>,
}

/// Combinations, each weighing the same however often it ran.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct Aggregate {
    /// The mean of their means; `completed`, `planned`, `function_calls`
    /// and `function_call_errors` added up.
    pub measures: TrendMeasures,
    /// The mean of their `completed / planned`.
    pub completed_share: Option<f64>,
    /// The mean of their `function_call_errors / function_calls`, those
    /// without a call left out.
    pub error_rate: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct Verdict {
    pub latest: String,
    pub previous: String,
    /// When the latest release's newest execution started.
    pub latest_started_at: String,
    /// Over `compared`; null when it is empty.
    pub latest_aggregate: Option<Aggregate>,
    pub previous_aggregate: Option<Aggregate>,
    /// The combinations both releases ran, over the tests both planned alike.
    pub compared: Vec<CombinationDelta>,
    pub left_out: Vec<LeftOutCombination>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct CombinationDelta {
    pub combination: Combination,
    pub latest: TrendMeasures,
    pub previous: TrendMeasures,
    /// Tests that do not compare: their definition changed (or is unknown),
    /// or only one release planned or ran them. A test both planned that one
    /// release never ran compares, as not completed there.
    pub tests_left_out: Vec<String>,
    /// The latest against the previous over the tests compared, as
    /// `harness-e2e compare` reads them; without a verdict when a side ran
    /// one run per test. Only the latest pair has one.
    pub calibrated: Option<crate::verdict::Summary>,
}

/// `only_in: None`: ran on both, but no test kept its `behavior_sha256`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct LeftOutCombination {
    pub combination: Combination,
    /// The only release of the two that ran it.
    pub only_in: Option<String>,
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
    pub measures: Option<TrendMeasures>,
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

/// A release's kept executions, oldest first, and by combination.
struct Line<'a> {
    release: &'a str,
    kept: Vec<&'a Ran<'a>>,
    combinations: BTreeMap<Key, (Combination, Vec<&'a Ran<'a>>)>,
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
    for ran in &executions {
        let finished = matches!(
            ran.execution.summary["state"].as_str(),
            Some("completed" | "interrupted")
        );
        if ran.point.source.role != TrendRole::ReleaseControl || !finished || ran.point.counted == 0
        {
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

    let mut points = Vec::<ReleasePoint>::new();
    for (index, line) in lines.iter().enumerate() {
        let before = index.checked_sub(1).map(|before| &lines[before]);
        let newest = line.kept[line.kept.len() - 1];
        let measured = line
            .combinations
            .values()
            .filter_map(|(_, group)| measured(group, |_| true))
            .collect::<Vec<_>>();
        let measured = measured.iter().collect::<Vec<_>>();
        points.push(ReleasePoint {
            release: line.release.to_owned(),
            first_started_at: line.kept[0].execution.started_at.to_owned(),
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
            aggregate: aggregate(&measured, &measured),
            joined: before
                .map(|before| missing(line, before))
                .unwrap_or_default(),
            left: before
                .map(|before| missing(before, line))
                .unwrap_or_default(),
            tests_not_run: line
                .kept
                .iter()
                .flat_map(|ran| &ran.point.tests)
                .filter(|test| test.state == "not_run")
                .count(),
            engine: newest.point.engine.clone(),
            workers: newest.point.workers.clone(),
            step: before.map(|before| {
                let (latest_aggregate, previous_aggregate) =
                    aggregates(&compare(line, before, false).0);
                ReleaseStep {
                    previous: before.release.to_owned(),
                    latest_aggregate,
                    previous_aggregate,
                }
            }),
        });
    }
    let verdict = match lines.as_slice() {
        [.., previous, latest] => {
            let (compared, left_out) = compare(latest, previous, true);
            let (latest_aggregate, previous_aggregate) = aggregates(&compared);
            Some(Verdict {
                latest: latest.release.to_owned(),
                previous: previous.release.to_owned(),
                latest_started_at: latest.kept[latest.kept.len() - 1]
                    .execution
                    .started_at
                    .to_owned(),
                latest_aggregate,
                previous_aggregate,
                compared,
                left_out,
            })
        }
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
                measures: ran.point.measures.clone(),
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
            let (series, stack) = key.clone();
            let combination = Combination {
                suite: series.suite,
                suite_label: suite_label(&group[group.len() - 1].execution),
                provider: series.provider,
                model: series.model,
                profile: series.profile,
                stack,
            };
            (key, (combination, group))
        })
        .collect();
    Line {
        release,
        kept,
        combinations,
    }
}

/// The combinations `line` ran and `other` did not.
fn missing(line: &Line, other: &Line) -> Vec<Combination> {
    line.combinations
        .iter()
        .filter(|(key, _)| !other.combinations.contains_key(*key))
        .map(|(_, (combination, _))| combination.clone())
        .collect()
}

/// A release against an earlier one: each combination both ran, over the
/// tests both ran alike; every other combination is left out. `calibrate`
/// also reads each combination's calibrated verdict.
fn compare(
    latest: &Line,
    previous: &Line,
    calibrate: bool,
) -> (Vec<CombinationDelta>, Vec<LeftOutCombination>) {
    let (mut compared, mut left_out) = (Vec::new(), Vec::new());
    let keys = latest
        .combinations
        .keys()
        .chain(previous.combinations.keys())
        .collect::<BTreeSet<_>>();
    let left = |combination: &Combination, only_in: Option<&str>| LeftOutCombination {
        combination: combination.clone(),
        only_in: only_in.map(str::to_owned),
    };
    for key in keys {
        match (latest.combinations.get(key), previous.combinations.get(key)) {
            (Some((combination, ran)), Some((_, ran_before))) => {
                match delta(combination, ran, ran_before, calibrate) {
                    Some(delta) => compared.push(delta),
                    None => left_out.push(left(combination, None)),
                }
            }
            (Some((combination, _)), None) => {
                left_out.push(left(combination, Some(latest.release)))
            }
            (None, Some((combination, _))) => {
                left_out.push(left(combination, Some(previous.release)))
            }
            (None, None) => {}
        }
    }
    (compared, left_out)
}

/// A combination both releases ran, measured over the tests both planned
/// or ran under one same definition; none when no test was. A release that
/// counted no run of such a test keeps it planned: it adds nothing to the
/// means and lowers Completed.
fn delta(
    combination: &Combination,
    latest: &[&Ran],
    previous: &[&Ran],
    calibrate: bool,
) -> Option<CombinationDelta> {
    let (latest_counted, previous_counted) = (counted(latest), counted(previous));
    let (latest_tests, previous_tests) = (tests(latest), tests(previous));
    let alike = latest_tests
        .intersection(&previous_tests)
        .copied()
        .filter(|test| {
            match (
                definition(&latest_counted, test),
                definition(&previous_counted, test),
            ) {
                (Some(Some(latest)), Some(Some(previous))) => latest == previous,
                (Some(None), _) | (_, Some(None)) => false,
                _ => true,
            }
        })
        .collect::<BTreeSet<_>>();
    let tests = latest_tests
        .union(&previous_tests)
        .copied()
        .collect::<BTreeSet<_>>();
    let keep = |test: &str| alike.contains(test);
    let runs = |group: &[&Ran]| {
        group
            .iter()
            .flat_map(|ran| ran.runs.iter().copied())
            .filter(|run| keep(&run.test))
            .map(verdict_run)
            .collect::<Vec<_>>()
    };
    Some(CombinationDelta {
        combination: combination.clone(),
        latest: measured(latest, keep)?,
        previous: measured(previous, keep)?,
        tests_left_out: tests
            .difference(&alike)
            .map(|test| (*test).to_owned())
            .collect(),
        calibrated: calibrate.then(|| {
            crate::verdict::compare(
                &runs(previous),
                &runs(latest),
                &crate::verdict::Rule::default(),
            )
            .summary()
        }),
    })
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

/// The tests a release's executions of a combination planned or ran.
fn tests<'a>(group: &[&'a Ran]) -> BTreeSet<&'a str> {
    group
        .iter()
        .flat_map(|ran| {
            let planned = ran.execution.parameters.scenarios.iter();
            planned
                .map(String::as_str)
                .chain(ran.runs.iter().map(|run| run.test.as_str()))
        })
        .collect()
}

fn counted<'a>(group: &[&Ran<'a>]) -> Vec<&'a TrendRun> {
    group
        .iter()
        .flat_map(|ran| ran.runs.iter().copied())
        .filter(|run| run.counted)
        .collect()
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

/// A combination's executions pooled: the runs of the tests kept, against
/// the runs planned for them.
fn measured(group: &[&Ran], keep: impl Fn(&str) -> bool) -> Option<TrendMeasures> {
    let runs = group
        .iter()
        .flat_map(|ran| ran.runs.iter().copied())
        .filter(|run| keep(&run.test))
        .collect::<Vec<_>>();
    let planned = group
        .iter()
        .map(|ran| planned(&ran.execution, &ran.runs, &keep))
        .sum();
    measures(&runs, planned)
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

/// Each delta's latest and previous measures, aggregated: each measure over
/// the combinations both releases have it for.
fn aggregates(compared: &[CombinationDelta]) -> (Option<Aggregate>, Option<Aggregate>) {
    let latest = compared
        .iter()
        .map(|delta| &delta.latest)
        .collect::<Vec<_>>();
    let previous = compared
        .iter()
        .map(|delta| &delta.previous)
        .collect::<Vec<_>>();
    (aggregate(&latest, &previous), aggregate(&previous, &latest))
}

/// Combinations weighing the same: the mean of their means and of their
/// shares, each over the combinations `other` (one per combination, in the
/// same order) has it for too; their counts added up, to show, never to
/// divide.
fn aggregate(all: &[&TrendMeasures], other: &[&TrendMeasures]) -> Option<Aggregate> {
    let mean_of = |of: fn(&TrendMeasures) -> Option<f64>| {
        mean(all.iter().zip(other).map(|(m, o)| of(o).and(of(m))))
    };
    (!all.is_empty()).then(|| Aggregate {
        measures: TrendMeasures {
            score_mean: mean_of(|m| m.score_mean),
            completed: all.iter().map(|m| m.completed).sum(),
            planned: all.iter().map(|m| m.planned).sum(),
            duration_ms_mean: mean_of(|m| m.duration_ms_mean),
            input_tokens_mean: mean_of(|m| m.input_tokens_mean),
            function_calls_mean: mean_of(|m| m.function_calls_mean),
            function_calls: sum(all.iter().map(|m| m.function_calls)),
            function_call_errors: sum(all.iter().map(|m| m.function_call_errors)),
            turns_mean: mean_of(|m| m.turns_mean),
            cost_usd_mean: mean_of(|m| m.cost_usd_mean),
            output_tokens_mean: mean_of(|m| m.output_tokens_mean),
            cache_read_tokens_mean: mean_of(|m| m.cache_read_tokens_mean),
            cache_write_tokens_mean: mean_of(|m| m.cache_write_tokens_mean),
            reasoning_tokens_mean: mean_of(|m| m.reasoning_tokens_mean),
            validation_retries_mean: mean_of(|m| m.validation_retries_mean),
            technical_retries_mean: mean_of(|m| m.technical_retries_mean),
        },
        completed_share: mean_of(|m| {
            (m.planned > 0).then(|| m.completed as f64 / m.planned as f64)
        }),
        error_rate: mean_of(|m| match (m.function_call_errors, m.function_calls) {
            (Some(errors), Some(calls)) if calls > 0.0 => Some(errors / calls),
            _ => None,
        }),
    })
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

    fn suite(id: &str, label: &str) -> impl FnOnce(&mut Value) {
        let suite = json!({"id": id, "label": label, "sha256": format!("sha256:{id}")});
        move |summary| summary["parameters"]["suite"] = suite
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

    fn score(aggregate: &Option<Aggregate>) -> Option<f64> {
        aggregate.as_ref()?.measures.score_mean
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
            overview.verdict.unwrap().compared[0]
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
                .flat_map(|test| (0..4).map(move |i| scored(&format!("t{test}"), score + i as f64)))
                .collect::<Vec<_>>()
        };
        let dropped = calibrated(repeated(90.0), repeated(60.0));
        assert_eq!(dropped.verdict, Some(crate::verdict::Verdict::Worse));
        // The read model keeps no cause, and the summary carries the warnings.
        assert!(dropped.warnings[0].contains("carry no cause"));
        assert!(dropped
            .warnings
            .iter()
            .any(|warning| warning.starts_with("t0 fell 30.0 points")));
        let quality = dropped.quality.unwrap();
        assert_eq!((quality.a, quality.b, quality.tests), (91.5, 61.5, 4));
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
                ("l2", vec![scored("t1", 90.0)]),
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
        assert_eq!(overview.releases[1].first_started_at, at(3));
        let [delta] = verdict.compared.as_slice() else {
            panic!("one delta: {:?}", verdict.compared);
        };
        assert_eq!(
            delta.combination,
            combination("regression", "Regression", "flash")
        );
        // t2 changed its definition: only t1 compares, run by l and l2.
        assert_eq!(delta.tests_left_out, ["t2"]);
        assert_eq!(
            (delta.latest.score_mean, delta.previous.score_mean),
            (Some(90.0), Some(80.0))
        );
        assert_eq!((delta.latest.completed, delta.latest.planned), (2, 2));
        assert_eq!(
            verdict.left_out,
            [
                LeftOutCombination {
                    combination: combination("onboarding", "Onboarding", "flash"),
                    only_in: Some("1.9.3".into()),
                },
                LeftOutCombination {
                    combination: combination("se", "SE", "flash"),
                    only_in: None,
                },
            ]
        );
        assert_eq!(
            (
                score(&verdict.latest_aggregate),
                score(&verdict.previous_aggregate)
            ),
            (Some(90.0), Some(80.0))
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
            score(&overview.verdict.as_ref().unwrap().latest_aggregate),
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
        assert_eq!(mark.measures.as_ref().unwrap().score_mean, Some(0.0));
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
        assert_eq!(overview.locals[0].measures, None);
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
        assert_eq!(
            (latest.executions.as_slice(), latest.tests_not_run),
            (&["l".to_owned()][..], 1)
        );
        let measures = &latest.aggregate.as_ref().unwrap().measures;
        assert_eq!(
            (measures.completed, measures.planned, measures.score_mean),
            (1, 2, Some(90.0))
        );
        assert_eq!(overview.verdict.unwrap().latest, "1.9.3");
    }

    #[test]
    fn a_test_both_releases_planned_that_the_latest_never_ran_lowers_its_completed_share() {
        let summaries = [
            rc("p", 1, "1.9.2"),
            with(rc("l", 2, "1.9.3"), |summary| {
                summary["state"] = json!("interrupted");
                summary["parameters"]["scenarios"] = json!(["t1", "t2", "t3"]);
            }),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                ("p", vec![scored("t1", 80.0), scored("t2", 60.0)]),
                ("l", vec![scored("t1", 90.0)]),
            ]),
        );
        let verdict = overview.verdict.as_ref().unwrap();
        let [delta] = verdict.compared.as_slice() else {
            panic!("one delta: {:?}", verdict.compared);
        };
        // t2 stays planned on both sides; only the latest planned t3.
        assert_eq!(delta.tests_left_out, ["t3"]);
        assert_eq!((delta.latest.completed, delta.latest.planned), (1, 2));
        assert_eq!((delta.previous.completed, delta.previous.planned), (2, 2));
        let share = |aggregate: &Option<Aggregate>| aggregate.as_ref()?.completed_share;
        assert_eq!(
            (
                share(&verdict.latest_aggregate),
                share(&verdict.previous_aggregate)
            ),
            (Some(0.5), Some(1.0))
        );
        let step = overview.releases[1].step.as_ref().unwrap();
        assert_eq!(share(&step.latest_aggregate), Some(0.5));
    }

    #[test]
    fn each_measure_compares_only_the_combinations_both_releases_measured() {
        let priced = |cost: Option<f64>, run: TrendRun| TrendRun {
            cost_usd: cost,
            ..run
        };
        let summaries = [
            rc("p1", 1, "1.9.2"),
            with(rc("p2", 1, "1.9.2"), suite("onboarding", "Onboarding")),
            rc("l1", 2, "1.9.3"),
            with(rc("l2", 2, "1.9.3"), suite("onboarding", "Onboarding")),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                ("p1", vec![priced(Some(0.25), scored("t1", 80.0))]),
                ("p2", vec![priced(Some(1.0), scored("t9", 40.0))]),
                ("l1", vec![priced(Some(0.5), scored("t1", 90.0))]),
                // Onboarding's model has no price on the latest release.
                ("l2", vec![priced(None, scored("t9", 60.0))]),
            ]),
        );
        let verdict = overview.verdict.as_ref().unwrap();
        let measures = |aggregate: &Option<Aggregate>| aggregate.as_ref().unwrap().measures.clone();
        let (latest, previous) = (
            measures(&verdict.latest_aggregate),
            measures(&verdict.previous_aggregate),
        );
        // The cost over regression alone on both sides; the score over both.
        assert_eq!(
            (latest.cost_usd_mean, previous.cost_usd_mean),
            (Some(0.5), Some(0.25))
        );
        assert_eq!(
            (latest.score_mean, previous.score_mean),
            (Some(75.0), Some(60.0))
        );
        // A release's own point still reads every combination it measured.
        assert_eq!(
            overview.releases[0]
                .aggregate
                .as_ref()
                .unwrap()
                .measures
                .cost_usd_mean,
            Some(0.625)
        );
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
                .map(|release| (release.release.as_str(), release.first_started_at.clone()))
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
    fn with_no_shared_combination_the_verdict_compares_nothing_and_the_line_says_what_changed() {
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
        let verdict = overview.verdict.as_ref().unwrap();
        assert!(verdict.compared.is_empty());
        assert_eq!(
            (&verdict.latest_aggregate, &verdict.previous_aggregate),
            (&None, &None)
        );
        let (flash, pro) = (
            combination("regression", "Regression", "flash"),
            combination("regression", "Regression", "pro"),
        );
        assert_eq!(
            verdict.left_out,
            [
                LeftOutCombination {
                    combination: flash.clone(),
                    only_in: Some("1.9.3".into()),
                },
                LeftOutCombination {
                    combination: pro.clone(),
                    only_in: Some("1.9.2".into()),
                },
            ]
        );
        let (first, latest) = (&overview.releases[0], &overview.releases[1]);
        assert!(first.joined.is_empty() && first.left.is_empty());
        assert_eq!(
            (latest.joined.clone(), latest.left.clone()),
            (vec![flash], vec![pro])
        );
        assert_eq!(
            latest.step,
            Some(ReleaseStep {
                previous: "1.9.2".into(),
                latest_aggregate: None,
                previous_aggregate: None,
            })
        );
    }

    #[test]
    fn each_shared_combination_weighs_the_same_and_runs_pool_within_a_release() {
        let summaries = [
            rc("p1", 1, "1.9.2"),
            with(rc("p2", 1, "1.9.2"), suite("onboarding", "Onboarding")),
            rc("l1", 2, "1.9.3"),
            rc("l2", 3, "1.9.3"),
            with(rc("l3", 3, "1.9.3"), suite("onboarding", "Onboarding")),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                ("p1", vec![scored("t1", 50.0)]),
                ("p2", vec![scored("t9", 30.0)]),
                ("l1", vec![scored("t1", 100.0)]),
                ("l2", vec![scored("t1", 60.0); 3]),
                ("l3", vec![scored("t9", 30.0)]),
            ]),
        );
        let verdict = overview.verdict.as_ref().unwrap();
        // Regression pools l1 and l2: (100 + 3 × 60) / 4.
        assert_eq!(
            verdict
                .compared
                .iter()
                .map(|delta| (delta.combination.suite.as_str(), delta.latest.score_mean))
                .collect::<Vec<_>>(),
            [("onboarding", Some(30.0)), ("regression", Some(70.0))]
        );
        assert_eq!(
            (
                score(&verdict.latest_aggregate),
                score(&verdict.previous_aggregate)
            ),
            (Some(50.0), Some(40.0))
        );
        let latest = &verdict.latest_aggregate.as_ref().unwrap().measures;
        assert_eq!(
            (latest.completed, latest.planned, latest.function_calls),
            (5, 5, Some(50.0))
        );
        assert_eq!(latest.duration_ms_mean, Some(1_000.0));
        assert_eq!(score(&overview.releases[1].aggregate), Some(50.0));
    }

    #[test]
    fn completed_share_and_error_rate_weigh_each_combination_the_same_however_often_it_ran() {
        let regression = || {
            vec![
                scored("t1", 80.0),
                TrendRun {
                    completed: false,
                    ..scored("t2", 0.0)
                },
            ]
        };
        let onboarding = || {
            ["t8", "t9"]
                .map(|test| TrendRun {
                    function_call_errors: Some(0.0),
                    ..scored(test, 50.0)
                })
                .to_vec()
        };
        let summaries = [
            rc("p1", 1, "1.9.2"),
            with(rc("p2", 1, "1.9.2"), suite("onboarding", "Onboarding")),
            rc("l1", 2, "1.9.3"),
            rc("l2", 2, "1.9.3"),
            rc("l3", 2, "1.9.3"),
            with(rc("l4", 2, "1.9.3"), suite("onboarding", "Onboarding")),
        ]
        .concat();
        let overview = overview(
            &summaries,
            &runs(vec![
                ("p1", regression()),
                ("p2", onboarding()),
                ("l1", regression()),
                ("l2", regression()),
                ("l3", regression()),
                ("l4", onboarding()),
            ]),
        );
        let verdict = overview.verdict.as_ref().unwrap();
        // Regression completes 1 run in 2 with 1 error in 10 calls however
        // often it ran; onboarding completes all with none.
        for aggregate in [
            &verdict.latest_aggregate,
            &verdict.previous_aggregate,
            &overview.releases[0].aggregate,
            &overview.releases[1].aggregate,
        ] {
            let aggregate = aggregate.as_ref().unwrap();
            assert_eq!(
                (aggregate.completed_share, aggregate.error_rate),
                (Some(0.75), Some(0.05))
            );
        }
        let counts = |aggregate: &Option<Aggregate>| {
            let measures = &aggregate.as_ref().unwrap().measures;
            (
                measures.completed,
                measures.planned,
                measures.function_calls,
                measures.function_call_errors,
            )
        };
        assert_eq!(
            counts(&verdict.latest_aggregate),
            (5, 8, Some(80.0), Some(6.0))
        );
        assert_eq!(
            counts(&verdict.previous_aggregate),
            (3, 4, Some(40.0), Some(2.0))
        );
    }

    #[test]
    fn every_release_steps_from_the_release_before_it() {
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
                ("c2", vec![scored("t1", 90.0)]),
            ]),
        );
        let steps = overview
            .releases
            .iter()
            .map(|release| {
                let step = release.step.as_ref()?;
                Some((
                    step.previous.as_str(),
                    score(&step.latest_aggregate),
                    score(&step.previous_aggregate),
                ))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            steps,
            [
                None,
                Some(("1.9.1", Some(70.0), Some(50.0))),
                Some(("1.9.2", Some(90.0), Some(70.0))),
            ]
        );
        let verdict = overview.verdict.as_ref().unwrap();
        let latest = &overview.releases[2];
        assert_eq!(
            latest.step,
            Some(ReleaseStep {
                previous: verdict.previous.clone(),
                latest_aggregate: verdict.latest_aggregate.clone(),
                previous_aggregate: verdict.previous_aggregate.clone(),
            })
        );
        // The newest execution's versions.
        assert_eq!(
            (latest.engine.as_deref(), latest.workers.clone()),
            (
                Some("0.24.3"),
                Some(BTreeMap::from([
                    ("harness".to_owned(), "1.9.3".to_owned()),
                    ("state".to_owned(), "2.0.1".to_owned()),
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
        assert_eq!(latest.first_started_at, at(3));
        assert_eq!(score(&latest.aggregate), Some(90.0));
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
}
