use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use serde_json::json;

use super::{runtime, Case, VERSION};
use crate::context::E2eContext;
use crate::scenarios::ScenarioId;
use crate::workflow::{
    ControlSource, DependencyPolicy, PortValueKind, ReplayPolicy, RequiredFunctionContract,
    StepCatalog, StepOperationalKind, StepPortDescriptor, StepTypeDescriptor, WorkflowCleanupHook,
    WorkflowCriterionDeclaration, WorkflowDefinitionV1, WorkflowLimits, WorkflowNodeV1,
};

pub const PREPARE: &str = "swe.prepare";
pub const SUBJECT: &str = "swe.subject";
pub const CAPTURE: &str = "swe.capture";

pub fn definition(scenario: ScenarioId) -> WorkflowDefinitionV1 {
    let case = Case::from_scenario(scenario).expect("SWE identity");
    WorkflowDefinitionV1 {
        schema_version: crate::workflow::WORKFLOW_SCHEMA_VERSION,
        id: case.id.into(),
        scenario_version: VERSION,
        description: case.description().into(),
        limits: WorkflowLimits {
            max_parallel: 1,
            max_nodes: 3,
            step_timeout_seconds: case.deadline_seconds(),
            workflow_timeout_seconds: case.deadline_seconds(),
            max_total_tokens: Some(case.tokens()),
            max_cost_usd: None,
            technical_retries: 0,
        },
        nodes: vec![
            node("prepare", PREPARE, &[], DependencyPolicy::Succeeded),
            node("work", SUBJECT, &["prepare"], DependencyPolicy::Succeeded),
            node("capture", CAPTURE, &["work"], DependencyPolicy::Terminal),
        ],
        criteria: vec![WorkflowCriterionDeclaration {
            id: "swe_delivery".into(),
            weight: 100,
            producer_node_id: "capture".into(),
            output_port: "delivery".into(),
            advisory: false,
        }],
    }
}

fn node(
    id: &str,
    step: &str,
    dependencies: &[&str],
    dependency_policy: DependencyPolicy,
) -> WorkflowNodeV1 {
    WorkflowNodeV1 {
        id: id.into(),
        step_type: step.into(),
        step_version: 1,
        config: json!({}),
        depends_on: dependencies.iter().map(|id| (*id).into()).collect(),
        inputs: BTreeMap::new(),
        activation: Default::default(),
        dependency_policy,
        required: true,
    }
}

pub fn descriptors() -> Vec<StepTypeDescriptor> {
    [PREPARE, SUBJECT, CAPTURE].into_iter().map(|id| {
        let capture = id == CAPTURE;
        StepTypeDescriptor {
            id: id.into(), version: 1,
            description: match id {
                PREPARE => "Export and verify the selected SWE entry snapshot and execution boundary.",
                SUBJECT => "Run one continuing Harness session with optional delegation and aggregate resource limits.",
                _ => "Capture committed SWE deliveries and the last unfinished attempt independently.",
            }.into(),
            config_schema: json!({"type":"object","additionalProperties":false}),
            inputs: BTreeMap::new(),
            outputs: BTreeMap::from([(
                if capture { "delivery" } else { "completed" }.into(),
                StepPortDescriptor {
                    kind: if capture { PortValueKind::Assessment } else { PortValueKind::Boolean },
                    optional: false,
                    control_source: (!capture).then_some(ControlSource::Deterministic),
                },
            )]),
            capabilities: vec!["swe::isolated-python-workspace".into()],
            required_functions: if id == SUBJECT {
                ["harness::send", "harness::metrics", "harness::status", "harness::session-tree", "harness::stop", "harness::teardown"]
                    .into_iter().map(|function_id| RequiredFunctionContract {
                        function_id: function_id.into(), request_schema_sha256: None, response_schema_sha256: None,
                    }).collect()
            } else { Vec::new() },
            replay_policy: if id == SUBJECT { ReplayPolicy::NonRepeatable } else { ReplayPolicy::Idempotent },
            operational_kind: match id { SUBJECT => StepOperationalKind::Harness, CAPTURE => StepOperationalKind::Assessment, _ => StepOperationalKind::Transformation },
        }
    }).collect()
}

pub fn register(
    catalog: &mut StepCatalog,
    scenario: ScenarioId,
    context: Arc<E2eContext>,
    model: &str,
    provider: &str,
) -> Result<Arc<dyn WorkflowCleanupHook>> {
    runtime::register(
        catalog,
        Case::from_scenario(scenario).expect("SWE identity"),
        context,
        model,
        provider,
    )
}
