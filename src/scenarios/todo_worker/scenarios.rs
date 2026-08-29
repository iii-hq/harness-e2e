use super::contracts::contract_for_identity;
use super::evidence::{
    captured_bundle, evidence_reason, probe_reason, validation_deliverable,
    validation_deliverable_contract,
};
use super::workspace::{persist_json, validation_bundle_path};
use super::*;

pub fn simple_scenario(run_id: &str) -> ScenarioSpec {
    let contract = task_contract(run_id).expect("run-scoped Todo contract");
    ScenarioSpec {
        id: SIMPLE_ID,
        version: VERSION,
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
        judge_reference: None,
        setup: Some(setup_workspace),
        evaluate: evaluate_simple,
        cleanup: Some(cleanup_atomic),
    }
}

pub fn simple_materialize(namespace: &str, seed: u64) -> Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        SIMPLE_ID,
        VERSION,
        seed,
        materialized_case_inputs()?,
        ComplexityProfile {
            planning_depth: 2,
            dependency_depth: 3,
            parallel_branches: 0,
            external_systems: 2,
            state_transitions: 10,
            wake_cycles: 0,
            validation_loops: 0,
            artifact_count: 1,
            coordination_edges: 1,
            ambiguity_level: 3,
            agent_owned_decomposition: false,
            material_invalidation_events: 0,
            replan_loops: 0,
            compensable_mutations: 0,
            durable_resume_cycles: 0,
            coherent_long_horizon: false,
        },
        vec![
            "e2e::control-plane-v1".into(),
            "iii::compose".into(),
            "iii::functions".into(),
            "iii::workers".into(),
        ],
        validation_deliverable_contract(SIMPLE_ASSESSMENTS),
    )?;
    Ok(MaterializedScenario {
        spec: simple_scenario(namespace),
        case,
        capture: Some(capture_simple),
    })
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
            agent_owned_decomposition: false,
            material_invalidation_events: 0,
            replan_loops: 0,
            compensable_mutations: 0,
            durable_resume_cycles: 0,
            coherent_long_horizon: false,
        },
        vec![
            "e2e::control-plane-v1".into(),
            "harness::independent_session".into(),
            "iii::compose".into(),
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
        Ok(assessment::build_evaluation([
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
        ]))
    })
}

fn cleanup_atomic<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move { cleanup_contract(context, &task_contract(run_id)?).await })
}
