use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use tokio::process::Command;

use crate::context::E2eContext;

use super::{
    ActivationPolicy, BooleanCondition, CapturedWorkflowAsset, ControlSource, DependencyPolicy,
    PortValueKind, ReplayPolicy, RequiredFunctionContract, StepCatalog, StepEvaluation,
    StepExecutor, StepExecutorContext, StepExecutorOutput, StepOperationalKind, StepPortDescriptor,
    StepTypeDescriptor, TypedPortValue, WorkflowAssetContent, WorkflowCleanupContext,
    WorkflowCleanupHook, WorkflowCriterionDeclaration, WorkflowDefinitionV1,
    WorkflowEvaluationOutcome, WorkflowEvaluationResult, WorkflowGateResult, WorkflowInputBinding,
    WorkflowLimits, WorkflowNodeV1, WorkflowProvenance,
};

pub const FIXTURE_PATH_ENV: &str = "HARNESS_E2E_SECURITY_FIXTURE_PATH";
pub const SCENARIO_ID: &str = "security_review";
const REPOSITORY: &str = "iii-hq/security-scan-e2e-fixture";
const SCHEDULED_REF: &str = "security-scan-e2e-scheduled";
const SEEDED_PATHS: [&str; 4] = [
    "src/vulnerable.rs",
    "package.json",
    ".env.example",
    ".github/workflows/insecure.yml",
];
const REQUEST_FUNCTION: &str = "security-scan::request";
const READ_FUNCTION: &str = "security-scan::read";
const LIST_FUNCTION: &str = "security-scan::list";
const RECONCILIATION_FUNCTION: &str = "security-scan::reconciliation";

const SECURITY_SCAN_CONTRACT_HASHES: [(&str, &str, &str); 4] = [
    (
        REQUEST_FUNCTION,
        "sha256:98d05e7144cf148707bfcf79382fda5cbd9c493424b7ce3aed934db61acf2994",
        "sha256:c749c1c1255471b8137115962184b1fbb8d4f15c15fb5ccb2a446fcd373aca98",
    ),
    (
        READ_FUNCTION,
        "sha256:20c305053371e147a2bd1802e81533bbccebf2f3d29d8269c27102713e7bcb0a",
        "sha256:d065d73944025a81235fe967ea837ffbee6dfeef9ae02cc61c57fe3c2197ea7c",
    ),
    (
        LIST_FUNCTION,
        "sha256:12bb3e62ac2c77b318d843ac9c62a86158d179118c36226eaef9ca3f0526a44b",
        "sha256:289b33d4b74d53f02fafa1e3d7d6f6d1494dcdf58fe397d0b5f83f611d9c73b6",
    ),
    (
        RECONCILIATION_FUNCTION,
        "sha256:ab5b929ded2087de6932a40a93c7d547854687e274c79a01ebc913efe92d6ab3",
        "sha256:532b0d7b93389c1b4598141a863dc789edb6be95a7af5cdd4555a691940f00a1",
    ),
];

#[derive(Debug, Clone, Copy)]
enum SecurityStepKind {
    ScanCommitA,
    SuggestCommitA,
    Reconciliation,
    ScheduledScanCommitB,
    ListRunHistory,
}

struct SecurityExecutor {
    context: Arc<E2eContext>,
    kind: SecurityStepKind,
    fixture: Arc<FixtureState>,
}

#[derive(Default)]
struct FixtureState {
    inner: Mutex<FixtureStateInner>,
}

#[derive(Default)]
struct FixtureStateInner {
    path: Option<PathBuf>,
    initial_head: Option<String>,
    scheduled_ref: Option<String>,
    scheduled_sha: Option<String>,
    suggest_expected: bool,
}

pub fn register_security_scan_steps(
    catalog: &mut StepCatalog,
    context: Arc<E2eContext>,
) -> Result<Arc<dyn WorkflowCleanupHook>> {
    let fixture = Arc::new(FixtureState::default());
    for (descriptor, kind) in descriptors() {
        catalog.register(
            descriptor,
            Arc::new(SecurityExecutor {
                context: context.clone(),
                kind,
                fixture: fixture.clone(),
            }),
        )?;
    }
    Ok(Arc::new(SecurityReviewCleanup { fixture }))
}

struct SecurityReviewCleanup {
    fixture: Arc<FixtureState>,
}

#[async_trait]
impl WorkflowCleanupHook for SecurityReviewCleanup {
    async fn cleanup(&self, _context: &WorkflowCleanupContext) -> Result<()> {
        self.fixture.restore().await
    }
}

pub fn descriptors_only() -> Vec<StepTypeDescriptor> {
    descriptors()
        .into_iter()
        .map(|(descriptor, _)| descriptor)
        .collect()
}

pub fn definition() -> WorkflowDefinitionV1 {
    WorkflowDefinitionV1 {
        schema_version: super::WORKFLOW_SCHEMA_VERSION,
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

fn descriptors() -> Vec<(StepTypeDescriptor, SecurityStepKind)> {
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

fn required_contract(function_id: &str) -> RequiredFunctionContract {
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

#[async_trait]
impl StepExecutor for SecurityExecutor {
    async fn preflight(&self, _context: &StepExecutorContext) -> Result<()> {
        for function in [
            REQUEST_FUNCTION,
            READ_FUNCTION,
            LIST_FUNCTION,
            RECONCILIATION_FUNCTION,
        ] {
            if !self.context.function_exists(function).await? {
                bail!("required security-scan function '{function}' is unavailable");
            }
        }
        Ok(())
    }

    async fn execute(&self, context: StepExecutorContext) -> Result<StepExecutorOutput> {
        match self.kind {
            SecurityStepKind::ScanCommitA => self.scan_commit_a(&context).await,
            SecurityStepKind::SuggestCommitA => self.suggest_commit_a(&context).await,
            SecurityStepKind::Reconciliation => self.reconciliation(&context).await,
            SecurityStepKind::ScheduledScanCommitB => self.scheduled_scan_commit_b(&context).await,
            SecurityStepKind::ListRunHistory => self.list_run_history(&context).await,
        }
    }

    async fn evaluate(
        &self,
        context: &StepExecutorContext,
        execution: &StepExecutorOutput,
        _assets: &[CapturedWorkflowAsset],
    ) -> Result<StepEvaluation> {
        let mut output = execution.evaluation.clone();
        if output.hard_gates.is_empty() {
            output.hard_gates.push(WorkflowGateResult {
                id: format!("{}_completed", context.node.id),
                passed: true,
                reason: "The deterministic security-scan step completed.".into(),
                evidence_ids: Vec::new(),
            });
        }
        Ok(output)
    }
}

impl SecurityExecutor {
    async fn scan_commit_a(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let mut result = StepExecutorOutput::default();
        let preflight = self
            .preflight_fixture(&operation_context(
                context,
                json!({"repository": REPOSITORY, "scheduled_ref": SCHEDULED_REF}),
                BTreeMap::new(),
            ))
            .await?;
        let repository = operation_output_string(&preflight, "repository")?;
        let commit_a = operation_output_string(&preflight, "commit_a")?;
        append_operation(&mut result, preflight, "contracts");

        let request = self
            .request_scan(&operation_context(
                context,
                json!({"mode": "scan", "expect_deduplicated": null}),
                typed_inputs([
                    ("repository", text_value(repository.clone())),
                    ("target_sha", text_value(commit_a.clone())),
                ]),
            ))
            .await?;
        let scan_run_id = operation_output_string(&request, "run_id")?;
        append_operation(&mut result, request, "request");

        let duplicate = self
            .request_scan(&operation_context(
                context,
                json!({"mode": "scan", "expect_deduplicated": true}),
                typed_inputs([
                    ("repository", text_value(repository.clone())),
                    ("target_sha", text_value(commit_a.clone())),
                    ("original_run_id", text_value(scan_run_id.clone())),
                ]),
            ))
            .await?;
        append_operation(&mut result, duplicate, "duplicate_request");

        let waited = self
            .wait_run(&operation_context(
                context,
                json!({"expected_mode": "scan", "timeout_seconds": 360, "poll_interval_ms": 500}),
                typed_inputs([
                    ("run_id", text_value(scan_run_id.clone())),
                    ("repository", text_value(repository.clone())),
                    ("target_sha", text_value(commit_a.clone())),
                ]),
            ))
            .await?;
        let report = operation_output_value(&waited, "report")?;
        let poll_metrics = waited.metrics.clone();
        append_operation(&mut result, waited, "run");

        let assessed = self
            .assess_report(&operation_context(
                context,
                json!({"mode": "scan", "seeded_paths": SEEDED_PATHS}),
                typed_inputs([("report", json_value(report.clone()))]),
            ))
            .await?;
        let findings_valid = operation_output_bool(&assessed, "findings_valid")?;
        let assessment = operation_output(&assessed, "assessment")?;
        let finding_count = report
            .get("findings")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let should_run_suggest = findings_valid && finding_count > 0;
        self.fixture.lock().suggest_expected = should_run_suggest;
        append_operation(&mut result, assessed, "report");

        let integrity = self
            .integrity(&operation_context(
                context,
                json!({"expected": "commit_a"}),
                BTreeMap::new(),
            ))
            .await?;
        append_operation(&mut result, integrity, "integrity");
        result.outputs = BTreeMap::from([
            ("repository".into(), text_value(repository)),
            ("commit_a".into(), text_value(commit_a)),
            ("scan_run_id".into(), text_value(scan_run_id)),
            ("report".into(), json_value(report)),
            ("should_run_suggest".into(), bool_value(should_run_suggest)),
            ("assessment".into(), assessment),
        ]);
        result.metrics = Some(json!({
            "request_count": 2,
            "finding_count": finding_count,
            "suggestion_branch_enabled": should_run_suggest,
            "poll": poll_metrics,
        }));
        Ok(result)
    }

    async fn suggest_commit_a(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let repository = input_string(context, "repository")?;
        let commit_a = input_string(context, "commit_a")?;
        let mut result = StepExecutorOutput::default();
        let request = self
            .request_scan(&operation_context(
                context,
                json!({"mode": "suggest", "expect_deduplicated": null}),
                typed_inputs([
                    ("repository", text_value(repository.clone())),
                    ("target_sha", text_value(commit_a.clone())),
                ]),
            ))
            .await?;
        let run_id = operation_output_string(&request, "run_id")?;
        append_operation(&mut result, request, "request");
        let waited = self
            .wait_run(&operation_context(
                context,
                json!({"expected_mode": "suggest", "timeout_seconds": 360, "poll_interval_ms": 500}),
                typed_inputs([
                    ("run_id", text_value(run_id.clone())),
                    ("repository", text_value(repository)),
                    ("target_sha", text_value(commit_a)),
                ]),
            ))
            .await?;
        let report = operation_output_value(&waited, "report")?;
        let poll_metrics = waited.metrics.clone();
        append_operation(&mut result, waited, "run");
        let assessed = self
            .assess_report(&operation_context(
                context,
                json!({"mode": "suggest", "seeded_paths": SEEDED_PATHS}),
                typed_inputs([("report", json_value(report.clone()))]),
            ))
            .await?;
        let assessment = operation_output(&assessed, "assessment")?;
        append_operation(&mut result, assessed, "report");
        let integrity = self
            .integrity(&operation_context(
                context,
                json!({"expected": "commit_a"}),
                BTreeMap::new(),
            ))
            .await?;
        append_operation(&mut result, integrity, "integrity");
        result.outputs = BTreeMap::from([
            ("run_id".into(), text_value(run_id)),
            ("report".into(), json_value(report)),
            ("assessment".into(), assessment),
        ]);
        result.metrics = Some(json!({"request_count": 1, "poll": poll_metrics}));
        Ok(result)
    }

    async fn reconciliation(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let run_id = input_string(context, "scan_run_id")?;
        let mut result = StepExecutorOutput::default();
        let cached = self
            .reconciliation_operation(&operation_context(
                context,
                json!({"refresh": false, "source": null, "severity": null, "limit": 100}),
                typed_inputs([("run_id", text_value(run_id.clone()))]),
            ))
            .await?;
        append_operation(&mut result, cached, "cached");
        let refreshed = self
            .reconciliation_operation(&operation_context(
                context,
                json!({"refresh": true, "source": null, "severity": null, "limit": 100}),
                typed_inputs([("run_id", text_value(run_id.clone()))]),
            ))
            .await?;
        let snapshot = operation_output_value(&refreshed, "snapshot")?;
        append_operation(&mut result, refreshed, "refreshed");
        let reread = self
            .reconciliation_operation(&operation_context(
                context,
                json!({"refresh": false, "source": null, "severity": null, "limit": 100}),
                typed_inputs([
                    ("run_id", text_value(run_id.clone())),
                    ("expected_snapshot", json_value(snapshot.clone())),
                ]),
            ))
            .await?;
        append_operation(&mut result, reread, "reread");
        let filtered = self
            .reconciliation_operation(&operation_context(
                context,
                json!({"refresh": false, "source": "dependabot", "severity": "high", "limit": 1}),
                typed_inputs([("run_id", text_value(run_id))]),
            ))
            .await?;
        append_operation(&mut result, filtered, "filtered");
        result.outputs = BTreeMap::from([("snapshot".into(), json_value(snapshot))]);
        result.metrics = Some(json!({"reconciliation_operations": 4}));
        Ok(result)
    }

    async fn scheduled_scan_commit_b(
        &self,
        context: &StepExecutorContext,
    ) -> Result<StepExecutorOutput> {
        let repository = input_string(context, "repository")?;
        let mut result = StepExecutorOutput::default();
        let created = self
            .create_scheduled_commit(&operation_context(
                context,
                json!({"scheduled_ref": SCHEDULED_REF}),
                BTreeMap::new(),
            ))
            .await?;
        let commit_b = operation_output_string(&created, "commit_b")?;
        append_operation(&mut result, created, "scheduled_commit");
        let waited = self
            .wait_scheduled(&operation_context(
                context,
                json!({"timeout_seconds": 180, "poll_interval_ms": 500}),
                typed_inputs([
                    ("repository", text_value(repository)),
                    ("target_sha", text_value(commit_b.clone())),
                ]),
            ))
            .await?;
        let run_id = operation_output_string(&waited, "run_id")?;
        let report = operation_output_value(&waited, "report")?;
        let poll_metrics = waited.metrics.clone();
        append_operation(&mut result, waited, "scheduled_run");
        let assessed = self
            .assess_report(&operation_context(
                context,
                json!({"mode": "scan", "seeded_paths": SEEDED_PATHS}),
                typed_inputs([("report", json_value(report.clone()))]),
            ))
            .await?;
        let assessment = operation_output(&assessed, "assessment")?;
        append_operation(&mut result, assessed, "report");
        let integrity = self
            .integrity(&operation_context(
                context,
                json!({"expected": "commit_b"}),
                BTreeMap::new(),
            ))
            .await?;
        append_operation(&mut result, integrity, "integrity");
        result.outputs = BTreeMap::from([
            ("run_id".into(), text_value(run_id)),
            ("commit_b".into(), text_value(commit_b)),
            ("report".into(), json_value(report)),
            ("assessment".into(), assessment),
        ]);
        result.metrics = Some(json!({"cron_poll": poll_metrics}));
        Ok(result)
    }

    async fn list_run_history(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let suggest_expected = self.fixture.lock().suggest_expected;
        let expected_modes = if suggest_expected {
            json!(["scan", "suggest"])
        } else {
            json!(["scan"])
        };
        let mut output = self
            .list(&operation_context(
                context,
                json!({
                    "repository": REPOSITORY,
                    "status": "completed",
                    "limit": 100,
                    "expected_count": if suggest_expected { 3 } else { 2 },
                    "expected_modes": expected_modes,
                    "expect_suggest": suggest_expected,
                }),
                BTreeMap::new(),
            ))
            .await?;
        output.metrics = Some(json!({
            "listed_run_count": output.outputs.get("runs").and_then(|value| value.value.get("runs")).and_then(Value::as_array).map_or(0, Vec::len),
            "suggestion_expected": suggest_expected,
        }));
        Ok(output)
    }

    async fn preflight_fixture(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let repository = config_string(context, "repository")?;
        let scheduled_ref = config_string(context, "scheduled_ref")?;
        let path = fixture_path()?;
        let head = git(&path, &["rev-parse", "HEAD"]).await?;
        validate_sha(&head)?;
        ensure_clean(&path).await?;
        if git_success(
            &path,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{scheduled_ref}"),
            ],
        )
        .await?
        {
            bail!("scheduled ref '{scheduled_ref}' must not exist before the workflow");
        }
        let info = self
            .context
            .trigger_value(
                "engine::functions::info",
                json!({"function_ids": [REQUEST_FUNCTION, READ_FUNCTION, LIST_FUNCTION, RECONCILIATION_FUNCTION]}),
            )
            .await?;
        validate_contract_info(&info)?;
        {
            let mut fixture = self.fixture.lock();
            fixture.path = Some(path.clone());
            fixture.initial_head = Some(head.clone());
            fixture.scheduled_ref = Some(scheduled_ref);
        }
        let contracts = info.get("functions").cloned().unwrap_or(Value::Null);
        Ok(output_with_asset(
            BTreeMap::from([
                ("repository".into(), text_value(repository)),
                ("commit_a".into(), text_value(head)),
                ("contracts".into(), json_value(contracts.clone())),
            ]),
            "preflight",
            contracts,
            &context.node.id,
        ))
    }

    async fn request_scan(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let repository = input_string(context, "repository")?;
        let target_sha = input_string(context, "target_sha")?;
        validate_sha(&target_sha)?;
        let mode = config_string(context, "mode")?;
        let expected_deduplicated = context
            .node
            .config
            .get("expect_deduplicated")
            .and_then(Value::as_bool);
        let response = self
            .context
            .trigger_value(
                REQUEST_FUNCTION,
                json!({"repository": repository, "target_sha": target_sha, "mode": mode}),
            )
            .await?;
        let run_id = required_string(&response, "run_id")?;
        let deduplicated = response
            .get("deduplicated")
            .and_then(Value::as_bool)
            .context("security-scan::request response is missing deduplicated")?;
        let original_run_id = context
            .inputs
            .get("original_run_id")
            .and_then(|value| value.value.as_str());
        let identity_matches = match (expected_deduplicated, original_run_id) {
            (Some(true), Some(original)) => original == run_id,
            (Some(true), None) => false,
            _ => true,
        };
        let deduplication_matches =
            expected_deduplicated.is_none_or(|expected| expected == deduplicated);
        let gate = WorkflowGateResult {
            id: "request_identity_and_deduplication".into(),
            passed: deduplication_matches && identity_matches,
            reason: format!(
                "Expected deduplicated={expected_deduplicated:?}; observed {deduplicated}; stable run identity={identity_matches}."
            ),
            evidence_ids: vec![format!("{}.request", context.node.id)],
        };
        Ok(output_with_internal_evaluation(
            output_with_asset(
                BTreeMap::from([
                    ("run_id".into(), text_value(run_id)),
                    ("deduplicated".into(), bool_value(deduplicated)),
                    ("response".into(), json_value(response.clone())),
                ]),
                "request",
                response,
                &context.node.id,
            ),
            vec![gate],
            Vec::new(),
        ))
    }

    async fn wait_run(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let run_id = input_string(context, "run_id")?;
        let repository = input_string(context, "repository")?;
        let target_sha = input_string(context, "target_sha")?;
        let expected_mode = config_string(context, "expected_mode")?;
        let timeout = Duration::from_secs(config_u64(context, "timeout_seconds")?);
        let interval = Duration::from_millis(config_u64(context, "poll_interval_ms")?);
        let started = Instant::now();
        let mut poll_count = 0_u64;
        loop {
            if *context.cancellation.borrow() {
                bail!("security-scan wait was cancelled");
            }
            let response = self
                .context
                .trigger_value(READ_FUNCTION, json!({"run_id": run_id}))
                .await?;
            poll_count += 1;
            let run = response
                .get("run")
                .filter(|value| !value.is_null())
                .cloned()
                .context("security-scan::read did not return the requested run")?;
            let status = required_string(&run, "status")?;
            if status == "completed" {
                let report = run
                    .get("report")
                    .cloned()
                    .context("completed security scan is missing report")?;
                let findings = report
                    .get("findings")
                    .and_then(Value::as_array)
                    .context("security report is missing findings[]")?;
                let has_findings = !findings.is_empty();
                let identity_valid = required_string(&run, "run_id")? == run_id
                    && required_string(&run, "repository")? == repository
                    && required_string(&run, "target_sha")? == target_sha
                    && required_string(&run, "mode")? == expected_mode;
                let mut output = output_with_internal_evaluation(
                    output_with_asset(
                        BTreeMap::from([
                            ("run".into(), json_value(run.clone())),
                            ("report".into(), json_value(report)),
                            ("has_findings".into(), bool_value(has_findings)),
                            ("run_id".into(), text_value(run_id)),
                        ]),
                        "run",
                        run,
                        &context.node.id,
                    ),
                    vec![gate(
                        "completed_run_identity",
                        identity_valid,
                        "Completed run retains the requested id, repository, full SHA and mode.",
                    )],
                    Vec::new(),
                );
                output.metrics = Some(json!({
                    "poll_count": poll_count,
                    "wait_duration_ms": started.elapsed().as_millis(),
                }));
                return Ok(output);
            }
            if matches!(status.as_str(), "failed" | "cancelled") {
                bail!(
                    "security scan run '{run_id}' ended as {status}: {}",
                    run.get("error").unwrap_or(&Value::Null)
                );
            }
            if started.elapsed() >= timeout {
                bail!(
                    "security scan run '{run_id}' did not complete within {}s",
                    timeout.as_secs()
                );
            }
            tokio::time::sleep(interval).await;
        }
    }

    async fn assess_report(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let report = input_value(context, "report")?.clone();
        let mode = config_string(context, "mode")?;
        let seeded_paths = context
            .node
            .config
            .get("seeded_paths")
            .and_then(Value::as_array)
            .context("seeded_paths must be an array")?
            .iter()
            .map(|value| value.as_str().context("seeded path must be a string"))
            .collect::<Result<BTreeSet<_>>>()?;
        let (gates, capability) = evaluate_report(
            &report,
            &mode,
            &seeded_paths,
            fixture_path().ok().as_deref(),
        )?;
        let findings_valid = gates.iter().all(|gate| gate.passed);
        let mut evaluations = vec![WorkflowEvaluationResult {
            id: "seeded_vulnerability_detection".into(),
            outcome: if capability.0 == capability.1 {
                WorkflowEvaluationOutcome::Passed
            } else {
                WorkflowEvaluationOutcome::Advisory
            },
            summary: format!(
                "Detected {} of {} explicitly seeded vulnerable paths.",
                capability.0, capability.1
            ),
            score: (capability.1 > 0).then(|| capability.0 as f64 / capability.1 as f64),
            evidence_ids: vec![format!("{}.report", context.node.id)],
        }];
        if mode == "suggest" {
            evaluations.push(
                evaluate_patch_applicability(&report, &self.fixture.path()?, &context.node.id)
                    .await?,
            );
        }
        let assessment_value = serde_json::to_value(if mode == "suggest" {
            evaluations.last().expect("suggest evaluation is present")
        } else {
            evaluations
                .first()
                .expect("detection evaluation is present")
        })?;
        Ok(output_with_internal_evaluation(
            output_with_asset(
                BTreeMap::from([
                    ("findings_valid".into(), bool_value(findings_valid)),
                    (
                        "assessment".into(),
                        TypedPortValue {
                            kind: PortValueKind::Assessment,
                            value: assessment_value,
                        },
                    ),
                    ("report".into(), json_value(report.clone())),
                ]),
                "report",
                report,
                &context.node.id,
            ),
            gates,
            evaluations,
        ))
    }

    async fn reconciliation_operation(
        &self,
        context: &StepExecutorContext,
    ) -> Result<StepExecutorOutput> {
        let run_id = input_string(context, "run_id")?;
        let request = json!({
            "run_id": run_id,
            "refresh": config_bool(context, "refresh")?,
            "source": context.node.config.get("source").cloned().unwrap_or(Value::Null),
            "severity": context.node.config.get("severity").cloned().unwrap_or(Value::Null),
            "limit": config_u64(context, "limit")?,
        });
        let snapshot = self
            .context
            .trigger_value(RECONCILIATION_FUNCTION, request)
            .await?;
        let mut gates = evaluate_reconciliation(&snapshot);
        gates.push(evaluate_reconciliation_filters(
            &snapshot,
            context.node.config.get("source").and_then(Value::as_str),
            context.node.config.get("severity").and_then(Value::as_str),
            config_u64(context, "limit")? as usize,
        ));
        if let Some(expected) = context.inputs.get("expected_snapshot") {
            gates.push(gate(
                "reconciliation_cache_stable",
                expected.value == snapshot,
                "A non-refresh reread returns the exact durable sanitized snapshot.",
            ));
        }
        let technical_failure = config_bool(context, "refresh")?
            .then(|| reconciliation_infrastructure_failure(&snapshot))
            .flatten();
        let mut output = output_with_internal_evaluation(
            output_with_asset(
                BTreeMap::from([("snapshot".into(), json_value(snapshot.clone()))]),
                "reconciliation",
                snapshot,
                &context.node.id,
            ),
            gates,
            Vec::new(),
        );
        output.technical_failure = technical_failure;
        Ok(output)
    }

    async fn list(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let repository = config_string(context, "repository")?;
        let status = context
            .node
            .config
            .get("status")
            .cloned()
            .unwrap_or(Value::Null);
        let response = self
            .context
            .trigger_value(
                LIST_FUNCTION,
                json!({
                    "repository": repository,
                    "status": status,
                    "limit": config_u64(context, "limit")?,
                }),
            )
            .await?;
        let runs = response
            .get("runs")
            .and_then(Value::as_array)
            .context("security-scan::list response is missing runs[]")?;
        let expected_count = config_u64(context, "expected_count")? as usize;
        let expected_modes = context.node.config["expected_modes"]
            .as_array()
            .context("expected_modes must be an array")?;
        let modes = runs
            .iter()
            .filter_map(|run| run.get("mode").and_then(Value::as_str))
            .collect::<BTreeSet<_>>();
        let expected_modes_present = expected_modes
            .iter()
            .filter_map(Value::as_str)
            .all(|mode| modes.contains(mode));
        let expected_status = status.as_str();
        let filters_valid = runs.len() <= config_u64(context, "limit")? as usize
            && runs.iter().all(|run| {
                run.get("repository").and_then(Value::as_str) == Some(repository.as_str())
                    && expected_status.is_none_or(|status| {
                        run.get("status").and_then(Value::as_str) == Some(status)
                    })
                    && run
                        .get("run_id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !id.is_empty())
            });
        let (commit_a, commit_b) = {
            let fixture = self.fixture.lock();
            (fixture.initial_head.clone(), fixture.scheduled_sha.clone())
        };
        let expect_suggest = config_bool(context, "expect_suggest")?;
        let expected_lifecycle_present =
            commit_a.zip(commit_b).is_some_and(|(commit_a, commit_b)| {
                let mut expected = vec![(commit_a.as_str(), "scan"), (commit_b.as_str(), "scan")];
                if expect_suggest {
                    expected.push((commit_a.as_str(), "suggest"));
                }
                expected.into_iter().all(|(sha, mode)| {
                    runs.iter().any(|run| {
                        run.get("target_sha").and_then(Value::as_str) == Some(sha)
                            && run.get("mode").and_then(Value::as_str) == Some(mode)
                    })
                })
            });
        let gates = vec![WorkflowGateResult {
            id: "list_filters_and_integrity".into(),
            passed: runs.len() >= expected_count
                && expected_modes_present
                && filters_valid
                && expected_lifecycle_present,
            reason: format!("Observed {} runs and modes {:?}.", runs.len(), modes),
            evidence_ids: vec![format!("{}.list", context.node.id)],
        }];
        Ok(output_with_internal_evaluation(
            output_with_asset(
                BTreeMap::from([("runs".into(), json_value(response.clone()))]),
                "list",
                response,
                &context.node.id,
            ),
            gates,
            Vec::new(),
        ))
    }

    async fn integrity(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let path = self.fixture.path()?;
        let head = git(&path, &["rev-parse", "HEAD"]).await?;
        let status = git(
            &path,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )
        .await?;
        let expected = config_string(context, "expected")?;
        let expected_sha = {
            let fixture = self.fixture.lock();
            match expected.as_str() {
                "commit_a" => fixture.initial_head.clone(),
                "commit_b" => fixture.scheduled_sha.clone(),
                _ => None,
            }
        }
        .context("expected fixture commit has not been materialized")?;
        let snapshot = json!({"head": head, "status": status, "expected": expected_sha});
        let gates = vec![WorkflowGateResult {
            id: "fixture_immutable".into(),
            passed: head == expected_sha && status.is_empty(),
            reason: format!(
                "Fixture HEAD is {head}; worktree status is bounded and empty={}",
                status.is_empty()
            ),
            evidence_ids: vec![format!("{}.integrity", context.node.id)],
        }];
        Ok(output_with_internal_evaluation(
            output_with_asset(
                BTreeMap::from([("snapshot".into(), json_value(snapshot.clone()))]),
                "integrity",
                snapshot,
                &context.node.id,
            ),
            gates,
            Vec::new(),
        ))
    }

    async fn create_scheduled_commit(
        &self,
        context: &StepExecutorContext,
    ) -> Result<StepExecutorOutput> {
        let path = self.fixture.path()?;
        ensure_clean(&path).await?;
        let scheduled_ref = config_string(context, "scheduled_ref")?;
        let marker = path.join("security-scan-e2e-scheduled.txt");
        std::fs::write(&marker, b"synthetic scheduled security scan fixture\n")
            .with_context(|| format!("write {}", marker.display()))?;
        git(&path, &["add", "--", "security-scan-e2e-scheduled.txt"]).await?;
        git(
            &path,
            &[
                "-c",
                "user.name=Harness E2E",
                "-c",
                "user.email=harness-e2e@example.invalid",
                "commit",
                "-m",
                "test: add scheduled security fixture",
            ],
        )
        .await?;
        let commit_b = git(&path, &["rev-parse", "HEAD"]).await?;
        validate_sha(&commit_b)?;
        git(
            &path,
            &[
                "update-ref",
                &format!("refs/heads/{scheduled_ref}"),
                &commit_b,
            ],
        )
        .await?;
        {
            let mut fixture = self.fixture.lock();
            fixture.scheduled_ref = Some(scheduled_ref);
            fixture.scheduled_sha = Some(commit_b.clone());
        }
        Ok(output_with_asset(
            BTreeMap::from([("commit_b".into(), text_value(commit_b.clone()))]),
            "scheduled-commit",
            json!({"commit_b": commit_b}),
            &context.node.id,
        ))
    }

    async fn wait_scheduled(&self, context: &StepExecutorContext) -> Result<StepExecutorOutput> {
        let repository = input_string(context, "repository")?;
        let target_sha = input_string(context, "target_sha")?;
        validate_sha(&target_sha)?;
        let timeout = Duration::from_secs(config_u64(context, "timeout_seconds")?);
        let interval = Duration::from_millis(config_u64(context, "poll_interval_ms")?);
        let started = Instant::now();
        let mut poll_count = 0_u64;
        loop {
            if *context.cancellation.borrow() {
                bail!("scheduled security-scan wait was cancelled");
            }
            let listed = self
                .context
                .trigger_value(
                    LIST_FUNCTION,
                    json!({"repository": repository, "limit": 100}),
                )
                .await?;
            poll_count += 1;
            let scheduled = listed
                .get("runs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|run| {
                    run.get("target_sha").and_then(Value::as_str) == Some(target_sha.as_str())
                        && run.get("mode").and_then(Value::as_str) == Some("scan")
                });
            if let Some(summary) = scheduled {
                let run_id = required_string(summary, "run_id")?;
                let read = self
                    .context
                    .trigger_value(READ_FUNCTION, json!({"run_id": run_id}))
                    .await?;
                if let Some(run) = read
                    .get("run")
                    .filter(|run| run.get("status").and_then(Value::as_str) == Some("completed"))
                {
                    let report = run
                        .get("report")
                        .cloned()
                        .context("completed scheduled run is missing report")?;
                    let evidence = json!({"listed": summary, "run": run});
                    let mut output = output_with_internal_evaluation(
                        output_with_asset(
                            BTreeMap::from([
                                ("run_id".into(), text_value(run_id)),
                                ("run".into(), json_value(run.clone())),
                                ("report".into(), json_value(report)),
                            ]),
                            "scheduled-run",
                            evidence,
                            &context.node.id,
                        ),
                        vec![gate(
                            "cron_created_exact_sha",
                            true,
                            "A cron-created scan completed for commit B without a manual request node.",
                        )],
                        Vec::new(),
                    );
                    output.metrics = Some(json!({
                        "poll_count": poll_count,
                        "wait_duration_ms": started.elapsed().as_millis(),
                    }));
                    return Ok(output);
                }
            }
            if started.elapsed() >= timeout {
                bail!(
                    "cron did not create and complete a scan for SHA {target_sha} within {}s",
                    timeout.as_secs()
                );
            }
            tokio::time::sleep(interval).await;
        }
    }
}

impl FixtureState {
    fn lock(&self) -> std::sync::MutexGuard<'_, FixtureStateInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn path(&self) -> Result<PathBuf> {
        self.lock()
            .path
            .clone()
            .context("fixture preflight has not run")
    }

    async fn restore(&self) -> Result<()> {
        let (path, initial, scheduled_ref) = {
            let fixture = self.lock();
            (
                fixture.path.clone(),
                fixture.initial_head.clone(),
                fixture.scheduled_ref.clone(),
            )
        };
        let (Some(path), Some(initial)) = (path, initial) else {
            return Ok(());
        };
        let current = git(&path, &["rev-parse", "HEAD"]).await?;
        if current != initial {
            git(&path, &["reset", "--hard", &initial]).await?;
        }
        if let Some(reference) = scheduled_ref {
            let _ = git(
                &path,
                &["update-ref", "-d", &format!("refs/heads/{reference}")],
            )
            .await;
        }
        let marker = path.join("security-scan-e2e-scheduled.txt");
        if marker.exists() {
            std::fs::remove_file(&marker)
                .with_context(|| format!("remove {}", marker.display()))?;
        }
        ensure_clean(&path).await
    }
}

fn evaluate_report(
    report: &Value,
    mode: &str,
    seeded_paths: &BTreeSet<&str>,
    fixture_path: Option<&Path>,
) -> Result<(Vec<WorkflowGateResult>, (usize, usize))> {
    let assessments = report.get("assessments").and_then(Value::as_object);
    let coverage_valid = assessments.is_some_and(|assessments| {
        ["vulnerabilities", "dependencies", "secrets", "supply_chain"]
            .iter()
            .all(|area| {
                assessments
                    .get(*area)
                    .and_then(|value| value.get("status"))
                    .and_then(Value::as_str)
                    .is_some_and(|status| matches!(status, "assessed" | "not_assessed"))
            })
    });
    let findings = report
        .get("findings")
        .and_then(Value::as_array)
        .context("security report is missing findings[]")?;
    let mut paths_valid = true;
    let mut privacy_valid = true;
    let mut patch_policy_valid = true;
    let mut detected = BTreeSet::new();
    for finding in findings {
        if mode == "scan"
            && finding
                .get("suggested_patch")
                .is_some_and(|value| !value.is_null())
        {
            patch_policy_valid = false;
        }
        if contains_forbidden_key(finding) || contains_sensitive_string(finding, fixture_path) {
            privacy_valid = false;
        }
        if let Some(location) = finding.get("location").filter(|value| !value.is_null()) {
            let path = location
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if path.is_empty()
                || Path::new(path).is_absolute()
                || path.split('/').any(|part| part == "..")
            {
                paths_valid = false;
            }
            let start = location.get("line_start").and_then(Value::as_u64);
            let end = location.get("line_end").and_then(Value::as_u64);
            if start == Some(0)
                || end == Some(0)
                || start.zip(end).is_some_and(|(start, end)| end < start)
            {
                paths_valid = false;
            }
            if seeded_paths.contains(path) {
                detected.insert(path);
            }
        }
    }
    Ok((
        vec![
            gate(
                "security_area_coverage",
                coverage_valid,
                "All four security areas are explicitly assessed or not_assessed.",
            ),
            gate(
                "report_paths_and_lines",
                paths_valid,
                "Finding paths are relative and line ranges are valid.",
            ),
            gate(
                "public_report_privacy",
                privacy_valid,
                "Public output excludes internal roots, session ids and operation nonces.",
            ),
            gate(
                "mode_patch_policy",
                patch_policy_valid,
                "Scan mode does not contain suggested patches.",
            ),
        ],
        (detected.len(), seeded_paths.len()),
    ))
}

async fn evaluate_patch_applicability(
    report: &Value,
    fixture_path: &Path,
    step_id: &str,
) -> Result<WorkflowEvaluationResult> {
    let patches = report
        .get("findings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|finding| finding.get("suggested_patch").and_then(Value::as_str))
        .filter(|patch| !patch.trim().is_empty())
        .collect::<Vec<_>>();
    if patches.is_empty() {
        return Ok(WorkflowEvaluationResult {
            id: "suggested_patch_applicability".into(),
            outcome: WorkflowEvaluationOutcome::NotEvaluated,
            summary: "Suggest mode produced no optional patch to check.".into(),
            score: None,
            evidence_ids: vec![format!("{step_id}.report")],
        });
    }

    let scratch =
        std::env::temp_dir().join(format!("harness-e2e-patch-check-{}", uuid::Uuid::new_v4()));
    let disposable = scratch.join("fixture");
    std::fs::create_dir(&scratch)
        .with_context(|| format!("create disposable patch root {}", scratch.display()))?;
    let disposable_text = disposable.to_string_lossy().into_owned();
    if let Err(error) = git(
        fixture_path,
        &["worktree", "add", "--detach", &disposable_text, "HEAD"],
    )
    .await
    {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err(error.context("create disposable worktree for suggested patch checks"));
    }

    let mut applicable = 0_usize;
    let mut check_error = None;
    for (index, patch) in patches.iter().enumerate() {
        let patch_path = scratch.join(format!("candidate-{index}.patch"));
        if let Err(error) = std::fs::write(&patch_path, patch.as_bytes()) {
            check_error = Some(anyhow::Error::from(error).context(format!(
                "write disposable patch candidate {}",
                patch_path.display()
            )));
            break;
        }
        let patch_text = patch_path.to_string_lossy().into_owned();
        match git_success(&disposable, &["apply", "--check", &patch_text]).await {
            Ok(true) => applicable += 1,
            Ok(false) => {}
            Err(error) => {
                check_error = Some(error.context("run git apply --check"));
                break;
            }
        }
    }

    let remove_result = git(
        fixture_path,
        &["worktree", "remove", "--force", &disposable_text],
    )
    .await;
    let filesystem_cleanup = std::fs::remove_dir_all(&scratch);
    remove_result.context("remove disposable patch-check worktree")?;
    if let Err(error) = filesystem_cleanup {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error).context("remove disposable patch-check files");
        }
    }
    if let Some(error) = check_error {
        return Err(error);
    }

    Ok(WorkflowEvaluationResult {
        id: "suggested_patch_applicability".into(),
        outcome: if applicable == patches.len() {
            WorkflowEvaluationOutcome::Passed
        } else {
            WorkflowEvaluationOutcome::Advisory
        },
        summary: format!(
            "{applicable} of {} optional suggested patches passed git apply --check in a disposable worktree.",
            patches.len()
        ),
        score: Some(applicable as f64 / patches.len() as f64),
        evidence_ids: vec![format!("{step_id}.report")],
    })
}

fn evaluate_reconciliation(snapshot: &Value) -> Vec<WorkflowGateResult> {
    let sources = snapshot.get("sources").and_then(Value::as_array);
    let records = snapshot.get("records").and_then(Value::as_array);
    let scopes_valid = sources.is_some_and(|sources| {
        let scopes = sources
            .iter()
            .filter_map(|source| {
                Some((
                    source.get("source")?.as_str()?,
                    source.get("scope")?.as_str()?,
                ))
            })
            .collect::<BTreeMap<_, _>>();
        scopes.get("dependabot") == Some(&"repository_default_branch")
            && scopes.get("code_scanning") == Some(&"repository_snapshot")
            && snapshot
                .get("harness")
                .and_then(|value| value.get("scope"))
                .and_then(Value::as_str)
                == Some("exact_commit")
    });
    let records_valid = records.is_some_and(|records| {
        records.iter().all(|record| {
            record
                .get("public_url")
                .and_then(Value::as_str)
                .is_some_and(|url| url.starts_with("https://github.com/"))
                && !contains_forbidden_key(record)
        })
    });
    let counts_valid = sources.is_some_and(|sources| {
        sources.iter().all(|source| {
            let status = source
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let count = source.get("record_count");
            match status {
                "complete" | "partial" => count.is_some_and(Value::is_u64),
                "unavailable"
                | "authentication_required"
                | "permission_denied"
                | "disabled"
                | "not_configured"
                | "not_collected" => count.is_none_or(Value::is_null),
                _ => false,
            }
        })
    });
    vec![
        gate(
            "reconciliation_scopes",
            scopes_valid,
            "Harness and GitHub sources retain distinct scopes.",
        ),
        gate(
            "reconciliation_records",
            records_valid,
            "Reconciliation records use sanitized public GitHub URLs.",
        ),
        gate(
            "reconciliation_counts",
            counts_valid,
            "Source counts are nullable only when collection produced no usable data.",
        ),
    ]
}

fn evaluate_reconciliation_filters(
    snapshot: &Value,
    source: Option<&str>,
    severity: Option<&str>,
    limit: usize,
) -> WorkflowGateResult {
    let records = snapshot.get("records").and_then(Value::as_array);
    let valid = records.is_some_and(|records| {
        records.len() <= limit
            && records.iter().all(|record| {
                source.is_none_or(|expected| {
                    record.get("source").and_then(Value::as_str) == Some(expected)
                }) && severity.is_none_or(|expected| {
                    record.get("severity").and_then(Value::as_str) == Some(expected)
                })
            })
            && snapshot.get("next_cursor").is_none_or(|cursor| {
                cursor.is_null() || cursor.as_str().is_some_and(|value| !value.is_empty())
            })
    });
    gate(
        "reconciliation_filters_and_pagination",
        valid,
        "Filtered records respect source, severity, bounded page size and cursor shape.",
    )
}

fn reconciliation_infrastructure_failure(snapshot: &Value) -> Option<String> {
    let unavailable = snapshot
        .get("sources")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|source| {
            let status = source.get("status")?.as_str()?;
            (!matches!(status, "complete" | "partial")).then(|| {
                format!(
                    "{}={status}",
                    source
                        .get("source")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                )
            })
        })
        .collect::<Vec<_>>();
    (!unavailable.is_empty()).then(|| {
        format!(
            "GitHub reconciliation infrastructure did not collect every source: {}",
            unavailable.join(", ")
        )
    })
}

fn gate(id: &str, passed: bool, reason: &str) -> WorkflowGateResult {
    WorkflowGateResult {
        id: id.into(),
        passed,
        reason: reason.into(),
        evidence_ids: Vec::new(),
    }
}

fn contains_forbidden_key(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            matches!(
                key.as_str(),
                "operation_nonce" | "session_id" | "worktree_id"
            ) || contains_forbidden_key(value)
        }),
        Value::Array(values) => values.iter().any(contains_forbidden_key),
        _ => false,
    }
}

fn contains_sensitive_string(value: &Value, fixture_path: Option<&Path>) -> bool {
    match value {
        Value::String(text) => fixture_path
            .and_then(Path::to_str)
            .is_some_and(|root| !root.is_empty() && text.contains(root)),
        Value::Array(values) => values
            .iter()
            .any(|value| contains_sensitive_string(value, fixture_path)),
        Value::Object(values) => values
            .values()
            .any(|value| contains_sensitive_string(value, fixture_path)),
        _ => false,
    }
}

fn output_with_asset(
    outputs: BTreeMap<String, TypedPortValue>,
    asset_id: &str,
    value: Value,
    step_id: &str,
) -> StepExecutorOutput {
    StepExecutorOutput {
        outputs,
        captured_assets: vec![CapturedWorkflowAsset {
            id: asset_id.into(),
            kind: "security_scan_evidence".into(),
            media_type: "application/json".into(),
            content: WorkflowAssetContent::Json(value),
            provenance: vec![WorkflowProvenance {
                source_step_id: step_id.into(),
                relation: "captured_before_cleanup".into(),
            }],
        }],
        ..StepExecutorOutput::default()
    }
}

fn output_with_internal_evaluation(
    mut output: StepExecutorOutput,
    mut gates: Vec<WorkflowGateResult>,
    mut evaluations: Vec<WorkflowEvaluationResult>,
) -> StepExecutorOutput {
    if let Some((asset, provenance)) = output.captured_assets.first().and_then(|asset| {
        asset
            .provenance
            .first()
            .map(|provenance| (asset, provenance))
    }) {
        let evidence_id = format!("{}.{}", provenance.source_step_id, asset.id);
        for gate in &mut gates {
            if gate.evidence_ids.is_empty() {
                gate.evidence_ids.push(evidence_id.clone());
            }
        }
        for evaluation in &mut evaluations {
            if evaluation.evidence_ids.is_empty() {
                evaluation.evidence_ids.push(evidence_id.clone());
            }
        }
    }
    output.evaluation = StepEvaluation {
        hard_gates: gates,
        evaluations,
    };
    output
}

fn operation_context(
    context: &StepExecutorContext,
    config: Value,
    inputs: BTreeMap<String, TypedPortValue>,
) -> StepExecutorContext {
    let mut operation = context.clone();
    operation.node.config = config;
    operation.inputs = inputs;
    operation
}

fn typed_inputs<const N: usize>(
    inputs: [(&str, TypedPortValue); N],
) -> BTreeMap<String, TypedPortValue> {
    inputs
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn operation_output(output: &StepExecutorOutput, key: &str) -> Result<TypedPortValue> {
    output
        .outputs
        .get(key)
        .cloned()
        .with_context(|| format!("security review operation did not produce '{key}'"))
}

fn operation_output_string(output: &StepExecutorOutput, key: &str) -> Result<String> {
    operation_output(output, key)?
        .value
        .as_str()
        .map(str::to_string)
        .with_context(|| format!("security review operation output '{key}' is not text"))
}

fn operation_output_bool(output: &StepExecutorOutput, key: &str) -> Result<bool> {
    operation_output(output, key)?
        .value
        .as_bool()
        .with_context(|| format!("security review operation output '{key}' is not boolean"))
}

fn operation_output_value(output: &StepExecutorOutput, key: &str) -> Result<Value> {
    Ok(operation_output(output, key)?.value)
}

fn append_operation(
    target: &mut StepExecutorOutput,
    mut operation: StepExecutorOutput,
    asset_id: &str,
) {
    for asset in &mut operation.captured_assets {
        let previous = asset.id.clone();
        asset.id = asset_id.to_string();
        let previous_evidence = asset
            .provenance
            .first()
            .map(|provenance| format!("{}.{}", provenance.source_step_id, previous));
        let current_evidence = asset
            .provenance
            .first()
            .map(|provenance| format!("{}.{}", provenance.source_step_id, asset_id));
        if let (Some(previous), Some(current)) = (previous_evidence, current_evidence) {
            for gate in &mut operation.evaluation.hard_gates {
                for evidence in &mut gate.evidence_ids {
                    if evidence == &previous {
                        *evidence = current.clone();
                    }
                }
            }
            for evaluation in &mut operation.evaluation.evaluations {
                for evidence in &mut evaluation.evidence_ids {
                    if evidence == &previous {
                        *evidence = current.clone();
                    }
                }
            }
        }
    }
    for gate in &mut operation.evaluation.hard_gates {
        gate.id = format!("{asset_id}.{}", gate.id);
    }
    for evaluation in &mut operation.evaluation.evaluations {
        evaluation.id = format!("{asset_id}.{}", evaluation.id);
    }
    target.captured_assets.extend(operation.captured_assets);
    target
        .evaluation
        .hard_gates
        .extend(operation.evaluation.hard_gates);
    target
        .evaluation
        .evaluations
        .extend(operation.evaluation.evaluations);
    if target.technical_failure.is_none() {
        target.technical_failure = operation.technical_failure;
    }
}

fn text_value(value: impl Into<String>) -> TypedPortValue {
    TypedPortValue {
        kind: PortValueKind::TextUtf8,
        value: Value::String(value.into()),
    }
}

fn bool_value(value: bool) -> TypedPortValue {
    TypedPortValue {
        kind: PortValueKind::Boolean,
        value: Value::Bool(value),
    }
}

fn json_value(value: Value) -> TypedPortValue {
    TypedPortValue {
        kind: PortValueKind::Json,
        value,
    }
}

fn config_string(context: &StepExecutorContext, key: &str) -> Result<String> {
    required_string(&context.node.config, key)
}

fn config_bool(context: &StepExecutorContext, key: &str) -> Result<bool> {
    context
        .node
        .config
        .get(key)
        .and_then(Value::as_bool)
        .with_context(|| {
            format!(
                "node '{}' config is missing boolean '{key}'",
                context.node.id
            )
        })
}

fn config_u64(context: &StepExecutorContext, key: &str) -> Result<u64> {
    context
        .node
        .config
        .get(key)
        .and_then(Value::as_u64)
        .with_context(|| {
            format!(
                "node '{}' config is missing integer '{key}'",
                context.node.id
            )
        })
}

fn input_string(context: &StepExecutorContext, key: &str) -> Result<String> {
    context
        .inputs
        .get(key)
        .and_then(|value| value.value.as_str())
        .map(str::to_string)
        .with_context(|| format!("node '{}' input is missing string '{key}'", context.node.id))
}

fn input_value<'a>(context: &'a StepExecutorContext, key: &str) -> Result<&'a Value> {
    context
        .inputs
        .get(key)
        .map(|value| &value.value)
        .with_context(|| format!("node '{}' input is missing '{key}'", context.node.id))
}

fn required_string(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .with_context(|| format!("response is missing non-empty string '{key}'"))
}

fn validate_sha(sha: &str) -> Result<()> {
    if sha.len() != 40 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("'{sha}' is not a full 40-character Git SHA");
    }
    Ok(())
}

fn fixture_path() -> Result<PathBuf> {
    let path = std::env::var_os(FIXTURE_PATH_ENV)
        .map(PathBuf::from)
        .with_context(|| format!("{FIXTURE_PATH_ENV} must point to the launcher-created clone"))?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("canonicalize fixture clone {}", path.display()))?;
    if !canonical.join(".git").exists() {
        bail!(
            "fixture path {} is not a standalone Git clone",
            canonical.display()
        );
    }
    Ok(canonical)
}

async fn ensure_clean(path: &Path) -> Result<()> {
    let status = git(path, &["status", "--porcelain=v1", "--untracked-files=all"]).await?;
    if !status.is_empty() {
        bail!("fixture clone is not clean: {status}");
    }
    Ok(())
}

async fn git(path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .stdin(Stdio::null())
        .output()
        .await
        .with_context(|| format!("run git {} in {}", args.join(" "), path.display()))?;
    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

async fn git_success(path: &Path, args: &[&str]) -> Result<bool> {
    Ok(Command::new("git")
        .args(args)
        .current_dir(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?
        .success())
}

fn validate_contract_info(info: &Value) -> Result<()> {
    let functions = info
        .get("functions")
        .and_then(Value::as_array)
        .context("engine::functions::info response is missing functions[]")?;
    for required in [
        REQUEST_FUNCTION,
        READ_FUNCTION,
        LIST_FUNCTION,
        RECONCILIATION_FUNCTION,
    ] {
        let function = functions
            .iter()
            .find(|function| function.get("function_id").and_then(Value::as_str) == Some(required))
            .with_context(|| format!("functions::info omitted '{required}'"))?;
        if !function.get("request_schema").is_some_and(Value::is_object)
            || !function
                .get("response_schema")
                .is_some_and(Value::is_object)
        {
            bail!("function '{required}' does not expose exact request/response JSON Schemas");
        }
        let expected = required_contract(required);
        let request_hash = crate::artifact::sha256_value(&function["request_schema"])?;
        let response_hash = crate::artifact::sha256_value(&function["response_schema"])?;
        if expected.request_schema_sha256.as_deref() != Some(request_hash.as_str())
            || expected.response_schema_sha256.as_deref() != Some(response_hash.as_str())
        {
            bail!("function '{required}' contract differs from the security-scan E2E v1 contract");
        }
    }
    Ok(())
}

fn object_schema(properties: &[(&str, Value)], required: &[&str]) -> Value {
    let properties = properties
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect::<Map<_, _>>();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn port(
    kind: PortValueKind,
    optional: bool,
    control_source: Option<ControlSource>,
) -> StepPortDescriptor {
    StepPortDescriptor {
        kind,
        optional,
        control_source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_report_rejects_unknown_coverage_absolute_paths_and_patches() {
        let report = json!({
            "summary": "bad",
            "assessments": {
                "vulnerabilities": {"status": "unknown"},
                "dependencies": {"status": "assessed"},
                "secrets": {"status": "assessed"},
                "supply_chain": {"status": "assessed"}
            },
            "findings": [{
                "location": {"path": "/tmp/repo/src/main.rs", "line_start": 0},
                "suggested_patch": "patch"
            }]
        });
        let (gates, _) = evaluate_report(
            &report,
            "scan",
            &BTreeSet::new(),
            Some(Path::new("/tmp/repo")),
        )
        .unwrap();
        assert!(gates.iter().all(|gate| !gate.passed));
    }

    #[test]
    fn descriptor_catalog_contains_no_configurable_function_ids() {
        for descriptor in descriptors_only() {
            let encoded = descriptor.config_schema.to_string();
            assert!(!encoded.contains("function_id"));
            descriptor.validate().unwrap();
        }
    }

    #[test]
    fn unavailable_reconciliation_sources_require_null_counts() {
        let snapshot = json!({
            "harness": {"scope": "exact_commit"},
            "sources": [
                {
                    "source": "dependabot",
                    "scope": "repository_default_branch",
                    "status": "unavailable",
                    "record_count": 3
                },
                {
                    "source": "code_scanning",
                    "scope": "repository_snapshot",
                    "status": "complete",
                    "record_count": 0
                }
            ],
            "records": []
        });
        let gates = evaluate_reconciliation(&snapshot);
        assert!(
            !gates
                .iter()
                .find(|gate| gate.id == "reconciliation_counts")
                .unwrap()
                .passed
        );
        assert!(reconciliation_infrastructure_failure(&snapshot)
            .unwrap()
            .contains("dependabot=unavailable"));

        let mut valid = snapshot;
        valid["sources"][0]["record_count"] = Value::Null;
        let gates = evaluate_reconciliation(&valid);
        assert!(gates.iter().all(|gate| gate.passed));
        assert!(reconciliation_infrastructure_failure(&valid).is_some());
        valid["sources"][0]["status"] = Value::String("complete".into());
        valid["sources"][0]["record_count"] = Value::from(0);
        assert!(reconciliation_infrastructure_failure(&valid).is_none());
    }

    #[tokio::test]
    async fn suggested_patches_are_checked_in_a_disposable_worktree() {
        let fixture = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("file.txt"), "old\n").unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["add", "file.txt"],
            vec![
                "-c",
                "user.name=Harness E2E",
                "-c",
                "user.email=harness-e2e@example.invalid",
                "commit",
                "-qm",
                "fixture",
            ],
        ] {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(fixture.path())
                .status()
                .unwrap()
                .success());
        }
        let report = json!({
            "findings": [{
                "suggested_patch": "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+new\n"
            }]
        });
        let evaluation = evaluate_patch_applicability(&report, fixture.path(), "assess")
            .await
            .unwrap();
        assert_eq!(evaluation.outcome, WorkflowEvaluationOutcome::Passed);
        assert_eq!(
            std::fs::read_to_string(fixture.path().join("file.txt")).unwrap(),
            "old\n"
        );
        assert!(git(fixture.path(), &["status", "--porcelain"])
            .await
            .unwrap()
            .is_empty());
    }

    #[test]
    fn full_workflow_is_valid_against_the_registered_catalog() {
        let mut catalog = StepCatalog::new();
        for descriptor in descriptors_only() {
            catalog.register_descriptor(descriptor).unwrap();
        }
        let definition = definition();
        let materialized = definition.validate(&catalog).unwrap();
        assert_eq!(materialized.definition.nodes.len(), 5);
        assert!(materialized
            .definition
            .nodes
            .iter()
            .any(|node| node.id == "scheduled_scan_commit_b"));
    }
}
