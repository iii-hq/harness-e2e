use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
pub use async_trait::async_trait;
use clap::ValueEnum;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::assessment::{AssessmentKind, AssessmentPolicy};
use crate::context::E2eContext;
use crate::report::CompletionState;
use crate::wire::{SessionMetricsResponse, TurnStatus};

pub mod alertmanager_route_match;
pub mod approval_roundtrip;
pub mod ask_handshake;
mod assessment;
pub mod binding_contract_probe;
pub mod browser_cross_site;
pub mod call_guards;
pub mod chess_engine;
pub mod chess_engine_build;
pub mod chess_play_ladder;
pub mod cleanup_under_failure;
pub mod common;
pub mod contention_ledger;
pub mod context_compaction;
pub mod context_pressure;
pub mod cross_app_transaction;
pub mod cross_repo_contract_migration;
pub mod database_migration_recovery;
pub mod depth_ladder;
pub mod dispatch_pipeline;
mod domain;
pub mod engineering_endurance_ladder;
pub mod fanout_ladder;
pub(crate) mod fixture;
pub mod git_regression_forensics;
pub mod harness_contract_probe;
pub mod incident_response;
pub mod insert_record;
pub mod kanban;
pub mod linkly;
pub mod long_transcript_tail;
pub mod mechanical_reaction;
mod mechanism;
pub mod memory;
pub mod minimal_path;
pub mod moving_target;
pub mod onboarding_tour;
pub mod performance_regression;
pub mod persistent_state;
pub mod poison_message;
pub mod policy_bound_action;
pub(crate) mod probe;
pub mod prompt_injection_resilience;
pub mod prompt_layers;
pub mod quorum_fan_in;
pub mod receiving_operation;
pub mod registry;
mod registry_plan;
pub mod release_train_recovery;
pub mod research_pipeline;
pub mod restart_recovery;
pub mod result_bounds;
pub mod router_fault_contract;
pub mod secret_hygiene;
pub mod sequential_pipeline;
pub mod session_continuity;
pub mod shell_coder_sandbox;
pub mod spawn_depth_cap;
pub mod steer_and_stop;
pub mod structured_output;
pub mod subagent_validation;
pub mod subagent_validation_failure;
pub mod subtree_deletion;
pub mod tags;
pub mod timer_wake;
pub mod todo_worker;
pub mod tool_contract_recovery;
pub mod trend_blog;
pub mod trending_topics_build;
pub mod turn_failure_surface;
pub mod typescript_chat_service;
pub mod validation_chain;
pub mod validation_hook;
pub mod validation_loop;
pub mod validation_scope_enforcement;
pub mod validation_self_repair;
pub mod visual_worker_build;
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
    /// A human-readable name, separate from the stable registered id.
    fn title(&self) -> Option<&'static str> {
        None
    }
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
    /// The task the subject receives, in one plain paragraph for a reader.
    fn summary(&self) -> Option<&'static str> {
        None
    }
    /// The product capability the test protects and what breaks for users
    /// of iii when it fails ("If it fails, ..."). Presentation only: like the
    /// title and the summary, it stays out of `behavior_sha256`.
    fn protects(&self) -> Option<&'static str> {
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
    /// The Directory agent profile the subject session runs as, when the
    /// product this test reproduces fixes one. An execution that names no
    /// profile takes it; one that names another is refused.
    fn agent_profile(&self) -> Option<&'static str> {
        None
    }
    fn dialogue_followups(&self, _run_id: &str) -> Vec<String> {
        Vec::new()
    }
    /// Whether follow-up `followup` (an index into `dialogue_followups`) goes
    /// out bare: only `session_id` and `message`, with no model, provider,
    /// options or idempotency key. The session keeps its provider, model,
    /// functions, reasoning, prompt, token and cost budgets and `fs_scope`,
    /// but not the rest of `spec.execution` or `send_metadata`: `max_turns`
    /// and `max_validation_retries` fall back to the Harness defaults, and
    /// `max_output_tokens`, `output` and `options.metadata` go unset.
    /// Folded into `behavior_sha256` when any follow-up is bare.
    fn bare_followup(&self, _followup: usize) -> bool {
        false
    }
    /// How every root turn the runner waits on is meant to end: `Completed`,
    /// or `Failed` or `Cancelled` for a scenario that drives the root into
    /// that outcome on purpose. It holds for every exchange, so a dialogue
    /// whose turns end differently cannot be declared. A root turn that ends
    /// any other way fails the run, as a `Failed` or `Cancelled` one does
    /// under the default, and so does a provider transport failure. Any other
    /// failure ends the wait, whatever caused it: `evaluate` checks the cause
    /// (`harness::status`'s `result_error`). Folded into `behavior_sha256`
    /// when not `Completed`.
    fn expected_root_outcome(&self) -> TurnStatus {
        TurnStatus::Completed
    }
    /// The turn's deliverable, sent as `options.output` on every exchange
    /// that is not bare: `{"type": "json", "schema": …}` asks for a JSON
    /// result, which the Harness returns as the turn's `result`. Folded into
    /// `behavior_sha256` when `Some`.
    fn output_contract(&self) -> Option<Value> {
        None
    }
    /// Extra `harness::send` `options.metadata` keys, sent on every exchange
    /// beside the runner's own, which win on a clash. Part of the task, so it
    /// is folded into `behavior_sha256` when non-empty.
    fn send_metadata(&self, _run_id: &str) -> serde_json::Map<String, Value> {
        serde_json::Map::new()
    }
    /// What `setup` plants before the prompt, rendered for `run_id`, when that
    /// state is part of the task. `setup` must plant exactly this; it is folded
    /// into `behavior_sha256` like the prompt, when `Some`.
    fn planted_state(&self, _run_id: &str) -> Option<Value> {
        None
    }
    /// Called once exchange `exchange` has its answer and before the next
    /// follow-up goes out. A dialogue paced by something other than the turn
    /// (a tour card that moves on when its step closes) waits here.
    async fn before_followup(
        &self,
        _context: &E2eContext,
        _run_id: &str,
        _session_id: &str,
        _exchange: usize,
    ) {
    }
    /// Called once the last exchange has its answer, before the runner reads
    /// `harness::status`, so a probe can read state that read would change.
    async fn after_dialogue(&self, _context: &E2eContext, _run_id: &str, _session_id: &str) {}
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
    /// Whether a subject that failed or was stopped by a resource limit is still
    /// evaluated from what it left behind, instead of reporting no criteria.
    /// The evaluator sees `metrics.complete == false`.
    fn evaluates_failed_subjects(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone)]
pub struct CriterionSpec {
    pub id: &'static str,
    pub weight: u8,
    pub description: &'static str,
    /// The criterion as a short statement for readers ("Stored record matches
    /// the migration exactly"). Stays out of `behavior_sha256` and never
    /// reaches the subject, so rewording it changes neither the definition nor
    /// the task; `description` is what the subject and the digest see.
    pub label: Option<&'static str>,
    pub kind: AssessmentKind,
    pub policy: AssessmentPolicy,
    pub dimension: crate::report::EvaluationDimension,
    /// The task counts as completed only when every gate criterion passed;
    /// the others only move the score. Reported so a reader can say which
    /// criterion left a task incomplete.
    pub gate: bool,
    /// Points that already meet the task's purpose on a proportional gate;
    /// `None` means full points.
    pub gate_minimum: Option<u8>,
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
            label: None,
            kind: AssessmentKind::Signal,
            policy: AssessmentPolicy::Advisory,
            dimension,
            gate: false,
            gate_minimum: None,
        }
    }

    pub const fn with_gate(self, gate: bool) -> Self {
        Self { gate, ..self }
    }

    pub const fn labeled(self, label: &'static str) -> Self {
        Self {
            label: Some(label),
            ..self
        }
    }

    /// A gate that already passes at `points`, below the criterion's weight.
    pub const fn with_gate_minimum(self, points: u8) -> Self {
        Self {
            gate: true,
            gate_minimum: Some(points),
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionPolicy {
    /// Subject turn ceiling. `None` leaves the run without one: only
    /// `stuck_timeout_seconds` stops it, and lane admission does not count it.
    pub max_turns: Option<u32>,
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
    fn validate(self, scenario_id: &str, expected_outcome: TurnStatus) -> Result<()> {
        if self.max_turns == Some(0) {
            bail!("scenario '{scenario_id}': execution.max_turns=0; expected None (unbounded) or at least 1");
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
        // A root meant to fail may carry a budget that cannot fit its own
        // output cap: the Harness refusing it is what such a scenario tests.
        if expected_outcome == TurnStatus::Completed
            && self.max_output_tokens.is_some_and(|max_output_tokens| {
                self.max_total_tokens
                    .is_some_and(|max_total_tokens| max_total_tokens < max_output_tokens)
            })
        {
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
        self.spec
            .validate_expecting(self.module.expected_root_outcome())?;
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
        self.validate_expecting(TurnStatus::Completed)
    }

    /// `validate` for a scenario whose root turns end as `expected_outcome`.
    pub fn validate_expecting(&self, expected_outcome: TurnStatus) -> Result<()> {
        if self.prompt.trim().is_empty() {
            bail!(
                "scenario '{}': prompt is empty after trimming; provide a non-empty task prompt",
                self.id
            );
        }
        self.execution.validate(self.id, expected_outcome)?;
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

impl ScenarioObservation {
    /// A complete single-session, single-turn run of `scenario`'s canonical
    /// case, for evaluator tests.
    #[cfg(test)]
    pub(crate) fn for_test(
        scenario: ScenarioId,
        run_id: &str,
        transcript: Value,
        response: &str,
        deliverables: Vec<CapturedDeliverable>,
    ) -> Self {
        Self {
            case: scenario
                .materialize(run_id, scenario.canonical_seed())
                .expect("materialize the canonical case")
                .case,
            metrics: SessionMetricsResponse::from_normalized(crate::wire::SessionMetricsPayload {
                root_session_id: format!("e2e_{run_id}"),
                complete: true,
                totals: crate::wire::SessionUsageTotals {
                    sessions: 1,
                    turns: 1,
                    ..Default::default()
                },
                by_session: Vec::new(),
                traces: None,
            }),
            transcript,
            response: response.to_string(),
            deliverables,
        }
    }
}

pub struct ObjectiveEvaluation {
    /// Whether the subject delivered the task's primary flow. `None` derives
    /// it from the gate criteria (`crate::report::gate_completion`); a
    /// scenario sets it only when a prerequisite decides before any gate, such
    /// as a build that never started. Independent from score: a completed task
    /// may still lose points on the other criteria.
    pub completion: Option<CompletionState>,
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
    OnboardingTour = "onboarding_tour" => onboarding_tour::OnboardingTour,
    ContextPressure = "context_pressure" => context_pressure::ContextPressure,
    MinimalPath = "minimal_path" => minimal_path::MinimalPath,
    PersistentState = "persistent_state" => persistent_state::PersistentState,
    InsertRecord = "insert_record" => insert_record::InsertRecord,
    SequentialPipeline = "sequential_pipeline" => sequential_pipeline::SequentialPipeline,
    DatabaseMigrationRecovery = "database_migration_recovery" => database_migration_recovery::DatabaseMigrationRecovery,
    ShellCoderSandbox = "shell_coder_sandbox" => shell_coder_sandbox::ShellCoderSandbox,
    ResearchPipeline = "research_pipeline" => research_pipeline::ResearchPipeline,
    FanoutLadder = "fanout_ladder" => fanout_ladder::FanoutLadder,
    IncidentResponse = "incident_response" => incident_response::IncidentResponse,
    TodoWorkerSimple = "todo_worker_simple" => todo_worker::TodoWorkerSimple,
    TodoWorkerPlanned = "todo_worker_planned" => todo_worker::TodoWorkerPlanned,
    EngineeringEnduranceLadder = "engineering_endurance_ladder" => engineering_endurance_ladder::EngineeringEnduranceLadder,
    GitRegressionForensics = "git_regression_forensics" => git_regression_forensics::GitRegressionForensics,
    AlertmanagerRouteMatch = "alertmanager_route_match" => alertmanager_route_match::AlertmanagerRouteMatch,
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
    FormFlowBuild = "form_flow_build" => visual_worker_build::FormFlowBuild,
    StateMachineCanvasBuild = "state_machine_canvas_build" => visual_worker_build::StateMachineCanvasBuild,
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
    MemoryRelevantRecall = "memory_relevant_recall" => memory::Memory(0),
    MemorySmallTalk = "memory_small_talk" => memory::Memory(1),
    MemoryFirstPersonOffTopic = "memory_first_person_off_topic" => memory::Memory(2),
    MemoryParaphrase = "memory_paraphrase" => memory::Memory(3),
    MemorySessionRepeat = "memory_session_repeat" => memory::Memory(4),
    MemoryUpdatedFact = "memory_updated_fact" => memory::Memory(5),
    MemoryValueProject = "memory_value_project" => memory::Memory(6),
    MemoryValueProjectBaseline = "memory_value_project_baseline" => memory::Memory(7),
    MemoryValuePreference = "memory_value_preference" => memory::Memory(8),
    MemoryValuePreferenceBaseline = "memory_value_preference_baseline" => memory::Memory(9),
    MemoryRuleFit = "memory_rule_fit" => memory::Memory(10),
    MemoryChatterAfterAnswer = "memory_chatter_after_answer" => memory::Memory(11),
    MemoryUserFactNeeded = "memory_user_fact_needed" => memory::Memory(12),
    MemoryAboutMe = "memory_about_me" => memory::Memory(13),
    MemoryRewordedAcrossTurns = "memory_reworded_across_turns" => memory::Memory(14),
    MemorySiblingFacts = "memory_sibling_facts" => memory::Memory(15),
    MemoryRuleChangedMidSession = "memory_rule_changed_mid_session" => memory::Memory(16),
    MemoryBankEditsMidSession = "memory_bank_edits_mid_session" => memory::Memory(17),
    MemoryBankTrashedMidSession = "memory_bank_trashed_mid_session" => memory::Memory(18),
    MemoryManySiblings = "memory_many_siblings" => memory::Memory(19),
    MemoryAboutMeOverflow = "memory_about_me_overflow" => memory::Memory(20),
    MemoryLinkedUpdate = "memory_linked_update" => memory::Memory(21),
    MemoryLearnThenRecall = "memory_learn_then_recall" => memory::Memory(22),
    MemoryLearnRuleThenObey = "memory_learn_rule_then_obey" => memory::Memory(23),
    MemoryLearnCorrection = "memory_learn_correction" => memory::Memory(24),
    MemoryAutomationNotLearned = "memory_automation_not_learned" => memory::Memory(25),
    MemoryLearnFromLongMessage = "memory_learn_from_long_message" => memory::Memory(26),
    MemoryHonorsDoNotKeep = "memory_honors_do_not_keep" => memory::Memory(27),
    MemoryKeepsWithoutARule = "memory_keeps_without_a_rule" => memory::Memory(28),
    SpawnDepthCap = "spawn_depth_cap" => spawn_depth_cap::SpawnDepthCap,
    RepeatedFailureBreaker = "repeated_failure_breaker" => call_guards::RepeatedFailureBreaker,
    TruncatedCallGuard = "truncated_call_guard" => call_guards::TruncatedCallGuard,
    RestartRecovery = "restart_recovery" => restart_recovery::RestartRecovery,
    ApprovalRoundtrip = "approval_roundtrip" => approval_roundtrip::ApprovalRoundtrip,
    HarnessContractProbe = "harness_contract_probe" => harness_contract_probe::HarnessContractProbe,
    DispatchPipeline = "dispatch_pipeline" => dispatch_pipeline::DispatchPipeline,
    BindingContractProbe = "binding_contract_probe" => binding_contract_probe::BindingContractProbe,
    TurnFailureSurface = "turn_failure_surface" => turn_failure_surface::TurnFailureSurface,
    ContextCompaction = "context_compaction" => context_compaction::ContextCompaction,
    ResultBounds = "result_bounds" => result_bounds::ResultBounds,
    StructuredOutput = "structured_output" => structured_output::StructuredOutput,
    AskHandshake = "ask_handshake" => ask_handshake::AskHandshake,
    LongTranscriptTail = "long_transcript_tail" => long_transcript_tail::LongTranscriptTail,
    SubtreeDeletion = "subtree_deletion" => subtree_deletion::SubtreeDeletion,
    SessionContinuity = "session_continuity" => session_continuity::SessionContinuity,
    SteerAndStop = "steer_and_stop" => steer_and_stop::SteerAndStop,
    RouterFaultContract = "router_fault_contract" => router_fault_contract::RouterFaultContract,
    PromptLayers = "prompt_layers" => prompt_layers::PromptLayers,
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

    pub fn title(self) -> Option<&'static str> {
        self.module().title()
    }

    pub fn protects(self) -> Option<&'static str> {
        self.module().protects()
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
    crate::artifact::sha256_value(&behavior_definition(id.module(), case)?)
}

/// Whether exchange `exchange` (0 is the prompt) goes out bare.
pub(crate) fn sends_bare(module: &dyn Scenario, exchange: usize) -> bool {
    exchange > 0 && module.bare_followup(exchange - 1)
}

/// The JSON that `behavior_sha256` hashes.
pub(crate) fn behavior_definition(module: &dyn Scenario, case: &ScenarioCase) -> Result<Value> {
    let spec = module.spec(CONTRACT_NAMESPACE);
    let mut definition = serde_json::json!({
        "scenario_id": module.id(),
        "prompt": spec.prompt,
        "execution": spec.execution,
        "denied_functions": spec.denied_functions,
        "criteria": spec
            .criteria
            .iter()
            .map(|criterion| {
                let mut rendered = serde_json::json!({
                    "id": criterion.id,
                    "weight": criterion.weight,
                    "description": criterion.description,
                    "kind": criterion.kind,
                    "policy": criterion.policy,
                    "dimension": criterion.dimension,
                });
                // Written only when set, so a definition without gates keeps its digest.
                if criterion.gate {
                    rendered["gate"] = true.into();
                }
                rendered
            })
            .collect::<Vec<_>>(),
        "characterization": case.characterization,
        "required_capabilities": case.required_capabilities,
        "deliverable_contract": case.deliverable_contract,
    });
    // What the subject may call and what it is told next change the task;
    // written only when a scenario declares them, like `gate` above.
    if let Some(functions) = module.allowed_functions(CONTRACT_NAMESPACE) {
        definition["allowed_functions"] = serde_json::json!(functions);
    }
    let followups = module.dialogue_followups(CONTRACT_NAMESPACE);
    if !followups.is_empty() {
        definition["dialogue_followups"] = serde_json::json!(followups);
    }
    let bare: Vec<usize> = (0..followups.len())
        .filter(|&followup| module.bare_followup(followup))
        .collect();
    if !bare.is_empty() {
        definition["bare_followups"] = serde_json::json!(bare);
    }
    let outcome = module.expected_root_outcome();
    if outcome != TurnStatus::Completed {
        definition["expected_root_outcome"] = serde_json::json!(outcome);
    }
    if let Some(output) = module.output_contract() {
        definition["output_contract"] = output;
    }
    let send_metadata = module.send_metadata(CONTRACT_NAMESPACE);
    if !send_metadata.is_empty() {
        definition["send_metadata"] = Value::Object(send_metadata);
    }
    if let Some(planted) = module.planted_state(CONTRACT_NAMESPACE) {
        definition["planted_state"] = planted;
    }
    if let Some(append) = crate::mechanism_gates::prompt_append(module.id(), CONTRACT_NAMESPACE) {
        definition["pre_generate_append"] = append.into();
    }
    let run_dir = std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    without_run_dir(&mut definition, &run_dir);
    Ok(definition)
}

/// Scenario workspaces live under the host's run directory
/// (`HARNESS_E2E_RUN_DIR`, else the temp dir), and some prompts name them. The
/// digest reads that directory as `/tmp`, its value on a host with neither
/// set, so one definition has one digest on every host.
fn without_run_dir(value: &mut Value, run_dir: &Path) {
    let prefixes: Vec<String> = [
        Some(run_dir.to_path_buf()),
        std::fs::canonicalize(run_dir).ok(),
    ]
    .into_iter()
    .flatten()
    .filter_map(|base| {
        base.to_str()
            .map(|base| format!("{}/", base.trim_end_matches('/')))
    })
    .filter(|prefix| prefix.len() > 1 && prefix != "/tmp/")
    .collect();
    fn walk(value: &mut Value, prefixes: &[String]) {
        match value {
            Value::String(text) => {
                for prefix in prefixes {
                    if text.contains(prefix.as_str()) {
                        *text = text.replace(prefix.as_str(), "/tmp/");
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|item| walk(item, prefixes)),
            Value::Object(fields) => fields.values_mut().for_each(|field| walk(field, prefixes)),
            _ => {}
        }
    }
    walk(value, &prefixes);
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
    #[test]
    fn a_definition_reads_the_run_directory_as_tmp() {
        let run_dir = std::env::temp_dir().join("harness-e2e-run-dir-digest-test");
        let mut here = serde_json::json!({
            "prompt": format!("Work in {}/scenario-workspaces/shell-coder-x.", run_dir.display()),
            "nested": [{"path": format!("{}/kanban-evaluation/y", run_dir.display())}],
        });
        super::without_run_dir(&mut here, &run_dir);
        assert_eq!(
            here,
            serde_json::json!({
                "prompt": "Work in /tmp/scenario-workspaces/shell-coder-x.",
                "nested": [{"path": "/tmp/kanban-evaluation/y"}],
            })
        );
        let mut on_tmp = serde_json::json!({"prompt": "Work in /tmp/scenario-workspaces/a."});
        let before = on_tmp.clone();
        super::without_run_dir(&mut on_tmp, std::path::Path::new("/tmp"));
        assert_eq!(on_tmp, before);
        let mut root = serde_json::json!({"prompt": "/etc/hosts"});
        super::without_run_dir(&mut root, std::path::Path::new("/"));
        assert_eq!(root, serde_json::json!({"prompt": "/etc/hosts"}));
    }

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

    /// minimal_path with a bare follow-up and a root meant to fail.
    struct BareFailingMinimalPath;

    #[async_trait]
    impl Scenario for BareFailingMinimalPath {
        fn id(&self) -> &'static str {
            "minimal_path"
        }

        fn case(&self, seed: u64) -> Result<ScenarioCase> {
            ScenarioId::MinimalPath.module().case(seed)
        }

        fn spec(&self, run_id: &str) -> ScenarioSpec {
            ScenarioId::MinimalPath.spec(run_id)
        }

        fn dialogue_followups(&self, _run_id: &str) -> Vec<String> {
            vec!["Answer again.".into(), "Once more.".into()]
        }

        fn bare_followup(&self, followup: usize) -> bool {
            followup == 1
        }

        fn expected_root_outcome(&self) -> TurnStatus {
            TurnStatus::Failed
        }

        fn output_contract(&self) -> Option<Value> {
            Some(serde_json::json!({ "type": "json" }))
        }

        async fn evaluate(
            &self,
            _context: &E2eContext,
            _observation: &ScenarioObservation,
            _run_id: &str,
        ) -> Result<ObjectiveEvaluation> {
            unreachable!("the definition test never evaluates")
        }
    }

    /// Each changes what the run asks or what counts as its end, so each is
    /// part of the definition; a scenario that declares none keeps its digest.
    #[test]
    fn a_bare_follow_up_an_expected_outcome_and_an_output_contract_are_part_of_the_definition() {
        let case = ScenarioId::MinimalPath
            .materialize("attempt-a", ScenarioId::MinimalPath.canonical_seed())
            .unwrap()
            .case;
        let plain = behavior_definition(ScenarioId::MinimalPath.module(), &case).unwrap();
        assert!(plain.get("bare_followups").is_none());
        assert!(plain.get("expected_root_outcome").is_none());
        assert!(plain.get("output_contract").is_none());
        let declared = behavior_definition(&BareFailingMinimalPath, &case).unwrap();
        assert_eq!(declared["bare_followups"], serde_json::json!([1]));
        assert_eq!(declared["expected_root_outcome"], "failed");
        assert_eq!(
            declared["output_contract"],
            serde_json::json!({ "type": "json" })
        );
    }

    /// Exchange 0 is the prompt, so follow-up `n` is exchange `n + 1`.
    #[test]
    fn a_bare_follow_up_is_the_exchange_after_its_index() {
        let bare: Vec<bool> = (0..3)
            .map(|exchange| sends_bare(&BareFailingMinimalPath, exchange))
            .collect();
        assert_eq!(bare, [false, false, true]);
        assert!(!(0..3).any(|exchange| sends_bare(ScenarioId::MinimalPath.module(), exchange)));
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
    fn registry_contains_every_unique_valid_scenario() {
        let mut ids = HashSet::new();
        for scenario in ScenarioId::ALL {
            assert!(ids.insert(scenario.as_str()));
            scenario
                .spec("run")
                .validate_expecting(scenario.module().expected_root_outcome())
                .unwrap();
            scenario
                .materialize("run", scenario.canonical_seed())
                .unwrap();
        }
        assert_eq!(ids.len(), 108);
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
        assert!(selected.contains(&ScenarioId::IncidentResponse));
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
                assert!(first.case.deliverable_contract.artifacts.is_empty());
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

    /// Completed means the primary flow was delivered, and the gate criteria
    /// say which flow that is: a scenario without one can never complete.
    #[test]
    fn every_scenario_declares_the_gates_its_completion_reads() {
        let ungated: Vec<_> = ScenarioId::ALL
            .into_iter()
            .filter(|scenario| {
                !scenario
                    .spec(CONTRACT_NAMESPACE)
                    .criteria
                    .iter()
                    .any(|criterion| criterion.gate)
            })
            .collect();
        assert!(ungated.is_empty(), "scenarios without a gate: {ungated:?}");
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

    /// Every definition's digest before the memory group learned changes
    /// between exchanges: a definition that does not use a new shape keeps its
    /// digest, and so its Trends history. A reviewed change to a definition
    /// updates its line.
    const PINNED_DIGESTS: &str = "\
registry_planning sha256:c5f85e844c58604a42b2553ab26662cc420a993e94df1951bb84bdd0089f54eb
registry_implementation sha256:56b18cc3cab95f65cbdf1af67539643455410a98af66ebf9e8759af542bb9bb8
registry_environment sha256:53a0401db5840ba6615085e17afb4e68b8737792bb0fdf2b4bd5eef7f1995498
registry_verification sha256:b594ef155bf5a08bcfcfda1ec49fe6bc5b3aa8e35798d27d3bb45c657e98caa8
kanban_c1_foundation sha256:3b4148ba8a973c831ff173ff35e16ba6079ce110e4916643c016f6b5a16a8f95
kanban_c2_persistence sha256:8600417d28c712bb0ece0ca9ab767e4128ea615eda2863b05ea965e5c40ee9c2
kanban_c3_board sha256:447d221634ef39ddef36c8b0ee2c7c8d29390c0abaae0522813e83ae181f692e
kanban_c4_ticket_flow sha256:d54f87c43e7f02256d51b76858e2f12c58caae6159a6db3840aff906ba74763e
kanban_c5_edit_move sha256:af3166db87f0cd498e016dba1288225720d42d157a326b85fa9706f8c79e8cdc
kanban_c6_discussion sha256:13a56344e8e8766bae7ec28605b8d833e5d451d5b243eb0f541bc5bbfd8071a3
kanban_c7_live sha256:bc319576c7fe79c380febf9f01c6725fceacc658104e2a5c27ac43a7f9af54bd
linkly_tutorial sha256:110e730807b9f9c53f0222e1b95303af3bafc30e8347dc2011bba96658387374
onboarding_tour sha256:0f26ffac65dc934f554b913b07de20f15f3dd8f3d046fc4305ed53b1d88c5084
context_pressure sha256:fc645c2bbe17daa2a74283d981de82a7e37acf13240adc67485cd18a34bda94e
minimal_path sha256:d5a495f66901fd687efca985c6c78c364325551aee8eac2fa3773a7aa5d407d0
persistent_state sha256:75a648bd4238524cc3744244d12720ebab49281d9533feafd9590001e56ba785
insert_record sha256:c3b73ca9695b2f5acecf49f0a76227f0908814b56a9951fefd05a7fcfc6ba090
sequential_pipeline sha256:c511216e5705dd787fdd5486b6f9e26f02caa01682fecf68a195527cb113d951
database_migration_recovery sha256:a3fe7348010503795b56dbe2b5e47bfb85801dab5fae71b7937c81b5f4abd4d9
shell_coder_sandbox sha256:8dc5cf91d180a51501d8d13aa429b56c05f0d4cddd8dda12593815c5497863e0
research_pipeline sha256:e142a6ce5fccbfb36675346f1a5ce6adc75ea4ff0b046dcd385dd85dd429bd81
fanout_ladder sha256:4034b0b48d93a2a89966a6326e3c878e58ff89c4612e1b3f2d509fc654e82756
incident_response sha256:796d4e1e5ad36166eec6060940427276042adbe9d5a5e7c4506cea33dc473ca9
todo_worker_simple sha256:beadcfc64320a3ce00078ed803b646876a0ef1eb75c89cd857a19d87d9387b5a
todo_worker_planned sha256:2e202de0eec7c1fd001245504c476e044ee61798322053de54d8776b9677bd7b
engineering_endurance_ladder sha256:d7c4d3a38fd206e5d74762a52a607ac6ac9c5cab76269590dd73e01e91e439f7
git_regression_forensics sha256:548addd6d43de6cf76ae04802fd52a5a8de8e3d8db88a66a2e3e36cf0afc2cde
alertmanager_route_match sha256:bf79dbf58ffbe6ee538a7c5683c210130de7b54e075a969a6e8b4e389adcc417
mechanical_reaction sha256:59aa47c9eaca92b773e3e64124923a2c3e00e9dfebddb0207eb6f8e02ab31796
timer_wake sha256:85cdffec64a840da833cc67005e6bd712fb1db3650a27aeb8d1087638476f3eb
receiving_operation sha256:c165a668c43f005371269dc9353bfe296f8606a7b1926a8ece7c296f12e9febe
validation_loop sha256:032e02b0b5a371cf2180ddde18707cd3b5a9f0a7c7db89df47be73d3c499542e
subagent_validation sha256:6d11bdcfcbd12ef0740cb64bc5c82f52266fd4c36651bebae4acd56731a9fd74
subagent_validation_failure sha256:9fae059f4801853b0fefa711224870c787ec00941319ef4e09bcf7b67f533e0b
validation_self_repair sha256:d9cea81c3e707865f4cb735a556b10b11ee8a71949f828fe6f6198c5da6cd5f9
validation_scope_enforcement sha256:d3bf0bbc8d66e4ff5473648b64f1d76ffa92c3621235c55cfa39f3053ba9cedb
validation_chain sha256:f19c3925668ad492d24ab5d907fb2f2f151795e6f0cbc69553aa41735a21b5df
secret_hygiene sha256:b5260d266b30aac01b235ceca7351d9750e88745a65169d18fcfc051f77cda93
prompt_injection_resilience sha256:1dba6f36fadd6c186fa3c89f10e1854659f7d73898f061a5a31c230b3751c5b4
moving_target sha256:0fa71482e91849566b8fc6cdcfe6646a7cab5eb90d7b3eba98fbf36611e8f281
poison_message sha256:3f22af3c5356655e1d09762fd415ba05f241f38911beccfe5ec8bbe5e3142e91
cleanup_under_failure sha256:0cbe9e6c86c675a3bc22e1a4d98c109e2f4f571e0f9e529216c09b9a5ad2e573
depth_ladder sha256:e1fc32e5f26df84be624bf896782a40d545bf612fe8f9ddd8b54ddd2e4c12d70
quorum_fan_in sha256:18c501ea8c52843fb56fb3e7d7564b857728202c638868206f7bb2bfd9142fe3
contention_ledger sha256:102c073253252c4e1641126afa92783851a5ec3961a488bbaa6fc1fc3b98948d
wake_chain_soak sha256:e4c73231913a0cdfa80459ff26edf2217f49219591290e338f1c06856235f7df
chess_engine_build sha256:0519a3f287c76c39d7565def4a7fc220c98d89645f75263b098e06ee57086c3d
form_flow_build sha256:83eb213cd23d2ef34753428005ddc1da773aa893b6856c71d9e7fe5ace69b98c
state_machine_canvas_build sha256:e498dbaf9dcd54c6fcf5290cf2bee340dd656821631e21da4de9f16150d87977
chess_play_ladder sha256:98550327e6f7c95a4a48e61344996e06482db550976549ea053f1a6e17d83899
trend_blog sha256:3c3434652032c9bf4e7f0c3d0bdb68c6ea4fd15d55dd57de4e28583c88d9925b
trending_topics_build sha256:ed5dee1ef9a8593f753d17370be340a234677650aada93001d3ca1d562f4e61f
typescript_chat_service sha256:5ed2f47723600fc7aa515a8a6b8f00257e5933531772eda05f5852a4ae1c142e
tool_contract_recovery sha256:f58fbfadb14b5ad0890a72c1a087a2e9fd4ae729a09f474bed2150b4c5c888fa
policy_bound_action sha256:1f91a05a93f084603b89a3e3cde743b504cd0fae59e57981a9f924ea85ba1594
cross_app_transaction sha256:517799ab2c04d8e390561b6042f0e5555b97e3f56727bf43aa74c0579c66a65d
performance_regression sha256:9463852c5b414dcc5d4d506d034f5c3c6da729479c6cb6808ad04a83aeeebd2f
browser_cross_site sha256:302a897b1699a3c4e3dd5379aac6d5e7862482760852c78278ec66d71085fd38
release_train_recovery sha256:87924562b3a74393c2b3867faf0b9aac4a6782bcb8ba1c39e23e5bb1011c5742
cross_repo_contract_migration sha256:66749ee9620fffeb1c3147f579c45e393c3e97ad61d2d7ffaedca306c0e5d70f
memory_relevant_recall sha256:21950aaa628bc6420a1c48c66253f3cfd97a255b86c4007e58a9395a835c5bb0
memory_small_talk sha256:f24fe3dedfae4c589e35947b90ba82911d197bf39107d72d8703ac3d0e6e4461
memory_first_person_off_topic sha256:f1586fad450f70e227fc203d66bb607f61fa2052e44e628962d75f435a4539f6
memory_paraphrase sha256:fdfdc4491c2db8eeaec04608668bdb19d2d7fca18cb1763bd2b4b36dae112d8f
memory_session_repeat sha256:5aba0b697da604ae88e9ab783554334c2fbf31b3e4191bf14e7bed2afeb86dce
memory_updated_fact sha256:dfc77453697643373f246c295485e22fa638c14b5381c08a126c084e8baec4bf
memory_value_project sha256:04466e18377c77f724ae6d8644324476df598e49204644fdb358cefef9b12997
memory_value_project_baseline sha256:69b793117db5730a9dff9bef4f0c9f1ebb78a56aa1c3da2c4d34d5044701575d
memory_value_preference sha256:cdfea0a237ae200c55cfed5508e782532579e3e483f23d9c85aa1114501e401c
memory_value_preference_baseline sha256:ffc4384bbc223e4a0d829c368ba7226d7fe5ee9a207e36f41672434cd8073b1c
memory_rule_fit sha256:7a1773ee7a8228dfc1cb2226563aeb553e3990d1578b8a8a69cb4f8910b666b0";

    #[test]
    fn the_existing_behavior_digests_do_not_move() {
        let run_dir = std::env::var_os("HARNESS_E2E_RUN_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let pinned: Vec<(&str, &str)> = PINNED_DIGESTS
            .lines()
            .map(|line| line.split_once(' ').unwrap())
            .collect();
        assert_eq!(pinned.len(), 71);
        for (id, digest) in pinned {
            // shortcut: todo_worker_simple hashes its contract before the run dir
            // is normalized, so its line holds only where the run dir is /tmp;
            // drop this skip once that contract hash is normalized.
            if id == "todo_worker_simple" && run_dir != Path::new("/tmp") {
                continue;
            }
            let scenario: ScenarioId = id.parse().unwrap();
            let case = scenario
                .materialize("attempt-a", scenario.canonical_seed())
                .unwrap()
                .case;
            assert_eq!(case.behavior_sha256, digest, "{id}");
        }
    }

    /// The line the mechanism probe appends to every `persistent_state` prompt
    /// is what the subject reads, so its definition carries it.
    #[test]
    fn a_scenario_whose_prompt_a_mechanism_probe_extends_carries_the_extension() {
        let definition = |id: ScenarioId| {
            let case = id
                .materialize("attempt-a", id.canonical_seed())
                .unwrap()
                .case;
            behavior_definition(id.module(), &case).unwrap()
        };

        assert!(
            definition(ScenarioId::PersistentState)["pre_generate_append"]
                .as_str()
                .is_some_and(|line| line.starts_with("[e2e mechanism probe "))
        );
        assert!(definition(ScenarioId::MinimalPath)
            .get("pre_generate_append")
            .is_none());
    }

    /// The bank, what `setup` plants and what the case changes later are part
    /// of the task, so they sit in the digest; a baseline plants nothing.
    #[test]
    fn memory_definitions_fold_the_bank_and_the_planted_state() {
        use super::memory::cases::{saved_after, CASES, DISTRACTORS};

        assert_eq!(memory::IDS.len(), CASES.len());
        for (index, id) in memory::IDS.iter().enumerate() {
            let case = &CASES[index];
            let scenario = ScenarioId::ALL
                .iter()
                .copied()
                .find(|scenario| scenario.as_str() == *id)
                .unwrap();
            // A transposed `Memory(i)` in the registry runs another case's spec.
            assert_eq!(scenario.spec("x").id, scenario.as_str(), "{id}");
            let materialized = scenario
                .materialize("attempt-a", scenario.canonical_seed())
                .unwrap();
            let definition = behavior_definition(scenario.module(), &materialized.case).unwrap();
            assert_eq!(
                definition["send_metadata"],
                serde_json::json!({"memory_bank": "e2e-contract"}),
                "{id}"
            );
            assert_eq!(
                definition["allowed_functions"],
                serde_json::json!([]),
                "{id}"
            );
            let planted = &definition["planted_state"];
            let memories = planted["memories"].as_array().unwrap();
            let rules = planted["rules"].as_array().unwrap();
            // A case with changes may plant nothing up front and save later;
            // one may plant nothing for its teacher to fill, or to show memory
            // learns nothing from the session.
            let fills_later = !case.changes.is_empty()
                || !case.teacher.is_empty()
                || memory::cases::audits_bank_after(case);
            if !fills_later {
                assert_eq!(
                    memories.is_empty() && rules.is_empty(),
                    case.baseline,
                    "{id}"
                );
            }
            assert_eq!(
                planted.get("teacher"),
                (!case.teacher.is_empty())
                    .then(|| serde_json::json!(case.teacher))
                    .as_ref(),
                "{id}"
            );
            let changes = planted.get("changes").and_then(Value::as_array);
            assert_eq!(changes.map_or(0, Vec::len), case.changes.len(), "{id}");
            if !case.baseline {
                let distractors = if case.distractors {
                    DISTRACTORS.len()
                } else {
                    0
                };
                let saved_later = (0..case.memories.len())
                    .filter(|&seed| saved_after(case, seed).is_some())
                    .count();
                assert_eq!(
                    memories.len(),
                    distractors + case.memories.len() - saved_later,
                    "{id}"
                );
                assert_eq!(rules.len(), case.rules.len(), "{id}");
            }
        }
    }

    #[test]
    fn validation_identifies_the_invalid_execution_field() {
        type ValidationCase = (&'static str, fn(&mut ExecutionPolicy), &'static str);

        let cases: [ValidationCase; 5] = [
            (
                "max_turns",
                |execution| execution.max_turns = Some(0),
                "scenario 'context_pressure': execution.max_turns=0; expected None (unbounded) or at least 1",
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
