use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{bail, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use super::{
    read_json, CrossRepoSimulator, ProducerContract, WorkspaceBoundaryGates, CANARY_EVIDENCE_ID,
    SCENARIO_ID, SCENARIO_VERSION,
};
use crate::workflow::{
    ActivationPolicy, AdaptiveAnchorPlacement, AdaptiveMaterializedWorkflow,
    AdaptiveNodeTemplateV1, AdaptivePlanNodeV1, AdaptiveTrustedAnchorV1, AdaptiveWorkflowPlanV1,
    AdaptiveWorkflowPolicyV1, BooleanCondition, ControlSource, DependencyPolicy, PortValueKind,
    ReplayPolicy, StepCatalog, StepExecutor, StepExecutorContext, StepExecutorOutput,
    StepOperationalKind, StepPortDescriptor, StepReconcileOutcome, StepReconcileState,
    StepTypeDescriptor, TypedPortValue, WorkflowCleanupContext, WorkflowCleanupHook,
    WorkflowCriterionDeclaration, WorkflowEvaluationOutcome, WorkflowEvaluationResult,
    WorkflowLimits, WorkflowNodeV1, ADAPTIVE_WORKFLOW_SCHEMA_VERSION,
};

const STEP_VERSION: u32 = 1;
const MATERIALIZE: &str = "cross_repo.materialize";
const INSPECT_VISIBLE: &str = "cross_repo.inspect_visible_contracts";
const MIGRATE_VISIBLE: &str = "cross_repo.migrate_visible_contract";
const VALIDATE_VISIBLE: &str = "cross_repo.validate_visible_matrix";
const REVEAL_CANARY: &str = "cross_repo.reveal_consumer_b_canary";
const INSPECT_CANARY: &str = "cross_repo.inspect_canary_invalidation";
const ADD_ALIAS: &str = "cross_repo.add_legacy_alias";
const VALIDATE_FULL: &str = "cross_repo.validate_full_matrix";
const VALIDATE_BOUNDARIES: &str = "cross_repo.validate_workspace_boundaries";
const RECONCILE: &str = "cross_repo.reconcile";

#[derive(Debug)]
pub struct CrossRepoRuntimeState {
    simulator: CrossRepoSimulator,
    cleanup_count: u32,
}

impl CrossRepoRuntimeState {
    pub fn cleanup_count(&self) -> u32 {
        self.cleanup_count
    }

    pub fn cleanup_complete(&self) -> bool {
        self.simulator.cleanup_complete()
    }

    pub fn workspace_root(&self) -> &Path {
        self.simulator.workspace_root()
    }

    pub fn final_gates(&self) -> Result<WorkspaceBoundaryGates> {
        let matrix = self.simulator.validate_full_matrix()?;
        if matrix.iter().any(|result| !result.passed) {
            bail!("cross-repository compatibility matrix did not converge");
        }
        self.simulator.validate_boundaries()
    }
}

pub struct CrossRepoAdaptiveRuntime {
    pub policy: AdaptiveWorkflowPolicyV1,
    pub plans: Vec<AdaptiveWorkflowPlanV1>,
    pub completed_before_replan: BTreeSet<String>,
    pub materialized: AdaptiveMaterializedWorkflow,
    pub catalog: Arc<StepCatalog>,
    pub cleanup_hook: Arc<dyn WorkflowCleanupHook>,
    pub state: Arc<Mutex<CrossRepoRuntimeState>>,
}

pub fn adaptive_policy() -> AdaptiveWorkflowPolicyV1 {
    let templates = [
        (
            "inspect_visible",
            INSPECT_VISIBLE,
            "Inspect the initially visible producer and consumer",
        ),
        (
            "migrate_visible",
            MIGRATE_VISIBLE,
            "Apply the first bounded contract migration",
        ),
        (
            "validate_visible",
            VALIDATE_VISIBLE,
            "Validate the visible compatibility matrix",
        ),
        (
            "reveal_canary",
            REVEAL_CANARY,
            "Reveal the hidden consumer canary",
        ),
        (
            "inspect_canary",
            INSPECT_CANARY,
            "Inspect the canary incompatibility",
        ),
        ("add_alias", ADD_ALIAS, "Add the bounded legacy route alias"),
        (
            "validate_full",
            VALIDATE_FULL,
            "Validate all consumer contracts",
        ),
        (
            "validate_boundaries",
            VALIDATE_BOUNDARIES,
            "Validate mutation and provenance boundaries",
        ),
    ]
    .into_iter()
    .map(|(id, step_type, description)| {
        let mutates_product = matches!(step_type, MIGRATE_VISIBLE | REVEAL_CANARY | ADD_ALIAS);
        AdaptiveNodeTemplateV1 {
            id: id.into(),
            description: description.into(),
            step_type: step_type.into(),
            step_version: STEP_VERSION,
            base_config: json!({}),
            inputs: BTreeMap::new(),
            activation: if mutates_product {
                ActivationPolicy::All(vec![BooleanCondition {
                    node_id: "materialize".into(),
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
        description: "Bounded multi-repository contract migration with a hidden-consumer invalidation, compensable fixture mutations, and deterministic compatibility gates.".into(),
        limits: WorkflowLimits {
            max_parallel: 1,
            max_nodes: 16,
            step_timeout_seconds: 60,
            workflow_timeout_seconds: 600,
            max_total_tokens: Some(636_000),
            max_cost_usd: Some(25.0),
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
                "materialize",
                MATERIALIZE,
            ),
            trusted_anchor(
                AdaptiveAnchorPlacement::AfterPlan,
                true,
                "reconcile",
                RECONCILE,
            ),
        ],
        criteria: vec![
            criterion("visible_contract_migration", 20),
            criterion("hidden_consumer_invalidation", 20),
            criterion("three_repo_compatibility", 30),
            criterion("workspace_boundaries", 20),
            criterion("migration_reconciliation", 10),
        ],
    }
}

pub fn reference_adaptive_plans(
    policy: &AdaptiveWorkflowPolicyV1,
) -> Result<(Vec<AdaptiveWorkflowPlanV1>, BTreeSet<String>)> {
    let policy_sha256 = policy.canonical_sha256()?;
    let first_nodes = vec![
        plan_node("inspect_visible", "inspect_visible", &[]),
        plan_node("migrate_visible", "migrate_visible", &["inspect_visible"]),
        plan_node("validate_visible", "validate_visible", &["migrate_visible"]),
        plan_node("reveal_canary", "reveal_canary", &["validate_visible"]),
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
        plan_node("inspect_canary", "inspect_canary", &["reveal_canary"]),
        plan_node("add_alias", "add_alias", &["inspect_canary"]),
        plan_node("validate_full", "validate_full", &["add_alias"]),
        plan_node(
            "validate_boundaries",
            "validate_boundaries",
            &["validate_full"],
        ),
    ]);
    let second = AdaptiveWorkflowPlanV1 {
        schema_version: ADAPTIVE_WORKFLOW_SCHEMA_VERSION,
        policy_sha256,
        revision: 2,
        supersedes_sha256: Some(first_sha256),
        reason: Some("the hidden consumer canary invalidated the v2-only route plan".into()),
        evidence_ids: vec![CANARY_EVIDENCE_ID.into()],
        nodes: second_nodes,
    };
    let completed = first
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect::<BTreeSet<_>>();
    Ok((vec![first, second], completed))
}

pub fn build_adaptive_runtime(
    fixture_root: &Path,
    workspace_root: &Path,
) -> Result<CrossRepoAdaptiveRuntime> {
    let state = Arc::new(Mutex::new(CrossRepoRuntimeState {
        simulator: CrossRepoSimulator::materialize(fixture_root, workspace_root)?,
        cleanup_count: 0,
    }));
    let catalog = Arc::new(step_catalog(state.clone())?);
    let policy = adaptive_policy();
    let (plans, completed_before_replan) = reference_adaptive_plans(&policy)?;
    let materialized = policy.materialize(&plans, &completed_before_replan, &catalog)?;
    let cleanup_hook: Arc<dyn WorkflowCleanupHook> = Arc::new(CrossRepoCleanup {
        state: state.clone(),
    });
    Ok(CrossRepoAdaptiveRuntime {
        policy,
        plans,
        completed_before_replan,
        materialized,
        catalog,
        cleanup_hook,
        state,
    })
}

fn step_catalog(state: Arc<Mutex<CrossRepoRuntimeState>>) -> Result<StepCatalog> {
    let mut catalog = StepCatalog::new();
    for (id, operation, replay_policy, description) in [
        (
            MATERIALIZE,
            CrossRepoOperation::Materialize,
            ReplayPolicy::Compensable,
            "Validate the owned fixture workspace",
        ),
        (
            INSPECT_VISIBLE,
            CrossRepoOperation::InspectVisible,
            ReplayPolicy::Idempotent,
            "Inspect initially visible repositories",
        ),
        (
            MIGRATE_VISIBLE,
            CrossRepoOperation::MigrateVisible,
            ReplayPolicy::Compensable,
            "Migrate visible producer and consumer",
        ),
        (
            VALIDATE_VISIBLE,
            CrossRepoOperation::ValidateVisible,
            ReplayPolicy::Idempotent,
            "Validate visible compatibility",
        ),
        (
            REVEAL_CANARY,
            CrossRepoOperation::RevealCanary,
            ReplayPolicy::Compensable,
            "Reveal hidden consumer canary",
        ),
        (
            INSPECT_CANARY,
            CrossRepoOperation::InspectCanary,
            ReplayPolicy::Idempotent,
            "Inspect canary invalidation",
        ),
        (
            ADD_ALIAS,
            CrossRepoOperation::AddAlias,
            ReplayPolicy::Compensable,
            "Add the legacy compatibility alias",
        ),
        (
            VALIDATE_FULL,
            CrossRepoOperation::ValidateFull,
            ReplayPolicy::Idempotent,
            "Validate full compatibility matrix",
        ),
        (
            VALIDATE_BOUNDARIES,
            CrossRepoOperation::ValidateBoundaries,
            ReplayPolicy::Idempotent,
            "Validate workspace boundaries",
        ),
        (
            RECONCILE,
            CrossRepoOperation::Reconcile,
            ReplayPolicy::Idempotent,
            "Reconcile final repository state",
        ),
    ] {
        catalog.register(
            descriptor(
                id,
                replay_policy,
                description,
                matches!(operation, CrossRepoOperation::Reconcile),
                matches!(operation, CrossRepoOperation::Materialize),
                if matches!(
                    operation,
                    CrossRepoOperation::Materialize
                        | CrossRepoOperation::MigrateVisible
                        | CrossRepoOperation::RevealCanary
                        | CrossRepoOperation::AddAlias
                ) {
                    StepOperationalKind::Product
                } else {
                    StepOperationalKind::Assessment
                },
            ),
            Arc::new(CrossRepoStep {
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
            "visible_contract_migration",
            "hidden_consumer_invalidation",
            "three_repo_compatibility",
            "workspace_boundaries",
            "migration_reconciliation",
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
        capabilities: vec!["fixture.cross_repo_contract".into()],
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
) -> AdaptiveTrustedAnchorV1 {
    AdaptiveTrustedAnchorV1 {
        placement,
        terminal,
        node: WorkflowNodeV1 {
            id: id.into(),
            step_type: step_type.into(),
            step_version: STEP_VERSION,
            config: json!({}),
            depends_on: Vec::new(),
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
enum CrossRepoOperation {
    Materialize,
    InspectVisible,
    MigrateVisible,
    ValidateVisible,
    RevealCanary,
    InspectCanary,
    AddAlias,
    ValidateFull,
    ValidateBoundaries,
    Reconcile,
}

struct CrossRepoStep {
    state: Arc<Mutex<CrossRepoRuntimeState>>,
    operation: CrossRepoOperation,
}

#[async_trait]
impl StepExecutor for CrossRepoStep {
    async fn execute(&self, _context: StepExecutorContext) -> Result<StepExecutorOutput> {
        let mut state = lock_state(&self.state)?;
        let evidence = execute_operation(&mut state, self.operation)?;
        if matches!(self.operation, CrossRepoOperation::Reconcile) {
            Ok(reconcile_output(evidence, &state.simulator)?)
        } else if matches!(self.operation, CrossRepoOperation::Materialize) {
            let mut result = output(evidence);
            result.outputs.insert(
                "authorized".into(),
                TypedPortValue {
                    kind: PortValueKind::Boolean,
                    value: Value::Bool(true),
                },
            );
            Ok(result)
        } else {
            Ok(output(evidence))
        }
    }

    async fn reconcile(
        &self,
        _context: &StepExecutorContext,
        _previous: &StepReconcileState,
    ) -> Result<StepReconcileOutcome> {
        let state = lock_state(&self.state)?;
        let simulator = &state.simulator;
        let completed = match self.operation {
            CrossRepoOperation::Materialize => simulator.workspace_root().exists(),
            CrossRepoOperation::MigrateVisible => {
                let producer: ProducerContract =
                    read_json(&simulator.workspace_root().join("producer/contract.json"))?;
                producer.current_contract_version == 2
            }
            CrossRepoOperation::RevealCanary => simulator.consumer_b_revealed,
            CrossRepoOperation::AddAlias => simulator
                .validate_full_matrix()
                .is_ok_and(|matrix| matrix.iter().all(|result| result.passed)),
            _ => false,
        };
        if completed {
            Ok(StepReconcileOutcome::Completed(output(json!({
                "reconciled": true
            }))))
        } else {
            Ok(StepReconcileOutcome::RetrySafe)
        }
    }
}

fn execute_operation(
    state: &mut CrossRepoRuntimeState,
    operation: CrossRepoOperation,
) -> Result<Value> {
    let simulator = &mut state.simulator;
    match operation {
        CrossRepoOperation::Materialize => Ok(json!({
            "workspace_root": simulator.workspace_root(),
            "visible_repositories": simulator.visible_repositories(),
        })),
        CrossRepoOperation::InspectVisible => {
            let visible = simulator.visible_repositories();
            if visible != ["producer", "consumer-a"] {
                bail!("hidden consumer was visible before the trusted canary");
            }
            Ok(json!({"visible_repositories": visible}))
        }
        CrossRepoOperation::MigrateVisible => {
            simulator.apply_reference_plan_a()?;
            Ok(json!({"migration": "plan_a"}))
        }
        CrossRepoOperation::ValidateVisible => {
            let matrix = simulator.validate_visible_matrix()?;
            if matrix.iter().any(|result| !result.passed) {
                bail!("visible compatibility matrix failed");
            }
            Ok(serde_json::to_value(matrix)?)
        }
        CrossRepoOperation::RevealCanary => {
            let outcome = simulator.run_trusted_canary()?;
            Ok(serde_json::to_value(outcome)?)
        }
        CrossRepoOperation::InspectCanary => {
            if !simulator.consumer_b_revealed {
                bail!("consumer-b was not revealed by the trusted canary");
            }
            let result = simulator
                .validate_full_matrix()?
                .into_iter()
                .find(|result| result.consumer_id == "consumer-b")
                .ok_or_else(|| anyhow::anyhow!("consumer-b canary result is missing"))?;
            if result.passed {
                bail!("canary no longer produces a material invalidation");
            }
            Ok(json!({"evidence_id": CANARY_EVIDENCE_ID, "reason": result.reason}))
        }
        CrossRepoOperation::AddAlias => {
            simulator.apply_reference_plan_b()?;
            Ok(json!({"migration": "plan_b"}))
        }
        CrossRepoOperation::ValidateFull => {
            let matrix = simulator.validate_full_matrix()?;
            if matrix.iter().any(|result| !result.passed) {
                bail!("full compatibility matrix failed");
            }
            Ok(serde_json::to_value(matrix)?)
        }
        CrossRepoOperation::ValidateBoundaries => {
            let gates = simulator.validate_boundaries()?;
            if !gates.passed() {
                bail!("workspace boundary gates failed");
            }
            Ok(serde_json::to_value(gates)?)
        }
        CrossRepoOperation::Reconcile => {
            let matrix = simulator.validate_full_matrix()?;
            let boundaries = simulator.validate_boundaries()?;
            if matrix.iter().any(|result| !result.passed) || !boundaries.passed() {
                bail!("cross-repository migration did not converge");
            }
            Ok(json!({"matrix": matrix, "boundaries": boundaries}))
        }
    }
}

fn output(evidence: Value) -> StepExecutorOutput {
    StepExecutorOutput {
        outputs: BTreeMap::from([(
            "evidence".into(),
            TypedPortValue {
                kind: PortValueKind::Json,
                value: evidence,
            },
        )]),
        ..StepExecutorOutput::default()
    }
}

fn reconcile_output(evidence: Value, simulator: &CrossRepoSimulator) -> Result<StepExecutorOutput> {
    let visible_passed = simulator
        .validate_visible_matrix()?
        .iter()
        .all(|result| result.passed);
    let full_passed = simulator
        .validate_full_matrix()?
        .iter()
        .all(|result| result.passed);
    let boundaries = simulator.validate_boundaries()?;
    let assessments = [
        (
            "visible_contract_migration",
            visible_passed,
            "The initially visible producer and consumer remain compatible after migration.",
            vec!["matrix.consumer_a"],
        ),
        (
            "hidden_consumer_invalidation",
            simulator.consumer_b_revealed,
            "The trusted canary revealed consumer B and bound revision two to its incompatibility evidence.",
            vec![CANARY_EVIDENCE_ID],
        ),
        (
            "three_repo_compatibility",
            full_passed,
            "Both old and new consumer contracts pass against the final producer contract.",
            vec!["matrix.consumer_a", "matrix.consumer_b"],
        ),
        (
            "workspace_boundaries",
            boundaries.passed(),
            "All changes stayed inside allowlisted paths with deterministic Git provenance and no network access.",
            vec!["workspace.boundaries", "git.provenance"],
        ),
        (
            "migration_reconciliation",
            full_passed && boundaries.passed(),
            "The terminal repository state reconciled successfully before mandatory owned-workspace cleanup.",
            vec!["migration.reconciled"],
        ),
    ];
    let mut result = output(evidence);
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
                })?,
            },
        );
    }
    Ok(result)
}

struct CrossRepoCleanup {
    state: Arc<Mutex<CrossRepoRuntimeState>>,
}

#[async_trait]
impl WorkflowCleanupHook for CrossRepoCleanup {
    async fn cleanup(&self, _context: &WorkflowCleanupContext) -> Result<()> {
        let mut state = lock_state(&self.state)?;
        if !state.simulator.cleanup_complete() {
            state.simulator.cleanup()?;
            state.cleanup_count += 1;
        }
        Ok(())
    }
}

fn lock_state(
    state: &Arc<Mutex<CrossRepoRuntimeState>>,
) -> Result<MutexGuard<'_, CrossRepoRuntimeState>> {
    state
        .lock()
        .map_err(|_| anyhow::anyhow!("cross-repository runtime state lock was poisoned"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/cross_repo_contract_migration")
    }

    #[test]
    fn reference_replan_is_policy_valid_and_evidence_bound() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let runtime = build_adaptive_runtime(&fixture_root(), &temporary.path().join("workspace"))?;
        assert_eq!(runtime.materialized.revisions.len(), 2);
        assert_eq!(runtime.materialized.definition.nodes.len(), 10);

        let mut missing_evidence = runtime.plans.clone();
        missing_evidence[1].evidence_ids.clear();
        missing_evidence[1].supersedes_sha256 = Some(missing_evidence[0].canonical_sha256()?);
        assert!(runtime
            .policy
            .materialize(
                &missing_evidence,
                &runtime.completed_before_replan,
                &runtime.catalog,
            )
            .is_err());
        Ok(())
    }

    #[tokio::test]
    async fn deterministic_steps_converge_and_cleanup_removes_owned_workspace() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let workspace = temporary.path().join("workspace");
        let runtime = build_adaptive_runtime(&fixture_root(), &workspace)?;
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
                    run_id: "cross-repo-runtime-test".into(),
                    attempt_id: "attempt-1".into(),
                    node: node.clone(),
                    replay_policy: registered.descriptor.replay_policy,
                    inputs: BTreeMap::new(),
                    output_dir: temporary.path().join("output"),
                    cancellation: tokio::sync::watch::channel(false).1,
                })
                .await?;
        }
        assert!(runtime.state.lock().unwrap().final_gates()?.passed());
        runtime
            .cleanup_hook
            .cleanup(&WorkflowCleanupContext {
                workflow_id: runtime.materialized.definition.id.clone(),
                workflow_sha256: runtime.materialized.definition.canonical_sha256()?,
                run_id: "cross-repo-runtime-test".into(),
                attempt_id: "attempt-1".into(),
                output_dir: temporary.path().join("output"),
            })
            .await?;
        let state = runtime.state.lock().unwrap();
        assert!(state.cleanup_complete());
        assert_eq!(state.cleanup_count(), 1);
        assert!(!workspace.exists());
        Ok(())
    }
}
