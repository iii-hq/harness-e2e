use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::assessment_projection::{
    analyzer_profile_sha256, assessment_profile_sha256, contracts_for_scenario, summarize,
    AssessmentSummary,
};
use super::presenter::{execution_summary, MAX_EXECUTIONS};
use super::store::{load_runs, StoredRun};
use crate::artifact;
use crate::assessment::{
    AssessmentKind, AssessmentPolicy, AssessmentSource, RunAssessmentContract,
};
use crate::identity::StackIdentity;
use crate::report::{E2eRunReport, E2eScenarioReport, EvaluationDimension, RunStatus};
use crate::scenarios::{
    stable_seed, ComplexityClassification, ExecutionPolicy, ScenarioCharacterization, ScenarioId,
    ScenarioSpec,
};

const DEFAULT_PAGE_SIZE: u16 = 25;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub(super) struct EvaluatedVersionsRequest {
    #[serde(default)]
    pub cohort_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub(super) struct TestsListRequest {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub cohort_id: Option<String>,
    #[serde(default)]
    pub from_version_id: Option<String>,
    #[serde(default)]
    pub to_version_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(super) struct TestVersionGetRequest {
    pub test_id: String,
    pub test_version: u32,
    pub cohort_id: String,
    pub from_version_id: String,
    pub to_version_id: String,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub(super) struct TestHistoryRequest {
    #[serde(default)]
    pub test_id: String,
    #[serde(default)]
    pub test_version: Option<u32>,
    #[serde(default)]
    pub case_id: Option<String>,
    #[serde(default)]
    pub subject_provider: Option<String>,
    #[serde(default)]
    pub subject_model: Option<String>,
    #[serde(default)]
    pub judge_provider: Option<String>,
    #[serde(default)]
    pub judge_model: Option<String>,
    #[serde(default)]
    pub system_version_id: Option<String>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct CohortDescriptor {
    pub id: String,
    pub lane: String,
    pub subject_provider: String,
    pub subject_model: String,
    pub judge_provider: Option<String>,
    pub judge_model: Option<String>,
    pub judge_protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct EvaluatedVersionDescriptor {
    pub id: String,
    pub cohort_id: String,
    pub label: String,
    pub stack_mode: String,
    pub completed_at: String,
    pub execution_count: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct EvaluatedVersionsResponse {
    pub revision: String,
    pub cohorts: Vec<CohortDescriptor>,
    pub versions: Vec<EvaluatedVersionDescriptor>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct VersionDescriptor {
    pub version: u32,
    pub execution_count: usize,
    pub run_count: usize,
    pub last_seen: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct OutcomeCounts {
    pub passed: usize,
    pub hard_gate_failed: usize,
    pub technical_failed: usize,
    pub infra_failed: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct MetricSamples {
    pub score: usize,
    pub cost_usd: usize,
    pub tokens: usize,
    pub duration_seconds: usize,
    pub turns: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct TestSideSummary {
    pub evaluated_version_id: String,
    pub execution_count: usize,
    pub total_runs: usize,
    pub scored_runs: usize,
    pub case_count: usize,
    pub median_score: Option<f64>,
    pub pass_rate: Option<f64>,
    pub median_cost_usd: Option<f64>,
    pub median_tokens: Option<f64>,
    pub median_duration_seconds: Option<f64>,
    pub outcomes: OutcomeCounts,
    pub samples: MetricSamples,
    pub assessment_summary: AssessmentSummary,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub(super) struct TestDelta {
    pub score: Option<f64>,
    pub cost_usd: Option<f64>,
    pub tokens: Option<f64>,
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct TestObservation {
    pub execution_id: String,
    pub evaluated_version_id: Option<String>,
    pub cohort_id: String,
    pub completed_at: String,
    pub case_id: String,
    pub contract_sha256: String,
    pub assessment_profile_sha256: String,
    pub analyzer_profile_sha256: String,
    pub status: String,
    pub median_score: Option<f64>,
    pub run_count: usize,
    pub scored_runs: usize,
    pub assessment_summary: AssessmentSummary,
    pub scenario_version: u32,
    pub seed: Option<u64>,
    pub system_version_id: Option<String>,
    pub system_label: String,
    pub stack_mode: String,
    pub harness_revision: Option<String>,
    pub system_revision: Option<String>,
    pub engine_revision: Option<String>,
    pub subject_provider: String,
    pub subject_model: String,
    pub judge_provider: Option<String>,
    pub judge_model: Option<String>,
    pub judge_protocol: Option<String>,
    pub median_cost_usd: Option<f64>,
    pub median_tokens: Option<f64>,
    pub median_duration_seconds: Option<f64>,
    pub median_function_calls: Option<f64>,
    pub median_function_call_errors: Option<f64>,
    pub median_turns: Option<f64>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct HistorySeries {
    pub id: String,
    pub case_id: String,
    pub scenario_version: u32,
    pub seed: Option<u64>,
    pub contract_sha256: String,
    pub assessment_profile_sha256: String,
    pub analyzer_profile_sha256: String,
    pub system_version_id: Option<String>,
    pub system_label: String,
    pub stack_mode: String,
    pub harness_revision: Option<String>,
    pub system_revision: Option<String>,
    pub engine_revision: Option<String>,
    pub subject_provider: String,
    pub subject_model: String,
    pub judge_provider: Option<String>,
    pub judge_model: Option<String>,
    pub judge_protocol: Option<String>,
    pub cohort_id: String,
    pub execution_count: usize,
    pub run_count: usize,
    pub median_score: Option<f64>,
    pub median_cost_usd: Option<f64>,
    pub median_tokens: Option<f64>,
    pub median_duration_seconds: Option<f64>,
    pub median_function_calls: Option<f64>,
    pub median_function_call_errors: Option<f64>,
    pub median_turns: Option<f64>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct HistorySystem {
    pub id: String,
    pub label: String,
}

/// Models are exposed as provider groups so a model name is never ambiguous
/// when two providers offer the same model identifier.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct HistoryModelGroup {
    pub provider: String,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct TestHistoryResponse {
    pub test_id: String,
    /// The version whose executions are shown (the latest with evidence by default).
    pub test_version: u32,
    /// The contract's current version, which may have no executions yet.
    pub current_version: Option<u32>,
    pub available_versions: Vec<VersionDescriptor>,
    pub cases: Vec<String>,
    pub subjects: Vec<String>,
    pub subject_models: Vec<HistoryModelGroup>,
    pub judge_models: Vec<HistoryModelGroup>,
    pub systems: Vec<HistorySystem>,
    pub series: Vec<HistorySeries>,
    pub observations: Vec<TestObservation>,
    pub total: usize,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct TestVersionResult {
    pub test_id: String,
    pub test_version: u32,
    pub compatibility: String,
    pub compatibility_reasons: Vec<String>,
    pub from: Option<TestSideSummary>,
    pub to: Option<TestSideSummary>,
    pub delta: TestDelta,
    pub from_observations: Vec<TestObservation>,
    pub to_observations: Vec<TestObservation>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct TestCatalogRow {
    pub test_id: String,
    pub lifecycle: String,
    pub current_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complexity: Option<ComplexityClassification>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub characterization: Option<ScenarioCharacterization>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calibration: Option<CalibrationProjection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<TestSpecProjection>,
    pub available_versions: Vec<VersionDescriptor>,
    pub selected_version: Option<u32>,
    pub result: Option<TestVersionResult>,
}

/// The scenario definition as a reader needs it: what the subject is asked to
/// do, how the result is scored, and the limits it runs under. Projected from
/// the materialized `ScenarioSpec` of the test's current version, so it always
/// describes the contract the dashboard is showing.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct TestSpecProjection {
    /// Editorial description; absent until the scenario defines a `SUMMARY`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The prompt as materialized for a representative run: run-scoped paths
    /// resolve against a catalog namespace, never a real execution.
    pub prompt: String,
    pub criteria: Vec<TestCriterionProjection>,
    pub execution: ExecutionPolicy,
    pub denied_functions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct TestCriterionProjection {
    pub id: String,
    pub weight: u8,
    pub description: String,
    pub kind: AssessmentKind,
    pub policy: AssessmentPolicy,
    pub dimension: EvaluationDimension,
    pub source: AssessmentSource,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct CalibrationProjection {
    pub maturity: String,
    pub compatible_sample_count: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct TestsListResponse {
    pub revision: String,
    pub rows: Vec<TestCatalogRow>,
    pub total: usize,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone)]
struct RunMetrics {
    score: Option<f64>,
    cost_usd: Option<f64>,
    tokens: Option<f64>,
    duration_seconds: Option<f64>,
    function_calls: Option<f64>,
    function_call_errors: Option<f64>,
    turns: Option<f64>,
    status: RunStatus,
    assessment: RunAssessmentContract,
}

#[derive(Debug, Clone)]
struct Observation {
    execution_id: String,
    evaluated_version_id: Option<String>,
    cohort_id: String,
    completed_at: String,
    case_id: String,
    contract_sha256: String,
    assessment_profile_sha256: String,
    analyzer_profile_sha256: String,
    status: String,
    scenario_version: u32,
    seed: Option<u64>,
    system_label: String,
    stack_mode: String,
    harness_revision: Option<String>,
    system_revision: Option<String>,
    engine_revision: Option<String>,
    subject_provider: String,
    subject_model: String,
    judge_provider: Option<String>,
    judge_model: Option<String>,
    judge_protocol: Option<String>,
    runs: Vec<RunMetrics>,
}

#[derive(Debug, Clone, Default)]
struct TestVersionEntry {
    observations: Vec<Observation>,
}

#[derive(Debug, Clone, Default)]
struct TestEntry {
    current_version: Option<u32>,
    current_classification: Option<ComplexityClassification>,
    current_characterization: Option<ScenarioCharacterization>,
    current_spec: Option<TestSpecProjection>,
    current_reference_verified: bool,
    versions: BTreeMap<u32, TestVersionEntry>,
}

#[derive(Debug, Clone)]
pub(super) struct DashboardReadModel {
    pub(super) revision: String,
    pub(super) summaries: Vec<Value>,
    cohorts: BTreeMap<String, CohortDescriptor>,
    evaluated_versions: BTreeMap<(String, String), EvaluatedVersionDescriptor>,
    tests: BTreeMap<String, TestEntry>,
}

impl DashboardReadModel {
    pub(super) fn load(runs_dir: &Path) -> Result<Self> {
        let mut stored = load_runs(runs_dir)?;
        stored.sort_by(|left, right| {
            right
                .metadata
                .started_at
                .cmp(&left.metadata.started_at)
                .then_with(|| right.metadata.id.cmp(&left.metadata.id))
        });
        stored.truncate(MAX_EXECUTIONS);

        let revision = artifact::sha256_value(
            &stored
                .iter()
                .map(|run| {
                    json!({
                        "id": run.metadata.id,
                        "completed_at": run.metadata.completed_at,
                        "status": run.metadata.status,
                    })
                })
                .collect::<Vec<_>>(),
        )?;
        let summaries = stored
            .iter()
            .map(|run| execution_summary(&run.metadata, run.report.as_ref()))
            .collect::<Result<Vec<_>>>()?;
        let mut model = Self {
            revision,
            summaries,
            cohorts: BTreeMap::new(),
            evaluated_versions: BTreeMap::new(),
            tests: current_tests()?,
        };
        for run in &stored {
            model.index_run(run)?;
        }
        Ok(model)
    }

    fn index_run(&mut self, stored: &StoredRun) -> Result<()> {
        let Some(report) = stored.report.as_ref() else {
            return Ok(());
        };
        let lane = report.execution.lane.clone();
        let cohort = CohortDescriptor {
            id: String::new(),
            lane,
            subject_provider: report.subject.provider.clone(),
            subject_model: report.subject.model.clone(),
            judge_provider: report.judge.as_ref().map(|judge| judge.provider.clone()),
            judge_model: report.judge.as_ref().map(|judge| judge.model.clone()),
            judge_protocol: report.judge_protocol.clone(),
        };
        let cohort_id = artifact::sha256_value(&json!({
            "lane": cohort.lane,
            "subject_provider": cohort.subject_provider,
            "subject_model": cohort.subject_model,
            "judge_provider": cohort.judge_provider,
            "judge_model": cohort.judge_model,
            "judge_protocol": cohort.judge_protocol,
        }))?;
        self.cohorts
            .entry(cohort_id.clone())
            .or_insert_with(|| CohortDescriptor {
                id: cohort_id.clone(),
                ..cohort
            });

        let evaluated = evaluated_version(report, &cohort_id, &stored.metadata.completed_at)?;
        if let Some(descriptor) = evaluated.as_ref() {
            self.evaluated_versions
                .entry((cohort_id.clone(), descriptor.id.clone()))
                .and_modify(|existing| {
                    existing.execution_count += 1;
                    if descriptor.completed_at > existing.completed_at {
                        existing.completed_at.clone_from(&descriptor.completed_at);
                    }
                })
                .or_insert_with(|| descriptor.clone());
        }

        for scenario in &report.scenarios {
            let test = self.tests.entry(scenario.scenario_id.clone()).or_default();
            let version = test.versions.entry(scenario.scenario_version).or_default();
            let contract_sha256 = scenario_contract_sha256(scenario)?;
            let contracts = contracts_for_scenario(report, scenario);
            let assessment_profile_sha256 =
                assessment_profile_sha256(scenario.scenario_version, &contracts)?;
            let analyzer_profile_sha256 = analyzer_profile_sha256(&contracts)?;
            let contract_by_run = contracts
                .iter()
                .map(|contract| {
                    (
                        (contract.run_id.as_str(), contract.attempt_id.as_str()),
                        *contract,
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let runs = scenario
                .runs
                .iter()
                .map(|run| {
                    let assessment = contract_by_run
                        .get(&(run.run_id.as_str(), run.attempt_id.as_str()))
                        .with_context(|| {
                            format!(
                                "scenario '{}' run '{}:{}' has no assessment projection",
                                scenario.scenario_id, run.run_id, run.attempt_id
                            )
                        })?;
                    Ok(run_metrics(run, assessment))
                })
                .collect::<Result<Vec<_>>>()?;
            let (system_label, stack_mode, system_revision, engine_revision) = evaluated
                .as_ref()
                .map(|value| {
                    (
                        value.label.clone(),
                        value.stack_mode.clone(),
                        system_revision(report),
                        report.system_under_test.engine_revision.clone(),
                    )
                })
                .unwrap_or_else(|| {
                    (
                        "Unknown system".into(),
                        "unknown".into(),
                        system_revision(report),
                        report.system_under_test.engine_revision.clone(),
                    )
                });
            version.observations.push(Observation {
                execution_id: stored.metadata.id.clone(),
                evaluated_version_id: evaluated.as_ref().map(|value| value.id.clone()),
                cohort_id: cohort_id.clone(),
                completed_at: stored.metadata.completed_at.clone(),
                case_id: scenario.case_id.clone(),
                contract_sha256,
                assessment_profile_sha256,
                analyzer_profile_sha256,
                status: scenario_status(scenario).into(),
                scenario_version: scenario.scenario_version,
                seed: scenario.case.as_ref().map(|case| case.seed),
                system_label,
                stack_mode,
                harness_revision: Some(report.system_under_test.e2e_revision.clone()),
                system_revision,
                engine_revision,
                subject_provider: report.subject.provider.clone(),
                subject_model: report.subject.model.clone(),
                judge_provider: report.judge.as_ref().map(|judge| judge.provider.clone()),
                judge_model: report.judge.as_ref().map(|judge| judge.model.clone()),
                judge_protocol: report.judge_protocol.clone(),
                runs,
            });
        }
        Ok(())
    }

    pub(super) fn evaluated_versions(
        &self,
        request: EvaluatedVersionsRequest,
    ) -> EvaluatedVersionsResponse {
        let cohorts = self
            .cohorts
            .values()
            .filter(|cohort| {
                request
                    .cohort_id
                    .as_deref()
                    .is_none_or(|id| cohort.id == id)
            })
            .cloned()
            .collect();
        let mut versions = self
            .evaluated_versions
            .values()
            .filter(|version| {
                request
                    .cohort_id
                    .as_deref()
                    .is_none_or(|id| version.cohort_id == id)
            })
            .cloned()
            .collect::<Vec<_>>();
        versions.sort_by(|left, right| {
            right
                .completed_at
                .cmp(&left.completed_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        EvaluatedVersionsResponse {
            revision: self.revision.clone(),
            cohorts,
            versions,
        }
    }

    pub(super) fn tests_list(&self, request: TestsListRequest) -> Result<TestsListResponse> {
        let limit = request.limit.unwrap_or(DEFAULT_PAGE_SIZE);
        if limit == 0 || usize::from(limit) > MAX_EXECUTIONS {
            bail!("test list limit must be between 1 and {MAX_EXECUTIONS}");
        }
        if let (Some(cohort), Some(from), Some(to)) = (
            request.cohort_id.as_deref(),
            request.from_version_id.as_deref(),
            request.to_version_id.as_deref(),
        ) {
            if from == to {
                bail!("comparison requires two distinct evaluated versions");
            }
            self.validate_comparison_context(cohort, from, to)?;
        }
        let offset = parse_cursor(request.cursor.as_deref(), &self.revision)?;
        let query = request
            .query
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let mut rows = self
            .tests
            .iter()
            .filter(|(test_id, _)| query.is_empty() || test_id.to_lowercase().contains(&query))
            .map(|(test_id, entry)| self.catalog_row(test_id, entry, &request))
            .collect::<Result<Vec<_>>>()?;
        rows.sort_by(|left, right| left.test_id.cmp(&right.test_id));
        let total = rows.len();
        let rows = rows
            .into_iter()
            .skip(offset)
            .take(usize::from(limit))
            .collect::<Vec<_>>();
        let end = offset.saturating_add(rows.len());
        Ok(TestsListResponse {
            revision: self.revision.clone(),
            rows,
            total,
            next_cursor: (end < total).then(|| format!("{}:{end}", self.revision)),
        })
    }

    fn catalog_row(
        &self,
        test_id: &str,
        entry: &TestEntry,
        request: &TestsListRequest,
    ) -> Result<TestCatalogRow> {
        let mut available_versions = entry
            .versions
            .iter()
            .map(|(version, value)| {
                version_descriptor(*version, value, request.cohort_id.as_deref())
            })
            .filter(|descriptor| {
                descriptor.execution_count > 0 || entry.current_version == Some(descriptor.version)
            })
            .collect::<Vec<_>>();
        available_versions.sort_by_key(|entry| std::cmp::Reverse(entry.version));
        let selected_version = select_version(
            entry,
            request.cohort_id.as_deref(),
            request.from_version_id.as_deref(),
            request.to_version_id.as_deref(),
        );
        let result = match (
            selected_version,
            request.cohort_id.as_deref(),
            request.from_version_id.as_deref(),
            request.to_version_id.as_deref(),
        ) {
            (Some(test_version), Some(cohort_id), Some(from), Some(to)) if from != to => {
                let mut result =
                    self.test_version_result(test_id, test_version, cohort_id, from, to)?;
                // The catalog carries the selected version's compact result so the first
                // render is one request. Evidence remains lazy through test-version-get.
                result.from_observations.clear();
                result.to_observations.clear();
                Some(result)
            }
            _ => None,
        };
        let observed = entry
            .versions
            .values()
            .any(|version| !version.observations.is_empty());
        let lifecycle = if !observed {
            "never_run"
        } else if entry.current_version.is_none() {
            "retired"
        } else {
            "active"
        };
        Ok(TestCatalogRow {
            test_id: test_id.into(),
            lifecycle: lifecycle.into(),
            current_version: entry.current_version,
            complexity: entry.current_classification.clone(),
            characterization: entry.current_characterization,
            calibration: entry.current_version.and_then(|version| {
                entry.versions.get(&version).map(|version| {
                    calibration_projection(version, entry.current_reference_verified)
                })
            }),
            spec: entry.current_spec.clone(),
            available_versions,
            selected_version,
            result,
        })
    }

    pub(super) fn test_version_get(
        &self,
        request: TestVersionGetRequest,
    ) -> Result<TestVersionResult> {
        if request.test_id.trim().is_empty() || request.test_version == 0 {
            bail!("test id and version are required");
        }
        if request.from_version_id == request.to_version_id {
            bail!("comparison requires two distinct evaluated versions");
        }
        self.validate_comparison_context(
            &request.cohort_id,
            &request.from_version_id,
            &request.to_version_id,
        )?;
        self.test_version_result(
            &request.test_id,
            request.test_version,
            &request.cohort_id,
            &request.from_version_id,
            &request.to_version_id,
        )
    }

    pub(super) fn test_history(&self, request: TestHistoryRequest) -> Result<TestHistoryResponse> {
        if request.test_id.trim().is_empty() {
            bail!("test id is required");
        }
        let entry = self
            .tests
            .get(&request.test_id)
            .with_context(|| format!("unknown test '{}'", request.test_id))?;
        let test_version = request
            .test_version
            .or_else(|| {
                entry.current_version.filter(|version| {
                    entry
                        .versions
                        .get(version)
                        .is_some_and(|value| !value.observations.is_empty())
                })
            })
            .or_else(|| {
                entry
                    .versions
                    .iter()
                    .rev()
                    .find(|(_, value)| !value.observations.is_empty())
                    .map(|(version, _)| *version)
            })
            .or(entry.current_version)
            .or_else(|| entry.versions.keys().max().copied())
            .context("test has no version")?;
        let version = entry.versions.get(&test_version).with_context(|| {
            format!("unknown test '{}' version {test_version}", request.test_id)
        })?;
        let mut observations = version
            .observations
            .iter()
            .filter(|observation| history_matches(observation, &request))
            .collect::<Vec<_>>();
        observations.sort_by(|left, right| {
            right
                .completed_at
                .cmp(&left.completed_at)
                .then_with(|| right.execution_id.cmp(&left.execution_id))
        });
        let total = observations.len();
        let offset = parse_history_cursor(request.cursor.as_deref(), &self.revision)?;
        let limit = request.limit.unwrap_or(DEFAULT_PAGE_SIZE);
        if limit == 0 || usize::from(limit) > MAX_EXECUTIONS {
            bail!("history list limit must be between 1 and {MAX_EXECUTIONS}");
        }
        let page = observations
            .iter()
            .skip(offset)
            .take(usize::from(limit))
            .map(public_observation)
            .collect::<Vec<_>>();
        let end = offset.saturating_add(page.len());
        let mut series = BTreeMap::<String, Vec<&Observation>>::new();
        for observation in observations {
            let key = history_series_key(observation);
            series.entry(key).or_default().push(observation);
        }
        let series = series
            .into_iter()
            .map(|(id, observations)| history_series(id, &observations))
            .collect();
        let mut cases = version
            .observations
            .iter()
            .map(|observation| observation.case_id.clone())
            .collect::<BTreeSet<_>>();
        let mut subjects = BTreeSet::new();
        let mut subject_models = BTreeMap::<String, BTreeSet<String>>::new();
        let mut judge_models = BTreeMap::<String, BTreeSet<String>>::new();
        let mut systems = BTreeMap::new();
        for observation in &version.observations {
            if history_matches(observation, &request) {
                cases.insert(observation.case_id.clone());
                subjects.insert(format!(
                    "{}/{}",
                    observation.subject_provider, observation.subject_model
                ));
                subject_models
                    .entry(observation.subject_provider.clone())
                    .or_default()
                    .insert(observation.subject_model.clone());
                if let (Some(provider), Some(model)) = (
                    observation.judge_provider.as_ref(),
                    observation.judge_model.as_ref(),
                ) {
                    judge_models
                        .entry(provider.clone())
                        .or_default()
                        .insert(model.clone());
                }
                let id = observation
                    .evaluated_version_id
                    .clone()
                    .unwrap_or_else(|| observation.system_label.clone());
                systems
                    .entry(id)
                    .or_insert_with(|| observation.system_label.clone());
            }
        }
        Ok(TestHistoryResponse {
            test_id: request.test_id,
            test_version,
            current_version: entry.current_version,
            available_versions: entry
                .versions
                .iter()
                .map(|(version, value)| version_descriptor(*version, value, None))
                .collect(),
            cases: cases.into_iter().collect(),
            subjects: subjects.into_iter().collect(),
            subject_models: history_model_groups(subject_models),
            judge_models: history_model_groups(judge_models),
            systems: systems
                .into_iter()
                .map(|(id, label)| HistorySystem { id, label })
                .collect(),
            series,
            observations: page,
            total,
            next_cursor: (end < total).then(|| format!("{}:{end}", self.revision)),
        })
    }

    fn validate_comparison_context(
        &self,
        cohort_id: &str,
        from_version_id: &str,
        to_version_id: &str,
    ) -> Result<()> {
        if !self.cohorts.contains_key(cohort_id) {
            bail!("unknown evaluation cohort '{cohort_id}'");
        }
        for (side, version_id) in [("from", from_version_id), ("to", to_version_id)] {
            if !self
                .evaluated_versions
                .contains_key(&(cohort_id.to_string(), version_id.to_string()))
            {
                bail!("unknown {side} evaluated version '{version_id}' for cohort '{cohort_id}'");
            }
        }
        Ok(())
    }

    fn test_version_result(
        &self,
        test_id: &str,
        test_version: u32,
        cohort_id: &str,
        from_version_id: &str,
        to_version_id: &str,
    ) -> Result<TestVersionResult> {
        let entry = self
            .tests
            .get(test_id)
            .with_context(|| format!("unknown test '{test_id}'"))?;
        let version = entry
            .versions
            .get(&test_version)
            .with_context(|| format!("unknown test '{test_id}' version {test_version}"))?;
        let from_observations = matching_observations(version, cohort_id, from_version_id);
        let to_observations = matching_observations(version, cohort_id, to_version_id);
        let from = side_summary(from_version_id, &from_observations);
        let to = side_summary(to_version_id, &to_observations);
        let (compatibility, compatibility_reasons) =
            compatibility(&from_observations, &to_observations);
        let delta = if compatibility == "compatible" {
            TestDelta {
                score: metric_difference(
                    from.as_ref().and_then(|value| value.median_score),
                    to.as_ref().and_then(|value| value.median_score),
                ),
                cost_usd: metric_difference(
                    from.as_ref().and_then(|value| value.median_cost_usd),
                    to.as_ref().and_then(|value| value.median_cost_usd),
                ),
                tokens: metric_difference(
                    from.as_ref().and_then(|value| value.median_tokens),
                    to.as_ref().and_then(|value| value.median_tokens),
                ),
                duration_seconds: metric_difference(
                    from.as_ref()
                        .and_then(|value| value.median_duration_seconds),
                    to.as_ref().and_then(|value| value.median_duration_seconds),
                ),
            }
        } else {
            TestDelta::default()
        };
        Ok(TestVersionResult {
            test_id: test_id.into(),
            test_version,
            compatibility: compatibility.into(),
            compatibility_reasons,
            from,
            to,
            delta,
            from_observations: from_observations.iter().map(public_observation).collect(),
            to_observations: to_observations.iter().map(public_observation).collect(),
        })
    }
}

fn current_tests() -> Result<BTreeMap<String, TestEntry>> {
    ScenarioId::ALL
        .iter()
        .map(|id| {
            let materialized = id.materialize("dashboard-catalog", stable_seed(id.as_str()))?;
            Ok((
                id.as_str().to_string(),
                TestEntry {
                    current_version: Some(materialized.spec.version),
                    current_classification: Some(materialized.case.complexity),
                    current_characterization: Some(materialized.case.characterization),
                    current_spec: Some(spec_projection(*id, &materialized.spec)),
                    current_reference_verified: matches!(
                        id,
                        ScenarioId::IncidentResponse
                            | ScenarioId::ReleaseTrainRecovery
                            | ScenarioId::CrossRepoContractMigration
                    ),
                    versions: BTreeMap::from([(
                        materialized.spec.version,
                        TestVersionEntry::default(),
                    )]),
                },
            ))
        })
        .collect()
}

/// Projects the parts of a `ScenarioSpec` a reader needs. The evaluator, setup
/// and cleanup hooks stay behind: they are runner wiring, not contract.
fn spec_projection(id: ScenarioId, spec: &ScenarioSpec) -> TestSpecProjection {
    TestSpecProjection {
        summary: id.summary().map(str::to_string),
        prompt: spec.prompt.clone(),
        criteria: spec
            .criteria
            .iter()
            .map(|criterion| TestCriterionProjection {
                id: criterion.id.to_string(),
                weight: criterion.weight,
                description: criterion.description.to_string(),
                kind: criterion.kind,
                policy: criterion.policy,
                dimension: criterion.dimension,
                source: criterion.source,
            })
            .collect(),
        execution: spec.execution,
        denied_functions: spec
            .denied_functions
            .iter()
            .map(|function| (*function).to_string())
            .collect(),
    }
}

type CalibrationGroupKey = (String, String, String, String, String);

fn calibration_projection(
    entry: &TestVersionEntry,
    reference_verified: bool,
) -> CalibrationProjection {
    let compatible_sample_count =
        largest_compatible_sample_group(entry.observations.iter().map(|observation| {
            (
                (
                    observation.cohort_id.clone(),
                    observation.case_id.clone(),
                    observation.contract_sha256.clone(),
                    observation.assessment_profile_sha256.clone(),
                    observation.analyzer_profile_sha256.clone(),
                ),
                observation.runs.len(),
            )
        }));
    CalibrationProjection {
        maturity: calibration_maturity(compatible_sample_count, reference_verified).into(),
        compatible_sample_count,
    }
}

fn calibration_maturity(compatible_sample_count: usize, reference_verified: bool) -> &'static str {
    match compatible_sample_count {
        0 if reference_verified => "reference_verified",
        0 => "candidate",
        1..=4 => "observed",
        5..=19 => "repeatable",
        _ => "tail_calibrated",
    }
}

fn largest_compatible_sample_group(
    samples: impl IntoIterator<Item = (CalibrationGroupKey, usize)>,
) -> usize {
    let mut groups = BTreeMap::<CalibrationGroupKey, usize>::new();
    for (key, sample_count) in samples {
        *groups.entry(key).or_default() += sample_count;
    }
    groups.into_values().max().unwrap_or_default()
}

fn evaluated_version(
    report: &crate::report::E2eReport,
    cohort_id: &str,
    completed_at: &str,
) -> Result<Option<EvaluatedVersionDescriptor>> {
    let system = &report.system_under_test;
    let (stack_mode, label) = match &system.stack {
        StackIdentity::Source {
            workers_revision, ..
        } => (
            "source",
            format!(
                "Source {}",
                workers_revision.chars().take(12).collect::<String>()
            ),
        ),
        StackIdentity::Registry {
            stack_versions,
            stack_lock_digest,
        } => {
            let label = stack_versions
                .iter()
                .take(2)
                .map(|(worker, version)| format!("{worker}@{version}"))
                .collect::<Vec<_>>()
                .join(" · ");
            (
                "registry",
                if label.is_empty() {
                    format!(
                        "Registry {}",
                        stack_lock_digest.chars().take(12).collect::<String>()
                    )
                } else {
                    label
                },
            )
        }
    };
    let id = artifact::sha256_value(&json!({
        "stack": system.stack,
        "engine_version": system.engine_version,
        "engine_revision": system.engine_revision,
        "harness_version": system.harness_version,
        "contract_hashes": system.contract_hashes,
    }))?;
    Ok(Some(EvaluatedVersionDescriptor {
        id,
        cohort_id: cohort_id.into(),
        label,
        stack_mode: stack_mode.into(),
        completed_at: completed_at.into(),
        execution_count: 1,
    }))
}

fn scenario_contract_sha256(scenario: &E2eScenarioReport) -> Result<String> {
    artifact::sha256_value(&json!({
        "scenario_id": scenario.scenario_id,
        "scenario_version": scenario.scenario_version,
        "case": scenario.case,
        "execution_policy": scenario.execution_policy,
    }))
}

fn run_metrics(run: &E2eRunReport, assessment: &RunAssessmentContract) -> RunMetrics {
    let tokens = run.metrics.as_ref().and_then(|metrics| {
        metrics
            .totals
            .input_tokens
            .zip(metrics.totals.output_tokens)
            .map(|(input, output)| (input + output) as f64)
    });
    RunMetrics {
        score: run.score.map(f64::from),
        cost_usd: run.cost.total_usd,
        tokens,
        duration_seconds: (run.wall_time_ms > 0).then(|| run.wall_time_ms as f64 / 1_000.0),
        function_calls: run
            .efficiency
            .as_ref()
            .and_then(|efficiency| efficiency.function_calls)
            .map(|value| value as f64)
            .or_else(|| {
                run.metrics.as_ref().and_then(|metrics| {
                    metrics
                        .complete
                        .then_some(metrics.totals.function_calls as f64)
                })
            }),
        function_call_errors: run
            .efficiency
            .as_ref()
            .and_then(|efficiency| efficiency.function_call_errors)
            .map(|value| value as f64)
            .or_else(|| {
                run.metrics.as_ref().and_then(|metrics| {
                    metrics
                        .complete
                        .then_some(metrics.totals.function_call_errors as f64)
                })
            }),
        turns: run
            .efficiency
            .as_ref()
            .and_then(|efficiency| efficiency.root_turns.zip(efficiency.child_turns))
            .map(|(root, child)| (root + child) as f64)
            .or_else(|| {
                run.metrics.as_ref().and_then(|metrics| {
                    (metrics.complete && metrics.totals.turns > 0)
                        .then_some(metrics.totals.turns as f64)
                })
            }),
        status: run.status,
        assessment: assessment.clone(),
    }
}

fn scenario_status(scenario: &E2eScenarioReport) -> &'static str {
    if scenario.aggregate.technical_failures > 0 {
        "technical_failed"
    } else if scenario.aggregate.hard_gate_failures > 0 {
        "hard_gate_failed"
    } else if scenario.passed {
        "passed"
    } else {
        "infra_failed"
    }
}

fn version_descriptor(
    version: u32,
    entry: &TestVersionEntry,
    cohort_id: Option<&str>,
) -> VersionDescriptor {
    let observations = entry
        .observations
        .iter()
        .filter(|observation| cohort_id.is_none_or(|id| observation.cohort_id == id))
        .collect::<Vec<_>>();
    VersionDescriptor {
        version,
        execution_count: observations
            .iter()
            .map(|observation| observation.execution_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        run_count: observations
            .iter()
            .map(|observation| observation.runs.len())
            .sum(),
        last_seen: observations
            .iter()
            .map(|observation| observation.completed_at.clone())
            .max(),
    }
}

fn select_version(
    entry: &TestEntry,
    cohort_id: Option<&str>,
    from_version_id: Option<&str>,
    to_version_id: Option<&str>,
) -> Option<u32> {
    let mut versions = entry.versions.keys().copied().collect::<Vec<_>>();
    versions.sort_by(|left, right| right.cmp(left));
    if let (Some(cohort_id), Some(from), Some(to)) = (cohort_id, from_version_id, to_version_id) {
        if let Some(version) = versions.iter().copied().find(|version| {
            let entry = &entry.versions[version];
            !matching_observations(entry, cohort_id, from).is_empty()
                && !matching_observations(entry, cohort_id, to).is_empty()
        }) {
            return Some(version);
        }
        if let Some(version) = versions.iter().copied().find(|version| {
            !matching_observations(&entry.versions[version], cohort_id, to).is_empty()
        }) {
            return Some(version);
        }
    }
    versions.into_iter().next()
}

fn matching_observations<'a>(
    entry: &'a TestVersionEntry,
    cohort_id: &str,
    evaluated_version_id: &str,
) -> Vec<&'a Observation> {
    entry
        .observations
        .iter()
        .filter(|observation| {
            observation.cohort_id == cohort_id
                && observation.evaluated_version_id.as_deref() == Some(evaluated_version_id)
        })
        .collect()
}

fn side_summary(
    evaluated_version_id: &str,
    observations: &[&Observation],
) -> Option<TestSideSummary> {
    if observations.is_empty() {
        return None;
    }
    let runs = observations
        .iter()
        .flat_map(|observation| observation.runs.iter())
        .collect::<Vec<_>>();
    let scores = runs.iter().filter_map(|run| run.score).collect::<Vec<_>>();
    let costs = runs
        .iter()
        .filter_map(|run| run.cost_usd)
        .collect::<Vec<_>>();
    let tokens = runs.iter().filter_map(|run| run.tokens).collect::<Vec<_>>();
    let durations = runs
        .iter()
        .filter_map(|run| run.duration_seconds)
        .collect::<Vec<_>>();
    let outcomes = OutcomeCounts {
        passed: runs
            .iter()
            .filter(|run| run.status == RunStatus::Passed)
            .count(),
        hard_gate_failed: runs
            .iter()
            .filter(|run| run.status == RunStatus::HardGateFailed)
            .count(),
        technical_failed: runs
            .iter()
            .filter(|run| {
                matches!(
                    run.status,
                    RunStatus::SubjectError | RunStatus::JudgeError | RunStatus::ResourceLimit
                )
            })
            .count(),
        infra_failed: runs
            .iter()
            .filter(|run| run.status == RunStatus::InfrastructureError)
            .count(),
    };
    Some(TestSideSummary {
        evaluated_version_id: evaluated_version_id.into(),
        execution_count: observations
            .iter()
            .map(|observation| observation.execution_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        total_runs: runs.len(),
        scored_runs: scores.len(),
        case_count: observations
            .iter()
            .map(|observation| observation.case_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        median_score: median(scores.clone()),
        pass_rate: (!runs.is_empty()).then(|| outcomes.passed as f64 / runs.len() as f64),
        median_cost_usd: median(costs.clone()),
        median_tokens: median(tokens.clone()),
        median_duration_seconds: median(durations.clone()),
        outcomes,
        samples: MetricSamples {
            score: scores.len(),
            cost_usd: costs.len(),
            tokens: tokens.len(),
            duration_seconds: durations.len(),
            turns: runs.iter().filter(|run| run.turns.is_some()).count(),
        },
        assessment_summary: summarize(runs.iter().map(|run| &run.assessment)),
    })
}

fn compatibility(from: &[&Observation], to: &[&Observation]) -> (&'static str, Vec<String>) {
    if from.is_empty() || to.is_empty() {
        return ("missing_side", vec!["comparison_side_missing".into()]);
    }
    let Some(from_cases) = case_contracts(from) else {
        return (
            "contract_conflict",
            vec!["scenario_contract_conflict".into()],
        );
    };
    let Some(to_cases) = case_contracts(to) else {
        return (
            "contract_conflict",
            vec!["scenario_contract_conflict".into()],
        );
    };
    if from_cases != to_cases {
        return ("contract_changed", vec!["scenario_contract_changed".into()]);
    }
    let Some(from_assessments) = case_profiles(from, |value| &value.assessment_profile_sha256)
    else {
        return (
            "assessment_conflict",
            vec!["assessment_profile_conflict".into()],
        );
    };
    let Some(to_assessments) = case_profiles(to, |value| &value.assessment_profile_sha256) else {
        return (
            "assessment_conflict",
            vec!["assessment_profile_conflict".into()],
        );
    };
    if from_assessments != to_assessments {
        return (
            "assessment_changed",
            vec!["assessment_profile_changed".into()],
        );
    }
    let Some(from_analyzers) = case_profiles(from, |value| &value.analyzer_profile_sha256) else {
        return (
            "analyzer_conflict",
            vec!["analyzer_profile_conflict".into()],
        );
    };
    let Some(to_analyzers) = case_profiles(to, |value| &value.analyzer_profile_sha256) else {
        return (
            "analyzer_conflict",
            vec!["analyzer_profile_conflict".into()],
        );
    };
    if from_analyzers != to_analyzers {
        return ("analyzer_changed", vec!["analyzer_profile_changed".into()]);
    }
    ("compatible", Vec::new())
}

fn case_contracts(observations: &[&Observation]) -> Option<BTreeMap<String, String>> {
    case_profiles(observations, |observation| &observation.contract_sha256)
}

fn case_profiles<'a>(
    observations: &[&'a Observation],
    profile: impl Fn(&'a Observation) -> &'a String,
) -> Option<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    for observation in observations {
        let profile = profile(observation);
        if values
            .insert(observation.case_id.clone(), profile.clone())
            .is_some_and(|existing| existing != *profile)
        {
            return None;
        }
    }
    Some(values)
}

fn public_observation(observation: &&Observation) -> TestObservation {
    let scores = observation
        .runs
        .iter()
        .filter_map(|run| run.score)
        .collect::<Vec<_>>();
    let costs = observation
        .runs
        .iter()
        .filter_map(|run| run.cost_usd)
        .collect::<Vec<_>>();
    let tokens = observation
        .runs
        .iter()
        .filter_map(|run| run.tokens)
        .collect::<Vec<_>>();
    let durations = observation
        .runs
        .iter()
        .filter_map(|run| run.duration_seconds)
        .collect::<Vec<_>>();
    let turns = observation
        .runs
        .iter()
        .filter_map(|run| run.turns)
        .collect::<Vec<_>>();
    let function_calls = observation
        .runs
        .iter()
        .filter_map(|run| run.function_calls)
        .collect::<Vec<_>>();
    let function_call_errors = observation
        .runs
        .iter()
        .filter_map(|run| run.function_call_errors)
        .collect::<Vec<_>>();
    TestObservation {
        execution_id: observation.execution_id.clone(),
        evaluated_version_id: observation.evaluated_version_id.clone(),
        cohort_id: observation.cohort_id.clone(),
        completed_at: observation.completed_at.clone(),
        case_id: observation.case_id.clone(),
        contract_sha256: observation.contract_sha256.clone(),
        assessment_profile_sha256: observation.assessment_profile_sha256.clone(),
        analyzer_profile_sha256: observation.analyzer_profile_sha256.clone(),
        status: observation.status.clone(),
        median_score: median(scores.clone()),
        run_count: observation.runs.len(),
        scored_runs: scores.len(),
        assessment_summary: summarize(observation.runs.iter().map(|run| &run.assessment)),
        scenario_version: observation.scenario_version,
        seed: observation.seed,
        system_version_id: observation.evaluated_version_id.clone(),
        system_label: observation.system_label.clone(),
        system_revision: observation.system_revision.clone(),
        stack_mode: observation.stack_mode.clone(),
        harness_revision: observation.harness_revision.clone(),
        engine_revision: observation.engine_revision.clone(),
        subject_provider: observation.subject_provider.clone(),
        subject_model: observation.subject_model.clone(),
        judge_provider: observation.judge_provider.clone(),
        judge_model: observation.judge_model.clone(),
        judge_protocol: observation.judge_protocol.clone(),
        median_cost_usd: median(costs),
        median_tokens: median(tokens),
        median_duration_seconds: median(durations),
        median_function_calls: median(function_calls),
        median_function_call_errors: median(function_call_errors),
        median_turns: median(turns),
    }
}

fn history_matches(observation: &Observation, request: &TestHistoryRequest) -> bool {
    request
        .case_id
        .as_deref()
        .is_none_or(|value| value == observation.case_id)
        && request
            .subject_provider
            .as_deref()
            .is_none_or(|value| value == observation.subject_provider)
        && request
            .subject_model
            .as_deref()
            .is_none_or(|value| value == observation.subject_model)
        && request
            .judge_provider
            .as_deref()
            .is_none_or(|value| observation.judge_provider.as_deref() == Some(value))
        && request
            .judge_model
            .as_deref()
            .is_none_or(|value| observation.judge_model.as_deref() == Some(value))
        && request
            .system_version_id
            .as_deref()
            .is_none_or(|value| observation.evaluated_version_id.as_deref() == Some(value))
        && request
            .result
            .as_deref()
            .is_none_or(|value| value.eq_ignore_ascii_case(&observation.status))
}

fn history_model_groups(groups: BTreeMap<String, BTreeSet<String>>) -> Vec<HistoryModelGroup> {
    groups
        .into_iter()
        .map(|(provider, models)| HistoryModelGroup {
            provider,
            models: models.into_iter().collect(),
        })
        .collect()
}

fn history_series_key(observation: &Observation) -> String {
    // Keep this key aligned with the identity boundary used by the metric
    // history. A test/case can legitimately be rerun with a different
    // contract (inputs or execution policy), and a cohort alone does not
    // capture that distinction. Likewise, retaining the optional seed and
    // report identity fields prevents an unknown value from being silently
    // merged with a known one when older reports are mixed in.
    let scenario_version = observation.scenario_version.to_string();
    let seed = observation
        .seed
        .map(|seed| seed.to_string())
        .unwrap_or_else(|| "unknown-seed".into());
    [
        scenario_version.as_str(),
        observation.case_id.as_str(),
        observation.contract_sha256.as_str(),
        seed.as_str(),
        observation.cohort_id.as_str(),
        observation
            .evaluated_version_id
            .as_deref()
            .unwrap_or_default(),
        observation.stack_mode.as_str(),
        observation.system_revision.as_deref().unwrap_or_default(),
        observation.harness_revision.as_deref().unwrap_or_default(),
        observation.engine_revision.as_deref().unwrap_or_default(),
        observation.assessment_profile_sha256.as_str(),
        observation.analyzer_profile_sha256.as_str(),
        observation.subject_provider.as_str(),
        observation.subject_model.as_str(),
        observation.judge_provider.as_deref().unwrap_or_default(),
        observation.judge_model.as_deref().unwrap_or_default(),
        observation.judge_protocol.as_deref().unwrap_or_default(),
    ]
    .join("::")
}

fn history_series(id: String, observations: &[&Observation]) -> HistorySeries {
    let runs = observations
        .iter()
        .flat_map(|observation| observation.runs.iter())
        .collect::<Vec<_>>();
    let scores = runs.iter().filter_map(|run| run.score).collect::<Vec<_>>();
    let costs = runs
        .iter()
        .filter_map(|run| run.cost_usd)
        .collect::<Vec<_>>();
    let tokens = runs.iter().filter_map(|run| run.tokens).collect::<Vec<_>>();
    let durations = runs
        .iter()
        .filter_map(|run| run.duration_seconds)
        .collect::<Vec<_>>();
    let turns = runs.iter().filter_map(|run| run.turns).collect::<Vec<_>>();
    let function_calls = runs
        .iter()
        .filter_map(|run| run.function_calls)
        .collect::<Vec<_>>();
    let function_call_errors = runs
        .iter()
        .filter_map(|run| run.function_call_errors)
        .collect::<Vec<_>>();
    let first = observations.first().expect("history series is non-empty");
    HistorySeries {
        id,
        case_id: first.case_id.clone(),
        scenario_version: first.scenario_version,
        seed: first.seed,
        contract_sha256: first.contract_sha256.clone(),
        assessment_profile_sha256: first.assessment_profile_sha256.clone(),
        analyzer_profile_sha256: first.analyzer_profile_sha256.clone(),
        system_version_id: first.evaluated_version_id.clone(),
        system_label: first.system_label.clone(),
        stack_mode: first.stack_mode.clone(),
        harness_revision: first.harness_revision.clone(),
        system_revision: first.system_revision.clone(),
        engine_revision: first.engine_revision.clone(),
        subject_provider: first.subject_provider.clone(),
        subject_model: first.subject_model.clone(),
        judge_provider: first.judge_provider.clone(),
        judge_model: first.judge_model.clone(),
        judge_protocol: first.judge_protocol.clone(),
        cohort_id: first.cohort_id.clone(),
        execution_count: observations
            .iter()
            .map(|observation| observation.execution_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        run_count: runs.len(),
        median_score: median(scores),
        median_cost_usd: median(costs),
        median_tokens: median(tokens),
        median_duration_seconds: median(durations),
        median_function_calls: median(function_calls),
        median_function_call_errors: median(function_call_errors),
        median_turns: median(turns),
    }
}

fn parse_history_cursor(cursor: Option<&str>, revision: &str) -> Result<usize> {
    let Some(cursor) = cursor.filter(|value| !value.is_empty()) else {
        return Ok(0);
    };
    let (cursor_revision, offset) = cursor
        .rsplit_once(':')
        .context("history cursor is invalid")?;
    if cursor_revision != revision {
        bail!("history cursor is stale; reload the first page");
    }
    offset.parse().context("history cursor is invalid")
}

fn system_revision(report: &crate::report::E2eReport) -> Option<String> {
    match &report.system_under_test.stack {
        StackIdentity::Source {
            workers_revision, ..
        } => Some(workers_revision.clone()),
        StackIdentity::Registry {
            stack_lock_digest, ..
        } => Some(stack_lock_digest.clone()),
    }
}

fn parse_cursor(cursor: Option<&str>, revision: &str) -> Result<usize> {
    let Some(cursor) = cursor.filter(|value| !value.is_empty()) else {
        return Ok(0);
    };
    let (cursor_revision, offset) = cursor
        .rsplit_once(':')
        .context("test list cursor is invalid")?;
    if cursor_revision != revision {
        bail!("test list cursor is stale; reload the first page");
    }
    offset.parse().context("test list cursor is invalid")
}

fn metric_difference(from: Option<f64>, to: Option<f64>) -> Option<f64> {
    from.zip(to).map(|(from, to)| to - from)
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let midpoint = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[midpoint - 1] + values[midpoint]) / 2.0
    } else {
        values[midpoint]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::{ComplexityMethod, ExecutionRealism};

    fn calibration_key(suffix: &str) -> CalibrationGroupKey {
        (
            format!("cohort-{suffix}"),
            format!("case-{suffix}"),
            format!("contract-{suffix}"),
            format!("assessment-{suffix}"),
            format!("analyzer-{suffix}"),
        )
    }

    #[test]
    fn current_catalog_projects_materialized_classification_and_realism() {
        let root = tempfile::tempdir().expect("temporary dashboard store should exist");
        let model = DashboardReadModel::load(root.path())
            .expect("current scenarios should materialize into the read model");
        let response = model
            .tests_list(TestsListRequest {
                query: Some(ScenarioId::ContextPressure.as_str().into()),
                ..TestsListRequest::default()
            })
            .expect("current catalog should be readable");
        let context_pressure = response
            .rows
            .first()
            .expect("context pressure should be registered");
        assert_eq!(
            context_pressure
                .complexity
                .as_ref()
                .expect("classification should be projected")
                .method,
            ComplexityMethod::CapabilityV2
        );
        assert_eq!(
            context_pressure
                .characterization
                .expect("characterization should be projected")
                .realism
                .execution,
            ExecutionRealism::Synthetic
        );
        assert_eq!(
            context_pressure
                .calibration
                .as_ref()
                .expect("calibration should be projected")
                .maturity,
            "candidate"
        );

        let forensics = model
            .tests_list(TestsListRequest {
                query: Some(ScenarioId::GitRegressionForensics.as_str().into()),
                ..TestsListRequest::default()
            })
            .expect("git forensics should be readable")
            .rows
            .into_iter()
            .next()
            .expect("git forensics should be registered");
        assert_eq!(
            forensics
                .characterization
                .expect("characterization should be projected")
                .realism
                .execution,
            ExecutionRealism::FrozenRealArtifact
        );
    }

    #[test]
    fn current_catalog_projects_the_prompt_and_the_scoring_contract() {
        let root = tempfile::tempdir().expect("temporary dashboard store should exist");
        let model = DashboardReadModel::load(root.path())
            .expect("current scenarios should materialize into the read model");
        let chess = model
            .tests_list(TestsListRequest {
                query: Some(ScenarioId::ChessEngineBuild.as_str().into()),
                ..TestsListRequest::default()
            })
            .expect("chess engine build should be readable")
            .rows
            .into_iter()
            .next()
            .expect("chess engine build should be registered");
        let spec = chess.spec.expect("the scoring contract should be projected");

        // The prompt reaches the reader as the subject receives it.
        assert!(spec.prompt.contains("perft(fen, depth)"));
        assert!(spec.prompt.contains("legalmoves"));
        assert!(spec
            .summary
            .as_deref()
            .is_some_and(|summary| summary.contains("frozen fixture repository")));

        // Weights, policy and the description of every criterion travel with it.
        let weights: Vec<_> = spec
            .criteria
            .iter()
            .map(|criterion| (criterion.id.as_str(), criterion.weight, criterion.policy))
            .collect();
        assert_eq!(
            weights,
            vec![
                ("perft_exact", 40, AssessmentPolicy::HardGate),
                ("legal_moves_correct", 30, AssessmentPolicy::HardGate),
                ("interface_contract", 20, AssessmentPolicy::HardGate),
                ("build_discipline", 10, AssessmentPolicy::Advisory),
            ]
        );
        assert_eq!(
            spec.criteria.iter().map(|entry| u32::from(entry.weight)).sum::<u32>(),
            100
        );
        assert!(spec.criteria[0].description.contains("kernel oracle"));

        // The limits the run answers to are part of the contract, not trivia.
        assert_eq!(spec.execution.max_turns, 48);
        assert_eq!(spec.denied_functions, ["http::*", "browser::*", "github::*"]);
    }

    #[test]
    fn a_scenario_without_an_editorial_summary_still_projects_its_contract() {
        let root = tempfile::tempdir().expect("temporary dashboard store should exist");
        let model = DashboardReadModel::load(root.path())
            .expect("current scenarios should materialize into the read model");
        let row = model
            .tests_list(TestsListRequest {
                query: Some(ScenarioId::ContextPressure.as_str().into()),
                ..TestsListRequest::default()
            })
            .expect("context pressure should be readable")
            .rows
            .into_iter()
            .next()
            .expect("context pressure should be registered");
        let spec = row.spec.expect("the scoring contract should be projected");
        assert!(spec.summary.is_none());
        assert!(!spec.prompt.is_empty());
        assert!(!spec.criteria.is_empty());
    }

    #[test]
    fn calibration_uses_only_the_largest_compatible_sample_group() {
        let primary = calibration_key("primary");
        let secondary = calibration_key("secondary");
        assert_eq!(
            largest_compatible_sample_group([(primary.clone(), 3), (primary, 2), (secondary, 19),]),
            19
        );
    }

    #[test]
    fn calibration_thresholds_do_not_label_observed_evidence_as_robust() {
        for (sample_count, expected) in [
            (0, "candidate"),
            (1, "observed"),
            (4, "observed"),
            (5, "repeatable"),
            (19, "repeatable"),
            (20, "tail_calibrated"),
        ] {
            let maturity = calibration_maturity(sample_count, false);
            assert_eq!(maturity, expected);
            assert!(!maturity.contains("robust"));
        }
        assert_eq!(calibration_maturity(0, true), "reference_verified");
    }
}
