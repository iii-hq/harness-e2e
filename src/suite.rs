use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use crate::artifact;
use crate::assessment::{
    AiAssessmentAvailability, AiFinalAssessment, AnalyzerIdentity, AnalyzerUsage,
    AssessmentOutcome, AssessmentResult, AssessmentScore, AssessmentTarget, AssessmentTargetKind,
    EvidenceReference, FinalAssessmentCleanup, FinalAssessmentExcerpt, FinalAssessmentInput,
    FinalAssessmentLongitudinalRobustness, FinalAssessmentMetric, FinalAssessmentSubject,
    FinalAssessmentValidation, FinalAssessmentValidationCoverage, FinalAssessmentValidationProbe,
    FinalAssessmentValidationRepeatability, SystemStatus,
};
use crate::asset::{self, AssetCaptureLimits};
use crate::context::E2eContext;
use crate::identity::{self, ExecutionIdentity, SystemUnderTestIdentity};
use crate::judge::{self, JudgeConfig};
use crate::markdown::{
    CompiledMarkdownScenario, MarkdownCriterion, MarkdownScenarioSource, RenderedMarkdownScenario,
    ScenarioKey,
};
use crate::report::{
    AdherenceAvailability, AdherenceRequirement, CostReport, CriterionReport, E2eManifest,
    E2eReport, E2eRunReport, E2eScenarioReport, EvaluationDimension, FailurePhase, HardGateReport,
    InstructionAdherenceReport, MarkdownExecutionReport, MarkdownPhaseReport, MarkdownPhaseStatus,
    ModelArtifact, ObservationMetricOrigin, ObservationRunContract, RetryAttemptReport, RunStatus,
    ScenarioFlowEvidence, ScenarioMeasurement,
};
use crate::scenarios::common;
use crate::scenarios::{
    CapturedDeliverableContent, ComplexityProfile, CriterionAward, DeliverableContract,
    MaterializedScenario, ObjectiveEvaluation, ScenarioCase, ScenarioDeliverableCapture,
    ScenarioExecutionKind, ScenarioId, ScenarioObservation, ScenarioSpec,
};
use crate::wire::{
    ControlPlaneEvidence, FunctionPolicy, MessageInput, Model, SendOptions, SendRequest,
    SendResponse, SessionInit, StatusReport,
};
use crate::workflow::{
    adaptive_runtime, composite_definition, composite_descriptor_catalog, composite_runtime,
    execute_adaptive_workflow, execute_workflow, observe_worker_contracts, plan_adaptive_workflow,
    AdaptivePlannerInvalidationV1, AdaptivePlannerMetadataV1, AdaptivePlannerReferenceCheckV1,
    AgentPlannerRequest, ResumableWorkflowExecutionRequest, ResumableWorkflowOutcome,
    WorkflowCleanupContext, WorkflowCleanupStatus, WorkflowExecutionRequest, WorkflowFailurePhase,
    WorkflowResumeIdentityV1, WorkflowResumeStore, WorkflowStepStatus,
};

const MAX_RUNS: u32 = 20;
const MAX_TECHNICAL_RETRIES: u8 = 3;
// Objective results are already durable when this advisory phase begins. Keep
// analyzer/provider stalls from holding the CLI child (and its plan lifecycle)
// indefinitely; timing out the advisory assessment never changes run status.
const FINAL_ASSESSMENT_BATCH_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) fn e2e_function_policy(spec: &ScenarioSpec, run_id: &str) -> FunctionPolicy {
    let mut deny = vec!["e2e::*".to_string()];
    deny.extend(
        spec.denied_functions
            .iter()
            .map(|function| (*function).to_string()),
    );
    deny.sort();
    deny.dedup();
    FunctionPolicy {
        allow: crate::scenarios::allowed_functions(spec.id, run_id)
            .unwrap_or_else(|| vec!["*".into()]),
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
    pub execution_id: Option<String>,
    pub subject: SubjectConfig,
    pub judge: Option<JudgeConfig>,
    /// Model that audits subject behavior from the captured transcript.
    /// Opt-in: `None` keeps the audit deterministic-only.
    pub audit_analyzer: Option<JudgeConfig>,
    pub output: PathBuf,
    pub scenarios: Vec<ScenarioKey>,
    /// Frozen local Markdown definitions captured when the execution was
    /// admitted. Embedded scenarios remain resolved from the binary catalog.
    pub local_markdown_scenarios: Vec<MarkdownScenarioSource>,
    pub runs: u32,
    pub seed: Option<u64>,
    pub rotating_seeds: Vec<u64>,
    pub technical_retries: u8,
    pub progress_interval: Option<Duration>,
    pub control: Option<SuiteControl>,
    pub observation_contract: Option<ObservationRunContract>,
    /// Exact immutable Markdown plan used by materialized replay. The runner
    /// recomputes and compares every field before executing any phase.
    pub materialized_markdown_plan: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveResumeAttempt {
    pub scenario_id: ScenarioId,
    pub run_id: String,
    pub attempt_id: String,
    pub resume_existing: bool,
    pub restore_planner: bool,
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
        scenario_id: ScenarioKey,
        run_id: String,
        attempt_id: String,
        session_id: String,
        resume_state_path: Option<String>,
    },
    AdaptiveResumeState {
        attempt_id: String,
        state_sha256: String,
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
    pub adaptive_resume: Option<AdaptiveResumeAttempt>,
}

pub async fn run_suite(config: SuiteRunConfig) -> Result<SuiteRunOutcome> {
    validate_config(&config)?;
    emit_phase(config.control.as_ref(), SuitePhase::Preflighting).await?;
    ensure_not_cancelled(config.control.as_ref())?;
    let execution_id = config
        .control
        .as_ref()
        .map(|control| control.execution_id.clone())
        .or_else(|| config.execution_id.clone())
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let started_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let context = Arc::new(
        E2eContext::connect(&config.url)
            .await
            .context("connect E2E runner")?,
    );
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
    let system_identity_sha256 = artifact::sha256_value(&system_under_test)?;
    let subject_model = resolve_model(&context, &config.subject.model, &config.subject.provider)
        .await
        .context("resolve subject model")?;
    let has_markdown = config
        .scenarios
        .iter()
        .any(|scenario| scenario.built_in().is_none());
    let judge_model = match config.judge.as_ref() {
        Some(judge) => match resolve_model(&context, &judge.model, &judge.provider).await {
            Ok(model) => Some(model),
            Err(error) if has_markdown => {
                return Err(error).context(
                    "resolve the explicit auxiliary model required by Markdown scenarios",
                );
            }
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
    let built_in_scenarios = config
        .scenarios
        .iter()
        .filter_map(ScenarioKey::built_in)
        .collect::<Vec<_>>();
    if built_in_scenarios.contains(&ScenarioId::SecurityReview) {
        crate::workflow::security_scan::register_local_adapter_if_configured(context.as_ref())
            .await
            .context("register fixture-backed local security-scan adapter")?;
    }
    let composite_definitions = built_in_scenarios
        .iter()
        .filter_map(|scenario| composite_definition(*scenario))
        .collect::<Vec<_>>();
    let composite_catalog = composite_descriptor_catalog(&built_in_scenarios)?;
    for definition in &composite_definitions {
        definition
            .validate(&composite_catalog)
            .with_context(|| format!("validate Rust-defined scenario '{}'", definition.id))?;
    }
    // Observe each composite independently. A missing fixture worker belongs to
    // that scenario's infrastructure result; it must not erase reports for
    // otherwise runnable scenarios in the same suite. Executable step
    // preflights still enforce function availability and exact contracts.
    let mut worker_contracts = Vec::new();
    for definition in &composite_definitions {
        match observe_worker_contracts(
            context.as_ref(),
            &composite_catalog,
            std::slice::from_ref(definition),
        )
        .await
        {
            Ok(observed) => {
                for contract in observed {
                    if !worker_contracts.iter().any(
                        |current: &crate::report::ObservedWorkerContract| {
                            current.function_id == contract.function_id
                        },
                    ) {
                        worker_contracts.push(contract);
                    }
                }
            }
            Err(error) => tracing::warn!(
                scenario = definition.id,
                error = %format!("{error:#}"),
                "deferring composite worker contract failure to the scenario attempt"
            ),
        }
    }
    worker_contracts.sort_by(|left, right| left.function_id.cmp(&right.function_id));
    emit_phase(config.control.as_ref(), SuitePhase::Materializing).await?;
    let mut scenario_reports = Vec::new();

    for scenario_key in &config.scenarios {
        ensure_not_cancelled(config.control.as_ref())?;
        for seed in case_seeds_for_key(scenario_key, config.seed, &config.rotating_seeds) {
            if let Some(scenario_id) = scenario_key.built_in() {
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
                            scenario_id,
                            subject: &config.subject,
                            judge_config: config.judge.as_ref(),
                            audit_analyzer: config.audit_analyzer.as_ref(),
                            seed,
                            technical_retries: config.technical_retries,
                            progress_interval: config.progress_interval,
                            control: config.control.as_ref(),
                            output: &config.output,
                            system_identity_sha256: &system_identity_sha256,
                            adaptive_resume: config
                                .control
                                .as_ref()
                                .and_then(|control| control.adaptive_resume.as_ref())
                                .filter(|resume| resume.scenario_id == scenario_id),
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
            } else {
                let definition = markdown_definition(&config, scenario_key.as_str())?;
                let scenario = &definition.scenario;
                let case = markdown_case(scenario, seed)?;
                preflight_case(&context, &control_plane, &case).await?;
                let mut runs = Vec::with_capacity(config.runs as usize);
                for repetition in 0..config.runs {
                    tracing::info!(
                        scenario = scenario.id,
                        case_id = case.case_id,
                        seed,
                        run = repetition + 1,
                        total_runs = config.runs,
                        "running Markdown-authored E2E scenario"
                    );
                    let run = run_markdown_with_technical_retries(
                        &context,
                        MarkdownRetryRequest {
                            scenario,
                            source: &definition.source,
                            subject: &config.subject,
                            auxiliary: config
                                .judge
                                .as_ref()
                                .expect("validated Markdown auxiliary model"),
                            audit_analyzer: config.audit_analyzer.as_ref(),
                            seed,
                            technical_retries: config.technical_retries,
                            progress_interval: config.progress_interval,
                            control: config.control.as_ref(),
                            output: &config.output,
                            system_identity_sha256: &system_identity_sha256,
                            runs: config.runs,
                            materialized_plan: config.materialized_markdown_plan.as_ref(),
                        },
                    )
                    .await;
                    let stop = run.status.is_technical_failure();
                    runs.push(run);
                    if stop {
                        tracing::warn!(
                            scenario = scenario.id,
                            seed,
                            "stopping Markdown case after a technical failure"
                        );
                        break;
                    }
                }
                scenario_reports.push(E2eScenarioReport::aggregate_case(
                    case,
                    crate::markdown::execution_policy(),
                    runs,
                ));
            }
        }
    }

    crate::scenarios::engineering_ticket::apply_handoff_efficiency(&mut scenario_reports);

    for contract in scenario_reports
        .iter()
        .flat_map(|scenario| &scenario.runs)
        .flat_map(|run| &run.worker_contracts)
    {
        if let Some(existing) = worker_contracts
            .iter()
            .find(|existing| existing.function_id == contract.function_id)
        {
            if existing != contract {
                bail!(
                    "function contract '{}' changed between preflight and scenario execution",
                    contract.function_id
                );
            }
        } else {
            worker_contracts.push(contract.clone());
        }
    }
    worker_contracts.sort_by(|left, right| left.function_id.cmp(&right.function_id));

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
        observation_contract: config.observation_contract.clone(),
        worker_contracts,
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
    report.observation_contract = config.observation_contract.clone();
    emit_phase(config.control.as_ref(), SuitePhase::Finalizing).await?;
    ensure_not_cancelled(config.control.as_ref())?;
    // Persist a complete objective result and immutable evidence before invoking
    // the complementary analyzer. A provider failure can therefore never erase
    // the completed execution.
    report.write_to(&config.output, &manifest)?;
    let final_assessment_count = report
        .scenarios
        .iter()
        .map(|scenario| scenario.runs.len())
        .sum::<usize>();
    tracing::info!(
        final_assessment_count,
        timeout_seconds = FINAL_ASSESSMENT_BATCH_TIMEOUT.as_secs(),
        "objective results persisted; starting advisory final assessments before runner shutdown"
    );
    let final_assessments_started = Instant::now();
    evaluate_final_assessments(&context, config.judge.as_ref(), &config.output, &mut report)
        .await?;
    tracing::info!(
        final_assessment_count,
        duration_ms = final_assessments_started
            .elapsed()
            .as_millis()
            .min(u64::MAX as u128) as u64,
        "advisory final assessments completed; persisting the final report"
    );
    let report_path = report.write_to(&config.output, &manifest)?;
    tracing::info!("final report persisted; shutting down the E2E connection");
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
    let total = report
        .scenarios
        .iter()
        .map(|scenario| scenario.runs.len())
        .sum::<usize>();
    let batch_started = Instant::now();
    let mut ordinal = 0usize;
    for scenario_index in 0..report.scenarios.len() {
        for run_index in 0..report.scenarios[scenario_index].runs.len() {
            ordinal += 1;
            let (scenario_id, run_id, attempt_id, run_status, input) = {
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
                    scenario.scenario_id.clone(),
                    run.run_id.clone(),
                    run.attempt_id.clone(),
                    run.status,
                    final_assessment_input(&report.execution.execution_id, scenario, run, contract),
                )
            };
            tracing::info!(
                scenario = scenario_id,
                run_id,
                attempt_id,
                status = ?run_status,
                ordinal,
                total,
                "starting advisory final assessment"
            );
            let assessment_started = Instant::now();
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
                tracing::warn!(
                    scenario = scenario_id,
                    run_id,
                    attempt_id,
                    ordinal,
                    total,
                    error = %format!("{error:#}"),
                    "advisory final assessment input was invalid"
                );
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
                    tracing::warn!(
                        scenario = scenario_id,
                        run_id,
                        attempt_id,
                        ordinal,
                        total,
                        error = %format!("{error:#}"),
                        "advisory final assessment input could not be persisted"
                    );
                    continue;
                }
            };

            let (assessment, attempts, usage) = match judge_config {
                Some(config) => {
                    let remaining =
                        FINAL_ASSESSMENT_BATCH_TIMEOUT.saturating_sub(batch_started.elapsed());
                    if remaining.is_zero() {
                        (
                            timed_out_final_assessment(&input, config, Duration::ZERO)?,
                            0,
                            None,
                        )
                    } else {
                        match tokio::time::timeout(
                            remaining,
                            judge::evaluate_final_assessment(context, config, &input),
                        )
                        .await
                        {
                            Ok(outcome) => {
                                let outcome = outcome?;
                                (outcome.assessment, outcome.attempts, outcome.usage)
                            }
                            Err(_) => (
                                timed_out_final_assessment(
                                    &input,
                                    config,
                                    assessment_started.elapsed(),
                                )?,
                                1,
                                None,
                            ),
                        }
                    }
                }
                None => (unavailable_final_assessment(&input)?, 0, None),
            };
            let availability = assessment.availability;

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
            tracing::info!(
                scenario = scenario_id,
                run_id,
                attempt_id,
                status = ?run_status,
                availability = ?availability,
                attempts,
                ordinal,
                total,
                duration_ms = assessment_started
                    .elapsed()
                    .as_millis()
                    .min(u64::MAX as u128) as u64,
                "advisory final assessment finished"
            );
        }
        report.scenarios[scenario_index].refresh_aggregate()?;
    }
    report.passed =
        !report.scenarios.is_empty() && report.scenarios.iter().all(|scenario| scenario.passed);
    Ok(())
}

fn timed_out_final_assessment(
    input: &FinalAssessmentInput,
    config: &JudgeConfig,
    elapsed: Duration,
) -> Result<AiFinalAssessment> {
    let elapsed_ms = elapsed.as_millis().min(u64::MAX as u128) as u64;
    let assessment = AiFinalAssessment {
        availability: AiAssessmentAvailability::Failed,
        result: None,
        analyzer: Some(AnalyzerIdentity {
            analyzer: "final-assessment".into(),
            provider: Some(config.provider.clone()),
            model: Some(config.model.clone()),
            input_sha256: input.sha256()?,
        }),
        analyzer_usage: Some(AnalyzerUsage {
            latency_ms: Some(elapsed_ms),
            input_tokens: None,
            output_tokens: None,
            cost_usd: None,
        }),
        reason: Some(format!(
            "final_assessment_timeout: advisory analysis exceeded the {} second suite finalization budget; the objective system status is unchanged",
            FINAL_ASSESSMENT_BATCH_TIMEOUT.as_secs()
        )),
    };
    assessment.validate()?;
    Ok(assessment)
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
    let validation = final_assessment_validation(scenario, run);
    if validation.is_none()
        && matches!(
            scenario.scenario_id.as_str(),
            crate::scenarios::todo_worker::SIMPLE_ID | crate::scenarios::todo_worker::PLANNED_ID
        )
    {
        limitations.push(
            "No complete Todo validation bundle was available for the final assessment projection."
                .into(),
        );
    }

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
        validation,
        excerpts,
        limitations,
    }
}

fn final_assessment_validation(
    scenario: &E2eScenarioReport,
    run: &E2eRunReport,
) -> Option<FinalAssessmentValidation> {
    use crate::scenarios::todo_worker::{ProbeOutcome, ValidationEvidenceBundle};

    let atomic = run.deliverables.iter().find_map(|deliverable| {
        if deliverable.kind != "todo_validation_evidence" {
            return None;
        }
        let CapturedDeliverableContent::Json(value) = &deliverable.content else {
            return None;
        };
        Some((
            serde_json::from_value::<ValidationEvidenceBundle>(value.clone()).ok()?,
            deliverable.artifact.as_ref()?.clone(),
        ))
    });
    let composite = run.semantic_tests.iter().find_map(|step| {
        let output = step.outputs.get("validation_bundle")?;
        let bundle =
            serde_json::from_value::<ValidationEvidenceBundle>(output.value.clone()).ok()?;
        let reference = step
            .assets
            .iter()
            .find(|asset| asset.kind == "todo_validation_evidence")?
            .artifact
            .clone();
        Some((bundle, reference))
    });
    let (bundle, reference) = atomic.or(composite)?;
    let final_attempt = bundle.attempts.last()?;
    let mut grouped = std::collections::BTreeMap::<String, Vec<_>>::new();
    for probe in &final_attempt.probes {
        grouped.entry(probe.id.clone()).or_default().push(probe);
    }
    let probes = grouped
        .into_iter()
        .take(12)
        .map(|(id, observations)| {
            let outcome = if observations
                .iter()
                .all(|probe| probe.outcome == ProbeOutcome::Passed)
            {
                "passed"
            } else if observations
                .iter()
                .any(|probe| probe.outcome == ProbeOutcome::InfrastructureError)
            {
                "infrastructure_error"
            } else if observations
                .iter()
                .any(|probe| probe.outcome == ProbeOutcome::Failed)
            {
                "failed"
            } else {
                "not_evaluated"
            };
            let summary = observations
                .iter()
                .map(|probe| {
                    format!(
                        "repetition={} outcome={:?} observed={}",
                        probe.repetition,
                        probe.outcome,
                        serde_json::to_string(&bounded_json(&probe.observed, 0))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            FinalAssessmentValidationProbe {
                id,
                outcome: outcome.into(),
                summary: summary.chars().take(1_000).collect(),
            }
        })
        .collect();
    let validation_attempts = bundle.attempts.len().try_into().unwrap_or(u32::MAX);
    let correction_attempts = bundle.nudges.min(validation_attempts.saturating_sub(1));
    let robustness = &scenario.aggregate.robustness;
    let mut limitations = bundle
        .limitations
        .iter()
        .map(|value| bounded_text(value))
        .collect::<Vec<_>>();
    if !robustness.eligible {
        limitations.push(format!(
            "Longitudinal reliability is unavailable: {} comparable run(s), minimum {}.",
            robustness.sample_size, robustness.minimum_sample_size
        ));
    }
    Some(FinalAssessmentValidation {
        bundle: EvidenceReference::from(&reference),
        contract_sha256: bundle.contract_sha256,
        plan_sha256: bundle.plan_sha256,
        candidate_sha256: bundle.subject.candidate_sha256,
        probes,
        coverage: FinalAssessmentValidationCoverage {
            required: bundle.coverage.required,
            covered: bundle.coverage.covered,
            omitted: bundle.coverage.omitted,
            complete: bundle.coverage.complete,
        },
        validation_attempts,
        correction_attempts,
        repeatability: FinalAssessmentValidationRepeatability {
            planned: bundle.repeatability.planned,
            completed: bundle.repeatability.completed,
            passed: bundle.repeatability.passed,
            interpretation: format!(
                "{}/{} planned CRUD cycles passed within this run; this is observed in-run repeatability, not broad reliability.",
                bundle.repeatability.passed, bundle.repeatability.planned
            ),
        },
        longitudinal_robustness: FinalAssessmentLongitudinalRobustness {
            sample_size: robustness.sample_size,
            minimum_sample_size: robustness.minimum_sample_size,
            eligible: robustness.eligible,
            technical_failure_rate: robustness.technical_failure_rate,
            flaky_rate: robustness.flaky_rate,
            unavailable_reasons: robustness.unavailable.clone(),
        },
        limitations,
    })
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

fn markdown_definition(config: &SuiteRunConfig, id: &str) -> Result<MarkdownScenarioSource> {
    if let Some(definition) = config
        .local_markdown_scenarios
        .iter()
        .find(|definition| definition.scenario.id == id)
    {
        crate::markdown::validate_local_definition(definition)?;
        return Ok(definition.clone());
    }
    crate::markdown::embedded_definition(id)
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
    if config.technical_retries > 0
        && config
            .scenarios
            .iter()
            .any(|scenario| !scenario.execution_kind().replay_safe())
    {
        bail!("non-replayable scenarios require --technical-retries 0");
    }
    if config.scenarios.is_empty() {
        bail!("at least one scenario is required");
    }
    if let Some(resume) = config
        .control
        .as_ref()
        .and_then(|control| control.adaptive_resume.as_ref())
    {
        if config.scenarios.as_slice() != [ScenarioKey::BuiltIn(resume.scenario_id)]
            || config.runs != 1
            || config.technical_retries != 0
            || !config.rotating_seeds.is_empty()
        {
            bail!("adaptive resume requires one isolated scenario, one run, and no replay");
        }
    }
    for scenario in &config.scenarios {
        let seed = config.seed.unwrap_or_else(|| scenario.canonical_seed());
        if let Some(scenario) = scenario.built_in() {
            scenario.materialize("validation", seed)?;
        } else {
            markdown_definition(config, scenario.as_str())?;
        }
    }
    if config
        .scenarios
        .iter()
        .any(|scenario| scenario.built_in().is_none())
        && config.judge.is_none()
    {
        bail!("Markdown scenarios require an explicit auxiliary model and provider");
    }
    if config.materialized_markdown_plan.is_some()
        && (config.scenarios.len() != 1 || config.scenarios[0].built_in().is_some())
    {
        bail!("materialized Markdown replay requires exactly one Markdown scenario");
    }
    if let Some(judge) = &config.judge {
        if judge.model.trim().is_empty() || judge.provider.trim().is_empty() {
            bail!("judge model and provider cannot be empty");
        }
    }
    Ok(())
}

fn case_seeds(scenario: ScenarioId, fixed: Option<u64>, rotating: &[u64]) -> Vec<u64> {
    if scenario.canonical_seed_only() {
        return vec![scenario.canonical_seed()];
    }
    let mut seeds = vec![fixed.unwrap_or_else(|| scenario.canonical_seed())];
    for seed in rotating {
        if !seeds.contains(seed) {
            seeds.push(*seed);
        }
    }
    seeds
}

fn case_seeds_for_key(scenario: &ScenarioKey, fixed: Option<u64>, rotating: &[u64]) -> Vec<u64> {
    if let Some(scenario) = scenario.built_in() {
        return case_seeds(scenario, fixed, rotating);
    }
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
    audit_analyzer: Option<&'a JudgeConfig>,
    seed: u64,
    progress_interval: Option<Duration>,
    control: Option<&'a SuiteControl>,
    output: &'a std::path::Path,
    system_identity_sha256: &'a str,
    existing_attempt_id: Option<&'a str>,
    resume_existing: bool,
    restore_planner: bool,
}

async fn run_once(context: &Arc<E2eContext>, request: AttemptRequest<'_>) -> E2eRunReport {
    let AttemptRequest {
        scenario_id,
        run_id,
        attempt_number,
        subject,
        judge_config,
        audit_analyzer,
        seed,
        progress_interval,
        control,
        output,
        system_identity_sha256,
        existing_attempt_id,
        resume_existing,
        restore_planner,
    } = request;
    let started = Instant::now();
    let attempt_id = existing_attempt_id
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let session_id = format!("e2e_{attempt_id}");
    if scenario_id.execution_kind() == ScenarioExecutionKind::AdaptiveFlow {
        return run_adaptive_once(
            context,
            AdaptiveAttemptRequest {
                scenario_id,
                run_id,
                attempt_number,
                subject,
                seed,
                control,
                output,
                attempt_id,
                started,
                system_identity_sha256,
                resume_existing,
                restore_planner,
            },
        )
        .await;
    }
    if scenario_id.execution_kind() == ScenarioExecutionKind::CompositeFlow {
        return run_composite_once(
            context,
            CompositeAttemptRequest {
                scenario_id,
                run_id,
                attempt_number,
                subject,
                seed,
                control,
                output,
                attempt_id,
                started,
            },
        )
        .await;
    }
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
            scenario_id: scenario_id.into(),
            run_id: run_id.to_string(),
            attempt_id: attempt_id.clone(),
            session_id: session_id.clone(),
            resume_state_path: None,
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
            context.as_ref(),
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
    // Status, score, cost, and efficiency are final; the behavioral audit
    // below is advisory evidence and only ever fills `report.audit`.
    let audit =
        crate::audit::run_audit(context.as_ref(), audit_analyzer, &spec, &case, &report).await;
    report.audit = Some(audit);
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

struct CompositeAttemptRequest<'a> {
    scenario_id: ScenarioId,
    run_id: &'a str,
    attempt_number: u32,
    subject: &'a SubjectConfig,
    seed: u64,
    control: Option<&'a SuiteControl>,
    output: &'a std::path::Path,
    attempt_id: String,
    started: Instant,
}

struct AdaptiveAttemptRequest<'a> {
    scenario_id: ScenarioId,
    run_id: &'a str,
    attempt_number: u32,
    subject: &'a SubjectConfig,
    seed: u64,
    control: Option<&'a SuiteControl>,
    output: &'a std::path::Path,
    attempt_id: String,
    started: Instant,
    system_identity_sha256: &'a str,
    resume_existing: bool,
    restore_planner: bool,
}

async fn run_adaptive_once(
    context: &Arc<E2eContext>,
    request: AdaptiveAttemptRequest<'_>,
) -> E2eRunReport {
    let AdaptiveAttemptRequest {
        scenario_id,
        run_id,
        attempt_number,
        subject,
        seed,
        control,
        output,
        attempt_id,
        started,
        system_identity_sha256,
        resume_existing,
        restore_planner,
    } = request;
    let session_id = format!("adaptive_{attempt_id}");
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
                format!("adaptive scenario materialization failed: {error:#}"),
            );
            report.wall_time_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            ensure_assessment_results(&spec, &mut report);
            report.refresh_dimensions(false);
            return report;
        }
    };
    let MaterializedScenario { spec, case, .. } = materialized;
    let mut report = E2eRunReport::new(
        run_id.to_string(),
        attempt_id.clone(),
        attempt_number,
        session_id.clone(),
        spec.prompt.clone(),
    );
    let execution_id = control
        .map(|control| control.execution_id.clone())
        .unwrap_or_else(|| run_id.to_string());
    let state_root = output.parent().unwrap_or(output).join(".workflow-state");
    let resume_store = WorkflowResumeStore::new(&state_root, &execution_id, run_id, &attempt_id);
    let resume_state_path = resume_store
        .as_ref()
        .ok()
        .map(|store| store.path().to_string_lossy().into_owned());
    if let Err(error) = emit_event(
        control,
        SuiteEvent::AttemptStarted {
            scenario_id: scenario_id.into(),
            run_id: run_id.to_string(),
            attempt_id: attempt_id.clone(),
            session_id,
            resume_state_path,
        },
    )
    .await
    {
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Setup,
            format!("persist adaptive attempt checkpoint: {error:#}"),
        );
    }
    if report.failures.is_empty() {
        let _ = emit_phase(control, SuitePhase::SettingUp).await;
        match adaptive_runtime(
            scenario_id,
            context.clone(),
            &subject.model,
            &subject.provider,
            output,
            &attempt_id,
        ) {
            Ok(mut runtime) => {
                let cancellation = control.map_or_else(
                    || watch::channel(false).1,
                    |control| control.cancellation.clone(),
                );
                let planner = match adaptive_planner_metadata(scenario_id, &spec) {
                    Ok(metadata) => {
                        plan_adaptive_workflow(AgentPlannerRequest {
                            context,
                            model: &subject.model,
                            provider: &subject.provider,
                            scenario_prompt: &spec.prompt,
                            policy: &runtime.policy,
                            catalog: &runtime.catalog,
                            metadata: &metadata,
                            execution_id: &execution_id,
                            run_id,
                            attempt_id: &attempt_id,
                            state_root: &state_root,
                            restored_attempt: restore_planner,
                            cancellation: Some(&cancellation),
                        })
                        .await
                    }
                    Err(error) => Err(error),
                };
                match planner {
                    Err(error) => {
                        let cleanup_result = runtime
                            .cleanup_hook
                            .cleanup(&WorkflowCleanupContext {
                                workflow_id: runtime.materialized.definition.id.clone(),
                                workflow_sha256: runtime
                                    .materialized
                                    .definition
                                    .canonical_sha256()
                                    .unwrap_or_default(),
                                run_id: run_id.into(),
                                attempt_id: attempt_id.clone(),
                                output_dir: output.into(),
                            })
                            .await;
                        let rendered = format!("{error:#}");
                        report.push_failure(
                            adaptive_planner_failure_status(&rendered),
                            FailurePhase::Execute,
                            format!("adaptive planning failed: {rendered}"),
                        );
                        if let Err(cleanup_error) = cleanup_result {
                            report.push_failure(
                                RunStatus::InfrastructureError,
                                FailurePhase::Cleanup,
                                format!(
                                    "cleanup after adaptive planning failure: {cleanup_error:#}"
                                ),
                            );
                        }
                    }
                    Ok(planner) => {
                        runtime.plans = planner.plans;
                        runtime.completed_node_ids = planner.completed_node_ids;
                        runtime.materialized = planner.materialized;
                        let planner_cost = planner
                            .evidence
                            .usage
                            .as_ref()
                            .and_then(|usage| usage.cost_usd);
                        match artifact::write_json(
                            output,
                            &PathBuf::from("evidence")
                                .join(run_id)
                                .join(&attempt_id)
                                .join("adaptive-plan-evidence.json"),
                            "adaptive-plan-evidence",
                            "adaptive_plan_evidence",
                            &planner.evidence,
                        ) {
                            Ok(evidence) => report.evidence.push(evidence),
                            Err(error) => report.push_failure(
                                RunStatus::InfrastructureError,
                                FailurePhase::Collect,
                                format!("persist adaptive plan evidence: {error:#}"),
                            ),
                        }
                        if report.failures.is_empty() {
                            let uses_harness = runtime
                                .materialized
                                .definition
                                .nodes
                                .iter()
                                .any(|node| node.step_type == crate::workflow::HARNESS_STEP_ID);
                            let bind_result = if uses_harness {
                                context.bind_turn_completed().await
                            } else {
                                Ok(())
                            };
                            if let Err(error) = bind_result {
                                report.push_failure(
                                    RunStatus::InfrastructureError,
                                    FailurePhase::Setup,
                                    format!("bind adaptive Harness observation: {error:#}"),
                                );
                            } else {
                                let _ = emit_phase(control, SuitePhase::Executing).await;
                                let scenario_contract_sha256 =
                                    crate::scenarios::scenario_contract_sha256(
                                        &case,
                                        spec.execution,
                                    );
                                let catalog_sha256 = runtime.catalog.canonical_sha256();
                                let workflow_sha256 =
                                    runtime.materialized.definition.canonical_sha256();
                                let identity =
                                    scenario_contract_sha256.and_then(|scenario_contract_sha256| {
                                        Ok(WorkflowResumeIdentityV1 {
                                            execution_id: execution_id.clone(),
                                            scenario_id: scenario_id.as_str().into(),
                                            scenario_contract_sha256,
                                            workflow_id: runtime.materialized.definition.id.clone(),
                                            workflow_sha256: workflow_sha256?,
                                            catalog_sha256: catalog_sha256?,
                                            policy_sha256: runtime
                                                .materialized
                                                .policy_sha256
                                                .clone(),
                                            plan_sha256: runtime
                                                .materialized
                                                .latest_plan_sha256
                                                .clone(),
                                            system_identity_sha256: system_identity_sha256.into(),
                                            model: subject.model.clone(),
                                            provider: subject.provider.clone(),
                                        })
                                    });
                                let outcome = match (identity, resume_store) {
                                    (Ok(identity), Ok(_)) => {
                                        execute_adaptive_workflow(
                                            &runtime.policy,
                                            &runtime.plans,
                                            &runtime.completed_node_ids,
                                            runtime.catalog,
                                            WorkflowExecutionRequest {
                                                output_dir: output.to_path_buf(),
                                                run_id: run_id.to_string(),
                                                attempt_id: Some(attempt_id.clone()),
                                                attempt_number,
                                                cancellation,
                                                cleanup_hook: runtime.cleanup_hook,
                                            },
                                            ResumableWorkflowExecutionRequest {
                                                state_root,
                                                identity,
                                                plan_revisions: Vec::new(),
                                                resume_existing,
                                            },
                                        )
                                        .await
                                    }
                                    (Err(error), _) | (_, Err(error)) => Err(error),
                                };
                                if uses_harness {
                                    if let Err(error) = context.unbind_turn_completed().await {
                                        report.push_failure(
                                            RunStatus::InfrastructureError,
                                            FailurePhase::Cleanup,
                                            format!(
                                                "unbind adaptive Harness observation: {error:#}"
                                            ),
                                        );
                                    }
                                }
                                match outcome {
                                    Ok(ResumableWorkflowOutcome::Completed(workflow)) => {
                                        populate_composite_report(&mut report, *workflow)
                                    }
                                    Ok(ResumableWorkflowOutcome::ExplicitlyCancelled) => {
                                        report.push_failure(
                                            RunStatus::InfrastructureError,
                                            FailurePhase::Execute,
                                            "adaptive workflow was cancelled",
                                        );
                                    }
                                    Ok(ResumableWorkflowOutcome::NeedsReconciliation(needs)) => {
                                        let _ = emit_event(
                                            control,
                                            SuiteEvent::AdaptiveResumeState {
                                                attempt_id: attempt_id.clone(),
                                                state_sha256: needs.resume_state_sha256.clone(),
                                            },
                                        )
                                        .await;
                                        report.push_failure(
                                            RunStatus::InfrastructureError,
                                            FailurePhase::Execute,
                                            format!(
                                                "needs_reconciliation:{}:{}",
                                                needs.node_id, needs.reason
                                            ),
                                        );
                                    }
                                    Err(error) => report.push_failure(
                                        RunStatus::InfrastructureError,
                                        FailurePhase::Execute,
                                        format!("execute adaptive scenario: {error:#}"),
                                    ),
                                }
                                if let Some(planner_cost) = planner_cost {
                                    let workflow_cost = report.cost.subject_usd.unwrap_or(0.0);
                                    report.cost.subject_usd = Some(workflow_cost + planner_cost);
                                    report.cost.total_usd = report.cost.subject_usd;
                                }
                                if let (Some(actual), Some(limit)) = (
                                    report.cost.subject_usd,
                                    case.inputs["workflow_resource_budgets"]["max_cost_usd"]
                                        .as_f64(),
                                ) {
                                    if actual > limit {
                                        report.push_failure(
                                            RunStatus::ResourceLimit,
                                            FailurePhase::Execute,
                                            format!(
                                                "adaptive aggregate subject cost ${actual:.6} exceeded the scenario envelope ${limit:.6}"
                                            ),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(error) => report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Setup,
                format!("materialize adaptive runtime: {error:#}"),
            ),
        }
    }
    report.wall_time_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    if report.assessment_results.is_empty() {
        ensure_assessment_results(&spec, &mut report);
    }
    report.update_efficiency(case.work);
    report.refresh_dimensions(false);
    let _ = emit_phase(control, SuitePhase::Persisting).await;
    let _ = emit_event(
        control,
        SuiteEvent::AttemptFinished {
            attempt_id: attempt_id.clone(),
        },
    )
    .await;
    report
}

fn adaptive_planner_failure_status(message: &str) -> RunStatus {
    if [
        "strict adaptive planner JSON",
        "agent-authored adaptive plans",
        "adaptive plan revision",
        "adaptive planner response",
        "trusted evidence ids",
        "unknown template",
        "node bound",
        "plan depth",
    ]
    .iter()
    .any(|signal| message.contains(signal))
    {
        RunStatus::SubjectError
    } else {
        RunStatus::InfrastructureError
    }
}

fn adaptive_planner_metadata(
    scenario_id: ScenarioId,
    spec: &ScenarioSpec,
) -> Result<AdaptivePlannerMetadataV1> {
    let invalidation = match scenario_id {
        ScenarioId::IncidentResponse => AdaptivePlannerInvalidationV1 {
            description: "A trusted candidate-validation probe invalidated the initial diagnosis-only plan and requires bounded remediation plus revalidation before terminal action."
                .into(),
            evidence_ids: vec![
                crate::workflow::incident_response::INVALIDATION_EVIDENCE_ID.into(),
            ],
        },
        ScenarioId::ReleaseTrainRecovery => AdaptivePlannerInvalidationV1 {
            description: "The trusted promotion preview exposed an incompatible historical latest graph and invalidated the stale null-CAS operation."
                .into(),
            evidence_ids: vec![
                crate::workflow::release_train_recovery::INVALIDATION_EVIDENCE_ID.into(),
            ],
        },
        ScenarioId::CrossRepoContractMigration => AdaptivePlannerInvalidationV1 {
            description: "The trusted canary revealed consumer B and proved that the v2-only route plan breaks backwards compatibility."
                .into(),
            evidence_ids: vec![
                crate::workflow::cross_repo_contract_migration::CANARY_EVIDENCE_ID.into(),
            ],
        },
        _ => bail!(
            "scenario '{}' has no runner-owned adaptive invalidation",
            scenario_id.as_str()
        ),
    };
    Ok(AdaptivePlannerMetadataV1 {
        scenario_id: scenario_id.as_str().into(),
        objective: spec.prompt.clone(),
        reference_checks: spec
            .criteria
            .iter()
            .map(|criterion| AdaptivePlannerReferenceCheckV1 {
                id: criterion.id.into(),
                description: criterion.description.into(),
            })
            .collect(),
        invalidation,
    })
}

async fn run_composite_once(
    context: &Arc<E2eContext>,
    request: CompositeAttemptRequest<'_>,
) -> E2eRunReport {
    let CompositeAttemptRequest {
        scenario_id,
        run_id,
        attempt_number,
        subject,
        seed,
        control,
        output,
        attempt_id,
        started,
    } = request;
    let session_id = format!("scenario_{attempt_id}");
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
                format!("composite scenario materialization failed: {error:#}"),
            );
            report.wall_time_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            ensure_assessment_results(&spec, &mut report);
            report.refresh_dimensions(false);
            return report;
        }
    };
    let MaterializedScenario { spec, case, .. } = materialized;
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
            scenario_id: scenario_id.into(),
            run_id: run_id.to_string(),
            attempt_id: attempt_id.clone(),
            session_id,
            resume_state_path: None,
        },
    )
    .await
    {
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Setup,
            format!("persist attempt checkpoint: {error:#}"),
        );
    }

    if report.failures.is_empty() {
        if let Err(error) = emit_phase(control, SuitePhase::SettingUp).await {
            report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Setup,
                format!("persist setup checkpoint: {error:#}"),
            );
        }
    }

    if report.failures.is_empty() {
        match composite_runtime(
            scenario_id,
            context.clone(),
            &subject.model,
            &subject.provider,
        ) {
            Ok(runtime) => {
                let uses_harness = runtime
                    .definition
                    .nodes
                    .iter()
                    .any(|node| node.step_type == crate::workflow::HARNESS_STEP_ID);
                let bind_result = if uses_harness {
                    context.bind_turn_completed().await
                } else {
                    Ok(())
                };
                if let Err(error) = bind_result {
                    report.push_failure(
                        RunStatus::InfrastructureError,
                        FailurePhase::Setup,
                        format!("bind composite Harness observation: {error:#}"),
                    );
                } else {
                    let _ = emit_phase(control, SuitePhase::Executing).await;
                    let cancellation = control.map_or_else(
                        || watch::channel(false).1,
                        |control| control.cancellation.clone(),
                    );
                    let outcome = execute_workflow(
                        &runtime.definition,
                        runtime.catalog,
                        WorkflowExecutionRequest {
                            output_dir: output.to_path_buf(),
                            run_id: run_id.to_string(),
                            attempt_id: Some(attempt_id.clone()),
                            attempt_number,
                            cancellation,
                            cleanup_hook: runtime.cleanup_hook,
                        },
                    )
                    .await;
                    if uses_harness {
                        if let Err(error) = context.unbind_turn_completed().await {
                            report.push_failure(
                                RunStatus::InfrastructureError,
                                FailurePhase::Cleanup,
                                format!("unbind composite Harness observation: {error:#}"),
                            );
                        }
                    }
                    match outcome {
                        Ok(workflow) => populate_composite_report(&mut report, workflow),
                        Err(error) => report.push_failure(
                            RunStatus::InfrastructureError,
                            FailurePhase::Execute,
                            format!("execute composite scenario: {error:#}"),
                        ),
                    }
                }
            }
            Err(error) => report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Setup,
                format!("materialize composite runtime: {error:#}"),
            ),
        }
    }

    report.wall_time_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    if report.assessment_results.is_empty() {
        ensure_assessment_results(&spec, &mut report);
    }
    report.update_efficiency(case.work);
    report.refresh_dimensions(false);
    let _ = emit_phase(control, SuitePhase::Persisting).await;
    let _ = emit_event(
        control,
        SuiteEvent::AttemptFinished {
            attempt_id: attempt_id.clone(),
        },
    )
    .await;
    report
}

fn populate_composite_report(
    report: &mut E2eRunReport,
    workflow: crate::workflow::WorkflowAttemptReport,
) {
    report.session_id = workflow
        .steps
        .iter()
        .find_map(|step| step.harness_session_id.clone())
        .unwrap_or_else(|| format!("scenario_{}", workflow.attempt_id));
    report.wall_time_ms = workflow.duration_ms;
    report.hard_gates = workflow
        .steps
        .iter()
        .flat_map(|step| {
            let required_completion = step.required.then(|| HardGateReport {
                id: format!("{}.required_test_completion", step.node_id),
                dimension: EvaluationDimension::StructuralIntegrity,
                passed: step.status == WorkflowStepStatus::Succeeded,
                reason: if step.status == WorkflowStepStatus::Succeeded {
                    "required semantic test completed successfully".into()
                } else {
                    format!("required semantic test ended with status {:?}", step.status)
                },
            });
            required_completion
                .into_iter()
                .chain(step.hard_gates.iter().map(|gate| HardGateReport {
                    id: format!("{}.{}", step.node_id, gate.id),
                    dimension: EvaluationDimension::StructuralIntegrity,
                    passed: gate.passed,
                    reason: gate.reason.clone(),
                }))
        })
        .collect();
    report.criteria = workflow
        .criteria
        .iter()
        .map(|criterion| CriterionReport {
            id: criterion.id.clone(),
            possible: criterion.weight,
            awarded: criterion.score.and_then(|score| {
                score
                    .is_finite()
                    .then(|| (score.clamp(0.0, 1.0) * f64::from(criterion.weight)).round() as u8)
            }),
            reason: criterion.summary.clone(),
        })
        .collect();
    let evaluated = workflow
        .criteria
        .iter()
        .filter_map(|criterion| {
            criterion
                .score
                .filter(|score| score.is_finite())
                .map(|score| {
                    (
                        score.clamp(0.0, 1.0) * f64::from(criterion.weight),
                        criterion.weight,
                    )
                })
        })
        .collect::<Vec<_>>();
    let evaluated_weight = evaluated
        .iter()
        .map(|(_, weight)| u16::from(*weight))
        .sum::<u16>();
    report.score = (evaluated_weight > 0).then(|| {
        (evaluated.iter().map(|(score, _)| score).sum::<f64>() / f64::from(evaluated_weight)
            * 100.0)
            .round()
            .clamp(0.0, 100.0) as u8
    });
    report.cost = CostReport {
        subject_usd: workflow.aggregate_cost_usd,
        judge_usd: Some(0.0),
        total_usd: workflow.aggregate_cost_usd,
    };
    for step in &workflow.steps {
        for failure in &step.failures {
            report.push_failure(
                if failure.technical {
                    RunStatus::InfrastructureError
                } else {
                    RunStatus::SubjectError
                },
                workflow_failure_phase(failure.phase),
                format!("semantic test '{}': {}", step.node_id, failure.message),
            );
        }
    }
    if workflow.cleanup.status == WorkflowCleanupStatus::Failed {
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Cleanup,
            workflow
                .cleanup
                .failure
                .clone()
                .unwrap_or_else(|| "mandatory composite cleanup failed".into()),
        );
    }
    if workflow.technical_failure
        && !report
            .failures
            .iter()
            .any(|failure| failure.domain == crate::report::FailureDomain::E2eInfrastructure)
    {
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Execute,
            "composite scenario ended with an unclassified technical failure",
        );
    }
    report.assessment_results =
        crate::assessment::semantic_test_assessments(&workflow.steps, &workflow.criteria);
    report.evidence.push(workflow.checkpoint.clone());
    report.scenario_flow = Some(ScenarioFlowEvidence {
        definition_sha256: workflow.workflow_sha256.clone(),
        snapshot: workflow.flow_snapshot.clone(),
        checkpoint: workflow.checkpoint.clone(),
        cleanup: workflow.cleanup.clone(),
    });
    report.semantic_tests = workflow.steps;
    if report.failures.is_empty() {
        report.finish(if workflow.passed {
            RunStatus::Passed
        } else {
            RunStatus::HardGateFailed
        });
    }
}

fn workflow_failure_phase(phase: WorkflowFailurePhase) -> FailurePhase {
    match phase {
        WorkflowFailurePhase::Preflight => FailurePhase::Setup,
        WorkflowFailurePhase::Execute | WorkflowFailurePhase::Cancel => FailurePhase::Execute,
        WorkflowFailurePhase::Capture | WorkflowFailurePhase::Persist => FailurePhase::Collect,
        WorkflowFailurePhase::Evaluate => FailurePhase::Evaluate,
        WorkflowFailurePhase::Cleanup => FailurePhase::Cleanup,
    }
}

pub(crate) fn markdown_case(
    scenario: &CompiledMarkdownScenario,
    seed: u64,
) -> Result<ScenarioCase> {
    ScenarioCase::new(
        scenario.id.clone(),
        scenario.version,
        seed,
        json!({
            "source_path": scenario.source_path,
            "plans": scenario.plans,
            "source_sha256": scenario.source_sha256,
            "behavior_sha256": scenario.behavior_sha256,
            "compiled_sha256": scenario.compiled_sha256,
        }),
        ComplexityProfile {
            planning_depth: 2,
            dependency_depth: 1,
            external_systems: 1,
            state_transitions: 2,
            validation_loops: 1,
            compensable_mutations: 1,
            ..ComplexityProfile::default()
        },
        vec![
            "iii::functions".into(),
            "iii::database".into(),
            "iii::state".into(),
        ],
        DeliverableContract::default(),
    )
}

struct MarkdownRetryRequest<'a> {
    scenario: &'a CompiledMarkdownScenario,
    source: &'a str,
    subject: &'a SubjectConfig,
    auxiliary: &'a JudgeConfig,
    audit_analyzer: Option<&'a JudgeConfig>,
    seed: u64,
    technical_retries: u8,
    progress_interval: Option<Duration>,
    control: Option<&'a SuiteControl>,
    output: &'a std::path::Path,
    system_identity_sha256: &'a str,
    runs: u32,
    materialized_plan: Option<&'a serde_json::Value>,
}

async fn run_markdown_with_technical_retries(
    context: &Arc<E2eContext>,
    request: MarkdownRetryRequest<'_>,
) -> E2eRunReport {
    let run_id = Uuid::new_v4().simple().to_string();
    let mut retry_attempts = Vec::with_capacity(request.technical_retries as usize);
    loop {
        let attempt_number = retry_attempts.len() as u32 + 1;
        let mut report = run_markdown_once(
            context,
            MarkdownAttemptRequest {
                scenario: request.scenario,
                source: request.source,
                subject: request.subject,
                auxiliary: request.auxiliary,
                audit_analyzer: request.audit_analyzer,
                run_id: &run_id,
                attempt_number,
                seed: request.seed,
                progress_interval: request.progress_interval,
                control: request.control,
                output: request.output,
                system_identity_sha256: request.system_identity_sha256,
                technical_retries: request.technical_retries,
                runs: request.runs,
                materialized_plan: request.materialized_plan,
            },
        )
        .await;
        if retry_attempts.len() < request.technical_retries as usize
            && is_retryable_technical_failure(&report)
            && request
                .control
                .is_none_or(|control| !*control.cancellation.borrow())
        {
            let reason = report
                .failures
                .first()
                .map(|failure| failure.message.as_str())
                .unwrap_or("transient technical failure");
            tracing::warn!(
                scenario = request.scenario.id,
                attempt = attempt_number,
                max_retries = request.technical_retries,
                reason,
                "retrying Markdown scenario after a technical failure"
            );
            retry_attempts.push(RetryAttemptReport::from(&report));
            continue;
        }
        report.attach_retry_attempts(retry_attempts);
        return report;
    }
}

struct MarkdownAttemptRequest<'a> {
    scenario: &'a CompiledMarkdownScenario,
    source: &'a str,
    subject: &'a SubjectConfig,
    auxiliary: &'a JudgeConfig,
    audit_analyzer: Option<&'a JudgeConfig>,
    run_id: &'a str,
    attempt_number: u32,
    seed: u64,
    progress_interval: Option<Duration>,
    control: Option<&'a SuiteControl>,
    output: &'a std::path::Path,
    system_identity_sha256: &'a str,
    technical_retries: u8,
    runs: u32,
    materialized_plan: Option<&'a serde_json::Value>,
}

struct MarkdownSessionObservation {
    session_id: String,
    metrics: crate::wire::SessionMetricsResponse,
    transcript: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidatorDecision {
    verdict: String,
    reason: String,
    #[serde(default)]
    evidence: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdherenceDecision {
    summary: String,
    requirements: Vec<AdherenceRequirement>,
}

async fn run_markdown_once(
    context: &Arc<E2eContext>,
    request: MarkdownAttemptRequest<'_>,
) -> E2eRunReport {
    let started = Instant::now();
    let attempt_id = Uuid::new_v4().simple().to_string();
    let subject_session_id = format!("e2e_{attempt_id}");
    let rendered_run_id = request
        .materialized_plan
        .and_then(|plan| plan.pointer("/rendered/run_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|run_id| safe_markdown_run_id(run_id))
        .unwrap_or(request.run_id);
    let rendered = crate::markdown::render(request.scenario, rendered_run_id, request.seed);
    let mut report = E2eRunReport::new(
        request.run_id.to_string(),
        attempt_id.clone(),
        request.attempt_number,
        subject_session_id.clone(),
        rendered.prompt.clone(),
    );
    let mut phases = markdown_phases(&rendered);
    let mut session_ids = Vec::new();
    let mut setup_receipts = Vec::new();
    let mut subject_receipts = Vec::new();
    let mut cleanup_receipts = Vec::new();
    let mut setup_started = false;
    let generated_plan = materialized_markdown_plan(&request, &rendered);
    let plan = request
        .materialized_plan
        .cloned()
        .unwrap_or_else(|| generated_plan.clone());
    if plan != generated_plan {
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Setup,
            "materialized Markdown plan does not match the current scenario, models, policies, budgets, stack, runner, runs, or retries",
        );
    }
    let plan_sha256 = match artifact::sha256_value(&plan) {
        Ok(hash) => Some(hash),
        Err(error) => {
            report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Setup,
                format!("hash materialized Markdown plan: {error:#}"),
            );
            None
        }
    };
    report.markdown_execution = Some(MarkdownExecutionReport {
        source_path: request.scenario.source_path.clone(),
        source_sha256: request.scenario.source_sha256.clone(),
        behavior_sha256: request.scenario.behavior_sha256.clone(),
        compiled_sha256: request.scenario.compiled_sha256.clone(),
        materialized_plan_sha256: plan_sha256.clone(),
        prompt_sha256: artifact::sha256_bytes(rendered.prompt.as_bytes()),
        pipeline_complete: false,
        phases: Vec::new(),
    });

    if let Err(error) = emit_event(
        request.control,
        SuiteEvent::AttemptStarted {
            scenario_id: ScenarioKey::Markdown(request.scenario.id.clone()),
            run_id: request.run_id.to_string(),
            attempt_id: attempt_id.clone(),
            session_id: subject_session_id.clone(),
            resume_state_path: None,
        },
    )
    .await
    {
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Setup,
            format!("persist Markdown attempt checkpoint: {error:#}"),
        );
    }

    if report.failures.is_empty() {
        if let Err(error) = persist_markdown_inputs(
            request.output,
            request.scenario,
            request.source,
            request.run_id,
            &attempt_id,
            &plan,
            &mut report,
        ) {
            report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Setup,
                format!("persist immutable Markdown inputs: {error:#}"),
            );
        }
    }
    if report.failures.is_empty() {
        if let Err(error) = emit_phase(request.control, SuitePhase::SettingUp).await {
            report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Setup,
                format!("persist setup checkpoint: {error:#}"),
            );
        }
    }
    if report.failures.is_empty() {
        if let Err(error) = context.bind_turn_completed().await {
            report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Setup,
                format!("bind harness::turn-completed: {error:#}"),
            );
        }
    }

    if report.failures.is_empty() {
        let setup_session_id = format!("e2e_{attempt_id}_setup");
        setup_started = true;
        session_ids.push(setup_session_id.clone());
        let setup_prompt = format!(
            "Prepare the isolated environment for the upcoming test. Execute only the authored setup instructions below. Do not attempt the test task itself. Every mutation must be run-scoped and reversible.\n\n{}",
            rendered.before_test
        );
        match run_markdown_session(
            context,
            MarkdownSessionRequest {
                scenario_id: &request.scenario.id,
                phase: "setup",
                session_id: &setup_session_id,
                prompt: &setup_prompt,
                model: &request.auxiliary.model,
                provider: &request.auxiliary.provider,
                functions: markdown_setup_policy(),
                max_turns: 16,
                max_output_tokens: Some(4_096),
                max_total_tokens: Some(80_000),
                max_validation_retries: Some(1),
                stuck_timeout: Duration::from_secs(180),
                progress_interval: request.progress_interval,
                control: request.control,
                idempotency_key: format!("e2e:{}:setup", attempt_id),
            },
        )
        .await
        {
            Ok(observation) => {
                setup_receipts = function_call_receipts(&observation.transcript);
                let setup_gaps = markdown_setup_gaps(
                    &rendered.before_test,
                    &setup_receipts,
                    observation.metrics.totals.function_call_errors,
                );
                if setup_gaps.is_empty() {
                    complete_markdown_phase(&mut phases, "setup", &observation, "");
                } else {
                    let reason = format!(
                        "setup did not perform the required operations: {}",
                        setup_gaps.join(", ")
                    );
                    fail_markdown_phase(&mut phases, "setup", &setup_session_id, &reason);
                    report.push_failure(
                        RunStatus::InfrastructureError,
                        FailurePhase::Setup,
                        format!("Markdown {reason}"),
                    );
                }
                if let Err(error) = persist_markdown_session(
                    request.output,
                    request.run_id,
                    &attempt_id,
                    "setup",
                    &observation,
                    &mut report,
                ) {
                    report.push_failure(
                        RunStatus::InfrastructureError,
                        FailurePhase::Collect,
                        format!("persist setup evidence: {error:#}"),
                    );
                }
            }
            Err(error) => {
                fail_markdown_phase(&mut phases, "setup", &setup_session_id, &error.to_string());
                report.push_failure(
                    RunStatus::InfrastructureError,
                    FailurePhase::Setup,
                    format!("Markdown setup failed: {error:#}"),
                );
            }
        }
    }

    if report.failures.is_empty() {
        let _ = emit_phase(request.control, SuitePhase::Executing).await;
        session_ids.push(subject_session_id.clone());
        match run_markdown_session(
            context,
            MarkdownSessionRequest {
                scenario_id: &request.scenario.id,
                phase: "subject",
                session_id: &subject_session_id,
                prompt: &rendered.prompt,
                model: &request.subject.model,
                provider: &request.subject.provider,
                functions: markdown_subject_policy(),
                max_turns: crate::markdown::execution_policy().max_turns,
                max_output_tokens: crate::markdown::execution_policy().max_output_tokens,
                max_total_tokens: crate::markdown::execution_policy().max_total_tokens,
                max_validation_retries: crate::markdown::execution_policy().max_validation_retries,
                stuck_timeout: Duration::from_secs(
                    crate::markdown::execution_policy().stuck_timeout_seconds,
                ),
                progress_interval: request.progress_interval,
                control: request.control,
                idempotency_key: format!("e2e:{}:subject", attempt_id),
            },
        )
        .await
        {
            Ok(observation) => {
                subject_receipts = function_call_receipts(&observation.transcript);
                report.metrics = Some(observation.metrics.clone());
                report.transcript = Some(observation.transcript.clone());
                complete_markdown_phase(&mut phases, "subject", &observation, "");
                if let Err(error) = persist_markdown_session(
                    request.output,
                    request.run_id,
                    &attempt_id,
                    "subject",
                    &observation,
                    &mut report,
                ) {
                    report.push_failure(
                        RunStatus::InfrastructureError,
                        FailurePhase::Collect,
                        format!("persist subject evidence: {error:#}"),
                    );
                }
            }
            Err(error) => {
                fail_markdown_phase(
                    &mut phases,
                    "subject",
                    &subject_session_id,
                    &error.to_string(),
                );
                report.push_failure(
                    RunStatus::SubjectError,
                    FailurePhase::Execute,
                    format!("Markdown subject failed: {error:#}"),
                );
            }
        }
    }

    if report.failures.is_empty() {
        let _ = emit_phase(request.control, SuitePhase::Collecting).await;
        evaluate_markdown_validations(
            context,
            &request,
            &rendered.validations,
            &attempt_id,
            &mut session_ids,
            &mut phases,
            &mut report,
        )
        .await;
    }

    if report.transcript.is_some() {
        let mut adherence_input = markdown_adherence_input(&rendered.prompt, &report);
        let mut adherence = match redact_markdown_artifact(&mut report, &mut adherence_input) {
            Ok(()) => {
                evaluate_markdown_adherence(context, request.auxiliary, &adherence_input).await
            }
            Err(error) => {
                report.push_failure(
                    RunStatus::InfrastructureError,
                    FailurePhase::Collect,
                    format!("prepare instruction-adherence evidence: {error:#}"),
                );
                InstructionAdherenceReport {
                    availability: AdherenceAvailability::Failed,
                    score: None,
                    summary: format!("instruction-adherence evidence was unavailable: {error:#}"),
                    requirements: Vec::new(),
                    analyzer: None,
                    analyzer_usage: None,
                }
            }
        };
        let adherence_phase = phases
            .iter_mut()
            .find(|phase| phase.phase == "adherence")
            .expect("adherence phase exists");
        adherence_phase.status = MarkdownPhaseStatus::Completed;
        adherence_phase.reason = format!("{:?}", adherence.availability).to_ascii_lowercase();
        let root = std::path::PathBuf::from("evidence")
            .join(request.run_id)
            .join(&attempt_id);
        let adherence_artifact =
            redact_markdown_artifact(&mut report, &mut adherence).and_then(|()| {
                artifact::write_json(
                    request.output,
                    &root.join("instruction-adherence.json"),
                    format!("{attempt_id}-instruction-adherence"),
                    "instruction-adherence",
                    &adherence,
                )
            });
        match adherence_artifact {
            Ok(reference) => report.evidence.push(reference),
            Err(error) => {
                adherence_phase.status = MarkdownPhaseStatus::Failed;
                adherence_phase.reason = format!("persist adherence evidence: {error:#}");
                report.push_failure(
                    RunStatus::InfrastructureError,
                    FailurePhase::Collect,
                    format!("persist instruction-adherence evidence: {error:#}"),
                );
            }
        }
        report.instruction_adherence = Some(adherence);
    }

    let _ = emit_phase(request.control, SuitePhase::CleaningUp).await;
    if setup_started || !subject_receipts.is_empty() {
        let cleanup_session_id = format!("e2e_{attempt_id}_cleanup");
        session_ids.push(cleanup_session_id.clone());
        let cleanup_actions =
            markdown_cleanup_actions(&setup_receipts, &subject_receipts).join("\n");
        let cleanup_prompt = format!(
            "Clean up only the run-scoped mutations represented by the trusted receipts below. Perform exactly the required cleanup actions, verify them with the corresponding read-only checks, and then stop. Never inspect, list, modify, delete, drop, or remove resources that are not named in the receipts. A surface with no mutating receipt is outside the cleanup scope. Do not create new persistent state.\n\nRequired cleanup actions derived from the receipts:\n{}\n\nAuthored setup:\n{}\n\nSetup receipts:\n{}\n\nSubject receipts:\n{}",
            cleanup_actions,
            rendered.before_test,
            serde_json::to_string(&setup_receipts).unwrap_or_else(|_| "[]".into()),
            serde_json::to_string(&subject_receipts).unwrap_or_else(|_| "[]".into()),
        );
        match run_markdown_session(
            context,
            MarkdownSessionRequest {
                scenario_id: &request.scenario.id,
                phase: "cleanup",
                session_id: &cleanup_session_id,
                prompt: &cleanup_prompt,
                model: &request.auxiliary.model,
                provider: &request.auxiliary.provider,
                functions: markdown_cleanup_policy(&setup_receipts, &subject_receipts),
                max_turns: 16,
                max_output_tokens: Some(4_096),
                max_total_tokens: Some(160_000),
                max_validation_retries: Some(1),
                stuck_timeout: Duration::from_secs(180),
                progress_interval: request.progress_interval,
                control: request.control,
                idempotency_key: format!("e2e:{}:cleanup", attempt_id),
            },
        )
        .await
        {
            Ok(observation) => {
                cleanup_receipts = function_call_receipts(&observation.transcript);
                let cleanup_gaps =
                    markdown_cleanup_gaps(&setup_receipts, &subject_receipts, &cleanup_receipts);
                if !cleanup_gaps.is_empty() {
                    let reason = format!(
                        "cleanup did not reverse the required mutations: {}",
                        cleanup_gaps.join(", ")
                    );
                    fail_markdown_phase(&mut phases, "cleanup", &cleanup_session_id, &reason);
                    report.push_failure(
                        RunStatus::InfrastructureError,
                        FailurePhase::Cleanup,
                        format!("Markdown {reason}"),
                    );
                } else {
                    complete_markdown_phase(&mut phases, "cleanup", &observation, "");
                }
                if let Err(error) = persist_markdown_session(
                    request.output,
                    request.run_id,
                    &attempt_id,
                    "cleanup",
                    &observation,
                    &mut report,
                ) {
                    report.push_failure(
                        RunStatus::InfrastructureError,
                        FailurePhase::Collect,
                        format!("persist cleanup evidence: {error:#}"),
                    );
                }
            }
            Err(error) => {
                fail_markdown_phase(
                    &mut phases,
                    "cleanup",
                    &cleanup_session_id,
                    &error.to_string(),
                );
                report.push_failure(
                    RunStatus::InfrastructureError,
                    FailurePhase::Cleanup,
                    format!("Markdown cleanup failed: {error:#}"),
                );
            }
        }
    } else {
        let cleanup = phases
            .iter_mut()
            .find(|phase| phase.phase == "cleanup")
            .expect("cleanup phase exists");
        cleanup.status = MarkdownPhaseStatus::Completed;
        cleanup.reason = "no mutable function receipts were observed".into();
    }

    let receipt_root = std::path::PathBuf::from("evidence")
        .join(request.run_id)
        .join(&attempt_id);
    let mut receipts = json!({
        "setup": setup_receipts,
        "subject": subject_receipts,
        "cleanup": cleanup_receipts,
    });
    let receipt_artifact = redact_markdown_artifact(&mut report, &mut receipts).and_then(|()| {
        artifact::write_json(
            request.output,
            &receipt_root.join("receipts.json"),
            format!("{attempt_id}-receipts"),
            "markdown-function-receipts",
            &receipts,
        )
    });
    match receipt_artifact {
        Ok(reference) => report.evidence.push(reference),
        Err(error) => report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Collect,
            format!("persist Markdown function receipts: {error:#}"),
        ),
    }

    for session_id in session_ids.iter().rev() {
        if let Err(error) = context.teardown(session_id).await {
            report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Cleanup,
                format!("teardown Markdown session '{session_id}': {error:#}"),
            );
        }
    }
    if let Err(error) = context.unbind_turn_completed().await {
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Cleanup,
            format!("unbind harness::turn-completed: {error:#}"),
        );
    }

    let pipeline_complete = phases
        .iter()
        .all(|phase| phase.status == MarkdownPhaseStatus::Completed);
    if let Some(execution) = report.markdown_execution.as_mut() {
        execution.pipeline_complete = pipeline_complete;
        execution.phases = phases;
    }
    report.wall_time_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    if report.failures.is_empty() {
        if report.validation_score.is_some() && pipeline_complete {
            report.finish(RunStatus::Passed);
        } else {
            report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Evaluate,
                "Markdown evaluation completed without an available score or complete pipeline",
            );
        }
    }
    report.update_cost(true);
    if let Ok(case) = markdown_case(request.scenario, request.seed) {
        report.update_efficiency(case.work);
    }
    report.refresh_dimensions(false);
    let audit_case = markdown_case(request.scenario, request.seed).ok();
    report.audit = Some(
        crate::audit::run_markdown_audit(
            context.as_ref(),
            request.audit_analyzer,
            &rendered.prompt,
            &markdown_subject_policy().deny,
            audit_case.as_ref(),
            &report,
        )
        .await,
    );
    if let Err(error) = emit_event(
        request.control,
        SuiteEvent::AttemptFinished {
            attempt_id: attempt_id.clone(),
        },
    )
    .await
    {
        report.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Cleanup,
            format!("persist Markdown attempt completion: {error:#}"),
        );
    }
    report
}

struct MarkdownSessionRequest<'a> {
    scenario_id: &'a str,
    phase: &'a str,
    session_id: &'a str,
    prompt: &'a str,
    model: &'a str,
    provider: &'a str,
    functions: FunctionPolicy,
    max_turns: u32,
    max_output_tokens: Option<u64>,
    max_total_tokens: Option<u64>,
    max_validation_retries: Option<u32>,
    stuck_timeout: Duration,
    progress_interval: Option<Duration>,
    control: Option<&'a SuiteControl>,
    idempotency_key: String,
}

async fn run_markdown_session(
    context: &E2eContext,
    request: MarkdownSessionRequest<'_>,
) -> Result<MarkdownSessionObservation> {
    ensure_not_cancelled(request.control)?;
    context.drain_turn_completed_events();
    let response: SendResponse = context
        .trigger(
            "harness::send",
            SendRequest {
                session_id: Some(request.session_id.to_string()),
                message: MessageInput::Text(request.prompt.to_string()),
                model: Some(request.model.to_string()),
                provider: Some(request.provider.to_string()),
                idempotency_key: Some(request.idempotency_key),
                session: Some(SessionInit {
                    title: Some(format!(
                        "Harness E2E Markdown: {} ({})",
                        request.scenario_id, request.phase
                    )),
                    metadata: Some(json!({
                        "e2e_scenario": request.scenario_id,
                        "e2e_markdown_phase": request.phase,
                    })),
                }),
                options: Some(SendOptions {
                    max_turns: Some(request.max_turns),
                    max_output_tokens: request.max_output_tokens,
                    max_total_tokens: request.max_total_tokens,
                    max_validation_retries: request.max_validation_retries,
                    functions: Some(request.functions),
                    metadata: None,
                }),
            },
        )
        .await
        .with_context(|| format!("send Markdown {} session", request.phase))?;
    if !response.accepted
        || response.session_id != request.session_id
        || response.merged == Some(true)
        || response.queued == Some(true)
    {
        bail!(
            "harness::send returned an unexpected response for Markdown {}: {response:?}",
            request.phase
        );
    }
    let metrics = context
        .wait_for_turn(
            request.scenario_id,
            request.session_id,
            &response.turn_id,
            request.stuck_timeout,
            request.progress_interval.is_some(),
            request.control.map(|control| &control.cancellation),
        )
        .await
        .with_context(|| format!("wait for Markdown {} session", request.phase))?;
    let transcript = context
        .transcript(request.session_id)
        .await
        .with_context(|| format!("collect Markdown {} transcript", request.phase))?;
    Ok(MarkdownSessionObservation {
        session_id: request.session_id.to_string(),
        metrics,
        transcript,
    })
}

async fn evaluate_markdown_validations(
    context: &E2eContext,
    request: &MarkdownAttemptRequest<'_>,
    validations: &[MarkdownCriterion],
    attempt_id: &str,
    session_ids: &mut Vec<String>,
    phases: &mut [MarkdownPhaseReport],
    report: &mut E2eRunReport,
) {
    let Some(transcript) = report.transcript.as_ref() else {
        return;
    };
    let evidence_digest = json!({
        "subject_transcript_sha256": artifact::sha256_value(transcript).ok(),
        "final_response": common::final_response(transcript),
        "function_calls": function_call_receipts(transcript),
        "trusted_metrics": report.metrics,
    });
    let mut score = 0u8;
    let mut available = true;
    for (index, criterion) in validations.iter().enumerate() {
        let session_id = format!("e2e_{attempt_id}_validation_{:02}", index + 1);
        session_ids.push(session_id.clone());
        let prompt = format!(
            "Evaluate exactly one criterion using only the trusted evidence digest and the read-only tools available to you. Return one JSON object and no Markdown: {{\"verdict\":\"passed|failed|inconclusive\",\"reason\":\"brief evidence-based reason\",\"evidence\":[\"specific evidence\"]}}. Use inconclusive whenever the evidence or a required read is unavailable.\n\nCriterion: {}\nWeight: {}%\nInstructions:\n{}\n\nTrusted evidence digest:\n{}",
            criterion.title,
            criterion.weight,
            criterion.instructions,
            serde_json::to_string(&evidence_digest).unwrap_or_else(|_| "{}".into()),
        );
        match run_markdown_session(
            context,
            MarkdownSessionRequest {
                scenario_id: &request.scenario.id,
                phase: "validation",
                session_id: &session_id,
                prompt: &prompt,
                model: &request.auxiliary.model,
                provider: &request.auxiliary.provider,
                functions: markdown_validator_policy(),
                max_turns: 8,
                max_output_tokens: Some(2_048),
                max_total_tokens: Some(80_000),
                max_validation_retries: Some(1),
                stuck_timeout: Duration::from_secs(120),
                progress_interval: request.progress_interval,
                control: request.control,
                idempotency_key: format!("e2e:{attempt_id}:validation:{index}"),
            },
        )
        .await
        {
            Ok(observation) => {
                if let Err(error) = persist_markdown_session(
                    request.output,
                    request.run_id,
                    attempt_id,
                    &format!("validation-{:02}", index + 1),
                    &observation,
                    report,
                ) {
                    available = false;
                    report.push_failure(
                        RunStatus::InfrastructureError,
                        FailurePhase::Collect,
                        format!("persist validator evidence: {error:#}"),
                    );
                    continue;
                }
                let response = common::final_response(&observation.transcript);
                match parse_json_object::<ValidatorDecision>(&response)
                    .and_then(validate_validator_decision)
                {
                    Ok(decision) if decision.verdict == "passed" => {
                        score = score.saturating_add(criterion.weight);
                        report.criteria.push(CriterionReport {
                            id: criterion.id.clone(),
                            possible: criterion.weight,
                            awarded: Some(criterion.weight),
                            reason: validation_reason(&decision),
                        });
                    }
                    Ok(decision) if decision.verdict == "failed" => {
                        report.criteria.push(CriterionReport {
                            id: criterion.id.clone(),
                            possible: criterion.weight,
                            awarded: Some(0),
                            reason: validation_reason(&decision),
                        });
                    }
                    Ok(decision) => {
                        available = false;
                        report.criteria.push(CriterionReport {
                            id: criterion.id.clone(),
                            possible: criterion.weight,
                            awarded: None,
                            reason: validation_reason(&decision),
                        });
                        report.push_failure(
                            RunStatus::JudgeError,
                            FailurePhase::Evaluate,
                            format!("validator '{}' returned inconclusive", criterion.id),
                        );
                    }
                    Err(error) => {
                        available = false;
                        report.criteria.push(CriterionReport {
                            id: criterion.id.clone(),
                            possible: criterion.weight,
                            awarded: None,
                            reason: format!("validator output was invalid: {error:#}"),
                        });
                        report.push_failure(
                            RunStatus::JudgeError,
                            FailurePhase::Evaluate,
                            format!("validator '{}' failed: {error:#}", criterion.id),
                        );
                    }
                }
            }
            Err(error) => {
                available = false;
                report.criteria.push(CriterionReport {
                    id: criterion.id.clone(),
                    possible: criterion.weight,
                    awarded: None,
                    reason: format!("validator session failed: {error:#}"),
                });
                report.push_failure(
                    RunStatus::JudgeError,
                    FailurePhase::Evaluate,
                    format!("validator '{}' session failed: {error:#}", criterion.id),
                );
            }
        }
    }
    let phase = phases
        .iter_mut()
        .find(|phase| phase.phase == "validations")
        .expect("validations phase exists");
    if available && report.criteria.len() == request.scenario.validations.len() {
        report.validation_score = Some(score);
        report.score = Some(score);
        phase.status = MarkdownPhaseStatus::Completed;
        phase.reason = format!("{} validators completed", report.criteria.len());
    } else {
        phase.status = MarkdownPhaseStatus::Failed;
        phase.reason = "one or more validators were unavailable or inconclusive".into();
    }
}

async fn evaluate_markdown_adherence(
    context: &E2eContext,
    config: &JudgeConfig,
    input: &serde_json::Value,
) -> InstructionAdherenceReport {
    let input_sha256 =
        artifact::sha256_value(input).unwrap_or_else(|_| artifact::sha256_bytes(b"unavailable"));
    let analyzer = AnalyzerIdentity {
        analyzer: "instruction-adherence".into(),
        provider: Some(config.provider.clone()),
        model: Some(config.model.clone()),
        input_sha256,
    };
    let prompt = format!(
        "Assess how faithfully the subject followed the authored prompt. Identify each atomic requirement and decide only whether it was followed. The runner calculates the numeric score deterministically; do not return a score. Use only the supplied evidence. Function outcomes are authoritative: a failed call has no successful side effect, and must not violate an outcome requirement when a later successful call plus validation evidence proves the required final state, unless the prompt explicitly prohibits failed attempts. Return one JSON object and no Markdown: {{\"summary\":\"brief conclusion\",\"requirements\":[{{\"id\":\"requirement_1\",\"instruction\":\"requirement text\",\"followed\":false,\"reason\":\"evidence-based reason\",\"confidence\":0.0,\"evidence\":[\"specific evidence\"]}}]}}. Confidence must be from 0 through 1.\n\nInput:\n{}",
        serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
    );
    let started = Instant::now();
    match judge::invoke(
        context,
        config,
        "You are an impartial instruction-adherence evaluator. Return exactly one JSON object without Markdown.",
        &prompt,
        4_096,
    )
    .await
    {
        Ok(response) => {
            let usage = judge::response_usage(&response);
            let analyzer_usage = AnalyzerUsage {
                latency_ms: Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64),
                input_tokens: usage.as_ref().and_then(|usage| usage.input_tokens),
                output_tokens: usage.as_ref().and_then(|usage| usage.output_tokens),
                cost_usd: usage.as_ref().and_then(|usage| usage.cost_usd),
            };
            match parse_json_object::<AdherenceDecision>(&judge::assistant_text(&response))
                .and_then(validate_adherence_decision)
            {
                Ok(decision) => InstructionAdherenceReport {
                    availability: AdherenceAvailability::Available,
                    score: Some(deterministic_adherence_score(&decision.requirements)),
                    summary: decision.summary,
                    requirements: decision.requirements,
                    analyzer: Some(analyzer),
                    analyzer_usage: Some(analyzer_usage),
                },
                Err(error) => InstructionAdherenceReport {
                    availability: AdherenceAvailability::Failed,
                    score: None,
                    summary: format!("instruction-adherence output was invalid: {error:#}"),
                    requirements: Vec::new(),
                    analyzer: Some(analyzer),
                    analyzer_usage: Some(analyzer_usage),
                },
            }
        }
        Err(error) => InstructionAdherenceReport {
            availability: AdherenceAvailability::Failed,
            score: None,
            summary: format!("instruction-adherence analyzer failed: {error:#}"),
            requirements: Vec::new(),
            analyzer: Some(analyzer),
            analyzer_usage: Some(AnalyzerUsage {
                latency_ms: Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64),
                ..AnalyzerUsage::default()
            }),
        },
    }
}

fn markdown_adherence_input(prompt: &str, report: &E2eRunReport) -> serde_json::Value {
    let transcript = report
        .transcript
        .as_ref()
        .unwrap_or(&serde_json::Value::Null);
    json!({
        "prompt": prompt,
        "final_response": common::final_response(transcript),
        "function_outcomes": adherence_function_outcomes(transcript),
        "trusted_metrics": report.metrics,
        "validation_results": adherence_validation_results(&report.criteria),
    })
}

fn adherence_function_outcomes(transcript: &serde_json::Value) -> Vec<serde_json::Value> {
    common::function_outcomes(transcript)
        .into_iter()
        .map(|outcome| {
            let mut result = serde_json::Map::new();
            if let Some(details) = outcome.details.as_ref() {
                for field in ["affected_rows", "row_count"] {
                    if let Some(value) = details.get(field).filter(|value| value.is_number()) {
                        result.insert(field.into(), value.clone());
                    }
                }
                for (field, count_field) in [
                    ("returned_rows", "returned_rows_count"),
                    ("rows", "rows_count"),
                ] {
                    if let Some(count) = details.get(field).and_then(serde_json::Value::as_array) {
                        result.insert(count_field.into(), json!(count.len()));
                    }
                }
            }
            let status = match outcome.is_error {
                Some(false) => "succeeded",
                Some(true) => "failed",
                None => "missing_result",
            };
            let mut evidence = json!({
                "ordinal": outcome.ordinal,
                "function_id": outcome.function_id,
                "arguments": outcome.arguments,
                "status": status,
            });
            if let Some(error_code) = outcome.error_code {
                evidence["error_code"] = json!(error_code);
            }
            if !result.is_empty() {
                evidence["result"] = serde_json::Value::Object(result);
            }
            evidence
        })
        .collect()
}

fn adherence_validation_results(criteria: &[CriterionReport]) -> Vec<serde_json::Value> {
    criteria
        .iter()
        .map(|criterion| {
            let verdict = match criterion.awarded {
                Some(awarded) if awarded == criterion.possible => "passed",
                Some(0) => "failed",
                Some(_) => "partial",
                None => "inconclusive",
            };
            json!({
                "criterion_id": criterion.id,
                "verdict": verdict,
                "reason": criterion.reason,
            })
        })
        .collect()
}

fn deterministic_adherence_score(requirements: &[AdherenceRequirement]) -> u8 {
    let followed = requirements
        .iter()
        .filter(|requirement| requirement.followed)
        .count();
    let total = requirements.len();
    if total == 0 {
        return 0;
    }
    u8::try_from((followed * 100 + total / 2) / total).unwrap_or(100)
}

fn materialized_markdown_plan(
    request: &MarkdownAttemptRequest<'_>,
    rendered: &RenderedMarkdownScenario,
) -> serde_json::Value {
    json!({
        "schema": "harness-e2e-materialized-markdown-plan/v2",
        "scenario": request.scenario,
        "rendered": rendered,
        "seed": request.seed,
        "subject": {
            "model": request.subject.model,
            "provider": request.subject.provider,
            "functions": markdown_subject_policy(),
            "execution": crate::markdown::execution_policy(),
        },
        "auxiliary": {
            "model": request.auxiliary.model,
            "provider": request.auxiliary.provider,
            "setup_functions": markdown_setup_policy(),
            "validator_functions": markdown_validator_policy(),
            "cleanup_functions": markdown_cleanup_capability_policy(),
            "cleanup_function_selection": "receipt-scoped/v1",
            "setup_execution": {
                "max_turns": 16,
                "max_output_tokens": 4096,
                "max_total_tokens": 80000,
                "max_validation_retries": 1,
                "stuck_timeout_seconds": 180,
            },
            "validator_execution": {
                "sessions_per_criterion": 1,
                "max_turns": 8,
                "max_output_tokens": 2048,
                "max_total_tokens": 80000,
                "max_validation_retries": 1,
                "stuck_timeout_seconds": 120,
            },
            "adherence_execution": {
                "max_output_tokens": 4096,
                "evidence": "bounded-function-outcomes-and-validation-results/v1",
                "scoring": "equal-weight-binary-requirements/v1",
            },
            "cleanup_execution": {
                "max_turns": 16,
                "max_output_tokens": 4096,
                "max_total_tokens": 160000,
                "max_validation_retries": 1,
                "stuck_timeout_seconds": 180,
            },
        },
        "audit": request.audit_analyzer.map(|analyzer| json!({
            "model": analyzer.model,
            "provider": analyzer.provider,
        })),
        "runner": {
            "identity": crate::report::RunnerIdentity::runtime(),
            "system_identity_sha256": request.system_identity_sha256,
        },
        "campaign": {
            "runs": request.runs,
            "technical_retries": request.technical_retries,
        },
    })
}

fn persist_markdown_inputs(
    output: &std::path::Path,
    scenario: &CompiledMarkdownScenario,
    source: &str,
    run_id: &str,
    attempt_id: &str,
    materialized_plan: &serde_json::Value,
    report: &mut E2eRunReport,
) -> Result<()> {
    let root = std::path::PathBuf::from("evidence")
        .join(run_id)
        .join(attempt_id);
    let source = source.as_bytes();
    let source_reference = artifact::write_bytes(
        output,
        &root.join("scenario.md"),
        format!("{attempt_id}-markdown-source"),
        "markdown-scenario-source",
        "text/markdown; charset=utf-8",
        source,
    )?;
    if artifact::sha256_bytes(source) != scenario.source_sha256 {
        bail!("Markdown source differs from its compiled hash");
    }
    let compiled_reference = artifact::write_json(
        output,
        &root.join("compiled-scenario.json"),
        format!("{attempt_id}-compiled-scenario"),
        "compiled-markdown-scenario",
        scenario,
    )?;
    let plan_reference = artifact::write_json(
        output,
        &root.join("materialized-plan.json"),
        format!("{attempt_id}-materialized-plan"),
        "materialized-markdown-plan",
        materialized_plan,
    )?;
    report
        .evidence
        .extend([source_reference, compiled_reference, plan_reference]);
    Ok(())
}

fn persist_markdown_session(
    output: &std::path::Path,
    run_id: &str,
    attempt_id: &str,
    phase: &str,
    observation: &MarkdownSessionObservation,
    report: &mut E2eRunReport,
) -> Result<()> {
    let root = std::path::PathBuf::from("evidence")
        .join(run_id)
        .join(attempt_id);
    let mut value = json!({
        "session_id": observation.session_id,
        "metrics": observation.metrics,
        "transcript": observation.transcript,
    });
    redact_markdown_artifact(report, &mut value)?;
    let reference = artifact::write_json(
        output,
        &root.join(format!("{phase}.json")),
        format!("{attempt_id}-{phase}"),
        "markdown-phase-evidence",
        &value,
    )?;
    report.evidence.push(reference);
    Ok(())
}

fn redact_markdown_artifact<T>(report: &mut E2eRunReport, value: &mut T) -> Result<()>
where
    T: Serialize + DeserializeOwned,
{
    let policy = crate::redaction::RedactionPolicy::from_environment();
    let mut encoded = serde_json::to_value(&*value).context("serialize Markdown evidence")?;
    report
        .asset_redaction
        .merge(policy.redact_value(&mut encoded));
    policy.assert_clean(
        &serde_json::to_vec(&encoded).context("encode redacted Markdown evidence")?,
    )?;
    *value = serde_json::from_value(encoded).context("decode redacted Markdown evidence")?;
    Ok(())
}

fn markdown_phases(scenario: &RenderedMarkdownScenario) -> Vec<MarkdownPhaseReport> {
    let pending = |phase: &str, input_sha256: String| MarkdownPhaseReport {
        phase: phase.into(),
        status: MarkdownPhaseStatus::Pending,
        session_id: None,
        input_sha256,
        transcript_sha256: None,
        reason: String::new(),
    };
    vec![
        pending(
            "setup",
            artifact::sha256_bytes(scenario.before_test.as_bytes()),
        ),
        pending(
            "subject",
            artifact::sha256_bytes(scenario.prompt.as_bytes()),
        ),
        pending(
            "validations",
            artifact::sha256_value(&scenario.validations)
                .unwrap_or_else(|_| artifact::sha256_bytes(b"unavailable")),
        ),
        pending(
            "adherence",
            artifact::sha256_bytes(scenario.prompt.as_bytes()),
        ),
        pending("cleanup", artifact::sha256_bytes(b"receipt-driven cleanup")),
    ]
}

fn safe_markdown_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= 128
        && run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn complete_markdown_phase(
    phases: &mut [MarkdownPhaseReport],
    phase: &str,
    observation: &MarkdownSessionObservation,
    reason: &str,
) {
    let phase = phases
        .iter_mut()
        .find(|candidate| candidate.phase == phase)
        .expect("Markdown phase exists");
    phase.status = MarkdownPhaseStatus::Completed;
    phase.session_id = Some(observation.session_id.clone());
    phase.transcript_sha256 = artifact::sha256_value(&observation.transcript).ok();
    phase.reason = reason.into();
}

fn fail_markdown_phase(
    phases: &mut [MarkdownPhaseReport],
    phase: &str,
    session_id: &str,
    reason: &str,
) {
    let phase = phases
        .iter_mut()
        .find(|candidate| candidate.phase == phase)
        .expect("Markdown phase exists");
    phase.status = MarkdownPhaseStatus::Failed;
    phase.session_id = Some(session_id.into());
    phase.reason = reason.into();
}

fn function_call_receipts(transcript: &serde_json::Value) -> Vec<serde_json::Value> {
    common::function_calls(transcript)
        .into_iter()
        .map(|call| {
            json!({
                "function_id": call.function_id,
                "arguments": call.arguments,
            })
        })
        .collect()
}

fn receipt_function_id(receipt: &serde_json::Value) -> Option<&str> {
    receipt
        .get("function_id")
        .and_then(serde_json::Value::as_str)
}

fn markdown_setup_gaps(
    instructions: &str,
    receipts: &[serde_json::Value],
    function_call_errors: u64,
) -> Vec<String> {
    let mut gaps = Vec::new();
    for function_id in [
        "worker::add",
        "database::execute",
        "database::executeBatch",
        "state::set",
    ] {
        if markdown_instructions_reference_function(instructions, function_id)
            && !receipts
                .iter()
                .any(|receipt| receipt_function_id(receipt) == Some(function_id))
        {
            gaps.push(format!("{function_id} was not called"));
        }
    }
    if function_call_errors > 0 {
        gaps.push(format!(
            "setup reported {function_call_errors} function-call error(s)"
        ));
    }
    let mut ever_created = BTreeSet::new();
    let mut live_created = BTreeSet::new();
    for statement in receipts.iter().flat_map(receipt_sql_statements) {
        if let Some(object) = sql_database_object(statement, "CREATE") {
            ever_created.insert(object.clone());
            live_created.insert(object);
        }
        if let Some(object) = sql_database_object(statement, "DROP") {
            live_created.remove(&object);
        }
    }
    for (kind, name) in ever_created.difference(&live_created) {
        gaps.push(format!(
            "setup removed created {kind} '{name}' before subject execution"
        ));
    }
    gaps
}

fn markdown_instructions_reference_function(instructions: &str, function_id: &str) -> bool {
    instructions.match_indices(function_id).any(|(offset, _)| {
        let bytes = instructions.as_bytes();
        let is_function_character =
            |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'-');
        let starts_at_boundary = offset == 0 || !is_function_character(bytes[offset - 1]);
        let end = offset + function_id.len();
        let ends_at_boundary = end == bytes.len() || !is_function_character(bytes[end]);
        starts_at_boundary && ends_at_boundary
    })
}

fn markdown_cleanup_gaps(
    setup: &[serde_json::Value],
    subject: &[serde_json::Value],
    cleanup: &[serde_json::Value],
) -> Vec<String> {
    let mutations = setup.iter().chain(subject);
    let mut gaps = BTreeSet::new();

    let added_workers = mutations
        .clone()
        .filter(|receipt| receipt_function_id(receipt) == Some("worker::add"))
        .map(|receipt| receipt_target(receipt, &["worker", "worker_id", "name", "slug"]))
        .collect::<Vec<_>>();
    for target in added_workers {
        if !cleanup.iter().any(|receipt| {
            receipt_function_id(receipt) == Some("worker::remove")
                && target.as_deref().is_none_or(|target| {
                    receipt_target(receipt, &["worker", "worker_id", "name", "slug"]).as_deref()
                        == Some(target)
                })
        }) {
            gaps.insert(match target {
                Some(target) => format!("worker '{target}' was not removed"),
                None => "an added worker was not removed".into(),
            });
        }
    }

    let state_keys = mutations
        .clone()
        .filter(|receipt| receipt_function_id(receipt) == Some("state::set"))
        .map(|receipt| receipt_target(receipt, &["key"]))
        .collect::<Vec<_>>();
    for key in state_keys {
        if !cleanup.iter().any(|receipt| {
            receipt_function_id(receipt) == Some("state::delete")
                && key
                    .as_deref()
                    .is_none_or(|key| receipt_target(receipt, &["key"]).as_deref() == Some(key))
        }) {
            gaps.insert(match key {
                Some(key) => format!("state key '{key}' was not deleted"),
                None => "run-scoped state was not deleted".into(),
            });
        }
    }

    let database_mutation = mutations.clone().any(|receipt| {
        matches!(
            receipt_function_id(receipt),
            Some("database::execute" | "database::executeBatch" | "database::transaction")
        )
    });
    let database_cleanup = cleanup.iter().any(|receipt| {
        matches!(
            receipt_function_id(receipt),
            Some("database::execute" | "database::executeBatch" | "database::transaction")
        )
    });
    if database_mutation && !database_cleanup {
        gaps.insert("database mutations had no reversing database operation".into());
    }

    let created_objects = mutations
        .flat_map(receipt_sql_statements)
        .filter_map(|sql| sql_database_object(sql, "CREATE"))
        .collect::<BTreeSet<_>>();
    let dropped_objects = cleanup
        .iter()
        .flat_map(receipt_sql_statements)
        .filter_map(|sql| sql_database_object(sql, "DROP"))
        .collect::<BTreeSet<_>>();
    for (kind, name) in created_objects.difference(&dropped_objects) {
        gaps.insert(format!("created {kind} '{name}' was not dropped"));
    }

    gaps.into_iter().collect()
}

fn markdown_cleanup_actions(
    setup: &[serde_json::Value],
    subject: &[serde_json::Value],
) -> Vec<String> {
    let mutations = setup.iter().chain(subject).collect::<Vec<_>>();
    let mut actions = BTreeSet::new();

    for receipt in &mutations {
        if receipt_function_id(receipt) == Some("worker::add") {
            if let Some(worker) = receipt_target(receipt, &["worker", "worker_id", "name", "slug"])
            {
                actions.insert(format!(
                    "- Remove exactly worker `{worker}` with `worker::remove`, then verify that worker only with `worker::status`."
                ));
            }
        }
        if receipt_function_id(receipt) == Some("state::set") {
            let scope = receipt_target(receipt, &["scope"]);
            let key = receipt_target(receipt, &["key"]);
            if let (Some(scope), Some(key)) = (scope, key) {
                actions.insert(format!(
                    "- Delete exactly scope `{scope}`, key `{key}` with one `state::delete`, then verify that exact target is absent with one `state::get`."
                ));
            }
        }
        for (kind, name) in receipt_sql_statements(receipt)
            .into_iter()
            .filter_map(|sql| sql_database_object(sql, "CREATE"))
        {
            actions.insert(format!(
                "- Drop exactly {kind} `{name}` with `database::execute`, then verify that exact {kind} is absent with `database::query`."
            ));
        }
    }

    if actions.is_empty() {
        actions.insert("- No reversible mutation target was derived; make no mutations.".into());
    }
    actions.into_iter().collect()
}

fn receipt_target(receipt: &serde_json::Value, fields: &[&str]) -> Option<String> {
    let arguments = receipt.get("arguments")?.as_object()?;
    fields.iter().find_map(|field| {
        arguments
            .get(*field)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    })
}

fn receipt_sql_statements(receipt: &serde_json::Value) -> Vec<&str> {
    match receipt_function_id(receipt) {
        Some("database::execute") => receipt
            .pointer("/arguments/sql")
            .and_then(serde_json::Value::as_str)
            .into_iter()
            .collect(),
        Some("database::executeBatch" | "database::transaction") => receipt
            .pointer("/arguments/statements")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|statement| {
                statement
                    .as_str()
                    .or_else(|| statement.get("sql").and_then(serde_json::Value::as_str))
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn sql_database_object(sql: &str, operation: &str) -> Option<(String, String)> {
    let tokens = sql
        .split_whitespace()
        .map(|token| token.trim_matches(['`', '"', '\'', ';', '(', ')', ',']))
        .collect::<Vec<_>>();
    let operation_index = tokens
        .iter()
        .position(|token| token.eq_ignore_ascii_case(operation))?;
    let object_offset = tokens[operation_index + 1..].iter().position(|token| {
        token.eq_ignore_ascii_case("TABLE") || token.eq_ignore_ascii_case("VIEW")
    })?;
    let kind_index = operation_index + object_offset + 1;
    let kind = tokens.get(kind_index)?.to_ascii_lowercase();
    let mut index = kind_index + 1;
    if tokens.get(index..index + 3).is_some_and(|tokens| {
        tokens[0].eq_ignore_ascii_case("IF")
            && tokens[1].eq_ignore_ascii_case("NOT")
            && tokens[2].eq_ignore_ascii_case("EXISTS")
    }) {
        index += 3;
    } else if tokens.get(index..index + 2).is_some_and(|tokens| {
        tokens[0].eq_ignore_ascii_case("IF") && tokens[1].eq_ignore_ascii_case("EXISTS")
    }) {
        index += 2;
    }
    let name = tokens
        .get(index)?
        .trim_matches(['`', '"', '\'', ';', '(', ')', ',']);
    (!name.is_empty()).then(|| (kind, name.to_ascii_lowercase()))
}

fn parse_json_object<T>(text: &str) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let start = text.find('{').context("response contains no JSON object")?;
    let end = text
        .rfind('}')
        .context("response contains no complete JSON object")?;
    serde_json::from_str(&text[start..=end]).context("decode response JSON object")
}

fn validate_validator_decision(decision: ValidatorDecision) -> Result<ValidatorDecision> {
    if !matches!(
        decision.verdict.as_str(),
        "passed" | "failed" | "inconclusive"
    ) {
        bail!("validator verdict must be passed, failed, or inconclusive");
    }
    if decision.reason.trim().is_empty() {
        bail!("validator reason cannot be empty");
    }
    Ok(decision)
}

fn validation_reason(decision: &ValidatorDecision) -> String {
    if decision.evidence.is_empty() {
        return decision.reason.clone();
    }
    format!(
        "{} Evidence: {}",
        decision.reason,
        decision.evidence.join("; ")
    )
}

fn validate_adherence_decision(decision: AdherenceDecision) -> Result<AdherenceDecision> {
    if decision.summary.trim().is_empty() {
        bail!("adherence summary is invalid");
    }
    if decision.requirements.is_empty() {
        bail!("adherence response must identify at least one requirement");
    }
    let mut ids = BTreeSet::new();
    for requirement in &decision.requirements {
        if requirement.id.trim().is_empty()
            || requirement.instruction.trim().is_empty()
            || requirement.reason.trim().is_empty()
            || !(0.0..=1.0).contains(&requirement.confidence)
            || !ids.insert(requirement.id.as_str())
        {
            bail!("adherence requirement is invalid");
        }
    }
    Ok(decision)
}

fn markdown_setup_policy() -> FunctionPolicy {
    FunctionPolicy {
        allow: [
            "engine::functions::list",
            "engine::functions::info",
            "worker::validate",
            "worker::add",
            "worker::status",
            "database::execute",
            "database::executeBatch",
            "database::query",
            "state::get",
            "state::set",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        deny: [
            "e2e::*",
            "harness::teardown",
            "worker::remove",
            "worker::clear",
            "state::clear",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        ..FunctionPolicy::default()
    }
}

fn markdown_subject_policy() -> FunctionPolicy {
    FunctionPolicy {
        allow: [
            "engine::functions::list",
            "engine::functions::info",
            "worker::status",
            "database::query",
            "database::execute",
            "database::executeBatch",
            "database::transaction",
            "state::get",
            "state::set",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        deny: [
            "e2e::*",
            "harness::teardown",
            "worker::add",
            "worker::remove",
            "worker::clear",
            "state::delete",
            "state::clear",
            "shell::*",
            "coder::*",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        ..FunctionPolicy::default()
    }
}

fn markdown_validator_policy() -> FunctionPolicy {
    FunctionPolicy {
        allow: [
            "engine::functions::list",
            "engine::functions::info",
            "database::query",
            "state::get",
            "worker::status",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        deny: [
            "e2e::*",
            "database::execute",
            "database::executeBatch",
            "database::transaction",
            "state::set",
            "state::delete",
            "state::clear",
            "worker::add",
            "worker::remove",
            "worker::clear",
            "shell::*",
            "coder::*",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        ..FunctionPolicy::default()
    }
}

fn markdown_cleanup_policy(
    setup: &[serde_json::Value],
    subject: &[serde_json::Value],
) -> FunctionPolicy {
    let mutations = setup.iter().chain(subject).collect::<Vec<_>>();
    let worker = mutations
        .iter()
        .any(|receipt| receipt_function_id(receipt) == Some("worker::add"));
    let state = mutations
        .iter()
        .any(|receipt| receipt_function_id(receipt) == Some("state::set"));
    let database = mutations.iter().any(|receipt| {
        matches!(
            receipt_function_id(receipt),
            Some("database::execute" | "database::executeBatch" | "database::transaction")
        )
    });
    markdown_cleanup_policy_for_surfaces(worker, database, state)
}

fn markdown_cleanup_capability_policy() -> FunctionPolicy {
    markdown_cleanup_policy_for_surfaces(true, true, true)
}

fn markdown_cleanup_policy_for_surfaces(
    worker: bool,
    database: bool,
    state: bool,
) -> FunctionPolicy {
    let mut allow = Vec::new();
    if worker {
        allow.extend(["worker::status", "worker::remove"]);
    }
    if database {
        allow.extend([
            "database::query",
            "database::execute",
            "database::executeBatch",
        ]);
    }
    if state {
        allow.extend(["state::get", "state::delete"]);
    }
    FunctionPolicy {
        allow: allow.into_iter().map(str::to_string).collect(),
        deny: ["e2e::*", "worker::add", "state::set"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        ..FunctionPolicy::default()
    }
}

struct RetryRequest<'a> {
    scenario_id: ScenarioId,
    subject: &'a SubjectConfig,
    judge_config: Option<&'a JudgeConfig>,
    audit_analyzer: Option<&'a JudgeConfig>,
    seed: u64,
    technical_retries: u8,
    progress_interval: Option<Duration>,
    control: Option<&'a SuiteControl>,
    output: &'a std::path::Path,
    system_identity_sha256: &'a str,
    adaptive_resume: Option<&'a AdaptiveResumeAttempt>,
}

async fn run_with_technical_retries(
    context: &Arc<E2eContext>,
    request: RetryRequest<'_>,
) -> E2eRunReport {
    let RetryRequest {
        scenario_id,
        subject,
        judge_config,
        audit_analyzer,
        seed,
        technical_retries,
        progress_interval,
        control,
        output,
        system_identity_sha256,
        adaptive_resume,
    } = request;
    let run_id = adaptive_resume
        .map(|resume| resume.run_id.clone())
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
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
                audit_analyzer,
                seed,
                progress_interval,
                control,
                output,
                system_identity_sha256,
                existing_attempt_id: adaptive_resume.map(|resume| resume.attempt_id.as_str()),
                resume_existing: adaptive_resume.is_some_and(|resume| resume.resume_existing),
                restore_planner: adaptive_resume.is_some_and(|resume| resume.restore_planner),
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
        setup(context, run_id)
            .await
            .map_err(|error| scenario_setup_failure(error.to_string()))?;
    }
    let required_functions = crate::scenarios::required_functions(spec.id, run_id);
    report.worker_contracts = context
        .observe_function_contracts(&required_functions)
        .await
        .map_err(|error| {
            scenario_setup_failure(format!("required function preflight: {error:#}"))
        })?;
    emit_phase(control, SuitePhase::Executing)
        .await
        .map_err(|error| infrastructure_failure(FailurePhase::Execute, error.to_string()))?;
    ensure_not_cancelled(control)
        .map_err(|error| infrastructure_failure(FailurePhase::Execute, error.to_string()))?;
    context
        .bind_turn_completed()
        .await
        .map_err(|error| infrastructure_failure(FailurePhase::Execute, error.to_string()))?;
    let mut messages = vec![spec.prompt.clone()];
    messages.extend(crate::scenarios::dialogue_followups(spec.id, run_id));
    let scripted_dialogue = messages.len() > 1;
    let mut metrics = None;
    for (exchange, message) in messages.into_iter().enumerate() {
        context.drain_turn_completed_events();
        let response: SendResponse = context
            .trigger(
                "harness::send",
                SendRequest {
                    session_id: Some(session_id.to_string()),
                    message: MessageInput::Text(message),
                    model: Some(subject.model.clone()),
                    provider: Some(subject.provider.clone()),
                    idempotency_key: Some(format!(
                        "e2e:{run_id}:{}:send:{exchange}",
                        spec.id
                    )),
                    session: (exchange == 0).then(|| SessionInit {
                        title: Some(format!("Harness E2E: {}", spec.id)),
                        metadata: Some(json!({
                            "e2e_run_id": run_id,
                            "e2e_scenario": spec.id,
                            "e2e_execution_kind": if scripted_dialogue { "scripted_dialogue" } else { "harness_turn" },
                        })),
                    }),
                    options: Some(SendOptions {
                        max_turns: Some(spec.execution.max_turns),
                        max_output_tokens: spec.execution.max_output_tokens,
                        max_total_tokens: spec.execution.max_total_tokens,
                        max_validation_retries: spec.execution.max_validation_retries,
                        functions: Some(e2e_function_policy(spec, run_id)),
                        metadata: filesystem_metadata.clone(),
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
                format!(
                    "harness::send exchange {exchange} returned an unexpected response: {response:?}"
                ),
            ));
        }
        metrics = Some(
            match context
                .wait_for_turn(
                    spec.id,
                    session_id,
                    &response.turn_id,
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
            },
        );
    }
    let metrics = metrics.expect("every scenario has at least one scripted message");
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
        report.scenario_measurements = captured_measurements(&captured).map_err(|error| {
            RunFailure::new(
                RunStatus::InfrastructureError,
                FailurePhase::Collect,
                format!(
                    "scenario '{}' emitted invalid longitudinal measurements: {error:#}",
                    spec.id
                ),
            )
        })?;
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
    for (index, evidence) in std::mem::take(&mut objective.advisory_evidence)
        .into_iter()
        .enumerate()
    {
        let relative = PathBuf::from("evidence")
            .join(&report.run_id)
            .join(&report.attempt_id)
            .join(format!("advisory-{index}.json"));
        match artifact::write_json(
            output,
            &relative,
            evidence.id,
            evidence.kind,
            &evidence.value,
        ) {
            Ok(reference) => report.evidence.push(reference),
            Err(error) => tracing::warn!(
                scenario = spec.id,
                run_id,
                attempt_id = report.attempt_id,
                %error,
                "failed to persist non-authoritative evaluator evidence"
            ),
        }
    }
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

fn captured_measurements(
    deliverables: &[crate::scenarios::CapturedDeliverable],
) -> Result<Vec<ScenarioMeasurement>> {
    let mut measurements = Vec::new();
    let mut ids = HashSet::new();
    for measurement in deliverables
        .iter()
        .filter_map(|deliverable| deliverable.content.as_json())
        .filter_map(|content| {
            content
                .get("measurements")
                .and_then(serde_json::Value::as_array)
        })
        .flatten()
    {
        let id = measurement
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .context("measurement id must be a non-empty string")?;
        let value = measurement
            .get("value")
            .and_then(serde_json::Value::as_f64)
            .filter(|value| value.is_finite())
            .context("measurement value must be a finite number")?;
        let unit = measurement
            .get("unit")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|unit| !unit.is_empty())
            .context("measurement unit must be a non-empty string")?;
        if !ids.insert(id.to_string()) {
            bail!("measurement id '{id}' is duplicated");
        }
        measurements.push(ScenarioMeasurement {
            id: id.to_string(),
            value,
            unit: unit.to_string(),
            origin: ObservationMetricOrigin::Observed,
        });
    }
    measurements.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(measurements)
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

fn scenario_setup_failure(message: String) -> RunFailure {
    infrastructure_failure(
        FailurePhase::Setup,
        format!("scenario setup failed: {message}"),
    )
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

    fn measurement_deliverable(
        measurements: serde_json::Value,
    ) -> crate::scenarios::CapturedDeliverable {
        crate::scenarios::CapturedDeliverable {
            id: "performance_evidence".into(),
            kind: "benchmark".into(),
            content: CapturedDeliverableContent::Json(serde_json::json!({
                "measurements": measurements,
            })),
            invariants: Vec::new(),
            provenance: Vec::new(),
        }
    }

    #[test]
    fn captured_scenario_measurements_are_strict_and_stably_ordered() {
        let captured = captured_measurements(&[measurement_deliverable(serde_json::json!([
            {"id": "z_work", "value": 2, "unit": "operations"},
            {"id": "a_ratio", "value": 0.5, "unit": "ratio"}
        ]))])
        .unwrap();
        assert_eq!(
            captured
                .iter()
                .map(|metric| metric.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a_ratio", "z_work"]
        );
        assert!(captured.iter().all(|metric| {
            metric.origin == ObservationMetricOrigin::Observed && metric.value.is_finite()
        }));

        let duplicate = captured_measurements(&[measurement_deliverable(serde_json::json!([
            {"id": "work", "value": 2, "unit": "operations"},
            {"id": "work", "value": 1, "unit": "operations"}
        ]))])
        .unwrap_err();
        assert_eq!(duplicate.to_string(), "measurement id 'work' is duplicated");
    }

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
                max_total_tokens: Some(1),
                stuck_timeout_seconds: 1,
                max_validation_retries: None,
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
        let policy = e2e_function_policy(&spec(), "test-run");
        assert_eq!(policy.allow, ["*"]);
        assert_eq!(policy.deny, ["e2e::*"]);
        assert_eq!(policy.expose, Default::default());
    }

    #[test]
    fn e2e_policy_applies_scenario_denies() {
        let mut scenario = spec();
        scenario.denied_functions = &["state::*"];
        let policy = e2e_function_policy(&scenario, "test-run");

        assert_eq!(policy.allow, ["*"]);
        assert_eq!(policy.deny, ["e2e::*", "state::*"]);
    }

    #[test]
    fn fixed_and_rotating_seeds_materialize_distinct_deduplicated_cases() {
        assert_eq!(
            case_seeds(ScenarioId::MechanicalReaction, Some(7), &[7, 8, 9, 8]),
            vec![7, 8, 9]
        );
        assert_eq!(
            case_seeds(ScenarioId::MechanicalReaction, None, &[]),
            vec![ScenarioId::MechanicalReaction.canonical_seed()]
        );
    }

    #[test]
    fn consolidated_scenarios_ignore_fixed_and_rotating_seed_matrices() {
        assert_eq!(
            case_seeds(ScenarioId::ChessPlayLadder, Some(7), &[8, 9]),
            vec![ScenarioId::ChessPlayLadder.canonical_seed()]
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
                advisory_evidence: Vec::new(),
            }
        )
        .is_ok());
        assert!(validate_objective_evaluation(
            &spec,
            &ObjectiveEvaluation {
                hard_gates: Vec::new(),
                awards: Vec::new(),
                advisory_evidence: Vec::new(),
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
                advisory_evidence: Vec::new(),
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
                advisory_evidence: Vec::new(),
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
                advisory_evidence: Vec::new(),
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
                advisory_evidence: Vec::new(),
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
    fn runner_owned_scenario_setup_failures_are_infrastructure_errors() {
        let failure = scenario_setup_failure("fixture digest mismatch".into());
        assert_eq!(failure.status, RunStatus::InfrastructureError);
        assert_eq!(failure.phase, FailurePhase::Setup);
        assert_eq!(
            failure.message,
            "scenario setup failed: fixture digest mismatch"
        );
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
                max_total_tokens: Some(100),
                stuck_timeout_seconds: 1,
                max_validation_retries: None,
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

        let timed_out = timed_out_final_assessment(
            &input,
            &JudgeConfig {
                model: "judge-model".into(),
                provider: "judge-provider".into(),
            },
            FINAL_ASSESSMENT_BATCH_TIMEOUT,
        )
        .unwrap();
        assert_eq!(timed_out.availability, AiAssessmentAvailability::Failed);
        assert_eq!(
            timed_out.analyzer.as_ref().unwrap().input_sha256,
            input.sha256().unwrap()
        );
        assert_eq!(
            timed_out.analyzer.as_ref().unwrap().model.as_deref(),
            Some("judge-model")
        );
        assert!(timed_out
            .reason
            .as_deref()
            .unwrap()
            .contains("objective system status is unchanged"));
    }

    #[test]
    fn markdown_policies_isolate_subject_and_read_only_validators() {
        let subject = markdown_subject_policy();
        assert!(!subject.allow.contains(&"*".to_string()));
        assert!(subject.allow.contains(&"database::execute".to_string()));
        assert!(subject.allow.contains(&"state::set".to_string()));
        assert!(subject.deny.contains(&"e2e::*".to_string()));
        for denied in ["worker::add", "worker::remove", "shell::*", "coder::*"] {
            assert!(!subject.allow.contains(&denied.to_string()));
            assert!(subject.deny.contains(&denied.to_string()));
        }

        let validator = markdown_validator_policy();
        for mutable in [
            "database::execute",
            "database::transaction",
            "state::set",
            "state::delete",
            "worker::add",
            "worker::remove",
            "shell::*",
            "coder::*",
        ] {
            assert!(validator.deny.contains(&mutable.to_string()));
            assert!(!validator.allow.contains(&mutable.to_string()));
        }
        assert!(validator.allow.contains(&"database::query".to_string()));
        assert!(validator.allow.contains(&"state::get".to_string()));

        let state_receipts = vec![json!({
            "function_id": "state::set",
            "arguments": {"scope": "owned-scope", "key": "owned-key", "value": 1}
        })];
        let cleanup = markdown_cleanup_policy(&state_receipts, &[]);
        assert_eq!(
            cleanup.allow,
            vec!["state::get".to_string(), "state::delete".to_string()]
        );
        for unrelated in [
            "engine::functions::info",
            "worker::remove",
            "database::query",
            "database::execute",
        ] {
            assert!(!cleanup.allow.contains(&unrelated.to_string()));
        }
        assert_eq!(
            markdown_cleanup_actions(&state_receipts, &[]),
            vec![
                "- Delete exactly scope `owned-scope`, key `owned-key` with one `state::delete`, then verify that exact target is absent with one `state::get`."
            ]
        );
    }

    #[test]
    fn markdown_dynamic_artifacts_use_the_shared_redaction_policy() {
        let mut report = E2eRunReport::new(
            "run".into(),
            "attempt".into(),
            1,
            "session".into(),
            "prompt".into(),
        );
        let mut value = json!({
            "password": "must-not-survive",
            "evidence": "safe",
        });

        redact_markdown_artifact(&mut report, &mut value).unwrap();

        assert_eq!(value["password"], "[REDACTED]");
        assert!(report.asset_redaction.changed());
    }

    #[test]
    fn markdown_cleanup_requires_matching_worker_database_and_state_reversals() {
        let setup = vec![
            json!({"function_id": "worker::add", "arguments": {"name": "database-fixture"}}),
            json!({"function_id": "database::executeBatch", "arguments": {
                "db": "primary",
                "statements": [
                    "CREATE TABLE IF NOT EXISTS markdown_case (id INTEGER)",
                    {"sql": "CREATE VIEW markdown_case_view AS SELECT id FROM markdown_case"}
                ]
            }}),
            json!({"function_id": "state::set", "arguments": {"key": "e2e/run/value"}}),
        ];
        let incomplete = vec![json!({"function_id": "database::execute", "arguments": {
            "db": "primary",
            "sql": "DELETE FROM markdown_case"
        }})];

        let gaps = markdown_cleanup_gaps(&setup, &[], &incomplete);
        assert!(gaps
            .iter()
            .any(|gap| gap == "worker 'database-fixture' was not removed"));
        assert!(gaps
            .iter()
            .any(|gap| gap == "created table 'markdown_case' was not dropped"));
        assert!(gaps
            .iter()
            .any(|gap| gap == "created view 'markdown_case_view' was not dropped"));
        assert!(gaps
            .iter()
            .any(|gap| gap == "state key 'e2e/run/value' was not deleted"));

        let complete = vec![
            json!({"function_id": "worker::remove", "arguments": {"name": "database-fixture"}}),
            json!({"function_id": "database::transaction", "arguments": {
                "db": "primary",
                "statements": [
                    {"sql": "DROP VIEW IF EXISTS markdown_case_view"},
                    {"sql": "DROP TABLE IF EXISTS markdown_case"}
                ]
            }}),
            json!({"function_id": "state::delete", "arguments": {"key": "e2e/run/value"}}),
        ];
        assert!(markdown_cleanup_gaps(&setup, &[], &complete).is_empty());
        let actions = markdown_cleanup_actions(&setup, &[]);
        assert!(actions
            .iter()
            .any(|action| action.contains("view `markdown_case_view`")));
        assert!(actions
            .iter()
            .any(|action| action.contains("table `markdown_case`")));
    }

    #[test]
    fn markdown_setup_requires_authored_mutations_to_be_called_without_errors() {
        let instructions = "Use `state::set` for the baseline and `worker::add` for the fixture.";
        let state_only = vec![json!({
            "function_id": "state::set",
            "arguments": {"scope": "run", "key": "value"}
        })];

        assert_eq!(
            markdown_setup_gaps(instructions, &state_only, 0),
            vec!["worker::add was not called"]
        );
        assert_eq!(
            markdown_setup_gaps(instructions, &state_only, 1),
            vec![
                "worker::add was not called",
                "setup reported 1 function-call error(s)",
            ]
        );

        let complete = vec![
            json!({"function_id": "state::set", "arguments": {}}),
            json!({"function_id": "worker::add", "arguments": {}}),
        ];
        assert!(markdown_setup_gaps(instructions, &complete, 0).is_empty());

        let batch_instructions = "Call `database::executeBatch` exactly once.";
        let batch = vec![json!({
            "function_id": "database::executeBatch",
            "arguments": {"db": "primary", "statements": []}
        })];
        assert!(markdown_setup_gaps(batch_instructions, &batch, 0).is_empty());

        let reverted = vec![json!({
            "function_id": "database::executeBatch",
            "arguments": {"db": "primary", "statements": [
                "DROP TABLE IF EXISTS owned_table",
                "CREATE TABLE owned_table (id INTEGER)",
                "DROP TABLE owned_table"
            ]}
        })];
        assert_eq!(
            markdown_setup_gaps(batch_instructions, &reverted, 0),
            vec!["setup removed created table 'owned_table' before subject execution"]
        );
    }

    #[test]
    fn markdown_adherence_evidence_distinguishes_failed_calls_from_side_effects() {
        let transcript = json!({"messages": [
            {"message": {"role": "assistant", "content": [{
                "type": "function_call",
                "id": "failed-insert",
                "function_id": "agent_trigger",
                "arguments": {
                    "function": "database::execute",
                    "payload": {
                        "db": "primary",
                        "sql": "INSERT INTO markdown_insert_record (text) VALUES (?)",
                        "params": ["harness-e2e-markdown"]
                    }
                }
            }]}},
            {"message": {
                "role": "function_result",
                "function_call_id": "failed-insert",
                "function_id": "database::execute",
                "is_error": true,
                "details": {"error": {
                    "code": "DRIVER_ERROR",
                    "message": "sensitive driver detail must not enter adherence evidence"
                }}
            }},
            {"message": {"role": "assistant", "content": [{
                "type": "function_call",
                "id": "successful-insert",
                "function_id": "agent_trigger",
                "arguments": {
                    "function": "database::execute",
                    "payload": {
                        "db": "primary",
                        "sql": "INSERT INTO markdown_insert_record (value) VALUES (?)",
                        "params": ["harness-e2e-markdown"]
                    }
                }
            }]}},
            {"message": {
                "role": "function_result",
                "function_call_id": "successful-insert",
                "function_id": "database::execute",
                "is_error": false,
                "details": {"affected_rows": 1, "returned_rows": []}
            }}
        ]});

        let outcomes = adherence_function_outcomes(&transcript);
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0]["status"], "failed");
        assert_eq!(outcomes[0]["error_code"], "DRIVER_ERROR");
        assert!(outcomes[0].get("result").is_none());
        assert_eq!(outcomes[1]["status"], "succeeded");
        assert_eq!(outcomes[1]["result"]["affected_rows"], 1);
        assert_eq!(outcomes[1]["result"]["returned_rows_count"], 0);
        assert!(!serde_json::to_string(&outcomes)
            .unwrap()
            .contains("sensitive driver detail"));
    }

    #[test]
    fn markdown_adherence_input_includes_completed_validation_evidence() {
        let scenario = crate::markdown::embedded_scenario("minimal_path").unwrap();
        let mut report = test_run_report();
        report.transcript = Some(json!({"messages": [{"message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "Inserted exactly one row."}]
        }}]}));
        report.criteria.push(CriterionReport {
            id: "01_record_created".into(),
            possible: 80,
            awarded: Some(80),
            reason: "Read-only query found exactly one matching row".into(),
        });

        let input = markdown_adherence_input(&scenario.prompt, &report);
        assert_eq!(input["validation_results"][0]["verdict"], "passed");
        assert_eq!(
            input["validation_results"][0]["reason"],
            "Read-only query found exactly one matching row"
        );
    }

    #[test]
    fn markdown_adherence_score_is_runner_owned_and_deterministic() {
        let requirement = |id: &str, followed| AdherenceRequirement {
            id: id.into(),
            instruction: format!("requirement {id}"),
            followed,
            reason: "trusted evidence".into(),
            confidence: 1.0,
            evidence: Vec::new(),
        };
        let mut requirements = vec![
            requirement("one", true),
            requirement("two", true),
            requirement("three", true),
            requirement("four", false),
        ];
        assert_eq!(deterministic_adherence_score(&requirements), 75);
        requirements[3].followed = true;
        assert_eq!(deterministic_adherence_score(&requirements), 100);
        assert_eq!(deterministic_adherence_score(&[]), 0);

        let duplicate = AdherenceDecision {
            summary: "duplicate requirement ids".into(),
            requirements: vec![requirement("same", true), requirement("same", true)],
        };
        assert!(validate_adherence_decision(duplicate).is_err());
    }

    #[test]
    fn identical_markdown_inputs_materialize_the_same_plan() {
        let definition = crate::markdown::embedded_definition("insert_record").unwrap();
        let scenario = definition.scenario;
        let subject = SubjectConfig {
            model: "subject-model".into(),
            provider: "subject-provider".into(),
        };
        let auxiliary = JudgeConfig {
            model: "auxiliary-model".into(),
            provider: "auxiliary-provider".into(),
        };
        let output = tempfile::tempdir().unwrap();
        let request = MarkdownAttemptRequest {
            scenario: &scenario,
            source: &definition.source,
            subject: &subject,
            auxiliary: &auxiliary,
            audit_analyzer: None,
            run_id: "run",
            attempt_number: 1,
            seed: 7,
            progress_interval: None,
            control: None,
            output: output.path(),
            system_identity_sha256: "sha256:system",
            technical_retries: 1,
            runs: 3,
            materialized_plan: None,
        };
        let rendered = crate::markdown::render(&scenario, "run-123", request.seed);
        let first = materialized_markdown_plan(&request, &rendered);
        let second = materialized_markdown_plan(&request, &rendered);
        assert_eq!(first, second);
        assert_eq!(
            artifact::sha256_value(&first).unwrap(),
            artifact::sha256_value(&second).unwrap()
        );
        assert_eq!(first["schema"], "harness-e2e-materialized-markdown-plan/v2");
        assert_eq!(first["rendered"]["run_id"], "run-123");
        assert_eq!(first["rendered"]["seed"], 7);
        assert!(!first["rendered"]["prompt"].as_str().unwrap().contains("{{"));
        assert!(!first.to_string().contains("attempt_id"));
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
