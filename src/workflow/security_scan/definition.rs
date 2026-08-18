use super::*;

pub fn definition() -> WorkflowDefinitionV1 {
    WorkflowDefinitionV1 {
        schema_version: crate::workflow::WORKFLOW_SCHEMA_VERSION,
        id: SCENARIO_ID.into(),
        scenario_version: 2,
        description: "Rust-defined local security review: scan and deduplication, optional suggestions, GitHub reconciliation, cron execution, final listing, and mandatory cleanup.".into(),
        limits: WorkflowLimits {
            max_parallel: 3,
            max_nodes: 8,
            step_timeout_seconds: 420,
            workflow_timeout_seconds: 1_800,
            max_total_tokens: Some(500_000),
            max_cost_usd: Some(25.0),
            technical_retries: 0,
        },
        nodes: vec![
            semantic_test("scan_commit_a", "security_review.scan_commit_a", &[], true),
            WorkflowNodeV1 {
                id: "suggest_commit_a".into(),
                step_type: "security_review.suggest_commit_a".into(),
                step_version: 1,
                config: json!({}),
                depends_on: vec!["scan_commit_a".into()],
                inputs: BTreeMap::from([
                    (
                        "repository".into(),
                        WorkflowInputBinding::Output {
                            node_id: "scan_commit_a".into(),
                            port: "repository".into(),
                        },
                    ),
                    (
                        "commit_a".into(),
                        WorkflowInputBinding::Output {
                            node_id: "scan_commit_a".into(),
                            port: "commit_a".into(),
                        },
                    ),
                ]),
                activation: ActivationPolicy::All(vec![BooleanCondition {
                    node_id: "scan_commit_a".into(),
                    port: "should_run_suggest".into(),
                    equals: true,
                }]),
                dependency_policy: DependencyPolicy::Succeeded,
                required: false,
            },
            WorkflowNodeV1 {
                inputs: BTreeMap::from([(
                    "scan_run_id".into(),
                    WorkflowInputBinding::Output {
                        node_id: "scan_commit_a".into(),
                        port: "scan_run_id".into(),
                    },
                )]),
                ..semantic_test(
                    "github_reconciliation",
                    "security_review.github_reconciliation",
                    &["scheduled_scan_commit_b"],
                    true,
                )
            },
            WorkflowNodeV1 {
                dependency_policy: DependencyPolicy::Terminal,
                inputs: BTreeMap::from([(
                    "repository".into(),
                    WorkflowInputBinding::Output {
                        node_id: "scan_commit_a".into(),
                        port: "repository".into(),
                    },
                )]),
                ..semantic_test(
                    "scheduled_scan_commit_b",
                    "security_review.scheduled_scan_commit_b",
                    &["scan_commit_a", "suggest_commit_a"],
                    true,
                )
            },
            WorkflowNodeV1 {
                dependency_policy: DependencyPolicy::Terminal,
                ..semantic_test(
                    "list_run_history",
                    "security_review.list_run_history",
                    &["github_reconciliation"],
                    true,
                )
            },
        ],
        criteria: vec![
            criterion("scan_a_detection", 60, "scan_commit_a"),
            criterion("suggest_a_quality", 20, "suggest_commit_a"),
            criterion("scheduled_b_detection", 20, "scheduled_scan_commit_b"),
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

fn criterion(id: &str, weight: u8, producer_node_id: &str) -> WorkflowCriterionDeclaration {
    WorkflowCriterionDeclaration {
        id: id.into(),
        weight,
        producer_node_id: producer_node_id.into(),
        output_port: "assessment".into(),
        advisory: true,
    }
}

pub(super) fn descriptors() -> Vec<(StepTypeDescriptor, SecurityStepKind)> {
    vec![
        (
            descriptor(
                "security_review.scan_commit_a",
                "Validate contracts and fixture, request the exact scan twice, await it, assess the report, and prove repository immutability.",
                object_schema(&[], &[]),
                BTreeMap::new(),
                BTreeMap::from([
                    ("repository".into(), port(PortValueKind::TextUtf8, false, None)),
                    ("commit_a".into(), port(PortValueKind::TextUtf8, false, None)),
                    ("scan_run_id".into(), port(PortValueKind::TextUtf8, false, None)),
                    ("report".into(), port(PortValueKind::Json, false, None)),
                    ("should_run_suggest".into(), port(PortValueKind::Boolean, false, Some(ControlSource::Deterministic))),
                    ("assessment".into(), port(PortValueKind::Assessment, false, None)),
                ]),
                ReplayPolicy::Idempotent,
                StepOperationalKind::Product,
            ),
            SecurityStepKind::ScanCommitA,
        ),
        (
            descriptor(
                "security_review.suggest_commit_a",
                "When deterministic scan output permits it, request suggestions, await the report, check patches in a disposable copy, and prove the fixture stayed unchanged.",
                object_schema(&[], &[]),
                BTreeMap::from([
                    ("repository".into(), port(PortValueKind::TextUtf8, false, None)),
                    ("commit_a".into(), port(PortValueKind::TextUtf8, false, None)),
                ]),
                BTreeMap::from([
                    ("run_id".into(), port(PortValueKind::TextUtf8, false, None)),
                    ("report".into(), port(PortValueKind::Json, false, None)),
                    ("assessment".into(), port(PortValueKind::Assessment, false, None)),
                ]),
                ReplayPolicy::Idempotent,
                StepOperationalKind::Product,
            ),
            SecurityStepKind::SuggestCommitA,
        ),
        (
            descriptor(
                "security_review.github_reconciliation",
                "Read cached state, refresh GitHub sources, verify the persisted reread, and exercise source/severity pagination filters.",
                object_schema(&[], &[]),
                BTreeMap::from([("scan_run_id".into(), port(PortValueKind::TextUtf8, false, None))]),
                BTreeMap::from([("snapshot".into(), port(PortValueKind::Json, false, None))]),
                ReplayPolicy::Idempotent,
                StepOperationalKind::Product,
            ),
            SecurityStepKind::Reconciliation,
        ),
        (
            descriptor(
                "security_review.scheduled_scan_commit_b",
                "Create the delayed ref, observe the cron-created exact-SHA scan, assess its report, and leave restoration to the mandatory cleanup hook.",
                object_schema(&[], &[]),
                BTreeMap::from([("repository".into(), port(PortValueKind::TextUtf8, false, None))]),
                BTreeMap::from([
                    ("run_id".into(), port(PortValueKind::TextUtf8, false, None)),
                    ("commit_b".into(), port(PortValueKind::TextUtf8, false, None)),
                    ("report".into(), port(PortValueKind::Json, false, None)),
                    ("assessment".into(), port(PortValueKind::Assessment, false, None)),
                ]),
                ReplayPolicy::Compensable,
                StepOperationalKind::Product,
            ),
            SecurityStepKind::ScheduledScanCommitB,
        ),
        (
            descriptor(
                "security_review.list_run_history",
                "Verify the final completed scan, optional suggestion, and cron run through bounded list filters.",
                object_schema(&[], &[]),
                BTreeMap::new(),
                BTreeMap::from([("runs".into(), port(PortValueKind::Json, false, None))]),
                ReplayPolicy::Idempotent,
                StepOperationalKind::Product,
            ),
            SecurityStepKind::ListRunHistory,
        ),
    ]
}

fn descriptor(
    id: &str,
    description: &str,
    config_schema: Value,
    inputs: BTreeMap<String, StepPortDescriptor>,
    outputs: BTreeMap<String, StepPortDescriptor>,
    replay_policy: ReplayPolicy,
    operational_kind: StepOperationalKind,
) -> StepTypeDescriptor {
    let required_functions = match operational_kind {
        StepOperationalKind::Product => [
            REQUEST_FUNCTION,
            READ_FUNCTION,
            LIST_FUNCTION,
            RECONCILIATION_FUNCTION,
        ]
        .into_iter()
        .map(required_contract)
        .collect(),
        _ => Vec::new(),
    };
    StepTypeDescriptor {
        id: id.into(),
        version: 1,
        description: description.into(),
        config_schema,
        inputs,
        outputs,
        capabilities: vec!["security_scan::v1".into()],
        required_functions,
        replay_policy,
        operational_kind,
    }
}

pub(crate) fn required_contract(function_id: &str) -> RequiredFunctionContract {
    let (_, request, response) = SECURITY_SCAN_CONTRACT_HASHES
        .iter()
        .find(|(observed, _, _)| *observed == function_id)
        .expect("security-scan contract hash is registered");
    RequiredFunctionContract {
        function_id: function_id.into(),
        request_schema_sha256: Some((*request).into()),
        response_schema_sha256: Some((*response).into()),
    }
}
