//! What a test's history shows of each run beyond its retained metrics: the
//! session tree the run spawned, the root session's calls by worker, the
//! token breakdown and how each criterion scored. All of it is read from the
//! run's native results, and only for the runs of the page the history
//! returns.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::Value;

use crate::report::{CompletionState, RunStatus};

/// One run of a history observation. Its status and totals come from the
/// retained projection; `details` from the run's native results.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct HistoryRun {
    pub run_id: String,
    pub attempt_id: String,
    pub status: RunStatus,
    pub completion: CompletionState,
    pub score: Option<f64>,
    pub duration_seconds: Option<f64>,
    pub turns: Option<f64>,
    pub function_calls: Option<f64>,
    pub function_call_errors: Option<f64>,
    /// Absent when the run's native results could not be read.
    pub details: Option<RunDetails>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema)]
pub(super) struct RunDetails {
    /// The root session first, each session followed by the ones it spawned.
    pub sessions: Vec<HistorySession>,
    pub child_sessions: usize,
    /// Calls of the root session (the only transcript a run keeps), by the
    /// worker that serves them, most called first.
    pub calls_by_worker: Vec<WorkerCalls>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    /// Every criterion the run was scored on, in contract order.
    pub criteria: Vec<RunCriterion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct HistorySession {
    pub session_id: String,
    pub parent_session_id: Option<String>,
    pub depth: u32,
    /// The agent profile the session was spawned with, when the root's
    /// transcript names it.
    pub label: Option<String>,
    pub turns: u64,
    pub function_calls: u64,
    pub function_call_errors: u64,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct WorkerCalls {
    pub worker: String,
    pub calls: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(super) struct RunCriterion {
    pub id: String,
    pub possible: f64,
    /// Absent when nobody evaluated the criterion.
    pub awarded: Option<f64>,
    pub reason: String,
}

// Only the parts of a native results.json the history reads. Runs stay raw
// until their scenario and case match, and each decodes on its own: a
// malformed run leaves the others, and its execution, readable. Every field
// has a default, so an older or partial report still yields what it has.
#[derive(Deserialize)]
struct NativeReport<'a> {
    #[serde(borrow, default)]
    scenarios: Vec<NativeScenario<'a>>,
}

#[derive(Deserialize)]
struct NativeScenario<'a> {
    #[serde(default)]
    scenario_id: String,
    #[serde(default)]
    case_id: String,
    #[serde(borrow, default)]
    runs: Vec<&'a RawValue>,
}

#[derive(Deserialize)]
struct NativeRun<'a> {
    #[serde(default)]
    run_id: String,
    #[serde(default)]
    attempt_id: String,
    #[serde(default)]
    criteria: Vec<NativeCriterion>,
    /// The root session's transcript, decoded only for a run the page shows.
    #[serde(borrow, default)]
    transcript: Option<&'a RawValue>,
    #[serde(default)]
    metrics: Option<NativeMetrics>,
}

#[derive(Deserialize)]
struct NativeCriterion {
    #[serde(default)]
    id: String,
    #[serde(default)]
    possible: f64,
    #[serde(default)]
    awarded: Option<f64>,
    #[serde(default)]
    reason: String,
}

#[derive(Default, Deserialize)]
struct NativeMetrics {
    #[serde(default)]
    root_session_id: String,
    #[serde(default)]
    by_session: Vec<NativeSession>,
    #[serde(default)]
    totals: Option<NativeTotals>,
    #[serde(default)]
    traces: Option<NativeTraces>,
}

#[derive(Deserialize)]
struct NativeSession {
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    parent_session_id: Option<String>,
    #[serde(default)]
    depth: u32,
    #[serde(default)]
    turns: u64,
    #[serde(default)]
    function_calls: u64,
    #[serde(default)]
    function_call_errors: u64,
}

#[derive(Deserialize)]
struct NativeTotals {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_read_tokens: Option<u64>,
    #[serde(default)]
    cache_write_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct NativeTraces {
    #[serde(default)]
    by_session: Vec<NativeTrace>,
}

#[derive(Deserialize)]
struct NativeTrace {
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    duration_ms: Option<u64>,
}

/// The results file an execution's result path points at: the file itself,
/// or the `results.json` in it or in its `results` directory.
pub(super) fn results_file(path: PathBuf) -> PathBuf {
    if !path.is_dir() {
        return path;
    }
    let nested = path.join("results").join("results.json");
    if nested.is_file() {
        nested
    } else {
        path.join("results.json")
    }
}

/// `(case_id, run_id, attempt_id)`: where a run's details belong.
pub(super) type RunKey = (String, String, String);

/// The details of the runs of one test's cases in a native results file,
/// read once for all of them. A run that does not decode is left out with a
/// warning; the others are kept.
pub(super) fn read_run_details(
    path: &Path,
    scenario_id: &str,
    cases: &BTreeSet<&str>,
) -> Result<BTreeMap<RunKey, RunDetails>> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let report: NativeReport = serde_json::from_slice(&bytes)
        .with_context(|| format!("decode the runs of {}", path.display()))?;
    let mut found = BTreeMap::new();
    for scenario in report.scenarios {
        if scenario.scenario_id != scenario_id || !cases.contains(scenario.case_id.as_str()) {
            continue;
        }
        for (index, raw) in scenario.runs.into_iter().enumerate() {
            match decode_run(raw) {
                Ok((run, transcript)) => {
                    found.insert(
                        (
                            scenario.case_id.clone(),
                            run.run_id.clone(),
                            run.attempt_id.clone(),
                        ),
                        details(run, transcript.as_ref()),
                    );
                }
                Err(error) => tracing::warn!(
                    path = %path.display(),
                    scenario_id,
                    case_id = %scenario.case_id,
                    run = index,
                    %error,
                    "a run's native results do not decode; its history row shows no sessions"
                ),
            }
        }
    }
    Ok(found)
}

fn decode_run(raw: &RawValue) -> serde_json::Result<(NativeRun<'_>, Option<Value>)> {
    let run: NativeRun = serde_json::from_str(raw.get())?;
    let transcript = run
        .transcript
        .map(|transcript| serde_json::from_str(transcript.get()))
        .transpose()?;
    Ok((run, transcript))
}

fn details(run: NativeRun, transcript: Option<&Value>) -> RunDetails {
    let metrics = run.metrics.unwrap_or_default();
    let calls = transcript
        .map(crate::scenarios::common::function_calls)
        .unwrap_or_default();
    // A child's profile, as the root named it when it spawned the child.
    let labels = calls
        .iter()
        .filter(|call| call.function_id == "harness::spawn")
        .filter_map(|call| {
            Some((
                call.arguments.get("session_id")?.as_str()?.to_string(),
                call.arguments.get("agent")?.as_str()?.to_string(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let durations = metrics
        .traces
        .iter()
        .flat_map(|traces| &traces.by_session)
        .filter_map(|trace| Some((trace.session_id.clone(), trace.duration_ms?)))
        .collect::<BTreeMap<_, _>>();
    let mut by_worker = BTreeMap::<String, u64>::new();
    for call in &calls {
        *by_worker
            .entry(worker_name(&call.function_id, &metrics.root_session_id))
            .or_default() += 1;
    }
    let mut calls_by_worker = by_worker
        .into_iter()
        .map(|(worker, calls)| WorkerCalls { worker, calls })
        .collect::<Vec<_>>();
    calls_by_worker.sort_by_key(|worker| std::cmp::Reverse(worker.calls));
    let sessions = in_tree_order(
        metrics
            .by_session
            .iter()
            .map(|session| HistorySession {
                session_id: session.session_id.clone(),
                parent_session_id: session.parent_session_id.clone(),
                depth: session.depth,
                label: labels.get(&session.session_id).cloned(),
                turns: session.turns,
                function_calls: session.function_calls,
                function_call_errors: session.function_call_errors,
                duration_ms: durations.get(&session.session_id).copied(),
            })
            .collect(),
    );
    let totals = metrics.totals.as_ref();
    RunDetails {
        child_sessions: sessions.iter().filter(|session| session.depth > 0).count(),
        sessions,
        calls_by_worker,
        input_tokens: totals.and_then(|totals| totals.input_tokens),
        output_tokens: totals.and_then(|totals| totals.output_tokens),
        cache_read_tokens: totals.and_then(|totals| totals.cache_read_tokens),
        cache_write_tokens: totals.and_then(|totals| totals.cache_write_tokens),
        criteria: run
            .criteria
            .into_iter()
            .map(|criterion| RunCriterion {
                id: criterion.id,
                possible: criterion.possible,
                awarded: criterion.awarded,
                reason: criterion.reason,
            })
            .collect(),
    }
}

/// The worker a function belongs to. A run-scoped worker is named after the
/// run's root session (`form_flow_1b116e699dc24c` for `e2e_1b116e69…`); it
/// keeps its plain name so the calls of two runs pair up.
fn worker_name(function_id: &str, root_session_id: &str) -> String {
    let worker = function_id.split("::").next().unwrap_or(function_id);
    let run = root_session_id.rsplit('_').next().unwrap_or_default();
    match worker.rsplit_once('_') {
        Some((name, suffix)) if suffix.len() >= 8 && run.starts_with(suffix) => name.to_string(),
        _ => worker.to_string(),
    }
}

/// Each session followed by the sessions it spawned, in reported order. A
/// session whose parent was not reported starts a tree of its own.
fn in_tree_order(sessions: Vec<HistorySession>) -> Vec<HistorySession> {
    let known = sessions
        .iter()
        .map(|session| session.session_id.clone())
        .collect::<BTreeSet<_>>();
    let (roots, mut rest): (Vec<_>, Vec<_>) = sessions.into_iter().partition(|session| {
        session
            .parent_session_id
            .as_ref()
            .is_none_or(|parent| !known.contains(parent))
    });
    let mut stack = roots.into_iter().rev().collect::<Vec<_>>();
    let mut ordered = Vec::with_capacity(known.len());
    while let Some(session) = stack.pop() {
        let (children, others): (Vec<_>, Vec<_>) = rest.into_iter().partition(|child| {
            child.parent_session_id.as_deref() == Some(session.session_id.as_str())
        });
        rest = others;
        stack.extend(children.into_iter().rev());
        ordered.push(session);
    }
    // Only a cycle leaves sessions behind; they are still shown.
    ordered.extend(rest);
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_malformed_run_leaves_the_other_runs_of_its_file_readable() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("results.json");
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "execution": {"execution_id": "e1"},
                "scenarios": [
                    {"scenario_id": "other", "case_id": "c1", "runs": [
                        {"run_id": "x", "attempt_id": "x1",
                         "transcript": {"messages": "never decoded"}}
                    ]},
                    {"scenario_id": "direct_answer", "case_id": "c1", "runs": [
                        {"run_id": "bad", "attempt_id": "b1",
                         "metrics": {"by_session": "not a list"}},
                        // A partial report: no attempt, no depth, no totals,
                        // a criterion without points.
                        {"run_id": "good",
                         "criteria": [{"id": "answer", "possible": 7.5, "reason": "partly"}],
                         "metrics": {"root_session_id": "e2e_ab", "by_session": [
                             {"session_id": "e2e_ab", "turns": 3}
                         ]}}
                    ]},
                    {"scenario_id": "direct_answer", "case_id": "c2", "runs": [
                        {"run_id": "other-case", "attempt_id": "o1"}
                    ]}
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let details = read_run_details(&path, "direct_answer", &BTreeSet::from(["c1"])).unwrap();
        assert_eq!(
            details.keys().cloned().collect::<Vec<_>>(),
            [("c1".to_string(), "good".to_string(), String::new())]
        );
        let good = details.values().next().unwrap();
        assert_eq!(good.sessions[0].turns, 3);
        assert_eq!(good.sessions[0].depth, 0);
        assert_eq!(good.criteria[0].possible, 7.5);
        assert_eq!(good.criteria[0].awarded, None);

        // A file that is missing or does not parse is an error the caller
        // warns about; it never panics.
        assert!(read_run_details(
            &root.path().join("gone.json"),
            "direct_answer",
            &BTreeSet::new()
        )
        .is_err());
        fs::write(&path, b"{\"scenarios\": [").unwrap();
        assert!(read_run_details(&path, "direct_answer", &BTreeSet::from(["c1"])).is_err());
    }

    #[test]
    fn run_scoped_workers_keep_their_plain_name() {
        let root = "e2e_1b116e699dc24ca9b0d8dde8e4dbc794";
        assert_eq!(
            worker_name("form_flow_1b116e699dc24c::preview", root),
            "form_flow"
        );
        assert_eq!(worker_name("coder::read-file", root), "coder");
        assert_eq!(worker_name("state_v2::get", root), "state_v2");
        assert_eq!(
            worker_name("form_flow_7e450dca99::get", root),
            "form_flow_7e450dca99"
        );
    }

    #[test]
    fn sessions_follow_their_parent_even_when_reported_later() {
        let session = |id: &str, parent: Option<&str>, depth| HistorySession {
            session_id: id.into(),
            parent_session_id: parent.map(Into::into),
            depth,
            label: None,
            turns: 0,
            function_calls: 0,
            function_call_errors: 0,
            duration_ms: None,
        };
        let ordered = in_tree_order(vec![
            session("backend", Some("lead"), 2),
            session("root", None, 0),
            session("reviewer", Some("root"), 1),
            session("lead", Some("root"), 1),
            session("orphan", Some("gone"), 3),
        ]);
        assert_eq!(
            ordered
                .iter()
                .map(|session| session.session_id.as_str())
                .collect::<Vec<_>>(),
            ["root", "reviewer", "lead", "backend", "orphan"]
        );
    }
}
