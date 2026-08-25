use super::*;

pub fn definition() -> WorkflowDefinitionV1 {
    WorkflowDefinitionV1 {
        schema_version: super::super::WORKFLOW_SCHEMA_VERSION,
        id: crate::scenarios::incident_response::ID.into(),
        scenario_version: crate::scenarios::incident_response::VERSION,
        description: "Code-owned incident response: preflight, baseline, reproduction, parallel triage, diagnosis, bounded remediation, deterministic promotion or rollback, reconciliation, report, and mandatory cleanup.".into(),
        limits: WorkflowLimits {
            max_parallel: 3,
            max_nodes: 20,
            step_timeout_seconds: 600,
            workflow_timeout_seconds: 3_600,
            // Reserve 64k tokens for the agent-owned two-revision planner.
            max_total_tokens: Some(686_000),
            max_cost_usd: Some(25.0),
            technical_retries: 0,
        },
        nodes: vec![
            semantic_test("preflight_fixture", "incident_response.preflight_fixture", &[], true),
            semantic_test("capture_baseline", "incident_response.capture_baseline", &["preflight_fixture"], true),
            semantic_test("deduplicate_alert", "incident_response.deduplicate_alert", &["capture_baseline"], true),
            semantic_test("reproduce_incident", "incident_response.reproduce_incident", &["deduplicate_alert"], true),
            harness_node("analyze_logs", prompts::ANALYZE_LOGS, &["reproduce_incident"], "reproduce_incident", "analysis_bundle", true),
            harness_node("analyze_metrics", prompts::ANALYZE_METRICS, &["reproduce_incident"], "reproduce_incident", "analysis_bundle", true),
            harness_node("analyze_trace_change", prompts::ANALYZE_TRACE_CHANGE, &["reproduce_incident"], "reproduce_incident", "analysis_bundle", true),
            WorkflowNodeV1 {
                inputs: BTreeMap::from([(
                    "reproduction".into(),
                    WorkflowInputBinding::Output {
                        node_id: "reproduce_incident".into(),
                        port: "reproduction".into(),
                    },
                )]),
                ..semantic_test(
                    "validate_triage",
                    "incident_response.validate_triage",
                    &["analyze_logs", "analyze_metrics", "analyze_trace_change"],
                    true,
                )
            },
            harness_node(
                "synthesize_diagnosis",
                prompts::SYNTHESIZE_DIAGNOSIS,
                &["validate_triage"],
                "validate_triage",
                "triage",
                true,
            ),
            WorkflowNodeV1 {
                inputs: BTreeMap::from([(
                    "triage".into(),
                    WorkflowInputBinding::Output {
                        node_id: "validate_triage".into(),
                        port: "triage".into(),
                    },
                )]),
                ..semantic_test(
                    "validate_diagnosis",
                    "incident_response.validate_diagnosis",
                    &["synthesize_diagnosis"],
                    true,
                )
            },
            conditional_harness_node(
                "apply_remediation",
                prompts::APPLY_REMEDIATION,
                &["validate_diagnosis"],
                "validate_diagnosis",
                "diagnosis",
                BooleanCondition {
                    node_id: "validate_diagnosis".into(),
                    port: "ready_for_remediation".into(),
                    equals: true,
                },
            ),
            WorkflowNodeV1 {
                required: false,
                dependency_policy: DependencyPolicy::Terminal,
                activation: ActivationPolicy::All(vec![BooleanCondition {
                    node_id: "validate_diagnosis".into(),
                    port: "ready_for_remediation".into(),
                    equals: true,
                }]),
                ..semantic_test(
                    "validate_candidate",
                    "incident_response.validate_candidate",
                    &["validate_diagnosis", "apply_remediation"],
                    false,
                )
            },
            WorkflowNodeV1 {
                dependency_policy: DependencyPolicy::Terminal,
                ..semantic_test(
                    "decide_terminal_action",
                    "incident_response.decide_terminal_action",
                    &["validate_diagnosis", "validate_candidate"],
                    true,
                )
            },
            conditional_semantic_test(
                "promote_candidate",
                "incident_response.promote_candidate",
                &["decide_terminal_action"],
                BooleanCondition {
                    node_id: "decide_terminal_action".into(),
                    port: "should_promote".into(),
                    equals: true,
                },
            ),
            conditional_semantic_test(
                "rollback_candidate",
                "incident_response.rollback_candidate",
                &["decide_terminal_action"],
                BooleanCondition {
                    node_id: "decide_terminal_action".into(),
                    port: "should_rollback".into(),
                    equals: true,
                },
            ),
            WorkflowNodeV1 {
                dependency_policy: DependencyPolicy::Terminal,
                ..semantic_test(
                    "reconcile_final_state",
                    "incident_response.reconcile_final_state",
                    &["promote_candidate", "rollback_candidate"],
                    true,
                )
            },
            harness_node(
                "write_incident_report",
                prompts::WRITE_INCIDENT_REPORT,
                &["reconcile_final_state"],
                "reconcile_final_state",
                "report_bundle",
                true,
            ),
            semantic_test(
                "validate_incident_report",
                "incident_response.validate_incident_report",
                &["write_incident_report"],
                true,
            ),
        ],
        criteria: vec![
            criterion("incident_reproduction", 15, "reproduce_incident", "assessment"),
            criterion(
                "evidence_grounded_diagnosis",
                20,
                "validate_diagnosis",
                "assessment",
            ),
            criterion(
                "remediation_integrity",
                25,
                "decide_terminal_action",
                "remediation_assessment",
            ),
            criterion(
                "safe_terminal_action",
                25,
                "decide_terminal_action",
                "terminal_assessment",
            ),
            criterion(
                "final_reconciliation",
                15,
                "reconcile_final_state",
                "assessment",
            ),
        ],
    }
}

fn semantic_test(
    id: &str,
    step_type: &str,
    dependencies: &[&str],
    required: bool,
) -> WorkflowNodeV1 {
    WorkflowNodeV1 {
        id: id.into(),
        step_type: step_type.into(),
        step_version: 1,
        config: json!({}),
        depends_on: dependencies.iter().map(|value| (*value).into()).collect(),
        inputs: BTreeMap::new(),
        activation: ActivationPolicy::Always,
        dependency_policy: DependencyPolicy::Succeeded,
        required,
    }
}

fn conditional_semantic_test(
    id: &str,
    step_type: &str,
    dependencies: &[&str],
    condition: BooleanCondition,
) -> WorkflowNodeV1 {
    WorkflowNodeV1 {
        required: false,
        activation: ActivationPolicy::All(vec![condition]),
        ..semantic_test(id, step_type, dependencies, false)
    }
}

fn harness_node(
    id: &str,
    prompt: &str,
    dependencies: &[&str],
    data_node: &str,
    data_port: &str,
    required: bool,
) -> WorkflowNodeV1 {
    WorkflowNodeV1 {
        id: id.into(),
        step_type: super::super::HARNESS_STEP_ID.into(),
        step_version: super::super::HARNESS_STEP_VERSION_V2,
        config: harness_config(prompt, false),
        depends_on: dependencies.iter().map(|value| (*value).into()).collect(),
        inputs: BTreeMap::from([
            (
                "data".into(),
                WorkflowInputBinding::Output {
                    node_id: data_node.into(),
                    port: data_port.into(),
                },
            ),
            (
                "workspace_root".into(),
                WorkflowInputBinding::Output {
                    node_id: "preflight_fixture".into(),
                    port: "workspace_root".into(),
                },
            ),
        ]),
        activation: ActivationPolicy::Always,
        dependency_policy: DependencyPolicy::Succeeded,
        required,
    }
}

fn conditional_harness_node(
    id: &str,
    prompt: &str,
    dependencies: &[&str],
    data_node: &str,
    data_port: &str,
    condition: BooleanCondition,
) -> WorkflowNodeV1 {
    WorkflowNodeV1 {
        required: false,
        activation: ActivationPolicy::All(vec![condition]),
        config: harness_config(prompt, true),
        ..harness_node(id, prompt, dependencies, data_node, data_port, false)
    }
}

fn harness_config(prompt: &str, remediation: bool) -> Value {
    let allow = if remediation {
        vec!["coder::*", "shell::exec"]
    } else {
        vec![
            "coder::info",
            "coder::read-file",
            "coder::create-file",
            "shell::exec",
        ]
    };
    json!({
        "prompt": prompt,
        "max_turns": if remediation { 24 } else { 12 },
        "max_output_tokens": 8192,
        "max_total_tokens": if remediation { 180000 } else { 90000 },
        "stuck_timeout_seconds": 600,
        "function_allow": allow,
        "function_deny": [
            "e2e::*",
            "incident-fixture::*",
            "database::*",
            "engine::*",
            "state::*",
            "storage::*",
            "worker::*"
        ]
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

pub(super) fn descriptors() -> Result<Vec<(StepTypeDescriptor, IncidentStepKind)>> {
    Ok(vec![
        pair(
            "incident_response.preflight_fixture",
            "Verify fixture path, Git state, immutable revisions, contract identity, and exact fixture worker contracts.",
            BTreeMap::new(),
            ports(&[("workspace_root", PortValueKind::TextUtf8, false, None), ("preflight", PortValueKind::Json, false, None)]),
            ReplayPolicy::Idempotent,
            StepOperationalKind::Assessment,
            (&FIXTURE_FUNCTIONS, IncidentStepKind::Preflight),
        )?,
        pair("incident_response.capture_baseline", "Capture repository, deploy, data, ledger, audit, and telemetry baseline before mutation.", BTreeMap::new(), ports(&[("baseline", PortValueKind::Json, false, None)]), ReplayPolicy::Idempotent, StepOperationalKind::Assessment, (&[BASELINE_FUNCTION], IncidentStepKind::Baseline))?,
        pair("incident_response.deduplicate_alert", "Submit the same synthetic alert twice and prove one stable incident identity.", BTreeMap::new(), ports(&[("incident", PortValueKind::Json, false, None)]), ReplayPolicy::Idempotent, StepOperationalKind::Product, (&[ALERT_FUNCTION], IncidentStepKind::Alert))?,
        pair("incident_response.reproduce_incident", "Execute two isolated seeded reproductions and capture bounded telemetry for independent analysis.", BTreeMap::new(), ports(&[("reproduction", PortValueKind::Json, false, None), ("analysis_bundle", PortValueKind::Json, false, None), ("assessment", PortValueKind::Assessment, false, None)]), ReplayPolicy::Compensable, StepOperationalKind::Product, (&[REPRODUCE_FUNCTION, TELEMETRY_FUNCTION], IncidentStepKind::Reproduce))?,
        pair("incident_response.validate_triage", "Validate three structured read-only analyses, evidence references, and deterministic fan-in.", ports(&[("reproduction", PortValueKind::Json, false, None)]), ports(&[("triage", PortValueKind::Json, false, None)]), ReplayPolicy::Idempotent, StepOperationalKind::Assessment, (&[], IncidentStepKind::ValidateTriage))?,
        pair("incident_response.validate_diagnosis", "Validate synthesis grounding and execute a fixture-owned falsification probe before mutation.", ports(&[("triage", PortValueKind::Json, false, None)]), ports(&[("ready_for_remediation", PortValueKind::Boolean, false, Some(ControlSource::Deterministic)), ("diagnosis", PortValueKind::Json, false, None), ("assessment", PortValueKind::Assessment, false, None)]), ReplayPolicy::Compensable, StepOperationalKind::Assessment, (&[VALIDATE_FUNCTION], IncidentStepKind::ValidateDiagnosis))?,
        pair("incident_response.validate_candidate", "Capture the candidate patch and deterministically validate path, test, replay, concurrency, ledger, audit, and canary invariants.", BTreeMap::new(), ports(&[("candidate_valid", PortValueKind::Boolean, false, Some(ControlSource::Deterministic))]), ReplayPolicy::Compensable, StepOperationalKind::Assessment, (&[VALIDATE_FUNCTION], IncidentStepKind::ValidateCandidate))?,
        pair("incident_response.decide_terminal_action", "Select exactly one deterministic terminal action from attempt-owned diagnosis and validation state.", BTreeMap::new(), ports(&[("should_promote", PortValueKind::Boolean, false, Some(ControlSource::Deterministic)), ("should_rollback", PortValueKind::Boolean, false, Some(ControlSource::Deterministic)), ("remediation_assessment", PortValueKind::Assessment, false, None), ("terminal_assessment", PortValueKind::Assessment, false, None)]), ReplayPolicy::Idempotent, StepOperationalKind::Assessment, (&[], IncidentStepKind::Decide))?,
        pair("incident_response.promote_candidate", "Promote only the exact candidate revision that passed every deterministic candidate gate.", BTreeMap::new(), BTreeMap::new(), ReplayPolicy::Compensable, StepOperationalKind::Product, (&[DEPLOY_FUNCTION], IncidentStepKind::Promote))?,
        pair("incident_response.rollback_candidate", "Restore the exact known-good revision when diagnosis or candidate validation cannot authorize promotion.", BTreeMap::new(), BTreeMap::new(), ReplayPolicy::Compensable, StepOperationalKind::Product, (&[DEPLOY_FUNCTION], IncidentStepKind::Rollback))?,
        pair("incident_response.reconcile_final_state", "Reconcile deployed revision, ledger, audit, incident state, and active resources after the exclusive terminal action.", BTreeMap::new(), ports(&[("final_state", PortValueKind::Json, false, None), ("report_bundle", PortValueKind::Json, false, None), ("assessment", PortValueKind::Assessment, false, None)]), ReplayPolicy::Idempotent, StepOperationalKind::Assessment, (&[RECONCILE_FUNCTION], IncidentStepKind::Reconcile))?,
        pair("incident_response.validate_incident_report", "Validate the bounded report's revision, action, validation, and evidence references without affecting system outcome.", BTreeMap::new(), ports(&[("validated", PortValueKind::Boolean, false, Some(ControlSource::Deterministic))]), ReplayPolicy::Idempotent, StepOperationalKind::Assessment, (&[], IncidentStepKind::ValidateReport))?,
    ])
}

fn pair(
    id: &str,
    description: &str,
    inputs: BTreeMap<String, StepPortDescriptor>,
    outputs: BTreeMap<String, StepPortDescriptor>,
    replay_policy: ReplayPolicy,
    operational_kind: StepOperationalKind,
    binding: (&[&str], IncidentStepKind),
) -> Result<(StepTypeDescriptor, IncidentStepKind)> {
    Ok((
        StepTypeDescriptor {
            id: id.into(),
            version: 1,
            description: description.into(),
            config_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
            inputs,
            outputs,
            capabilities: vec!["incident_fixture::v1".into()],
            required_functions: binding
                .0
                .iter()
                .map(|function| required_contract(function))
                .collect::<Result<Vec<_>>>()?,
            replay_policy,
            operational_kind,
        },
        binding.1,
    ))
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
