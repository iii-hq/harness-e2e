use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::audit::AuditSeverity;
use crate::longitudinal::{CaseComparison, ComparisonSummary, DeltaValue};
use crate::report::{E2eReport, RunStatus};

use super::{
    HarnessImprovementProposalV1, ImprovementDirection, ImprovementLoopSpecV1, ImprovementMetric,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImprovementDecisionOutcome {
    AcceptedRepeatable,
    Rejected,
    InsufficientEvidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImprovementDecision {
    pub outcome: ImprovementDecisionOutcome,
    pub accepted: bool,
    pub maturity: String,
    pub objective_metric: ImprovementMetric,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective_delta: Option<DeltaValue>,
    pub objective_met: bool,
    pub comparison_gate_passed: bool,
    pub reasons: Vec<String>,
}

pub fn decide_candidate(
    spec: &ImprovementLoopSpecV1,
    baseline: &E2eReport,
    candidate: &E2eReport,
    comparison: &ComparisonSummary,
    proposal: &HarnessImprovementProposalV1,
) -> ImprovementDecision {
    let mut reasons = Vec::new();
    let mut insufficient = false;
    if !comparison.comparable {
        insufficient = true;
        reasons.push("baseline and candidate do not form a comparable cohort".into());
    }
    if !comparison.cohort.excluded_cases.is_empty() {
        insufficient = true;
        reasons.push("one or more materialized cases were excluded from comparison".into());
    }
    for scenario_id in &spec.scenarios {
        if !comparison
            .cases
            .iter()
            .any(|case| &case.key.scenario_id == scenario_id)
        {
            insufficient = true;
            reasons.push(format!(
                "frozen scenario '{scenario_id}' is absent from the comparable cohort"
            ));
        }
    }
    for case in &comparison.cases {
        let required_runs = spec.acceptance_policy.required_runs_per_case;
        if case.from.included_runs < required_runs || case.to.included_runs < required_runs {
            insufficient = true;
            reasons.push(format!(
                "scenario '{}' has {}/{} eligible runs; {} are required on each side",
                case.key.scenario_id, case.from.included_runs, case.to.included_runs, required_runs
            ));
        }
    }
    if comparison.regressions.iter().any(|signal| signal.blocking) {
        reasons.push("the canonical longitudinal comparison contains a blocking regression".into());
    }
    if !comparison.gate_passed {
        reasons.push("the canonical longitudinal comparison gate did not pass".into());
    }
    reasons.extend(sentinel_status_regressions(spec, baseline, candidate));
    reasons.extend(sentinel_metric_regressions(spec, comparison));
    if candidate_has_critical_audit(candidate) {
        reasons.push("candidate contains a critical behavioral-audit finding".into());
    }

    let target_case = comparison
        .cases
        .iter()
        .find(|case| case.key.scenario_id == proposal.objective.scenario_id);
    let objective_delta =
        target_case.and_then(|case| metric_delta_for(case, proposal.objective.metric));
    let objective_met = objective_delta.is_some_and(|delta| {
        objective_satisfied(
            proposal.objective.metric,
            proposal.objective.direction,
            proposal.objective.minimum_change,
            delta,
        )
    });
    if target_case.is_none() || objective_delta.is_none() {
        insufficient = true;
        reasons.push("the frozen target objective metric is unavailable".into());
    } else if !objective_met {
        reasons.push(format!(
            "target objective {:?} did not reach the frozen minimum change {}",
            proposal.objective.metric, proposal.objective.minimum_change
        ));
    }

    reasons.sort();
    reasons.dedup();
    let accepted = !insufficient && reasons.is_empty() && comparison.gate_passed && objective_met;
    let outcome = if accepted {
        ImprovementDecisionOutcome::AcceptedRepeatable
    } else if insufficient {
        ImprovementDecisionOutcome::InsufficientEvidence
    } else {
        ImprovementDecisionOutcome::Rejected
    };
    ImprovementDecision {
        outcome,
        accepted,
        maturity: if accepted {
            "repeatable".into()
        } else if insufficient {
            "insufficient".into()
        } else {
            "directional".into()
        },
        objective_metric: proposal.objective.metric,
        objective_delta,
        objective_met,
        comparison_gate_passed: comparison.gate_passed,
        reasons,
    }
}

pub fn metric_delta_for(case: &CaseComparison, metric: ImprovementMetric) -> Option<DeltaValue> {
    match metric {
        ImprovementMetric::DeliverableSuccessRate => case.delta.deliverable_success_rate,
        ImprovementMetric::StructuralIntegrityRate => case.delta.structural_integrity_rate,
        ImprovementMetric::TechnicalFailureRate => case.delta.technical_failure_rate,
        ImprovementMetric::MedianScore => case.delta.median_score,
        ImprovementMetric::FunctionCalls => case.delta.p50_function_calls,
        ImprovementMetric::FunctionCallErrors => case.delta.p50_function_call_errors,
        ImprovementMetric::ValidationRetries => case.delta.p50_validation_retries,
        ImprovementMetric::Turns => case.delta.p50_turns,
        ImprovementMetric::WallTime => case.delta.p50_wall_time_ms,
        ImprovementMetric::TotalTokens => case.delta.p50_total_tokens,
        ImprovementMetric::Cost => case.delta.p50_cost_usd,
        ImprovementMetric::WorkAmplification => case.delta.p50_work_amplification,
    }
}

fn objective_satisfied(
    metric: ImprovementMetric,
    direction: ImprovementDirection,
    minimum_change: f64,
    delta: DeltaValue,
) -> bool {
    let observed = if metric.uses_relative_change() {
        let Some(relative) = delta.relative_ratio else {
            return false;
        };
        relative
    } else {
        delta.absolute
    };
    match direction {
        ImprovementDirection::Increase => observed >= minimum_change,
        ImprovementDirection::Decrease => observed <= -minimum_change,
    }
}

fn sentinel_status_regressions(
    spec: &ImprovementLoopSpecV1,
    baseline: &E2eReport,
    candidate: &E2eReport,
) -> Vec<String> {
    spec.scenarios
        .iter()
        .filter(|scenario| *scenario != &spec.target_scenario)
        .filter_map(|scenario_id| {
            let baseline_passed = baseline
                .scenarios
                .iter()
                .find(|scenario| &scenario.scenario_id == scenario_id)
                .is_some_and(|scenario| scenario.runs.iter().all(deterministic_run_passed));
            let candidate_passed = candidate
                .scenarios
                .iter()
                .find(|scenario| &scenario.scenario_id == scenario_id)
                .is_some_and(|scenario| scenario.runs.iter().all(deterministic_run_passed));
            (baseline_passed && !candidate_passed).then(|| {
                format!("sentinel '{scenario_id}' introduced a non-passing deterministic result")
            })
        })
        .collect()
}

fn deterministic_run_passed(run: &crate::report::E2eRunReport) -> bool {
    matches!(run.status, RunStatus::Passed | RunStatus::JudgeError)
        && run.hard_gates.iter().all(|gate| gate.passed)
}

fn sentinel_metric_regressions(
    spec: &ImprovementLoopSpecV1,
    comparison: &ComparisonSummary,
) -> Vec<String> {
    let thresholds = &spec.thresholds;
    let mut reasons = Vec::new();
    for case in comparison
        .cases
        .iter()
        .filter(|case| case.key.scenario_id != spec.target_scenario)
    {
        for (label, delta, threshold) in [
            (
                "cost",
                case.delta.p50_cost_usd,
                thresholds.sentinel_resource_increase_ratio,
            ),
            (
                "wall time",
                case.delta.p50_wall_time_ms,
                thresholds.sentinel_resource_increase_ratio,
            ),
            (
                "function calls",
                case.delta.p50_function_calls,
                thresholds.sentinel_resource_increase_ratio,
            ),
            (
                "tokens",
                case.delta.p50_total_tokens,
                thresholds.sentinel_resource_increase_ratio,
            ),
            (
                "work amplification",
                case.delta.p50_work_amplification,
                thresholds.sentinel_resource_increase_ratio,
            ),
            (
                "turns",
                case.delta.p50_turns,
                thresholds.sentinel_turns_increase_ratio,
            ),
        ] {
            if delta
                .and_then(|value| value.relative_ratio)
                .is_some_and(|ratio| ratio > threshold)
            {
                reasons.push(format!(
                    "sentinel '{}' median {label} increased beyond {:.0}%",
                    case.key.scenario_id,
                    threshold * 100.0
                ));
            }
        }
        for (label, delta) in [
            ("function errors", case.delta.p50_function_call_errors),
            ("validation retries", case.delta.p50_validation_retries),
        ] {
            if delta.is_some_and(|value| value.absolute >= thresholds.sentinel_discrete_increase) {
                reasons.push(format!(
                    "sentinel '{}' median {label} increased by at least {}",
                    case.key.scenario_id, thresholds.sentinel_discrete_increase
                ));
            }
        }
    }
    reasons
}

fn candidate_has_critical_audit(candidate: &E2eReport) -> bool {
    candidate
        .scenarios
        .iter()
        .flat_map(|scenario| &scenario.runs)
        .filter_map(|run| run.audit.as_ref())
        .flat_map(|audit| &audit.flags)
        .any(|flag| flag.severity == AuditSeverity::Critical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::improvement::{ImprovementObjective, ImprovementThresholds};
    use crate::longitudinal::{
        BenchmarkDelta, CaseCohortKey, CaseMetrics, CohortAudit, ComparisonSummary,
        ExecutionCohortIdentity,
    };
    use crate::scenarios::ComplexityTier;

    fn case(scenario_id: &str, delta: BenchmarkDelta) -> CaseComparison {
        CaseComparison {
            key: CaseCohortKey {
                execution: ExecutionCohortIdentity {
                    lane: "local".into(),
                    stack_mode: "source".into(),
                    subject_provider: "provider".into(),
                    subject_model: "model".into(),
                    judge_provider: None,
                    judge_model: None,
                    judge_protocol: None,
                    e2e_repository: Some("repo".into()),
                },
                scenario_id: scenario_id.into(),
                scenario_version: 1,
                case_id: format!("{scenario_id}-case"),
                seed: 4_404,
                inputs_sha256: format!("sha256:{}", "a".repeat(64)),
                contract_sha256: format!("sha256:{}", "b".repeat(64)),
            },
            complexity_tier: ComplexityTier::L1Sequential,
            from_run_ids: (0..5).map(|i| format!("from-{i}")).collect(),
            to_run_ids: (0..5).map(|i| format!("to-{i}")).collect(),
            from: CaseMetrics {
                included_runs: 5,
                ..Default::default()
            },
            to: CaseMetrics {
                included_runs: 5,
                ..Default::default()
            },
            delta,
            regressions: Vec::new(),
        }
    }

    #[test]
    fn continuous_objective_requires_the_frozen_relative_improvement() {
        let comparison = case(
            "tool_contract_recovery",
            BenchmarkDelta {
                p50_function_calls: Some(DeltaValue {
                    absolute: -2.0,
                    relative_ratio: Some(-0.20),
                }),
                ..Default::default()
            },
        );
        let objective = ImprovementObjective {
            scenario_id: "tool_contract_recovery".into(),
            metric: ImprovementMetric::FunctionCalls,
            direction: ImprovementDirection::Decrease,
            minimum_change: ImprovementThresholds::default().continuous_minimum_ratio,
        };
        let delta = metric_delta_for(&comparison, objective.metric).unwrap();
        assert!(objective_satisfied(
            objective.metric,
            objective.direction,
            objective.minimum_change,
            delta
        ));
    }

    #[test]
    fn judge_error_is_advisory_but_failed_hard_gate_is_not() {
        let mut run = crate::report::E2eRunReport::new(
            "run".into(),
            "attempt".into(),
            1,
            "session".into(),
            "prompt".into(),
        );
        run.status = RunStatus::JudgeError;
        assert!(deterministic_run_passed(&run));

        run.hard_gates.push(crate::report::HardGateReport {
            id: "durable_effect".into(),
            dimension: crate::report::EvaluationDimension::StructuralIntegrity,
            passed: false,
            reason: "missing deterministic state".into(),
        });
        assert!(!deterministic_run_passed(&run));
    }

    #[test]
    fn sentinel_resource_regression_is_reported_at_repeatable_sample_size() {
        let temp = tempfile::tempdir().unwrap();
        let spec = super::super::tests::valid_spec(temp.path());
        let comparison = ComparisonSummary {
            comparison_id: "comparison".into(),
            from_execution_id: "from".into(),
            to_execution_id: "to".into(),
            from_revision: Some(spec.base_revision.clone()),
            to_revision: Some("c".repeat(40)),
            baseline_id: None,
            judge_errors_advisory: true,
            comparable: true,
            gate_passed: true,
            reasons: Vec::new(),
            cohort: CohortAudit::default(),
            cases: vec![case(
                "minimal_path",
                BenchmarkDelta {
                    p50_function_calls: Some(DeltaValue {
                        absolute: 3.0,
                        relative_ratio: Some(0.30),
                    }),
                    ..Default::default()
                },
            )],
            regressions: Vec::new(),
        };
        let reasons = sentinel_metric_regressions(&spec, &comparison);
        assert!(reasons.iter().any(|reason| reason.contains("minimal_path")));
    }

    fn fixed_comparison(spec: &ImprovementLoopSpecV1) -> ComparisonSummary {
        ComparisonSummary {
            comparison_id: "comparison".into(),
            from_execution_id: "from".into(),
            to_execution_id: "to".into(),
            from_revision: Some(spec.base_revision.clone()),
            to_revision: Some("c".repeat(40)),
            baseline_id: None,
            judge_errors_advisory: true,
            comparable: true,
            gate_passed: true,
            reasons: Vec::new(),
            cohort: CohortAudit::default(),
            cases: spec
                .scenarios
                .iter()
                .map(|scenario| {
                    case(
                        scenario,
                        if scenario == &spec.target_scenario {
                            BenchmarkDelta {
                                p50_function_call_errors: Some(DeltaValue {
                                    absolute: -1.0,
                                    relative_ratio: None,
                                }),
                                ..Default::default()
                            }
                        } else {
                            BenchmarkDelta::default()
                        },
                    )
                })
                .collect(),
            regressions: Vec::new(),
        }
    }

    #[test]
    fn deterministic_evidence_can_accept_exactly_the_frozen_objective() {
        let temp = tempfile::tempdir().unwrap();
        let spec = super::super::tests::valid_spec(temp.path());
        let baseline = super::super::tests::trace_report("secret-a");
        let candidate = baseline.clone();
        let (_, proposal) = super::super::tests::valid_input_and_proposal();
        let decision = decide_candidate(
            &spec,
            &baseline,
            &candidate,
            &fixed_comparison(&spec),
            &proposal,
        );
        assert!(decision.accepted);
        assert_eq!(
            decision.outcome,
            ImprovementDecisionOutcome::AcceptedRepeatable
        );
    }

    #[test]
    fn missing_metric_or_frozen_case_is_insufficient_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let spec = super::super::tests::valid_spec(temp.path());
        let baseline = super::super::tests::trace_report("secret-a");
        let candidate = baseline.clone();
        let (_, proposal) = super::super::tests::valid_input_and_proposal();
        let mut comparison = fixed_comparison(&spec);
        comparison.cases[0].delta.p50_function_call_errors = None;
        comparison.cases.pop();
        let decision = decide_candidate(&spec, &baseline, &candidate, &comparison, &proposal);
        assert!(!decision.accepted);
        assert_eq!(
            decision.outcome,
            ImprovementDecisionOutcome::InsufficientEvidence
        );
    }

    #[test]
    fn canonical_comparison_gate_cannot_be_overridden_by_the_advisor() {
        let temp = tempfile::tempdir().unwrap();
        let spec = super::super::tests::valid_spec(temp.path());
        let baseline = super::super::tests::trace_report("secret-a");
        let candidate = baseline.clone();
        let (_, proposal) = super::super::tests::valid_input_and_proposal();
        let mut comparison = fixed_comparison(&spec);
        comparison.gate_passed = false;
        let decision = decide_candidate(&spec, &baseline, &candidate, &comparison, &proposal);
        assert!(!decision.accepted);
        assert_eq!(decision.outcome, ImprovementDecisionOutcome::Rejected);
    }

    #[test]
    fn controlled_h1_regression_is_rejected_before_h2_is_accepted() {
        let temp = tempfile::tempdir().unwrap();
        let spec = super::super::tests::valid_spec(temp.path());
        let baseline = super::super::tests::trace_report("secret-a");
        let candidate = baseline.clone();
        let (_, proposal) = super::super::tests::valid_input_and_proposal();

        let mut h1 = fixed_comparison(&spec);
        let sentinel = h1
            .cases
            .iter_mut()
            .find(|case| case.key.scenario_id == "minimal_path")
            .unwrap();
        sentinel.delta.p50_function_calls = Some(DeltaValue {
            absolute: 3.0,
            relative_ratio: Some(0.30),
        });
        let h1_decision = decide_candidate(&spec, &baseline, &candidate, &h1, &proposal);
        assert_eq!(h1_decision.outcome, ImprovementDecisionOutcome::Rejected);
        assert!(h1_decision.objective_met);
        assert!(h1_decision
            .reasons
            .iter()
            .any(|reason| reason.contains("minimal_path")));

        let h2_decision = decide_candidate(
            &spec,
            &baseline,
            &candidate,
            &fixed_comparison(&spec),
            &proposal,
        );
        assert_eq!(
            h2_decision.outcome,
            ImprovementDecisionOutcome::AcceptedRepeatable
        );
        assert!(h2_decision.accepted);
    }
}
