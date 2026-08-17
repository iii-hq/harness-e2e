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
use crate::assessment::RunAssessmentContract;
use crate::identity::StackIdentity;
use crate::report::{E2eRunReport, E2eScenarioReport, RunStatus};
use crate::scenarios::ScenarioId;

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
    pub available_versions: Vec<VersionDescriptor>,
    pub selected_version: Option<u32>,
    pub result: Option<TestVersionResult>,
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
    duration_seconds: f64,
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
    runs: Vec<RunMetrics>,
}

#[derive(Debug, Clone, Default)]
struct TestVersionEntry {
    observations: Vec<Observation>,
}

#[derive(Debug, Clone, Default)]
struct TestEntry {
    current_version: Option<u32>,
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
            tests: current_tests(),
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

fn current_tests() -> BTreeMap<String, TestEntry> {
    ScenarioId::ALL
        .iter()
        .map(|id| {
            let spec = id.spec("dashboard-catalog");
            (
                id.as_str().to_string(),
                TestEntry {
                    current_version: Some(spec.version),
                    versions: BTreeMap::from([(spec.version, TestVersionEntry::default())]),
                },
            )
        })
        .collect()
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
        duration_seconds: run.wall_time_ms as f64 / 1_000.0,
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
        .map(|run| run.duration_seconds)
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
