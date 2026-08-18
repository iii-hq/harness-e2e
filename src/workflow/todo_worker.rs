//! Five-node plan/compile/build/validate workflow for the Todo Worker scenario.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::todo_worker::{
    self, CompiledValidationPlan, TodoProbeRunner, TodoTaskContract, ValidationEvidenceBundle,
    PLANNED_CRITERIA, PLANNED_ID, RAW_PLAN_FILE, VERSION,
};

use super::{
    CapturedWorkflowAsset, ControlSource, DependencyPolicy, HarnessStepPolicy, PortValueKind,
    ReplayPolicy, StepCatalog, StepEvaluation, StepExecutor, StepExecutorContext,
    StepExecutorOutput, StepOperationalKind, StepPortDescriptor, StepTypeDescriptor,
    TypedPortValue, WorkflowAssetContent, WorkflowCleanupContext, WorkflowCleanupHook,
    WorkflowCriterionDeclaration, WorkflowDefinitionV1, WorkflowEvaluationOutcome,
    WorkflowEvaluationResult, WorkflowGateResult, WorkflowInputBinding, WorkflowLimits,
    WorkflowNodeV1, WorkflowProvenance,
};

const PREPARE_STEP: &str = "todo_worker.prepare_workspace";
const COMPILE_STEP: &str = "todo_worker.compile_validation_plan";
const VALIDATE_STEP: &str = "todo_worker.validate";
const RAW_PLAN_ASSET: &str = "validation_plan_raw";
const RAW_PLAN_EVIDENCE: &str = "compile_validation_plan.validation_plan_raw";
const COMPILED_PLAN_ASSET: &str = "validation_plan_compiled";
const COMPILED_PLAN_EVIDENCE: &str = "compile_validation_plan.validation_plan_compiled";
const VALIDATION_ASSET_ID: &str = "todo_validation_evidence";
pub const VALIDATION_ASSET: &str = "validate_todo_worker.todo_validation_evidence";

const PLANNER_PROMPT: &str = r#"Planeje a criação de um Todo Worker para o contrato fornecido.

Não implemente o worker e não instale nada. Escreva exatamente um arquivo validation-plan.json na raiz do workspace. O arquivo deve ser JSON estrito com scenario_version=1, task_contract_sha256, summary, implementation_tasks (1..8) e validation_checks (1..12). Cada task contém apenas id, objective e completion_signal. Cada check contém id, probe_id, rationale e, somente quando aplicável, repetitions ou concurrency.

O catálogo fechado de probes é: manifest_valid, worker_live, function_surface, todo_crud_isolated, todo_invalid_contracts, todo_repeatability e todo_concurrent_create. Os cinco primeiros são obrigatórios. todo_repeatability aceita repetitions de 1 a 3. todo_concurrent_create aceita concurrency de 1 a 5. Não escreva shell, SQL, function ids ou expressões executáveis como probes. Revise o JSON contra o contrato antes de concluir."#;

const BUILDER_PROMPT: &str = r#"Execute o plano compilado fornecido e construa o Todo Worker.

O plano compilado e seu task_contract são autoritativos. Trabalhe somente no workspace_root indicado. Preserve exatamente worker_name, function_ids e request_response_schemas. O iii.worker.yaml deve declarar o nome run-scoped, scripts.start explícito e não pode declarar scripts.setup. Você pode escolher linguagem e armazenamento. Valide o manifest, instale a origem local com wait=false, acompanhe worker::status e teste o comportamento antes de concluir. Não modifique o plano compilado."#;

pub fn definition() -> WorkflowDefinitionV1 {
    WorkflowDefinitionV1 {
        schema_version: super::WORKFLOW_SCHEMA_VERSION,
        id: PLANNED_ID.into(),
        scenario_version: VERSION,
        description: "Plan a run-scoped Todo Worker, compile its closed validation plan, build it in a separate Harness session, and execute every compiled hard gate independently.".into(),
        limits: WorkflowLimits {
            max_parallel: 1,
            max_nodes: 5,
            step_timeout_seconds: 600,
            workflow_timeout_seconds: 1_800,
            max_total_tokens: None,
            max_cost_usd: Some(30.0),
            technical_retries: 0,
        },
        nodes: vec![
            node("prepare_workspace", PREPARE_STEP, &[], BTreeMap::new()),
            WorkflowNodeV1 {
                id: "plan_todo_worker".into(),
                step_type: super::HARNESS_STEP_ID.into(),
                step_version: super::HARNESS_STEP_VERSION_V2,
                config: harness_config(PLANNER_PROMPT, 12, None, true),
                depends_on: vec!["prepare_workspace".into()],
                inputs: BTreeMap::from([
                    (
                        "data".into(),
                        output("prepare_workspace", "task_contract"),
                    ),
                    (
                        "workspace_root".into(),
                        output("prepare_workspace", "workspace_root"),
                    ),
                ]),
                activation: Default::default(),
                dependency_policy: DependencyPolicy::Succeeded,
                required: true,
            },
            WorkflowNodeV1 {
                inputs: BTreeMap::from([
                    (
                        "task_contract".into(),
                        output("prepare_workspace", "task_contract"),
                    ),
                    (
                        "workspace_root".into(),
                        output("prepare_workspace", "workspace_root"),
                    ),
                ]),
                ..node(
                    "compile_validation_plan",
                    COMPILE_STEP,
                    &["plan_todo_worker"],
                    BTreeMap::new(),
                )
            },
            WorkflowNodeV1 {
                id: "build_todo_worker".into(),
                step_type: super::HARNESS_STEP_ID.into(),
                step_version: super::HARNESS_STEP_VERSION_V2,
                config: harness_config(BUILDER_PROMPT, 48, None, false),
                depends_on: vec!["compile_validation_plan".into()],
                inputs: BTreeMap::from([
                    (
                        "data".into(),
                        output("compile_validation_plan", "compiled_plan"),
                    ),
                    (
                        "workspace_root".into(),
                        output("prepare_workspace", "workspace_root"),
                    ),
                ]),
                activation: Default::default(),
                dependency_policy: DependencyPolicy::Succeeded,
                required: true,
            },
            WorkflowNodeV1 {
                inputs: BTreeMap::from([(
                    "compiled_plan".into(),
                    output("compile_validation_plan", "compiled_plan"),
                )]),
                ..node(
                    "validate_todo_worker",
                    VALIDATE_STEP,
                    &["build_todo_worker"],
                    BTreeMap::new(),
                )
            },
        ],
        criteria: vec![
            criterion(
                PLANNED_CRITERIA[0].id,
                PLANNED_CRITERIA[0].weight,
                "compile_validation_plan",
                "planning_assessment",
            ),
            criterion(
                PLANNED_CRITERIA[1].id,
                PLANNED_CRITERIA[1].weight,
                "validate_todo_worker",
                "construction_assessment",
            ),
            criterion(
                PLANNED_CRITERIA[2].id,
                PLANNED_CRITERIA[2].weight,
                "validate_todo_worker",
                "coverage_assessment",
            ),
            criterion(
                PLANNED_CRITERIA[3].id,
                PLANNED_CRITERIA[3].weight,
                "validate_todo_worker",
                "functional_assessment",
            ),
        ],
    }
}

pub fn descriptors_only() -> Result<Vec<StepTypeDescriptor>> {
    descriptors()
}

pub fn harness_policy() -> Result<HarnessStepPolicy> {
    HarnessStepPolicy::new(
        [todo_worker::harness_workspace_base()?],
        ["harness::*".to_string(), "e2e::*".to_string()],
    )
}

pub fn register_steps(
    catalog: &mut StepCatalog,
    context: Arc<E2eContext>,
) -> Result<Arc<dyn WorkflowCleanupHook>> {
    for (descriptor, kind) in descriptor_kinds()? {
        catalog.register(
            descriptor,
            Arc::new(TodoExecutor {
                context: context.clone(),
                kind,
            }),
        )?;
    }
    Ok(Arc::new(TodoCleanup { context }))
}

fn descriptors() -> Result<Vec<StepTypeDescriptor>> {
    Ok(descriptor_kinds()?
        .into_iter()
        .map(|(descriptor, _)| descriptor)
        .collect())
}

fn descriptor_kinds() -> Result<Vec<(StepTypeDescriptor, TodoStepKind)>> {
    Ok(vec![
        (
            descriptor(
                PREPARE_STEP,
                "Create the attempt-owned Todo workspace and immutable task contract.",
                BTreeMap::new(),
                ports(&[
                    ("workspace_root", PortValueKind::TextUtf8, false, None),
                    ("task_contract", PortValueKind::Json, false, None),
                ]),
                ReplayPolicy::Idempotent,
                StepOperationalKind::Transformation,
            )?,
            TodoStepKind::Prepare,
        ),
        (
            descriptor(
                COMPILE_STEP,
                "Compile the planner file against the fixed probe catalog and emit deterministic build authorization.",
                ports(&[
                    ("workspace_root", PortValueKind::TextUtf8, false, None),
                    ("task_contract", PortValueKind::Json, false, None),
                ]),
                ports(&[
                    ("compiled_plan", PortValueKind::Json, false, None),
                    ("ready_for_build", PortValueKind::Boolean, false, Some(ControlSource::Deterministic)),
                    ("planning_assessment", PortValueKind::Assessment, false, None),
                ]),
                ReplayPolicy::Idempotent,
                StepOperationalKind::Transformation,
            )?,
            TodoStepKind::Compile,
        ),
        (
            descriptor(
                VALIDATE_STEP,
                "Install and independently validate every compiled Todo check, emitting immutable evidence and hard gates.",
                ports(&[("compiled_plan", PortValueKind::Json, false, None)]),
                ports(&[
                    ("construction_assessment", PortValueKind::Assessment, false, None),
                    ("coverage_assessment", PortValueKind::Assessment, false, None),
                    ("functional_assessment", PortValueKind::Assessment, false, None),
                    ("validation_bundle", PortValueKind::Json, false, None),
                ]),
                ReplayPolicy::Compensable,
                StepOperationalKind::Assessment,
            )?,
            TodoStepKind::Validate,
        ),
    ])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TodoStepKind {
    Prepare,
    Compile,
    Validate,
}

struct TodoExecutor {
    context: Arc<E2eContext>,
    kind: TodoStepKind,
}

#[async_trait]
impl StepExecutor for TodoExecutor {
    async fn preflight(&self, _context: &StepExecutorContext) -> Result<()> {
        if self.kind == TodoStepKind::Validate {
            for function in [
                "worker::validate",
                "worker::add",
                "worker::status",
                "worker::remove",
                "engine::functions::info",
            ] {
                if !self.context.function_exists(function).await? {
                    bail!("required Todo validation mechanism '{function}' is unavailable");
                }
            }
        }
        Ok(())
    }

    async fn execute(&self, context: StepExecutorContext) -> Result<StepExecutorOutput> {
        match self.kind {
            TodoStepKind::Prepare => execute_prepare(&context),
            TodoStepKind::Compile => execute_compile(&context),
            TodoStepKind::Validate => execute_validate(self.context.as_ref(), &context).await,
        }
    }

    async fn evaluate(
        &self,
        _context: &StepExecutorContext,
        execution: &StepExecutorOutput,
        _assets: &[CapturedWorkflowAsset],
    ) -> Result<StepEvaluation> {
        Ok(execution.evaluation.clone())
    }
}

fn execute_prepare(context: &StepExecutorContext) -> Result<StepExecutorOutput> {
    let contract = todo_worker::task_contract(&context.attempt_id)?;
    let root = todo_worker::prepare_owned_workspace(&contract)?;
    let contract_value = serde_json::to_value(&contract)?;
    Ok(StepExecutorOutput {
        outputs: BTreeMap::from([
            ("workspace_root".into(), text_value(root.to_string_lossy())),
            ("task_contract".into(), json_value(contract_value)),
        ]),
        evaluation: StepEvaluation {
            hard_gates: vec![gate(
                "todo_workspace_prepared",
                true,
                format!("prepared owned workspace for {}", contract.worker_name),
                [],
            )],
            evaluations: Vec::new(),
        },
        ..StepExecutorOutput::default()
    })
}

fn execute_compile(context: &StepExecutorContext) -> Result<StepExecutorOutput> {
    let contract: TodoTaskContract = input_json(context, "task_contract")?;
    let workspace = input_text(context, "workspace_root")?;
    if workspace != contract.workspace_root {
        bail!("compiler workspace differs from the immutable Todo contract");
    }
    let raw_path = Path::new(workspace).join(RAW_PLAN_FILE);
    let raw = match fs::read(&raw_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("read {}", raw_path.display())),
    };
    let compiled = todo_worker::compile_validation_plan(raw.as_deref(), &contract)?;
    let compiled_value = serde_json::to_value(&compiled)?;
    let raw_value = raw
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        .unwrap_or_else(|| {
            json!({
                "present": raw.is_some(),
                "raw_plan_sha256": compiled.raw_plan_sha256,
                "utf8_preview": raw.as_deref().map(|bytes| String::from_utf8_lossy(bytes).chars().take(4096).collect::<String>())
            })
        });
    let planning = evaluation(
        PLANNED_CRITERIA[0].id,
        compiled.ready_for_build,
        if compiled.ready_for_build {
            format!(
                "plan compiled with {} task(s) and {} hard-gated check(s)",
                compiled.implementation_tasks.len(),
                compiled.compiled_checks.len()
            )
        } else {
            format!("plan rejected: {:?}", compiled.diagnostics)
        },
        [COMPILED_PLAN_EVIDENCE.into()],
    );
    let gates = [
        "plan_present",
        "plan_schema_valid",
        "plan_compilable",
        "plan_coverage_complete",
    ]
    .into_iter()
    .map(|id| {
        gate(
            id,
            compiled.planning_gate(id),
            format!("compiler diagnostics={:?}", compiled.diagnostics),
            [RAW_PLAN_EVIDENCE.into(), COMPILED_PLAN_EVIDENCE.into()],
        )
    })
    .collect();
    Ok(StepExecutorOutput {
        outputs: BTreeMap::from([
            ("compiled_plan".into(), json_value(compiled_value.clone())),
            (
                "ready_for_build".into(),
                bool_value(compiled.ready_for_build),
            ),
            ("planning_assessment".into(), assessment_value(&planning)?),
        ]),
        captured_assets: vec![
            asset(
                context.node.id.as_str(),
                RAW_PLAN_ASSET,
                "todo_validation_plan_raw",
                raw_value,
            ),
            asset(
                context.node.id.as_str(),
                COMPILED_PLAN_ASSET,
                "todo_validation_plan_compiled",
                compiled_value,
            ),
        ],
        evaluation: StepEvaluation {
            hard_gates: gates,
            evaluations: vec![planning],
        },
        ..StepExecutorOutput::default()
    })
}

async fn execute_validate(
    e2e: &E2eContext,
    context: &StepExecutorContext,
) -> Result<StepExecutorOutput> {
    let compiled: CompiledValidationPlan = input_json(context, "compiled_plan")?;
    compiled.validate_integrity()?;
    let contract = compiled.task_contract.clone();
    let bundle = TodoProbeRunner::new(contract.clone())?
        .run_compiled(e2e.client(), &compiled)
        .await?;
    todo_worker::persist_planned_validation_bundle(&contract, &bundle)?;
    let evidence = serde_json::to_value(&bundle)?;
    let construction_passed = ["manifest_valid", "worker_live", "function_surface"]
        .into_iter()
        .all(|probe| bundle.probe_passed(probe));
    let functional_passed = compiled
        .compiled_checks
        .iter()
        .all(|check| bundle.probe_passed(&check.probe_id));
    let coverage_passed = bundle.evidence_complete();
    let construction = evaluation(
        PLANNED_CRITERIA[1].id,
        construction_passed,
        format!(
            "manifest/live/surface passed={construction_passed}; candidate={:?}",
            bundle.subject.candidate_sha256
        ),
        [VALIDATION_ASSET.into()],
    );
    let coverage = evaluation(
        PLANNED_CRITERIA[2].id,
        coverage_passed,
        format!(
            "required={:?}; omitted={:?}",
            bundle.coverage.required, bundle.coverage.omitted
        ),
        [VALIDATION_ASSET.into()],
    );
    let functional = evaluation(
        PLANNED_CRITERIA[3].id,
        functional_passed,
        format!(
            "executed {} compiled hard-gated check(s); passed={functional_passed}",
            compiled.compiled_checks.len()
        ),
        [VALIDATION_ASSET.into()],
    );
    let mut gates = compiled
        .compiled_checks
        .iter()
        .map(|check| {
            gate(
                &check.id,
                bundle.probe_passed(&check.probe_id),
                probe_summary(&bundle, &check.probe_id),
                [VALIDATION_ASSET.into()],
            )
        })
        .collect::<Vec<_>>();
    gates.push(gate(
        "evidence_complete",
        coverage_passed,
        format!(
            "coverage complete={}, attempts={}, plan={:?}",
            bundle.coverage.complete,
            bundle.attempts.len(),
            bundle.plan_sha256
        ),
        [VALIDATION_ASSET.into()],
    ));
    Ok(StepExecutorOutput {
        outputs: BTreeMap::from([
            (
                "construction_assessment".into(),
                assessment_value(&construction)?,
            ),
            ("coverage_assessment".into(), assessment_value(&coverage)?),
            (
                "functional_assessment".into(),
                assessment_value(&functional)?,
            ),
            ("validation_bundle".into(), json_value(evidence.clone())),
        ]),
        captured_assets: vec![asset(
            context.node.id.as_str(),
            VALIDATION_ASSET_ID,
            "todo_validation_evidence",
            evidence,
        )],
        evaluation: StepEvaluation {
            hard_gates: gates,
            evaluations: vec![construction, coverage, functional],
        },
        ..StepExecutorOutput::default()
    })
}

struct TodoCleanup {
    context: Arc<E2eContext>,
}

#[async_trait]
impl WorkflowCleanupHook for TodoCleanup {
    async fn cleanup(&self, context: &WorkflowCleanupContext) -> Result<()> {
        let contract = todo_worker::task_contract(&context.attempt_id)?;
        todo_worker::cleanup_contract(self.context.as_ref(), &contract).await
    }
}

fn node(
    id: &str,
    step_type: &str,
    dependencies: &[&str],
    inputs: BTreeMap<String, WorkflowInputBinding>,
) -> WorkflowNodeV1 {
    WorkflowNodeV1 {
        id: id.into(),
        step_type: step_type.into(),
        step_version: 1,
        config: json!({}),
        depends_on: dependencies.iter().map(|value| (*value).into()).collect(),
        inputs,
        activation: Default::default(),
        dependency_policy: DependencyPolicy::Succeeded,
        required: true,
    }
}

fn output(node_id: &str, port: &str) -> WorkflowInputBinding {
    WorkflowInputBinding::Output {
        node_id: node_id.into(),
        port: port.into(),
    }
}

fn harness_config(
    prompt: &str,
    max_turns: u32,
    max_total_tokens: Option<u64>,
    planner: bool,
) -> Value {
    json!({
        "prompt": prompt,
        "max_turns": max_turns,
        "max_output_tokens": 8192,
        "max_total_tokens": max_total_tokens,
        "stuck_timeout_seconds": 600,
        "function_allow": if planner {
            vec!["coder::*", "shell::exec"]
        } else {
            vec!["coder::*", "shell::exec", "worker::validate", "worker::add", "worker::status", "engine::functions::info"]
        },
        "function_deny": if planner {
            vec!["worker::*", "engine::*", "harness::*", "e2e::*", "state::*", "database::*"]
        } else {
            vec!["harness::*", "e2e::*", "state::*", "database::*", "worker::remove", "worker::clear"]
        }
    })
}

fn criterion(
    id: &str,
    weight: u8,
    producer_node_id: &str,
    output_port: &str,
) -> WorkflowCriterionDeclaration {
    WorkflowCriterionDeclaration {
        id: id.into(),
        weight,
        producer_node_id: producer_node_id.into(),
        output_port: output_port.into(),
        advisory: false,
    }
}

fn descriptor(
    id: &str,
    description: &str,
    inputs: BTreeMap<String, StepPortDescriptor>,
    outputs: BTreeMap<String, StepPortDescriptor>,
    replay_policy: ReplayPolicy,
    operational_kind: StepOperationalKind,
) -> Result<StepTypeDescriptor> {
    let descriptor = StepTypeDescriptor {
        id: id.into(),
        version: 1,
        description: description.into(),
        config_schema: json!({"type": "object", "additionalProperties": false}),
        inputs,
        outputs,
        capabilities: Vec::new(),
        required_functions: Vec::new(),
        replay_policy,
        operational_kind,
    };
    descriptor.validate()?;
    Ok(descriptor)
}

fn ports(
    definitions: &[(&str, PortValueKind, bool, Option<ControlSource>)],
) -> BTreeMap<String, StepPortDescriptor> {
    definitions
        .iter()
        .map(|(id, kind, optional, control_source)| {
            (
                (*id).into(),
                StepPortDescriptor {
                    kind: *kind,
                    optional: *optional,
                    control_source: *control_source,
                },
            )
        })
        .collect()
}

fn input_text<'a>(context: &'a StepExecutorContext, id: &str) -> Result<&'a str> {
    context
        .inputs
        .get(id)
        .with_context(|| format!("missing workflow input '{id}'"))?
        .value
        .as_str()
        .with_context(|| format!("workflow input '{id}' is not text"))
}

fn input_json<T>(context: &StepExecutorContext, id: &str) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let value = context
        .inputs
        .get(id)
        .with_context(|| format!("missing workflow input '{id}'"))?
        .value
        .clone();
    serde_json::from_value(value).with_context(|| format!("decode workflow input '{id}'"))
}

fn text_value(value: impl Into<String>) -> TypedPortValue {
    TypedPortValue {
        kind: PortValueKind::TextUtf8,
        value: Value::String(value.into()),
    }
}

fn json_value(value: Value) -> TypedPortValue {
    TypedPortValue {
        kind: PortValueKind::Json,
        value,
    }
}

fn bool_value(value: bool) -> TypedPortValue {
    TypedPortValue {
        kind: PortValueKind::Boolean,
        value: Value::Bool(value),
    }
}

fn assessment_value(value: &WorkflowEvaluationResult) -> Result<TypedPortValue> {
    Ok(TypedPortValue {
        kind: PortValueKind::Assessment,
        value: serde_json::to_value(value)?,
    })
}

fn asset(step_id: &str, id: &str, kind: &str, content: Value) -> CapturedWorkflowAsset {
    CapturedWorkflowAsset {
        id: id.into(),
        kind: kind.into(),
        media_type: "application/json".into(),
        content: WorkflowAssetContent::Json(content),
        provenance: vec![WorkflowProvenance {
            source_step_id: step_id.into(),
            relation: "captured_before_cleanup".into(),
        }],
    }
}

fn gate(
    id: &str,
    passed: bool,
    reason: impl Into<String>,
    evidence_ids: impl IntoIterator<Item = String>,
) -> WorkflowGateResult {
    WorkflowGateResult {
        id: id.into(),
        passed,
        reason: reason.into(),
        evidence_ids: evidence_ids.into_iter().collect(),
    }
}

fn evaluation(
    id: &str,
    passed: bool,
    summary: impl Into<String>,
    evidence_ids: impl IntoIterator<Item = String>,
) -> WorkflowEvaluationResult {
    WorkflowEvaluationResult {
        id: id.into(),
        outcome: if passed {
            WorkflowEvaluationOutcome::Passed
        } else {
            WorkflowEvaluationOutcome::Failed
        },
        summary: summary.into(),
        score: Some(if passed { 1.0 } else { 0.0 }),
        evidence_ids: evidence_ids.into_iter().collect(),
    }
}

fn probe_summary(bundle: &ValidationEvidenceBundle, probe_id: &str) -> String {
    let observations = bundle
        .attempts
        .last()
        .into_iter()
        .flat_map(|attempt| &attempt.probes)
        .filter(|probe| probe.id == probe_id)
        .map(|probe| {
            format!(
                "repetition={} outcome={:?} observed={}",
                probe.repetition,
                probe.outcome,
                serde_json::to_string(&probe.observed).unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    if observations.is_empty() {
        format!("probe '{probe_id}' was not observed")
    } else {
        observations.join("; ").chars().take(2_048).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_exactly_five_sequential_nodes() {
        let definition = definition();
        assert_eq!(definition.nodes.len(), 5);
        assert_eq!(definition.limits.max_nodes, 5);
        assert_eq!(definition.limits.max_parallel, 1);
        assert_eq!(definition.limits.technical_retries, 0);
        assert_eq!(definition.nodes[0].id, "prepare_workspace");
        assert_eq!(definition.nodes[4].id, "validate_todo_worker");
    }

    #[test]
    fn planner_and_builder_harness_budgets_are_unbounded() {
        let definition = definition();
        let planner = &definition.nodes[1];
        let builder = &definition.nodes[3];
        assert_eq!(planner.config["max_turns"], 12);
        assert!(planner.config["max_total_tokens"].is_null());
        assert_eq!(builder.config["max_turns"], 48);
        assert!(builder.config["max_total_tokens"].is_null());
        assert_ne!(planner.id, builder.id);
        let planner_session = super::super::builtin::workflow_session_id("attempt-1", &planner.id);
        let builder_session = super::super::builtin::workflow_session_id("attempt-1", &builder.id);
        assert_ne!(planner_session, builder_session);
    }

    #[test]
    fn descriptors_validate_the_definition() {
        let mut catalog = StepCatalog::new();
        catalog
            .register_descriptor(super::super::harness_descriptor_v2().unwrap())
            .unwrap();
        for descriptor in descriptors_only().unwrap() {
            catalog.register_descriptor(descriptor).unwrap();
        }
        definition().validate(&catalog).unwrap();
    }
}
