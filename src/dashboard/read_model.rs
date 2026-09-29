use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::assessment_projection::{
    assessment_profile_sha256, contracts_for_scenario, summarize, AssessmentSummary,
};
use super::presenter::{stored_execution_summary, MAX_EXECUTIONS};
use super::run_sessions::{read_run_details, results_file, HistoryRun};
#[cfg(test)]
use super::store::load_runs;
use super::store::StoredRun;
use crate::artifact;
use crate::assessment::{AssessmentKind, AssessmentPolicy, RunAssessmentContract};
use crate::control::ExecutionRecord;
use crate::identity::StackIdentity;
use crate::report::{
    CompletionState, E2eRunReport, E2eScenarioReport, EvaluationDimension, RunStatus,
    TechnicalState,
};
use crate::scenarios::{
    stable_seed, ExecutionPolicy, ScenarioCharacterization, ScenarioId, ScenarioSpec,
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
    pub test_version: String,
    pub cohort_id: String,
    pub from_version_id: String,
    pub to_version_id: String,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub(super) struct TestHistoryRequest {
    #[serde(default)]
    pub test_id: String,
    #[serde(default)]
    pub test_version: Option<String>,
    #[serde(default)]
    pub case_id: Option<String>,
    #[serde(default)]
    pub subject_provider: Option<String>,
    #[serde(default)]
    pub subject_model: Option<String>,
    #[serde(default)]
    pub system_version_id: Option<String>,
    #[serde(default)]
    pub result: Option<String>,
    /// Only these executions, at most two: the runs an A × B compares, found
    /// wherever they are in the history.
    #[serde(default)]
    pub executions: Option<Vec<String>>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(super) struct CohortDescriptor {
    pub id: String,
    pub lane: String,
    pub subject_provider: String,
    pub subject_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    pub version: String,
    pub execution_count: usize,
    /// The runs a test history lists: one per execution and case.
    pub observation_count: usize,
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
    pub mean_score: Option<f64>,
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
    pub status: String,
    pub mean_score: Option<f64>,
    pub run_count: usize,
    pub scored_runs: usize,
    pub assessment_summary: AssessmentSummary,
    pub behavior_sha256: String,
    pub seed: Option<u64>,
    pub system_version_id: Option<String>,
    pub system_label: String,
    pub stack_mode: String,
    pub harness_revision: Option<String>,
    pub system_revision: Option<String>,
    pub engine_revision: Option<String>,
    pub subject_provider: String,
    pub subject_model: String,
    pub median_cost_usd: Option<f64>,
    pub median_tokens: Option<f64>,
    pub median_duration_seconds: Option<f64>,
    pub median_function_calls: Option<f64>,
    pub median_function_call_errors: Option<f64>,
    pub median_turns: Option<f64>,
    pub runs: Vec<HistoryRun>,
    /// The execution as the Executions list shows it: the plan execution the
    /// run belongs to, when it has one. Only a history page carries it.
    pub plan_execution_id: Option<String>,
    /// That execution's name, when it has one.
    pub execution_label: Option<String>,
    /// The agent profile the subject ran with, when it had one.
    pub agent_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct HistorySeries {
    pub id: String,
    pub case_id: String,
    pub behavior_sha256: String,
    pub seed: Option<u64>,
    pub contract_sha256: String,
    pub assessment_profile_sha256: String,
    pub system_version_id: Option<String>,
    pub system_label: String,
    pub stack_mode: String,
    pub harness_revision: Option<String>,
    pub system_revision: Option<String>,
    pub engine_revision: Option<String>,
    pub subject_provider: String,
    pub subject_model: String,
    pub cohort_id: String,
    pub execution_count: usize,
    pub run_count: usize,
    pub mean_score: Option<f64>,
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
    /// The definition whose executions are shown (the latest with evidence by default).
    pub test_version: String,
    /// The current definition's digest, which may have no executions yet.
    pub current_version: Option<String>,
    pub available_versions: Vec<VersionDescriptor>,
    pub cases: Vec<String>,
    pub subjects: Vec<String>,
    pub subject_models: Vec<HistoryModelGroup>,
    pub systems: Vec<HistorySystem>,
    pub series: Vec<HistorySeries>,
    pub observations: Vec<TestObservation>,
    pub total: usize,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct TestVersionResult {
    pub test_id: String,
    pub test_version: String,
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
    pub current_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub characterization: Option<ScenarioCharacterization>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calibration: Option<CalibrationProjection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<TestSpecProjection>,
    pub available_versions: Vec<VersionDescriptor>,
    pub selected_version: Option<String>,
    pub result: Option<TestVersionResult>,
    /// The most recent retained run, on any definition.
    pub last_run: Option<LastRun>,
    /// The scores of the last runs, oldest first; null where a run has none.
    pub recent_scores: Vec<Option<f64>>,
    /// Runs retained on the current definition.
    pub runs_current: usize,
    /// Runs retained on every definition.
    pub runs_total: usize,
}

/// How many scores `recent_scores` carries.
const RECENT_SCORES: usize = 8;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct LastRun {
    /// When its execution completed.
    pub at: String,
    pub score: Option<f64>,
    pub status: RunStatus,
    pub completion: CompletionState,
    pub definition: RunDefinition,
}

/// Which definition a run evaluated, next to the test's current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum RunDefinition {
    /// The test's current definition.
    Current,
    /// An earlier definition (or the last one of a retired test).
    Previous,
}

/// The catalog's run facts for one test, from the same observations the
/// test history reads: every run in the order it completed.
fn run_facts(entry: &TestEntry) -> (Option<LastRun>, Vec<Option<f64>>, usize, usize) {
    let mut observations = entry
        .versions
        .iter()
        .flat_map(|(version, value)| value.observations.iter().map(move |o| (version, o)))
        .collect::<Vec<_>>();
    observations.sort_by(|(_, left), (_, right)| {
        left.completed_at
            .cmp(&right.completed_at)
            .then_with(|| left.execution_id.cmp(&right.execution_id))
    });
    let runs = observations
        .iter()
        .flat_map(|(version, observation)| {
            observation
                .runs
                .iter()
                .map(move |run| (*version, *observation, run))
        })
        .collect::<Vec<_>>();
    let current = |version: &str| entry.current_version.as_deref() == Some(version);
    // The current definition's last run when there is one: the test history
    // opens on that definition, so the row and the history agree.
    let last_run = runs
        .iter()
        .rev()
        .find(|(version, ..)| current(version))
        .or(runs.last())
        .map(|(version, observation, run)| LastRun {
            at: observation.completed_at.clone(),
            score: run.score,
            status: run.status,
            completion: run.completion,
            definition: if current(version) {
                RunDefinition::Current
            } else {
                RunDefinition::Previous
            },
        });
    let recent_scores = runs[runs.len().saturating_sub(RECENT_SCORES)..]
        .iter()
        .map(|(_, _, run)| run.score)
        .collect();
    let runs_current = runs.iter().filter(|(version, ..)| current(version)).count();
    (last_run, recent_scores, runs_current, runs.len())
}

/// The scenario definition as a reader needs it: what the subject is asked to
/// do, how the result is scored, and the limits it runs under. Projected from
/// the materialized `ScenarioSpec` of the test's current version, so it always
/// describes the contract the dashboard is showing.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct TestSpecProjection {
    /// Human-readable test name; absent on responses from older runners.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
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
    pub gate: bool,
    pub kind: AssessmentKind,
    pub policy: AssessmentPolicy,
    pub dimension: EvaluationDimension,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunMetrics {
    score: Option<f64>,
    cost_usd: Option<f64>,
    tokens: Option<f64>,
    duration_seconds: Option<f64>,
    function_calls: Option<f64>,
    function_call_errors: Option<f64>,
    turns: Option<f64>,
    completion: CompletionState,
    status: RunStatus,
    assessment: RunAssessmentContract,
    /// Absent in a projection written before trends read it; projections
    /// are kept as written.
    #[serde(default)]
    technical: Option<TechnicalState>,
    #[serde(default)]
    input_tokens: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Observation {
    execution_id: String,
    evaluated_version_id: Option<String>,
    cohort_id: String,
    completed_at: String,
    case_id: String,
    contract_sha256: String,
    assessment_profile_sha256: String,
    status: String,
    behavior_sha256: String,
    seed: Option<u64>,
    system_label: String,
    stack_mode: String,
    harness_revision: Option<String>,
    system_revision: Option<String>,
    engine_revision: Option<String>,
    subject_provider: String,
    subject_model: String,
    runs: Vec<RunMetrics>,
}

/// The durable dashboard view of one execution. It deliberately carries only
/// summaries, identity, metrics, and assessment results; native evidence
/// remains in the execution bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ExecutionProjection {
    pub(crate) summary: Value,
    cohort: Option<CohortDescriptor>,
    evaluated_version: Option<EvaluatedVersionDescriptor>,
    tests: BTreeMap<String, BTreeMap<String, Vec<Observation>>>,
}

#[derive(Debug, Clone, Default)]
struct TestVersionEntry {
    observations: Vec<Observation>,
}

#[derive(Debug, Clone, Default)]
struct TestEntry {
    current_version: Option<String>,
    current_characterization: Option<ScenarioCharacterization>,
    current_spec: Option<TestSpecProjection>,
    current_reference_verified: bool,
    versions: BTreeMap<String, TestVersionEntry>,
}

#[derive(Debug, Clone)]
pub(crate) struct DashboardReadModel {
    pub(super) revision: String,
    pub(crate) summaries: Vec<Value>,
    cohorts: BTreeMap<String, CohortDescriptor>,
    evaluated_versions: BTreeMap<(String, String), EvaluatedVersionDescriptor>,
    tests: BTreeMap<String, TestEntry>,
    /// Where each execution's native results are, relative to the runs
    /// directory. An execution missing here is read from its own directory.
    result_paths: BTreeMap<String, String>,
}

impl DashboardReadModel {
    /// Every record is listed; the runs a later attempt of the same slot
    /// replaced (`discarded`) are left out of tests, versions and cohorts.
    pub(crate) fn from_records(
        records: Vec<ExecutionRecord>,
        discarded: &BTreeSet<String>,
    ) -> Result<Self> {
        let mut model = Self::indexed(
            records
                .iter()
                .map(|record| match record.dashboard_projection.as_ref() {
                    Some(projection) => serde_json::from_value(projection.clone())
                        .context("decode dashboard execution projection"),
                    None => ExecutionProjection::from_record(record),
                })
                .collect::<Result<Vec<_>>>()?,
            discarded,
        )?;
        model.result_paths = records
            .into_iter()
            .filter_map(|record| Some((record.execution_id, record.result_path?)))
            .collect();
        Ok(model)
    }

    #[cfg(test)]
    pub(crate) fn from_projections(projections: Vec<ExecutionProjection>) -> Result<Self> {
        Self::indexed(projections, &BTreeSet::new())
    }

    fn indexed(
        mut projections: Vec<ExecutionProjection>,
        discarded: &BTreeSet<String>,
    ) -> Result<Self> {
        for projection in &mut projections {
            if !projection.summary["totals"]["turns"].is_null() {
                continue;
            }
            let turns = projection.summary["scenario_metrics"]
                .as_array()
                .filter(|metrics| !metrics.is_empty())
                .and_then(|metrics| {
                    metrics
                        .iter()
                        .map(|metric| {
                            let count = metric["run_count"].as_u64()?;
                            if count == 0 || metric["samples"]["turns"].as_u64()? != count {
                                return None;
                            }
                            Some(metric["averages"]["turns"].as_f64()? * count as f64)
                        })
                        .sum::<Option<f64>>()
                });
            projection.summary["totals"]["turns"] = json!(turns);
        }
        let revision = artifact::sha256_value(
            &projections
                .iter()
                .map(|projection| {
                    json!({
                        "id": projection.summary["id"],
                        "completed_at": projection.summary["completed_at"],
                        "status": projection.summary["status"],
                    })
                })
                .collect::<Vec<_>>(),
        )?;
        let mut summaries = projections
            .iter()
            .map(|projection| projection.summary.clone())
            .collect::<Vec<_>>();
        summaries.sort_by(|left, right| {
            right["started_at"]
                .as_str()
                .cmp(&left["started_at"].as_str())
                .then_with(|| right["id"].as_str().cmp(&left["id"].as_str()))
        });
        let mut model = Self {
            revision,
            summaries,
            cohorts: BTreeMap::new(),
            evaluated_versions: BTreeMap::new(),
            tests: current_tests()?,
            result_paths: BTreeMap::new(),
        };
        for projection in projections {
            if projection.summary["id"]
                .as_str()
                .is_some_and(|id| discarded.contains(id))
            {
                continue;
            }
            model.index_projection(projection);
        }
        Ok(model)
    }

    #[cfg(test)]
    pub(crate) fn load(runs_dir: &Path) -> Result<Self> {
        Self::from_stored(load_runs(runs_dir)?)
    }

    #[cfg(test)]
    fn from_stored(mut stored: Vec<StoredRun>) -> Result<Self> {
        stored.sort_by(|left, right| {
            right
                .metadata
                .started_at
                .cmp(&left.metadata.started_at)
                .then_with(|| right.metadata.id.cmp(&left.metadata.id))
        });

        stored.truncate(MAX_EXECUTIONS);
        Self::from_projections(
            stored
                .iter()
                .map(ExecutionProjection::from_stored)
                .collect::<Result<Vec<_>>>()?,
        )
    }

    fn index_projection(&mut self, projection: ExecutionProjection) {
        if let Some(cohort) = projection.cohort {
            self.cohorts.entry(cohort.id.clone()).or_insert(cohort);
        }
        if let Some(descriptor) = projection.evaluated_version {
            self.evaluated_versions
                .entry((descriptor.cohort_id.clone(), descriptor.id.clone()))
                .and_modify(|existing| {
                    existing.execution_count += descriptor.execution_count;
                    if descriptor.completed_at > existing.completed_at {
                        existing.completed_at.clone_from(&descriptor.completed_at);
                    }
                })
                .or_insert(descriptor);
        }
        for (test_id, versions) in projection.tests {
            let test = self.tests.entry(test_id).or_default();
            for (version, observations) in versions {
                test.versions
                    .entry(version)
                    .or_default()
                    .observations
                    .extend(observations);
            }
        }
    }
}

impl ExecutionProjection {
    pub(crate) fn from_record(record: &ExecutionRecord) -> Result<Self> {
        Self::from_stored(&StoredRun {
            metadata: super::controller::metadata_from_record(record),
            report: record.report.clone(),
            live_progress: None,
            live_progress_error: None,
        })
    }

    fn from_stored(stored: &StoredRun) -> Result<Self> {
        let summary = stored_execution_summary(stored)?;
        let Some(report) = stored.report.as_ref() else {
            return Ok(Self {
                summary,
                cohort: None,
                evaluated_version: None,
                tests: BTreeMap::new(),
            });
        };
        let lane = report.execution.lane.clone();
        let cohort = CohortDescriptor {
            id: String::new(),
            lane,
            subject_provider: report.subject.provider.clone(),
            subject_model: report.subject.model.clone(),
        };
        let cohort_id = artifact::sha256_value(&json!({
            "lane": cohort.lane,
            "subject_provider": cohort.subject_provider,
            "subject_model": cohort.subject_model,
        }))?;
        let cohort = CohortDescriptor {
            id: cohort_id.clone(),
            ..cohort
        };

        let evaluated = evaluated_version(report, &cohort_id, &stored.metadata.completed_at)?;
        let mut tests = BTreeMap::new();
        for scenario in &report.scenarios {
            let contract_sha256 = scenario_contract_sha256(scenario)?;
            let contracts = contracts_for_scenario(report, scenario);
            let assessment_profile_sha256 =
                assessment_profile_sha256(scenario.behavior_sha256.as_deref(), &contracts)?;
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
            tests
                .entry(scenario.scenario_id.clone())
                .or_insert_with(BTreeMap::new)
                .entry(definition_key(scenario))
                .or_insert_with(Vec::new)
                .push(Observation {
                    execution_id: stored.metadata.id.clone(),
                    evaluated_version_id: evaluated.as_ref().map(|value| value.id.clone()),
                    cohort_id: cohort_id.clone(),
                    completed_at: stored.metadata.completed_at.clone(),
                    case_id: scenario.case_id.clone(),
                    contract_sha256,
                    assessment_profile_sha256,
                    status: scenario_status(scenario).into(),
                    behavior_sha256: definition_key(scenario),
                    seed: scenario.case.as_ref().map(|case| case.seed),
                    system_label,
                    stack_mode,
                    harness_revision: Some(report.system_under_test.e2e_revision.clone()),
                    system_revision,
                    engine_revision,
                    subject_provider: report.subject.provider.clone(),
                    subject_model: report.subject.model.clone(),
                    runs,
                });
        }
        Ok(Self {
            summary,
            cohort: Some(cohort),
            evaluated_version: evaluated,
            tests,
        })
    }
}

impl DashboardReadModel {
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
        let current = entry.current_version.as_deref();
        let mut available_versions = entry
            .versions
            .iter()
            .map(|(version, value)| {
                version_descriptor(version.clone(), value, request.cohort_id.as_deref())
            })
            .filter(|descriptor| {
                descriptor.execution_count > 0 || current == Some(descriptor.version.as_str())
            })
            .collect::<Vec<_>>();
        // The current definition first, then the most recently observed ones.
        available_versions.sort_by(|left, right| {
            let left_current = current == Some(left.version.as_str());
            let right_current = current == Some(right.version.as_str());
            right_current
                .cmp(&left_current)
                .then_with(|| right.last_seen.cmp(&left.last_seen))
                .then_with(|| left.version.cmp(&right.version))
        });
        let selected_version = select_version(
            entry,
            request.cohort_id.as_deref(),
            request.from_version_id.as_deref(),
            request.to_version_id.as_deref(),
        );
        let result = match (
            selected_version.as_deref(),
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
        let (last_run, recent_scores, runs_current, runs_total) = run_facts(entry);
        Ok(TestCatalogRow {
            test_id: test_id.into(),
            lifecycle: lifecycle.into(),
            current_version: entry.current_version.clone(),
            characterization: entry.current_characterization,
            calibration: entry.current_version.as_ref().and_then(|version| {
                entry.versions.get(version).map(|version| {
                    calibration_projection(version, entry.current_reference_verified)
                })
            }),
            spec: entry.current_spec.clone(),
            available_versions,
            selected_version,
            result,
            last_run,
            recent_scores,
            runs_current,
            runs_total,
        })
    }

    pub(super) fn test_version_get(
        &self,
        request: TestVersionGetRequest,
    ) -> Result<TestVersionResult> {
        if request.test_id.trim().is_empty() || request.test_version.trim().is_empty() {
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
            &request.test_version,
            &request.cohort_id,
            &request.from_version_id,
            &request.to_version_id,
        )
    }

    pub(super) fn test_history(&self, request: TestHistoryRequest) -> Result<TestHistoryResponse> {
        if request.test_id.trim().is_empty() {
            bail!("test id is required");
        }
        if request.executions.as_ref().is_some_and(|ids| ids.len() > 2) {
            bail!("a test history is asked for at most two executions");
        }
        let entry = self
            .tests
            .get(&request.test_id)
            .with_context(|| format!("unknown test '{}'", request.test_id))?;
        let all = request.test_version.as_deref() == Some(ALL_DEFINITIONS);
        let test_version = request
            .test_version
            .clone()
            .or_else(|| {
                entry.current_version.clone().filter(|version| {
                    entry
                        .versions
                        .get(version)
                        .is_some_and(|value| !value.observations.is_empty())
                })
            })
            .or_else(|| latest_observed_version(entry))
            .or_else(|| entry.current_version.clone())
            .or_else(|| entry.versions.keys().next().cloned())
            .context("test has no version")?;
        let versions = if all {
            entry.versions.values().collect::<Vec<_>>()
        } else {
            vec![entry.versions.get(&test_version).with_context(|| {
                format!("unknown test '{}' version {test_version}", request.test_id)
            })?]
        };
        let in_scope = || {
            versions
                .iter()
                .flat_map(|version| version.observations.iter())
        };
        let mut observations = in_scope()
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
        let mut cases = in_scope()
            .map(|observation| observation.case_id.clone())
            .collect::<BTreeSet<_>>();
        let mut subjects = BTreeSet::new();
        let mut subject_models = BTreeMap::<String, BTreeSet<String>>::new();
        let mut systems = BTreeMap::new();
        for observation in in_scope() {
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
            current_version: entry.current_version.clone(),
            available_versions: entry
                .versions
                .iter()
                .map(|(version, value)| version_descriptor(version.clone(), value, None))
                .collect(),
            cases: cases.into_iter().collect(),
            subjects: subjects.into_iter().collect(),
            subject_models: history_model_groups(subject_models),
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

    /// Every run of every test, by the native run it belongs to, as trends
    /// measure them. Replaced attempts are already left out.
    pub(super) fn trend_runs(&self) -> BTreeMap<String, Vec<super::trends::TrendRun>> {
        let mut runs = BTreeMap::<String, Vec<_>>::new();
        for (test, entry) in &self.tests {
            for observation in entry.versions.values().flat_map(|v| &v.observations) {
                let behavior_sha256 = Some(observation.behavior_sha256.clone())
                    .filter(|definition| definition != UNMATERIALIZED_DEFINITION);
                let native = runs.entry(observation.execution_id.clone()).or_default();
                native.extend(observation.runs.iter().map(|run| {
                    super::trends::TrendRun {
                        test: test.clone(),
                        behavior_sha256: behavior_sha256.clone(),
                        // ponytail: a projection written before runs kept their
                        // technical state counts a run by its score (an invalid
                        // run's is dropped): a valid unscored run of one is left
                        // out, and none has input tokens. Reprojecting lifts it.
                        counted: run
                            .technical
                            .map_or(run.score.is_some(), |t| t == TechnicalState::Valid),
                        completed: run.completion == CompletionState::Completed,
                        score: run.score,
                        duration_ms: run.duration_seconds.map(|seconds| seconds * 1_000.0),
                        input_tokens: run.input_tokens,
                        function_calls: run.function_calls,
                        function_call_errors: run.function_call_errors,
                        turns: run.turns,
                    }
                }));
            }
        }
        runs
    }

    /// Completes a history page with what only the runs' native results and
    /// the listed executions hold: each run's sessions, calls and
    /// criteria, and the execution each observation is listed under.
    pub(super) fn attach_run_details(
        &self,
        history: &mut TestHistoryResponse,
        summaries: &[Value],
        runs_dir: &Path,
    ) {
        let summary = |id: &str| summaries.iter().find(|summary| summary["id"] == id);
        let text = |value: &Value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        let mut files = BTreeMap::<PathBuf, Vec<usize>>::new();
        for (index, observation) in history.observations.iter_mut().enumerate() {
            let native = summary(&observation.execution_id);
            let plan = native
                .and_then(|native| native["parent_plan_execution_id"].as_str())
                .and_then(summary);
            observation.plan_execution_id = plan.and_then(|plan| text(&plan["id"]));
            observation.execution_label = plan.or(native).and_then(|value| text(&value["label"]));
            observation.agent_profile = native
                .into_iter()
                .chain(plan)
                .find_map(|value| text(&value["parameters"]["agent"]));
            let path = results_file(
                runs_dir.join(
                    self.result_paths
                        .get(&observation.execution_id)
                        .unwrap_or(&observation.execution_id),
                ),
            );
            files.entry(path).or_default().push(index);
        }
        // Each file is read once, for every case of the test it holds.
        for (path, indices) in files {
            let cases = indices
                .iter()
                .map(|index| history.observations[*index].case_id.as_str())
                .collect::<BTreeSet<_>>();
            let mut details = match read_run_details(&path, &history.test_id, &cases) {
                Ok(details) => details,
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %format!("{error:#}"),
                        "a history page has no readable native results for an execution"
                    );
                    continue;
                }
            };
            for index in indices {
                let observation = &mut history.observations[index];
                for run in &mut observation.runs {
                    run.details = details.remove(&(
                        observation.case_id.clone(),
                        run.run_id.clone(),
                        run.attempt_id.clone(),
                    ));
                }
            }
        }
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
        test_version: &str,
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
            .get(test_version)
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
                    from.as_ref().and_then(|value| value.mean_score),
                    to.as_ref().and_then(|value| value.mean_score),
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
            test_version: test_version.into(),
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
                    current_version: Some(materialized.case.behavior_sha256.clone()),
                    current_characterization: Some(materialized.case.characterization),
                    current_spec: Some(spec_projection(*id, &materialized.spec)),
                    current_reference_verified: matches!(
                        id,
                        ScenarioId::IncidentResponse
                            | ScenarioId::ReleaseTrainRecovery
                            | ScenarioId::CrossRepoContractMigration
                    ),
                    versions: BTreeMap::from([(
                        materialized.case.behavior_sha256.clone(),
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
        title: id.title().map(str::to_string),
        summary: id.summary().map(str::to_string),
        prompt: spec.prompt.clone(),
        criteria: spec
            .criteria
            .iter()
            .map(|criterion| TestCriterionProjection {
                id: criterion.id.to_string(),
                weight: criterion.weight,
                description: criterion.description.to_string(),
                gate: criterion.gate,
                kind: criterion.kind,
                policy: criterion.policy,
                dimension: criterion.dimension,
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

type CalibrationGroupKey = (String, String, String, String);

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

/// History groups executions by the definition that evaluated them. A slot
/// whose case never materialized has no definition digest and is grouped
/// under this marker instead of being merged into a real definition.
pub(super) const UNMATERIALIZED_DEFINITION: &str = "unmaterialized";

/// Asks the history for the executions of every definition at once.
pub(super) const ALL_DEFINITIONS: &str = "all";

fn definition_key(scenario: &E2eScenarioReport) -> String {
    scenario
        .behavior_sha256
        .clone()
        .unwrap_or_else(|| UNMATERIALIZED_DEFINITION.to_string())
}

fn scenario_contract_sha256(scenario: &E2eScenarioReport) -> Result<String> {
    artifact::sha256_value(&json!({
        "scenario_id": scenario.scenario_id,
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
        // Only a technically valid run's score counts toward a mean.
        score: (run.technical == crate::report::TechnicalState::Valid)
            .then_some(run.score)
            .flatten()
            .map(f64::from),
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
        completion: run.completion,
        status: run.status,
        assessment: assessment.clone(),
        technical: Some(run.technical),
        input_tokens: run
            .metrics
            .as_ref()
            .and_then(|metrics| metrics.totals.input_tokens)
            .map(|tokens| tokens as f64),
    }
}

fn scenario_status(scenario: &E2eScenarioReport) -> &'static str {
    if scenario.aggregate.technical_failures > 0 {
        "technical_failed"
    } else if scenario.aggregate.planned_runs > 0
        && scenario.aggregate.completed_runs == scenario.aggregate.planned_runs
    {
        "passed"
    } else {
        "incomplete"
    }
}

fn version_descriptor(
    version: String,
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
        observation_count: observations.len(),
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

/// The definition digest whose observations were completed most recently.
fn latest_observed_version(entry: &TestEntry) -> Option<String> {
    entry
        .versions
        .iter()
        .filter(|(_, value)| !value.observations.is_empty())
        .max_by(|(left_version, left), (right_version, right)| {
            latest_seen(left)
                .cmp(&latest_seen(right))
                .then_with(|| right_version.cmp(left_version))
        })
        .map(|(version, _)| version.clone())
}

fn latest_seen(entry: &TestVersionEntry) -> Option<&str> {
    entry
        .observations
        .iter()
        .map(|observation| observation.completed_at.as_str())
        .max()
}

fn select_version(
    entry: &TestEntry,
    cohort_id: Option<&str>,
    from_version_id: Option<&str>,
    to_version_id: Option<&str>,
) -> Option<String> {
    let mut versions = entry.versions.keys().cloned().collect::<Vec<_>>();
    // Most recently observed definitions first, then the digest for stability.
    versions.sort_by(|left, right| {
        latest_seen(&entry.versions[right])
            .cmp(&latest_seen(&entry.versions[left]))
            .then_with(|| left.cmp(right))
    });
    if let (Some(cohort_id), Some(from), Some(to)) = (cohort_id, from_version_id, to_version_id) {
        if let Some(version) = versions.iter().find(|version| {
            let entry = &entry.versions[*version];
            !matching_observations(entry, cohort_id, from).is_empty()
                && !matching_observations(entry, cohort_id, to).is_empty()
        }) {
            return Some(version.clone());
        }
        if let Some(version) = versions.iter().find(|version| {
            !matching_observations(&entry.versions[*version], cohort_id, to).is_empty()
        }) {
            return Some(version.clone());
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
            .filter(|run| {
                matches!(run.status, RunStatus::Passed | RunStatus::HardGateFailed)
                    && run.completion == CompletionState::Completed
            })
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
                    RunStatus::SubjectError | RunStatus::ResourceLimit
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
        mean_score: mean(&scores),
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
        status: observation.status.clone(),
        mean_score: mean(&scores),
        run_count: observation.runs.len(),
        scored_runs: scores.len(),
        assessment_summary: summarize(observation.runs.iter().map(|run| &run.assessment)),
        behavior_sha256: observation.behavior_sha256.clone(),
        seed: observation.seed,
        system_version_id: observation.evaluated_version_id.clone(),
        system_label: observation.system_label.clone(),
        system_revision: observation.system_revision.clone(),
        stack_mode: observation.stack_mode.clone(),
        harness_revision: observation.harness_revision.clone(),
        engine_revision: observation.engine_revision.clone(),
        subject_provider: observation.subject_provider.clone(),
        subject_model: observation.subject_model.clone(),
        median_cost_usd: median(costs),
        median_tokens: median(tokens),
        median_duration_seconds: median(durations),
        median_function_calls: median(function_calls),
        median_function_call_errors: median(function_call_errors),
        median_turns: median(turns),
        runs: observation
            .runs
            .iter()
            .map(|run| HistoryRun {
                run_id: run.assessment.run_id.clone(),
                attempt_id: run.assessment.attempt_id.clone(),
                status: run.status,
                completion: run.completion,
                score: run.score,
                duration_seconds: run.duration_seconds,
                turns: run.turns,
                function_calls: run.function_calls,
                function_call_errors: run.function_call_errors,
                details: None,
            })
            .collect(),
        plan_execution_id: None,
        execution_label: None,
        agent_profile: None,
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
            .system_version_id
            .as_deref()
            .is_none_or(|value| observation.evaluated_version_id.as_deref() == Some(value))
        && request
            .result
            .as_deref()
            .is_none_or(|value| value.eq_ignore_ascii_case(&observation.status))
        && request
            .executions
            .as_ref()
            .is_none_or(|ids| ids.contains(&observation.execution_id))
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
    let seed = observation
        .seed
        .map(|seed| seed.to_string())
        .unwrap_or_else(|| "unknown-seed".into());
    [
        observation.behavior_sha256.as_str(),
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
        observation.subject_provider.as_str(),
        observation.subject_model.as_str(),
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
        behavior_sha256: first.behavior_sha256.clone(),
        seed: first.seed,
        contract_sha256: first.contract_sha256.clone(),
        assessment_profile_sha256: first.assessment_profile_sha256.clone(),
        system_version_id: first.evaluated_version_id.clone(),
        system_label: first.system_label.clone(),
        stack_mode: first.stack_mode.clone(),
        harness_revision: first.harness_revision.clone(),
        system_revision: first.system_revision.clone(),
        engine_revision: first.engine_revision.clone(),
        subject_provider: first.subject_provider.clone(),
        subject_model: first.subject_model.clone(),
        cohort_id: first.cohort_id.clone(),
        execution_count: observations
            .iter()
            .map(|observation| observation.execution_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        run_count: runs.len(),
        mean_score: mean(&scores),
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

fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
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
    use crate::scenarios::ExecutionRealism;

    fn calibration_key(suffix: &str) -> CalibrationGroupKey {
        (
            format!("cohort-{suffix}"),
            format!("case-{suffix}"),
            format!("contract-{suffix}"),
            format!("assessment-{suffix}"),
        )
    }

    #[test]
    fn execution_projection_roundtrip_keeps_history_without_native_evidence() {
        let mut report = super::super::tests::report();
        report.scenarios[0].runs[0].transcript = Some(json!({"private": "transcript"}));
        let projection = ExecutionProjection::from_stored(&StoredRun {
            metadata: super::super::tests::metadata(),
            report: Some(report),
            live_progress: None,
            live_progress_error: None,
        })
        .unwrap();
        let serialized = serde_json::to_string(&projection).unwrap();
        assert!(!serialized.contains("prompt"));
        assert!(!serialized.contains("transcript"));
        let restored = serde_json::from_str(&serialized).unwrap();
        let model = DashboardReadModel::from_projections(vec![restored]).unwrap();
        assert_eq!(
            model
                .evaluated_versions(EvaluatedVersionsRequest::default())
                .cohorts
                .len(),
            1
        );
        let history = model
            .test_history(TestHistoryRequest {
                test_id: "direct_answer".into(),
                ..TestHistoryRequest::default()
            })
            .unwrap();
        assert_eq!(history.total, 1);
        assert_eq!(history.observations[0].mean_score, Some(90.0));
        assert_eq!(history.observations[0].median_tokens, None);
    }

    #[test]
    fn trend_runs_count_valid_runs_and_read_an_older_projection_by_its_score() {
        let projection = |id: &str, technical| {
            let mut metadata = super::super::tests::metadata();
            metadata.id = id.into();
            let mut report = super::super::tests::report();
            report.scenarios[0].runs[0].technical = technical;
            ExecutionProjection::from_stored(&StoredRun {
                metadata,
                report: Some(report),
                live_progress: None,
                live_progress_error: None,
            })
            .unwrap()
        };
        // Written before runs kept their technical state and input tokens.
        let older = serde_json::to_string(&projection("local-older", TechnicalState::Valid))
            .unwrap()
            .replace(r#","technical":"valid""#, "")
            .replace(r#","input_tokens":null"#, "");
        assert!(!older.contains("\"technical\":"));
        let model = DashboardReadModel::indexed(
            vec![
                projection("local-valid", TechnicalState::Valid),
                projection("local-invalid", TechnicalState::TechnicalInvalid),
                serde_json::from_str(&older).unwrap(),
            ],
            &BTreeSet::new(),
        )
        .unwrap();
        let runs = model.trend_runs();
        let counted = |id: &str| {
            let run = &runs[id][0];
            (run.test.as_str(), run.counted, run.score, run.duration_ms)
        };
        assert_eq!(
            counted("local-valid"),
            ("direct_answer", true, Some(90.0), Some(1_500.0))
        );
        assert_eq!(
            counted("local-invalid"),
            ("direct_answer", false, None, Some(1_500.0))
        );
        assert_eq!(
            counted("local-older"),
            ("direct_answer", true, Some(90.0), Some(1_500.0))
        );
    }

    #[test]
    fn a_replaced_attempt_stays_listed_but_out_of_test_history() {
        let projection = |id: &str| {
            let mut metadata = super::super::tests::metadata();
            metadata.id = id.into();
            ExecutionProjection::from_stored(&StoredRun {
                metadata,
                report: Some(super::super::tests::report()),
                live_progress: None,
                live_progress_error: None,
            })
            .unwrap()
        };
        let history = |discarded: &[&str]| {
            DashboardReadModel::indexed(
                vec![projection("local-replaced"), projection("local-current")],
                &discarded.iter().map(|id| (*id).to_owned()).collect(),
            )
            .unwrap()
        };
        let direct = TestHistoryRequest {
            test_id: "direct_answer".into(),
            ..TestHistoryRequest::default()
        };
        assert_eq!(history(&[]).test_history(direct.clone()).unwrap().total, 2);
        let model = history(&["local-replaced"]);
        // Still listed, so its evidence opens from the attempt it was.
        assert_eq!(model.summaries.len(), 2);
        let current = model.test_history(direct).unwrap();
        assert_eq!(current.total, 1);
        assert_eq!(current.observations[0].execution_id, "local-current");
        assert_eq!(current.series[0].execution_count, 1);
        let row = model
            .tests_list(TestsListRequest::default())
            .unwrap()
            .rows
            .into_iter()
            .find(|row| row.test_id == "direct_answer")
            .unwrap();
        assert_eq!(row.available_versions[0].execution_count, 1);
        assert_eq!(
            model
                .evaluated_versions(EvaluatedVersionsRequest::default())
                .versions[0]
                .execution_count,
            1
        );
    }

    #[test]
    fn the_catalog_carries_each_tests_last_run_recent_scores_and_run_counts() {
        let projection = |id: &str, completed_at: &str, definition: &str, scores: &[Option<u8>]| {
            let mut metadata = super::super::tests::metadata();
            metadata.id = id.into();
            metadata.completed_at = completed_at.into();
            let mut report = super::super::tests::report();
            let template = report.scenarios[0].runs[0].clone();
            let runs = scores
                .iter()
                .enumerate()
                .map(|(index, score)| {
                    let mut run = template.clone();
                    run.run_id = format!("run-{index}");
                    run.score = *score;
                    if score.is_none() {
                        run.status = RunStatus::InfrastructureError;
                    }
                    run
                })
                .collect();
            let policy = report.scenarios[0].execution_policy;
            let mut scenario = E2eScenarioReport::aggregate("direct_answer", policy, runs);
            scenario.behavior_sha256 = Some(definition.into());
            report.scenarios = vec![scenario];
            report.assessment_contract =
                crate::assessment::AssessmentContract::from_assessment_evidence(&report);
            ExecutionProjection::from_stored(&StoredRun {
                metadata,
                report: Some(report),
                live_progress: None,
                live_progress_error: None,
            })
            .unwrap()
        };
        // Listed newest first, as the store does; the facts follow completion.
        let mut model = DashboardReadModel::from_projections(vec![
            projection(
                "local-new",
                "2026-08-09T10:00:00Z",
                "new",
                &[Some(40), None],
            ),
            projection("local-old", "2026-08-08T10:00:00Z", "old", &[Some(90)]),
        ])
        .unwrap();
        let row = |model: &DashboardReadModel| {
            model
                .tests_list(TestsListRequest::default())
                .unwrap()
                .rows
                .into_iter()
                .find(|row| row.test_id == "direct_answer")
                .unwrap()
        };
        model
            .tests
            .get_mut("direct_answer")
            .unwrap()
            .current_version = Some("new".into());
        let current = row(&model);
        assert_eq!(current.recent_scores, vec![Some(90.0), Some(40.0), None]);
        assert_eq!((current.runs_current, current.runs_total), (2, 3));
        let last = current.last_run.unwrap();
        assert_eq!(last.at, "2026-08-09T10:00:00Z");
        assert_eq!(last.score, None);
        assert_eq!(last.status, RunStatus::InfrastructureError);
        assert_eq!(last.definition, RunDefinition::Current);

        // A run of the current definition, then runs of another one: the
        // last run is the current definition's, where the history opens.
        model
            .tests
            .get_mut("direct_answer")
            .unwrap()
            .current_version = Some("old".into());
        let earlier = row(&model);
        assert_eq!((earlier.runs_current, earlier.runs_total), (1, 3));
        let last = earlier.last_run.unwrap();
        assert_eq!(last.at, "2026-08-08T10:00:00Z");
        assert_eq!(last.score, Some(90.0));
        assert_eq!(last.definition, RunDefinition::Current);

        // The definition moved on: the runs stay, none of them current.
        model
            .tests
            .get_mut("direct_answer")
            .unwrap()
            .current_version = Some("newer".into());
        let changed = row(&model);
        assert_eq!((changed.runs_current, changed.runs_total), (0, 3));
        assert_eq!(
            changed.last_run.unwrap().definition,
            RunDefinition::Previous
        );

        // Only the last eight scores travel.
        let many = DashboardReadModel::from_projections(vec![projection(
            "local-many",
            "2026-08-10T10:00:00Z",
            "new",
            &(1..=10).map(|score| Some(score * 10)).collect::<Vec<_>>(),
        )])
        .unwrap();
        let many = row(&many);
        assert_eq!(many.runs_total, 10);
        assert_eq!(
            many.recent_scores,
            (3..=10)
                .map(|score| Some(f64::from(score * 10)))
                .collect::<Vec<_>>()
        );
        let never = model
            .tests_list(TestsListRequest {
                query: Some(ScenarioId::MinimalPath.as_str().into()),
                ..TestsListRequest::default()
            })
            .unwrap()
            .rows
            .into_iter()
            .next()
            .unwrap();
        assert!(never.last_run.is_none() && never.recent_scores.is_empty());
        assert_eq!((never.runs_current, never.runs_total), (0, 0));
    }

    #[test]
    fn retained_projection_restores_only_fully_reported_turns() {
        let mut projection = ExecutionProjection::from_stored(&StoredRun {
            metadata: super::super::tests::metadata(),
            report: Some(super::super::tests::report()),
            live_progress: None,
            live_progress_error: None,
        })
        .unwrap();
        projection.summary["totals"]
            .as_object_mut()
            .unwrap()
            .remove("turns");
        projection.summary["scenario_metrics"] = json!([
            {"run_count": 2, "averages": {"turns": 3.0}, "samples": {"turns": 2}},
            {"run_count": 1, "averages": {"turns": 5.0}, "samples": {"turns": 1}}
        ]);
        let restored = DashboardReadModel::from_projections(vec![projection.clone()]).unwrap();
        assert_eq!(restored.summaries[0]["totals"]["turns"], 11.0);

        projection.summary["scenario_metrics"][0]["samples"]["turns"] = json!(0);
        let incomplete = DashboardReadModel::from_projections(vec![projection.clone()]).unwrap();
        assert!(incomplete.summaries[0]["totals"]["turns"].is_null());

        projection.summary["scenario_metrics"][0]["samples"]["turns"] = json!(2);
        for metric in projection.summary["scenario_metrics"]
            .as_array_mut()
            .unwrap()
        {
            metric["averages"]["turns"] = json!(0);
        }
        let zero = DashboardReadModel::from_projections(vec![projection]).unwrap();
        assert_eq!(zero.summaries[0]["totals"]["turns"], 0.0);
    }

    #[test]
    fn current_catalog_projects_materialized_realism() {
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
        let spec = chess
            .spec
            .expect("the scoring contract should be projected");

        // The prompt reaches the reader as the subject receives it.
        assert!(spec.prompt.contains("worker-compose.yaml"));
        assert!(spec.prompt.contains("legal_moves"));
        assert!(spec.prompt.contains("console:script"));
        assert!(spec.prompt.contains("#/worker/"));
        assert!(spec
            .summary
            .as_deref()
            .is_some_and(|summary| summary.contains("run-scoped iii Worker")));

        // Weights, policy and the description of every criterion travel with it.
        let weights: Vec<_> = spec
            .criteria
            .iter()
            .map(|criterion| (criterion.id.as_str(), criterion.weight, criterion.policy))
            .collect();
        assert_eq!(
            weights,
            vec![
                ("runtime_contract", 10, AssessmentPolicy::Advisory),
                ("rules_start", 7, AssessmentPolicy::Advisory),
                ("rules_castling", 7, AssessmentPolicy::Advisory),
                ("rules_en_passant", 7, AssessmentPolicy::Advisory),
                ("rules_promotion", 7, AssessmentPolicy::Advisory),
                ("rules_check_evasion", 7, AssessmentPolicy::Advisory),
                ("play_contract", 10, AssessmentPolicy::Advisory),
                ("invalid_inputs", 10, AssessmentPolicy::Advisory),
                ("console_delivery", 15, AssessmentPolicy::Advisory),
                ("playable_ui", 10, AssessmentPolicy::Advisory),
                ("evidence_complete", 10, AssessmentPolicy::Advisory),
            ]
        );
        assert_eq!(
            spec.criteria
                .iter()
                .map(|entry| u32::from(entry.weight))
                .sum::<u32>(),
            100
        );
        assert!(spec.criteria[0].description.contains("run-scoped Worker"));

        // The limits the run answers to are part of the contract, not trivia.
        assert_eq!(spec.execution.max_turns, Some(256));
        assert_eq!(spec.execution.max_output_tokens, Some(65_536));
        assert_eq!(spec.execution.max_total_tokens, Some(6_000_000));
        assert_eq!(spec.execution.stuck_timeout_seconds, 1_800);
        assert!(spec.denied_functions.is_empty());
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
