use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::artifact::{self, ArtifactReference};
use crate::identity::{StackIdentity, SystemUnderTestIdentity};
use crate::report::{
    DimensionReport, E2eReport, E2eRunReport, E2eScenarioReport, EvaluationDimension, RunStatus,
};
use crate::scenarios::ComplexityTier;

pub const COMPARISON_POLICY_VERSION: u32 = 1;
pub const CAPABILITY_POLICY_VERSION: u32 = 1;
const MINIMUM_ROBUSTNESS_SAMPLE: usize = 5;
const MINIMUM_TAIL_SAMPLE: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RegressionThresholds {
    pub deliverable_success_drop: f64,
    pub structural_integrity_drop: f64,
    pub technical_failure_increase: f64,
    pub p95_cost_increase_ratio: f64,
    pub p95_wall_time_increase_ratio: f64,
    pub p95_turns_increase_ratio: f64,
}

impl Default for RegressionThresholds {
    fn default() -> Self {
        Self {
            deliverable_success_drop: 0.03,
            structural_integrity_drop: 0.02,
            technical_failure_increase: 0.02,
            p95_cost_increase_ratio: 0.20,
            p95_wall_time_increase_ratio: 0.20,
            p95_turns_increase_ratio: 0.25,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityPolicy {
    pub policy_version: u32,
    pub minimum_sample_size: u32,
    pub tail_minimum_sample_size: u32,
    pub minimum_deliverable_success_rate: f64,
    pub minimum_structural_integrity_rate: f64,
    pub maximum_technical_failure_rate: f64,
    pub maximum_p95_cost_usd: Option<f64>,
    pub maximum_p95_wall_time_ms: Option<u64>,
}

impl Default for CapabilityPolicy {
    fn default() -> Self {
        Self {
            policy_version: CAPABILITY_POLICY_VERSION,
            minimum_sample_size: MINIMUM_ROBUSTNESS_SAMPLE as u32,
            tail_minimum_sample_size: MINIMUM_TAIL_SAMPLE as u32,
            minimum_deliverable_success_rate: 0.90,
            minimum_structural_integrity_rate: 0.95,
            maximum_technical_failure_rate: 0.02,
            maximum_p95_cost_usd: None,
            maximum_p95_wall_time_ms: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ComparisonPolicy {
    #[serde(default)]
    pub regression: RegressionThresholds,
    #[serde(default)]
    pub capability: CapabilityPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionCohortIdentity {
    pub lane: String,
    pub stack_mode: String,
    pub subject_provider: String,
    pub subject_model: String,
    pub judge_provider: Option<String>,
    pub judge_model: Option<String>,
    pub judge_protocol: Option<String>,
    pub e2e_repository: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CaseCohortKey {
    pub execution: ExecutionCohortIdentity,
    pub scenario_id: String,
    pub scenario_version: u32,
    pub case_id: String,
    pub seed: u64,
    pub inputs_sha256: String,
    pub contract_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonSide {
    Current,
    Baseline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExcludedRun {
    pub side: ComparisonSide,
    pub scenario_id: String,
    pub case_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExcludedCase {
    pub side: ComparisonSide,
    pub scenario_id: String,
    pub case_id: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RateEstimate {
    pub successes: u32,
    pub sample_size: u32,
    pub rate: f64,
    pub ci95_lower: f64,
    pub ci95_upper: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CaseMetrics {
    pub included_runs: u32,
    pub excluded_infrastructure_runs: u32,
    pub deliverable_success: Option<RateEstimate>,
    pub structural_integrity: Option<RateEstimate>,
    pub technical_failure: Option<RateEstimate>,
    pub flaky_rate: Option<f64>,
    pub median_score: Option<f64>,
    pub p50_cost_usd: Option<f64>,
    pub p95_cost_usd: Option<f64>,
    pub p50_wall_time_ms: Option<f64>,
    pub p95_wall_time_ms: Option<f64>,
    pub p50_turns: Option<f64>,
    pub p95_turns: Option<f64>,
    pub retry_rate: Option<f64>,
    pub p50_work_amplification: Option<f64>,
    pub p95_work_amplification: Option<f64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeltaValue {
    pub absolute: f64,
    pub relative_ratio: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BenchmarkDelta {
    pub deliverable_success_rate: Option<DeltaValue>,
    pub structural_integrity_rate: Option<DeltaValue>,
    pub technical_failure_rate: Option<DeltaValue>,
    pub flaky_rate: Option<DeltaValue>,
    pub median_score: Option<DeltaValue>,
    pub p50_cost_usd: Option<DeltaValue>,
    pub p95_cost_usd: Option<DeltaValue>,
    pub p50_wall_time_ms: Option<DeltaValue>,
    pub p95_wall_time_ms: Option<DeltaValue>,
    pub p50_turns: Option<DeltaValue>,
    pub p95_turns: Option<DeltaValue>,
    pub retry_rate: Option<DeltaValue>,
    pub p50_work_amplification: Option<DeltaValue>,
    pub p95_work_amplification: Option<DeltaValue>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegressionDimension {
    Deliverable,
    StructuralIntegrity,
    TechnicalReliability,
    Cost,
    WallTime,
    Turns,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RegressionSignal {
    pub scenario_id: String,
    pub case_id: String,
    pub dimension: RegressionDimension,
    pub observed_delta: f64,
    pub threshold: f64,
    pub blocking: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CaseComparison {
    pub key: CaseCohortKey,
    pub complexity_tier: ComplexityTier,
    pub current_run_ids: Vec<String>,
    pub baseline_run_ids: Vec<String>,
    pub current: CaseMetrics,
    pub baseline: CaseMetrics,
    pub delta: BenchmarkDelta,
    pub regressions: Vec<RegressionSignal>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CohortAudit {
    pub included_case_ids: Vec<String>,
    pub excluded_cases: Vec<ExcludedCase>,
    pub excluded_runs: Vec<ExcludedRun>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TierCapability {
    pub tier: ComplexityTier,
    pub sample_size: u32,
    pub statistically_eligible: bool,
    pub reliable: bool,
    pub deliverable_success: Option<RateEstimate>,
    pub structural_integrity: Option<RateEstimate>,
    pub technical_failure: Option<RateEstimate>,
    pub flaky_rate: Option<f64>,
    pub p95_cost_usd: Option<f64>,
    pub p95_wall_time_ms: Option<f64>,
    pub cost_per_successful_deliverable: Option<f64>,
    pub p50_work_amplification: Option<f64>,
    pub p95_work_amplification: Option<f64>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityFrontier {
    pub policy: CapabilityPolicy,
    pub highest_statistically_eligible_tier: Option<ComplexityTier>,
    pub highest_reliable_tier: Option<ComplexityTier>,
    pub tiers: Vec<TierCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PromotedBaselineIdentity {
    pub baseline_id: String,
    pub name: String,
    pub version: u32,
    pub execution_id: String,
    pub report_sha256: String,
    pub promoted_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ComparisonSummary {
    pub comparison_id: String,
    pub policy_version: u32,
    pub current_execution_id: String,
    pub baseline_execution_id: String,
    pub current_revision: Option<String>,
    pub baseline_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promoted_baseline: Option<PromotedBaselineIdentity>,
    pub comparable: bool,
    pub gate_passed: bool,
    pub reasons: Vec<String>,
    pub cohort: CohortAudit,
    pub cases: Vec<CaseComparison>,
    pub regressions: Vec<RegressionSignal>,
    pub current_capability: CapabilityFrontier,
    pub baseline_capability: CapabilityFrontier,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ComparisonResponse {
    pub comparison: ComparisonSummary,
    pub artifacts: Vec<ArtifactReference>,
}

pub fn compare_reports(
    current_execution_id: &str,
    current_lane: &str,
    current: &E2eReport,
    baseline_execution_id: &str,
    baseline_lane: &str,
    baseline: &E2eReport,
    policy: ComparisonPolicy,
) -> Result<ComparisonSummary> {
    let current_identity = execution_identity(current_lane, current);
    let baseline_identity = execution_identity(baseline_lane, baseline);
    let mut reasons = identity_differences(&current_identity, &baseline_identity);
    let identity_matches = reasons.is_empty();
    let mut cohort = CohortAudit::default();
    let current_cases = cases_by_identity(current);
    let baseline_cases = cases_by_identity(baseline);
    let identities = current_cases
        .keys()
        .chain(baseline_cases.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut cases = Vec::new();

    for identity in identities {
        let current_case = current_cases.get(&identity).copied();
        let baseline_case = baseline_cases.get(&identity).copied();
        let (Some(current_case), Some(baseline_case)) = (current_case, baseline_case) else {
            let (side, scenario) = match (current_case, baseline_case) {
                (Some(scenario), None) => (ComparisonSide::Current, scenario),
                (None, Some(scenario)) => (ComparisonSide::Baseline, scenario),
                (None, None) => unreachable!("identity came from one of the maps"),
                (Some(_), Some(_)) => unreachable!("handled above"),
            };
            exclude_case(
                &mut cohort,
                side,
                scenario,
                vec!["case is absent from the other execution".into()],
            );
            continue;
        };

        let current_key = case_key(current_identity.clone(), current_case)?;
        let baseline_key = case_key(baseline_identity.clone(), baseline_case)?;
        let mut case_reasons = Vec::new();
        if !identity_matches {
            case_reasons.extend(reasons.iter().cloned());
        }
        if current_key.contract_sha256 != baseline_key.contract_sha256 {
            case_reasons.push("materialized scenario contract fingerprint differs".into());
        }
        if !case_reasons.is_empty() {
            exclude_case(
                &mut cohort,
                ComparisonSide::Current,
                current_case,
                case_reasons.clone(),
            );
            exclude_case(
                &mut cohort,
                ComparisonSide::Baseline,
                baseline_case,
                case_reasons,
            );
            continue;
        }

        let (current_runs, current_excluded) = eligible_runs(ComparisonSide::Current, current_case);
        let (baseline_runs, baseline_excluded) =
            eligible_runs(ComparisonSide::Baseline, baseline_case);
        cohort.excluded_runs.extend(current_excluded);
        cohort.excluded_runs.extend(baseline_excluded);
        if current_runs.is_empty() || baseline_runs.is_empty() {
            if current_runs.is_empty() {
                cohort.excluded_cases.push(ExcludedCase {
                    side: ComparisonSide::Current,
                    scenario_id: current_case.scenario_id.clone(),
                    case_id: current_case.case_id.clone(),
                    reasons: vec!["no eligible runs remain after infrastructure exclusion".into()],
                });
            }
            if baseline_runs.is_empty() {
                cohort.excluded_cases.push(ExcludedCase {
                    side: ComparisonSide::Baseline,
                    scenario_id: baseline_case.scenario_id.clone(),
                    case_id: baseline_case.case_id.clone(),
                    reasons: vec!["no eligible runs remain after infrastructure exclusion".into()],
                });
            }
            continue;
        }
        let current_metrics = case_metrics(&current_runs, current_case.runs.len());
        let baseline_metrics = case_metrics(&baseline_runs, baseline_case.runs.len());
        let delta = benchmark_delta(&current_metrics, &baseline_metrics);
        let regressions = regression_signals(
            &current_case.scenario_id,
            &current_case.case_id,
            &delta,
            policy.regression,
        );
        cohort.included_case_ids.push(current_case.case_id.clone());
        cases.push(CaseComparison {
            key: current_key,
            complexity_tier: current_case
                .case
                .as_ref()
                .context("comparable v2 scenario has no materialized case")?
                .complexity
                .tier,
            current_run_ids: current_runs.iter().map(|run| run.run_id.clone()).collect(),
            baseline_run_ids: baseline_runs.iter().map(|run| run.run_id.clone()).collect(),
            current: current_metrics,
            baseline: baseline_metrics,
            delta,
            regressions,
        });
    }

    if cases.is_empty() {
        reasons.push("no comparable materialized cases remain after cohort selection".into());
    }
    if !cohort.excluded_cases.is_empty() {
        reasons.push(format!(
            "{} materialized case entries were excluded from comparison",
            cohort.excluded_cases.len()
        ));
    }
    reasons.sort();
    reasons.dedup();
    cohort.included_case_ids.sort();
    let regressions = cases
        .iter()
        .flat_map(|case| case.regressions.iter().cloned())
        .collect::<Vec<_>>();
    let current_capability = capability_frontier(&cases, current, policy.capability);
    let baseline_capability = capability_frontier(&cases, baseline, policy.capability);
    let comparable = !cases.is_empty();
    let gate_passed = comparable
        && cohort.excluded_cases.is_empty()
        && regressions.iter().all(|signal| !signal.blocking);
    let mut summary = ComparisonSummary {
        comparison_id: String::new(),
        policy_version: COMPARISON_POLICY_VERSION,
        current_execution_id: current_execution_id.into(),
        baseline_execution_id: baseline_execution_id.into(),
        current_revision: revision(current),
        baseline_revision: revision(baseline),
        promoted_baseline: None,
        comparable,
        gate_passed,
        reasons,
        cohort,
        cases,
        regressions,
        current_capability,
        baseline_capability,
    };
    refresh_comparison_id(&mut summary)?;
    Ok(summary)
}

pub fn refresh_comparison_id(comparison: &mut ComparisonSummary) -> Result<()> {
    comparison.comparison_id.clear();
    comparison.comparison_id = artifact::sha256_value(comparison)?;
    Ok(())
}

pub fn write_comparison(
    output: &Path,
    comparison: &ComparisonSummary,
) -> Result<Vec<ArtifactReference>> {
    let comparison_directory = Path::new("comparisons").join(
        comparison
            .comparison_id
            .strip_prefix("sha256:")
            .unwrap_or(&comparison.comparison_id),
    );
    let delta_path = comparison_directory.join("e2e-delta.json");
    let mut delta_bytes = serde_json::to_vec_pretty(comparison)?;
    delta_bytes.push(b'\n');
    let delta = artifact::write_bytes(
        output,
        &delta_path,
        "e2e_delta",
        "comparison",
        "application/json",
        &delta_bytes,
    )?;
    let markdown = comparison_markdown(comparison);
    let summary_path = comparison_directory.join("e2e-summary.md");
    let summary = artifact::write_bytes(
        output,
        &summary_path,
        "e2e_summary",
        "comparison_summary",
        "text/markdown",
        markdown.as_bytes(),
    )?;
    Ok(vec![delta, summary])
}

fn execution_identity(lane: &str, report: &E2eReport) -> ExecutionCohortIdentity {
    ExecutionCohortIdentity {
        lane: lane.into(),
        stack_mode: report
            .system_under_test
            .as_ref()
            .map(|system| match system.stack {
                StackIdentity::Source { .. } => "source",
                StackIdentity::Registry { .. } => "registry",
            })
            .unwrap_or("unknown")
            .into(),
        subject_provider: report.subject.provider.clone(),
        subject_model: report.subject.model.clone(),
        judge_provider: report.judge.as_ref().map(|judge| judge.provider.clone()),
        judge_model: report.judge.as_ref().map(|judge| judge.model.clone()),
        judge_protocol: report.judge_protocol.clone(),
        e2e_repository: report
            .system_under_test
            .as_ref()
            .map(|system| system.e2e_repository.clone()),
    }
}

fn identity_differences(
    current: &ExecutionCohortIdentity,
    baseline: &ExecutionCohortIdentity,
) -> Vec<String> {
    let mut reasons = Vec::new();
    for (name, differs) in [
        ("execution lane differs", current.lane != baseline.lane),
        (
            "stack mode differs",
            current.stack_mode != baseline.stack_mode,
        ),
        (
            "subject model identity differs",
            current.subject_provider != baseline.subject_provider
                || current.subject_model != baseline.subject_model,
        ),
        (
            "judge model identity differs",
            current.judge_provider != baseline.judge_provider
                || current.judge_model != baseline.judge_model,
        ),
        (
            "judge protocol differs",
            current.judge_protocol != baseline.judge_protocol,
        ),
        (
            "E2E repository identity differs",
            current.e2e_repository != baseline.e2e_repository,
        ),
    ] {
        if differs {
            reasons.push(name.into());
        }
    }
    reasons
}

fn cases_by_identity(report: &E2eReport) -> BTreeMap<(String, String), &E2eScenarioReport> {
    report
        .scenarios
        .iter()
        .map(|scenario| {
            (
                (scenario.scenario_id.clone(), scenario.case_id.clone()),
                scenario,
            )
        })
        .collect()
}

fn case_key(
    execution: ExecutionCohortIdentity,
    scenario: &E2eScenarioReport,
) -> Result<CaseCohortKey> {
    let case = scenario
        .case
        .as_ref()
        .context("comparison requires a materialized v2 case")?;
    let contract_sha256 = artifact::sha256_value(&serde_json::json!({
        "scenario_id": scenario.scenario_id,
        "scenario_version": scenario.scenario_version,
        "case": case,
        "execution_policy": scenario.execution_policy,
    }))?;
    Ok(CaseCohortKey {
        execution,
        scenario_id: scenario.scenario_id.clone(),
        scenario_version: scenario.scenario_version,
        case_id: scenario.case_id.clone(),
        seed: case.seed,
        inputs_sha256: case.inputs_sha256.clone(),
        contract_sha256,
    })
}

fn eligible_runs(
    side: ComparisonSide,
    scenario: &E2eScenarioReport,
) -> (Vec<&E2eRunReport>, Vec<ExcludedRun>) {
    let mut included = Vec::new();
    let mut excluded = Vec::new();
    for run in &scenario.runs {
        if run.status == RunStatus::InfrastructureError {
            excluded.push(ExcludedRun {
                side: side.clone(),
                scenario_id: scenario.scenario_id.clone(),
                case_id: scenario.case_id.clone(),
                run_id: run.run_id.clone(),
                attempt_id: run.attempt_id.clone(),
                reason: "E2E infrastructure failure is outside the capability cohort".into(),
            });
        } else {
            included.push(run);
        }
    }
    (included, excluded)
}

fn exclude_case(
    cohort: &mut CohortAudit,
    side: ComparisonSide,
    scenario: &E2eScenarioReport,
    reasons: Vec<String>,
) {
    let run_reason = format!("case excluded: {}", reasons.join("; "));
    cohort.excluded_cases.push(ExcludedCase {
        side: side.clone(),
        scenario_id: scenario.scenario_id.clone(),
        case_id: scenario.case_id.clone(),
        reasons,
    });
    cohort
        .excluded_runs
        .extend(scenario.runs.iter().map(|run| ExcludedRun {
            side: side.clone(),
            scenario_id: scenario.scenario_id.clone(),
            case_id: scenario.case_id.clone(),
            run_id: run.run_id.clone(),
            attempt_id: run.attempt_id.clone(),
            reason: run_reason.clone(),
        }));
}

fn case_metrics(runs: &[&E2eRunReport], total_runs: usize) -> CaseMetrics {
    let mut unavailable = BTreeMap::new();
    let deliverable_success = dimension_rate(runs, EvaluationDimension::Deliverable);
    if deliverable_success.is_none() {
        unavailable.insert(
            "deliverable_success".into(),
            "one or more included runs lacks a deliverable outcome".into(),
        );
    }
    let structural_integrity = dimension_rate(runs, EvaluationDimension::StructuralIntegrity);
    if structural_integrity.is_none() {
        unavailable.insert(
            "structural_integrity".into(),
            "one or more included runs lacks a structural outcome".into(),
        );
    }
    let technical_failure = (!runs.is_empty()).then(|| {
        rate_estimate(
            runs.iter()
                .filter(|run| {
                    matches!(
                        run.status,
                        RunStatus::SubjectError | RunStatus::JudgeError | RunStatus::ResourceLimit
                    )
                })
                .count(),
            runs.len(),
        )
    });
    let flaky_rate = if runs.len() >= 2 {
        let mut statuses = BTreeMap::<RunStatus, usize>::new();
        for run in runs {
            *statuses.entry(run.status).or_default() += 1;
        }
        let modal = statuses.values().copied().max().unwrap_or(0);
        Some(runs.len().saturating_sub(modal) as f64 / runs.len() as f64)
    } else {
        unavailable.insert(
            "flaky_rate".into(),
            "requires at least two included runs".into(),
        );
        None
    };
    let scores = runs
        .iter()
        .filter_map(|run| run.score.map(f64::from))
        .collect::<Vec<_>>();
    let costs = collect_complete(runs, |run| run.cost.total_usd);
    let wall_times = runs
        .iter()
        .map(|run| run.wall_time_ms as f64)
        .collect::<Vec<_>>();
    let turns = collect_complete(runs, |run| {
        run.efficiency
            .as_ref()
            .and_then(|value| value.root_turns.zip(value.child_turns))
            .map(|(root, child)| (root + child) as f64)
    });
    let amplification = collect_complete(runs, |run| run.efficiency.as_ref()?.work_amplification);
    let retry_rate = (!runs.is_empty()).then(|| {
        runs.iter()
            .filter(|run| !run.retry_attempts.is_empty())
            .count() as f64
            / runs.len() as f64
    });
    let p95_cost_usd = tail_metric(&costs, "p95_cost_usd", &mut unavailable);
    let p95_wall_time_ms = tail_metric(
        &Some(wall_times.clone()),
        "p95_wall_time_ms",
        &mut unavailable,
    );
    let p95_turns = tail_metric(&turns, "p95_turns", &mut unavailable);
    let p95_work_amplification =
        tail_metric(&amplification, "p95_work_amplification", &mut unavailable);
    if costs.is_none() {
        unavailable.insert(
            "cost".into(),
            "one or more included runs lacks total cost".into(),
        );
    }
    if turns.is_none() {
        unavailable.insert(
            "turns".into(),
            "one or more included runs lacks turn metrics".into(),
        );
    }
    if amplification.is_none() {
        unavailable.insert(
            "work_amplification".into(),
            "one or more included runs lacks work amplification".into(),
        );
    }
    CaseMetrics {
        included_runs: runs.len().try_into().unwrap_or(u32::MAX),
        excluded_infrastructure_runs: total_runs
            .saturating_sub(runs.len())
            .try_into()
            .unwrap_or(u32::MAX),
        deliverable_success,
        structural_integrity,
        technical_failure,
        flaky_rate,
        median_score: median(&scores),
        p50_cost_usd: costs.as_deref().and_then(median),
        p95_cost_usd,
        p50_wall_time_ms: median(&wall_times),
        p95_wall_time_ms,
        p50_turns: turns.as_deref().and_then(median),
        p95_turns,
        retry_rate,
        p50_work_amplification: amplification.as_deref().and_then(median),
        p95_work_amplification,
        unavailable,
    }
}

fn benchmark_delta(current: &CaseMetrics, baseline: &CaseMetrics) -> BenchmarkDelta {
    let mut unavailable = BTreeMap::new();
    macro_rules! delta {
        ($field:literal, $current:expr, $baseline:expr) => {{
            let value = metric_delta($current, $baseline);
            if value.is_none() {
                unavailable.insert(
                    $field.into(),
                    "metric unavailable in current or baseline cohort".into(),
                );
            }
            value
        }};
    }
    BenchmarkDelta {
        deliverable_success_rate: delta!(
            "deliverable_success_rate",
            current.deliverable_success.as_ref().map(|value| value.rate),
            baseline
                .deliverable_success
                .as_ref()
                .map(|value| value.rate)
        ),
        structural_integrity_rate: delta!(
            "structural_integrity_rate",
            current
                .structural_integrity
                .as_ref()
                .map(|value| value.rate),
            baseline
                .structural_integrity
                .as_ref()
                .map(|value| value.rate)
        ),
        technical_failure_rate: delta!(
            "technical_failure_rate",
            current.technical_failure.as_ref().map(|value| value.rate),
            baseline.technical_failure.as_ref().map(|value| value.rate)
        ),
        flaky_rate: delta!("flaky_rate", current.flaky_rate, baseline.flaky_rate),
        median_score: delta!("median_score", current.median_score, baseline.median_score),
        p50_cost_usd: delta!("p50_cost_usd", current.p50_cost_usd, baseline.p50_cost_usd),
        p95_cost_usd: delta!("p95_cost_usd", current.p95_cost_usd, baseline.p95_cost_usd),
        p50_wall_time_ms: delta!(
            "p50_wall_time_ms",
            current.p50_wall_time_ms,
            baseline.p50_wall_time_ms
        ),
        p95_wall_time_ms: delta!(
            "p95_wall_time_ms",
            current.p95_wall_time_ms,
            baseline.p95_wall_time_ms
        ),
        p50_turns: delta!("p50_turns", current.p50_turns, baseline.p50_turns),
        p95_turns: delta!("p95_turns", current.p95_turns, baseline.p95_turns),
        retry_rate: delta!("retry_rate", current.retry_rate, baseline.retry_rate),
        p50_work_amplification: delta!(
            "p50_work_amplification",
            current.p50_work_amplification,
            baseline.p50_work_amplification
        ),
        p95_work_amplification: delta!(
            "p95_work_amplification",
            current.p95_work_amplification,
            baseline.p95_work_amplification
        ),
        unavailable,
    }
}

fn regression_signals(
    scenario_id: &str,
    case_id: &str,
    delta: &BenchmarkDelta,
    policy: RegressionThresholds,
) -> Vec<RegressionSignal> {
    let mut signals = Vec::new();
    let mut push = |dimension, observed, threshold, reason: &str| {
        signals.push(RegressionSignal {
            scenario_id: scenario_id.into(),
            case_id: case_id.into(),
            dimension,
            observed_delta: observed,
            threshold,
            blocking: true,
            reason: reason.into(),
        });
    };
    if delta
        .deliverable_success_rate
        .is_some_and(|value| value.absolute < -policy.deliverable_success_drop)
    {
        push(
            RegressionDimension::Deliverable,
            delta.deliverable_success_rate.expect("checked").absolute,
            -policy.deliverable_success_drop,
            "deliverable success rate fell beyond policy",
        );
    }
    if delta
        .structural_integrity_rate
        .is_some_and(|value| value.absolute < -policy.structural_integrity_drop)
    {
        push(
            RegressionDimension::StructuralIntegrity,
            delta.structural_integrity_rate.expect("checked").absolute,
            -policy.structural_integrity_drop,
            "structural integrity rate fell beyond policy",
        );
    }
    if delta
        .technical_failure_rate
        .is_some_and(|value| value.absolute > policy.technical_failure_increase)
    {
        push(
            RegressionDimension::TechnicalReliability,
            delta.technical_failure_rate.expect("checked").absolute,
            policy.technical_failure_increase,
            "technical failure rate grew beyond policy",
        );
    }
    for (dimension, value, threshold, reason) in [
        (
            RegressionDimension::Cost,
            delta.p95_cost_usd,
            policy.p95_cost_increase_ratio,
            "p95 cost grew beyond policy",
        ),
        (
            RegressionDimension::WallTime,
            delta.p95_wall_time_ms,
            policy.p95_wall_time_increase_ratio,
            "p95 wall time grew beyond policy",
        ),
        (
            RegressionDimension::Turns,
            delta.p95_turns,
            policy.p95_turns_increase_ratio,
            "p95 turns grew beyond policy",
        ),
    ] {
        if value
            .and_then(|value| value.relative_ratio)
            .is_some_and(|ratio| ratio > threshold)
        {
            push(
                dimension,
                value
                    .and_then(|value| value.relative_ratio)
                    .expect("checked"),
                threshold,
                reason,
            );
        }
    }
    signals
}

fn capability_frontier(
    cases: &[CaseComparison],
    report: &E2eReport,
    policy: CapabilityPolicy,
) -> CapabilityFrontier {
    let selected = cases
        .iter()
        .map(|case| {
            (
                (case.key.scenario_id.clone(), case.key.case_id.clone()),
                case.complexity_tier,
            )
        })
        .collect::<BTreeMap<_, _>>();
    capability_for_selection(report, &selected, policy)
}

pub fn capability_for_report(report: &E2eReport, policy: CapabilityPolicy) -> CapabilityFrontier {
    let selected = report
        .scenarios
        .iter()
        .filter_map(|scenario| {
            scenario.case.as_ref().map(|case| {
                (
                    (scenario.scenario_id.clone(), scenario.case_id.clone()),
                    case.complexity.tier,
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    capability_for_selection(report, &selected, policy)
}

fn capability_for_selection(
    report: &E2eReport,
    selected: &BTreeMap<(String, String), ComplexityTier>,
    policy: CapabilityPolicy,
) -> CapabilityFrontier {
    let tiers = [
        ComplexityTier::L0Atomic,
        ComplexityTier::L1Sequential,
        ComplexityTier::L2Stateful,
        ComplexityTier::L3Concurrent,
        ComplexityTier::L4Coordinated,
        ComplexityTier::L5Adaptive,
    ];
    let mut reports = Vec::new();
    for tier in tiers {
        let cohort = report
            .scenarios
            .iter()
            .filter(|scenario| {
                selected.get(&(scenario.scenario_id.clone(), scenario.case_id.clone()))
                    == Some(&tier)
            })
            .flat_map(|scenario| {
                scenario
                    .runs
                    .iter()
                    .filter(|run| run.status != RunStatus::InfrastructureError)
            })
            .collect::<Vec<_>>();
        if cohort.is_empty() {
            continue;
        }
        let sample_size = cohort.len().try_into().unwrap_or(u32::MAX);
        let deliverable_success = dimension_rate(&cohort, EvaluationDimension::Deliverable);
        let structural_integrity =
            dimension_rate(&cohort, EvaluationDimension::StructuralIntegrity);
        let technical_failure = Some(rate_estimate(
            cohort
                .iter()
                .filter(|run| {
                    matches!(
                        run.status,
                        RunStatus::SubjectError | RunStatus::JudgeError | RunStatus::ResourceLimit
                    )
                })
                .count(),
            cohort.len(),
        ));
        let flaky_rate = cohort_flaky_rate(&cohort);
        let costs = collect_complete(&cohort, |run| run.cost.total_usd);
        let wall_times = cohort
            .iter()
            .map(|run| run.wall_time_ms as f64)
            .collect::<Vec<_>>();
        let amplification =
            collect_complete(&cohort, |run| run.efficiency.as_ref()?.work_amplification);
        let p95_cost_usd = capability_tail(&costs, policy.tail_minimum_sample_size);
        let p95_wall_time_ms = capability_tail(&Some(wall_times), policy.tail_minimum_sample_size);
        let p50_work_amplification = amplification.as_deref().and_then(median);
        let p95_work_amplification =
            capability_tail(&amplification, policy.tail_minimum_sample_size);
        let deliverable_successes = deliverable_success
            .as_ref()
            .map(|estimate| estimate.successes)
            .unwrap_or(0);
        let cost_per_successful_deliverable = costs.as_ref().and_then(|values| {
            (deliverable_successes > 0)
                .then(|| values.iter().sum::<f64>() / f64::from(deliverable_successes))
        });
        let mut reasons = Vec::new();
        if sample_size < policy.minimum_sample_size {
            reasons.push(format!(
                "requires at least {} runs; observed {sample_size}",
                policy.minimum_sample_size
            ));
        }
        if deliverable_success
            .as_ref()
            .is_none_or(|value| value.rate < policy.minimum_deliverable_success_rate)
        {
            reasons.push("deliverable success threshold not met or unavailable".into());
        }
        if structural_integrity
            .as_ref()
            .is_none_or(|value| value.rate < policy.minimum_structural_integrity_rate)
        {
            reasons.push("structural integrity threshold not met or unavailable".into());
        }
        if technical_failure
            .as_ref()
            .is_none_or(|value| value.rate > policy.maximum_technical_failure_rate)
        {
            reasons.push("technical failure threshold not met or unavailable".into());
        }
        let statistically_eligible = reasons.is_empty();
        match policy.maximum_p95_cost_usd {
            Some(limit) if p95_cost_usd.is_some_and(|value| value <= limit) => {}
            Some(_) => reasons.push("p95 cost budget not met or unavailable".into()),
            None => reasons.push("p95 cost budget is not configured".into()),
        }
        match policy.maximum_p95_wall_time_ms {
            Some(limit) if p95_wall_time_ms.is_some_and(|value| value <= limit as f64) => {}
            Some(_) => reasons.push("p95 wall-time budget not met or unavailable".into()),
            None => reasons.push("p95 wall-time budget is not configured".into()),
        }
        reports.push(TierCapability {
            tier,
            sample_size,
            statistically_eligible,
            reliable: reasons.is_empty(),
            deliverable_success,
            structural_integrity,
            technical_failure,
            flaky_rate,
            p95_cost_usd,
            p95_wall_time_ms,
            cost_per_successful_deliverable,
            p50_work_amplification,
            p95_work_amplification,
            reasons,
        });
    }
    let highest_statistically_eligible_tier = reports
        .iter()
        .filter(|tier| tier.statistically_eligible)
        .max_by_key(|tier| tier_rank(tier.tier))
        .map(|tier| tier.tier);
    let highest_reliable_tier = reports
        .iter()
        .filter(|tier| tier.reliable)
        .max_by_key(|tier| tier_rank(tier.tier))
        .map(|tier| tier.tier);
    CapabilityFrontier {
        policy,
        highest_statistically_eligible_tier,
        highest_reliable_tier,
        tiers: reports,
    }
}

fn capability_tail(values: &Option<Vec<f64>>, minimum_sample_size: u32) -> Option<f64> {
    let values = values.as_ref()?;
    (values.len() >= minimum_sample_size as usize)
        .then(|| percentile(values, 95))
        .flatten()
}

fn cohort_flaky_rate(runs: &[&E2eRunReport]) -> Option<f64> {
    if runs.len() < 2 {
        return None;
    }
    let mut statuses = BTreeMap::<RunStatus, usize>::new();
    for run in runs {
        *statuses.entry(run.status).or_default() += 1;
    }
    let modal = statuses.values().copied().max().unwrap_or(0);
    Some(runs.len().saturating_sub(modal) as f64 / runs.len() as f64)
}

fn dimension_rate(runs: &[&E2eRunReport], dimension: EvaluationDimension) -> Option<RateEstimate> {
    if runs.is_empty() {
        return None;
    }
    let outcomes = runs
        .iter()
        .map(|run| dimension_outcome(run, dimension))
        .collect::<Option<Vec<_>>>()?;
    Some(rate_estimate(
        outcomes.iter().filter(|value| **value).count(),
        outcomes.len(),
    ))
}

fn dimension_outcome(run: &E2eRunReport, dimension: EvaluationDimension) -> Option<bool> {
    run.dimensions
        .iter()
        .find(|value: &&DimensionReport| value.dimension == dimension)
        .and_then(|value| value.passed)
}

fn rate_estimate(successes: usize, sample_size: usize) -> RateEstimate {
    let n = sample_size as f64;
    let p = successes as f64 / n;
    let z = 1.959_963_984_540_054_f64;
    let denominator = 1.0 + z * z / n;
    let center = (p + z * z / (2.0 * n)) / denominator;
    let margin = z * ((p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt()) / denominator;
    RateEstimate {
        successes: successes.try_into().unwrap_or(u32::MAX),
        sample_size: sample_size.try_into().unwrap_or(u32::MAX),
        rate: p,
        ci95_lower: (center - margin).max(0.0),
        ci95_upper: (center + margin).min(1.0),
    }
}

fn collect_complete(
    runs: &[&E2eRunReport],
    value: impl Fn(&E2eRunReport) -> Option<f64>,
) -> Option<Vec<f64>> {
    runs.iter()
        .map(|run| value(run))
        .collect::<Option<Vec<_>>>()
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

fn metric_delta(current: Option<f64>, baseline: Option<f64>) -> Option<DeltaValue> {
    current.zip(baseline).map(|(current, baseline)| DeltaValue {
        absolute: current - baseline,
        relative_ratio: (baseline != 0.0).then(|| (current - baseline) / baseline.abs()),
    })
}

fn tier_rank(tier: ComplexityTier) -> u8 {
    match tier {
        ComplexityTier::L0Atomic => 0,
        ComplexityTier::L1Sequential => 1,
        ComplexityTier::L2Stateful => 2,
        ComplexityTier::L3Concurrent => 3,
        ComplexityTier::L4Coordinated => 4,
        ComplexityTier::L5Adaptive => 5,
    }
}

fn revision(report: &E2eReport) -> Option<String> {
    report.system_under_test.as_ref().and_then(system_revision)
}

fn system_revision(system: &SystemUnderTestIdentity) -> Option<String> {
    match &system.stack {
        StackIdentity::Source {
            workers_revision, ..
        } => Some(workers_revision.clone()),
        StackIdentity::Registry {
            stack_lock_digest, ..
        } => Some(stack_lock_digest.clone()),
    }
}

fn comparison_markdown(comparison: &ComparisonSummary) -> String {
    let status = if comparison.gate_passed {
        "PASS"
    } else {
        "FAIL"
    };
    let mut output = format!(
        "# Harness E2E comparison: {status}\n\nComparison: `{}`  \nCurrent: `{}` at `{}`  \nBaseline execution: `{}` at `{}`  \nComparable cases: {}  \nExcluded cases: {}  \nExcluded runs: {}\n\n",
        comparison.comparison_id,
        comparison.current_execution_id,
        comparison.current_revision.as_deref().unwrap_or("unknown"),
        comparison.baseline_execution_id,
        comparison.baseline_revision.as_deref().unwrap_or("unknown"),
        comparison.cohort.included_case_ids.len(),
        comparison.cohort.excluded_cases.len(),
        comparison.cohort.excluded_runs.len(),
    );
    if let Some(baseline) = &comparison.promoted_baseline {
        output.push_str(&format!(
            "Promoted baseline: `{}` v{} (`{}`)  \nEvidence hash: `{}`\n\n",
            baseline.name, baseline.version, baseline.baseline_id, baseline.report_sha256,
        ));
    }
    if !comparison.reasons.is_empty() {
        output.push_str("## Cohort warnings\n\n");
        for reason in &comparison.reasons {
            output.push_str(&format!("- {reason}\n"));
        }
        output.push('\n');
    }
    output.push_str("## Independent regressions\n\n");
    if comparison.regressions.is_empty() {
        output.push_str("No blocking regression crossed its policy threshold.\n\n");
    } else {
        for regression in &comparison.regressions {
            let seed = comparison
                .cases
                .iter()
                .find(|case| {
                    case.key.scenario_id == regression.scenario_id
                        && case.key.case_id == regression.case_id
                })
                .map(|case| case.key.seed.to_string())
                .unwrap_or_else(|| "unknown".into());
            output.push_str(&format!(
                "- `{}` / `{}` / seed `{seed}` / `{:?}`: {} (observed {:.4}, threshold {:.4})\n",
                regression.scenario_id,
                regression.case_id,
                regression.dimension,
                regression.reason,
                regression.observed_delta,
                regression.threshold,
            ));
        }
        output.push('\n');
    }
    output.push_str("## Capability frontier\n\n");
    output.push_str(&format!(
        "Current highest reliable tier: `{}`  \nBaseline highest reliable tier: `{}`\n",
        comparison
            .current_capability
            .highest_reliable_tier
            .map(|tier| format!("{tier:?}"))
            .unwrap_or_else(|| "not established".into()),
        comparison
            .baseline_capability
            .highest_reliable_tier
            .map(|tier| format!("{tier:?}"))
            .unwrap_or_else(|| "not established".into()),
    ));
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{ExecutionIdentity, StackIdentity, SystemUnderTestIdentity};
    use crate::report::{CostReport, E2eScenarioReport};
    use crate::scenarios::ExecutionPolicy;
    use crate::scenarios::{ComplexityProfile, DeliverableContract, ScenarioCase};

    #[test]
    fn wilson_interval_exposes_sample_size_and_uncertainty() {
        let estimate = rate_estimate(9, 10);
        assert_eq!(estimate.sample_size, 10);
        assert_eq!(estimate.successes, 9);
        assert!((estimate.rate - 0.9).abs() < f64::EPSILON);
        assert!(estimate.ci95_lower < estimate.rate);
        assert!(estimate.ci95_upper > estimate.rate);
    }

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

    #[test]
    fn independent_regressions_do_not_require_an_aggregate_score() {
        let delta = BenchmarkDelta {
            deliverable_success_rate: Some(DeltaValue {
                absolute: -0.04,
                relative_ratio: Some(-0.04),
            }),
            p95_wall_time_ms: Some(DeltaValue {
                absolute: 30.0,
                relative_ratio: Some(0.30),
            }),
            ..BenchmarkDelta::default()
        };
        let signals =
            regression_signals("scenario", "case", &delta, RegressionThresholds::default());
        assert_eq!(signals.len(), 2);
        assert!(signals.iter().all(|signal| signal.blocking));
        assert!(signals
            .iter()
            .any(|signal| signal.dimension == RegressionDimension::Deliverable));
        assert!(signals
            .iter()
            .any(|signal| signal.dimension == RegressionDimension::WallTime));
    }

    #[test]
    fn comparisons_allow_revision_changes_but_audit_cases_and_runs() {
        let baseline = report("1111111111111111111111111111111111111111", false, false);
        let current = report("2222222222222222222222222222222222222222", true, true);
        let comparison = compare_reports(
            "current",
            "daily",
            &current,
            "baseline",
            "daily",
            &baseline,
            ComparisonPolicy::default(),
        )
        .unwrap();

        assert!(comparison.comparable);
        assert_eq!(comparison.cases.len(), 1);
        assert_eq!(comparison.cohort.excluded_runs.len(), 1);
        assert_eq!(
            comparison.current_revision.as_deref(),
            Some("2222222222222222222222222222222222222222")
        );
        assert!(comparison
            .regressions
            .iter()
            .any(|signal| signal.dimension == RegressionDimension::Deliverable));
        assert!(comparison
            .regressions
            .iter()
            .any(|signal| signal.dimension == RegressionDimension::Cost));
        assert!(!comparison.gate_passed);
    }

    #[test]
    fn comparison_artifacts_are_immutable_and_verifiable() {
        let baseline = report("1111111111111111111111111111111111111111", false, false);
        let current = report("2222222222222222222222222222222222222222", false, false);
        let comparison = compare_reports(
            "current",
            "daily",
            &current,
            "baseline",
            "daily",
            &baseline,
            ComparisonPolicy::default(),
        )
        .unwrap();
        let replay = compare_reports(
            "current",
            "daily",
            &current,
            "baseline",
            "daily",
            &baseline,
            ComparisonPolicy::default(),
        )
        .unwrap();
        let output = tempfile::tempdir().unwrap();
        let artifacts = write_comparison(output.path(), &comparison).unwrap();

        assert_eq!(comparison.comparison_id, replay.comparison_id);
        assert!(comparison.comparison_id.starts_with("sha256:"));
        assert_eq!(artifacts.len(), 2);
        assert!(artifacts[0].path.ends_with("/e2e-delta.json"));
        assert!(artifacts[1].path.ends_with("/e2e-summary.md"));
        for artifact in &artifacts {
            artifact.verify(output.path()).unwrap();
        }
        assert_eq!(
            artifacts,
            write_comparison(output.path(), &comparison).unwrap()
        );
    }

    #[test]
    fn excluded_or_changed_cases_cannot_silently_pass_the_gate() {
        let baseline = report("1111111111111111111111111111111111111111", false, false);
        let mut current = report("2222222222222222222222222222222222222222", false, false);
        current.scenarios[0].execution_policy.max_turns += 1;

        let comparison = compare_reports(
            "current",
            "daily",
            &current,
            "baseline",
            "daily",
            &baseline,
            ComparisonPolicy::default(),
        )
        .unwrap();

        assert!(!comparison.comparable);
        assert!(!comparison.gate_passed);
        assert_eq!(comparison.cohort.excluded_cases.len(), 2);
        assert_eq!(comparison.cohort.excluded_runs.len(), 40);
        assert!(comparison
            .reasons
            .iter()
            .any(|reason| reason.contains("case entries were excluded")));
    }

    #[test]
    fn an_infrastructure_only_side_has_no_comparable_cohort() {
        let baseline = report("1111111111111111111111111111111111111111", false, false);
        let mut current = report("2222222222222222222222222222222222222222", false, false);
        for run in &mut current.scenarios[0].runs {
            run.status = RunStatus::InfrastructureError;
        }

        let comparison = compare_reports(
            "current",
            "daily",
            &current,
            "baseline",
            "daily",
            &baseline,
            ComparisonPolicy::default(),
        )
        .unwrap();

        assert!(!comparison.comparable);
        assert!(!comparison.gate_passed);
        assert_eq!(comparison.cohort.excluded_runs.len(), 20);
        assert_eq!(comparison.cohort.excluded_cases.len(), 1);
    }

    #[test]
    fn capability_requires_explicit_budgets_after_statistical_thresholds_pass() {
        let report = report("1111111111111111111111111111111111111111", false, false);
        let advisory = capability_for_report(&report, CapabilityPolicy::default());
        let tier = advisory.tiers.first().unwrap();

        assert_eq!(tier.sample_size, 20);
        assert!(tier.statistically_eligible);
        assert!(!tier.reliable);
        assert_eq!(tier.p95_cost_usd, Some(1.0));
        assert_eq!(tier.p95_wall_time_ms, Some(100.0));
        assert!(advisory.highest_reliable_tier.is_none());
        assert_eq!(
            advisory.highest_statistically_eligible_tier,
            Some(tier.tier)
        );

        let enforced = capability_for_report(
            &report,
            CapabilityPolicy {
                maximum_p95_cost_usd: Some(1.0),
                maximum_p95_wall_time_ms: Some(100),
                ..CapabilityPolicy::default()
            },
        );
        assert_eq!(enforced.highest_reliable_tier, Some(tier.tier));
        assert!(enforced.tiers.first().unwrap().reliable);
    }

    fn report(revision: &str, regress: bool, include_infra: bool) -> E2eReport {
        let case = ScenarioCase::new(
            "coordination.2",
            1,
            7,
            serde_json::json!({"variant": "canonical"}),
            ComplexityProfile {
                parallel_branches: 2,
                artifact_count: 1,
                ..ComplexityProfile::default()
            },
            vec!["iii::state".into()],
            DeliverableContract::default(),
        )
        .unwrap();
        let mut runs = (0..20)
            .map(|index| comparable_run(index, regress && index == 0, regress))
            .collect::<Vec<_>>();
        if include_infra {
            let mut infra = comparable_run(20, false, regress);
            infra.status = RunStatus::InfrastructureError;
            infra.run_id = "infra-run".into();
            runs.push(infra);
        }
        let scenario = E2eScenarioReport::aggregate_case(
            case,
            ExecutionPolicy {
                max_turns: 10,
                max_output_tokens: Some(100),
                max_total_tokens: 1_000,
                stuck_timeout_seconds: 30,
            },
            runs,
        );
        E2eReport::new(
            ExecutionIdentity {
                execution_id: revision[..8].into(),
                lane: "daily".into(),
                started_at: "2026-08-12T00:00:00Z".into(),
                completed_at: "2026-08-12T00:01:00Z".into(),
            },
            SystemUnderTestIdentity {
                stack: StackIdentity::Source {
                    workers_repository: "iii-hq/workers".into(),
                    workers_revision: revision.into(),
                },
                engine_version: "1.0.0".into(),
                engine_revision: None,
                harness_version: "1.0.0".into(),
                e2e_repository: "iii-hq/harness-e2e".into(),
                e2e_revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                contract_hashes: BTreeMap::from([(
                    "harness::metrics".into(),
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .into(),
                )]),
            },
            crate::report::ModelArtifact {
                model: "model".into(),
                provider: "provider".into(),
                context_window: 100_000,
                max_output_tokens: 10_000,
                supports_tools: Some(true),
                supports_vision: Some(false),
            },
            None,
            None,
            None,
            vec![scenario],
        )
    }

    fn comparable_run(index: u64, failed: bool, regress: bool) -> E2eRunReport {
        let mut run = E2eRunReport::new(
            format!("run-{index}"),
            format!("attempt-{index}"),
            1,
            format!("session-{index}"),
            "prompt".into(),
        );
        run.status = if failed {
            RunStatus::HardGateFailed
        } else {
            RunStatus::Passed
        };
        run.score = Some(if failed { 0 } else { 100 });
        run.wall_time_ms = if regress { 130 } else { 100 };
        let cost = if regress { 1.30 } else { 1.0 };
        run.cost = CostReport {
            subject_usd: Some(cost),
            judge_usd: Some(0.0),
            total_usd: Some(cost),
        };
        run.dimensions = vec![
            DimensionReport {
                dimension: EvaluationDimension::Deliverable,
                passed: Some(!failed),
                signals: serde_json::json!({}),
            },
            DimensionReport {
                dimension: EvaluationDimension::StructuralIntegrity,
                passed: Some(!failed),
                signals: serde_json::json!({}),
            },
        ];
        run
    }
}
