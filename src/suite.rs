use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use serde_json::json;
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use crate::artifact;
use crate::assessment::{
    AiAssessmentAvailability, AiFinalAssessment, AnalyzerIdentity, AnalyzerUsage,
    AssessmentOutcome, AssessmentResult, AssessmentScore, AssessmentTarget, AssessmentTargetKind,
    EvidenceReference, FinalAssessmentCleanup, FinalAssessmentExcerpt, FinalAssessmentInput,
    FinalAssessmentMetric, FinalAssessmentSubject, SystemStatus,
};
use crate::asset::{self, AssetCaptureLimits};
use crate::context::E2eContext;
use crate::identity::{self, ExecutionIdentity, SystemUnderTestIdentity};
use crate::judge::{self, JudgeConfig};
use crate::report::{
    CriterionReport, E2eManifest, E2eReport, E2eRunReport, E2eScenarioReport, EvaluationDimension,
    FailurePhase, HardGateReport, ModelArtifact, RetryAttemptReport, RunStatus,
};
use crate::scenarios::common;
use crate::scenarios::{
    CriterionAward, MaterializedScenario, ObjectiveEvaluation, ScenarioCase,
    ScenarioDeliverableCapture, ScenarioId, ScenarioObservation, ScenarioSpec,
};
use crate::wire::{
    ControlPlaneEvidence, FunctionPolicy, MessageInput, Model, SendOptions, SendRequest,
    SendResponse, SessionInit, StatusReport,
};

const MAX_RUNS: u32 = 20;
const MAX_TECHNICAL_RETRIES: u8 = 3;

pub(crate) fn e2e_function_policy(spec: &ScenarioSpec) -> FunctionPolicy {
    let mut deny = vec!["e2e::*".to_string()];
    deny.extend(
        spec.denied_functions
            .iter()
            .map(|function| (*function).to_string()),
    );
    deny.sort();
    deny.dedup();
    FunctionPolicy {
        allow: vec!["*".into()],
        deny,
        ..FunctionPolicy::default()
    }
}

#[derive(Debug, Clone)]
pub struct SubjectConfig {
    pub model: String,
    pub provider: String,
}

pub struct SuiteRunConfig {
    pub url: String,
    pub subject: SubjectConfig,
    pub judge: Option<JudgeConfig>,
    pub output: PathBuf,
    pub scenarios: Vec<ScenarioId>,
    pub runs: u32,
    pub seed: Option<u64>,
    pub rotating_seeds: Vec<u64>,
    pub technical_retries: u8,
    pub progress_interval: Option<Duration>,
    pub control: Option<SuiteControl>,
}

pub struct SuiteRunOutcome {
    pub report: E2eReport,
    pub manifest: E2eManifest,
    pub report_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuitePhase {
    Preflighting,
    Materializing,
    SettingUp,
    Executing,
    Collecting,
    Evaluating,
    Persisting,
    CleaningUp,
    Finalizing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuiteEvent {
    Phase(SuitePhase),
    AttemptStarted {
        scenario_id: ScenarioId,
        attempt_id: String,
        session_id: String,
    },
    AttemptFinished {
        attempt_id: String,
    },
}

pub struct SuiteEventEnvelope {
    pub event: SuiteEvent,
    acknowledgement: oneshot::Sender<std::result::Result<(), String>>,
}

impl SuiteEventEnvelope {
    pub fn acknowledge(self, result: Result<()>) {
        let _ = self
            .acknowledgement
            .send(result.map_err(|error| format!("{error:#}")));
    }
}

#[derive(Clone)]
pub struct SuiteControl {
    pub execution_id: String,
    pub lane: String,
    pub events: mpsc::Sender<SuiteEventEnvelope>,
    pub cancellation: watch::Receiver<bool>,
}

pub async fn run_suite(config: SuiteRunConfig) -> Result<SuiteRunOutcome> {
    validate_config(&config)?;
    emit_phase(config.control.as_ref(), SuitePhase::Preflighting).await?;
    ensure_not_cancelled(config.control.as_ref())?;
    let execution_id = config
        .control
        .as_ref()
        .map(|control| control.execution_id.clone())
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let started_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let context = E2eContext::connect(&config.url)
        .await
        .context("connect E2E runner")?;
    let control_plane = context
        .preflight_control_plane()
        .await
        .context("preflight Harness control-plane contract")?;
    let runtime_versions = context
        .runtime_versions()
        .await
        .context("discover iii and Harness versions")?;
    let system_under_test = SystemUnderTestIdentity::from_environment(
        runtime_versions.engine,
        runtime_versions.harness,
        &control_plane,
    )
    .context("resolve system-under-test identity")?;
    let subject_model = resolve_model(&context, &config.subject.model, &config.subject.provider)
        .await
        .context("resolve subject model")?;
    let judge_model = match config.judge.as_ref() {
        Some(judge) => match resolve_model(&context, &judge.model, &judge.provider).await {
            Ok(model) => Some(model),
            Err(error) => {
                tracing::warn!(
                    provider = judge.provider,
                    model = judge.model,
                    error = %format!("{error:#}"),
                    "configured analyzer is unavailable; preserving execution and recording advisory unavailability"
                );
                None
            }
        },
        None => None,
    };
    emit_phase(config.control.as_ref(), SuitePhase::Materializing).await?;
    let mut scenario_reports = Vec::new();

    for scenario_id in &config.scenarios {
        ensure_not_cancelled(config.control.as_ref())?;
        for seed in case_seeds(*scenario_id, config.seed, &config.rotating_seeds) {
            let definition = scenario_id
                .materialize("validation", seed)
                .with_context(|| format!("materialize scenario {}", scenario_id.as_str()))?;
            preflight_case(&context, &control_plane, &definition.case).await?;
            let mut runs = Vec::with_capacity(config.runs as usize);
            for repetition in 0..config.runs {
                tracing::info!(
                    scenario = scenario_id.as_str(),
                    case_id = definition.case.case_id,
                    seed,
                    run = repetition + 1,
                    total_runs = config.runs,
                    "running E2E quality scenario case"
                );
                let run = run_with_technical_retries(
                    &context,
                    RetryRequest {
                        scenario_id: *scenario_id,
                        subject: &config.subject,
                        judge_config: config.judge.as_ref(),
                        seed,
                        technical_retries: config.technical_retries,
                        progress_interval: config.progress_interval,
                        control: config.control.as_ref(),
                        output: &config.output,
                    },
                )
                .await;
                let stop = run.status.is_technical_failure();
                runs.push(run);
                if stop {
                    tracing::warn!(
                        scenario = scenario_id.as_str(),
                        seed,
                        "stopping case after a technical failure"
                    );
                    break;
                }
            }
            scenario_reports.push(E2eScenarioReport::aggregate_case(
                definition.case,
                definition.spec.execution,
                runs,
            ));
        }
    }

    let completed_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let execution = ExecutionIdentity {
        execution_id,
        lane: config
            .control
            .as_ref()
            .map(|control| control.lane.clone())
            .or_else(|| identity::nonempty_env("HARNESS_E2E_LANE"))
            .unwrap_or_else(|| "local".into()),
        started_at,
        completed_at,
    };
    let subject = ModelArtifact::from(subject_model);
    let judge = judge_model.map(ModelArtifact::from);
    let manifest = E2eManifest {
        execution: execution.clone(),
        system_under_test: system_under_test.clone(),
        subject: subject.clone(),
        judge: judge.clone(),
        control_plane,
    };
    let mut report = E2eReport::new(
        execution,
        system_under_test,
        subject,
        judge,
        config
            .judge
            .as_ref()
            .map(|_| judge::JUDGE_PROTOCOL.to_string()),
        identity::nonempty_env("HARNESS_E2E_ENGINE_REVISION"),
        scenario_reports,
    );
    emit_phase(config.control.as_ref(), SuitePhase::Finalizing).await?;
    ensure_not_cancelled(config.control.as_ref())?;
    // Persist a complete objective result and immutable evidence before invoking
    // the complementary analyzer. A provider failure can therefore never erase
    // the completed execution.
    report.write_to(&config.output, &manifest)?;
    evaluate_final_assessments(&context, config.judge.as_ref(), &config.output, &mut report)
        .await?;
    let report_path = report.write_to(&config.output, &manifest)?;
    context.shutdown().await;
    Ok(SuiteRunOutcome {
        report,
        manifest,
        report_path,
    })
}

const MAX_FINAL_ASSESSMENT_ITEMS: usize = 12;
const MAX_FINAL_ASSESSMENT_TEXT_CHARS: usize = 500;
const MAX_FINAL_ASSESSMENT_EVIDENCE_ITEMS: usize = 4;
const MAX_FINAL_ASSESSMENT_VALUE_ITEMS: usize = 16;
const MAX_FINAL_ASSESSMENT_VALUE_DEPTH: usize = 4;
const MAX_FINAL_ASSESSMENT_VALUE_BYTES: usize = 4 * 1024;

async fn evaluate_final_assessments(
    context: &E2eContext,
    judge_config: Option<&JudgeConfig>,
    output: &std::path::Path,
    report: &mut E2eReport,
) -> Result<()> {
    for scenario_index in 0..report.scenarios.len() {
        for run_index in 0..report.scenarios[scenario_index].runs.len() {
            let (run_id, attempt_id, input) = {
                let scenario = &report.scenarios[scenario_index];
                let run = &scenario.runs[run_index];
                let contract = report
                    .assessment_contract
                    .runs
                    .iter()
                    .find(|candidate| {
                        candidate.run_id == run.run_id && candidate.attempt_id == run.attempt_id
                    })
                    .with_context(|| {
                        format!(
                            "missing preliminary assessment contract for '{}:{}'",
                            run.run_id, run.attempt_id
                        )
                    })?;
                (
                    run.run_id.clone(),
                    run.attempt_id.clone(),
                    final_assessment_input(&report.execution.execution_id, scenario, run, contract),
                )
            };
            if let Err(error) = input.validate() {
                attach_final_assessment(
                    report,
                    scenario_index,
                    run_index,
                    &run_id,
                    &attempt_id,
                    None,
                    0,
                    None,
                    judge_config.is_some(),
                    failed_unprepared_final_assessment(format!(
                        "final_assessment_input_invalid: {error:#}"
                    )),
                )?;
                continue;
            }
            let input_reference = match artifact::write_json(
                output,
                &PathBuf::from("evidence")
                    .join(&run_id)
                    .join(&attempt_id)
                    .join("final-assessment-input.json"),
                "final-assessment-input",
                "final_assessment_input",
                &input,
            ) {
                Ok(reference) => reference,
                Err(error) => {
                    attach_final_assessment(
                        report,
                        scenario_index,
                        run_index,
                        &run_id,
                        &attempt_id,
                        None,
                        0,
                        None,
                        judge_config.is_some(),
                        failed_unprepared_final_assessment(format!(
                            "final_assessment_input_persistence_failed: {error:#}"
                        )),
                    )?;
                    continue;
                }
            };

            let (assessment, attempts, usage) = match judge_config {
                Some(config) => {
                    let outcome = judge::evaluate_final_assessment(context, config, &input).await?;
                    (outcome.assessment, outcome.attempts, outcome.usage)
                }
                None => (unavailable_final_assessment(&input)?, 0, None),
            };

            attach_final_assessment(
                report,
                scenario_index,
                run_index,
                &run_id,
                &attempt_id,
                Some(input_reference),
                attempts,
                usage.as_ref(),
                judge_config.is_some(),
                assessment,
            )?;
        }
        report.scenarios[scenario_index].refresh_aggregate()?;
    }
    report.passed =
        !report.scenarios.is_empty() && report.scenarios.iter().all(|scenario| scenario.passed);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn attach_final_assessment(
    report: &mut E2eReport,
    scenario_index: usize,
    run_index: usize,
    run_id: &str,
    attempt_id: &str,
    input_reference: Option<crate::artifact::ArtifactReference>,
    attempts: u8,
    usage: Option<&crate::report::ModelUsageReport>,
    judge_expected: bool,
    assessment: AiFinalAssessment,
) -> Result<()> {
    {
        let run = &mut report.scenarios[scenario_index].runs[run_index];
        run.final_assessment_input = input_reference;
        accumulate_judge_telemetry(run, attempts, usage);
        run.update_cost(judge_expected);
    }
    report
        .assessment_contract
        .set_final_assessment(run_id, attempt_id, assessment)
}

fn failed_unprepared_final_assessment(reason: String) -> AiFinalAssessment {
    AiFinalAssessment {
        availability: AiAssessmentAvailability::Failed,
        result: None,
        analyzer: None,
        analyzer_usage: None,
        reason: Some(reason),
    }
}

fn unavailable_final_assessment(input: &FinalAssessmentInput) -> Result<AiFinalAssessment> {
    let assessment = AiFinalAssessment {
        availability: AiAssessmentAvailability::Unavailable,
        result: None,
        analyzer: Some(AnalyzerIdentity {
            analyzer: "final-assessment".into(),
            provider: None,
            model: None,
            input_sha256: input.sha256()?,
        }),
        analyzer_usage: None,
        reason: Some(
            "final_assessment_unavailable: no analyzer provider and model were configured".into(),
        ),
    };
    assessment.validate()?;
    Ok(assessment)
}

fn final_assessment_input(
    execution_id: &str,
    scenario: &E2eScenarioReport,
    run: &E2eRunReport,
    contract: &crate::assessment::RunAssessmentContract,
) -> FinalAssessmentInput {
    let mut limitations = vec![
        "Raw transcript content is excluded; only immutable transcript evidence identity is supplied."
            .into(),
        "Generated asset content is excluded; only validated assessment summaries and immutable evidence identities are supplied."
            .into(),
    ];
    let mut assessments = contract
        .assessments
        .iter()
        .take(MAX_FINAL_ASSESSMENT_ITEMS)
        .cloned()
        .map(|mut assessment| {
            assessment.summary = bounded_text(&assessment.summary);
            assessment
                .evidence
                .truncate(MAX_FINAL_ASSESSMENT_EVIDENCE_ITEMS);
            assessment
        })
        .collect::<Vec<_>>();
    if contract.assessments.len() > assessments.len() {
        limitations.push(format!(
            "Only the first {} per-requirement assessments were included.",
            assessments.len()
        ));
    }
    let mut assets = contract
        .assets
        .iter()
        .take(MAX_FINAL_ASSESSMENT_ITEMS)
        .cloned()
        .map(|mut asset| {
            asset.validation.summary = bounded_text(&asset.validation.summary);
            asset
                .validation
                .evidence
                .truncate(MAX_FINAL_ASSESSMENT_EVIDENCE_ITEMS);
            asset.qualitative_assessment.summary =
                bounded_text(&asset.qualitative_assessment.summary);
            asset
                .qualitative_assessment
                .evidence
                .truncate(MAX_FINAL_ASSESSMENT_EVIDENCE_ITEMS);
            asset
        })
        .collect::<Vec<_>>();
    if contract.assets.len() > assets.len() {
        limitations.push(format!(
            "Only the first {} asset assessments were included.",
            assets.len()
        ));
    }
    let dimensions = run
        .dimensions
        .iter()
        .take(MAX_FINAL_ASSESSMENT_ITEMS)
        .cloned()
        .map(|mut dimension| {
            dimension.signals = bounded_json(&dimension.signals, 0);
            dimension
        })
        .collect();
    let failures = run
        .failures
        .iter()
        .take(MAX_FINAL_ASSESSMENT_ITEMS)
        .cloned()
        .map(|mut failure| {
            failure.message = bounded_text(&failure.message);
            failure
        })
        .collect::<Vec<_>>();
    if run.failures.len() > failures.len() {
        limitations.push(format!(
            "Only the first {} execution failures were included.",
            failures.len()
        ));
    }
    let cleanup_failures = failures
        .iter()
        .filter(|failure| failure.phase == FailurePhase::Cleanup)
        .map(|failure| failure.message.clone())
        .collect::<Vec<_>>();
    let excerpts = run
        .evidence
        .iter()
        .filter(|reference| reference.kind == "transcript")
        .take(2)
        .map(|reference| FinalAssessmentExcerpt {
            kind: "transcript".into(),
            summary: "A sanitized transcript is available as immutable evidence; its raw content was not sent to the final analyzer."
                .into(),
            evidence: EvidenceReference::from(reference),
        })
        .collect();

    FinalAssessmentInput {
        subject: FinalAssessmentSubject {
            execution_id: execution_id.to_string(),
            run_id: run.run_id.clone(),
            attempt_id: run.attempt_id.clone(),
            scenario_id: scenario.scenario_id.clone(),
            scenario_version: scenario.scenario_version,
            case_id: scenario.case_id.clone(),
            system_status: SystemStatus::from(run.status),
        },
        assessments: std::mem::take(&mut assessments),
        assets: std::mem::take(&mut assets),
        dimensions,
        failures,
        metrics: final_assessment_metrics(scenario, run),
        cleanup: FinalAssessmentCleanup {
            succeeded: cleanup_failures.is_empty(),
            failures: cleanup_failures,
        },
        excerpts,
        limitations,
    }
}

fn final_assessment_metrics(
    scenario: &E2eScenarioReport,
    run: &E2eRunReport,
) -> Vec<FinalAssessmentMetric> {
    let mut metrics = Vec::new();
    push_final_metric(
        &mut metrics,
        "wall_time",
        Some(run.wall_time_ms as f64),
        "ms",
    );
    push_final_metric(
        &mut metrics,
        "objective_score",
        run.score.map(f64::from),
        "points",
    );
    push_final_metric(&mut metrics, "subject_cost", run.cost.subject_usd, "usd");
    push_final_metric(&mut metrics, "judge_cost", run.cost.judge_usd, "usd");
    if let Some(efficiency) = &run.efficiency {
        push_final_metric(
            &mut metrics,
            "function_calls",
            efficiency.function_calls.map(|value| value as f64),
            "count",
        );
        push_final_metric(
            &mut metrics,
            "function_call_errors",
            efficiency.function_call_errors.map(|value| value as f64),
            "count",
        );
        push_final_metric(
            &mut metrics,
            "validation_retries",
            efficiency.validation_retries.map(|value| value as f64),
            "count",
        );
        push_final_metric(
            &mut metrics,
            "work_amplification",
            efficiency.work_amplification,
            "ratio",
        );
        push_final_metric(
            &mut metrics,
            "technical_attempts",
            Some(f64::from(efficiency.technical_attempts)),
            "count",
        );
    }
    let robustness = &scenario.aggregate.robustness;
    push_final_metric(
        &mut metrics,
        "robustness_sample_size",
        Some(f64::from(robustness.sample_size)),
        "count",
    );
    push_final_metric(
        &mut metrics,
        "technical_failure_rate",
        robustness.technical_failure_rate,
        "ratio",
    );
    push_final_metric(&mut metrics, "flaky_rate", robustness.flaky_rate, "ratio");
    metrics
}

fn push_final_metric(
    metrics: &mut Vec<FinalAssessmentMetric>,
    id: &str,
    value: Option<f64>,
    unit: &str,
) {
    if let Some(value) = value.filter(|value| value.is_finite()) {
        metrics.push(FinalAssessmentMetric {
            id: id.into(),
            value,
            unit: unit.into(),
        });
    }
}

fn bounded_text(value: &str) -> String {
    let mut bounded = value
        .chars()
        .take(MAX_FINAL_ASSESSMENT_TEXT_CHARS)
        .collect::<String>();
    if value.chars().count() > MAX_FINAL_ASSESSMENT_TEXT_CHARS {
        bounded.push('…');
    }
    bounded
}

fn bounded_json(value: &serde_json::Value, depth: usize) -> serde_json::Value {
    let bounded = bounded_json_inner(value, depth);
    if serde_json::to_vec(&bounded)
        .is_ok_and(|encoded| encoded.len() <= MAX_FINAL_ASSESSMENT_VALUE_BYTES)
    {
        bounded
    } else {
        serde_json::json!({
            "omitted": true,
            "reason": "bounded final assessment signal exceeded 4096 bytes",
            "sha256": artifact::sha256_value(value).ok(),
        })
    }
}

fn bounded_json_inner(value: &serde_json::Value, depth: usize) -> serde_json::Value {
    if depth >= MAX_FINAL_ASSESSMENT_VALUE_DEPTH {
        return serde_json::Value::String("[omitted at depth limit]".into());
    }
    match value {
        serde_json::Value::String(value) => serde_json::Value::String(bounded_text(value)),
        serde_json::Value::Array(values) => serde_json::Value::Array(
            values
                .iter()
                .take(MAX_FINAL_ASSESSMENT_VALUE_ITEMS)
                .map(|value| bounded_json_inner(value, depth + 1))
                .collect(),
        ),
        serde_json::Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .take(MAX_FINAL_ASSESSMENT_VALUE_ITEMS)
                    .map(|(key, value)| (key.clone(), bounded_json_inner(value, depth + 1)))
                    .collect(),
            )
        }
        value => value.clone(),
    }
}

async fn preflight_case(
    context: &E2eContext,
    expected: &ControlPlaneEvidence,
    case: &ScenarioCase,
) -> Result<()> {
    let observed = context.preflight_control_plane().await.with_context(|| {
        format!(
            "preflight Harness control-plane contract before case {}",
            case.case_id
        )
    })?;
    ensure_control_plane_unchanged(expected, &observed).with_context(|| {
        format!(
            "control-plane contract changed before case {}",
            case.case_id
        )
    })
}

fn ensure_control_plane_unchanged(
    expected: &ControlPlaneEvidence,
    observed: &ControlPlaneEvidence,
) -> Result<()> {
    let expected_sha = crate::artifact::sha256_value(expected)?;
    let observed_sha = crate::artifact::sha256_value(observed)?;
    if expected_sha != observed_sha {
        bail!(
            "control-plane fingerprint changed from {expected_sha} to {observed_sha} during the suite"
        );
    }
    Ok(())
}

async fn emit_phase(control: Option<&SuiteControl>, phase: SuitePhase) -> Result<()> {
    emit_event(control, SuiteEvent::Phase(phase)).await
}

async fn emit_event(control: Option<&SuiteControl>, event: SuiteEvent) -> Result<()> {
    let Some(control) = control else {
        return Ok(());
    };
    let (acknowledgement, received) = oneshot::channel();
    control
        .events
        .send(SuiteEventEnvelope {
            event,
            acknowledgement,
        })
        .await
        .context("publish E2E execution checkpoint")?;
    received
        .await
        .context("E2E checkpoint receiver stopped")?
        .map_err(anyhow::Error::msg)
}

fn ensure_not_cancelled(control: Option<&SuiteControl>) -> Result<()> {
    if control.is_some_and(|control| *control.cancellation.borrow()) {
        bail!("E2E execution was cancelled");
    }
    Ok(())
}

fn validate_config(config: &SuiteRunConfig) -> Result<()> {
    for (name, value) in [
        ("model", config.subject.model.as_str()),
        ("provider", config.subject.provider.as_str()),
    ] {
        if value.trim().is_empty() {
            bail!("{name} cannot be empty");
        }
    }
    if !(1..=MAX_RUNS).contains(&config.runs) {
        bail!("runs must be between 1 and {MAX_RUNS}");
    }
    if config.technical_retries > MAX_TECHNICAL_RETRIES {
        bail!("technical retries must be between 0 and {MAX_TECHNICAL_RETRIES}");
    }
    if config.scenarios.is_empty() {
        bail!("at least one scenario is required");
    }
    for scenario in &config.scenarios {
        let seed = config.seed.unwrap_or_else(|| scenario.canonical_seed());
        scenario.materialize("validation", seed)?;
    }
    if let Some(judge) = &config.judge {
        if judge.model.trim().is_empty() || judge.provider.trim().is_empty() {
            bail!("judge model and provider cannot be empty");
        }
    }
    Ok(())
}

fn case_seeds(scenario: ScenarioId, fixed: Option<u64>, rotating: &[u64]) -> Vec<u64> {
    let mut seeds = vec![fixed.unwrap_or_else(|| scenario.canonical_seed())];
    for seed in rotating {
        if !seeds.contains(seed) {
            seeds.push(*seed);
        }
    }
    seeds
}

async fn resolve_model(context: &E2eContext, model: &str, provider: &str) -> Result<Model> {
    let response = context
        .trigger_value(
            "router::models::get",
            json!({ "id": model, "provider": provider }),
        )
        .await
        .with_context(|| format!("query catalog for {provider}/{model}"))?;
    if response.is_null() {
        bail!("model {provider}/{model} is not registered in the router catalog");
    }
    let resolved: Model = serde_json::from_value(
        response
            .get("model")
            .cloned()
            .context("router::models::get response is missing model")?,
    )
    .context("decode router catalog model")?;
    if resolved.id != model || resolved.provider != provider {
        bail!(
            "catalog resolved {provider}/{model} as {}/{}; exact model identity is required",
            resolved.provider,
            resolved.id
        );
    }
    let stream_function = format!("provider::{provider}::stream");
    if !context
        .function_exists(&stream_function)
        .await
        .with_context(|| format!("check live provider function {stream_function}"))?
    {
        bail!(
            "model {provider}/{model} is present in the catalog, but provider {provider} is not \
             running; missing function {stream_function}"
        );
    }
    Ok(resolved)
}

struct AttemptRequest<'a> {
    scenario_id: ScenarioId,
    run_id: &'a str,
    attempt_number: u32,
    subject: &'a SubjectConfig,
    judge_config: Option<&'a JudgeConfig>,
    seed: u64,
    progress_interval: Option<Duration>,
    control: Option<&'a SuiteControl>,
    output: &'a std::path::Path,
}

async fn run_once(context: &E2eContext, request: AttemptRequest<'_>) -> E2eRunReport {
    let AttemptRequest {
        scenario_id,
        run_id,
        attempt_number,
        subject,
        judge_config,
        seed,
        progress_interval,
        control,
        output,
    } = request;
    let started = Instant::now();
    let attempt_id = Uuid::new_v4().simple().to_string();
    let session_id = format!("e2e_{attempt_id}");
    let materialized = match scenario_id.materialize(&attempt_id, seed) {
        Ok(materialized) => materialized,
        Err(error) => {
            let spec = scenario_id.spec(&attempt_id);
            let mut report = E2eRunReport::new(
                run_id.to_string(),
                attempt_id,
                attempt_number,
                session_id,
                spec.prompt.clone(),
            );
            report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Setup,
                format!("scenario materialization failed: {error:#}"),
            );
            report.wall_time_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            ensure_assessment_results(&spec, &mut report);
            report.refresh_dimensions(false);
            return report;
        }
    };
    let MaterializedScenario {
        spec,
        case,
        capture,
    } = materialized;
    let expects_deliverables = !case.deliverable_contract.artifacts.is_empty();
    let mut report = E2eRunReport::new(
        run_id.to_string(),
        attempt_id.clone(),
        attempt_number,
        session_id.clone(),
        spec.prompt.clone(),
    );

    if let Err(error) = emit_event(
        control,
        SuiteEvent::AttemptStarted {
            scenario_id,
            attempt_id: attempt_id.clone(),
            session_id: session_id.clone(),
        },
    )
    .await
    {
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Setup,
            format!("persist attempt checkpoint: {error:#}"),
        );
        ensure_assessment_results(&spec, &mut report);
        report.refresh_dimensions(expects_deliverables);
        return report;
    }
    if let Err(error) = emit_phase(control, SuitePhase::SettingUp).await {
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Setup,
            format!("persist setup checkpoint: {error:#}"),
        );
    }

    if report.failures.is_empty() {
        if let Err(error) = execute(
            context,
            ExecutionRequest {
                subject,
                judge_config,
                run_id: &attempt_id,
                session_id: &session_id,
                spec: &spec,
                case: &case,
                capture,
                progress_interval,
                control,
                output,
            },
            &mut report,
        )
        .await
        {
            report.push_failure(error.status, error.phase, error.message);
        }
    }

    if let Err(error) = emit_phase(control, SuitePhase::Persisting).await {
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Collect,
            format!("persist evaluation checkpoint: {error:#}"),
        );
    }
    if let Err(error) = emit_phase(control, SuitePhase::CleaningUp).await {
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Cleanup,
            format!("persist cleanup checkpoint: {error:#}"),
        );
    }
    if let Err(error) = context.unbind_turn_completed().await {
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Cleanup,
            format!(
                "scenario '{}': unbind harness::turn-completed failed: {error:#}",
                spec.id
            ),
        );
    }
    if let Err(error) = context.teardown(&session_id).await {
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Cleanup,
            format!(
                "scenario '{}': harness::teardown failed: {error:#}",
                spec.id
            ),
        );
    }
    if let Some(cleanup) = spec.cleanup {
        if let Err(error) = cleanup(context, &attempt_id).await {
            report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Cleanup,
                format!("scenario '{}': scenario cleanup failed: {error:#}", spec.id),
            );
        }
    }
    asset::reconcile_after_cleanup(output, &report.deliverables, &mut report.asset_assessments);
    if let Some(capture_manifest) = report.asset_capture_manifest.clone() {
        match asset::persist_after_cleanup(output, &capture_manifest, &report.asset_assessments) {
            Ok(reconciliation_manifest) => {
                report.asset_capture_manifest = Some(reconciliation_manifest.clone());
                report.evidence.push(reconciliation_manifest);
            }
            Err(error) => report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Cleanup,
                format!("persist post-cleanup asset reconciliation: {error:#}"),
            ),
        }
    }
    if report
        .asset_assessments
        .iter()
        .any(|asset| asset.validation.outcome != crate::assessment::AssetValidationOutcome::Valid)
    {
        if let Some(gate) = report
            .hard_gates
            .iter_mut()
            .find(|gate| gate.id == "deliverable_contract")
        {
            gate.passed = false;
            gate.reason =
                "captured asset evidence did not survive deterministic validation and cleanup"
                    .into();
        }
    }
    report.wall_time_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    ensure_assessment_results(&spec, &mut report);
    if report.failures.is_empty() {
        if report.hard_gates.iter().any(|gate| !gate.passed) {
            // Judge-backed scenarios skip the judge on a gate failure, leaving no
            // criterion awards; the run must still enter the aggregate as a score.
            report.score.get_or_insert(0);
            report.finish(RunStatus::HardGateFailed);
        } else if report.score.is_some()
            || report.assessment_results.iter().all(|assessment| {
                assessment.policy == crate::assessment::AssessmentPolicy::Advisory
            })
        {
            report.finish(RunStatus::Passed);
        } else {
            report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Evaluate,
                format!(
                    "scenario '{}': evaluation completed without a score; expected criterion awards or a judge score",
                    spec.id
                ),
            );
        }
    }
    report.update_cost(
        spec.needs_judge() || (!report.asset_assessments.is_empty() && judge_config.is_some()),
    );
    report.update_efficiency(case.work);
    report.refresh_dimensions(expects_deliverables);
    if let Err(error) = emit_event(
        control,
        SuiteEvent::AttemptFinished {
            attempt_id: attempt_id.clone(),
        },
    )
    .await
    {
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Cleanup,
            format!("persist attempt completion: {error:#}"),
        );
        report.refresh_dimensions(expects_deliverables);
    }
    report
}

struct RetryRequest<'a> {
    scenario_id: ScenarioId,
    subject: &'a SubjectConfig,
    judge_config: Option<&'a JudgeConfig>,
    seed: u64,
    technical_retries: u8,
    progress_interval: Option<Duration>,
    control: Option<&'a SuiteControl>,
    output: &'a std::path::Path,
}

async fn run_with_technical_retries(
    context: &E2eContext,
    request: RetryRequest<'_>,
) -> E2eRunReport {
    let RetryRequest {
        scenario_id,
        subject,
        judge_config,
        seed,
        technical_retries,
        progress_interval,
        control,
        output,
    } = request;
    let run_id = Uuid::new_v4().simple().to_string();
    let mut retry_attempts = Vec::with_capacity(technical_retries as usize);
    loop {
        let attempt_number = retry_attempts.len() as u32 + 1;
        let mut report = run_once(
            context,
            AttemptRequest {
                scenario_id,
                run_id: &run_id,
                attempt_number,
                subject,
                judge_config,
                seed,
                progress_interval,
                control,
                output,
            },
        )
        .await;
        if retry_attempts.len() < technical_retries as usize
            && is_retryable_technical_failure(&report)
            && control.is_none_or(|control| !*control.cancellation.borrow())
        {
            let reason = report
                .failures
                .first()
                .map(|failure| failure.message.as_str())
                .unwrap_or("transient technical failure");
            tracing::warn!(
                scenario = scenario_id.as_str(),
                attempt = retry_attempts.len() + 1,
                max_retries = technical_retries,
                reason,
                "retrying E2E scenario after a transient technical failure"
            );
            retry_attempts.push(RetryAttemptReport::from(&report));
            continue;
        }
        report.attach_retry_attempts(retry_attempts);
        return report;
    }
}

struct RunFailure {
    status: RunStatus,
    phase: FailurePhase,
    message: String,
}

struct ExecutionRequest<'a> {
    subject: &'a SubjectConfig,
    judge_config: Option<&'a JudgeConfig>,
    run_id: &'a str,
    session_id: &'a str,
    spec: &'a ScenarioSpec,
    case: &'a ScenarioCase,
    capture: Option<ScenarioDeliverableCapture>,
    progress_interval: Option<Duration>,
    control: Option<&'a SuiteControl>,
    output: &'a std::path::Path,
}

impl RunFailure {
    fn new(status: RunStatus, phase: FailurePhase, message: impl Into<String>) -> Self {
        Self {
            status,
            phase,
            message: message.into(),
        }
    }
}

async fn execute(
    context: &E2eContext,
    request: ExecutionRequest<'_>,
    report: &mut E2eRunReport,
) -> Result<(), RunFailure> {
    let ExecutionRequest {
        subject,
        judge_config,
        run_id,
        session_id,
        spec,
        case,
        capture,
        progress_interval,
        control,
        output,
    } = request;
    let stuck_timeout = Duration::from_secs(spec.execution.stuck_timeout_seconds);
    let filesystem_metadata = prepare_filesystem_root(spec)?;
    if let Some(setup) = spec.setup {
        setup(context, run_id).await.map_err(|error| {
            subject_failure(
                FailurePhase::Execute,
                format!("scenario setup failed: {error}"),
            )
        })?;
    }
    emit_phase(control, SuitePhase::Executing)
        .await
        .map_err(|error| infrastructure_failure(FailurePhase::Execute, error.to_string()))?;
    ensure_not_cancelled(control)
        .map_err(|error| infrastructure_failure(FailurePhase::Execute, error.to_string()))?;
    context
        .bind_turn_completed()
        .await
        .map_err(|error| infrastructure_failure(FailurePhase::Execute, error.to_string()))?;
    let response: SendResponse = context
        .trigger(
            "harness::send",
            SendRequest {
                session_id: Some(session_id.to_string()),
                message: MessageInput::Text(spec.prompt.clone()),
                model: Some(subject.model.clone()),
                provider: Some(subject.provider.clone()),
                idempotency_key: Some(format!("e2e:{run_id}:{}:send", spec.id)),
                session: Some(SessionInit {
                    title: Some(format!("Harness E2E: {}", spec.id)),
                    metadata: Some(json!({
                        "e2e_run_id": run_id,
                        "e2e_scenario": spec.id,
                    })),
                }),
                options: Some(SendOptions {
                    max_turns: Some(spec.execution.max_turns),
                    max_output_tokens: spec.execution.max_output_tokens,
                    max_total_tokens: Some(spec.execution.max_total_tokens),
                    functions: Some(e2e_function_policy(spec)),
                    metadata: filesystem_metadata,
                }),
            },
        )
        .await
        .map_err(|error| subject_failure(FailurePhase::Execute, error.to_string()))?;
    if !response.accepted
        || response.session_id != session_id
        || response.merged == Some(true)
        || response.queued == Some(true)
    {
        return Err(RunFailure::new(
            RunStatus::SubjectError,
            FailurePhase::Execute,
            format!("harness::send returned an unexpected response: {response:?}"),
        ));
    }

    let metrics = match context
        .wait_for_tree(
            spec.id,
            session_id,
            stuck_timeout,
            progress_interval.is_some(),
            control.map(|control| &control.cancellation),
        )
        .await
    {
        Ok(metrics) => metrics,
        Err(error) => {
            capture_partial_observation(context, session_id, report).await;
            return Err(subject_failure(FailurePhase::Execute, error.to_string()));
        }
    };
    let terminal_status = match context
        .trigger::<_, Option<StatusReport>>("harness::status", json!({ "session_id": session_id }))
        .await
    {
        Ok(Some(status)) => status,
        Ok(None) => {
            capture_partial_observation(context, session_id, report).await;
            return Err(collection_failure(
                FailurePhase::Collect,
                format!("harness::status returned no report for {session_id}"),
            ));
        }
        Err(error) => {
            capture_partial_observation(context, session_id, report).await;
            return Err(collection_failure(FailurePhase::Collect, error.to_string()));
        }
    };
    report.terminal_status = Some(terminal_status);
    emit_phase(control, SuitePhase::Collecting)
        .await
        .map_err(|error| infrastructure_failure(FailurePhase::Collect, error.to_string()))?;
    let transcript = context.transcript(session_id).await.map_err(|error| {
        RunFailure::new(
            RunStatus::InfrastructureError,
            FailurePhase::Collect,
            error.to_string(),
        )
    })?;
    let response = common::final_response(&transcript);
    let mut observation = ScenarioObservation {
        case: case.clone(),
        metrics,
        transcript,
        response,
        deliverables: Vec::new(),
    };
    report.transcript = Some(observation.transcript.clone());
    report.metrics = Some(observation.metrics.clone());
    if let Some(capture) = capture {
        let captured = match capture(context, &observation, run_id).await {
            Ok(captured) => captured,
            Err(error) => {
                let mut message = format!(
                    "scenario '{}' asset capture was unreadable: {error:#}",
                    spec.id
                );
                let mut evaluation = asset::failed_capture_evaluation(
                    case,
                    crate::assessment::AssetValidationOutcome::Unreadable,
                    &message,
                );
                match asset::persist_before_cleanup(
                    output,
                    &report.run_id,
                    &report.attempt_id,
                    &mut evaluation,
                ) {
                    Ok(manifest) => {
                        report.asset_capture_manifest = Some(manifest.clone());
                        report.evidence.push(manifest);
                    }
                    Err(persist_error) => {
                        message.push_str(&format!(
                            "; persist unreadable asset inventory: {persist_error:#}"
                        ));
                    }
                }
                report.asset_assessments = evaluation.assessments;
                report.asset_redaction.merge(evaluation.redaction);
                return Err(RunFailure::new(
                    RunStatus::InfrastructureError,
                    FailurePhase::Collect,
                    message,
                ));
            }
        };
        let mut evaluation =
            asset::evaluate_assets(case, captured.clone(), AssetCaptureLimits::default()).map_err(
                |error| {
                    RunFailure::new(
                        RunStatus::InfrastructureError,
                        FailurePhase::Evaluate,
                        format!(
                            "scenario '{}' deterministic asset validation failed: {error:#}",
                            spec.id
                        ),
                    )
                },
            )?;
        let manifest = match asset::persist_before_cleanup(
            output,
            &report.run_id,
            &report.attempt_id,
            &mut evaluation,
        ) {
            Ok(manifest) => manifest,
            Err(error) => {
                report.deliverables = evaluation.deliverables;
                report.asset_assessments = evaluation.assessments;
                report.asset_redaction.merge(evaluation.redaction);
                return Err(RunFailure::new(
                    RunStatus::InfrastructureError,
                    FailurePhase::Collect,
                    format!(
                        "scenario '{}' persist asset evidence before cleanup: {error:#}",
                        spec.id
                    ),
                ));
            }
        };
        report.deliverables = evaluation.deliverables;
        report.asset_assessments = evaluation.assessments;
        report.asset_redaction.merge(evaluation.redaction);
        report.asset_capture_manifest = Some(manifest.clone());
        report.evidence.push(manifest);
        observation.deliverables = captured;
    }
    emit_phase(control, SuitePhase::Evaluating)
        .await
        .map_err(|error| infrastructure_failure(FailurePhase::Evaluate, error.to_string()))?;
    let mut objective = (spec.evaluate)(context, &observation, run_id)
        .await
        .map_err(|error| {
            RunFailure::new(
                RunStatus::InfrastructureError,
                FailurePhase::Evaluate,
                format!("scenario '{}' evaluator failed: {error:#}", spec.id),
            )
        })?;
    if !case.deliverable_contract.artifacts.is_empty() {
        let passed = !report.asset_assessments.is_empty()
            && report.asset_assessments.iter().all(|asset| {
                asset.validation.outcome == crate::assessment::AssetValidationOutcome::Valid
            });
        objective.hard_gates.push(HardGateReport {
            id: "deliverable_contract".to_string(),
            dimension: EvaluationDimension::Deliverable,
            passed,
            reason: format!(
                "captured {} asset validation result(s); deterministic contract valid={passed}",
                report.asset_assessments.len()
            ),
        });
    }
    validate_objective_evaluation(spec, &objective).map_err(|error| {
        RunFailure::new(
            RunStatus::InfrastructureError,
            FailurePhase::Evaluate,
            error.to_string(),
        )
    })?;
    report.hard_gates = objective.hard_gates;
    let mut awards = objective.awards;
    let mut criterion_judge = CriterionJudgeState::NotRequested;

    if spec.needs_judge() && report.hard_gates.iter().all(|gate| gate.passed) {
        match judge_config {
            None => {
                criterion_judge = CriterionJudgeState::Unavailable(
                    "judge_unavailable: no judge provider and model were configured",
                );
            }
            Some(judge_config) => {
                match judge::evaluate(context, judge_config, spec, &observation.response)
                    .await
                    .map_err(|error| {
                        RunFailure::new(
                            RunStatus::InfrastructureError,
                            FailurePhase::Evaluate,
                            format!("scenario '{}' prepare judge evaluation: {error:#}", spec.id),
                        )
                    })? {
                    judge::JudgeEvaluation::Completed(outcome) => {
                        let judge::JudgeOutcome {
                            awards: judge_awards,
                            confidences,
                            attempts,
                            usage,
                            analyzer,
                            analyzer_usage,
                        } = outcome;
                        accumulate_judge_telemetry(report, attempts, usage.as_ref());
                        awards = judge_awards;
                        criterion_judge = CriterionJudgeState::Completed {
                            confidences,
                            analyzer,
                            usage: analyzer_usage,
                        };
                    }
                    judge::JudgeEvaluation::Failed(failure) => {
                        accumulate_judge_telemetry(
                            report,
                            failure.attempts,
                            failure.usage.as_ref(),
                        );
                        criterion_judge = CriterionJudgeState::Failed(failure);
                    }
                }
            }
        }
    } else if spec.needs_judge() {
        criterion_judge = CriterionJudgeState::Unavailable(
            "judge_not_evaluated: objective hard gate failure prevented advisory judging",
        );
    }

    report.criteria = criterion_reports(spec, awards);
    report.assessment_results = materialize_assessment_results(
        spec,
        &report.criteria,
        &report.hard_gates,
        &criterion_judge,
    );
    update_score(report);

    if !report.asset_assessments.is_empty() {
        match judge_config {
            Some(judge_config) => {
                let outcome = judge::evaluate_asset_quality(
                    context,
                    judge_config,
                    &report.deliverables,
                    &report.asset_assessments,
                )
                .await
                .map_err(|error| {
                    RunFailure::new(
                        RunStatus::InfrastructureError,
                        FailurePhase::Evaluate,
                        format!("scenario '{}' prepare asset judge: {error:#}", spec.id),
                    )
                })?;
                for (asset, qualitative) in report.asset_assessments.iter_mut().zip(outcome.results)
                {
                    asset.qualitative_assessment = qualitative;
                }
                accumulate_judge_telemetry(report, outcome.attempts, outcome.usage.as_ref());
            }
            None => mark_asset_judge_unavailable(
                &mut report.asset_assessments,
                "judge_unavailable: no judge provider and model were configured",
            ),
        }
    }
    Ok(())
}

enum CriterionJudgeState {
    NotRequested,
    Unavailable(&'static str),
    Completed {
        confidences: HashMap<String, f64>,
        analyzer: AnalyzerIdentity,
        usage: AnalyzerUsage,
    },
    Failed(judge::JudgeFailure),
}

fn ensure_assessment_results(spec: &ScenarioSpec, report: &mut E2eRunReport) {
    let declared = spec.declared_assessments();
    let complete = report.assessment_results.len() == declared.len()
        && report
            .assessment_results
            .iter()
            .zip(&declared)
            .all(|(result, declaration)| result.criterion_id == declaration.criterion_id);
    if complete {
        return;
    }
    let reason = report
        .failures
        .first()
        .map(|failure| format!("assessment_not_evaluated: {}", failure.message))
        .unwrap_or_else(|| {
            "assessment_not_evaluated: execution did not reach assessment materialization".into()
        });
    report.assessment_results = materialize_assessment_results(
        spec,
        &report.criteria,
        &report.hard_gates,
        &CriterionJudgeState::NotRequested,
    );
    for result in &mut report.assessment_results {
        result.outcome = AssessmentOutcome::NotEvaluated;
        result.score = None;
        result.confidence = None;
        result.summary = reason.clone();
        result.analyzer = None;
        result.analyzer_usage = None;
    }
}

fn materialize_assessment_results(
    spec: &ScenarioSpec,
    criteria: &[CriterionReport],
    hard_gates: &[HardGateReport],
    judge_state: &CriterionJudgeState,
) -> Vec<AssessmentResult> {
    spec.declared_assessments()
        .into_iter()
        .map(|declaration| {
            let criterion = criteria
                .iter()
                .find(|criterion| criterion.id == declaration.criterion_id);
            let hard_gate_failed = hard_gates
                .iter()
                .any(|gate| gate.id == declaration.criterion_id && !gate.passed);
            let (outcome, score, confidence, summary, analyzer, analyzer_usage) = if declaration
                .source
                == crate::assessment::AssessmentSource::Judge
            {
                match judge_state {
                    CriterionJudgeState::Completed {
                        confidences,
                        analyzer,
                        usage,
                    } => {
                        let awarded = criterion.and_then(|criterion| criterion.awarded);
                        (
                            score_assessment_outcome(
                                awarded,
                                declaration.possible,
                                hard_gate_failed,
                            ),
                            awarded.map(|awarded| AssessmentScore {
                                awarded,
                                possible: declaration.possible,
                            }),
                            confidences.get(&declaration.criterion_id).copied(),
                            criterion
                                .map(|criterion| criterion.reason.clone())
                                .unwrap_or_else(|| "Judge returned no criterion result.".into()),
                            Some(analyzer.clone()),
                            Some(usage.clone()),
                        )
                    }
                    CriterionJudgeState::Failed(failure) => (
                        failure.kind.outcome(),
                        None,
                        None,
                        failure.summary(),
                        Some(failure.analyzer.clone()),
                        Some(failure.analyzer_usage.clone()),
                    ),
                    CriterionJudgeState::Unavailable(reason) => (
                        AssessmentOutcome::Unavailable,
                        None,
                        None,
                        (*reason).to_string(),
                        None,
                        None,
                    ),
                    CriterionJudgeState::NotRequested => (
                        AssessmentOutcome::NotEvaluated,
                        None,
                        None,
                        "Judge assessment was not requested.".into(),
                        None,
                        None,
                    ),
                }
            } else {
                let awarded = criterion.and_then(|criterion| criterion.awarded);
                (
                    score_assessment_outcome(awarded, declaration.possible, hard_gate_failed),
                    awarded.map(|awarded| AssessmentScore {
                        awarded,
                        possible: declaration.possible,
                    }),
                    None,
                    criterion
                        .map(|criterion| criterion.reason.clone())
                        .unwrap_or_else(|| "Deterministic assessment was not evaluated.".into()),
                    None,
                    None,
                )
            };
            AssessmentResult {
                criterion_id: declaration.criterion_id.clone(),
                target: AssessmentTarget {
                    kind: AssessmentTargetKind::Criterion,
                    id: declaration.criterion_id,
                },
                kind: declaration.kind,
                policy: declaration.policy,
                dimension: declaration.dimension,
                source: declaration.source,
                outcome,
                score,
                confidence,
                summary,
                evidence: Vec::new(),
                analyzer,
                analyzer_usage,
            }
        })
        .collect()
}

fn score_assessment_outcome(
    awarded: Option<u8>,
    possible: u8,
    hard_gate_failed: bool,
) -> AssessmentOutcome {
    match awarded {
        None => AssessmentOutcome::NotEvaluated,
        Some(_) if hard_gate_failed => AssessmentOutcome::Failed,
        Some(awarded) if awarded == possible => AssessmentOutcome::Passed,
        Some(0) => AssessmentOutcome::Failed,
        Some(_) => AssessmentOutcome::Partial,
    }
}

fn mark_asset_judge_unavailable(
    assessments: &mut [crate::assessment::AssetAssessmentResult],
    reason: &str,
) {
    for asset in assessments {
        if asset.validation.evidence.is_empty() {
            asset.qualitative_assessment.outcome = AssessmentOutcome::NotEvaluated;
            asset.qualitative_assessment.summary =
                "Asset quality was not evaluated because no immutable content evidence was captured."
                    .into();
            continue;
        }
        asset.qualitative_assessment.outcome = AssessmentOutcome::Unavailable;
        asset.qualitative_assessment.score = None;
        asset.qualitative_assessment.confidence = None;
        asset.qualitative_assessment.summary = reason.to_string();
        asset.qualitative_assessment.evidence = asset.validation.evidence.clone();
        asset.qualitative_assessment.analyzer = None;
        asset.qualitative_assessment.analyzer_usage = None;
    }
}

fn accumulate_judge_telemetry(
    report: &mut E2eRunReport,
    attempts: u8,
    usage: Option<&crate::report::ModelUsageReport>,
) {
    if attempts > 0 {
        report.judge_attempts = Some(report.judge_attempts.unwrap_or(0).saturating_add(attempts));
    }
    let Some(usage) = usage else {
        return;
    };
    report.judge_usage = Some(match report.judge_usage.take() {
        None => usage.clone(),
        Some(existing) => crate::report::ModelUsageReport {
            input_tokens: sum_usage(existing.input_tokens, usage.input_tokens),
            output_tokens: sum_usage(existing.output_tokens, usage.output_tokens),
            cache_read_tokens: sum_usage(existing.cache_read_tokens, usage.cache_read_tokens),
            cache_write_tokens: sum_usage(existing.cache_write_tokens, usage.cache_write_tokens),
            reasoning_tokens: sum_usage(existing.reasoning_tokens, usage.reasoning_tokens),
            cost_usd: existing
                .cost_usd
                .zip(usage.cost_usd)
                .map(|(left, right)| left + right),
        },
    });
}

fn sum_usage(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    left.zip(right)
        .and_then(|(left, right)| left.checked_add(right))
}

fn prepare_filesystem_root(spec: &ScenarioSpec) -> Result<Option<serde_json::Value>, RunFailure> {
    let Some(root) = spec.filesystem_root.as_ref() else {
        return Ok(None);
    };
    if !root.is_absolute() {
        return Err(RunFailure::new(
            RunStatus::InfrastructureError,
            FailurePhase::Execute,
            format!(
                "scenario {} filesystem root must be absolute: {}",
                spec.id,
                root.display()
            ),
        ));
    }
    std::fs::create_dir_all(root).map_err(|error| {
        RunFailure::new(
            RunStatus::InfrastructureError,
            FailurePhase::Execute,
            format!(
                "create scenario {} filesystem root {}: {error}",
                spec.id,
                root.display()
            ),
        )
    })?;
    let root = root.to_str().ok_or_else(|| {
        RunFailure::new(
            RunStatus::InfrastructureError,
            FailurePhase::Execute,
            format!(
                "scenario {} filesystem root is not valid UTF-8: {}",
                spec.id,
                root.display()
            ),
        )
    })?;
    Ok(Some(json!({ "fs_scope": { "root": root } })))
}

async fn capture_partial_observation(
    context: &E2eContext,
    session_id: &str,
    report: &mut E2eRunReport,
) {
    match context.metrics(session_id).await {
        Ok(metrics) => report.metrics = Some(metrics),
        Err(error) => tracing::warn!(
            session_id,
            %error,
            "could not capture partial E2E metrics"
        ),
    }
    match context.transcript(session_id).await {
        Ok(transcript) => report.transcript = Some(transcript),
        Err(error) => tracing::warn!(
            session_id,
            %error,
            "could not capture partial E2E transcript"
        ),
    }
}

fn is_resource_limit(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "token budget",
        "tokens remain",
        "max_total_tokens",
        "cost budget",
        "scenario exceeded",
        "no observable progress",
        "maximum turn",
        "turn limit",
        "context length",
        "input limit",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn is_retryable_technical_failure(report: &E2eRunReport) -> bool {
    if !matches!(
        report.status,
        RunStatus::SubjectError | RunStatus::JudgeError | RunStatus::InfrastructureError
    ) || report
        .failures
        .iter()
        .any(|failure| failure.phase == FailurePhase::Cleanup)
    {
        return false;
    }
    report.failures.iter().any(|failure| {
        let lower = failure.message.to_ascii_lowercase();
        [
            "stream ended without",
            "terminal frame",
            "connection reset",
            "connection closed",
            "broken pipe",
            "temporarily unavailable",
            "service unavailable",
            "rate limit",
            "too many requests",
            "http 429",
            "status 429",
            "status 502",
            "status 503",
            "status 504",
            "transport error",
            "network error",
            "timed out",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    })
}

fn subject_failure(phase: FailurePhase, message: String) -> RunFailure {
    let status = if is_resource_limit(&message) {
        RunStatus::ResourceLimit
    } else {
        RunStatus::SubjectError
    };
    RunFailure::new(status, phase, message)
}

fn collection_failure(phase: FailurePhase, message: String) -> RunFailure {
    let status = if is_resource_limit(&message) {
        RunStatus::ResourceLimit
    } else {
        RunStatus::InfrastructureError
    };
    RunFailure::new(status, phase, message)
}

fn infrastructure_failure(phase: FailurePhase, message: String) -> RunFailure {
    RunFailure::new(RunStatus::InfrastructureError, phase, message)
}

fn update_score(report: &mut E2eRunReport) {
    report.score = report.criteria.iter().try_fold(0_u8, |score, criterion| {
        criterion
            .awarded
            .and_then(|awarded| score.checked_add(awarded))
    });
}

fn validate_objective_evaluation(
    spec: &ScenarioSpec,
    evaluation: &ObjectiveEvaluation,
) -> Result<()> {
    let mut gate_ids = HashSet::new();
    for gate in &evaluation.hard_gates {
        if gate.id.trim().is_empty() {
            bail!(
                "scenario '{}': evaluation contract violation: hard gate id is empty; expected a stable non-empty identifier",
                spec.id
            );
        }
        if gate.reason.trim().is_empty() {
            bail!(
                "scenario '{}': evaluation contract violation: hard gate '{}' has an empty reason; include the observed evidence",
                spec.id, gate.id
            );
        }
        if !gate_ids.insert(gate.id.as_str()) {
            bail!(
                "scenario '{}': evaluation contract violation: hard gate '{}' was returned more than once; expected unique gate ids",
                spec.id, gate.id
            );
        }
    }

    if spec.needs_judge() {
        if evaluation.awards.is_empty() {
            return Ok(());
        }
        bail!(
            "scenario '{}': evaluation contract violation: judge-backed evaluator returned awards [{}]; expected no awards because the judge owns criterion scoring",
            spec.id,
            evaluation
                .awards
                .iter()
                .map(|award| format!("'{}'", award.id))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let criteria: HashMap<_, _> = spec
        .criteria
        .iter()
        .map(|criterion| (criterion.id, criterion))
        .collect();
    let mut seen = HashSet::new();
    for award in &evaluation.awards {
        if award.id.trim().is_empty() {
            bail!(
                "scenario '{}': evaluation contract violation: criterion award id is empty; expected one award per configured criterion",
                spec.id
            );
        }
        if award.reason.trim().is_empty() {
            bail!(
                "scenario '{}': evaluation contract violation: criterion '{}' has an empty reason; include the observed evidence",
                spec.id, award.id
            );
        }
        let criterion = criteria
            .get(award.id.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "scenario '{}': evaluation contract violation: unknown criterion '{}'; expected one of [{}]; action: return exactly one award for each configured criterion",
                    spec.id,
                    award.id,
                    spec.criteria
                        .iter()
                        .map(|criterion| format!("'{}'", criterion.id))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        if award.awarded > criterion.weight {
            bail!(
                "scenario '{}': evaluation contract violation: criterion '{}' awarded {}; expected awarded in 0..={}; action: reduce the award or change the configured weight",
                spec.id, award.id,
                award.awarded,
                criterion.weight
            );
        }
        if !seen.insert(award.id.as_str()) {
            bail!(
                "scenario '{}': evaluation contract violation: criterion '{}' was returned more than once; expected exactly one award per configured criterion",
                spec.id, criterion.id
            );
        }
    }
    let missing = spec
        .criteria
        .iter()
        .filter(|criterion| !seen.contains(criterion.id))
        .map(|criterion| format!("'{}'", criterion.id))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        let received = evaluation
            .awards
            .iter()
            .map(|award| format!("'{}'", award.id))
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "scenario '{}': evaluation contract violation: missing awards [{}]; received [{}]; expected exactly one award per configured criterion",
            spec.id,
            missing.join(", "),
            received
        );
    }
    Ok(())
}

fn criterion_reports(spec: &ScenarioSpec, awards: Vec<CriterionAward>) -> Vec<CriterionReport> {
    let mut awards: HashMap<_, _> = awards
        .into_iter()
        .map(|award| (award.id, (award.awarded, award.reason)))
        .collect();
    spec.criteria
        .iter()
        .map(|criterion| {
            let award = awards.remove(criterion.id);
            CriterionReport {
                id: criterion.id.to_string(),
                possible: criterion.weight,
                awarded: award.as_ref().map(|(awarded, _)| *awarded),
                reason: award
                    .map(|(_, reason)| reason)
                    .unwrap_or_else(|| "not evaluated".into()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control_plane(hash: &str) -> ControlPlaneEvidence {
        ControlPlaneEvidence {
            functions: vec![crate::wire::FunctionContractEvidence {
                function_id: "harness::send".into(),
                request_schema: serde_json::json!({"type": "object"}),
                response_schema: serde_json::json!({"type": "object"}),
                sha256: hash.into(),
            }],
        }
    }

    #[test]
    fn per_case_preflight_rejects_control_plane_drift() {
        let expected = control_plane(
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let same = control_plane(
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let changed = control_plane(
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );

        ensure_control_plane_unchanged(&expected, &same).unwrap();
        assert!(ensure_control_plane_unchanged(&expected, &changed)
            .unwrap_err()
            .to_string()
            .contains("fingerprint changed"));
    }
    use crate::report::HardGateReport;
    use crate::scenarios::{CriterionSpec, ExecutionPolicy, ScenarioEvaluator};

    fn evaluator<'a>(
        _context: &'a E2eContext,
        _observation: &'a ScenarioObservation,
        _run_id: &'a str,
    ) -> crate::scenarios::EvaluationFuture<'a> {
        unreachable!()
    }

    fn spec() -> ScenarioSpec {
        ScenarioSpec {
            id: "case",
            version: 1,
            prompt: "prompt".into(),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: 1,
                max_output_tokens: Some(1),
                max_total_tokens: 1,
                stuck_timeout_seconds: 1,
            },
            denied_functions: &[],
            criteria: vec![CriterionSpec::advisory_judge("objective", 100, "objective")],
            judge_reference: None,
            setup: None,
            evaluate: evaluator as ScenarioEvaluator,
            cleanup: None,
        }
    }

    fn mixed_assessment_spec() -> ScenarioSpec {
        let mut spec = spec();
        spec.criteria = vec![
            CriterionSpec::required_deterministic(
                "required",
                70,
                "Required deterministic behavior.",
                EvaluationDimension::StructuralIntegrity,
            ),
            CriterionSpec::advisory_judge("quality", 30, "Advisory judge quality signal."),
        ];
        spec.judge_reference = Some(serde_json::json!({"expected": "quality"}));
        spec
    }

    #[test]
    fn materializes_one_result_per_declaration_without_losing_gate_or_partial_score() {
        let mut spec = spec();
        spec.criteria = vec![
            CriterionSpec::required_deterministic(
                "required",
                70,
                "Required deterministic behavior.",
                EvaluationDimension::StructuralIntegrity,
            ),
            CriterionSpec::advisory_deterministic(
                "signal",
                30,
                "Advisory deterministic signal.",
                EvaluationDimension::Efficiency,
            ),
        ];
        let criteria = vec![
            CriterionReport {
                id: "required".into(),
                possible: 70,
                awarded: Some(35),
                reason: "required behavior was incomplete".into(),
            },
            CriterionReport {
                id: "signal".into(),
                possible: 30,
                awarded: Some(12),
                reason: "partial efficiency evidence".into(),
            },
        ];
        let gates = vec![HardGateReport {
            id: "required".into(),
            dimension: EvaluationDimension::StructuralIntegrity,
            passed: false,
            reason: "required behavior was incomplete".into(),
        }];

        let results = materialize_assessment_results(
            &spec,
            &criteria,
            &gates,
            &CriterionJudgeState::NotRequested,
        );

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].criterion_id, "required");
        assert_eq!(
            results[0].policy,
            crate::assessment::AssessmentPolicy::HardGate
        );
        assert_eq!(results[0].outcome, AssessmentOutcome::Failed);
        assert_eq!(results[0].score.as_ref().unwrap().awarded, 35);
        assert_eq!(
            results[1].policy,
            crate::assessment::AssessmentPolicy::Advisory
        );
        assert_eq!(results[1].dimension, EvaluationDimension::Efficiency);
        assert_eq!(results[1].outcome, AssessmentOutcome::Partial);
        assert_eq!(results[1].score.as_ref().unwrap().awarded, 12);
    }

    #[test]
    fn malformed_judge_result_is_advisory_and_preserves_deterministic_result() {
        let spec = mixed_assessment_spec();
        let criteria = vec![
            CriterionReport {
                id: "required".into(),
                possible: 70,
                awarded: Some(70),
                reason: "deterministic evidence passed".into(),
            },
            CriterionReport {
                id: "quality".into(),
                possible: 30,
                awarded: None,
                reason: "not evaluated".into(),
            },
        ];
        let failure = judge::JudgeFailure {
            kind: judge::JudgeFailureKind::MalformedOutput,
            message: "response omitted quality".into(),
            attempts: 3,
            usage: None,
            analyzer: AnalyzerIdentity {
                analyzer: "criterion-assessment".into(),
                provider: Some("provider".into()),
                model: Some("model".into()),
                input_sha256: format!("sha256:{}", "a".repeat(64)),
            },
            analyzer_usage: AnalyzerUsage {
                latency_ms: Some(10),
                ..AnalyzerUsage::default()
            },
        };

        let results = materialize_assessment_results(
            &spec,
            &criteria,
            &[],
            &CriterionJudgeState::Failed(failure),
        );

        assert_eq!(results[0].outcome, AssessmentOutcome::Passed);
        assert_eq!(results[0].score.as_ref().unwrap().awarded, 70);
        assert_eq!(results[1].outcome, AssessmentOutcome::Error);
        assert!(results[1].summary.starts_with("judge_malformed_output:"));
        assert_eq!(
            results[1].policy,
            crate::assessment::AssessmentPolicy::Advisory
        );
        results.iter().for_each(|result| result.validate().unwrap());
    }

    #[test]
    fn unavailable_judge_is_explicit_without_inventing_a_score() {
        let spec = mixed_assessment_spec();
        let criteria = vec![
            CriterionReport {
                id: "required".into(),
                possible: 70,
                awarded: Some(70),
                reason: "passed".into(),
            },
            CriterionReport {
                id: "quality".into(),
                possible: 30,
                awarded: None,
                reason: "not evaluated".into(),
            },
        ];
        let results = materialize_assessment_results(
            &spec,
            &criteria,
            &[],
            &CriterionJudgeState::Unavailable("judge_unavailable: provider was not configured"),
        );

        assert_eq!(results[1].outcome, AssessmentOutcome::Unavailable);
        assert!(results[1].score.is_none());
        assert!(results[1].analyzer.is_none());
        results[1].validate().unwrap();
    }

    #[test]
    fn execution_failure_still_materializes_every_declared_assessment() {
        let spec = mixed_assessment_spec();
        let mut report = test_run_report();
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Execute,
            "subject transport failed",
        );

        ensure_assessment_results(&spec, &mut report);

        assert_eq!(report.assessment_results.len(), spec.criteria.len());
        assert_eq!(report.assessment_results[0].criterion_id, "required");
        assert_eq!(report.assessment_results[1].criterion_id, "quality");
        assert!(report
            .assessment_results
            .iter()
            .all(|result| result.outcome == AssessmentOutcome::NotEvaluated));
        assert!(report
            .assessment_results
            .iter()
            .all(|result| result.summary.contains("subject transport failed")));
    }

    #[test]
    fn e2e_policy_denies_the_control_plane_without_scenario_overrides() {
        let policy = e2e_function_policy(&spec());
        assert_eq!(policy.allow, ["*"]);
        assert_eq!(policy.deny, ["e2e::*"]);
        assert_eq!(policy.expose, Default::default());
    }

    #[test]
    fn e2e_policy_applies_scenario_denies() {
        let mut scenario = spec();
        scenario.denied_functions = &["state::*"];
        let policy = e2e_function_policy(&scenario);

        assert_eq!(policy.allow, ["*"]);
        assert_eq!(policy.deny, ["e2e::*", "state::*"]);
    }

    #[test]
    fn fixed_and_rotating_seeds_materialize_distinct_deduplicated_cases() {
        assert_eq!(
            case_seeds(ScenarioId::PersistentState, Some(7), &[7, 8, 9, 8]),
            vec![7, 8, 9]
        );
        assert_eq!(
            case_seeds(ScenarioId::PersistentState, None, &[]),
            vec![ScenarioId::PersistentState.canonical_seed()]
        );
    }

    #[test]
    fn objective_awards_must_be_complete_and_bounded() {
        let spec = spec();
        assert!(validate_objective_evaluation(
            &spec,
            &ObjectiveEvaluation {
                hard_gates: Vec::new(),
                awards: vec![CriterionAward {
                    id: "objective".into(),
                    awarded: 100,
                    reason: "ok".into(),
                }],
            }
        )
        .is_ok());
        assert!(validate_objective_evaluation(
            &spec,
            &ObjectiveEvaluation {
                hard_gates: Vec::new(),
                awards: Vec::new(),
            }
        )
        .is_err());
        let error = validate_objective_evaluation(
            &spec,
            &ObjectiveEvaluation {
                hard_gates: Vec::new(),
                awards: vec![CriterionAward {
                    id: "objective".into(),
                    awarded: 101,
                    reason: "too high".into(),
                }],
            },
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "scenario 'case': evaluation contract violation: criterion 'objective' awarded 101; expected awarded in 0..=100; action: reduce the award or change the configured weight"
        );
    }

    #[test]
    fn objective_contract_errors_identify_the_invalid_ids_and_values() {
        let spec = spec();
        let unknown = validate_objective_evaluation(
            &spec,
            &ObjectiveEvaluation {
                hard_gates: Vec::new(),
                awards: vec![CriterionAward {
                    id: "unknown".into(),
                    awarded: 1,
                    reason: "observed".into(),
                }],
            },
        )
        .unwrap_err();
        assert_eq!(
            unknown.to_string(),
            "scenario 'case': evaluation contract violation: unknown criterion 'unknown'; expected one of ['objective']; action: return exactly one award for each configured criterion"
        );

        let duplicate = validate_objective_evaluation(
            &spec,
            &ObjectiveEvaluation {
                hard_gates: Vec::new(),
                awards: vec![
                    CriterionAward {
                        id: "objective".into(),
                        awarded: 1,
                        reason: "first".into(),
                    },
                    CriterionAward {
                        id: "objective".into(),
                        awarded: 1,
                        reason: "second".into(),
                    },
                ],
            },
        )
        .unwrap_err();
        assert_eq!(
            duplicate.to_string(),
            "scenario 'case': evaluation contract violation: criterion 'objective' was returned more than once; expected exactly one award per configured criterion"
        );

        let empty_gate = validate_objective_evaluation(
            &spec,
            &ObjectiveEvaluation {
                hard_gates: vec![HardGateReport {
                    id: String::new(),
                    dimension: EvaluationDimension::StructuralIntegrity,
                    passed: false,
                    reason: "observed".into(),
                }],
                awards: vec![CriterionAward {
                    id: "objective".into(),
                    awarded: 1,
                    reason: "observed".into(),
                }],
            },
        )
        .unwrap_err();
        assert_eq!(
            empty_gate.to_string(),
            "scenario 'case': evaluation contract violation: hard gate id is empty; expected a stable non-empty identifier"
        );
    }

    #[test]
    fn hard_gate_failure_prevents_a_passing_run() {
        let mut report = test_run_report();
        report.hard_gates = vec![HardGateReport {
            id: "gate".into(),
            dimension: EvaluationDimension::StructuralIntegrity,
            passed: false,
            reason: "failed".into(),
        }];
        report.criteria = criterion_reports(
            &spec(),
            vec![CriterionAward {
                id: "objective".into(),
                awarded: 100,
                reason: "ok".into(),
            }],
        );
        update_score(&mut report);
        report.finish(if report.hard_gates.iter().all(|gate| gate.passed) {
            RunStatus::Passed
        } else {
            RunStatus::HardGateFailed
        });
        assert_eq!(report.status, RunStatus::HardGateFailed);
    }

    #[test]
    fn token_budget_failures_are_classified_as_resource_limits() {
        let failure = subject_failure(
            FailurePhase::Execute,
            "generation requires more tokens than remain in the token budget".into(),
        );
        assert_eq!(failure.status, RunStatus::ResourceLimit);
        let collection = collection_failure(
            FailurePhase::Collect,
            "scenario exceeded 600s while waiting for the complete session tree".into(),
        );
        assert_eq!(collection.status, RunStatus::ResourceLimit);
    }

    #[test]
    fn only_transient_technical_failures_are_retried() {
        let mut transient = test_run_report();
        transient.push_failure(
            RunStatus::SubjectError,
            FailurePhase::Execute,
            "zai stream ended without a terminal frame",
        );
        assert!(is_retryable_technical_failure(&transient));

        let mut deterministic = test_run_report();
        deterministic.push_failure(
            RunStatus::JudgeError,
            FailurePhase::Evaluate,
            "judge returned an invalid criterion set",
        );
        assert!(!is_retryable_technical_failure(&deterministic));

        let mut cleanup = test_run_report();
        cleanup.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Cleanup,
            "connection reset during cleanup",
        );
        assert!(!is_retryable_technical_failure(&cleanup));

        let mut budget = test_run_report();
        budget.push_failure(
            RunStatus::ResourceLimit,
            FailurePhase::Execute,
            "scenario exceeded its deadline",
        );
        assert!(!is_retryable_technical_failure(&budget));
    }

    #[test]
    fn final_input_excludes_raw_content_and_keeps_stable_evidence_identity() {
        let mut run = test_run_report();
        run.status = RunStatus::Passed;
        run.prompt = "secret prompt content that must not reach the analyzer".into();
        run.transcript = Some(serde_json::json!({
            "messages": ["secret transcript content that must not reach the analyzer"]
        }));
        run.evidence.push(crate::artifact::ArtifactReference {
            id: "transcript".into(),
            kind: "transcript".into(),
            path: "evidence/run/attempt/transcript.json".into(),
            sha256: format!("sha256:{}", "a".repeat(64)),
            size_bytes: 42,
            media_type: "application/json".into(),
        });
        run.dimensions = vec![crate::report::DimensionReport {
            dimension: EvaluationDimension::Efficiency,
            passed: None,
            signals: serde_json::json!({
                "deep": {"one": {"two": {"three": "deep content must be omitted"}}}
            }),
        }];
        let scenario = E2eScenarioReport::aggregate(
            "direct_answer",
            2,
            ExecutionPolicy {
                max_turns: 1,
                max_output_tokens: Some(100),
                max_total_tokens: 100,
                stuck_timeout_seconds: 1,
            },
            vec![run],
        );
        let run = &scenario.runs[0];
        let contract = crate::assessment::RunAssessmentContract {
            run_id: run.run_id.clone(),
            attempt_id: run.attempt_id.clone(),
            system_status: SystemStatus::Passed,
            assessments: Vec::new(),
            assets: Vec::new(),
            ai_final_assessment: AiFinalAssessment::not_evaluated("preliminary persistence"),
            effective_status: crate::assessment::EffectiveStatus::Passed,
        };

        let input = final_assessment_input("execution-1", &scenario, run, &contract);
        input.validate().unwrap();
        let encoded = serde_json::to_string(&input).unwrap();
        assert!(!encoded.contains("secret prompt content"));
        assert!(!encoded.contains("secret transcript content"));
        assert!(!encoded.contains("deep content must be omitted"));
        assert_eq!(input.excerpts[0].evidence.artifact_id, "transcript");

        let unavailable = unavailable_final_assessment(&input).unwrap();
        assert_eq!(
            unavailable.availability,
            AiAssessmentAvailability::Unavailable
        );
        assert_eq!(
            unavailable.analyzer.unwrap().input_sha256,
            input.sha256().unwrap()
        );
    }

    fn test_run_report() -> E2eRunReport {
        E2eRunReport::new(
            "run".into(),
            "attempt".into(),
            1,
            "session".into(),
            "prompt".into(),
        )
    }
}
