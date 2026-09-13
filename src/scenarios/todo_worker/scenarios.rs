use super::contracts::contract_for_identity;
use super::evidence::{
    captured_bundle, evidence_reason, probe_reason, validation_deliverable,
    validation_deliverable_contract,
};
use super::workspace::{persist_json, validation_bundle_path};
use super::*;

/// The single-session Todo Worker build, validated by an independent probe run.
pub struct TodoWorkerSimple;

impl Scenario for TodoWorkerSimple {
    fn id(&self) -> &'static str {
        SIMPLE_ID
    }

    fn characterization(&self) -> Result<ScenarioCharacterization> {
        Ok(ScenarioCharacterization::realistic())
    }

    fn case(&self, seed: u64) -> Result<ScenarioCase> {
        ScenarioCase::new(
            SIMPLE_ID,
            seed,
            materialized_case_inputs()?,
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiCompose,
                Capability::IiiFunctions,
                Capability::IiiWorkers,
            ],
            validation_deliverable_contract(SIMPLE_ASSESSMENTS),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        simple_scenario(run_id)
    }

    fn setup<'a>(&'a self, context: &'a E2eContext, run_id: &'a str) -> Option<CleanupFuture<'a>> {
        Some(setup_workspace(context, run_id))
    }

    fn capture<'a>(
        &'a self,
        context: &'a E2eContext,
        observation: &'a ScenarioObservation,
        run_id: &'a str,
    ) -> Option<DeliverableCaptureFuture<'a>> {
        Some(capture_simple(context, observation, run_id))
    }

    fn evaluate<'a>(
        &'a self,
        context: &'a E2eContext,
        observation: &'a ScenarioObservation,
        run_id: &'a str,
    ) -> EvaluationFuture<'a> {
        evaluate_simple(context, observation, run_id)
    }

    fn cleanup<'a>(
        &'a self,
        context: &'a E2eContext,
        run_id: &'a str,
    ) -> Option<CleanupFuture<'a>> {
        Some(cleanup_atomic(context, run_id))
    }
}

/// The planned build: one planner session, one separate builder, one independent validator.
pub struct TodoWorkerPlanned;

impl Scenario for TodoWorkerPlanned {
    fn id(&self) -> &'static str {
        PLANNED_ID
    }

    fn execution_kind(&self) -> ScenarioExecutionKind {
        ScenarioExecutionKind::CompositeFlow
    }

    fn characterization(&self) -> Result<ScenarioCharacterization> {
        Ok(ScenarioCharacterization::realistic())
    }

    fn case(&self, seed: u64) -> Result<ScenarioCase> {
        ScenarioCase::new(
            PLANNED_ID,
            seed,
            materialized_case_inputs()?,
            vec![
                Capability::E2eControlPlaneV1,
                Capability::HarnessIndependentSession,
                Capability::IiiCompose,
                Capability::IiiFunctions,
                Capability::IiiWorkers,
            ],
            DeliverableContract::default(),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        planned_scenario(run_id)
    }

    fn evaluate<'a>(
        &'a self,
        context: &'a E2eContext,
        observation: &'a ScenarioObservation,
        run_id: &'a str,
    ) -> EvaluationFuture<'a> {
        composite_only_evaluator(context, observation, run_id)
    }
}

fn simple_scenario(run_id: &str) -> ScenarioSpec {
    let contract = task_contract(run_id).expect("run-scoped Todo contract");
    ScenarioSpec {
        id: SIMPLE_ID,
        prompt: format!(
            "Create a todo worker and make it live.\n\n<todo_task_contract>\n{}\n</todo_task_contract>\n\nCreate the worker only inside the supplied workspace. Declare it in the root worker-compose.yaml, validate with compose::validate, start its local stack with compose::up and wait=false, poll worker::status until it is running, inspect all four function contracts, and test the behavior before reporting completion.",
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
    }
}

fn planned_scenario(run_id: &str) -> ScenarioSpec {
    let contract = task_contract(run_id).expect("run-scoped Todo contract");
    ScenarioSpec {
        id: PLANNED_ID,
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
    }
}

fn composite_only_evaluator<'a>(
    _context: &'a E2eContext,
    _observation: &'a ScenarioObservation,
    _run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move { bail!("todo_worker_planned must run through CompositeFlow") })
}

fn materialized_case_inputs() -> Result<Value> {
    let exemplar = contract_for_identity(
        "todo-e2e-attempt_id",
        Path::new("/run-dir/scenario-workspaces/todo-e2e-attempt_id"),
    )?;
    Ok(json!({
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

async fn preflight_validation_mechanism(
    context: &E2eContext,
    post_turn_auditor: bool,
) -> Result<()> {
    let mut required = vec![
        "compose::validate",
        "compose::up",
        "compose::down",
        "worker::status",
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

fn evaluate_simple<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    _run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let bundle = captured_bundle(observation)?;
        Ok(assessment::build_evaluation(
            if bundle.subject.candidate_sha256.is_some() {
                crate::report::CompletionState::Completed
            } else {
                crate::report::CompletionState::TaskIncomplete
            },
            [
                SIMPLE_ASSESSMENTS[0].full_or_zero(
                    bundle.probe_passed("compose_valid"),
                    probe_reason(&bundle, "compose_valid"),
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
            ],
        ))
    })
}

fn cleanup_atomic<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move { cleanup_contract(context, &task_contract(run_id)?).await })
}
