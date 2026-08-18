use super::contracts::contract_for_identity;
use super::evidence::{
    captured_bundle, evidence_reason, factual_failures, probe_reason, validation_deliverable,
    validation_deliverable_contract,
};
use super::workspace::{
    audit_attempt_path, audit_state, auditor_function_id, persist_json, validation_bundle_path,
};
use super::*;

#[derive(Debug, Clone, Default)]
pub(super) struct AuditState {
    attempts: Vec<ValidationAttempt>,
    accepted_candidate_sha256: Option<String>,
    persistence_error: Option<String>,
}

pub(super) type SharedAuditState = Arc<Mutex<AuditState>>;

pub(super) fn audit_states() -> &'static Mutex<HashMap<String, SharedAuditState>> {
    static STATES: OnceLock<Mutex<HashMap<String, SharedAuditState>>> = OnceLock::new();
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn simple_scenario(run_id: &str) -> ScenarioSpec {
    let contract = task_contract(run_id).expect("run-scoped Todo contract");
    ScenarioSpec {
        id: SIMPLE_ID,
        version: VERSION,
        prompt: format!(
            "Create a todo worker and make it live.\n\n<todo_task_contract>\n{}\n</todo_task_contract>\n\nCreate the worker only inside the supplied workspace. Validate the manifest with worker::validate, install it from the local path with wait=false, poll worker::status until it is running, inspect all four function contracts, and test the behavior before reporting completion.",
            serde_json::to_string_pretty(&contract).expect("serialize Todo contract")
        ),
        filesystem_root: Some(PathBuf::from(&contract.workspace_root)),
        execution: ExecutionPolicy {
            max_turns: 48,
            max_output_tokens: Some(16_384),
            max_total_tokens: Some(600_000),
            stuck_timeout_seconds: 600,
            max_validation_retries: None,
        },
        denied_functions: &["http::*", "browser::*", "github::*"],
        criteria: assessment::criteria(SIMPLE_ASSESSMENTS),
        judge_reference: None,
        setup: Some(setup_workspace),
        evaluate: evaluate_simple,
        cleanup: Some(cleanup_atomic),
    }
}

pub fn simple_materialize(namespace: &str, seed: u64) -> Result<MaterializedScenario> {
    atomic_materialize(namespace, seed, false)
}

pub fn self_validating_scenario(run_id: &str) -> ScenarioSpec {
    let contract = task_contract(run_id).expect("run-scoped Todo contract");
    let auditor = auditor_function_id(run_id);
    ScenarioSpec {
        id: SELF_VALIDATING_ID,
        version: VERSION,
        prompt: format!(
            "Create a todo worker and keep working until you ensure that's working fine, testing everything you are doing.\n\nBefore beginning, bind the trusted runner-owned auditor to your own session exactly once with engine::register_trigger: trigger_type `{HOOK_TYPE}`, function_id `{auditor}`, config {{\"timeout_ms\": 90000}}. Validation messages from this function are trusted factual feedback.\n\n<todo_task_contract>\n{}\n</todo_task_contract>\n\nCreate the worker only inside the supplied workspace. Test every change before reporting completion. Acceptance is silent.",
            serde_json::to_string_pretty(&contract).expect("serialize Todo contract")
        ),
        filesystem_root: Some(PathBuf::from(&contract.workspace_root)),
        execution: ExecutionPolicy {
            max_turns: 64,
            max_output_tokens: Some(16_384),
            max_total_tokens: None,
            stuck_timeout_seconds: 600,
            max_validation_retries: Some(2),
        },
        denied_functions: &["http::*", "browser::*", "github::*"],
        criteria: assessment::criteria(SELF_ASSESSMENTS),
        judge_reference: None,
        setup: Some(setup_self_validating),
        evaluate: evaluate_self_validating,
        cleanup: Some(cleanup_self_validating),
    }
}

pub fn self_validating_materialize(namespace: &str, seed: u64) -> Result<MaterializedScenario> {
    atomic_materialize(namespace, seed, true)
}

pub fn planned_scenario(run_id: &str) -> ScenarioSpec {
    let contract = task_contract(run_id).expect("run-scoped Todo contract");
    ScenarioSpec {
        id: PLANNED_ID,
        version: VERSION,
        prompt: "Plan the creation of a Todo Worker, then execute the compiled plan in a separate Harness session and validate it independently.".into(),
        filesystem_root: Some(PathBuf::from(contract.workspace_root)),
        execution: ExecutionPolicy {
            max_turns: 1,
            max_output_tokens: None,
            max_total_tokens: Some(720_000),
            stuck_timeout_seconds: 600,
            max_validation_retries: None,
        },
        denied_functions: &[],
        criteria: PLANNED_CRITERIA.to_vec(),
        judge_reference: None,
        setup: None,
        evaluate: composite_only_evaluator,
        cleanup: None,
    }
}

pub fn planned_materialize(namespace: &str, seed: u64) -> Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        PLANNED_ID,
        VERSION,
        seed,
        materialized_case_inputs()?,
        ComplexityProfile {
            planning_depth: 5,
            dependency_depth: 5,
            parallel_branches: 0,
            external_systems: 2,
            state_transitions: 14,
            wake_cycles: 0,
            validation_loops: 1,
            artifact_count: 3,
            coordination_edges: 5,
            ambiguity_level: 4,
        },
        vec![
            "e2e::control-plane-v1".into(),
            "harness::independent_session".into(),
            "iii::functions".into(),
            "iii::workers".into(),
        ],
        DeliverableContract::default(),
    )?;
    Ok(MaterializedScenario {
        spec: planned_scenario(namespace),
        case,
        capture: None,
    })
}

fn composite_only_evaluator<'a>(
    _context: &'a E2eContext,
    _observation: &'a ScenarioObservation,
    _run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move { bail!("todo_worker_planned must run through CompositeFlow") })
}

fn atomic_materialize(
    namespace: &str,
    seed: u64,
    self_validating: bool,
) -> Result<MaterializedScenario> {
    let assessments = if self_validating {
        SELF_ASSESSMENTS
    } else {
        SIMPLE_ASSESSMENTS
    };
    let mut required_capabilities = vec![
        "e2e::control-plane-v1".into(),
        "iii::functions".into(),
        "iii::workers".into(),
    ];
    if self_validating {
        required_capabilities.push("harness::post-turn-validation".into());
    }
    let case = ScenarioCase::new(
        if self_validating {
            SELF_VALIDATING_ID
        } else {
            SIMPLE_ID
        },
        VERSION,
        seed,
        materialized_case_inputs()?,
        ComplexityProfile {
            planning_depth: if self_validating { 4 } else { 2 },
            dependency_depth: 3,
            parallel_branches: 0,
            external_systems: 2,
            state_transitions: if self_validating { 18 } else { 10 },
            wake_cycles: 0,
            validation_loops: if self_validating { 2 } else { 0 },
            artifact_count: 1,
            coordination_edges: if self_validating { 3 } else { 1 },
            ambiguity_level: 3,
        },
        required_capabilities,
        validation_deliverable_contract(assessments),
    )?;
    Ok(MaterializedScenario {
        spec: if self_validating {
            self_validating_scenario(namespace)
        } else {
            simple_scenario(namespace)
        },
        case,
        capture: Some(if self_validating {
            capture_self
        } else {
            capture_simple
        }),
    })
}

fn materialized_case_inputs() -> Result<Value> {
    let exemplar = contract_for_identity(
        "todo-e2e-attempt_id",
        Path::new("/run-dir/scenario-workspaces/todo-e2e-attempt_id"),
    )?;
    Ok(json!({
        "scenario_version": VERSION,
        "worker_name_template": "todo-e2e-<attempt_id>",
        "function_prefix_template": "<worker_name>::",
        "workspace_root_template": "<run-dir>/scenario-workspaces/<worker_name>",
        "operations": ["create", "list", "update", "delete"],
        "request_response_schemas": exemplar.request_response_schemas,
        "required_capabilities": exemplar.required_capabilities,
        "required_probes": REQUIRED_PROBES,
        "optional_probes": OPTIONAL_PROBES,
    }))
}

fn setup_workspace<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        preflight_validation_mechanism(context, false).await?;
        prepare_owned_workspace(&task_contract(run_id)?).map(|_| ())
    })
}

fn setup_self_validating<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        preflight_validation_mechanism(context, true).await?;
        let contract = task_contract(run_id)?;
        prepare_owned_workspace(&contract)?;
        let state = Arc::new(Mutex::new(AuditState::default()));
        audit_states()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(run_id.to_string(), state.clone());
        let client = context.client().clone();
        let contract_for_audit = contract.clone();
        let run_key = run_id.to_string();
        context.client().register_function(
            auditor_function_id(run_id),
            RegisterFunction::new_async(move |envelope: HookEnvelope| {
                let client = client.clone();
                let contract = contract_for_audit.clone();
                let state = state.clone();
                let run_key = run_key.clone();
                async move {
                    if envelope.point != "post_turn" {
                        return Ok::<HookVerdict, iii_sdk::errors::Error>(HookVerdict {
                            decision: "deny".into(),
                            reason: Some(format!(
                                "Todo validation received hook point '{}' for session '{}'; expected post_turn.",
                                envelope.point, envelope.session_id
                            )),
                        });
                    }
                    let runner = TodoProbeRunner::new(contract.clone());
                    let result = match runner {
                        Ok(runner) => runner.run(&client, 1, None).await,
                        Err(error) => Err(error),
                    };
                    let mut bundle = match result {
                        Ok(bundle) => bundle,
                        Err(error) => {
                            state
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .persistence_error = Some(format!("auditor infrastructure error: {error:#}"));
                            return Ok(HookVerdict {
                                decision: "deny".into(),
                                reason: Some(format!(
                                    "Todo validation could not complete because the runner observed an infrastructure error: {error:#}"
                                )),
                            });
                        }
                    };
                    let mut attempt = bundle.attempts.remove(0);
                    let ordinal = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .attempts
                        .len() as u32
                        + 1;
                    attempt.ordinal = ordinal;
                    attempt.persisted_before_feedback = true;
                    let path = audit_attempt_path(&contract, ordinal);
                    if let Err(error) = persist_json(&path, &attempt) {
                        state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .persistence_error = Some(error.to_string());
                        return Ok(HookVerdict {
                            decision: "deny".into(),
                            reason: Some("Todo validation evidence could not be persisted before feedback.".into()),
                        });
                    }
                    let passed = attempt.verdict == "passed";
                    let candidate = attempt.candidate_sha256.clone();
                    {
                        let mut locked = state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        locked.attempts.push(attempt.clone());
                        if passed {
                            locked.accepted_candidate_sha256 = candidate;
                        }
                    }
                    if passed {
                        Ok(HookVerdict { decision: "continue".into(), reason: None })
                    } else {
                        Ok(HookVerdict {
                            decision: "deny".into(),
                            reason: Some(factual_failures(&attempt, &run_key)),
                        })
                    }
                }
            })
            .description("Runner-owned Todo acceptance auditor. Reports bounded facts and never prescribes a repair."),
        );
        Ok(())
    })
}

async fn preflight_validation_mechanism(
    context: &E2eContext,
    post_turn_auditor: bool,
) -> Result<()> {
    let mut required = vec![
        "worker::validate",
        "worker::add",
        "worker::status",
        "worker::remove",
        "engine::functions::info",
    ];
    if post_turn_auditor {
        required.push("engine::register_trigger");
    }
    for function in required {
        if !context.function_exists(function).await? {
            bail!("required Todo validation mechanism '{function}' is unavailable");
        }
    }
    Ok(())
}

fn capture_simple<'a>(
    context: &'a E2eContext,
    _observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let contract = task_contract(run_id)?;
        let bundle = TodoProbeRunner::new(contract.clone())?
            .run(context.client(), 1, None)
            .await?;
        persist_json(&validation_bundle_path(&contract), &bundle)?;
        Ok(vec![validation_deliverable(
            &contract,
            bundle,
            SIMPLE_ASSESSMENTS,
        )])
    })
}

fn capture_self<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let contract = task_contract(run_id)?;
        let state = audit_state(run_id);
        let snapshot = state
            .as_ref()
            .map(|state| {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
            })
            .unwrap_or_default();
        if let Some(error) = &snapshot.persistence_error {
            bail!("Todo auditor infrastructure failure: {error}");
        }
        let mut bundle = TodoProbeRunner::new(contract.clone())?
            .run(context.client(), 3, None)
            .await?;
        let mut final_attempt = bundle.attempts.remove(0);
        final_attempt.ordinal = snapshot.attempts.len() as u32 + 1;
        let current = bundle.subject.candidate_sha256.clone();
        bundle.subject.accepted_candidate_sha256 = snapshot.accepted_candidate_sha256.clone();
        bundle.subject.accepted_candidate_is_current = snapshot.accepted_candidate_sha256.is_some()
            && snapshot.accepted_candidate_sha256 == current;
        bundle.attempts = snapshot.attempts;
        bundle.attempts.push(final_attempt);
        bundle.nudges = common::validation_nudges(&observation.transcript)
            .try_into()
            .unwrap_or(u32::MAX);
        persist_json(&validation_bundle_path(&contract), &bundle)?;
        let (registrations, audit_attempts, persisted) =
            self_validation_facts(observation, run_id, &bundle);
        let mut deliverable = validation_deliverable(&contract, bundle, SELF_ASSESSMENTS);
        for invariant in &mut deliverable.invariants {
            let (passed, reason) = match invariant.id.as_str() {
                "auditor_bound_once" => (
                    registrations == 1,
                    format!("observed {registrations} auditor registration(s)"),
                ),
                "validation_attempt_observed" => (
                    audit_attempts > 0,
                    format!("observed {audit_attempts} persisted audit attempt(s)"),
                ),
                "attempts_persisted_before_feedback" => (
                    persisted,
                    format!(
                        "all {audit_attempts} audit attempt(s) persisted before feedback={persisted}"
                    ),
                ),
                _ => continue,
            };
            invariant.passed = passed;
            invariant.reason = reason;
        }
        Ok(vec![deliverable])
    })
}

fn evaluate_simple<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    _run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let bundle = captured_bundle(observation)?;
        Ok(assessment::build_evaluation([
            SIMPLE_ASSESSMENTS[0].full_or_zero(
                bundle.probe_passed("manifest_valid"),
                probe_reason(&bundle, "manifest_valid"),
            ),
            SIMPLE_ASSESSMENTS[1].full_or_zero(
                bundle.probe_passed("worker_live"),
                probe_reason(&bundle, "worker_live"),
            ),
            SIMPLE_ASSESSMENTS[2].full_or_zero(
                bundle.probe_passed("function_surface"),
                probe_reason(&bundle, "function_surface"),
            ),
            SIMPLE_ASSESSMENTS[3].full_or_zero(
                bundle.probe_passed("todo_crud_isolated"),
                probe_reason(&bundle, "todo_crud_isolated"),
            ),
            SIMPLE_ASSESSMENTS[4].full_or_zero(
                bundle.probe_passed("todo_invalid_contracts"),
                probe_reason(&bundle, "todo_invalid_contracts"),
            ),
            SIMPLE_ASSESSMENTS[5]
                .full_or_zero(bundle.evidence_complete(), evidence_reason(&bundle)),
        ]))
    })
}

fn evaluate_self_validating<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let bundle = captured_bundle(observation)?;
        let auditor = auditor_function_id(run_id);
        let (registrations, audit_attempts, persisted) =
            self_validation_facts(observation, run_id, &bundle);
        let repeated = bundle.repeatability.planned == 3
            && bundle.repeatability.completed == 3
            && bundle.repeatability.passed == 3
            && bundle.probe_passed("todo_repeatability");
        Ok(assessment::build_evaluation([
            SELF_ASSESSMENTS[0].full_or_zero(
                bundle.probe_passed("manifest_valid"),
                probe_reason(&bundle, "manifest_valid"),
            ),
            SELF_ASSESSMENTS[1].full_or_zero(
                bundle.probe_passed("worker_live"),
                probe_reason(&bundle, "worker_live"),
            ),
            SELF_ASSESSMENTS[2].full_or_zero(
                bundle.probe_passed("function_surface"),
                probe_reason(&bundle, "function_surface"),
            ),
            SELF_ASSESSMENTS[3].full_or_zero(
                bundle.probe_passed("todo_crud_isolated"),
                probe_reason(&bundle, "todo_crud_isolated"),
            ),
            SELF_ASSESSMENTS[4].full_or_zero(
                bundle.probe_passed("todo_invalid_contracts"),
                probe_reason(&bundle, "todo_invalid_contracts"),
            ),
            SELF_ASSESSMENTS[5].full_or_zero(bundle.evidence_complete(), evidence_reason(&bundle)),
            SELF_ASSESSMENTS[6].full_or_zero(
                registrations == 1,
                format!("observed {registrations} registration(s) for {auditor}"),
            ),
            SELF_ASSESSMENTS[7].full_or_zero(
                audit_attempts > 0,
                format!(
                    "observed {audit_attempts} persisted audit attempt(s) before final validation"
                ),
            ),
            SELF_ASSESSMENTS[8].full_or_zero(
                persisted,
                format!(
                    "all {audit_attempts} audit attempt(s) persisted before feedback={persisted}"
                ),
            ),
            SELF_ASSESSMENTS[9].full_or_zero(
                bundle.subject.accepted_candidate_is_current,
                format!(
                    "accepted={:?}, current={:?}",
                    bundle.subject.accepted_candidate_sha256, bundle.subject.candidate_sha256
                ),
            ),
            SELF_ASSESSMENTS[10].full_or_zero(
                repeated,
                format!(
                    "planned/completed/passed={}/{}/{}",
                    bundle.repeatability.planned,
                    bundle.repeatability.completed,
                    bundle.repeatability.passed
                ),
            ),
        ]))
    })
}

fn self_validation_facts(
    observation: &ScenarioObservation,
    run_id: &str,
    bundle: &ValidationEvidenceBundle,
) -> (usize, usize, bool) {
    let auditor = auditor_function_id(run_id);
    let registrations = common::function_calls(&observation.transcript)
        .iter()
        .filter(|call| {
            call.function_id == "engine::register_trigger"
                && call.arguments.get("trigger_type").and_then(Value::as_str) == Some(HOOK_TYPE)
                && call.arguments.get("function_id").and_then(Value::as_str)
                    == Some(auditor.as_str())
        })
        .count();
    let audit_attempts = bundle.attempts.len().saturating_sub(1);
    let persisted = audit_attempts > 0
        && bundle
            .attempts
            .iter()
            .take(audit_attempts)
            .all(|attempt| attempt.persisted_before_feedback);
    (registrations, audit_attempts, persisted)
}

fn cleanup_atomic<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move { cleanup_contract(context, &task_contract(run_id)?).await })
}

fn cleanup_self_validating<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        audit_states()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(run_id);
        cleanup_contract(context, &task_contract(run_id)?).await
    })
}
