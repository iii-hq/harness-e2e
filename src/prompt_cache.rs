//! Prompt-cache prefix stability. A prefix-matching provider cache (DeepSeek)
//! only pays when every request of a session starts with what the previous
//! one sent, so the Harness splits a stable prompt head off (prompt sections)
//! and names each already-sent row a request rewrote (prefix divergences).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::harness_invariants::{HarnessInvariant, HarnessViolation};
use crate::report::{CompletionState, E2eRunReport, EvaluatorAvailability, RunStatus};
use crate::wire::{SessionMetricsPayload, SessionUsageTotals};

/// Executions of the same setup a run's cache-hit ratio is held against.
pub const BASELINE_EXECUTIONS: usize = 7;
/// Fewer earlier executions than this hold the run to no baseline.
const MIN_BASELINE_EXECUTIONS: usize = 3;
// shortcut: reads at most this many of the setup's latest executions, so a
// scenario run rarely can stay without a baseline; index the history if so.
const MAX_SCANNED_EXECUTIONS: usize = 100;

/// A session whose requests carried no shared prompt prefix
/// (`prompt_sections_fallback`: `disabled`, `no_stable_prefix` or
/// `prefix_rewritten`) pays the whole prompt on every step.
pub(crate) fn violations(report: &E2eRunReport) -> Vec<HarnessViolation> {
    let Some(metrics) = report.metrics.as_ref() else {
        return Vec::new();
    };
    metrics
        .by_session
        .iter()
        .filter_map(|session| {
            let reason = session
                .context
                .as_ref()?
                .get("prompt_sections_fallback")?
                .as_str()?;
            Some(HarnessViolation {
                invariant: HarnessInvariant::PromptSectionsFallback,
                detail: format!(
                    "session {} sent no shared prompt prefix: {reason}",
                    session.session_id
                ),
            })
        })
        .collect()
}

/// The prefix divergences no compaction explains, over every session's
/// snapshot: a rewrite caused by `prune` or `system_prompt` on a step that
/// wrote a `compaction` record is the context-manager making room. `None`
/// when no session has a snapshot. A trend, not a gate yet: the Harness
/// tracks sent rows in memory, so a restart forgets them, and a divergence
/// without a cause can come from a provider wire change alone.
// shortcut: compaction steps come from the root transcript only, so a child
// session's compaction rewrite counts; read child transcripts once they are
// captured.
pub(crate) fn unexpected_prefix_divergences(
    metrics: &SessionMetricsPayload,
    transcript: Option<&Value>,
) -> Option<u64> {
    let compactions = messages(transcript)
        .iter()
        .filter(|entry| {
            entry.pointer("/custom/custom_type").and_then(Value::as_str) == Some("compaction")
        })
        .filter_map(|entry| entry.get("entry_id").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    let contexts = metrics
        .by_session
        .iter()
        .filter_map(|session| session.context.as_ref())
        .collect::<Vec<_>>();
    if contexts.is_empty() {
        return None;
    }
    let explained = |divergence: &Value| {
        let cause = divergence.get("cause").and_then(Value::as_str);
        let turn = divergence.get("turn_id").and_then(Value::as_str);
        let step = divergence.get("step").and_then(Value::as_u64);
        matches!(cause, Some("prune" | "system_prompt"))
            && turn.zip(step).is_some_and(|(turn, step)| {
                compactions.contains(format!("e_{turn}_{step}_compaction").as_str())
            })
    };
    Some(
        contexts
            .iter()
            .filter_map(|context| context.get("prefix_divergences")?.as_array())
            .flatten()
            .filter(|divergence| !explained(divergence))
            .count() as u64,
    )
}

/// The share of the run's billed prompt the provider served from its cache,
/// only when the root session sent more than one request: the first request
/// of a session has nothing to reuse.
pub fn cache_hit_ratio(totals: &SessionUsageTotals, transcript: Option<&Value>) -> Option<f64> {
    let steps = messages(transcript)
        .iter()
        .filter(|entry| entry.pointer("/message/role").and_then(Value::as_str) == Some("assistant"))
        .count();
    let read = totals.cache_read_tokens?;
    let prompt = totals
        .input_tokens?
        .saturating_add(read)
        .saturating_add(totals.cache_write_tokens.unwrap_or(0));
    (steps > 1 && prompt > 0).then(|| read as f64 / prompt as f64)
}

/// What makes two executions' cache-hit ratios comparable.
pub struct Setup<'a> {
    pub lane: &'a str,
    pub model: &'a str,
    pub provider: &'a str,
    pub agent: Option<&'a str>,
}

/// The median cache-hit ratio of each scenario over the latest
/// [`BASELINE_EXECUTIONS`] executions of the setup in `history` that completed
/// before `before` (RFC 3339), one ratio per execution: the mean over its
/// passed multi-step runs. A scenario with fewer than three such executions
/// has none. An unreadable execution is skipped.
pub fn baselines(
    history: &Path,
    setup: &Setup<'_>,
    before: &str,
    scenarios: &[&str],
) -> BTreeMap<String, f64> {
    let mut candidates = std::fs::read_dir(history)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let directory = report_directory(&entry.ok()?.path())?;
            let manifest: HistoryManifest =
                serde_json::from_slice(&std::fs::read(directory.join("manifest.json")).ok()?)
                    .ok()?;
            let completed = manifest.execution.completed_at?;
            (manifest.execution.lane == setup.lane
                && manifest.subject.model == setup.model
                && manifest.subject.provider == setup.provider
                && manifest.subject.agent.map(|agent| agent.id).as_deref() == setup.agent
                && completed.as_str() < before)
                .then_some((completed, directory))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.0.cmp(&left.0));
    let mut samples = scenarios
        .iter()
        .map(|scenario| (scenario.to_string(), Vec::new()))
        .collect::<BTreeMap<_, Vec<f64>>>();
    for (_, directory) in candidates.into_iter().take(MAX_SCANNED_EXECUTIONS) {
        if samples
            .values()
            .all(|ratios| ratios.len() >= BASELINE_EXECUTIONS)
        {
            break;
        }
        let Some(report) = std::fs::read(directory.join("results.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<HistoryReport>(&bytes).ok())
        else {
            continue;
        };
        for scenario in report.scenarios {
            let Some(ratios) = samples
                .get_mut(&scenario.scenario_id)
                .filter(|ratios| ratios.len() < BASELINE_EXECUTIONS)
            else {
                continue;
            };
            let runs = scenario
                .runs
                .iter()
                .filter(|run| run.status == RunStatus::Passed)
                .filter_map(|run| {
                    cache_hit_ratio(&run.metrics.as_ref()?.totals, run.transcript.as_ref())
                })
                .collect::<Vec<_>>();
            if !runs.is_empty() {
                ratios.push(runs.iter().sum::<f64>() / runs.len() as f64);
            }
        }
    }
    samples
        .into_iter()
        .filter(|(_, ratios)| ratios.len() >= MIN_BASELINE_EXECUTIONS)
        .map(|(scenario, mut ratios)| {
            ratios.sort_by(f64::total_cmp);
            let middle = ratios.len() / 2;
            let median = if ratios.len() % 2 == 0 {
                (ratios[middle - 1] + ratios[middle]) / 2.0
            } else {
                ratios[middle]
            };
            (scenario, median)
        })
        .collect()
}

/// A passed multi-step run whose cache-hit ratio fell below half its
/// scenario's baseline lost the prompt cache: incomplete, like a broken
/// Harness invariant, and the score stays. True when the run changed.
pub fn hold_to_baseline(run: &mut E2eRunReport, baseline: Option<f64>) -> bool {
    let Some(baseline) = baseline else {
        return false;
    };
    if run.status != RunStatus::Passed {
        return false;
    }
    let Some(ratio) = run
        .metrics
        .as_ref()
        .and_then(|metrics| cache_hit_ratio(&metrics.totals, run.transcript.as_ref()))
    else {
        return false;
    };
    if ratio >= baseline / 2.0 {
        return false;
    }
    run.harness_violations.push(HarnessViolation {
        invariant: HarnessInvariant::PromptCacheRegressed,
        detail: format!(
            "the provider cache served {:.1}% of the prompt, under half the {:.1}% median of \
             this setup's latest executions",
            ratio * 100.0,
            baseline * 100.0
        ),
    });
    run.set_completion(
        CompletionState::TaskIncomplete,
        EvaluatorAvailability::Available,
    );
    true
}

fn messages(transcript: Option<&Value>) -> &[Value] {
    transcript
        .and_then(|transcript| transcript.get("messages"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

/// Where an execution's results sit: `<id>/results` from `run --runs-dir`,
/// `<id>` itself from the control plane.
fn report_directory(execution: &Path) -> Option<PathBuf> {
    [execution.join("results"), execution.to_path_buf()]
        .into_iter()
        .find(|directory| directory.join("results.json").is_file())
}

#[derive(Deserialize)]
struct HistoryManifest {
    execution: HistoryExecution,
    subject: HistorySubject,
}

#[derive(Deserialize)]
struct HistoryExecution {
    lane: String,
    completed_at: Option<String>,
}

#[derive(Deserialize)]
struct HistorySubject {
    model: String,
    provider: String,
    agent: Option<HistoryAgent>,
}

#[derive(Deserialize)]
struct HistoryAgent {
    id: String,
}

#[derive(Deserialize)]
struct HistoryReport {
    scenarios: Vec<HistoryScenario>,
}

#[derive(Deserialize)]
struct HistoryScenario {
    scenario_id: String,
    #[serde(default)]
    runs: Vec<HistoryRun>,
}

#[derive(Deserialize)]
struct HistoryRun {
    status: RunStatus,
    metrics: Option<HistoryMetrics>,
    transcript: Option<Value>,
}

#[derive(Deserialize)]
struct HistoryMetrics {
    totals: SessionUsageTotals,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::wire::{Observed, SessionUsage};

    fn session(id: &str, context: Option<Value>) -> SessionUsage {
        SessionUsage {
            session_id: id.into(),
            parent_session_id: None,
            depth: 0,
            turns: 1,
            function_calls: 0,
            function_call_errors: 0,
            validation_retries: None,
            transient_resumes: None,
            wake_resumes: None,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            cost_usd: None,
            context,
        }
    }

    fn payload(totals: SessionUsageTotals, contexts: Vec<Option<Value>>) -> SessionMetricsPayload {
        SessionMetricsPayload {
            root_session_id: "root".into(),
            complete: true,
            totals,
            by_session: contexts
                .into_iter()
                .enumerate()
                .map(|(index, context)| session(&format!("s{index}"), context))
                .collect(),
            traces: None,
        }
    }

    fn totals(input: u64, cache_read: u64) -> SessionUsageTotals {
        SessionUsageTotals {
            input_tokens: Some(input),
            cache_read_tokens: Some(cache_read),
            ..SessionUsageTotals::default()
        }
    }

    fn assistant() -> Value {
        json!({ "message": { "role": "assistant", "content": [] } })
    }

    fn run(totals: SessionUsageTotals, steps: usize) -> E2eRunReport {
        let mut run = E2eRunReport::new("run".into(), "a".into(), 1, "root".into(), "p".into());
        run.status = RunStatus::Passed;
        run.completion = CompletionState::Completed;
        run.transcript = Some(json!({ "messages": vec![assistant(); steps] }));
        run.metrics = Some(Observed::from_normalized(payload(totals, Vec::new())));
        run
    }

    #[test]
    fn a_session_without_a_shared_prompt_prefix_is_flagged() {
        let mut report = run(totals(1, 1), 2);
        report.metrics = Some(Observed::from_normalized(payload(
            SessionUsageTotals::default(),
            vec![
                Some(json!({ "prompt_surface_digest": "sha256:a" })),
                Some(json!({ "prompt_sections_fallback": "prefix_rewritten" })),
                None,
            ],
        )));

        let violations = violations(&report);

        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[0].invariant,
            HarnessInvariant::PromptSectionsFallback
        );
        assert!(violations[0].detail.contains("session s1"));
        assert!(violations[0].detail.contains("prefix_rewritten"));
    }

    #[test]
    fn only_a_compaction_explains_a_prefix_divergence() {
        let divergence = |cause: Option<&str>, step: u64| json!({ "turn_id": "t_1", "step": step, "index": 0, "cause": cause });
        let metrics = payload(
            SessionUsageTotals::default(),
            vec![
                Some(json!({ "prefix_divergences": [
                    divergence(Some("system_prompt"), 5),
                    divergence(Some("prune"), 5),
                    // A cause on a step that did not compact, and none at all.
                    divergence(Some("system_prompt"), 6),
                    divergence(None, 5),
                ] })),
                Some(json!({ "prefix_divergences": [divergence(None, 2)] })),
                None,
            ],
        );
        let transcript = json!({ "messages": [
            { "entry_id": "e_t_1_5_compaction", "custom": { "custom_type": "compaction" } },
            { "entry_id": "e_t_1_6_notice_0", "custom": { "custom_type": "model_notice" } },
        ] });

        assert_eq!(
            unexpected_prefix_divergences(&metrics, Some(&transcript)),
            Some(3)
        );
        let without_snapshots = payload(SessionUsageTotals::default(), vec![None]);
        assert_eq!(
            unexpected_prefix_divergences(&without_snapshots, Some(&transcript)),
            None
        );
    }

    #[test]
    fn the_cache_hit_ratio_is_the_cached_share_of_a_multi_step_prompt() {
        let transcript = json!({ "messages": [assistant(), assistant()] });
        let mut written = totals(100, 300);
        written.cache_write_tokens = Some(100);

        assert_eq!(cache_hit_ratio(&written, Some(&transcript)), Some(0.6));
        let one_step = json!({ "messages": [assistant()] });
        assert_eq!(cache_hit_ratio(&written, Some(&one_step)), None);
        let unreported = SessionUsageTotals {
            input_tokens: Some(100),
            ..SessionUsageTotals::default()
        };
        assert_eq!(cache_hit_ratio(&unreported, Some(&transcript)), None);
    }

    fn store(
        history: &Path,
        id: &str,
        lane: &str,
        completed_at: &str,
        runs: &[(&str, &str, u64, u64)],
    ) {
        let directory = history.join(id);
        std::fs::create_dir_all(&directory).unwrap();
        let manifest = json!({
            "execution": { "execution_id": id, "lane": lane, "completed_at": completed_at },
            "subject": { "model": "deepseek-flash", "provider": "deepseek" },
        });
        let mut scenarios = BTreeMap::<&str, Vec<Value>>::new();
        for (scenario, status, input, cache_read) in runs {
            scenarios.entry(scenario).or_default().push(json!({
                "status": status,
                "metrics": { "totals": {
                    "sessions": 1, "turns": 1, "function_calls": 0, "function_call_errors": 0,
                    "input_tokens": input, "cache_read_tokens": cache_read,
                } },
                "transcript": { "messages": [assistant(), assistant()] },
            }));
        }
        let results = json!({ "scenarios": scenarios
            .into_iter()
            .map(|(id, runs)| json!({ "scenario_id": id, "runs": runs }))
            .collect::<Vec<_>>() });
        std::fs::write(directory.join("manifest.json"), manifest.to_string()).unwrap();
        std::fs::write(directory.join("results.json"), results.to_string()).unwrap();
    }

    fn setup() -> Setup<'static> {
        Setup {
            lane: "local",
            model: "deepseek-flash",
            provider: "deepseek",
            agent: None,
        }
    }

    #[test]
    fn the_baseline_is_the_median_of_the_setups_latest_seven_executions() {
        let history = tempfile::tempdir().unwrap();
        // Ratios 0.1 .. 0.9 on days 1 .. 9; the latest seven are 0.3 .. 0.9.
        for day in 1..=9u64 {
            store(
                history.path(),
                &format!("e{day}"),
                "local",
                &format!("2026-10-0{day}T00:00:00.000Z"),
                &[("cached", "passed", 100 - 10 * day, 10 * day)],
            );
        }
        // Another lane, a later execution and a failed run never count.
        store(
            history.path(),
            "lane",
            "other",
            "2026-10-09T01:00:00.000Z",
            &[("cached", "passed", 100, 0)],
        );
        store(
            history.path(),
            "later",
            "local",
            "2026-10-11T00:00:00.000Z",
            &[("cached", "passed", 100, 0)],
        );
        store(
            history.path(),
            "failed",
            "local",
            "2026-10-09T02:00:00.000Z",
            &[("cached", "hard_gate_failed", 100, 0)],
        );
        // Two executions are not a baseline.
        for day in 1..=2u64 {
            store(
                history.path(),
                &format!("rare{day}"),
                "local",
                &format!("2026-10-0{day}T05:00:00.000Z"),
                &[("rare", "passed", 50, 50)],
            );
        }

        let baselines = baselines(
            history.path(),
            &setup(),
            "2026-10-10T00:00:00.000Z",
            &["cached", "rare"],
        );

        assert_eq!(baselines.len(), 1);
        assert!((baselines["cached"] - 0.6).abs() < 1e-9);
    }

    #[test]
    fn a_passed_run_under_half_its_baseline_is_incomplete_with_its_score() {
        let mut regressed = run(totals(80, 20), 3);
        regressed.score = Some(100);

        assert!(hold_to_baseline(&mut regressed, Some(0.5)));
        assert_eq!(regressed.completion, CompletionState::TaskIncomplete);
        assert_eq!(regressed.status, RunStatus::Passed);
        assert_eq!(regressed.score, Some(100));
        assert_eq!(
            regressed.harness_violations[0].invariant,
            HarnessInvariant::PromptCacheRegressed
        );
        assert!(regressed.harness_violations[0].detail.contains("20.0%"));

        let mut held = run(totals(75, 25), 3);
        assert!(!hold_to_baseline(&mut held, Some(0.5)));
        assert!(!hold_to_baseline(&mut held, None));
        let mut one_step = run(totals(100, 0), 1);
        assert!(!hold_to_baseline(&mut one_step, Some(0.5)));
        let mut failed = run(totals(100, 0), 3);
        failed.status = RunStatus::HardGateFailed;
        assert!(!hold_to_baseline(&mut failed, Some(0.5)));
        assert_eq!(held.completion, CompletionState::Completed);
        assert!(held.harness_violations.is_empty());
    }
}
