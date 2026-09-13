use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::json;
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use crate::artifact;
use crate::assessment::{
    AssessmentOutcome, AssessmentResult, AssessmentScore, AssessmentTarget, AssessmentTargetKind,
};
use crate::asset::{self, AssetCaptureLimits};
use crate::context::E2eContext;
use crate::identity::{self, ExecutionIdentity, SystemUnderTestIdentity};
use crate::report::{
    CostReport, CriterionReport, E2eManifest, E2eReport, E2eRunReport, E2eScenarioReport,
    FailurePhase, ModelArtifact, ObservationMetricOrigin, ObservationRunContract,
    RetryAttemptReport, RunStatus, ScenarioFlowEvidence, ScenarioMeasurement,
};
use crate::scenarios::common;
use crate::scenarios::{
    CriterionAward, MaterializedScenario, ObjectiveEvaluation, ScenarioCase,
    ScenarioDeliverableCapture, ScenarioExecutionKind, ScenarioId, ScenarioObservation,
    ScenarioSpec,
};
use crate::wire::{
    ControlPlaneEvidence, FunctionPolicy, MessageInput, Model, SendOptions, SendRequest,
    SendResponse, SessionInit, StatusReport, TurnStatus,
};
use crate::workflow::{
    adaptive_runtime, composite_definition, composite_descriptor_catalog, composite_runtime,
    execute_adaptive_workflow, execute_workflow, observe_worker_contracts, plan_adaptive_workflow,
    AdaptivePlannerInvalidation, AdaptivePlannerMetadata, AdaptivePlannerReferenceCheck,
    AgentPlannerRequest, ResumableWorkflowExecutionRequest, ResumableWorkflowOutcome,
    WorkflowCleanupContext, WorkflowCleanupStatus, WorkflowExecutionRequest, WorkflowFailurePhase,
    WorkflowResumeIdentity, WorkflowResumeStore,
};

const MAX_RUNS: u32 = 20;
const MAX_TECHNICAL_RETRIES: u8 = 3;

/// Per-scenario spend ceiling passed to the Harness; `None` leaves the
/// Harness default in place. Kanban runs are isolated evaluations capped at
/// five dollars; the Linkly tutorial is a 65-minute agentic build that cost
/// about a dollar on DeepSeek V4 Pro at 2026-08 prices, so ten covers a slow
/// run on a pricier provider without hiding a runaway loop.
fn subject_cost_cap_usd(scenario_id: &str) -> Option<f64> {
    if crate::scenarios::kanban::IDS.contains(&scenario_id) {
        Some(5.0)
    } else if scenario_id == crate::scenarios::linkly::ID {
        Some(10.0)
    } else {
        None
    }
}

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
    pub output: PathBuf,
    pub scenarios: Vec<ScenarioId>,
    pub runs: u32,
    pub seed: Option<u64>,
    pub rotating_seeds: Vec<u64>,
    pub technical_retries: u8,
    pub progress_interval: Option<Duration>,
    /// Soft admission budget: stop starting slots after this elapsed duration.
    /// The current slot still finishes capture and cleanup.
    pub slot_start_deadline_seconds: Option<u64>,
    pub control: Option<SuiteControl>,
    pub observation_contract: Option<ObservationRunContract>,
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
    /// Absent when persistence failed; the redacted report is still returned.
    pub report_path: Option<PathBuf>,
}

enum PreparedSuiteCase {
    BuiltIn {
        key: ScenarioId,
        seed: u64,
        definition: MaterializedScenario,
        preflight_error: Option<String>,
        runs: Vec<E2eRunReport>,
    },
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
    SlotInventoryCommitted {
        slots: Vec<crate::journal::JournalSlot>,
    },
    Phase(SuitePhase),
    AttemptStarted {
        scenario_id: ScenarioId,
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
    SubjectObservationCommitted {
        slot_id: String,
        attempt_id: String,
        artifact: crate::artifact::ArtifactReference,
    },
    RunCommitted {
        slot_id: String,
        run_id: String,
        artifact: crate::artifact::ArtifactReference,
    },
    SlotDeferred {
        slot_id: String,
        reason: String,
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
    let suite_started = Instant::now();
    let suite_deadline = config.slot_start_deadline_seconds.map(Duration::from_secs);
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
    let context = E2eContext::connect(&config.url)
        .await
        .context("connect E2E runner")?;
    context.initialize_execution_outputs(config.scenarios.iter().copied().map(ScenarioId::as_str));
    let context = Arc::new(context);
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
    let built_in_scenarios = config.scenarios.to_vec();
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
    let slots = planned_slots(&config);
    let mut persistence_errors = Vec::new();
    preserve_event(
        config.control.as_ref(),
        SuiteEvent::SlotInventoryCommitted {
            slots: slots.clone(),
        },
        &mut persistence_errors,
    )
    .await;
    let mut prepared_cases = Vec::new();
    let mut deferred_cases = Vec::new();

    for scenario_key in &config.scenarios {
        for seed in case_seeds_for_key(scenario_key, config.seed, &config.rotating_seeds) {
            let scenario_id = *scenario_key;
            let definition = match scenario_id.materialize("validation", seed) {
                Ok(definition) => definition,
                Err(error) => {
                    let reason =
                        format!("materialize scenario {}: {error:#}", scenario_id.as_str());
                    deferred_cases.push(
                        defer_planned_case(
                            config.control.as_ref(),
                            &slots,
                            scenario_key,
                            seed,
                            config.runs,
                            reason,
                            &mut persistence_errors,
                        )
                        .await,
                    );
                    continue;
                }
            };
            let preflight_error = preflight_case(&context, &control_plane, &definition.case)
                .await
                .err()
                .map(|error| format!("preflight case {}: {error:#}", definition.case.case_id));
            prepared_cases.push(PreparedSuiteCase::BuiltIn {
                key: *scenario_key,
                seed,
                definition,
                preflight_error,
                runs: Vec::with_capacity(config.runs as usize),
            });
        }
    }

    // Execute one slot per prepared case before starting the next repetition.
    // A late failure therefore cannot consume the whole suite budget while
    // leaving every later scenario without a single observation.
    let mut current_repetition = None;
    for (repetition, index) in round_robin_slots(config.runs, prepared_cases.len()) {
        if current_repetition != Some(repetition) {
            context.reset_execution_outputs();
            current_repetition = Some(repetition);
        }
        let prepared = &mut prepared_cases[index];
        if let Some(reason) = slot_deferral_reason(
            !persistence_errors.is_empty(),
            config
                .control
                .as_ref()
                .is_some_and(|control| *control.cancellation.borrow()),
            suite_started.elapsed(),
            suite_deadline,
        ) {
            let (key, seed) = match prepared {
                PreparedSuiteCase::BuiltIn { key, seed, .. } => (key, *seed),
            };
            preserve_event(
                config.control.as_ref(),
                SuiteEvent::SlotDeferred {
                    slot_id: slot_id(key, seed, repetition),
                    reason: reason.into(),
                },
                &mut persistence_errors,
            )
            .await;
            continue;
        }
        let (slot_key, seed, mut run, subject_observed) = match prepared {
            PreparedSuiteCase::BuiltIn {
                key,
                seed,
                definition,
                preflight_error,
                ..
            } => {
                let scenario_id = *key;
                tracing::info!(
                    scenario = scenario_id.as_str(),
                    case_id = definition.case.case_id,
                    seed = *seed,
                    run = repetition + 1,
                    total_runs = config.runs,
                    "running E2E quality scenario case"
                );
                let subject_observed = preflight_error.is_none();
                let run = if let Some(error) = preflight_error.as_ref() {
                    preflight_failure_run(&definition.spec, error.clone())
                } else {
                    run_with_technical_retries(
                        &context,
                        RetryRequest {
                            scenario_id,
                            subject: &config.subject,
                            seed: *seed,
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
                    .await
                };
                (*key, *seed, run, subject_observed)
            }
        };
        incorporate_worker_contracts(&mut worker_contracts, &mut run);
        let checkpoint_result = commit_run_checkpoint(
            config.control.as_ref(),
            &config.output,
            &slot_id(&slot_key, seed, repetition),
            &run,
            subject_observed,
        )
        .await;
        match prepared {
            PreparedSuiteCase::BuiltIn { runs, .. } => runs.push(run),
        }
        if let Err(error) = checkpoint_result {
            persistence_errors.push(format!("commit run checkpoint: {error:#}"));
        }
    }

    let mut scenario_reports = prepared_cases
        .into_iter()
        .map(|prepared| match prepared {
            PreparedSuiteCase::BuiltIn {
                definition, runs, ..
            } => E2eScenarioReport::aggregate_case_with_planned(
                definition.case,
                definition.spec.execution,
                config.runs,
                runs,
            ),
        })
        .chain(deferred_cases)
        .collect::<Vec<_>>();
    for scenario in &mut scenario_reports {
        if scenario.aggregate.deferred_runs > 0 && scenario.deferral_reason.is_none() {
            scenario.deferral_reason = slot_deferral_reason(
                !persistence_errors.is_empty(),
                config
                    .control
                    .as_ref()
                    .is_some_and(|control| *control.cancellation.borrow()),
                suite_started.elapsed(),
                suite_deadline,
            )
            .map(str::to_owned);
        }
    }

    crate::scenarios::engineering_ticket::apply_handoff_efficiency(&mut scenario_reports);

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
    let manifest = E2eManifest {
        execution: execution.clone(),
        system_under_test: system_under_test.clone(),
        subject: subject.clone(),
        control_plane,
        observation_contract: config.observation_contract.clone(),
        worker_contracts,
    };
    let mut report = E2eReport::new(
        execution,
        system_under_test,
        subject,
        identity::nonempty_env("HARNESS_E2E_ENGINE_REVISION"),
        scenario_reports,
    );
    preserve_event(
        config.control.as_ref(),
        SuiteEvent::Phase(SuitePhase::Finalizing),
        &mut persistence_errors,
    )
    .await;
    for error in persistence_errors {
        report.record_persistence_error(error);
    }
    report.slot_start_deadline_seconds = config.slot_start_deadline_seconds;
    report.observation_contract = config.observation_contract.clone();
    let report_path =
        persist_report_preserving_observations(&mut report, &manifest, &config.output)?;
    if report_path.is_none() {
        redact_unpersisted_report(&mut report)?;
    }
    tracing::info!(
        persisted = report_path.is_some(),
        "shutting down the E2E connection"
    );
    context.shutdown().await;
    Ok(SuiteRunOutcome {
        report,
        manifest,
        report_path,
    })
}

fn persist_report_preserving_observations(
    report: &mut E2eReport,
    manifest: &E2eManifest,
    output: &Path,
) -> Result<Option<PathBuf>> {
    match report.write_to(output, manifest) {
        Ok(path) => Ok(Some(path)),
        Err(error) => {
            report.record_persistence_error(format!("persist results: {error:#}"));
            Ok(None)
        }
    }
}

fn redact_unpersisted_report(report: &mut E2eReport) -> Result<()> {
    // Only sanitize the final handoff. Earlier serialization would discard
    // skipped capture fields that a subsequent persistence attempt still needs.
    let mut value = serde_json::to_value(&*report)?;
    let redaction = crate::redaction::RedactionPolicy::from_environment().redact_value(&mut value);
    *report = serde_json::from_value(value)?;
    report.redaction.merge(redaction);
    Ok(())
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

async fn emit_attempt_phase(
    control: Option<&SuiteControl>,
    phase: SuitePhase,
    failure_phase: FailurePhase,
    report: &mut E2eRunReport,
) -> bool {
    if let Err(error) = emit_phase(control, phase).await {
        record_checkpoint_failure(
            report,
            failure_phase,
            format!("persist {phase:?} phase checkpoint: {error:#}"),
        );
        false
    } else {
        true
    }
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

fn slot_id(scenario: &ScenarioId, seed: u64, repetition: u32) -> String {
    let digest = artifact::sha256_value(&json!({
        "scenario_id": scenario.as_str(),
        "seed": seed.to_string(),
        "repetition": repetition,
    }))
    .expect("slot identity is serializable");
    format!("slot-{}", &digest["sha256:".len().."sha256:".len() + 24])
}

fn planned_slots(config: &SuiteRunConfig) -> Vec<crate::journal::JournalSlot> {
    let mut slots = Vec::new();
    for scenario in &config.scenarios {
        for seed in case_seeds_for_key(scenario, config.seed, &config.rotating_seeds) {
            let case_id = scenario
                .materialize("inventory", seed)
                .map(|definition| definition.case.case_id)
                .unwrap_or_else(|_| format!("unresolved:{}:{seed}", scenario.as_str()));
            for repetition in 0..config.runs {
                slots.push(crate::journal::JournalSlot {
                    slot_id: slot_id(scenario, seed, repetition),
                    ordinal: slots.len().try_into().unwrap_or(u64::MAX),
                    scenario_id: scenario.as_str().to_string(),
                    case_id: case_id.clone(),
                    seed: seed.to_string(),
                    repetition,
                });
            }
        }
    }
    slots
}

#[allow(clippy::too_many_arguments)]
async fn defer_planned_case(
    control: Option<&SuiteControl>,
    slots: &[crate::journal::JournalSlot],
    scenario: &ScenarioId,
    seed: u64,
    runs: u32,
    reason: String,
    persistence_errors: &mut Vec<String>,
) -> E2eScenarioReport {
    for repetition in 0..runs {
        preserve_event(
            control,
            SuiteEvent::SlotDeferred {
                slot_id: slot_id(scenario, seed, repetition),
                reason: reason.clone(),
            },
            persistence_errors,
        )
        .await;
    }
    let case_id = slots
        .iter()
        .find(|slot| slot.slot_id == slot_id(scenario, seed, 0))
        .map(|slot| slot.case_id.clone())
        .unwrap_or_else(|| format!("unresolved:{}:{seed}", scenario.as_str()));
    let execution = scenario.spec("validation").execution;
    E2eScenarioReport::deferred(scenario.as_str().into(), case_id, execution, runs, reason)
}

fn round_robin_slots(runs: u32, cases: usize) -> impl Iterator<Item = (u32, usize)> {
    (0..runs).flat_map(move |repetition| (0..cases).map(move |case| (repetition, case)))
}

fn slot_deferral_reason(
    persistence_failed: bool,
    cancelled: bool,
    elapsed: Duration,
    deadline: Option<Duration>,
) -> Option<&'static str> {
    if persistence_failed {
        Some("execution persistence unavailable; remaining slots require reconciliation")
    } else if cancelled {
        Some("execution cancelled before this slot started")
    } else if deadline.is_some_and(|deadline| elapsed >= deadline) {
        Some("slot-start deadline exhausted before this slot started")
    } else {
        None
    }
}

/// Stop admitting work after durability fails, but never discard observed runs.
async fn preserve_event(
    control: Option<&SuiteControl>,
    event: SuiteEvent,
    errors: &mut Vec<String>,
) {
    if errors.is_empty() {
        if let Err(error) = emit_event(control, event).await {
            errors.push(format!("persist execution event: {error:#}"));
        }
    }
}

fn record_checkpoint_failure(report: &mut E2eRunReport, phase: FailurePhase, message: String) {
    report.push_typed_failure(
        RunStatus::InfrastructureError,
        phase,
        "journal_checkpoint_failed",
        crate::report::RetryScope::None,
        crate::report::ContaminationScope::None,
        message,
    );
}

pub fn resolve_slot_start_deadline(explicit: Option<u64>) -> Result<Option<u64>> {
    let configured = std::env::var("HARNESS_E2E_SUITE_DEADLINE_SECONDS").ok();
    let value = explicit
        .map(Ok)
        .or_else(|| configured.map(|value| value.parse::<u64>()));
    let value = value
        .transpose()
        .context("HARNESS_E2E_SUITE_DEADLINE_SECONDS must be a positive integer")?;
    if value == Some(0) {
        bail!("slot-start deadline must be greater than zero");
    }
    Ok(value)
}

fn write_immutable_json_artifact<T: Serialize>(
    output: &std::path::Path,
    relative_path: &std::path::Path,
    id: String,
    kind: &str,
    value: &T,
) -> Result<crate::artifact::ArtifactReference> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .with_context(|| format!("serialize durable {kind} checkpoint"))?;
    bytes.push(b'\n');
    artifact::write_bytes(output, relative_path, id, kind, "application/json", &bytes)
}

async fn commit_run_checkpoint(
    control: Option<&SuiteControl>,
    output: &std::path::Path,
    slot_id: &str,
    run: &E2eRunReport,
    subject_observed: bool,
) -> Result<()> {
    // Full, redacted run evidence is durable independently of the event sink.
    // This also applies to CLI runs, which have no control-plane journal.
    let mut checkpoint = json!({
        "schema": "harness-e2e-run-checkpoint",
        "slot_id": slot_id,
        "run_id": run.run_id,
        "attempt_id": run.attempt_id,
        "attempts": run.retry_attempts.len() + 1,
        "status": run.status,
        "completion": run.completion,
        "technical": run.technical,
        "score": run.score,
        "cost": run.cost,
        "metrics": run.metrics,
        "run": run,
    });
    // These fields are intentionally skipped by the public Results serializer,
    // but are required to retain the complete pre-finalization evidence.
    checkpoint["capture"] = json!({
        "deliverables": run.deliverables.iter().map(|item| json!({
            "id": item.id, "content": item.content,
        })).collect::<Vec<_>>(),
        "terminal_status": run.terminal_status,
        "assessment_results": run.assessment_results,
        "asset_assessments": run.asset_assessments,
        "asset_capture_manifest": run.asset_capture_manifest,
        "asset_redaction": run.asset_redaction,
        "retry_attempts": run.retry_attempts.iter().map(|attempt| json!({
            "attempt_id": attempt.attempt_id,
            "deliverables": attempt.deliverables.iter().map(|item| json!({
                "id": item.id, "content": item.content,
            })).collect::<Vec<_>>(),
            "assessment_results": attempt.assessment_results,
            "asset_assessments": attempt.asset_assessments,
            "asset_capture_manifest": attempt.asset_capture_manifest,
            "asset_redaction": attempt.asset_redaction,
        })).collect::<Vec<_>>(),
    });
    crate::redaction::RedactionPolicy::from_environment().redact_value(&mut checkpoint);
    let run_artifact = write_immutable_json_artifact(
        output,
        &PathBuf::from("journal")
            .join("runs")
            .join(slot_id)
            .join(format!("{}.json", run.run_id)),
        format!("{slot_id}-{}-run", run.run_id),
        "run_checkpoint",
        &checkpoint,
    )?;
    if run
        .failures
        .iter()
        .chain(
            run.retry_attempts
                .iter()
                .flat_map(|attempt| &attempt.failures),
        )
        .any(|failure| failure.code == "journal_checkpoint_failed")
    {
        bail!(
            "attempt lifecycle checkpoint failed; full run preserved at {}",
            run_artifact.path
        );
    }
    let Some(control) = control else {
        return Ok(());
    };
    let attempts = run
        .retry_attempts
        .iter()
        .map(|attempt| {
            (
                attempt.attempt_id.as_str(),
                attempt.attempt_number,
                attempt.status,
                attempt.completion,
                attempt.technical,
                attempt.score,
                &attempt.cost,
                attempt.metrics.as_ref(),
            )
        })
        .chain(std::iter::once((
            run.attempt_id.as_str(),
            run.attempt_number,
            run.status,
            run.completion,
            run.technical,
            run.score,
            &run.cost,
            run.metrics.as_ref(),
        )));
    for attempt in attempts.filter(|attempt| subject_observed && attempt.7.is_some()) {
        let (attempt_id, attempt_number, status, completion, technical, score, cost, metrics) =
            attempt;
        let checkpoint = json!({
            "schema": "harness-e2e-subject-observation-checkpoint",
            "slot_id": slot_id,
            "run_id": run.run_id,
            "attempt_id": attempt_id,
            "attempt_number": attempt_number,
            "status": status,
            "completion": completion,
            "technical": technical,
            "score": score,
            "cost": cost,
            "metrics": metrics,
        });
        let artifact = write_immutable_json_artifact(
            output,
            &PathBuf::from("journal")
                .join("observations")
                .join(slot_id)
                .join(format!("{attempt_id}.json")),
            format!("{slot_id}-{attempt_id}-subject-observation"),
            "subject_observation_checkpoint",
            &checkpoint,
        )?;
        emit_event(
            Some(control),
            SuiteEvent::SubjectObservationCommitted {
                slot_id: slot_id.to_string(),
                attempt_id: attempt_id.to_string(),
                artifact,
            },
        )
        .await?;
    }
    emit_event(
        Some(control),
        SuiteEvent::RunCommitted {
            slot_id: slot_id.to_string(),
            run_id: run.run_id.clone(),
            artifact: run_artifact,
        },
    )
    .await
}

fn preflight_failure_run(spec: &ScenarioSpec, message: String) -> E2eRunReport {
    let run_id = Uuid::new_v4().simple().to_string();
    let attempt_id = Uuid::new_v4().simple().to_string();
    let mut report = E2eRunReport::new(run_id, attempt_id, 1, String::new(), spec.prompt.clone());
    report.push_failure(RunStatus::InfrastructureError, FailurePhase::Setup, message);
    ensure_assessment_results(spec, &mut report);
    report.refresh_dimensions(false);
    report
}

fn incorporate_worker_contracts(
    observed: &mut Vec<crate::report::ObservedWorkerContract>,
    run: &mut E2eRunReport,
) {
    for contract in run.worker_contracts.clone() {
        if let Some(existing) = observed
            .iter()
            .find(|existing| existing.function_id == contract.function_id)
        {
            if existing != &contract {
                run.push_failure(
                    RunStatus::InfrastructureError,
                    FailurePhase::Collect,
                    format!(
                        "function contract '{}' changed between preflight and scenario execution",
                        contract.function_id
                    ),
                );
            }
        } else {
            observed.push(contract);
        }
    }
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
    validate_registry_handoff_order(&config.scenarios)?;
    if let Some(resume) = config
        .control
        .as_ref()
        .and_then(|control| control.adaptive_resume.as_ref())
    {
        if config.scenarios.as_slice() != [resume.scenario_id]
            || config.runs != 1
            || config.technical_retries != 0
            || !config.rotating_seeds.is_empty()
        {
            bail!("adaptive resume requires one isolated scenario, one run, and no replay");
        }
    }
    // Scenario materialization is slot-scoped. Keeping it out of request
    // validation lets one broken definition become an explicit deferred slot
    // instead of erasing the whole execution.
    Ok(())
}

fn validate_registry_handoff_order(scenarios: &[ScenarioId]) -> Result<()> {
    let implementation = scenarios
        .iter()
        .position(|scenario| *scenario == ScenarioId::RegistryImplementation);
    let verification = scenarios
        .iter()
        .position(|scenario| *scenario == ScenarioId::RegistryVerification);
    if matches!((implementation, verification), (Some(implementation), Some(verification)) if verification < implementation)
    {
        bail!("registry_implementation must precede registry_verification in the same execution");
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

fn case_seeds_for_key(scenario: &ScenarioId, fixed: Option<u64>, rotating: &[u64]) -> Vec<u64> {
    case_seeds(*scenario, fixed, rotating)
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
    context.begin_execution_output_attempt(scenario_id.as_str());
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
            scenario_id,
            run_id: run_id.to_string(),
            attempt_id: attempt_id.clone(),
            session_id: session_id.clone(),
            resume_state_path: None,
        },
    )
    .await
    {
        record_checkpoint_failure(
            &mut report,
            FailurePhase::Setup,
            format!("persist attempt checkpoint: {error:#}"),
        );
        ensure_assessment_results(&spec, &mut report);
        report.refresh_dimensions(expects_deliverables);
        return report;
    }
    emit_attempt_phase(
        control,
        SuitePhase::SettingUp,
        FailurePhase::Setup,
        &mut report,
    )
    .await;

    if report.failures.is_empty() {
        if let Err(error) = execute(
            context.as_ref(),
            ExecutionRequest {
                subject,
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
            report.push_typed_failure(
                error.status,
                error.phase,
                error.code,
                error.retry_scope,
                error.contamination,
                error.message,
            );
        }
    }

    emit_attempt_phase(
        control,
        SuitePhase::Persisting,
        FailurePhase::Collect,
        &mut report,
    )
    .await;
    emit_attempt_phase(
        control,
        SuitePhase::CleaningUp,
        FailurePhase::Cleanup,
        &mut report,
    )
    .await;
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
    if crate::scenarios::kanban::IDS.contains(&spec.id) && report.deliverables.is_empty() {
        let diagnostic =
            crate::scenarios::kanban::diagnostics(&attempt_id).and_then(|mut value| {
                let policy = crate::redaction::RedactionPolicy::from_environment();
                report
                    .asset_redaction
                    .merge(policy.redact_value(&mut value));
                policy.assert_clean(&serde_json::to_vec(&value)?)?;
                artifact::write_json(
                    output,
                    &PathBuf::from("evidence")
                        .join(run_id)
                        .join(&attempt_id)
                        .join("kanban-controller.json"),
                    "kanban_controller",
                    "runtime_diagnostics",
                    &value,
                )
            });
        match diagnostic {
            Ok(reference) => report.evidence.push(reference),
            Err(error) => report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Collect,
                format!("preserve Kanban controller diagnostics: {error:#}"),
            ),
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
    report.wall_time_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    finish_native_assessment(&spec, &mut report);
    report.update_cost();
    report.update_efficiency();
    report.refresh_dimensions(expects_deliverables);
    // Status, score, cost, and efficiency are final; the behavioral audit
    // below is advisory evidence and only ever fills `report.audit`.
    let audit = crate::audit::run_audit(&spec, &report);
    report.audit = Some(audit);
    if let Err(error) = emit_event(
        control,
        SuiteEvent::AttemptFinished {
            attempt_id: attempt_id.clone(),
        },
    )
    .await
    {
        record_checkpoint_failure(
            &mut report,
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
            scenario_id,
            run_id: run_id.to_string(),
            attempt_id: attempt_id.clone(),
            session_id,
            resume_state_path,
        },
    )
    .await
    {
        record_checkpoint_failure(
            &mut report,
            FailurePhase::Setup,
            format!("persist adaptive attempt checkpoint: {error:#}"),
        );
    }
    if report.failures.is_empty()
        && emit_attempt_phase(
            control,
            SuitePhase::SettingUp,
            FailurePhase::Setup,
            &mut report,
        )
        .await
    {
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
                        if report.failures.is_empty()
                            && emit_attempt_phase(
                                control,
                                SuitePhase::Executing,
                                FailurePhase::Execute,
                                &mut report,
                            )
                            .await
                        {
                            let uses_harness =
                                runtime.materialized.definition.nodes.iter().any(|node| {
                                    crate::workflow::opens_harness_session(&node.step_type)
                                });
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
                                        Ok(WorkflowResumeIdentity {
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
    report.update_efficiency();
    report.refresh_dimensions(false);
    emit_attempt_phase(
        control,
        SuitePhase::Persisting,
        FailurePhase::Collect,
        &mut report,
    )
    .await;
    if let Err(error) = emit_event(
        control,
        SuiteEvent::AttemptFinished {
            attempt_id: attempt_id.clone(),
        },
    )
    .await
    {
        record_checkpoint_failure(
            &mut report,
            FailurePhase::Cleanup,
            format!("persist adaptive attempt completion: {error:#}"),
        );
    }
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
) -> Result<AdaptivePlannerMetadata> {
    let invalidation = match scenario_id {
        ScenarioId::IncidentResponse => AdaptivePlannerInvalidation {
            description: "A trusted candidate-validation probe invalidated the initial diagnosis-only plan and requires bounded remediation plus revalidation before terminal action."
                .into(),
            evidence_ids: vec![
                crate::workflow::incident_response::INVALIDATION_EVIDENCE_ID.into(),
            ],
        },
        ScenarioId::ReleaseTrainRecovery => AdaptivePlannerInvalidation {
            description: "The trusted promotion preview exposed an incompatible historical latest graph and invalidated the stale null-CAS operation."
                .into(),
            evidence_ids: vec![
                crate::workflow::release_train_recovery::INVALIDATION_EVIDENCE_ID.into(),
            ],
        },
        ScenarioId::CrossRepoContractMigration => AdaptivePlannerInvalidation {
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
    Ok(AdaptivePlannerMetadata {
        scenario_id: scenario_id.as_str().into(),
        objective: spec.prompt.clone(),
        reference_checks: spec
            .criteria
            .iter()
            .map(|criterion| AdaptivePlannerReferenceCheck {
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
            scenario_id,
            run_id: run_id.to_string(),
            attempt_id: attempt_id.clone(),
            session_id,
            resume_state_path: None,
        },
    )
    .await
    {
        record_checkpoint_failure(
            &mut report,
            FailurePhase::Setup,
            format!("persist attempt checkpoint: {error:#}"),
        );
    }

    if report.failures.is_empty() {
        emit_attempt_phase(
            control,
            SuitePhase::SettingUp,
            FailurePhase::Setup,
            &mut report,
        )
        .await;
    }

    if report.failures.is_empty()
        && emit_attempt_phase(
            control,
            SuitePhase::Executing,
            FailurePhase::Execute,
            &mut report,
        )
        .await
    {
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
                    .any(|node| crate::workflow::opens_harness_session(&node.step_type))
                    || crate::scenarios::swe_service::is_swe(scenario_id);
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
                        Ok(workflow) => {
                            let terminal = crate::scenarios::swe_service::is_swe(scenario_id)
                                .then(|| {
                                    crate::scenarios::swe_service::execution_outcome(
                                        output,
                                        &attempt_id,
                                    )
                                })
                                .flatten();
                            populate_composite_report_with_terminal(&mut report, workflow, terminal)
                        }
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

    if crate::scenarios::swe_service::is_swe(scenario_id) {
        if let Err(error) =
            crate::scenarios::swe_service::attach_report(output, &attempt_id, &case, &mut report)
        {
            report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Collect,
                format!("capture SWE evidence: {error:#}"),
            );
        }
    }
    report.wall_time_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    if report.assessment_results.is_empty() {
        ensure_assessment_results(&spec, &mut report);
    }
    report.update_efficiency();
    report.refresh_dimensions(crate::scenarios::swe_service::is_swe(scenario_id));
    emit_attempt_phase(
        control,
        SuitePhase::Persisting,
        FailurePhase::Collect,
        &mut report,
    )
    .await;
    if let Err(error) = emit_event(
        control,
        SuiteEvent::AttemptFinished {
            attempt_id: attempt_id.clone(),
        },
    )
    .await
    {
        record_checkpoint_failure(
            &mut report,
            FailurePhase::Cleanup,
            format!("persist composite attempt completion: {error:#}"),
        );
    }
    report
}

fn populate_composite_report(
    report: &mut E2eRunReport,
    workflow: crate::workflow::WorkflowAttemptReport,
) {
    populate_composite_report_with_terminal(report, workflow, None)
}

pub(crate) fn populate_composite_report_with_terminal(
    report: &mut E2eRunReport,
    workflow: crate::workflow::WorkflowAttemptReport,
    terminal: Option<RunStatus>,
) {
    report.session_id = workflow
        .steps
        .iter()
        .find_map(|step| step.harness_session_id.clone())
        .unwrap_or_else(|| format!("scenario_{}", workflow.attempt_id));
    report.wall_time_ms = workflow.duration_ms;
    report.criteria = workflow
        .criteria
        .iter()
        .map(|criterion| CriterionReport {
            id: criterion.id.clone(),
            description: None,
            possible: criterion.weight,
            awarded: criterion.score.and_then(|score| {
                score
                    .is_finite()
                    .then(|| (score.clamp(0.0, 1.0) * f64::from(criterion.weight)).round() as u8)
            }),
            reason: criterion.summary.clone(),
        })
        .collect();
    report.score = crate::report::criteria_score(&report.criteria);
    report.cost = CostReport {
        subject_usd: workflow.aggregate_cost_usd,
        total_usd: workflow.aggregate_cost_usd,
    };
    for step in &workflow.steps {
        for failure in &step.failures {
            report.push_failure(
                if let Some(status) = terminal.filter(|_| {
                    matches!(failure.phase, WorkflowFailurePhase::Cancel)
                        || (failure.phase == WorkflowFailurePhase::Execute
                            && step.step_type != crate::scenarios::swe_service::workflow::CAPTURE)
                }) {
                    status
                } else if failure.technical {
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
        && terminal.is_none()
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
        report.finish(RunStatus::Passed);
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

struct RetryRequest<'a> {
    scenario_id: ScenarioId,
    subject: &'a SubjectConfig,
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
            && scenario_id.execution_kind().replay_safe()
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
    code: String,
    retry_scope: crate::report::RetryScope,
    contamination: crate::report::ContaminationScope,
    message: String,
}

struct ExecutionRequest<'a> {
    subject: &'a SubjectConfig,
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
            code: crate::report::classify_failure(status, phase),
            retry_scope: crate::report::RetryScope::None,
            contamination: crate::report::ContaminationScope::None,
            message: message.into(),
        }
    }

    fn retryable(
        status: RunStatus,
        phase: FailurePhase,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status,
            phase,
            code: code.into(),
            retry_scope: crate::report::RetryScope::SameSlot,
            contamination: crate::report::ContaminationScope::None,
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
    // Engineering fixtures are allocated during setup. Use that validated,
    // attempt-owned root for the subject and its descendants, never the host cwd.
    let filesystem_metadata =
        crate::scenarios::engineering_ticket::prepared_filesystem_root(spec.id, run_id)
            .map_err(|error| scenario_setup_failure(error.to_string()))?
            .or(
                crate::scenarios::linkly::prepared_filesystem_root(spec.id, run_id)
                    .map_err(|error| scenario_setup_failure(error.to_string()))?,
            )
            .map(|root| json!({ "fs_scope": { "root": root } }))
            .or(filesystem_metadata);
    let required_functions = crate::scenarios::required_functions(spec.id, run_id);
    report.worker_contracts = context
        .observe_function_contracts(&required_functions)
        .await
        .map_err(|error| {
            scenario_setup_failure(format!("required function preflight: {error:#}"))
        })?;
    emit_phase(control, SuitePhase::Executing)
        .await
        .map_err(|error| journal_checkpoint_failure(FailurePhase::Execute, error.to_string()))?;
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
                        max_cost_usd: subject_cost_cap_usd(spec.id),
                        max_output_tokens: spec.execution.max_output_tokens,
                        max_total_tokens: spec.execution.max_total_tokens,
                        max_validation_retries: spec.execution.max_validation_retries,
                        functions: Some(e2e_function_policy(spec, run_id)),
                        metadata: filesystem_metadata.clone(),
                    }),
                },
            )
            .await
            .map_err(subject_dispatch_failure)?;
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
                    let failure = subject_failure(FailurePhase::Execute, error.to_string());
                    capture_partial_observation(context, session_id, report).await;
                    capture_failed_subject_assets(
                        context, capture, case, session_id, output, control, report,
                    )
                    .await;
                    return Err(failure);
                }
            },
        );
        // Preserve known usage before any later status/transcript read can fail.
        report.metrics = metrics.clone();
    }
    let metrics = metrics.expect("every scenario has at least one scripted message");
    let terminal_status = match context
        .trigger::<_, Option<StatusReport>>(
            "harness::status",
            json!({ "session_id": session_id, "verbose": true }),
        )
        .await
    {
        Ok(Some(status)) => status,
        Ok(None) => {
            capture_partial_observation(context, session_id, report).await;
            return Err(RunFailure::new(
                RunStatus::InfrastructureError,
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
    emit_attempt_phase(
        control,
        SuitePhase::Collecting,
        FailurePhase::Collect,
        report,
    )
    .await;
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
        let captured = capture_assets_before_cleanup(
            context,
            capture,
            &observation,
            spec.id,
            run_id,
            output,
            report,
        )
        .await?;
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
    emit_attempt_phase(
        control,
        SuitePhase::Evaluating,
        FailurePhase::Evaluate,
        report,
    )
    .await;
    let objective = (spec.evaluate)(context, &observation, run_id)
        .await
        .map_err(|error| {
            RunFailure::new(
                RunStatus::InfrastructureError,
                FailurePhase::Evaluate,
                format!("scenario '{}' evaluator failed: {error:#}", spec.id),
            )
        })?;
    apply_objective_evaluation(spec, report, objective)
}

fn apply_objective_evaluation(
    spec: &ScenarioSpec,
    report: &mut E2eRunReport,
    objective: ObjectiveEvaluation,
) -> Result<(), RunFailure> {
    validate_objective_evaluation(spec, &objective).map_err(|error| {
        RunFailure::new(
            RunStatus::InfrastructureError,
            FailurePhase::Evaluate,
            error.to_string(),
        )
    })?;
    report.set_completion(
        objective.completion,
        crate::report::EvaluatorAvailability::Available,
    );
    report.criteria = criterion_reports(spec, objective.awards);
    report.assessment_results = materialize_assessment_results(spec, &report.criteria);
    update_score(report);
    if let Some(error) = objective.infrastructure_error {
        return Err(infrastructure_failure(FailurePhase::Evaluate, error));
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

fn finish_native_assessment(spec: &ScenarioSpec, report: &mut E2eRunReport) {
    ensure_assessment_results(spec, report);
    if report.failures.is_empty() {
        if report.evaluators.completion == crate::report::EvaluatorAvailability::Available
            || report.assessment_results.is_empty()
        {
            report.finish(RunStatus::Passed);
        } else {
            report.push_failure(
                RunStatus::InfrastructureError,
                FailurePhase::Evaluate,
                format!(
                    "scenario '{}': evaluation completed without criterion observations",
                    spec.id
                ),
            );
        }
    }
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
    report.assessment_results = materialize_assessment_results(spec, &report.criteria);
    for result in &mut report.assessment_results {
        result.outcome = AssessmentOutcome::NotEvaluated;
        result.score = None;
        result.summary = reason.clone();
    }
}

fn materialize_assessment_results(
    spec: &ScenarioSpec,
    criteria: &[CriterionReport],
) -> Vec<AssessmentResult> {
    spec.declared_assessments()
        .into_iter()
        .map(|declaration| {
            let criterion = criteria
                .iter()
                .find(|criterion| criterion.id == declaration.criterion_id);
            let awarded = criterion.and_then(|criterion| criterion.awarded);
            AssessmentResult {
                criterion_id: declaration.criterion_id.clone(),
                target: AssessmentTarget {
                    kind: AssessmentTargetKind::Criterion,
                    id: declaration.criterion_id,
                },
                kind: declaration.kind,
                policy: declaration.policy,
                dimension: declaration.dimension,
                outcome: score_assessment_outcome(awarded, declaration.possible),
                score: awarded.map(|awarded| AssessmentScore {
                    awarded,
                    possible: declaration.possible,
                }),
                summary: criterion
                    .map(|criterion| criterion.reason.clone())
                    .unwrap_or_else(|| "Deterministic assessment was not evaluated.".into()),
                evidence: Vec::new(),
            }
        })
        .collect()
}

fn score_assessment_outcome(awarded: Option<u8>, possible: u8) -> AssessmentOutcome {
    match awarded {
        None => AssessmentOutcome::NotEvaluated,
        Some(awarded) if awarded == possible => AssessmentOutcome::Passed,
        Some(0) => AssessmentOutcome::Failed,
        Some(_) => AssessmentOutcome::Partial,
    }
}

async fn capture_assets_before_cleanup(
    context: &E2eContext,
    capture: ScenarioDeliverableCapture,
    observation: &ScenarioObservation,
    scenario_id: &str,
    run_id: &str,
    output: &Path,
    report: &mut E2eRunReport,
) -> Result<Vec<crate::scenarios::CapturedDeliverable>, RunFailure> {
    let captured = match capture(context, observation, run_id).await {
        Ok(captured) => captured,
        Err(error) => {
            let mut message = format!(
                "scenario '{}' asset capture was unreadable: {error:#}",
                scenario_id
            );
            let mut evaluation = asset::failed_capture_evaluation(
                &observation.case,
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
    let mut evaluation = asset::evaluate_assets(
        &observation.case,
        captured.clone(),
        AssetCaptureLimits::default(),
    )
    .map_err(|error| {
        RunFailure::new(
            RunStatus::InfrastructureError,
            FailurePhase::Evaluate,
            format!(
                "scenario '{}' deterministic asset validation failed: {error:#}",
                scenario_id
            ),
        )
    })?;
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
                    scenario_id
                ),
            ));
        }
    };
    report.deliverables = evaluation.deliverables;
    report.asset_assessments = evaluation.assessments;
    report.asset_redaction.merge(evaluation.redaction);
    report.asset_capture_manifest = Some(manifest.clone());
    report.evidence.push(manifest);
    Ok(captured)
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

async fn capture_failed_subject_assets(
    context: &E2eContext,
    capture: Option<ScenarioDeliverableCapture>,
    case: &ScenarioCase,
    session_id: &str,
    output: &Path,
    control: Option<&SuiteControl>,
    report: &mut E2eRunReport,
) {
    let Some(capture) = capture else {
        return;
    };
    if control.is_some_and(|control| *control.cancellation.borrow()) {
        return;
    }
    let status = match context
        .trigger::<_, Option<StatusReport>>(
            "harness::status",
            json!({ "session_id": session_id, "verbose": true }),
        )
        .await
    {
        Ok(Some(status)) => status,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(
                session_id,
                %error,
                "could not confirm failed E2E subject was quiescent before asset capture"
            );
            return;
        }
    };
    capture_confirmed_failed_subject_assets(
        context,
        capture,
        case,
        &status,
        control.is_some_and(|control| *control.cancellation.borrow()),
        output,
        report,
    )
    .await;
}

async fn capture_confirmed_failed_subject_assets(
    context: &E2eContext,
    capture: ScenarioDeliverableCapture,
    case: &ScenarioCase,
    status: &StatusReport,
    cancelled: bool,
    output: &Path,
    report: &mut E2eRunReport,
) {
    let Some(observation) = failed_subject_observation(case, report, status, cancelled) else {
        return;
    };
    report.terminal_status = Some(status.clone());
    let attempt_id = report.attempt_id.clone();
    if let Err(error) = capture_assets_before_cleanup(
        context,
        capture,
        &observation,
        case.scenario_id.as_str(),
        &attempt_id,
        output,
        report,
    )
    .await
    {
        tracing::warn!(
            scenario = case.scenario_id,
            session_id = report.session_id,
            error = %error.message,
            "could not preserve assets from failed E2E subject"
        );
    }
}

fn failed_subject_observation(
    case: &ScenarioCase,
    report: &E2eRunReport,
    status: &StatusReport,
    cancelled: bool,
) -> Option<ScenarioObservation> {
    let metrics = report.metrics.as_ref()?;
    let transcript = report.transcript.as_ref()?;
    if cancelled
        || !case.deliverable_contract.capture_before_cleanup
        || !metrics.complete
        || status.status != TurnStatus::Failed
        || !status.pending_function_calls.is_empty()
    {
        return None;
    }
    let mut incomplete_metrics = serde_json::to_value(metrics).ok()?;
    incomplete_metrics["complete"] = false.into();
    let metrics = serde_json::from_value(incomplete_metrics).ok()?;
    Some(ScenarioObservation {
        case: case.clone(),
        metrics,
        transcript: transcript.clone(),
        response: common::final_response(transcript),
        deliverables: Vec::new(),
    })
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
    !report.failures.is_empty()
        && report.failures.iter().all(|failure| {
            failure.retry_scope == crate::report::RetryScope::SameSlot
                && failure.contamination == crate::report::ContaminationScope::None
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

fn subject_dispatch_failure(error: anyhow::Error) -> RunFailure {
    if crate::context::request_not_dispatched(&error) {
        RunFailure::retryable(
            RunStatus::InfrastructureError,
            FailurePhase::Execute,
            "harness_send_not_dispatched",
            format!("{error:#}"),
        )
    } else {
        // A lost response is not proof that send did not execute. Read failures
        // retry the read itself in E2eContext, never a completed subject run.
        subject_failure(FailurePhase::Execute, format!("{error:#}"))
    }
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

fn journal_checkpoint_failure(phase: FailurePhase, message: String) -> RunFailure {
    let mut failure = infrastructure_failure(phase, message);
    failure.code = "journal_checkpoint_failed".into();
    failure
}

fn scenario_setup_failure(message: String) -> RunFailure {
    infrastructure_failure(
        FailurePhase::Setup,
        format!("scenario setup failed: {message}"),
    )
}

fn update_score(report: &mut E2eRunReport) {
    report.score = crate::report::criteria_score(&report.criteria);
}

fn validate_objective_evaluation(
    spec: &ScenarioSpec,
    evaluation: &ObjectiveEvaluation,
) -> Result<()> {
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
        if let Some(awarded) = award.awarded.filter(|awarded| *awarded > criterion.weight) {
            bail!(
                "scenario '{}': evaluation contract violation: criterion '{}' awarded {}; expected awarded in 0..={}; action: reduce the award or change the configured weight",
                spec.id, award.id,
                awarded,
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
                description: Some(criterion.description.to_string()),
                possible: criterion.weight,
                awarded: award.as_ref().and_then(|(awarded, _)| *awarded),
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
    use crate::report::{CompletionState, EvaluationDimension};
    use crate::scenarios::{
        ArtifactExpectation, CapturedDeliverable, CapturedDeliverableContent, CapturedInvariant,
        InvariantSpec, ProvenanceEvidence,
    };
    use crate::wire::{
        SessionMetricsPayload, SessionMetricsResponse, SessionUsageTotals, StatusReportPayload,
    };

    fn terminal_metrics(complete: bool) -> SessionMetricsResponse {
        SessionMetricsResponse::from_normalized(SessionMetricsPayload {
            root_session_id: "session".into(),
            complete,
            totals: SessionUsageTotals {
                sessions: 1,
                turns: 69,
                function_calls: 82,
                ..Default::default()
            },
            by_session: Vec::new(),
            traces: None,
        })
    }

    fn terminal_status(status: TurnStatus, pending: Vec<String>) -> StatusReport {
        StatusReport::from_normalized(StatusReportPayload {
            session_id: "session".into(),
            turn_id: Some("turn".into()),
            status,
            step: 69,
            turn_count: 69,
            max_turns: 100,
            pending_function_calls: pending,
            children: Vec::new(),
            queued: Vec::new(),
            expects_wake: false,
            result_error: Some("token budget exhausted".into()),
            validation_retries: 0,
            transient_resumes: 0,
        })
    }

    fn failed_capture_case() -> ScenarioCase {
        ScenarioCase::new(
            "failed_capture",
            1,
            json!({}),
            vec![],
            crate::scenarios::DeliverableContract {
                artifacts: vec![ArtifactExpectation {
                    id: "result".into(),
                    kind: "json".into(),
                    media_type: "application/json".into(),
                    schema: json!({"type": "object"}),
                    max_size_bytes: 1024,
                }],
                invariants: vec![InvariantSpec {
                    id: "preserved".into(),
                    description: "The partial result is preserved.".into(),
                }],
                provenance_required: true,
                capture_before_cleanup: true,
            },
        )
        .unwrap()
        .sealed_for_tests()
    }

    fn partial_asset_capture<'a>(
        _context: &'a E2eContext,
        observation: &'a ScenarioObservation,
        attempt_id: &'a str,
    ) -> crate::scenarios::DeliverableCaptureFuture<'a> {
        Box::pin(async move {
            assert!(!observation.metrics.complete);
            assert_eq!(attempt_id, "attempt");
            Ok(vec![CapturedDeliverable {
                id: "result".into(),
                kind: "json".into(),
                content: json!({"partial": true}).into(),
                invariants: vec![CapturedInvariant {
                    id: "preserved".into(),
                    passed: true,
                    reason: "captured before cleanup".into(),
                }],
                provenance: vec![ProvenanceEvidence {
                    kind: "function_call".into(),
                    source_id: "capture-1".into(),
                    relation: "created".into(),
                }],
            }])
        })
    }

    fn failed_asset_capture<'a>(
        _context: &'a E2eContext,
        observation: &'a ScenarioObservation,
        attempt_id: &'a str,
    ) -> crate::scenarios::DeliverableCaptureFuture<'a> {
        Box::pin(async move {
            assert!(!observation.metrics.complete);
            assert_eq!(attempt_id, "attempt");
            anyhow::bail!("secondary capture failure")
        })
    }

    fn resource_limited_report() -> E2eRunReport {
        let mut report = test_run_report();
        report.metrics = Some(terminal_metrics(true));
        report.transcript = Some(json!([
            {"role": "assistant", "content": "partial implementation"}
        ]));
        report.finish(RunStatus::ResourceLimit);
        report.score = Some(17);
        report.score = Some(17);
        report
    }

    #[tokio::test]
    async fn resource_failure_preserves_partial_assets_without_changing_outcome() {
        let context = E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let output = tempfile::tempdir().unwrap();
        let mut report = resource_limited_report();

        capture_confirmed_failed_subject_assets(
            &context,
            partial_asset_capture,
            &failed_capture_case(),
            &terminal_status(TurnStatus::Failed, Vec::new()),
            false,
            output.path(),
            &mut report,
        )
        .await;

        assert_eq!(report.status, RunStatus::ResourceLimit);
        assert_eq!(
            report.completion,
            crate::report::CompletionState::TaskIncomplete
        );
        assert_eq!(report.technical, crate::report::TechnicalState::Valid);
        assert_eq!(report.score, Some(17));
        assert_eq!(report.score, Some(17));
        assert_eq!(
            report.terminal_status.as_ref().map(|status| status.status),
            Some(TurnStatus::Failed)
        );
        assert!(report.scenario_measurements.is_empty());
        assert_eq!(report.deliverables.len(), 1);
        assert_eq!(report.evidence.len(), 1);
        report.evidence[0].verify(output.path()).unwrap();
    }

    #[tokio::test]
    async fn capture_failure_remains_secondary_to_resource_failure() {
        let context = E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let output = tempfile::tempdir().unwrap();
        let mut report = resource_limited_report();

        capture_confirmed_failed_subject_assets(
            &context,
            failed_asset_capture,
            &failed_capture_case(),
            &terminal_status(TurnStatus::Failed, Vec::new()),
            false,
            output.path(),
            &mut report,
        )
        .await;

        assert_eq!(report.status, RunStatus::ResourceLimit);
        assert_eq!(
            report.completion,
            crate::report::CompletionState::TaskIncomplete
        );
        assert_eq!(report.technical, crate::report::TechnicalState::Valid);
        assert_eq!(report.score, Some(17));
        assert_eq!(report.score, Some(17));
        assert_eq!(
            report.terminal_status.as_ref().map(|status| status.status),
            Some(TurnStatus::Failed)
        );
        assert!(report.scenario_measurements.is_empty());
        assert!(report.deliverables.is_empty());
        assert_eq!(report.evidence.len(), 1);
        report.evidence[0].verify(output.path()).unwrap();
    }

    #[test]
    fn terminal_failed_subject_is_captured_as_incomplete_without_mutating_report_metrics() {
        let case = ScenarioId::RegistryImplementation
            .materialize("failed-capture", 1)
            .unwrap()
            .case;
        let mut report = test_run_report();
        report.metrics = Some(terminal_metrics(true));
        report.transcript = Some(json!([
            {"role": "assistant", "content": "partial implementation"}
        ]));
        let status = terminal_status(TurnStatus::Failed, Vec::new());

        let observation = failed_subject_observation(&case, &report, &status, false).unwrap();

        assert!(report.metrics.as_ref().unwrap().complete);
        assert!(!observation.metrics.complete);
        assert_eq!(observation.transcript, report.transcript.clone().unwrap());
        assert!(observation.deliverables.is_empty());
    }

    #[test]
    fn failed_subject_capture_requires_complete_observation_and_quiescent_failure() {
        let case = ScenarioId::RegistryImplementation
            .materialize("failed-capture-gates", 1)
            .unwrap()
            .case;
        let mut report = test_run_report();
        report.metrics = Some(terminal_metrics(true));
        report.transcript = Some(json!([]));

        assert!(failed_subject_observation(
            &case,
            &report,
            &terminal_status(TurnStatus::Failed, Vec::new()),
            true,
        )
        .is_none());
        assert!(failed_subject_observation(
            &case,
            &report,
            &terminal_status(TurnStatus::Running, Vec::new()),
            false,
        )
        .is_none());
        assert!(failed_subject_observation(
            &case,
            &report,
            &terminal_status(TurnStatus::Failed, vec!["browser::screenshot".into()]),
            false,
        )
        .is_none());

        report.metrics = Some(terminal_metrics(false));
        assert!(failed_subject_observation(
            &case,
            &report,
            &terminal_status(TurnStatus::Failed, Vec::new()),
            false,
        )
        .is_none());
        report.metrics = Some(terminal_metrics(true));
        report.transcript = None;
        assert!(failed_subject_observation(
            &case,
            &report,
            &terminal_status(TurnStatus::Failed, Vec::new()),
            false,
        )
        .is_none());
    }

    #[test]
    fn registry_verification_cannot_precede_its_implementation() {
        let implementation = ScenarioId::RegistryImplementation;
        let verification = ScenarioId::RegistryVerification;
        assert!(validate_registry_handoff_order(&[implementation, verification]).is_ok());
        assert!(validate_registry_handoff_order(&[verification, implementation]).is_err());
    }

    fn checkpoint_deliverable() -> crate::report::DeliverableReport {
        crate::report::DeliverableReport {
            id: "retained-output".into(),
            kind: "test".into(),
            media_type: "application/json".into(),
            content_format: crate::report::DeliverableContentFormat::Json,
            content_sha256: "sha256:fixture".into(),
            content_size_bytes: 1,
            schema_valid: true,
            provenance_valid: true,
            invariants: vec![],
            provenance: vec![],
            preview: json!({}),
            artifact: None,
            content: CapturedDeliverableContent::Json(json!({
                "evidence": "retained output", "api_key": "private-secret",
            })),
        }
    }

    #[test]
    fn final_report_write_failure_preserves_redacted_observations_without_a_fake_path() {
        let output = tempfile::NamedTempFile::new().unwrap();
        let execution = ExecutionIdentity {
            execution_id: "execution".into(),
            lane: "test".into(),
            started_at: "2026-09-04T00:00:00Z".into(),
            completed_at: "2026-09-04T00:01:00Z".into(),
        };
        let system = crate::identity::SystemUnderTestIdentity {
            stack: crate::identity::StackIdentity::Source {
                workers_repository: "iii-hq/workers".into(),
                workers_revision: "0".repeat(40),
            },
            engine_version: "0.22.0".into(),
            engine_revision: None,
            harness_version: "1.8.0".into(),
            e2e_repository: "iii-hq/harness-e2e".into(),
            e2e_revision: "0".repeat(40),
            contract_hashes: Default::default(),
        };
        let subject = ModelArtifact {
            model: "model".into(),
            provider: "provider".into(),
            context_window: 1000,
            max_output_tokens: 100,
            supports_tools: Some(true),
            supports_vision: None,
        };
        let manifest = E2eManifest {
            execution: execution.clone(),
            system_under_test: system.clone(),
            subject: subject.clone(),
            control_plane: ControlPlaneEvidence { functions: vec![] },
            observation_contract: None,
            worker_contracts: vec![],
        };
        let materialized = ScenarioId::ContextPressure.materialize("test", 1).unwrap();
        let mut run = test_run_report();
        run.transcript = Some(json!({"text":"retained evidence", "api_key":"private-secret"}));
        run.deliverables.push(checkpoint_deliverable());
        run.finish(RunStatus::Passed);
        let scenario = E2eScenarioReport::aggregate_case_with_planned(
            materialized.case,
            materialized.spec.execution,
            2,
            vec![run],
        );
        let mut report = E2eReport::new(execution, system, subject, None, vec![scenario]);
        assert!(
            persist_report_preserving_observations(&mut report, &manifest, output.path())
                .unwrap()
                .is_none()
        );
        // A subsequent persistence attempt must still have the captured bytes;
        // the public Results serialization intentionally skips this field.
        assert_eq!(
            report.scenarios[0].runs[0].deliverables[0]
                .content
                .as_json()
                .unwrap()["evidence"],
            "retained output"
        );
        redact_unpersisted_report(&mut report).unwrap();
        assert_eq!(report.scenarios[0].aggregate.observed_runs, 1);
        assert_eq!(report.scenarios[0].aggregate.planned_runs, 2);
        assert_eq!(
            report.scenarios[0].runs[0].transcript.as_ref().unwrap()["text"],
            "retained evidence"
        );
        assert_ne!(
            report.scenarios[0].runs[0].transcript.as_ref().unwrap()["api_key"],
            "private-secret"
        );
        assert!(!report.passed);
        assert_eq!(report.persistence_errors.len(), 1);
        assert_eq!(report.report_state, crate::report::ReportState::Partial);
    }

    #[test]
    fn schedule_is_round_robin_and_soft_deadline_only_blocks_new_slots() {
        assert_eq!(
            round_robin_slots(2, 3).collect::<Vec<_>>(),
            vec![(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2)]
        );
        assert_eq!(round_robin_slots(2, 0).count(), 0);
        assert!(slot_deferral_reason(
            false,
            false,
            Duration::from_secs(9),
            Some(Duration::from_secs(10))
        )
        .is_none());
        assert!(slot_deferral_reason(
            false,
            false,
            Duration::from_secs(10),
            Some(Duration::from_secs(10))
        )
        .unwrap()
        .contains("deadline"));
        assert!(slot_deferral_reason(true, false, Duration::ZERO, None)
            .unwrap()
            .contains("persistence"));
        assert!(slot_deferral_reason(false, true, Duration::ZERO, None)
            .unwrap()
            .contains("cancelled"));
    }

    #[tokio::test]
    async fn failed_materialization_retains_every_planned_slot_without_fabricating_a_case() {
        let key = ScenarioId::MinimalPath;
        let slots = vec![crate::journal::JournalSlot {
            slot_id: slot_id(&key, 9, 0),
            ordinal: 0,
            scenario_id: key.as_str().into(),
            case_id: "unresolved:retained-inventory-identity".into(),
            seed: "9".into(),
            repetition: 0,
        }];
        let mut errors = Vec::new();
        let deferred = defer_planned_case(
            None,
            &slots,
            &key,
            9,
            3,
            "invalid source".into(),
            &mut errors,
        )
        .await;
        assert_eq!(deferred.case_id, slots[0].case_id);
        assert!(deferred.case.is_none());
        assert_eq!(deferred.aggregate.planned_runs, 3);
        assert_eq!(deferred.aggregate.observed_runs, 0);
        assert_eq!(deferred.aggregate.deferred_runs, 3);
        assert_eq!(deferred.deferral_reason.as_deref(), Some("invalid source"));
        assert!(errors.is_empty());
    }

    fn checkpoint_control() -> (SuiteControl, mpsc::Receiver<SuiteEventEnvelope>) {
        let (events, receiver) = mpsc::channel(8);
        (
            SuiteControl {
                execution_id: "execution".into(),
                lane: "local".into(),
                events,
                cancellation: watch::channel(false).1,
                adaptive_resume: None,
            },
            receiver,
        )
    }

    #[tokio::test]
    async fn rejected_phase_prevents_new_work_even_when_later_events_succeed() {
        for (phase, failure_phase) in [
            (SuitePhase::SettingUp, FailurePhase::Setup),
            (SuitePhase::Executing, FailurePhase::Execute),
            (SuitePhase::Collecting, FailurePhase::Collect),
            (SuitePhase::Evaluating, FailurePhase::Evaluate),
            (SuitePhase::Persisting, FailurePhase::Collect),
            (SuitePhase::CleaningUp, FailurePhase::Cleanup),
        ] {
            let output = tempfile::tempdir().unwrap();
            let (control, mut receiver) = checkpoint_control();
            let acknowledgements = tokio::spawn(async move {
                receiver
                    .recv()
                    .await
                    .unwrap()
                    .acknowledge(Err(anyhow::anyhow!("phase checkpoint rejected")));
                receiver.recv().await.unwrap().acknowledge(Ok(()));
            });
            let mut run = test_run_report();
            run.transcript = Some(json!({"text": "already captured evidence"}));
            assert!(!emit_attempt_phase(Some(&control), phase, failure_phase, &mut run).await);
            assert_eq!(run.failures[0].code, "journal_checkpoint_failed");
            assert_eq!(run.failures[0].retry_scope, crate::report::RetryScope::None);
            assert!(!is_retryable_technical_failure(&run));
            emit_event(
                Some(&control),
                SuiteEvent::AttemptFinished {
                    attempt_id: run.attempt_id.clone(),
                },
            )
            .await
            .unwrap();
            acknowledgements.await.unwrap();
            let error = commit_run_checkpoint(Some(&control), output.path(), "slot", &run, true)
                .await
                .unwrap_err();
            assert!(error
                .to_string()
                .contains("attempt lifecycle checkpoint failed"));
            assert_eq!(run.transcript.unwrap()["text"], "already captured evidence");
            assert!(slot_deferral_reason(true, false, Duration::ZERO, None).is_some());
        }
    }

    #[test]
    fn execute_phase_failure_has_nonretryable_checkpoint_classification() {
        let failure = journal_checkpoint_failure(FailurePhase::Execute, "ack rejected".into());
        assert_eq!(failure.status, RunStatus::InfrastructureError);
        assert_eq!(failure.code, "journal_checkpoint_failed");
        assert_eq!(failure.retry_scope, crate::report::RetryScope::None);
    }

    #[tokio::test]
    async fn failed_journal_commit_preserves_full_redacted_run_and_stops_new_slots() {
        let output = tempfile::tempdir().unwrap();
        let (control, mut receiver) = checkpoint_control();
        let rejected = tokio::spawn(async move {
            receiver
                .recv()
                .await
                .unwrap()
                .acknowledge(Err(anyhow::anyhow!("journal unavailable")));
        });
        let mut run = test_run_report();
        run.transcript = Some(json!({"text":"retained evidence", "api_key":"private-secret"}));
        run.deliverables.push(checkpoint_deliverable());
        run.retry_attempts.push(RetryAttemptReport::from(&run));
        run.finish(RunStatus::Passed);
        let error = commit_run_checkpoint(Some(&control), output.path(), "slot", &run, false)
            .await
            .unwrap_err();
        rejected.await.unwrap();
        assert!(error.to_string().contains("journal unavailable"));
        let bytes = std::fs::read(
            output
                .path()
                .join("journal/runs/slot")
                .join(format!("{}.json", run.run_id)),
        )
        .unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved["run"]["transcript"]["text"], "retained evidence");
        assert_ne!(saved["run"]["transcript"]["api_key"], "private-secret");
        assert!(saved["run"].get("hard_gates").is_none());
        assert!(saved["run"]["criteria"].is_array());
        assert_eq!(saved["run"]["status"], "passed");
        for content in [
            &saved["capture"]["deliverables"][0]["content"]["content"],
            &saved["capture"]["retry_attempts"][0]["deliverables"][0]["content"]["content"],
        ] {
            assert_eq!(content["evidence"], "retained output");
            assert_ne!(content["api_key"], "private-secret");
        }
        assert!(slot_deferral_reason(true, false, Duration::ZERO, None).is_some());
    }

    #[tokio::test]
    async fn missing_attempt_lifecycle_never_emits_an_unknown_observation() {
        let output = tempfile::tempdir().unwrap();
        let (control, mut receiver) = checkpoint_control();
        let mut run = test_run_report();
        record_checkpoint_failure(&mut run, FailurePhase::Setup, "start rejected".into());
        assert!(
            commit_run_checkpoint(Some(&control), output.path(), "slot", &run, true)
                .await
                .is_err()
        );
        assert!(receiver.try_recv().is_err());
        assert!(output
            .path()
            .join("journal/runs/slot")
            .join(format!("{}.json", run.run_id))
            .is_file());
    }

    #[tokio::test]
    async fn checkpoint_reaches_a_real_journal_and_replays_without_the_subject() {
        use crate::journal::{
            ExecutionJournal, ExecutionJournalEventKind as Event, ExecutionJournalHeader,
            JournalSlot, EXECUTION_JOURNAL_SCHEMA,
        };
        let output = tempfile::tempdir().unwrap();
        let journal = ExecutionJournal::initialize(
            output.path(),
            &ExecutionJournalHeader {
                schema: EXECUTION_JOURNAL_SCHEMA.into(),
                execution_id: "execution".into(),
                request_sha256: "sha256:request".into(),
                result_contract_sha256: crate::report::RESULT_CONTRACT_SHA256.into(),
                created_at: "now".into(),
                request: json!({}),
                runner: json!({}),
            },
        )
        .unwrap();
        journal
            .append(
                "now".into(),
                Event::SlotInventoryCommitted {
                    slots: vec![JournalSlot {
                        slot_id: "slot".into(),
                        ordinal: 0,
                        scenario_id: "case".into(),
                        case_id: "case".into(),
                        seed: "1".into(),
                        repetition: 0,
                    }],
                },
            )
            .unwrap();
        let (control, mut receiver) = checkpoint_control();
        let sink_journal = journal.clone();
        let sink = tokio::spawn(async move {
            let envelope = receiver.recv().await.unwrap();
            let result = match &envelope.event {
                SuiteEvent::RunCommitted {
                    slot_id,
                    run_id,
                    artifact,
                } => sink_journal
                    .append(
                        "now".into(),
                        Event::RunCommitted {
                            slot_id: slot_id.clone(),
                            run_id: run_id.clone(),
                            artifact: artifact.clone(),
                        },
                    )
                    .map(|_| ()),
                _ => panic!("unexpected event"),
            };
            envelope.acknowledge(result);
        });
        commit_run_checkpoint(
            Some(&control),
            output.path(),
            "slot",
            &test_run_report(),
            false,
        )
        .await
        .unwrap();
        sink.await.unwrap();
        assert_eq!(journal.replay().unwrap().runs_committed, 1);
    }

    #[tokio::test]
    async fn persistence_failure_is_recorded_once_and_preserves_the_safe_stop() {
        let (control, mut receiver) = checkpoint_control();
        let sink = tokio::spawn(async move {
            receiver
                .recv()
                .await
                .unwrap()
                .acknowledge(Err(anyhow::anyhow!("append refused")));
            assert!(receiver.recv().await.is_none());
        });
        let mut errors = Vec::new();
        preserve_event(
            Some(&control),
            SuiteEvent::Phase(SuitePhase::Executing),
            &mut errors,
        )
        .await;
        preserve_event(
            Some(&control),
            SuiteEvent::Phase(SuitePhase::Finalizing),
            &mut errors,
        )
        .await;
        assert_eq!(errors.len(), 1);
        drop(control);
        sink.await.unwrap();
    }

    #[test]
    fn ambiguous_transport_after_send_is_never_replayed() {
        for error in [
            iii_sdk::errors::Error::NotConnected,
            iii_sdk::errors::Error::Timeout,
            iii_sdk::errors::Error::WebSocket("reset".into()),
        ] {
            let failure = subject_dispatch_failure(anyhow::Error::new(error));
            assert_eq!(failure.retry_scope, crate::report::RetryScope::None);
        }
    }

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
            criteria: vec![CriterionSpec::scored(
                "objective",
                100,
                "objective",
                EvaluationDimension::StructuralIntegrity,
            )],
            setup: None,
            evaluate: evaluator as ScenarioEvaluator,
            cleanup: None,
        }
    }

    fn mixed_assessment_spec() -> ScenarioSpec {
        let mut spec = spec();
        spec.criteria = vec![
            CriterionSpec::scored(
                "required",
                70,
                "Required deterministic behavior.",
                EvaluationDimension::StructuralIntegrity,
            ),
            CriterionSpec::scored(
                "quality",
                30,
                "Advisory deterministic quality signal.",
                EvaluationDimension::Deliverable,
            ),
        ];
        spec
    }

    #[test]
    fn materializes_one_result_per_numeric_criterion() {
        let mut spec = spec();
        spec.criteria = vec![
            CriterionSpec::scored(
                "required",
                70,
                "Required deterministic behavior.",
                EvaluationDimension::StructuralIntegrity,
            ),
            CriterionSpec::scored(
                "signal",
                30,
                "Advisory deterministic signal.",
                EvaluationDimension::Efficiency,
            ),
        ];
        let criteria = vec![
            CriterionReport {
                id: "required".into(),
                description: None,
                possible: 70,
                awarded: Some(35),
                reason: "required behavior was incomplete".into(),
            },
            CriterionReport {
                id: "signal".into(),
                description: None,
                possible: 30,
                awarded: Some(12),
                reason: "partial efficiency evidence".into(),
            },
        ];
        let results = materialize_assessment_results(&spec, &criteria);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].criterion_id, "required");
        assert_eq!(
            results[0].policy,
            crate::assessment::AssessmentPolicy::Advisory
        );
        assert_eq!(results[0].outcome, AssessmentOutcome::Partial);
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
    fn partial_native_assessment_sums_the_evaluated_points_only() {
        let spec = mixed_assessment_spec();
        let mut report = test_run_report();
        apply_objective_evaluation(
            &spec,
            &mut report,
            ObjectiveEvaluation {
                completion: CompletionState::TaskIncomplete,
                awards: vec![
                    CriterionAward {
                        id: "required".into(),
                        awarded: Some(0),
                        reason: "Build failed".into(),
                    },
                    CriterionAward {
                        id: "quality".into(),
                        awarded: None,
                        reason: "Build prerequisite failed; browser checks were not run".into(),
                    },
                ],
                infrastructure_error: None,
            },
        )
        .unwrap_or_else(|error| panic!("{}", error.message));
        finish_native_assessment(&spec, &mut report);
        assert_eq!(report.technical, crate::report::TechnicalState::Valid);
        assert_eq!(report.completion, CompletionState::TaskIncomplete);
        assert_eq!(report.criteria[0].awarded, Some(0));
        assert_eq!(report.criteria[1].awarded, None);
        assert_eq!(
            report.assessment_results[0].outcome,
            AssessmentOutcome::Failed
        );
        assert_eq!(
            report.assessment_results[1].outcome,
            AssessmentOutcome::NotEvaluated
        );
        // The unevaluated criterion adds nothing; the evaluated one awarded zero.
        assert_eq!(report.score, Some(0));
        assert_eq!(report.score, Some(0));
    }

    #[test]
    fn native_finalizer_rejects_absent_evaluation() {
        let spec = mixed_assessment_spec();
        let mut report = test_run_report();
        finish_native_assessment(&spec, &mut report);
        assert_eq!(report.status, RunStatus::InfrastructureError);
        assert!(report.failures[0]
            .message
            .contains("without criterion observations"));
    }

    #[test]
    fn native_infrastructure_failure_preserves_prior_criterion_observations() {
        let spec = mixed_assessment_spec();
        let mut report = test_run_report();
        let error = apply_objective_evaluation(
            &spec,
            &mut report,
            ObjectiveEvaluation {
                completion: CompletionState::Undetermined,
                awards: vec![
                    CriterionAward {
                        id: "required".into(),
                        awarded: Some(70),
                        reason: "Delivery verified".into(),
                    },
                    CriterionAward {
                        id: "quality".into(),
                        awarded: None,
                        reason: "Browser failed to launch".into(),
                    },
                ],
                infrastructure_error: Some("Browser executable unavailable".into()),
            },
        )
        .unwrap_err();
        assert_eq!(error.status, RunStatus::InfrastructureError);
        report.push_failure(error.status, error.phase, error.message);
        ensure_assessment_results(&spec, &mut report);
        assert_eq!(
            report.technical,
            crate::report::TechnicalState::TechnicalInvalid
        );
        assert_eq!(
            report.assessment_results[0].score.as_ref().unwrap().awarded,
            70
        );
        assert_eq!(
            report.assessment_results[1].outcome,
            AssessmentOutcome::NotEvaluated
        );
        // The observed criterion keeps its points; the technical axis says the
        // run cannot be trusted, and aggregates leave it out of the mean.
        assert_eq!(report.score, Some(70));
        assert_eq!(
            report.technical,
            crate::report::TechnicalState::TechnicalInvalid
        );
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
                completion: crate::report::CompletionState::Completed,
                infrastructure_error: None,
                awards: vec![CriterionAward {
                    id: "objective".into(),
                    awarded: Some(100),
                    reason: "ok".into(),
                }],
            }
        )
        .is_ok());
        assert!(validate_objective_evaluation(
            &spec,
            &ObjectiveEvaluation {
                completion: crate::report::CompletionState::Completed,
                infrastructure_error: None,
                awards: Vec::new(),
            }
        )
        .is_err());
        let error = validate_objective_evaluation(
            &spec,
            &ObjectiveEvaluation {
                completion: crate::report::CompletionState::Completed,
                infrastructure_error: None,
                awards: vec![CriterionAward {
                    id: "objective".into(),
                    awarded: Some(101),
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
    fn criterion_reports_reuse_the_declared_description() {
        let reports = criterion_reports(
            &spec(),
            vec![CriterionAward {
                id: "objective".into(),
                awarded: Some(100),
                reason: "measured evidence".into(),
            }],
        );

        assert_eq!(reports[0].description.as_deref(), Some("objective"));
        assert_eq!(reports[0].reason, "measured evidence");
    }

    #[test]
    fn objective_contract_errors_identify_the_invalid_ids_and_values() {
        let spec = spec();
        let unknown = validate_objective_evaluation(
            &spec,
            &ObjectiveEvaluation {
                completion: crate::report::CompletionState::Completed,
                infrastructure_error: None,
                awards: vec![CriterionAward {
                    id: "unknown".into(),
                    awarded: Some(1),
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
                completion: crate::report::CompletionState::Completed,
                infrastructure_error: None,
                awards: vec![
                    CriterionAward {
                        id: "objective".into(),
                        awarded: Some(1),
                        reason: "first".into(),
                    },
                    CriterionAward {
                        id: "objective".into(),
                        awarded: Some(1),
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
    fn only_explicitly_typed_transient_failures_are_retried() {
        let mut transient = test_run_report();
        transient.push_typed_failure(
            RunStatus::SubjectError,
            FailurePhase::Execute,
            "subject_transport_transient",
            crate::report::RetryScope::SameSlot,
            crate::report::ContaminationScope::None,
            "zai stream ended without a terminal frame",
        );
        assert!(is_retryable_technical_failure(&transient));

        let mut untyped = test_run_report();
        untyped.push_failure(
            RunStatus::SubjectError,
            FailurePhase::Execute,
            "zai stream ended without a terminal frame",
        );
        assert!(!is_retryable_technical_failure(&untyped));

        let mut deterministic = test_run_report();
        deterministic.push_failure(
            RunStatus::InfrastructureError,
            FailurePhase::Evaluate,
            "evaluator returned an invalid criterion set",
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
