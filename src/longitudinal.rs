use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::artifact::{self, ArtifactReference};
use crate::identity::{StackIdentity, SystemUnderTestIdentity};
use crate::report::{
    DimensionReport, E2eReport, E2eRunReport, E2eScenarioReport, EvaluationDimension, RunStatus,
};
use crate::scenarios::ComplexityTier;

const MINIMUM_ROBUSTNESS_SAMPLE: usize = 5;
const MINIMUM_TAIL_SAMPLE: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RegressionThresholds {
    pub deliverable_success_drop: f64,
    pub structural_integrity_drop: f64,
    pub technical_failure_increase: f64,
    #[serde(alias = "p95_cost_increase_ratio")]
    pub cost_increase_ratio: f64,
    #[serde(alias = "p95_wall_time_increase_ratio")]
    pub wall_time_increase_ratio: f64,
    pub p95_turns_increase_ratio: f64,
}

impl Default for RegressionThresholds {
    fn default() -> Self {
        Self {
            deliverable_success_drop: 0.03,
            structural_integrity_drop: 0.02,
            technical_failure_increase: 0.02,
            cost_increase_ratio: 0.20,
            wall_time_increase_ratio: 0.20,
            p95_turns_increase_ratio: 0.25,
        }
    }
}

impl RegressionThresholds {
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("deliverable_success_drop", self.deliverable_success_drop),
            ("structural_integrity_drop", self.structural_integrity_drop),
            (
                "technical_failure_increase",
                self.technical_failure_increase,
            ),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                bail!("{name} must be a finite rate between 0.0 and 1.0");
            }
        }
        for (name, value) in [
            ("cost_increase_ratio", self.cost_increase_ratio),
            ("wall_time_increase_ratio", self.wall_time_increase_ratio),
            ("p95_turns_increase_ratio", self.p95_turns_increase_ratio),
        ] {
            if !value.is_finite() || value <= 0.0 {
                bail!("{name} must be a finite ratio greater than zero");
            }
        }
        Ok(())
    }
}

/// Partial per-scenario replacement for [`RegressionThresholds`]: a named field
/// replaces the baseline value, an omitted field inherits it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThresholdOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliverable_success_drop: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structural_integrity_drop: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub technical_failure_increase: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_increase_ratio: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_time_increase_ratio: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95_turns_increase_ratio: Option<f64>,
}

impl ThresholdOverrides {
    pub fn apply(&self, base: RegressionThresholds) -> RegressionThresholds {
        RegressionThresholds {
            deliverable_success_drop: self
                .deliverable_success_drop
                .unwrap_or(base.deliverable_success_drop),
            structural_integrity_drop: self
                .structural_integrity_drop
                .unwrap_or(base.structural_integrity_drop),
            technical_failure_increase: self
                .technical_failure_increase
                .unwrap_or(base.technical_failure_increase),
            cost_increase_ratio: self.cost_increase_ratio.unwrap_or(base.cost_increase_ratio),
            wall_time_increase_ratio: self
                .wall_time_increase_ratio
                .unwrap_or(base.wall_time_increase_ratio),
            p95_turns_increase_ratio: self
                .p95_turns_increase_ratio
                .unwrap_or(base.p95_turns_increase_ratio),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ComparisonPolicy {
    #[serde(default)]
    pub regression: RegressionThresholds,
    /// Identifier of the reviewed baseline these thresholds came from, when one
    /// was loaded. Absent when the code-default thresholds are in effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_id: Option<String>,
    /// Per-scenario partial threshold overrides carried from the reviewed baseline.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scenario_overrides: BTreeMap<String, ThresholdOverrides>,
}

impl ComparisonPolicy {
    pub fn thresholds_for(&self, scenario_id: &str) -> RegressionThresholds {
        self.scenario_overrides
            .get(scenario_id)
            .map_or(self.regression, |overrides| {
                overrides.apply(self.regression)
            })
    }
}

/// Repo-relative location of the reviewed default comparison baseline.
pub const DEFAULT_BASELINE_RELATIVE_PATH: &str = "config/baselines/default.json";

/// A reviewed, checked-in comparison baseline: the regression thresholds a
/// comparison gates on, owned by `config/baselines/` rather than code.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BaselinePolicy {
    pub baseline_id: String,
    /// Review provenance: who or what produced these values, and when.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub review_notes: String,
    pub thresholds: RegressionThresholds,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scenario_overrides: BTreeMap<String, ThresholdOverrides>,
}

impl BaselinePolicy {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let baseline: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("decode comparison baseline {}", path.display()))?;
        baseline
            .validate()
            .with_context(|| format!("validate comparison baseline {}", path.display()))?;
        Ok(baseline)
    }

    pub fn validate(&self) -> Result<()> {
        validate_identifier(&self.baseline_id, "baseline_id")?;
        self.thresholds
            .validate()
            .context("baseline thresholds are invalid")?;
        for (scenario_id, overrides) in &self.scenario_overrides {
            validate_identifier(scenario_id, "scenario_overrides key")?;
            overrides
                .apply(self.thresholds)
                .validate()
                .with_context(|| {
                    format!("merged thresholds for scenario '{scenario_id}' are invalid")
                })?;
        }
        Ok(())
    }

    pub fn thresholds_for(&self, scenario_id: &str) -> RegressionThresholds {
        self.scenario_overrides
            .get(scenario_id)
            .map_or(self.thresholds, |overrides| {
                overrides.apply(self.thresholds)
            })
    }

    pub fn into_policy(self) -> ComparisonPolicy {
        ComparisonPolicy {
            regression: self.thresholds,
            baseline_id: Some(self.baseline_id),
            scenario_overrides: self.scenario_overrides,
        }
    }
}

/// The checked-in default baseline: next to the crate manifest in source
/// checkouts (which covers tests), otherwise relative to the working directory
/// of the running binary.
pub fn default_baseline_path() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_BASELINE_RELATIVE_PATH);
    if manifest.is_file() {
        manifest
    } else {
        PathBuf::from(DEFAULT_BASELINE_RELATIVE_PATH)
    }
}

/// Resolve the comparison policy for a compare invocation. An explicit baseline
/// path must load; without one the checked-in default baseline is used when it
/// exists, otherwise the code-default thresholds apply and the returned policy
/// carries no `baseline_id`.
pub fn load_comparison_policy(baseline: Option<&Path>) -> Result<ComparisonPolicy> {
    match baseline {
        Some(path) => Ok(BaselinePolicy::read(path)?.into_policy()),
        None => {
            let path = default_baseline_path();
            if path.is_file() {
                Ok(BaselinePolicy::read(&path)?.into_policy())
            } else {
                Ok(ComparisonPolicy::default())
            }
        }
    }
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        bail!("{label} must be a non-empty portable identifier up to 128 bytes");
    }
    Ok(())
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
    From,
    To,
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
    #[serde(default)]
    pub statistic: String,
    #[serde(default)]
    pub n_from: u32,
    #[serde(default)]
    pub n_to: u32,
    #[serde(default)]
    pub maturity: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CaseComparison {
    pub key: CaseCohortKey,
    pub complexity_tier: ComplexityTier,
    pub from_run_ids: Vec<String>,
    pub to_run_ids: Vec<String>,
    pub from: CaseMetrics,
    pub to: CaseMetrics,
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
pub struct ComparisonSummary {
    pub comparison_id: String,
    pub from_execution_id: String,
    pub to_execution_id: String,
    pub from_revision: Option<String>,
    pub to_revision: Option<String>,
    /// Reviewed baseline the regression thresholds came from; absent when the
    /// code-default thresholds gated this comparison.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_id: Option<String>,
    pub comparable: bool,
    pub gate_passed: bool,
    pub reasons: Vec<String>,
    pub cohort: CohortAudit,
    pub cases: Vec<CaseComparison>,
    pub regressions: Vec<RegressionSignal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ComparisonResponse {
    pub comparison: ComparisonSummary,
    pub artifacts: Vec<ArtifactReference>,
}

pub fn compare_reports(
    from_execution_id: &str,
    from_lane: &str,
    from: &E2eReport,
    to_execution_id: &str,
    to_lane: &str,
    to: &E2eReport,
    policy: ComparisonPolicy,
) -> Result<ComparisonSummary> {
    let from_identity = execution_identity(from_lane, from);
    let to_identity = execution_identity(to_lane, to);
    let mut reasons = identity_differences(&from_identity, &to_identity);
    let identity_matches = reasons.is_empty();
    let mut cohort = CohortAudit::default();
    let from_cases = cases_by_identity(from);
    let to_cases = cases_by_identity(to);
    let identities = from_cases
        .keys()
        .chain(to_cases.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut cases = Vec::new();

    for identity in identities {
        let from_case = from_cases.get(&identity).copied();
        let to_case = to_cases.get(&identity).copied();
        let (Some(from_case), Some(to_case)) = (from_case, to_case) else {
            let (side, scenario) = match (from_case, to_case) {
                (Some(scenario), None) => (ComparisonSide::From, scenario),
                (None, Some(scenario)) => (ComparisonSide::To, scenario),
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

        let from_key = case_key(from_identity.clone(), from_case)?;
        let to_key = case_key(to_identity.clone(), to_case)?;
        let mut case_reasons = Vec::new();
        if !identity_matches {
            case_reasons.extend(reasons.iter().cloned());
        }
        if from_key.contract_sha256 != to_key.contract_sha256 {
            case_reasons.push("materialized scenario contract fingerprint differs".into());
        }
        if !case_reasons.is_empty() {
            exclude_case(
                &mut cohort,
                ComparisonSide::From,
                from_case,
                case_reasons.clone(),
            );
            exclude_case(&mut cohort, ComparisonSide::To, to_case, case_reasons);
            continue;
        }

        let (from_runs, from_excluded) = eligible_runs(ComparisonSide::From, from_case);
        let (to_runs, to_excluded) = eligible_runs(ComparisonSide::To, to_case);
        cohort.excluded_runs.extend(from_excluded);
        cohort.excluded_runs.extend(to_excluded);
        if from_runs.is_empty() || to_runs.is_empty() {
            if from_runs.is_empty() {
                cohort.excluded_cases.push(ExcludedCase {
                    side: ComparisonSide::From,
                    scenario_id: from_case.scenario_id.clone(),
                    case_id: from_case.case_id.clone(),
                    reasons: vec!["no eligible runs remain after infrastructure exclusion".into()],
                });
            }
            if to_runs.is_empty() {
                cohort.excluded_cases.push(ExcludedCase {
                    side: ComparisonSide::To,
                    scenario_id: to_case.scenario_id.clone(),
                    case_id: to_case.case_id.clone(),
                    reasons: vec!["no eligible runs remain after infrastructure exclusion".into()],
                });
            }
            continue;
        }
        let from_metrics = case_metrics(&from_runs, from_case.runs.len());
        let to_metrics = case_metrics(&to_runs, to_case.runs.len());
        let delta = benchmark_delta(&from_metrics, &to_metrics);
        let regressions = regression_signals(
            &from_case.scenario_id,
            &from_case.case_id,
            &from_metrics,
            &to_metrics,
            &delta,
            policy.thresholds_for(&from_case.scenario_id),
        );
        cohort.included_case_ids.push(from_case.case_id.clone());
        cases.push(CaseComparison {
            key: from_key,
            complexity_tier: from_case
                .case
                .as_ref()
                .context("comparable v2 scenario has no materialized case")?
                .complexity
                .tier,
            from_run_ids: from_runs.iter().map(|run| run.run_id.clone()).collect(),
            to_run_ids: to_runs.iter().map(|run| run.run_id.clone()).collect(),
            from: from_metrics,
            to: to_metrics,
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
    let comparable = !cases.is_empty();
    let gate_passed = comparable
        && cohort.excluded_cases.is_empty()
        && regressions.iter().all(|signal| !signal.blocking);
    let mut summary = ComparisonSummary {
        comparison_id: String::new(),
        from_execution_id: from_execution_id.into(),
        to_execution_id: to_execution_id.into(),
        from_revision: revision(from),
        to_revision: revision(to),
        baseline_id: policy.baseline_id,
        comparable,
        gate_passed,
        reasons,
        cohort,
        cases,
        regressions,
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
        stack_mode: match &report.system_under_test.stack {
            StackIdentity::Source { .. } => "source",
            StackIdentity::Registry { .. } => "registry",
        }
        .into(),
        subject_provider: report.subject.provider.clone(),
        subject_model: report.subject.model.clone(),
        judge_provider: report.judge.as_ref().map(|judge| judge.provider.clone()),
        judge_model: report.judge.as_ref().map(|judge| judge.model.clone()),
        judge_protocol: report.judge_protocol.clone(),
        e2e_repository: Some(report.system_under_test.e2e_repository.clone()),
    }
}

fn identity_differences(
    from: &ExecutionCohortIdentity,
    to: &ExecutionCohortIdentity,
) -> Vec<String> {
    let mut reasons = Vec::new();
    for (name, differs) in [
        ("execution lane differs", from.lane != to.lane),
        ("stack mode differs", from.stack_mode != to.stack_mode),
        (
            "subject model identity differs",
            from.subject_provider != to.subject_provider || from.subject_model != to.subject_model,
        ),
        (
            "judge model identity differs",
            from.judge_provider != to.judge_provider || from.judge_model != to.judge_model,
        ),
        (
            "judge protocol differs",
            from.judge_protocol != to.judge_protocol,
        ),
        (
            "E2E repository identity differs",
            from.e2e_repository != to.e2e_repository,
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
                reason: "E2E infrastructure failure is outside the comparison cohort".into(),
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

fn benchmark_delta(from: &CaseMetrics, to: &CaseMetrics) -> BenchmarkDelta {
    let mut unavailable = BTreeMap::new();
    macro_rules! delta {
        ($field:literal, $from:expr, $to:expr) => {{
            let value = metric_delta($from, $to);
            if value.is_none() {
                unavailable.insert(
                    $field.into(),
                    "metric unavailable in from or to cohort".into(),
                );
            }
            value
        }};
    }
    BenchmarkDelta {
        deliverable_success_rate: delta!(
            "deliverable_success_rate",
            from.deliverable_success.as_ref().map(|value| value.rate),
            to.deliverable_success.as_ref().map(|value| value.rate)
        ),
        structural_integrity_rate: delta!(
            "structural_integrity_rate",
            from.structural_integrity.as_ref().map(|value| value.rate),
            to.structural_integrity.as_ref().map(|value| value.rate)
        ),
        technical_failure_rate: delta!(
            "technical_failure_rate",
            from.technical_failure.as_ref().map(|value| value.rate),
            to.technical_failure.as_ref().map(|value| value.rate)
        ),
        flaky_rate: delta!("flaky_rate", from.flaky_rate, to.flaky_rate),
        median_score: delta!("median_score", from.median_score, to.median_score),
        p50_cost_usd: delta!("p50_cost_usd", from.p50_cost_usd, to.p50_cost_usd),
        p95_cost_usd: delta!("p95_cost_usd", from.p95_cost_usd, to.p95_cost_usd),
        p50_wall_time_ms: delta!(
            "p50_wall_time_ms",
            from.p50_wall_time_ms,
            to.p50_wall_time_ms
        ),
        p95_wall_time_ms: delta!(
            "p95_wall_time_ms",
            from.p95_wall_time_ms,
            to.p95_wall_time_ms
        ),
        p50_turns: delta!("p50_turns", from.p50_turns, to.p50_turns),
        p95_turns: delta!("p95_turns", from.p95_turns, to.p95_turns),
        retry_rate: delta!("retry_rate", from.retry_rate, to.retry_rate),
        p50_work_amplification: delta!(
            "p50_work_amplification",
            from.p50_work_amplification,
            to.p50_work_amplification
        ),
        p95_work_amplification: delta!(
            "p95_work_amplification",
            from.p95_work_amplification,
            to.p95_work_amplification
        ),
        unavailable,
    }
}

fn regression_signals(
    scenario_id: &str,
    case_id: &str,
    from: &CaseMetrics,
    to: &CaseMetrics,
    delta: &BenchmarkDelta,
    policy: RegressionThresholds,
) -> Vec<RegressionSignal> {
    let mut signals = Vec::new();
    let n_from = from.included_runs;
    let n_to = to.included_runs;
    let maturity = evidence_maturity(n_from, n_to);
    let mut push =
        |dimension, observed, threshold, statistic: &str, blocking: bool, reason: String| {
            signals.push(RegressionSignal {
                scenario_id: scenario_id.into(),
                case_id: case_id.into(),
                dimension,
                observed_delta: observed,
                threshold,
                blocking,
                reason,
                statistic: statistic.into(),
                n_from,
                n_to,
                maturity: maturity.into(),
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
            "rate",
            true,
            "deliverable success rate fell beyond policy".into(),
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
            "rate",
            true,
            "structural integrity rate fell beyond policy".into(),
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
            "rate",
            true,
            "technical failure rate grew beyond policy".into(),
        );
    }
    for (dimension, p95, median, threshold, label) in [
        (
            RegressionDimension::Cost,
            delta.p95_cost_usd,
            delta.p50_cost_usd,
            policy.cost_increase_ratio,
            "cost",
        ),
        (
            RegressionDimension::WallTime,
            delta.p95_wall_time_ms,
            delta.p50_wall_time_ms,
            policy.wall_time_increase_ratio,
            "wall time",
        ),
    ] {
        let Some((value, statistic)) = p95
            .map(|value| (value, "p95"))
            .or_else(|| median.map(|value| (value, "median")))
        else {
            continue;
        };
        if value.relative_ratio.is_some_and(|ratio| ratio > threshold) {
            let blocking = statistic == "p95"
                && n_from >= MINIMUM_TAIL_SAMPLE as u32
                && n_to >= MINIMUM_TAIL_SAMPLE as u32;
            push(
                dimension,
                value.relative_ratio.expect("checked"),
                threshold,
                statistic,
                blocking,
                if blocking {
                    format!("relative p95 {label} grew beyond policy")
                } else {
                    format!("relative median {label} grew beyond policy (advisory until both sides have {MINIMUM_TAIL_SAMPLE} samples)")
                },
            );
        }
    }
    if delta
        .p95_turns
        .and_then(|value| value.relative_ratio)
        .is_some_and(|ratio| ratio > policy.p95_turns_increase_ratio)
    {
        push(
            RegressionDimension::Turns,
            delta
                .p95_turns
                .and_then(|value| value.relative_ratio)
                .expect("checked"),
            policy.p95_turns_increase_ratio,
            "p95",
            n_from >= MINIMUM_TAIL_SAMPLE as u32 && n_to >= MINIMUM_TAIL_SAMPLE as u32,
            "p95 turns grew beyond policy".into(),
        );
    }
    signals
}

fn evidence_maturity(n_from: u32, n_to: u32) -> &'static str {
    let sample = n_from.min(n_to);
    if sample >= MINIMUM_TAIL_SAMPLE as u32 {
        "validated"
    } else if sample >= MINIMUM_ROBUSTNESS_SAMPLE as u32 {
        "repeatable"
    } else {
        "directional"
    }
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

fn metric_delta(from: Option<f64>, to: Option<f64>) -> Option<DeltaValue> {
    from.zip(to).map(|(from, to)| DeltaValue {
        absolute: to - from,
        relative_ratio: (from != 0.0).then(|| (to - from) / from.abs()),
    })
}

fn revision(report: &E2eReport) -> Option<String> {
    system_revision(&report.system_under_test)
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
    let status = if !comparison.comparable || !comparison.cohort.excluded_cases.is_empty() {
        "INCOMPLETE"
    } else if comparison.gate_passed {
        "PASS"
    } else {
        "FAIL"
    };
    let mut output = format!(
        "# Harness E2E comparison: {status}\n\nComparison: `{}`  \nFrom: `{}` at `{}`  \nTo: `{}` at `{}`  \nBaseline: `{}`  \nComparable cases: {}  \nExcluded cases: {}  \nExcluded runs: {}\n\n",
        comparison.comparison_id,
        comparison.from_execution_id,
        comparison.from_revision.as_deref().unwrap_or("unknown"),
        comparison.to_execution_id,
        comparison.to_revision.as_deref().unwrap_or("unknown"),
        comparison
            .baseline_id
            .as_deref()
            .unwrap_or("code defaults (no reviewed baseline)"),
        comparison.cohort.included_case_ids.len(),
        comparison.cohort.excluded_cases.len(),
        comparison.cohort.excluded_runs.len(),
    );
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
        let from = CaseMetrics {
            included_runs: 20,
            ..CaseMetrics::default()
        };
        let to = from.clone();
        let signals = regression_signals(
            "scenario",
            "case",
            &from,
            &to,
            &delta,
            RegressionThresholds::default(),
        );
        assert_eq!(signals.len(), 2);
        assert!(signals
            .iter()
            .find(|signal| signal.dimension == RegressionDimension::Deliverable)
            .is_some_and(|signal| signal.blocking));
        assert!(signals
            .iter()
            .find(|signal| signal.dimension == RegressionDimension::WallTime)
            .is_some_and(|signal| signal.blocking));
        assert!(signals
            .iter()
            .any(|signal| signal.dimension == RegressionDimension::Deliverable));
        assert!(signals
            .iter()
            .any(|signal| signal.dimension == RegressionDimension::WallTime));
    }

    #[test]
    fn local_comparisons_fall_back_to_relative_medians_before_p95_is_available() {
        let delta = BenchmarkDelta {
            p50_cost_usd: Some(DeltaValue {
                absolute: 0.30,
                relative_ratio: Some(0.30),
            }),
            p50_wall_time_ms: Some(DeltaValue {
                absolute: 25.0,
                relative_ratio: Some(0.25),
            }),
            ..BenchmarkDelta::default()
        };

        let from = CaseMetrics {
            included_runs: 1,
            ..CaseMetrics::default()
        };
        let to = from.clone();
        let signals = regression_signals(
            "scenario",
            "case",
            &from,
            &to,
            &delta,
            RegressionThresholds::default(),
        );

        assert_eq!(signals.len(), 2);
        assert!(signals.iter().all(|signal| !signal.blocking));
        assert!(signals
            .iter()
            .all(|signal| signal.reason.contains("relative median")));
    }

    #[test]
    fn comparisons_allow_revision_changes_but_audit_cases_and_runs() {
        let to = report("1111111111111111111111111111111111111111", true, true);
        let from = report("2222222222222222222222222222222222222222", false, false);
        let comparison = compare_reports(
            "from",
            "daily",
            &from,
            "to",
            "daily",
            &to,
            ComparisonPolicy::default(),
        )
        .unwrap();

        assert!(comparison.comparable);
        assert_eq!(comparison.cases.len(), 1);
        assert_eq!(comparison.cohort.excluded_runs.len(), 1);
        assert_eq!(
            comparison.from_revision.as_deref(),
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
        assert!(comparison
            .regressions
            .iter()
            .any(|signal| signal.dimension == RegressionDimension::WallTime));
        assert!(!comparison.gate_passed);
    }

    #[test]
    fn comparison_artifacts_are_immutable_and_verifiable() {
        let to = report("1111111111111111111111111111111111111111", false, false);
        let from = report("2222222222222222222222222222222222222222", false, false);
        let comparison = compare_reports(
            "from",
            "daily",
            &from,
            "to",
            "daily",
            &to,
            ComparisonPolicy::default(),
        )
        .unwrap();
        let replay = compare_reports(
            "from",
            "daily",
            &from,
            "to",
            "daily",
            &to,
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
        let to = report("1111111111111111111111111111111111111111", false, false);
        let mut from = report("2222222222222222222222222222222222222222", false, false);
        from.scenarios[0].execution_policy.max_turns += 1;

        let comparison = compare_reports(
            "from",
            "daily",
            &from,
            "to",
            "daily",
            &to,
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
        let to = report("1111111111111111111111111111111111111111", false, false);
        let mut from = report("2222222222222222222222222222222222222222", false, false);
        for run in &mut from.scenarios[0].runs {
            run.status = RunStatus::InfrastructureError;
        }

        let comparison = compare_reports(
            "from",
            "daily",
            &from,
            "to",
            "daily",
            &to,
            ComparisonPolicy::default(),
        )
        .unwrap();

        assert!(!comparison.comparable);
        assert!(!comparison.gate_passed);
        assert_eq!(comparison.cohort.excluded_runs.len(), 20);
        assert_eq!(comparison.cohort.excluded_cases.len(), 1);
    }

    fn checked_in_baseline() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_BASELINE_RELATIVE_PATH)
    }

    #[test]
    fn checked_in_default_baseline_matches_code_defaults() {
        let path = checked_in_baseline();
        let baseline = BaselinePolicy::read(&path).unwrap();
        assert_eq!(baseline.baseline_id, "default");
        assert!(!baseline.review_notes.is_empty());
        assert_eq!(baseline.thresholds, RegressionThresholds::default());
        assert!(baseline.scenario_overrides.is_empty());
        assert_eq!(default_baseline_path(), path);

        let policy = load_comparison_policy(None).unwrap();
        assert_eq!(policy.baseline_id.as_deref(), Some("default"));
        assert_eq!(policy.regression, RegressionThresholds::default());
        assert!(policy.scenario_overrides.is_empty());
    }

    #[test]
    fn scenario_overrides_replace_only_named_fields() {
        let baseline = BaselinePolicy {
            baseline_id: "default".into(),
            scenario_overrides: BTreeMap::from([(
                "todo_worker_simple".into(),
                ThresholdOverrides {
                    cost_increase_ratio: Some(0.50),
                    ..ThresholdOverrides::default()
                },
            )]),
            ..BaselinePolicy::default()
        };
        baseline.validate().unwrap();

        let merged = baseline.thresholds_for("todo_worker_simple");
        assert_eq!(merged.cost_increase_ratio, 0.50);
        assert_eq!(
            merged.deliverable_success_drop,
            baseline.thresholds.deliverable_success_drop
        );
        assert_eq!(
            merged.wall_time_increase_ratio,
            baseline.thresholds.wall_time_increase_ratio
        );
        assert_eq!(
            baseline.thresholds_for("some_other_scenario"),
            baseline.thresholds
        );

        let policy = baseline.into_policy();
        assert_eq!(policy.baseline_id.as_deref(), Some("default"));
        assert_eq!(
            policy
                .thresholds_for("todo_worker_simple")
                .cost_increase_ratio,
            0.50
        );
        assert_eq!(
            policy.thresholds_for("some_other_scenario"),
            policy.regression
        );
    }

    #[test]
    fn baseline_rejects_unknown_fields_bad_values_and_bad_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("baseline.json");
        let mut document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(checked_in_baseline()).unwrap()).unwrap();
        document["surprise"] = serde_json::json!(true);
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        let error = BaselinePolicy::read(&path).unwrap_err();
        assert!(format!("{error:#}").contains("decode comparison baseline"));

        let valid = BaselinePolicy {
            baseline_id: "default".into(),
            ..BaselinePolicy::default()
        };
        valid.validate().unwrap();

        let bad_ratio = BaselinePolicy {
            thresholds: RegressionThresholds {
                cost_increase_ratio: 0.0,
                ..RegressionThresholds::default()
            },
            ..valid.clone()
        };
        assert!(bad_ratio.validate().is_err());

        let bad_rate = BaselinePolicy {
            thresholds: RegressionThresholds {
                deliverable_success_drop: -0.01,
                ..RegressionThresholds::default()
            },
            ..valid.clone()
        };
        assert!(bad_rate.validate().is_err());

        let empty_key = BaselinePolicy {
            scenario_overrides: BTreeMap::from([(String::new(), ThresholdOverrides::default())]),
            ..valid.clone()
        };
        assert!(empty_key.validate().is_err());

        let bad_merge = BaselinePolicy {
            scenario_overrides: BTreeMap::from([(
                "todo_worker_simple".into(),
                ThresholdOverrides {
                    wall_time_increase_ratio: Some(f64::NAN),
                    ..ThresholdOverrides::default()
                },
            )]),
            ..valid.clone()
        };
        assert!(bad_merge.validate().is_err());

        let bad_id = BaselinePolicy {
            baseline_id: "not portable!".into(),
            ..valid
        };
        assert!(bad_id.validate().is_err());
    }

    #[test]
    fn scenario_overrides_apply_per_case_in_compare_reports() {
        let to = report("1111111111111111111111111111111111111111", true, false);
        let from = report("2222222222222222222222222222222222222222", false, false);
        let policy = ComparisonPolicy {
            baseline_id: Some("default".into()),
            scenario_overrides: BTreeMap::from([(
                "todo_worker_simple".into(),
                ThresholdOverrides {
                    deliverable_success_drop: Some(0.10),
                    structural_integrity_drop: Some(0.10),
                    cost_increase_ratio: Some(0.50),
                    wall_time_increase_ratio: Some(0.50),
                    ..ThresholdOverrides::default()
                },
            )]),
            ..ComparisonPolicy::default()
        };
        let comparison =
            compare_reports("from", "daily", &from, "to", "daily", &to, policy).unwrap();

        assert_eq!(comparison.baseline_id.as_deref(), Some("default"));
        assert!(comparison.comparable);
        assert!(comparison.regressions.is_empty());
        assert!(comparison.gate_passed);

        let strict = compare_reports(
            "from",
            "daily",
            &from,
            "to",
            "daily",
            &to,
            ComparisonPolicy::default(),
        )
        .unwrap();
        assert!(strict.baseline_id.is_none());
        assert!(!strict.regressions.is_empty());
        assert!(!strict.gate_passed);
    }

    fn report(revision: &str, regress: bool, include_infra: bool) -> E2eReport {
        let case = ScenarioCase::new(
            "todo_worker_simple",
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
                max_total_tokens: Some(1_000),
                stuck_timeout_seconds: 30,
                max_validation_retries: None,
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
