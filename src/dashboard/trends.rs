//! How one suite moves over time on one model and profile: each execution of
//! a series is a point, measured over its technically valid runs only.
//! Everything here is computed from what the read model already holds.
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::plans::store::{ExecutionParameters, ExecutionSource, StackWorker, Where};

pub(super) const NOT_RECORDED: &str = "not_recorded";
pub(super) const ANY: &str = "any";
/// Stack rows that are the runner and the engine, not workers of the stack.
const RUNNER: &str = "harness-e2e";
const ENGINE: &str = "compose";

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub(super) struct TrendsRequest {
    /// A suite id, or the sha256 of an unsaved suite.
    #[serde(default)]
    pub suite: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Null or empty: no profile; absent: any.
    #[serde(default, deserialize_with = "present")]
    pub profile: Option<Option<String>>,
    /// A stack name, `not_recorded` or `any`; absent, or one the series
    /// never ran on: the stack of the series' latest execution.
    #[serde(default)]
    pub stack: Option<String>,
}

/// A field sent as null is present: `Some(None)`.
fn present<'de, D: Deserializer<'de>>(value: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(value).map(Some)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
pub(super) struct TrendSeriesKey {
    pub suite: String,
    pub provider: String,
    pub model: String,
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct TrendSeries {
    #[serde(flatten)]
    pub key: TrendSeriesKey,
    pub suite_label: String,
    pub executions: usize,
    pub latest_at: String,
    pub r#where: Vec<Where>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub(super) struct StackCount {
    pub name: String,
    pub executions: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct TrendsResponse {
    /// Every series, latest execution first.
    pub series: Vec<TrendSeries>,
    /// The request's series, else the one with the latest execution.
    pub selected: Option<TrendSeriesKey>,
    /// The stack filter applied.
    pub stack: String,
    /// Of the selected series, then `not_recorded` (if any) and `any`.
    pub stacks: Vec<StackCount>,
    /// Oldest first.
    pub points: Vec<TrendPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct TrendSource {
    /// `local`, `docker` or `github`.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_control_execution_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub(super) struct TrendStack {
    pub name: Option<String>,
    /// Not recorded, joined by its workers' names.
    pub matched_by_workers: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct TrendPoint {
    pub execution_id: String,
    pub label: Option<String>,
    pub started_at: String,
    pub source: TrendSource,
    pub stack: TrendStack,
    pub engine: Option<String>,
    pub runner: Option<String>,
    /// Identity per worker, runner and engine left out; null: not recorded.
    pub workers: Option<BTreeMap<String, String>>,
    pub planned: Option<Vec<String>>,
    /// Runs observed.
    pub runs: usize,
    /// Technically valid runs.
    pub counted: usize,
    /// When nothing counted: the execution's error, else why.
    pub reason: Option<String>,
    /// Null when nothing counted.
    pub measures: Option<TrendMeasures>,
    /// Planned tests first, in plan order, then any other that ran.
    pub tests: Vec<TrendTest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct TrendMeasures {
    pub score_mean: Option<f64>,
    pub completed: usize,
    /// The runs planned (tests × runs per test), else the runs observed:
    /// what `completed` counts.
    pub planned: usize,
    pub duration_ms_mean: Option<f64>,
    pub input_tokens_mean: Option<f64>,
    pub function_calls_mean: Option<f64>,
    pub function_calls: Option<f64>,
    pub function_call_errors: Option<f64>,
    pub turns_mean: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct TrendTest {
    pub id: String,
    /// `scored`, `no_score`, `technical_invalid` or `not_run`.
    pub state: String,
    /// Mean of the counted runs' scores.
    pub score: Option<f64>,
    pub behavior_sha256: Option<String>,
}

/// One run of a test as the read model holds it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct TrendRun {
    pub test: String,
    pub behavior_sha256: Option<String>,
    /// Technically valid: only these are measured.
    pub counted: bool,
    pub completed: bool,
    pub score: Option<f64>,
    pub duration_ms: Option<f64>,
    pub input_tokens: Option<f64>,
    pub function_calls: Option<f64>,
    pub function_call_errors: Option<f64>,
    pub turns: Option<f64>,
}

/// An execution of the Console that belongs to a series.
struct Execution<'a> {
    summary: &'a Value,
    id: &'a str,
    started_at: &'a str,
    key: TrendSeriesKey,
    parameters: ExecutionParameters,
    workers: Vec<StackWorker>,
}

/// The trends of the requested series, from the executions the Console
/// lists (its own and, marked with their parent, their native runs) and the
/// runs of each native run.
pub(super) fn trends(
    request: &TrendsRequest,
    summaries: &[Value],
    runs: &BTreeMap<String, Vec<TrendRun>>,
) -> TrendsResponse {
    let mut children = BTreeMap::<&str, Vec<&Value>>::new();
    for summary in summaries {
        if let Some(parent) = summary["parent_plan_execution_id"].as_str() {
            children.entry(parent).or_default().push(summary);
        }
    }
    let mut groups = BTreeMap::<TrendSeriesKey, Vec<Execution>>::new();
    for execution in summaries.iter().filter_map(execution) {
        groups
            .entry(execution.key.clone())
            .or_default()
            .push(execution);
    }
    for executions in groups.values_mut() {
        executions.sort_by(|left, right| instant(left.started_at).cmp(&instant(right.started_at)));
    }
    let mut ordered = groups.into_values().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        let (left, right) = (left.last().unwrap(), right.last().unwrap());
        instant(right.started_at).cmp(&instant(left.started_at))
    });
    let series = ordered
        .iter()
        .map(|executions| {
            let latest = executions.last().unwrap();
            let mut r#where = Vec::new();
            for execution in executions {
                if !r#where.contains(&execution.parameters.r#where) {
                    r#where.push(execution.parameters.r#where);
                }
            }
            TrendSeries {
                key: latest.key.clone(),
                suite_label: latest
                    .parameters
                    .suite
                    .as_ref()
                    .map(|suite| suite.label.clone())
                    .filter(|label| !label.is_empty())
                    .unwrap_or_else(|| latest.key.suite.clone()),
                executions: executions.len(),
                latest_at: latest.started_at.to_owned(),
                r#where,
            }
        })
        .collect::<Vec<_>>();
    let Some(executions) = ordered
        .iter()
        .find(|executions| requested(request, &executions[0].key))
        .or(ordered.first())
    else {
        return TrendsResponse {
            series,
            selected: None,
            stack: ANY.into(),
            stacks: Vec::new(),
            points: Vec::new(),
        };
    };

    let stacks = stacks_of(executions);
    let name = |stack: &TrendStack| stack.name.clone().unwrap_or_else(|| NOT_RECORDED.into());
    let mut counts = Vec::<StackCount>::new();
    for stack in stacks.iter().rev() {
        let name = name(stack);
        match counts.iter_mut().find(|count| count.name == name) {
            Some(count) => count.executions += 1,
            None => counts.push(StackCount {
                name,
                executions: 1,
            }),
        }
    }
    // Recorded stacks first, latest first; then the unrecorded ones and all.
    counts.sort_by_key(|count| count.name == NOT_RECORDED);
    counts.push(StackCount {
        name: ANY.into(),
        executions: executions.len(),
    });
    // A stack the series never ran on filters nothing: the default applies.
    let applied = request
        .stack
        .clone()
        .filter(|stack| counts.iter().any(|count| &count.name == stack))
        .unwrap_or_else(|| name(stacks.last().unwrap()));
    let points = executions
        .iter()
        .zip(stacks)
        .filter(|(_, stack)| applied == ANY || name(stack) == applied)
        .map(|(execution, stack)| {
            let natives = children
                .get(execution.id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let runs = natives
                .iter()
                .filter_map(|native| runs.get(native["id"].as_str()?))
                .flatten()
                .collect::<Vec<_>>();
            point(execution, stack, natives, &runs)
        })
        .collect();
    TrendsResponse {
        series,
        selected: Some(executions[0].key.clone()),
        stack: applied,
        stacks: counts,
        points,
    }
}

/// The execution a summary lists, when it belongs to a series: an execution
/// of the Console (not one of its native runs) that names a suite and a model.
fn execution(summary: &Value) -> Option<Execution<'_>> {
    if summary.get("parent_plan_execution_id").is_some() {
        return None;
    }
    let parameters =
        serde_json::from_value::<ExecutionParameters>(summary["parameters"].clone()).ok()?;
    let suite = parameters.suite.as_ref()?;
    let suite = suite
        .id
        .clone()
        .or_else(|| Some(suite.sha256.clone()).filter(|sha| !sha.is_empty()))?;
    if parameters.model.is_empty() {
        return None;
    }
    Some(Execution {
        summary,
        id: summary["id"].as_str()?,
        started_at: summary["started_at"].as_str().unwrap_or_default(),
        key: TrendSeriesKey {
            suite,
            provider: parameters.provider.clone(),
            model: parameters.model.clone(),
            profile: parameters.agent.clone().filter(|agent| !agent.is_empty()),
        },
        workers: serde_json::from_value(summary["stack"].clone()).unwrap_or_default(),
        parameters,
    })
}

/// Whether a series is the one asked for: every field sent matches.
fn requested(request: &TrendsRequest, key: &TrendSeriesKey) -> bool {
    let matches = |asked: &Option<String>, value: &str| asked.as_deref().is_none_or(|a| a == value);
    matches(&request.suite, &key.suite)
        && matches(&request.provider, &key.provider)
        && matches(&request.model, &key.model)
        && request.profile.as_ref().is_none_or(|profile| {
            profile.as_deref().filter(|p| !p.is_empty()) == key.profile.as_deref()
        })
}

/// The stack of each execution (oldest first). One not recorded joins the
/// stack whose earliest execution ran workers of the same names.
fn stacks_of(executions: &[Execution]) -> Vec<TrendStack> {
    let mut earliest = Vec::<(&str, BTreeSet<&str>)>::new();
    for execution in executions {
        if let Some(stack) = &execution.parameters.stack {
            if !earliest.iter().any(|(name, _)| *name == stack.name) {
                earliest.push((&stack.name, names(&execution.workers)));
            }
        }
    }
    executions
        .iter()
        .map(|execution| match &execution.parameters.stack {
            Some(stack) => TrendStack {
                name: Some(stack.name.clone()),
                matched_by_workers: false,
            },
            None => {
                let workers = names(&execution.workers);
                let matched = earliest
                    .iter()
                    .find(|(_, names)| !workers.is_empty() && *names == workers)
                    .map(|(name, _)| (*name).to_owned());
                TrendStack {
                    matched_by_workers: matched.is_some(),
                    name: matched,
                }
            }
        })
        .collect()
}

fn names(workers: &[StackWorker]) -> BTreeSet<&str> {
    workers.iter().map(|worker| worker.name.as_str()).collect()
}

/// The release a worker ran: the version its compose lock resolved; on this
/// harness, which runs from its compose file and no lock, the version the
/// file pinned when it names one (`latest` does not).
fn version(worker: &StackWorker, local: bool) -> Option<String> {
    worker.resolved.clone().or_else(|| {
        worker
            .requested
            .clone()
            .filter(|version| local && !version.is_empty() && version != "latest")
    })
}

/// What identifies the build a worker ran: its commit (`@` and 7 hex, `*`
/// when the checkout had uncommitted changes), else its release. Never
/// `observed`: that is the binary's own Cargo version, which releases did
/// not always move.
pub(super) fn identity(worker: &StackWorker, local: bool) -> Option<String> {
    if let Some(commit) = worker.commit.as_deref().filter(|commit| !commit.is_empty()) {
        let dirty = if worker.dirty == Some(true) { "*" } else { "" };
        return Some(format!("@{}{dirty}", &commit[..commit.len().min(7)]));
    }
    version(worker, local)
}

/// Every worker's identity, runner and engine left out; a worker that ran
/// different builds in different groups lists them all.
fn workers(rows: &[StackWorker], local: bool) -> Option<BTreeMap<String, String>> {
    let mut builds = BTreeMap::<&str, BTreeSet<String>>::new();
    for row in rows
        .iter()
        .filter(|row| ![RUNNER, ENGINE].contains(&row.name.as_str()))
    {
        if let Some(identity) = identity(row, local) {
            builds.entry(&row.name).or_default().insert(identity);
        }
    }
    (!builds.is_empty()).then(|| {
        builds
            .into_iter()
            .map(|(name, builds)| {
                (
                    name.to_owned(),
                    builds.into_iter().collect::<Vec<_>>().join(", "),
                )
            })
            .collect()
    })
}

/// The runner's release (its own version when none is recorded: the runner
/// reports it as released), with the commit it was built from when it ran
/// from a checkout.
fn runner(rows: &[StackWorker], local: bool) -> Option<String> {
    let row = rows.iter().find(|row| row.name == RUNNER)?;
    let version = version(row, local).or_else(|| row.observed.clone());
    let commit = row.commit.is_some().then(|| identity(row, local)).flatten();
    match (version, commit) {
        (Some(version), Some(commit)) => Some(format!("{version}{commit}")),
        (version, commit) => version.or(commit),
    }
}

/// The engine its native runs reported, else the `iii:` its stack pinned,
/// else the compose row.
fn engine(execution: &Execution, natives: &[&Value]) -> Option<String> {
    natives
        .iter()
        .find_map(|native| native["engine_version"].as_str().filter(|v| !v.is_empty()))
        .map(str::to_owned)
        .or_else(|| {
            let stack = execution.parameters.stack.as_ref()?;
            let yaml = serde_yaml::from_str::<Value>(&stack.yaml).ok()?;
            yaml["iii"].as_str().map(str::to_owned)
        })
        .or_else(|| {
            let row = execution.workers.iter().find(|row| row.name == ENGINE)?;
            row.observed.clone().or_else(|| row.requested.clone())
        })
}

fn source(summary: &Value) -> TrendSource {
    let source =
        serde_json::from_value::<ExecutionSource>(summary["source"].clone()).unwrap_or_default();
    let kind = |kind: &str| TrendSource {
        kind: kind.into(),
        run_id: None,
        run_attempt: None,
        release_control_execution_id: None,
        url: None,
    };
    match source {
        ExecutionSource::Local => kind("local"),
        ExecutionSource::Docker { .. } => kind("docker"),
        ExecutionSource::Github {
            run_id,
            run_attempt,
            url,
            release_control_execution_id,
            ..
        } => TrendSource {
            run_id: Some(run_id),
            run_attempt: Some(run_attempt),
            release_control_execution_id,
            url: Some(url).filter(|url| !url.is_empty()),
            ..kind("github")
        },
    }
}

fn point(
    execution: &Execution,
    stack: TrendStack,
    natives: &[&Value],
    runs: &[&TrendRun],
) -> TrendPoint {
    let planned = &execution.parameters.scenarios;
    let counted = runs
        .iter()
        .filter(|run| run.counted)
        .copied()
        .collect::<Vec<_>>();
    let mut ids = planned.iter().map(String::as_str).collect::<Vec<_>>();
    for run in runs {
        if !ids.contains(&run.test.as_str()) {
            ids.push(&run.test);
        }
    }
    let tests = ids
        .into_iter()
        .map(|id| {
            let ran = runs.iter().filter(|run| run.test == id).collect::<Vec<_>>();
            let valid = ran
                .iter()
                .filter(|run| run.counted)
                .map(|run| **run)
                .collect::<Vec<_>>();
            let score = mean(valid.iter().map(|run| run.score));
            TrendTest {
                id: id.to_owned(),
                state: if ran.is_empty() {
                    "not_run"
                } else if valid.is_empty() {
                    "technical_invalid"
                } else if score.is_some() {
                    "scored"
                } else {
                    "no_score"
                }
                .into(),
                score,
                behavior_sha256: ran.iter().find_map(|run| run.behavior_sha256.clone()),
            }
        })
        .collect();
    let measures = (!counted.is_empty()).then(|| TrendMeasures {
        score_mean: mean(counted.iter().map(|run| run.score)),
        completed: counted.iter().filter(|run| run.completed).count(),
        planned: if planned.is_empty() {
            runs.len()
        } else {
            planned.len() * execution.parameters.runs.max(1) as usize
        },
        duration_ms_mean: mean(counted.iter().map(|run| run.duration_ms)),
        input_tokens_mean: mean(counted.iter().map(|run| run.input_tokens)),
        function_calls_mean: mean(counted.iter().map(|run| run.function_calls)),
        function_calls: sum(counted.iter().map(|run| run.function_calls)),
        function_call_errors: sum(counted.iter().map(|run| run.function_call_errors)),
        turns_mean: mean(counted.iter().map(|run| run.turns)),
    });
    let summary = execution.summary;
    let source = source(summary);
    let local = source.kind == "local";
    TrendPoint {
        execution_id: execution.id.to_owned(),
        label: summary["label"]
            .as_str()
            .filter(|label| !label.is_empty())
            .map(str::to_owned),
        started_at: execution.started_at.to_owned(),
        source,
        stack,
        engine: engine(execution, natives),
        runner: runner(&execution.workers, local),
        workers: workers(&execution.workers, local),
        planned: (!planned.is_empty()).then(|| planned.clone()),
        runs: runs.len(),
        counted: counted.len(),
        reason: counted.is_empty().then(|| {
            summary["first_failure"]["message"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    if runs.is_empty() {
                        "No run was recorded".into()
                    } else {
                        "Every run is technically invalid".into()
                    }
                })
        }),
        measures,
        tests,
    }
}

/// The mean of the values present; none present is none.
fn mean(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    let values = values.flatten().collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn sum(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    values
        .flatten()
        .fold(None, |total, value| Some(total.unwrap_or(0.0) + value))
}

/// RFC 3339 instants compare as instants, whatever their offset.
fn instant(value: &str) -> (Option<chrono::DateTime<chrono::Utc>>, &str) {
    (
        chrono::DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|at| at.with_timezone(&chrono::Utc)),
        value,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plans::store::WorkerSource;
    use serde_json::json;

    /// A packaged worker as a lock that asked for `latest` records it.
    fn worker(name: &str, resolved: Option<&str>, observed: Option<&str>) -> StackWorker {
        StackWorker {
            name: name.into(),
            source: WorkerSource::Package,
            requested: Some("latest".into()),
            resolved: resolved.map(str::to_owned),
            observed: observed.map(str::to_owned),
            commit: None,
            dirty: None,
            groups: Vec::new(),
        }
    }

    /// An execution of the Console as `execution_summaries` lists it.
    fn listed(id: &str, at: &str, stack: Option<&str>, workers: &[&str]) -> Value {
        json!({
            "id": id, "label": "", "started_at": at, "source": {"kind": "local"},
            "stack": workers.iter().map(|name| worker(name, Some("1.0.0"), None)).collect::<Vec<_>>(),
            "parameters": {
                "suite": {"id": "regression", "label": "Regression", "sha256": "sha256:r"},
                "scenarios": [], "runs": 1, "technical_retries": 0,
                "model": "flash", "provider": "deepseek", "agent": null, "where": "github",
                "stack": stack.map(|name| json!({"name": name, "yaml": "iii: 0.24.2\n", "sha256": ""})),
            },
        })
    }

    fn native(id: &str, parent: &str) -> Value {
        json!({"id": id, "parent_plan_execution_id": parent, "engine_version": "0.24.3"})
    }

    fn run(test: &str, counted: bool, completed: bool, score: Option<f64>) -> TrendRun {
        TrendRun {
            test: test.into(),
            behavior_sha256: Some(format!("sha256:{test}")),
            counted,
            completed,
            score,
            duration_ms: Some(1_000.0),
            input_tokens: Some(100.0),
            function_calls: Some(10.0),
            function_call_errors: Some(1.0),
            turns: Some(5.0),
        }
    }

    /// Software engineering on Sep 24: 15 planned, 13 ran (one of them
    /// technically invalid, one valid but incomplete), two never ran.
    fn sep_24(runs_per_test: u32) -> TrendPoint {
        let planned = (1..=15).map(|n| format!("t{n:02}")).collect::<Vec<_>>();
        let mut execution = listed(
            "plan-a",
            "2026-09-24T10:00:00Z",
            Some("default"),
            &["harness"],
        );
        execution["parameters"]["scenarios"] = json!(planned);
        execution["parameters"]["runs"] = json!(runs_per_test);
        let mut runs = planned[..11]
            .iter()
            .map(|test| run(test, true, true, Some(100.0)))
            .collect::<Vec<_>>();
        runs.push(run("t12", true, false, Some(50.0)));
        runs.push(TrendRun {
            input_tokens: Some(1_000_000.0),
            duration_ms: Some(9_000_000.0),
            function_calls: Some(1_000.0),
            turns: Some(1_000.0),
            ..run("t13", false, true, Some(0.0))
        });
        let response = trends(
            &TrendsRequest::default(),
            &[execution, native("n1", "plan-a")],
            &BTreeMap::from([("n1".to_owned(), runs)]),
        );
        response.points.into_iter().next().unwrap()
    }

    #[test]
    fn an_invalid_run_is_left_out_of_every_measure() {
        let point = sep_24(1);
        assert_eq!((point.runs, point.counted, point.reason), (13, 12, None));
        let measures = point.measures.unwrap();
        assert_eq!(measures.score_mean, Some((11.0 * 100.0 + 50.0) / 12.0));
        assert_eq!(measures.duration_ms_mean, Some(1_000.0));
        assert_eq!(measures.input_tokens_mean, Some(100.0));
        assert_eq!(measures.function_calls_mean, Some(10.0));
        assert_eq!(measures.function_calls, Some(120.0));
        assert_eq!(measures.function_call_errors, Some(12.0));
        assert_eq!(measures.turns_mean, Some(5.0));
        let invalid = &point.tests[12];
        assert_eq!(
            (invalid.id.as_str(), invalid.state.as_str(), invalid.score),
            ("t13", "technical_invalid", None)
        );
    }

    #[test]
    fn tests_completed_counts_valid_completed_runs_over_the_planned_tests() {
        let point = sep_24(1);
        let measures = point.measures.unwrap();
        // t12 did not complete and t13 does not count.
        assert_eq!((measures.completed, measures.planned), (11, 15));
        assert_eq!(point.planned.unwrap().len(), 15);
        let states = point
            .tests
            .iter()
            .map(|test| (test.id.as_str(), test.state.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(states.len(), 15);
        assert_eq!(states[11], ("t12", "scored"));
        assert_eq!(&states[13..], [("t14", "not_run"), ("t15", "not_run")]);
        assert_eq!(point.tests[13].behavior_sha256, None);
        // Runs, as `completed` counts them: each planned test twice.
        assert_eq!(sep_24(2).measures.unwrap().planned, 30);
    }

    #[test]
    fn a_worker_is_its_commit_else_its_lock_version_never_what_it_observed() {
        // The binary reported a frozen Cargo version while 1.8.36 ran.
        let harness = worker("harness", Some("1.8.36"), Some("1.8.8-rc.3"));
        assert_eq!(identity(&harness, false).as_deref(), Some("1.8.36"));
        let latest = worker("state", None, Some("0.22.17"));
        assert_eq!(identity(&latest, false), None);
        assert_eq!(identity(&latest, true), None);
        // This harness runs from its compose file: a concrete pin names the
        // release there, and only there.
        let pinned = StackWorker {
            requested: Some("0.22.17".into()),
            ..latest.clone()
        };
        assert_eq!(identity(&pinned, true).as_deref(), Some("0.22.17"));
        assert_eq!(identity(&pinned, false), None);
        let built = StackWorker {
            commit: Some(format!("3f2a9c1{}", "d".repeat(33))),
            dirty: Some(true),
            ..harness.clone()
        };
        assert_eq!(identity(&built, false).as_deref(), Some("@3f2a9c1*"));

        let runner = StackWorker {
            commit: Some("abc1234ef".into()),
            ..worker(RUNNER, Some("0.17.0"), Some("0.17.0"))
        };
        let rows = [
            harness,
            latest,
            runner,
            worker(ENGINE, None, Some("0.24.3")),
        ];
        assert_eq!(
            workers(&rows, false),
            Some(BTreeMap::from([("harness".into(), "1.8.36".into())]))
        );
        assert_eq!(
            super::runner(&rows, false).as_deref(),
            Some("0.17.0@abc1234")
        );
        assert_eq!(workers(&[], false), None);
    }

    #[test]
    fn an_unrecorded_stack_joins_the_stack_whose_first_execution_ran_the_same_workers() {
        let summaries = [
            listed("e1", "2026-09-20T10:00:00Z", Some("default"), &["a", "b"]),
            listed(
                "e2",
                "2026-09-21T10:00:00Z",
                Some("default"),
                &["a", "b", "c"],
            ),
            listed("e3", "2026-09-22T10:00:00Z", None, &["b", "a"]),
            // Only the first execution under a stack names its workers.
            listed("e4", "2026-09-23T10:00:00Z", None, &["a", "b", "c"]),
            listed("e5", "2026-09-24T10:00:00Z", Some("18w"), &["x"]),
        ];
        let runs = BTreeMap::new();
        let latest = trends(&TrendsRequest::default(), &summaries, &runs);
        assert_eq!(latest.stack, "18w");
        assert_eq!(
            latest
                .points
                .iter()
                .map(|p| p.execution_id.as_str())
                .collect::<Vec<_>>(),
            ["e5"]
        );
        let count = |name: &str, executions| StackCount {
            name: name.into(),
            executions,
        };
        assert_eq!(
            latest.stacks,
            [
                count("18w", 1),
                count("default", 3),
                count(NOT_RECORDED, 1),
                count(ANY, 5)
            ]
        );

        let request = |stack: &str| TrendsRequest {
            stack: Some(stack.into()),
            ..TrendsRequest::default()
        };
        let default = trends(&request("default"), &summaries, &runs);
        assert_eq!(
            default
                .points
                .iter()
                .map(|p| (p.execution_id.as_str(), p.stack.matched_by_workers))
                .collect::<Vec<_>>(),
            [("e1", false), ("e2", false), ("e3", true)]
        );
        let unrecorded = trends(&request(NOT_RECORDED), &summaries, &runs);
        assert_eq!(unrecorded.points[0].execution_id, "e4");
        assert_eq!(
            unrecorded.points[0].stack,
            TrendStack {
                name: None,
                matched_by_workers: false
            }
        );
        assert_eq!(trends(&request(ANY), &summaries, &runs).points.len(), 5);
        // A stack the series never ran on: the default applies, and says so.
        let gone = trends(&request("gone"), &summaries, &runs);
        assert_eq!(gone.stack, "18w");
        assert_eq!(gone.points[0].execution_id, "e5");
    }

    #[test]
    fn series_are_suite_provider_model_and_profile() {
        let at = |day: u32| format!("2026-09-{day:02}T10:00:00Z");
        let mut other_sha = listed("b", &at(2), None, &[]);
        other_sha["parameters"]["suite"]["sha256"] = json!("sha256:changed");
        let mut profiled = listed("c", &at(3), None, &[]);
        profiled["parameters"]["agent"] = json!("tech-lead");
        let mut unsaved = listed("d", &at(4), None, &[]);
        unsaved["parameters"]["suite"] = json!({"label": "", "sha256": "sha256:unsaved"});
        let mut no_suite = listed("e", &at(5), None, &[]);
        no_suite["parameters"]
            .as_object_mut()
            .unwrap()
            .remove("suite");
        let mut no_model = listed("f", &at(6), None, &[]);
        no_model["parameters"]["model"] = json!("");
        let mut child = listed("g", &at(7), None, &[]);
        child["parent_plan_execution_id"] = json!("a");
        let summaries = [
            listed("a", &at(1), None, &[]),
            other_sha,
            profiled,
            unsaved,
            no_suite,
            no_model,
            child,
        ];
        let runs = BTreeMap::new();
        let response = trends(&TrendsRequest::default(), &summaries, &runs);
        let key = |suite: &str, profile: Option<&str>| TrendSeriesKey {
            suite: suite.into(),
            provider: "deepseek".into(),
            model: "flash".into(),
            profile: profile.map(str::to_owned),
        };
        assert_eq!(
            response
                .series
                .iter()
                .map(|series| (series.key.clone(), series.executions))
                .collect::<Vec<_>>(),
            [
                (key("sha256:unsaved", None), 1),
                (key("regression", Some("tech-lead")), 1),
                (key("regression", None), 2),
            ]
        );
        assert_eq!(response.series[2].suite_label, "Regression");
        assert_eq!(response.series[2].latest_at, at(2));
        assert_eq!(response.selected, Some(key("sha256:unsaved", None)));

        let asked = |request: Value| {
            let request = serde_json::from_value::<TrendsRequest>(request).unwrap();
            trends(&request, &summaries, &runs).selected
        };
        assert_eq!(
            asked(json!({"suite": "regression", "profile": null})),
            Some(key("regression", None))
        );
        assert_eq!(
            asked(json!({"suite": "regression", "profile": ""})),
            Some(key("regression", None))
        );
        // No profile sent: the suite's latest series, whatever its profile.
        assert_eq!(
            asked(json!({"suite": "regression"})),
            Some(key("regression", Some("tech-lead")))
        );
    }

    #[test]
    fn an_execution_with_no_counted_run_has_no_measures_and_says_why() {
        let mut failed = listed("failed", "2026-09-22T10:00:00Z", Some("default"), &[]);
        failed["parameters"]["scenarios"] = json!(["t01"]);
        failed["first_failure"] =
            json!({"kind": "execution", "message": "No group of this run left a native run"});
        let invalid = listed("invalid", "2026-09-23T10:00:00Z", Some("default"), &[]);
        let summaries = [failed, invalid, native("n1", "invalid")];
        let runs = BTreeMap::from([("n1".to_owned(), vec![run("t01", false, false, None)])]);
        let points = trends(&TrendsRequest::default(), &summaries, &runs).points;
        assert_eq!(
            points
                .iter()
                .map(|p| (p.runs, p.counted, p.measures.is_none(), p.reason.as_deref()))
                .collect::<Vec<_>>(),
            [
                (0, 0, true, Some("No group of this run left a native run")),
                (1, 0, true, Some("Every run is technically invalid")),
            ]
        );
        assert_eq!(points[0].tests[0].state, "not_run");
        // Without a native report the engine is the one its stack pinned.
        assert_eq!(points[0].engine.as_deref(), Some("0.24.2"));
        assert_eq!(points[1].engine.as_deref(), Some("0.24.3"));
    }
}
