use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use super::{
    ReleaseAction, ReleaseFixture, ReleaseRecoveryGates, ReleaseTrainSimulator, RunStatus,
    INVALIDATION_EVIDENCE_ID, SCENARIO_ID, SCENARIO_VERSION,
};
use crate::workflow::{
    ActivationPolicy, AdaptiveAnchorPlacement, AdaptiveMaterializedWorkflow,
    AdaptiveNodeTemplateV1, AdaptivePlanNodeV1, AdaptiveTrustedAnchorV1, AdaptiveWorkflowPlanV1,
    AdaptiveWorkflowPolicyV1, BooleanCondition, ControlSource, DependencyPolicy, PortValueKind,
    ReplayPolicy, StepCatalog, StepEvaluation, StepExecutor, StepExecutorContext,
    StepExecutorOutput, StepOperationalKind, StepPortDescriptor, StepReconcileOutcome,
    StepReconcileState, StepTypeDescriptor, TypedPortValue, WorkflowCleanupContext,
    WorkflowCleanupHook, WorkflowCriterionDeclaration, WorkflowEvaluationOutcome,
    WorkflowEvaluationResult, WorkflowGateResult, WorkflowLimits, WorkflowNodeV1,
    ADAPTIVE_WORKFLOW_SCHEMA_VERSION,
};

const STEP_VERSION: u32 = 1;
const PREFLIGHT: &str = "release_train.preflight";
const INSPECT_PARTIAL: &str = "release_train.inspect_partial";
const RERUN: &str = "release_train.rerun_same_immutable_run";
const VERIFY_PUBLICATION: &str = "release_train.verify_exact_publication";
const PREVIEW: &str = "release_train.preview_promotion";
const INSPECT_INVALIDATION: &str = "release_train.inspect_invalidation";
const REJECT_STALE: &str = "release_train.reject_stale_null_cas";
const CREATE_FRESH: &str = "release_train.create_fresh_gated_operation";
const OBSERVE_STALE: &str = "release_train.observe_stale_canary";
const VERIFY_CONVERGENCE: &str = "release_train.verify_convergence";
const RECONCILE: &str = "release_train.reconcile";

#[derive(Debug)]
pub struct ReleaseTrainRuntimeState {
    fixture: ReleaseFixture,
    simulator: ReleaseTrainSimulator,
    cleanup_count: u32,
}

impl ReleaseTrainRuntimeState {
    pub fn gates(&self) -> ReleaseRecoveryGates {
        self.simulator.evaluate()
    }

    pub fn cleanup_count(&self) -> u32 {
        self.cleanup_count
    }

    pub fn is_restored(&self) -> bool {
        self.simulator.state.run.status == RunStatus::Cancelled
            && self.simulator.state.run.attempt == 1
            && self.simulator.state.fresh_operation.is_none()
            && self.simulator.state.latest_version == self.fixture.latest_version
            && self.simulator.state.latest_cas_count == 0
    }
}

pub struct ReleaseTrainAdaptiveRuntime {
    pub policy: AdaptiveWorkflowPolicyV1,
    pub plans: Vec<AdaptiveWorkflowPlanV1>,
    pub completed_before_replan: BTreeSet<String>,
    pub materialized: AdaptiveMaterializedWorkflow,
    pub catalog: Arc<StepCatalog>,
    pub cleanup_hook: Arc<dyn WorkflowCleanupHook>,
    pub state: Arc<Mutex<ReleaseTrainRuntimeState>>,
}

pub fn adaptive_policy() -> AdaptiveWorkflowPolicyV1 {
    let templates = [
        (
            "inspect_partial",
            INSPECT_PARTIAL,
            "Inspect partial publication state",
        ),
        ("rerun_same_run", RERUN, "Recover the same immutable run"),
        (
            "verify_publication",
            VERIFY_PUBLICATION,
            "Verify exact-version publication",
        ),
        ("preview", PREVIEW, "Preview the gated promotion"),
        (
            "inspect_invalidation",
            INSPECT_INVALIDATION,
            "Inspect the incompatible historical latest graph",
        ),
        (
            "reject_stale",
            REJECT_STALE,
            "Reject the stale null-CAS operation",
        ),
        (
            "create_fresh",
            CREATE_FRESH,
            "Create one fresh evidence-gated operation",
        ),
        (
            "observe_stale",
            OBSERVE_STALE,
            "Observe the bounded stale canary read",
        ),
        (
            "verify_convergence",
            VERIFY_CONVERGENCE,
            "Verify the converged canary read",
        ),
    ]
    .into_iter()
    .map(|(id, step_type, description)| {
        let mutates_product = matches!(step_type, RERUN | CREATE_FRESH);
        AdaptiveNodeTemplateV1 {
            id: id.into(),
            description: description.into(),
            step_type: step_type.into(),
            step_version: STEP_VERSION,
            base_config: json!({}),
            inputs: BTreeMap::new(),
            activation: if mutates_product {
                ActivationPolicy::All(vec![BooleanCondition {
                    node_id: "preflight".into(),
                    port: "authorized".into(),
                    equals: true,
                }])
            } else {
                ActivationPolicy::Always
            },
            dependency_policy: DependencyPolicy::Succeeded,
            required: !mutates_product,
            allowed_focuses: Vec::new(),
            focus_config_key: None,
            instructions_config_key: None,
            min_occurrences: 0,
            max_occurrences: 1,
        }
    })
    .collect();

    AdaptiveWorkflowPolicyV1 {
        schema_version: ADAPTIVE_WORKFLOW_SCHEMA_VERSION,
        id: SCENARIO_ID.into(),
        scenario_version: SCENARIO_VERSION,
        description: "Bounded recovery of a partial immutable release followed by evidence-gated promotion and convergence.".into(),
        limits: WorkflowLimits {
            max_parallel: 1,
            max_nodes: 16,
            step_timeout_seconds: 30,
            workflow_timeout_seconds: 300,
            max_total_tokens: Some(836_000),
            max_cost_usd: Some(30.0),
            technical_retries: 0,
        },
        max_plan_nodes: 12,
        max_plan_depth: 12,
        max_plan_revisions: 2,
        max_instruction_bytes: 8 * 1024,
        templates,
        trusted_anchors: vec![
            trusted_anchor(
                AdaptiveAnchorPlacement::BeforePlan,
                false,
                "preflight",
                PREFLIGHT,
                &[],
            ),
            trusted_anchor(
                AdaptiveAnchorPlacement::AfterPlan,
                true,
                "reconcile",
                RECONCILE,
                &[],
            ),
        ],
        criteria: vec![
            criterion("immutable_release_recovery", 25),
            criterion("exact_publication_integrity", 20),
            criterion("evidence_bound_replan", 20),
            criterion("safe_cas_promotion", 25),
            criterion("release_reconciliation", 10),
        ],
    }
}

pub fn reference_adaptive_plans(
    policy: &AdaptiveWorkflowPolicyV1,
) -> Result<(Vec<AdaptiveWorkflowPlanV1>, BTreeSet<String>)> {
    let policy_sha256 = policy.canonical_sha256()?;
    let first_nodes = vec![
        plan_node("inspect_partial", "inspect_partial", &[]),
        plan_node("rerun_same_run", "rerun_same_run", &["inspect_partial"]),
        plan_node(
            "verify_publication",
            "verify_publication",
            &["rerun_same_run"],
        ),
        plan_node("preview", "preview", &["verify_publication"]),
    ];
    let first = AdaptiveWorkflowPlanV1 {
        schema_version: ADAPTIVE_WORKFLOW_SCHEMA_VERSION,
        policy_sha256: policy_sha256.clone(),
        revision: 1,
        supersedes_sha256: None,
        reason: None,
        evidence_ids: Vec::new(),
        nodes: first_nodes.clone(),
    };
    let first_sha256 = first.canonical_sha256()?;
    let mut second_nodes = first_nodes;
    second_nodes.extend([
        plan_node("inspect_invalidation", "inspect_invalidation", &["preview"]),
        plan_node("reject_stale", "reject_stale", &["inspect_invalidation"]),
        plan_node("create_fresh", "create_fresh", &["reject_stale"]),
        plan_node("observe_stale", "observe_stale", &["create_fresh"]),
        plan_node(
            "verify_convergence",
            "verify_convergence",
            &["observe_stale"],
        ),
    ]);
    let second = AdaptiveWorkflowPlanV1 {
        schema_version: ADAPTIVE_WORKFLOW_SCHEMA_VERSION,
        policy_sha256,
        revision: 2,
        supersedes_sha256: Some(first_sha256),
        reason: Some("promotion preview invalidated the null-CAS recovery plan".into()),
        evidence_ids: vec![INVALIDATION_EVIDENCE_ID.into()],
        nodes: second_nodes,
    };
    let completed = first
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect::<BTreeSet<_>>();
    Ok((vec![first, second], completed))
}

pub fn build_adaptive_runtime(fixture_path: &Path) -> Result<ReleaseTrainAdaptiveRuntime> {
    let fixture: ReleaseFixture = serde_json::from_slice(
        &std::fs::read(fixture_path)
            .with_context(|| format!("read release fixture {}", fixture_path.display()))?,
    )
    .with_context(|| format!("parse release fixture {}", fixture_path.display()))?;
    let state = Arc::new(Mutex::new(ReleaseTrainRuntimeState {
        simulator: ReleaseTrainSimulator::new(fixture.clone())?,
        fixture,
        cleanup_count: 0,
    }));
    let catalog = Arc::new(step_catalog(state.clone())?);
    let policy = adaptive_policy();
    let (plans, completed_before_replan) = reference_adaptive_plans(&policy)?;
    let materialized = policy.materialize(&plans, &completed_before_replan, &catalog)?;
    let cleanup_hook: Arc<dyn WorkflowCleanupHook> = Arc::new(ReleaseCleanup {
        state: state.clone(),
    });
    Ok(ReleaseTrainAdaptiveRuntime {
        policy,
        plans,
        completed_before_replan,
        materialized,
        catalog,
        cleanup_hook,
        state,
    })
}

fn step_catalog(state: Arc<Mutex<ReleaseTrainRuntimeState>>) -> Result<StepCatalog> {
    let mut catalog = StepCatalog::new();
    for (id, operation, replay_policy, description) in [
        (
            PREFLIGHT,
            ReleaseOperation::Preflight,
            ReplayPolicy::Idempotent,
            "Validate immutable fixture identity",
        ),
        (
            INSPECT_PARTIAL,
            ReleaseOperation::InspectPartial,
            ReplayPolicy::Idempotent,
            "Inspect partial publication",
        ),
        (
            RERUN,
            ReleaseOperation::Rerun,
            ReplayPolicy::Compensable,
            "Rerun the same immutable release run",
        ),
        (
            VERIFY_PUBLICATION,
            ReleaseOperation::VerifyPublication,
            ReplayPolicy::Idempotent,
            "Verify exact publication",
        ),
        (
            PREVIEW,
            ReleaseOperation::Preview,
            ReplayPolicy::Idempotent,
            "Preview promotion and emit invalidation evidence",
        ),
        (
            INSPECT_INVALIDATION,
            ReleaseOperation::InspectInvalidation,
            ReplayPolicy::Idempotent,
            "Inspect invalidated latest graph",
        ),
        (
            REJECT_STALE,
            ReleaseOperation::RejectStale,
            ReplayPolicy::Idempotent,
            "Reject stale null-CAS operation",
        ),
        (
            CREATE_FRESH,
            ReleaseOperation::CreateFresh,
            ReplayPolicy::Compensable,
            "Create fresh gated promotion",
        ),
        (
            OBSERVE_STALE,
            ReleaseOperation::ObserveStale,
            ReplayPolicy::Idempotent,
            "Observe bounded stale canary",
        ),
        (
            VERIFY_CONVERGENCE,
            ReleaseOperation::VerifyConvergence,
            ReplayPolicy::Idempotent,
            "Observe and verify convergence",
        ),
        (
            RECONCILE,
            ReleaseOperation::Reconcile,
            ReplayPolicy::Idempotent,
            "Evaluate final release invariants",
        ),
    ] {
        catalog.register(
            descriptor(
                id,
                replay_policy,
                description,
                matches!(operation, ReleaseOperation::Reconcile),
                matches!(operation, ReleaseOperation::Preflight),
                if matches!(
                    operation,
                    ReleaseOperation::Rerun | ReleaseOperation::CreateFresh
                ) {
                    StepOperationalKind::Product
                } else {
                    StepOperationalKind::Assessment
                },
            ),
            Arc::new(ReleaseStep {
                state: state.clone(),
                operation,
            }),
        )?;
    }
    Ok(catalog)
}

fn descriptor(
    id: &str,
    replay_policy: ReplayPolicy,
    description: &str,
    terminal: bool,
    authorization_source: bool,
    operational_kind: StepOperationalKind,
) -> StepTypeDescriptor {
    let mut outputs = BTreeMap::from([(
        "evidence".into(),
        StepPortDescriptor {
            kind: PortValueKind::Json,
            optional: false,
            control_source: None,
        },
    )]);
    if terminal {
        for id in [
            "immutable_release_recovery",
            "exact_publication_integrity",
            "evidence_bound_replan",
            "safe_cas_promotion",
            "release_reconciliation",
        ] {
            outputs.insert(
                id.into(),
                StepPortDescriptor {
                    kind: PortValueKind::Assessment,
                    optional: false,
                    control_source: None,
                },
            );
        }
    }
    if authorization_source {
        outputs.insert(
            "authorized".into(),
            StepPortDescriptor {
                kind: PortValueKind::Boolean,
                optional: false,
                control_source: Some(ControlSource::Deterministic),
            },
        );
    }
    StepTypeDescriptor {
        id: id.into(),
        version: STEP_VERSION,
        description: description.into(),
        config_schema: json!({"type": "object", "additionalProperties": false}),
        inputs: BTreeMap::new(),
        outputs,
        capabilities: vec!["fixture.release_train".into()],
        required_functions: Vec::new(),
        replay_policy,
        operational_kind,
    }
}

fn criterion(id: &str, weight: u8) -> WorkflowCriterionDeclaration {
    WorkflowCriterionDeclaration {
        id: id.into(),
        weight,
        producer_node_id: "reconcile".into(),
        output_port: id.into(),
        advisory: false,
    }
}

fn trusted_anchor(
    placement: AdaptiveAnchorPlacement,
    terminal: bool,
    id: &str,
    step_type: &str,
    depends_on: &[&str],
) -> AdaptiveTrustedAnchorV1 {
    AdaptiveTrustedAnchorV1 {
        placement,
        terminal,
        node: WorkflowNodeV1 {
            id: id.into(),
            step_type: step_type.into(),
            step_version: STEP_VERSION,
            config: json!({}),
            depends_on: depends_on.iter().map(|value| (*value).into()).collect(),
            inputs: BTreeMap::new(),
            activation: Default::default(),
            dependency_policy: DependencyPolicy::Succeeded,
            required: true,
        },
    }
}

fn plan_node(id: &str, template_id: &str, depends_on: &[&str]) -> AdaptivePlanNodeV1 {
    AdaptivePlanNodeV1 {
        id: id.into(),
        template_id: template_id.into(),
        depends_on: depends_on.iter().map(|value| (*value).into()).collect(),
        focus: None,
        instructions: None,
    }
}

#[derive(Clone, Copy)]
enum ReleaseOperation {
    Preflight,
    InspectPartial,
    Rerun,
    VerifyPublication,
    Preview,
    InspectInvalidation,
    RejectStale,
    CreateFresh,
    ObserveStale,
    VerifyConvergence,
    Reconcile,
}

struct ReleaseStep {
    state: Arc<Mutex<ReleaseTrainRuntimeState>>,
    operation: ReleaseOperation,
}

#[async_trait]
impl StepExecutor for ReleaseStep {
    async fn execute(&self, _context: StepExecutorContext) -> Result<StepExecutorOutput> {
        let mut state = lock_state(&self.state)?;
        let evidence = execute_operation(&mut state, self.operation)?;
        if matches!(self.operation, ReleaseOperation::Reconcile) {
            Ok(reconcile_output(evidence, &state.simulator.evaluate()))
        } else if matches!(self.operation, ReleaseOperation::Preflight) {
            let mut result = output(evidence, None);
            result.outputs.insert(
                "authorized".into(),
                TypedPortValue {
                    kind: PortValueKind::Boolean,
                    value: Value::Bool(true),
                },
            );
            Ok(result)
        } else {
            Ok(output(evidence, None))
        }
    }

    async fn reconcile(
        &self,
        _context: &StepExecutorContext,
        _previous: &StepReconcileState,
    ) -> Result<StepReconcileOutcome> {
        let state = lock_state(&self.state)?;
        let completed = match self.operation {
            ReleaseOperation::Rerun => {
                state.simulator.state.run.attempt == 2
                    && state.simulator.state.run.status == RunStatus::Succeeded
            }
            ReleaseOperation::CreateFresh => state.simulator.state.fresh_operation.is_some(),
            _ => false,
        };
        if completed {
            Ok(StepReconcileOutcome::Completed(output(
                json!({"reconciled": true}),
                None,
            )))
        } else {
            Ok(StepReconcileOutcome::RetrySafe)
        }
    }
}

fn execute_operation(
    state: &mut ReleaseTrainRuntimeState,
    operation: ReleaseOperation,
) -> Result<Value> {
    let simulator = &mut state.simulator;
    match operation {
        ReleaseOperation::Preflight => Ok(json!({
            "tag": simulator.state.tag.name,
            "version": simulator.state.tag.version,
            "attempt": simulator.state.run.attempt,
        })),
        ReleaseOperation::InspectPartial => {
            if simulator.state.run.published_assets.is_empty()
                || simulator.state.run.published_assets == simulator.state.run.required_assets
            {
                bail!("fixture no longer represents a partial publication");
            }
            Ok(json!({"partial_publication": true}))
        }
        ReleaseOperation::Rerun => {
            let run_id = simulator.state.run.run_id;
            let tag = simulator.state.tag.name.clone();
            let version = simulator.state.tag.version.clone();
            Ok(serde_json::to_value(simulator.apply(
                ReleaseAction::RerunSameImmutableRun {
                    run_id,
                    tag,
                    version,
                },
            )?)?)
        }
        ReleaseOperation::VerifyPublication => {
            let complete = simulator.state.exact_version_published
                && simulator.state.run.published_assets == simulator.state.run.required_assets;
            if !complete {
                bail!("exact-version publication is incomplete");
            }
            Ok(json!({"exact_publication": true}))
        }
        ReleaseOperation::Preview => Ok(serde_json::to_value(
            simulator.apply(ReleaseAction::PreviewPromotion)?,
        )?),
        ReleaseOperation::InspectInvalidation => {
            if !simulator.state.previewed || simulator.state.latest_graph_compatible {
                bail!("promotion preview did not produce the expected invalidation");
            }
            Ok(json!({"evidence_id": INVALIDATION_EVIDENCE_ID}))
        }
        ReleaseOperation::RejectStale => {
            let operation_id = simulator.state.stale_operation.id.clone();
            Ok(serde_json::to_value(simulator.apply(
                ReleaseAction::RejectStaleNullCas { operation_id },
            )?)?)
        }
        ReleaseOperation::CreateFresh => {
            let expected_latest = simulator.state.latest_version.clone();
            Ok(serde_json::to_value(simulator.apply(
                ReleaseAction::CreateFreshGatedOperation { expected_latest },
            )?)?)
        }
        ReleaseOperation::ObserveStale => Ok(serde_json::to_value(
            simulator.apply(ReleaseAction::ObserveCanary)?,
        )?),
        ReleaseOperation::VerifyConvergence => Ok(serde_json::to_value(
            simulator.apply(ReleaseAction::ObserveCanary)?,
        )?),
        ReleaseOperation::Reconcile => {
            let gates = simulator.evaluate();
            if !gates.passed() {
                bail!("release train recovery did not satisfy all deterministic gates");
            }
            Ok(serde_json::to_value(gates)?)
        }
    }
}

fn output(evidence: Value, gate: Option<WorkflowGateResult>) -> StepExecutorOutput {
    StepExecutorOutput {
        outputs: BTreeMap::from([(
            "evidence".into(),
            TypedPortValue {
                kind: PortValueKind::Json,
                value: evidence,
            },
        )]),
        evaluation: StepEvaluation {
            hard_gates: gate.into_iter().collect(),
            evaluations: Vec::new(),
        },
        ..StepExecutorOutput::default()
    }
}

fn reconcile_output(evidence: Value, gates: &ReleaseRecoveryGates) -> StepExecutorOutput {
    let assessments = [
        (
            "immutable_release_recovery",
            gates.immutable_identity && gates.same_run_attempt_two,
            "The original immutable identity completed as attempt two of the same run.",
            vec!["run.attempt_2", "registry.exact_version"],
        ),
        (
            "exact_publication_integrity",
            gates.exact_publication,
            "The exact version contains the complete required asset set.",
            vec!["registry.exact_version"],
        ),
        (
            "evidence_bound_replan",
            gates.stale_operation_rejected,
            "The incompatible latest graph invalidated plan one and the stale null-CAS operation was rejected.",
            vec![INVALIDATION_EVIDENCE_ID],
        ),
        (
            "safe_cas_promotion",
            gates.one_fresh_gated_operation && gates.one_latest_cas,
            "One fresh operation preserved expected_latest and performed one authorized CAS.",
            vec!["operation.fresh", "cas.expected_latest"],
        ),
        (
            "release_reconciliation",
            gates.stale_then_converged_canary && gates.converged_latest,
            "The canary observed bounded staleness and then converged before mandatory cleanup.",
            vec!["canary.stale", "canary.converged"],
        ),
    ];
    let mut result = output(evidence, None);
    for (id, passed, summary, evidence_ids) in assessments {
        result.outputs.insert(
            id.into(),
            TypedPortValue {
                kind: PortValueKind::Assessment,
                value: serde_json::to_value(WorkflowEvaluationResult {
                    id: id.into(),
                    outcome: if passed {
                        WorkflowEvaluationOutcome::Passed
                    } else {
                        WorkflowEvaluationOutcome::Failed
                    },
                    summary: summary.into(),
                    score: Some(if passed { 1.0 } else { 0.0 }),
                    evidence_ids: evidence_ids.into_iter().map(str::to_string).collect(),
                })
                .expect("serialize deterministic release assessment"),
            },
        );
    }
    result
}

struct ReleaseCleanup {
    state: Arc<Mutex<ReleaseTrainRuntimeState>>,
}

#[async_trait]
impl WorkflowCleanupHook for ReleaseCleanup {
    async fn cleanup(&self, _context: &WorkflowCleanupContext) -> Result<()> {
        let mut state = lock_state(&self.state)?;
        state.simulator = ReleaseTrainSimulator::new(state.fixture.clone())?;
        state.cleanup_count += 1;
        Ok(())
    }
}

fn lock_state(
    state: &Arc<Mutex<ReleaseTrainRuntimeState>>,
) -> Result<MutexGuard<'_, ReleaseTrainRuntimeState>> {
    state
        .lock()
        .map_err(|_| anyhow::anyhow!("release train runtime state lock was poisoned"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/release_train_recovery/initial_state.json")
    }

    #[test]
    fn reference_replan_is_policy_valid_and_preserves_completed_nodes() -> Result<()> {
        let runtime = build_adaptive_runtime(&fixture_path())?;
        assert_eq!(runtime.materialized.revisions.len(), 2);
        assert_eq!(runtime.materialized.definition.nodes.len(), 11);
        assert!(runtime
            .plans
            .last()
            .unwrap()
            .evidence_ids
            .contains(&INVALIDATION_EVIDENCE_ID.into()));

        let mut changed = runtime.plans.clone();
        changed[1].nodes[0].template_id = "rerun_same_run".into();
        changed[1].supersedes_sha256 = Some(changed[0].canonical_sha256()?);
        assert!(runtime
            .policy
            .materialize(&changed, &runtime.completed_before_replan, &runtime.catalog)
            .is_err());
        Ok(())
    }

    #[tokio::test]
    async fn deterministic_steps_converge_and_cleanup_compensates_fixture_state() -> Result<()> {
        let runtime = build_adaptive_runtime(&fixture_path())?;
        for node in &runtime.materialized.definition.nodes {
            let registered = runtime
                .catalog
                .get(&node.step_type, node.step_version)
                .unwrap();
            registered
                .executor
                .execute(StepExecutorContext {
                    workflow_id: runtime.materialized.definition.id.clone(),
                    workflow_sha256: runtime.materialized.definition.canonical_sha256()?,
                    run_id: "release-runtime-test".into(),
                    attempt_id: "attempt-1".into(),
                    node: node.clone(),
                    replay_policy: registered.descriptor.replay_policy,
                    inputs: BTreeMap::new(),
                    output_dir: tempfile::tempdir()?.path().to_path_buf(),
                    cancellation: tokio::sync::watch::channel(false).1,
                })
                .await?;
        }
        assert!(runtime.state.lock().unwrap().gates().passed());
        runtime
            .cleanup_hook
            .cleanup(&WorkflowCleanupContext {
                workflow_id: runtime.materialized.definition.id.clone(),
                workflow_sha256: runtime.materialized.definition.canonical_sha256()?,
                run_id: "release-runtime-test".into(),
                attempt_id: "attempt-1".into(),
                output_dir: tempfile::tempdir()?.path().to_path_buf(),
            })
            .await?;
        let state = runtime.state.lock().unwrap();
        assert!(state.is_restored());
        assert_eq!(state.cleanup_count(), 1);
        Ok(())
    }
}
