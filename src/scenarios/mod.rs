use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{bail, Result};
pub use async_trait::async_trait;
use clap::ValueEnum;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::assessment::{AssessmentKind, AssessmentPolicy};
use crate::context::E2eContext;
use crate::report::CompletionState;
use crate::wire::SessionMetricsResponse;

mod assessment;
pub mod browser_cross_site;
pub mod chess_engine;
pub mod chess_engine_build;
pub mod chess_play_ladder;
pub mod cleanup_under_failure;
pub mod common;
pub mod contention_ledger;
pub mod context_pressure;
pub mod cross_app_transaction;
pub mod cross_repo_contract_migration;
pub mod database_migration_recovery;
pub mod depth_ladder;
mod domain;
pub mod engineering_endurance_ladder;
pub mod engineering_ticket;
pub mod fanout_ladder;
pub(crate) mod fixture;
pub mod git_regression_forensics;
pub mod incident_response;
pub mod insert_record;
pub mod kanban;
pub mod linkly;
pub mod mechanical_reaction;
pub mod minimal_path;
pub mod moving_target;
pub mod performance_regression;
pub mod persistent_state;
pub mod poison_message;
pub mod policy_bound_action;
pub mod prompt_injection_resilience;
pub mod quorum_fan_in;
pub mod receiving_operation;
pub mod registry;
mod registry_plan;
pub mod release_train_recovery;
pub mod research_pipeline;
pub mod secret_hygiene;
pub mod security_review;
pub mod sequential_pipeline;
pub mod shell_coder_sandbox;
pub mod subagent_validation;
pub mod subagent_validation_failure;
pub mod swe_service;
pub mod timer_wake;
pub mod todo_worker;
pub mod tool_contract_recovery;
pub mod trend_blog;
pub mod trending_topics_build;
pub mod typescript_chat_service;
pub mod validation_chain;
pub mod validation_hook;
pub mod validation_loop;
pub mod validation_scope_enforcement;
pub mod validation_self_repair;
pub mod wake_chain_soak;

pub use domain::{
    is_sha256, scenario_contract_sha256, stable_seed, ArtifactExpectation, Capability,
    CapturedDeliverable, CapturedDeliverableContent, CapturedInvariant, DeliverableContract,
    ExecutionRealism, HumanHorizon, HumanHorizonBasis, InvariantSpec, ProvenanceEvidence,
    ScenarioCase, ScenarioCharacterization, ScenarioRealism, ShadowMode,
};

/// One registered E2E test. The registry below holds a `&'static dyn Scenario`
/// per id; materialization, digest sealing and validation are generic, so a
/// module only states what is its own: the case, the spec, and the hooks it
/// actually uses.
#[async_trait]
pub trait Scenario: Send + Sync {
    /// The registered id, equal to the `ScenarioId` string.
    fn id(&self) -> &'static str;
    fn execution_kind(&self) -> ScenarioExecutionKind {
        ScenarioExecutionKind::HarnessTurn
    }
    /// Seed of the retained canonical cohort: the stable digest of the id
    /// unless the scenario pins one.
    fn canonical_seed(&self) -> u64 {
        stable_seed(self.id())
    }
    /// Scenarios with one retained canonical cohort take no rotating seeds.
    fn canonical_seed_only(&self) -> bool {
        false
    }
    /// The unsealed case for one seed: inputs, capabilities and contract.
    fn case(&self, seed: u64) -> Result<ScenarioCase>;
    /// The prompt, execution policy, denied functions and criteria of one run.
    fn spec(&self, run_id: &str) -> ScenarioSpec;
    /// The spec the subject sees for one materialized case. It defaults to
    /// the definition; a module whose prompt states case-specific values
    /// renders them from the case here, while `spec` keeps the canonical
    /// rendering that identifies the definition.
    fn case_spec(&self, _case: &ScenarioCase, run_id: &str) -> ScenarioSpec {
        self.spec(run_id)
    }
    fn characterization(&self) -> Result<ScenarioCharacterization> {
        Ok(ScenarioCharacterization::synthetic())
    }
    /// A one-paragraph summary for the Console, when the module states one.
    fn summary(&self) -> Option<&'static str> {
        None
    }
    /// Functions the run registers itself and therefore needs available.
    fn required_functions(&self, _run_id: &str) -> Vec<String> {
        Vec::new()
    }
    /// The closed function surface the subject may call, when the scenario limits it.
    fn allowed_functions(&self, _run_id: &str) -> Option<Vec<String>> {
        None
    }
    fn dialogue_followups(&self, _run_id: &str) -> Vec<String> {
        Vec::new()
    }
    /// The attempt-owned filesystem root that `setup` prepared for the
    /// subject, when the scenario allocates one.
    fn prepared_root(&self, _run_id: &str) -> Result<Option<PathBuf>> {
        Ok(None)
    }
    /// Runs before the prompt is sent; a failure aborts the run.
    async fn setup(&self, _context: &E2eContext, _run_id: &str) -> Result<()> {
        Ok(())
    }
    /// Captures the deliverables the case declares, before cleanup. The suite
    /// calls it only when the deliverable contract declares artifacts, so a
    /// scenario that declares artifacts must override it: the default is the
    /// incoherent case and fails the attempt instead of capturing nothing.
    async fn capture(
        &self,
        _context: &E2eContext,
        _observation: &ScenarioObservation,
        _run_id: &str,
    ) -> Result<Vec<CapturedDeliverable>> {
        bail!(
            "scenario '{}' declares deliverable artifacts but has no capture hook",
            self.id()
        )
    }
    async fn evaluate(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> Result<ObjectiveEvaluation>;
    async fn cleanup(&self, _context: &E2eContext, _run_id: &str) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct CriterionSpec {
    pub id: &'static str,
    pub weight: u8,
    pub description: &'static str,
    pub kind: AssessmentKind,
    pub policy: AssessmentPolicy,
    pub dimension: crate::report::EvaluationDimension,
}

impl CriterionSpec {
    pub const fn scored(
        id: &'static str,
        weight: u8,
        description: &'static str,
        dimension: crate::report::EvaluationDimension,
    ) -> Self {
        Self {
            id,
            weight,
            description,
            kind: AssessmentKind::Signal,
            policy: AssessmentPolicy::Advisory,
            dimension,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionPolicy {
    pub max_turns: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    /// Shared Harness token budget. `None` leaves the Harness budget
    /// unbounded for this scenario.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_total_tokens: Option<u64>,
    /// Stop only after this many seconds without observable useful progress.
    /// Large scenarios have no fixed wall-clock deadline.
    pub stuck_timeout_seconds: u64,
    /// Optional per-session cap for post-turn validation denials. `None`
    /// preserves the Harness-configured default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_validation_retries: Option<u32>,
}

impl ExecutionPolicy {
    fn validate(self, scenario_id: &str) -> Result<()> {
        if self.max_turns == 0 {
            bail!("scenario '{scenario_id}': execution.max_turns=0; expected at least 1");
        }
        if self.max_output_tokens == Some(0) {
            bail!(
                "scenario '{scenario_id}': execution.max_output_tokens=0; expected None (provider limit) or at least 1"
            );
        }
        if self.max_total_tokens == Some(0) {
            bail!("scenario '{scenario_id}': execution.max_total_tokens=0; expected None (unbounded) or at least 1");
        }
        if self.stuck_timeout_seconds == 0 {
            bail!(
                "scenario '{scenario_id}': execution.stuck_timeout_seconds=0; expected at least 1"
            );
        }
        if self.max_output_tokens.is_some_and(|max_output_tokens| {
            self.max_total_tokens
                .is_some_and(|max_total_tokens| max_total_tokens < max_output_tokens)
        }) {
            let max_output_tokens = self.max_output_tokens.expect("checked above");
            let max_total_tokens = self.max_total_tokens.expect("checked above");
            bail!(
                "scenario '{scenario_id}': execution.max_total_tokens={} is lower than execution.max_output_tokens={max_output_tokens}; expected max_total_tokens >= max_output_tokens",
                max_total_tokens
            );
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct ScenarioSpec {
    pub id: &'static str,
    pub prompt: String,
    pub filesystem_root: Option<PathBuf>,
    pub execution: ExecutionPolicy,
    pub denied_functions: &'static [&'static str],
    pub criteria: Vec<CriterionSpec>,
}

pub struct MaterializedScenario {
    pub spec: ScenarioSpec,
    pub case: ScenarioCase,
    pub module: &'static dyn Scenario,
}

impl MaterializedScenario {
    pub fn validate(&self) -> Result<()> {
        self.spec.validate()?;
        // A scenario module materializes an unsealed case; `ScenarioId::materialize`
        // seals it with the definition digest before the case leaves the crate.
        self.case.validate_shape()?;
        if !self.case.behavior_sha256.is_empty() {
            self.case.validate()?;
        }
        if self.spec.id != self.case.scenario_id || self.spec.id != self.module.id() {
            bail!(
                "materialized scenario id '{}' differs from case id '{}' or module id '{}'",
                self.spec.id,
                self.case.scenario_id,
                self.module.id()
            );
        }
        Ok(())
    }
}

impl ScenarioSpec {
    pub fn validate(&self) -> Result<()> {
        if self.prompt.trim().is_empty() {
            bail!(
                "scenario '{}': prompt is empty after trimming; provide a non-empty task prompt",
                self.id
            );
        }
        self.execution.validate(self.id)?;
        let mut ids = HashMap::new();
        for (index, criterion) in self.criteria.iter().enumerate() {
            if criterion.id.trim().is_empty() {
                bail!(
                    "scenario '{}': criteria[{index}].id is empty after trimming; use a stable non-empty identifier",
                    self.id
                );
            }
            if criterion.weight == 0 {
                bail!(
                    "scenario '{}': criterion '{}' has weight=0; expected at least 1",
                    self.id,
                    criterion.id
                );
            }
            if criterion.description.trim().is_empty() {
                bail!(
                    "scenario '{}': criterion '{}' has an empty description; every assessment must be explainable",
                    self.id,
                    criterion.id
                );
            }
            if criterion.kind == AssessmentKind::AssetValidation {
                bail!(
                    "scenario '{}': criterion '{}' uses asset-only assessment metadata",
                    self.id,
                    criterion.id
                );
            }
            if criterion.policy == AssessmentPolicy::HardGate
                && criterion.kind != AssessmentKind::RequiredCheck
            {
                bail!(
                    "scenario '{}': hard-gated criterion '{}' must be a required check",
                    self.id,
                    criterion.id
                );
            }
            if let Some(first_index) = ids.insert(criterion.id, index) {
                bail!(
                    "scenario '{}': criterion id '{}' is duplicated at indexes {first_index} and {index}; criterion ids must be unique",
                    self.id, criterion.id
                );
            }
        }
        let total: u16 = self
            .criteria
            .iter()
            .map(|criterion| u16::from(criterion.weight))
            .sum();
        if total != 100 {
            let declared = self
                .criteria
                .iter()
                .map(|criterion| format!("{}={}", criterion.id, criterion.weight))
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "scenario '{}': criterion weights total={total}; expected exactly 100; declared weights=[{declared}]",
                self.id
            );
        }
        Ok(())
    }
}

pub struct ScenarioObservation {
    pub case: ScenarioCase,
    pub metrics: SessionMetricsResponse,
    pub transcript: Value,
    pub response: String,
    pub deliverables: Vec<CapturedDeliverable>,
}

pub struct ObjectiveEvaluation {
    /// Whether the subject reached the task's terminal state. This is
    /// deliberately independent from score: a completed task may still be
    /// objectively wrong or low quality.
    pub completion: CompletionState,
    pub awards: Vec<CriterionAward>,
    /// Execution failure after partial observations were obtained.
    pub infrastructure_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioExecutionKind {
    HarnessTurn,
    ScriptedDialogue,
    CompositeFlow,
    AdaptiveFlow,
}

impl ScenarioExecutionKind {
    pub const fn replay_safe(self) -> bool {
        matches!(self, Self::HarnessTurn)
    }
}

pub struct CriterionAward {
    pub id: String,
    pub awarded: Option<u8>,
    pub reason: String,
}

/// The registry: one line per test. The id is what the enum prints and
/// parses; the expression is the module value that implements [`Scenario`].
macro_rules! scenarios {
    ($($variant:ident = $id:literal => $module:expr,)+) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ValueEnum)]
        #[serde(rename_all = "snake_case")]
        pub enum ScenarioId {
            $(
                #[value(name = $id)]
                $variant,
            )+
        }

        impl ScenarioId {
            pub const ALL: [Self; scenarios!(@count $($variant)+)] = [$(Self::$variant,)+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $id,)+
                }
            }

            /// The module that implements this test.
            pub fn module(self) -> &'static dyn Scenario {
                match self {
                    $(Self::$variant => &$module,)+
                }
            }
        }
    };
    (@count) => { 0usize };
    (@count $head:ident $($tail:ident)*) => { 1usize + scenarios!(@count $($tail)*) };
}

scenarios! {
    RegistryPlanning = "registry_planning" => registry::Registry(1),
    RegistryImplementation = "registry_implementation" => registry::Registry(2),
    RegistryEnvironment = "registry_environment" => registry::Registry(3),
    RegistryVerification = "registry_verification" => registry::Registry(4),
    KanbanC1Foundation = "kanban_c1_foundation" => kanban::Kanban(0),
    KanbanC2Persistence = "kanban_c2_persistence" => kanban::Kanban(1),
    KanbanC3Board = "kanban_c3_board" => kanban::Kanban(2),
    KanbanC4TicketFlow = "kanban_c4_ticket_flow" => kanban::Kanban(3),
    KanbanC5EditMove = "kanban_c5_edit_move" => kanban::Kanban(4),
    KanbanC6Discussion = "kanban_c6_discussion" => kanban::Kanban(5),
    KanbanC7Live = "kanban_c7_live" => kanban::Kanban(6),
    LinklyTutorial = "linkly_tutorial" => linkly::LinklyTutorial,
    ContextPressure = "context_pressure" => context_pressure::ContextPressure,
    MinimalPath = "minimal_path" => minimal_path::MinimalPath,
    PersistentState = "persistent_state" => persistent_state::PersistentState,
    InsertRecord = "insert_record" => insert_record::InsertRecord,
    SequentialPipeline = "sequential_pipeline" => sequential_pipeline::SequentialPipeline,
    DatabaseMigrationRecovery = "database_migration_recovery" => database_migration_recovery::DatabaseMigrationRecovery,
    ShellCoderSandbox = "shell_coder_sandbox" => shell_coder_sandbox::ShellCoderSandbox,
    ResearchPipeline = "research_pipeline" => research_pipeline::ResearchPipeline,
    FanoutLadder = "fanout_ladder" => fanout_ladder::FanoutLadder,
    SecurityReview = "security_review" => security_review::SecurityReview,
    IncidentResponse = "incident_response" => incident_response::IncidentResponse,
    TodoWorkerSimple = "todo_worker_simple" => todo_worker::TodoWorkerSimple,
    TodoWorkerPlanned = "todo_worker_planned" => todo_worker::TodoWorkerPlanned,
    EngineeringTicket = "engineering_ticket" => engineering_ticket::EngineeringTicket,
    EngineeringTicketGitHandoff = "engineering_ticket_git_handoff" => engineering_ticket::EngineeringTicketGitHandoff,
    EngineeringEnduranceLadder = "engineering_endurance_ladder" => engineering_endurance_ladder::EngineeringEnduranceLadder,
    GitRegressionForensics = "git_regression_forensics" => git_regression_forensics::GitRegressionForensics,
    MechanicalReaction = "mechanical_reaction" => mechanical_reaction::MechanicalReaction,
    TimerWake = "timer_wake" => timer_wake::TimerWake,
    ReceivingOperation = "receiving_operation" => receiving_operation::ReceivingOperation,
    ValidationLoop = "validation_loop" => validation_loop::ValidationLoop,
    SubagentValidation = "subagent_validation" => subagent_validation::SubagentValidation,
    SubagentValidationFailure = "subagent_validation_failure" => subagent_validation_failure::SubagentValidationFailure,
    ValidationSelfRepair = "validation_self_repair" => validation_self_repair::ValidationSelfRepair,
    ValidationScopeEnforcement = "validation_scope_enforcement" => validation_scope_enforcement::ValidationScopeEnforcement,
    ValidationChain = "validation_chain" => validation_chain::ValidationChain,
    SecretHygiene = "secret_hygiene" => secret_hygiene::SecretHygiene,
    PromptInjectionResilience = "prompt_injection_resilience" => prompt_injection_resilience::PromptInjectionResilience,
    MovingTarget = "moving_target" => moving_target::MovingTarget,
    PoisonMessage = "poison_message" => poison_message::PoisonMessage,
    CleanupUnderFailure = "cleanup_under_failure" => cleanup_under_failure::CleanupUnderFailure,
    DepthLadder = "depth_ladder" => depth_ladder::DepthLadder,
    QuorumFanIn = "quorum_fan_in" => quorum_fan_in::QuorumFanIn,
    ContentionLedger = "contention_ledger" => contention_ledger::ContentionLedger,
    WakeChainSoak = "wake_chain_soak" => wake_chain_soak::WakeChainSoak,
    ChessEngineBuild = "chess_engine_build" => chess_engine_build::ChessEngineBuild,
    ChessPlayLadder = "chess_play_ladder" => chess_play_ladder::ChessPlayLadder,
    TrendBlog = "trend_blog" => trend_blog::TrendBlog,
    TrendingTopicsBuild = "trending_topics_build" => trending_topics_build::TrendingTopicsBuild,
    TypescriptChatService = "typescript_chat_service" => typescript_chat_service::TypescriptChatService,
    ToolContractRecovery = "tool_contract_recovery" => tool_contract_recovery::ToolContractRecovery,
    PolicyBoundAction = "policy_bound_action" => policy_bound_action::PolicyBoundAction,
    CrossAppTransaction = "cross_app_transaction" => cross_app_transaction::CrossAppTransaction,
    PerformanceRegression = "performance_regression" => performance_regression::PerformanceRegression,
    BrowserCrossSite = "browser_cross_site" => browser_cross_site::BrowserCrossSite,
    ReleaseTrainRecovery = "release_train_recovery" => release_train_recovery::ReleaseTrainRecovery,
    CrossRepoContractMigration = "cross_repo_contract_migration" => cross_repo_contract_migration::CrossRepoContractMigration,
    SweConfigIsolation = "swe_config_isolation" => swe_service::SweService(ScenarioId::SweConfigIsolation),
    SweCacheInvalidation = "swe_cache_invalidation" => swe_service::SweService(ScenarioId::SweCacheInvalidation),
    SweBatchReplay = "swe_batch_replay" => swe_service::SweService(ScenarioId::SweBatchReplay),
    SweReplayRecovery = "swe_replay_recovery" => swe_service::SweService(ScenarioId::SweReplayRecovery),
    SweContractMigration = "swe_contract_migration" => swe_service::SweService(ScenarioId::SweContractMigration),
    SweTenantIsolation = "swe_tenant_isolation" => swe_service::SweService(ScenarioId::SweTenantIsolation),
    SweReplayPerformance = "swe_replay_performance" => swe_service::SweService(ScenarioId::SweReplayPerformance),
    SweReleaseHandoff = "swe_release_handoff" => swe_service::SweService(ScenarioId::SweReleaseHandoff),
    SweServiceJourney = "swe_service_journey" => swe_service::SweService(ScenarioId::SweServiceJourney),
}

impl ScenarioId {
    pub fn spec(self, run_id: &str) -> ScenarioSpec {
        self.module().spec(run_id)
    }

    /// Materialize one case: the module states the case and the spec, the
    /// registry seals the definition digest and validates the whole.
    pub fn materialize(self, namespace: &str, seed: u64) -> Result<MaterializedScenario> {
        let module = self.module();
        let case = module
            .case(seed)?
            .with_characterization(module.characterization()?)?;
        let spec = module.case_spec(&case, namespace);
        let behavior_sha256 = behavior_sha256(self, &case)?;
        let case = case.seal(behavior_sha256)?;
        let materialized = MaterializedScenario { spec, case, module };
        materialized.validate()?;
        Ok(materialized)
    }

    pub fn canonical_seed(self) -> u64 {
        self.module().canonical_seed()
    }

    /// Scenarios with one retained canonical cohort do not participate in
    /// rotating-seed runs.
    pub fn canonical_seed_only(self) -> bool {
        self.module().canonical_seed_only()
    }

    pub fn execution_kind(self) -> ScenarioExecutionKind {
        self.module().execution_kind()
    }

    pub fn summary(self) -> Option<&'static str> {
        self.module().summary()
    }
}

impl std::fmt::Display for ScenarioId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for ScenarioId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == value)
            .ok_or_else(|| anyhow::anyhow!("unknown E2E scenario '{value}'"))
    }
}

/// Namespace used to render the definition a behavior digest covers. Run
/// identifiers, attempt namespaces, and seeds must not leak into the digest,
/// so the canonical spec is rendered under this fixed name.
pub const CONTRACT_NAMESPACE: &str = "contract";

/// Digest of a scenario definition: what the subject is asked, how the run is
/// bounded, how it is scored, and the case-independent contract. Seed-specific
/// inputs are excluded so every seed of one definition shares the digest.
pub fn behavior_sha256(id: ScenarioId, case: &ScenarioCase) -> Result<String> {
    let spec = id.spec(CONTRACT_NAMESPACE);
    crate::artifact::sha256_value(&serde_json::json!({
        "scenario_id": id.as_str(),
        "prompt": spec.prompt,
        "execution": spec.execution,
        "denied_functions": spec.denied_functions,
        "criteria": spec
            .criteria
            .iter()
            .map(|criterion| serde_json::json!({
                "id": criterion.id,
                "weight": criterion.weight,
                "description": criterion.description,
                "kind": criterion.kind,
                "policy": criterion.policy,
                "dimension": criterion.dimension,
            }))
            .collect::<Vec<_>>(),
        "characterization": case.characterization,
        "required_capabilities": case.required_capabilities,
        "deliverable_contract": case.deliverable_contract,
    }))
}

pub fn selected(requested: &[ScenarioId]) -> Vec<ScenarioId> {
    if requested.is_empty() {
        return ScenarioId::ALL.into_iter().collect();
    }
    requested.iter().copied().fold(Vec::new(), |mut ids, id| {
        if !ids.contains(&id) {
            ids.push(id);
        }
        ids
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// A scenario that declares artifacts without overriding `capture`.
    struct ArtifactsWithoutCapture;

    #[async_trait]
    impl Scenario for ArtifactsWithoutCapture {
        fn id(&self) -> &'static str {
            "artifacts_without_capture"
        }

        fn case(&self, _seed: u64) -> Result<ScenarioCase> {
            unreachable!("the default capture test never materializes")
        }

        fn spec(&self, _run_id: &str) -> ScenarioSpec {
            unreachable!("the default capture test never builds the spec")
        }

        async fn evaluate(
            &self,
            _context: &E2eContext,
            _observation: &ScenarioObservation,
            _run_id: &str,
        ) -> Result<ObjectiveEvaluation> {
            unreachable!("the default capture test never evaluates")
        }
    }

    #[tokio::test]
    async fn the_default_capture_refuses_to_capture_nothing() {
        let context = E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let materialized = ScenarioId::MinimalPath.materialize("probe", 7).unwrap();
        let observation = ScenarioObservation {
            case: materialized.case,
            metrics: SessionMetricsResponse::from_normalized(crate::wire::SessionMetricsPayload {
                root_session_id: "probe".into(),
                complete: true,
                totals: Default::default(),
                by_session: Vec::new(),
                traces: None,
            }),
            transcript: Value::Null,
            response: String::new(),
            deliverables: Vec::new(),
        };
        let error = ArtifactsWithoutCapture
            .capture(&context, &observation, "probe")
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("declares deliverable artifacts but has no capture hook"));
    }

    #[test]
    fn registry_contains_sixty_eight_unique_valid_scenarios() {
        let mut ids = HashSet::new();
        for scenario in ScenarioId::ALL {
            assert!(ids.insert(scenario.as_str()));
            scenario.spec("run").validate().unwrap();
            scenario
                .materialize("run", scenario.canonical_seed())
                .unwrap();
        }
        assert_eq!(ids.len(), 68);
    }

    #[test]
    fn explicit_selection_preserves_order_and_deduplicates() {
        assert_eq!(
            selected(&[
                ScenarioId::ContextPressure,
                ScenarioId::ShellCoderSandbox,
                ScenarioId::ContextPressure,
            ]),
            vec![ScenarioId::ContextPressure, ScenarioId::ShellCoderSandbox]
        );
    }

    #[test]
    fn default_selection_includes_every_registered_scenario() {
        let selected = selected(&[]);
        assert!(selected.contains(&ScenarioId::SecurityReview));
        assert!(selected.contains(&ScenarioId::IncidentResponse));
        assert!(selected.contains(&ScenarioId::EngineeringTicket));
        assert!(selected.contains(&ScenarioId::EngineeringTicketGitHandoff));
        assert!(selected.contains(&ScenarioId::EngineeringEnduranceLadder));
        assert!(selected.contains(&ScenarioId::GitRegressionForensics));
        assert!(selected.contains(&ScenarioId::TodoWorkerSimple));
        assert!(selected.contains(&ScenarioId::TodoWorkerPlanned));
        assert_eq!(selected.len(), ScenarioId::ALL.len());
    }

    #[test]
    fn materialized_cases_are_stable_across_attempt_namespaces() {
        let first = ScenarioId::MechanicalReaction
            .materialize("attempt-a", 42)
            .unwrap();
        let retry = ScenarioId::MechanicalReaction
            .materialize("attempt-b", 42)
            .unwrap();
        let other_seed = ScenarioId::MechanicalReaction
            .materialize("attempt-c", 43)
            .unwrap();

        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs, retry.case.inputs);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert_ne!(first.spec.prompt, retry.spec.prompt);
        assert_ne!(first.case.case_id, other_seed.case.case_id);
        assert_ne!(first.case.inputs, other_seed.case.inputs);
        assert_eq!(first.case.behavior_sha256, retry.case.behavior_sha256);
        assert_eq!(first.case.behavior_sha256, other_seed.case.behavior_sha256);
        assert!(is_sha256(&first.case.behavior_sha256));
    }

    #[test]
    fn converted_scenarios_publish_deliverable_contracts() {
        let state = ScenarioId::MovingTarget.materialize("state", 7).unwrap();
        let coordination = ScenarioId::SubagentValidation
            .materialize("coordination", 7)
            .unwrap();

        assert!(state.case.deliverable_contract.capture_before_cleanup);
        assert!(coordination.case.deliverable_contract.provenance_required);
    }

    #[test]
    fn automation_and_state_scenarios_publish_reproducible_deliverable_cases() {
        for scenario in [
            ScenarioId::MechanicalReaction,
            ScenarioId::TimerWake,
            ScenarioId::ReceivingOperation,
        ] {
            let first = scenario.materialize("attempt-a", 91).unwrap();
            let retry = scenario.materialize("attempt-b", 91).unwrap();
            assert_eq!(first.case.case_id, retry.case.case_id, "{scenario:?}");
            assert_eq!(first.case.inputs, retry.case.inputs, "{scenario:?}");
            assert!(
                !first.case.deliverable_contract.artifacts.is_empty(),
                "{scenario:?}"
            );
            assert!(
                first.case.deliverable_contract.capture_before_cleanup,
                "{scenario:?}"
            );
            assert!(
                first.case.deliverable_contract.provenance_required,
                "{scenario:?}"
            );
        }
    }

    #[test]
    fn production_scenarios_capture_each_declared_artifact() {
        for scenario in [ScenarioId::ShellCoderSandbox, ScenarioId::ResearchPipeline] {
            let first = scenario.materialize("attempt-a", 127).unwrap();
            let retry = scenario.materialize("attempt-b", 127).unwrap();
            assert_eq!(first.case.case_id, retry.case.case_id, "{scenario:?}");
            assert_eq!(first.case.inputs, retry.case.inputs, "{scenario:?}");
            assert!(
                !first.case.deliverable_contract.artifacts.is_empty(),
                "{scenario:?}"
            );
            assert!(
                first
                    .case
                    .deliverable_contract
                    .invariants
                    .iter()
                    .all(|invariant| !invariant.description.trim().is_empty()),
                "{scenario:?}"
            );
        }
    }

    #[test]
    fn validated_delegation_cases_publish_success_and_failure_deliverables() {
        for scenario in [
            ScenarioId::SubagentValidation,
            ScenarioId::SubagentValidationFailure,
        ] {
            let materialized = scenario.materialize("delegation", 211).unwrap();
            assert!(
                !materialized.case.deliverable_contract.artifacts.is_empty(),
                "{scenario:?}"
            );
            assert!(
                materialized
                    .case
                    .required_capabilities
                    .contains(&Capability::E2eSubagents),
                "{scenario:?}"
            );
        }
    }

    #[test]
    fn every_non_atomic_scenario_has_a_reproducible_deliverable_contract() {
        for scenario in ScenarioId::ALL {
            let seed = scenario.canonical_seed();
            let first = scenario.materialize("attempt-a", seed).unwrap();
            let retry = scenario.materialize("attempt-b", seed).unwrap();
            assert_eq!(first.case.case_id, retry.case.case_id, "{scenario:?}");
            assert_eq!(first.case.inputs, retry.case.inputs, "{scenario:?}");
            assert_eq!(
                first.case.inputs_sha256, retry.case.inputs_sha256,
                "{scenario:?}"
            );

            if matches!(
                scenario.execution_kind(),
                ScenarioExecutionKind::CompositeFlow | ScenarioExecutionKind::AdaptiveFlow
            ) {
                if swe_service::is_swe(scenario) {
                    assert!(!first.case.deliverable_contract.artifacts.is_empty());
                    assert!(first.case.deliverable_contract.capture_before_cleanup);
                } else {
                    assert!(first.case.deliverable_contract.artifacts.is_empty());
                }
                continue;
            }

            assert!(
                !first.case.deliverable_contract.artifacts.is_empty(),
                "{scenario:?}"
            );
            assert!(
                first.case.deliverable_contract.capture_before_cleanup,
                "{scenario:?}"
            );
            for artifact in &first.case.deliverable_contract.artifacts {
                jsonschema::JSONSchema::compile(&artifact.schema)
                    .unwrap_or_else(|error| panic!("{scenario:?} invalid schema: {error}"));
            }
        }
    }

    #[test]
    fn behavior_digests_are_stable_per_definition_and_distinct_across_scenarios() {
        let mut digests = HashSet::new();
        for scenario in ScenarioId::ALL {
            let seed = scenario.canonical_seed();
            let first = scenario.materialize("attempt-a", seed).unwrap();
            let again = scenario.materialize("attempt-b", seed).unwrap();
            let other_seed = scenario
                .materialize("attempt-c", seed.wrapping_add(1))
                .unwrap();
            assert!(is_sha256(&first.case.behavior_sha256), "{scenario:?}");
            assert_eq!(
                first.case.behavior_sha256, again.case.behavior_sha256,
                "{scenario:?} digest depends on the attempt namespace"
            );
            assert_eq!(
                first.case.behavior_sha256, other_seed.case.behavior_sha256,
                "{scenario:?} digest depends on the seed"
            );
            assert!(
                digests.insert(first.case.behavior_sha256.clone()),
                "{scenario:?} shares a digest with another scenario"
            );
        }
    }

    #[test]
    fn validation_identifies_the_invalid_execution_field() {
        type ValidationCase = (&'static str, fn(&mut ExecutionPolicy), &'static str);

        let cases: [ValidationCase; 5] = [
            (
                "max_turns",
                |execution| execution.max_turns = 0,
                "scenario 'context_pressure': execution.max_turns=0; expected at least 1",
            ),
            (
                "max_output_tokens",
                |execution| execution.max_output_tokens = Some(0),
                "scenario 'context_pressure': execution.max_output_tokens=0; expected None (provider limit) or at least 1",
            ),
            (
                "max_total_tokens",
                |execution| execution.max_total_tokens = Some(0),
                "scenario 'context_pressure': execution.max_total_tokens=0; expected None (unbounded) or at least 1",
            ),
            (
                "stuck_timeout_seconds",
                |execution| execution.stuck_timeout_seconds = 0,
                "scenario 'context_pressure': execution.stuck_timeout_seconds=0; expected at least 1",
            ),
            (
                "total_token_order",
                |execution| execution.max_total_tokens = Some(1),
                "scenario 'context_pressure': execution.max_total_tokens=1 is lower than execution.max_output_tokens=16384; expected max_total_tokens >= max_output_tokens",
            ),
        ];

        for (field, mutate, expected) in cases {
            let mut spec = ScenarioId::ContextPressure.spec("run");
            mutate(&mut spec.execution);
            assert_eq!(
                spec.validate().unwrap_err().to_string(),
                expected,
                "{field}"
            );
        }
    }

    #[test]
    fn validation_reports_criterion_values_before_weight_total() {
        let mut spec = ScenarioId::ContextPressure.spec("run");
        spec.criteria = vec![CriterionSpec::scored(
            "durable_result",
            0,
            "invalid",
            crate::report::EvaluationDimension::StructuralIntegrity,
        )];

        assert_eq!(
            spec.validate().unwrap_err().to_string(),
            "scenario 'context_pressure': criterion 'durable_result' has weight=0; expected at least 1"
        );
    }

    #[test]
    fn validation_reports_duplicate_criterion_indexes() {
        let mut spec = ScenarioId::ContextPressure.spec("run");
        spec.criteria = vec![
            CriterionSpec::scored(
                "duplicate",
                50,
                "first",
                crate::report::EvaluationDimension::StructuralIntegrity,
            ),
            CriterionSpec::scored(
                "duplicate",
                50,
                "second",
                crate::report::EvaluationDimension::StructuralIntegrity,
            ),
        ];

        assert_eq!(
            spec.validate().unwrap_err().to_string(),
            "scenario 'context_pressure': criterion id 'duplicate' is duplicated at indexes 0 and 1; criterion ids must be unique"
        );
    }

    #[test]
    fn consolidated_scenarios_normalize_requested_seeds_to_the_retained_case() {
        for scenario in ScenarioId::ALL
            .into_iter()
            .filter(|scenario| scenario.canonical_seed_only())
        {
            let materialized = scenario.materialize("attempt", 7).unwrap();
            assert_eq!(materialized.case.seed, scenario.canonical_seed());
        }
    }
}
