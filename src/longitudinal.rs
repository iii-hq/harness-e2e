use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[cfg(test)]
use crate::report::TechnicalState;
use crate::report::{CompletionState, E2eRunReport};

const MINIMUM_TAIL_SAMPLE: usize = 20;

/// Subject-side usage across all observed attempts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConsumptionMetrics {
    pub observed_runs: u32,
    pub completed_runs: u32,
    pub total_tokens_consumed: Option<u64>,
    pub tokens_per_completion: Option<f64>,
    pub p50_total_tokens: Option<f64>,
    pub p95_total_tokens: Option<f64>,
    pub p50_function_calls: Option<f64>,
    pub p95_function_calls: Option<f64>,
    pub unavailable: BTreeMap<String, String>,
}

fn complete_efficiency(run: &E2eRunReport) -> Option<&crate::report::EfficiencyReport> {
    let efficiency = run.efficiency.as_ref()?;
    (!efficiency.unavailable.contains_key("retry_efficiency")
        && efficiency.technical_attempts as usize == run.retry_attempts.len() + 1)
        .then_some(efficiency)
}

pub fn consumption_metrics(runs: &[E2eRunReport]) -> ConsumptionMetrics {
    let mut result = ConsumptionMetrics {
        observed_runs: runs.len().try_into().unwrap_or(u32::MAX),
        completed_runs: runs
            .iter()
            .filter(|r| r.completion == CompletionState::Completed)
            .count()
            .try_into()
            .unwrap_or(u32::MAX),
        ..ConsumptionMetrics::default()
    };
    let identities: BTreeSet<_> = runs.iter().map(|r| (&r.run_id, &r.attempt_id)).collect();
    if identities.len() != runs.len() || runs.is_empty() {
        result.unavailable.insert(
            "consumption".into(),
            "no observations or duplicate run/attempt identities".into(),
        );
        return result;
    }
    let tokens = runs
        .iter()
        .map(|run| complete_efficiency(run)?.total_tokens)
        .collect::<Option<Vec<_>>>();
    let calls = runs
        .iter()
        .map(|run| Some(complete_efficiency(run)?.function_calls? as f64))
        .collect::<Option<Vec<_>>>();
    result.total_tokens_consumed = tokens
        .as_ref()
        .and_then(|values| values.iter().try_fold(0_u64, |sum, n| sum.checked_add(*n)));
    let token_values =
        tokens.map(|values| values.into_iter().map(|n| n as f64).collect::<Vec<_>>());
    result.p50_total_tokens = token_values.as_deref().and_then(median);
    result.p95_total_tokens =
        tail_metric(&token_values, "p95_total_tokens", &mut result.unavailable);
    result.p50_function_calls = calls.as_deref().and_then(median);
    result.p95_function_calls = tail_metric(&calls, "p95_function_calls", &mut result.unavailable);
    result.tokens_per_completion = result
        .total_tokens_consumed
        .zip((result.completed_runs > 0).then_some(result.completed_runs))
        .map(|(n, d)| n as f64 / f64::from(d));
    if result.total_tokens_consumed.is_none() {
        result.unavailable.insert(
            "total_tokens_consumed".into(),
            "missing attempt usage, incomplete retry evidence, or token sum overflow".into(),
        );
    }
    if calls.is_none() {
        result.unavailable.insert(
            "function_calls".into(),
            "missing call telemetry or incomplete retry evidence".into(),
        );
    }
    if result.completed_runs == 0 {
        result
            .unavailable
            .insert("tokens_per_completion".into(), "no completed runs".into());
    }
    result
}

fn tail_metric(
    values: &Option<Vec<f64>>,
    field: &str,
    unavailable: &mut BTreeMap<String, String>,
) -> Option<f64> {
    let Some(values) = values else {
        return None;
    };
    if values.len() < MINIMUM_TAIL_SAMPLE {
        unavailable.insert(
            field.into(),
            format!(
                "requires at least {MINIMUM_TAIL_SAMPLE} comparable runs; observed {}",
                values.len()
            ),
        );
        return None;
    }
    percentile(values, 95)
}

fn median(values: &[f64]) -> Option<f64> {
    percentile(values, 50)
}

fn percentile(values: &[f64], percentile: usize) -> Option<f64> {
    if values.is_empty() || !(1..=100).contains(&percentile) {
        return None;
    }
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    if percentile == 50 && values.len().is_multiple_of(2) {
        let middle = values.len() / 2;
        return Some((values[middle - 1] + values[middle]) / 2.0);
    }
    let rank = percentile.saturating_mul(values.len()).saturating_add(99) / 100;
    values.get(rank.saturating_sub(1)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::RunStatus;

    #[test]
    fn tail_metrics_require_twenty_complete_samples() {
        let mut unavailable = BTreeMap::new();
        assert_eq!(
            tail_metric(&Some(vec![1.0; 19]), "p95", &mut unavailable),
            None
        );
        assert!(unavailable.contains_key("p95"));
        assert_eq!(
            tail_metric(
                &Some((1..=20).map(f64::from).collect()),
                "p95",
                &mut BTreeMap::new()
            ),
            Some(19.0)
        );
    }

    fn usage_run(index: u64, tokens: u64) -> E2eRunReport {
        let mut run = E2eRunReport::new(
            format!("run-{index}"),
            format!("attempt-{index}"),
            1,
            format!("session-{index}"),
            "prompt".into(),
        );
        run.status = RunStatus::Passed;
        run.completion = CompletionState::Completed;
        run.technical = TechnicalState::Valid;
        run.score = Some(100);
        run.efficiency = Some(
            serde_json::from_value(serde_json::json!({
                "wall_time_ms": 100, "root_turns": 2, "child_turns": 0, "child_sessions": 0,
                "function_calls": 0, "function_call_errors": 0, "validation_retries": 0,
                "transient_resumes": 0, "wake_resumes": 0, "effective_fan_out": 0,
                "critical_path_ms": 100, "input_tokens": tokens, "output_tokens": 0,
                "total_tokens": tokens, "cost_usd": null,
                "observed_work": 2, "technical_attempts": 1,
                "observed_complexity": {}, "unavailable": {}
            }))
            .unwrap(),
        );
        run
    }

    #[test]
    fn consumption_preserves_failed_and_infrastructure_usage_without_rewarding_failure() {
        let first = usage_run(1, 100);
        let mut failed = usage_run(2, 20);
        failed.completion = CompletionState::TaskIncomplete;
        failed.status = RunStatus::Passed;
        let mut infra = usage_run(3, 10);
        infra.completion = CompletionState::Undetermined;
        infra.technical = TechnicalState::TechnicalInvalid;
        infra.status = RunStatus::InfrastructureError;
        let metrics = consumption_metrics(&[first, failed, infra]);
        assert_eq!(metrics.total_tokens_consumed, Some(130));
        assert_eq!(metrics.p95_total_tokens, None);
        assert_eq!(metrics.p50_function_calls, Some(0.0));
    }

    #[test]
    fn incomplete_retry_and_duplicate_observations_never_supply_cheap_efficiency() {
        let mut run = usage_run(1, 100);
        let retry = crate::report::RetryAttemptReport::from(&usage_run(2, 20));
        run.retry_attempts.push(retry);
        assert_eq!(
            consumption_metrics(&[run.clone()]).total_tokens_consumed,
            None
        );
        run.efficiency.as_mut().unwrap().technical_attempts = 2;
        run.efficiency.as_mut().unwrap().total_tokens = Some(120);
        assert_eq!(
            consumption_metrics(&[run.clone()]).total_tokens_consumed,
            Some(120)
        );
        run.efficiency
            .as_mut()
            .unwrap()
            .unavailable
            .insert("retry_efficiency".into(), "missing attempt".into());
        assert_eq!(
            consumption_metrics(&[run.clone()]).total_tokens_consumed,
            None
        );
        assert!(consumption_metrics(&[run.clone(), run])
            .unavailable
            .contains_key("consumption"));
    }

    #[test]
    fn consumption_tail_requires_twenty_samples() {
        let runs: Vec<_> = (1..=20).map(|n| usage_run(n, n)).collect();
        assert_eq!(consumption_metrics(&runs[..19]).p95_total_tokens, None);
        assert_eq!(consumption_metrics(&runs).p95_total_tokens, Some(19.0));
    }
}
