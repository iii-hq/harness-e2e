use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use anyhow::{bail, Result};
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
pub mod depth_ladder;
mod domain;
pub mod engineering_endurance_ladder;
pub mod engineering_ticket;
pub mod fanout_ladder;
pub mod git_regression_forensics;
pub mod incident_response;
pub mod mechanical_reaction;
pub mod moving_target;
pub mod performance_regression;
pub mod poison_message;
pub mod policy_bound_action;
pub mod prompt_injection_resilience;
pub mod quorum_fan_in;
pub mod receiving_operation;
pub mod registry;
pub mod release_train_recovery;
pub mod research_pipeline;
pub mod secret_hygiene;
pub mod security_review;
pub mod shell_coder_sandbox;
pub mod subagent_validation;
pub mod subagent_validation_failure;
pub mod swe_service;
pub mod timer_wake;
pub mod todo_worker;
pub mod tool_contract_recovery;
pub mod trend_blog;
pub mod typescript_chat_service;
pub mod validation_chain;
pub mod validation_hook;
pub mod validation_loop;
pub mod validation_scope_enforcement;
pub mod validation_self_repair;
pub mod wake_chain_soak;

pub use domain::{
    scenario_contract_sha256, stable_seed, ArtifactExpectation, CapturedDeliverable,
    CapturedDeliverableContent, CapturedInvariant, ComplexityClassification, ComplexityMethod,
    ComplexityProfile, ComplexityTier, DeliverableContract, ExecutionRealism, HumanHorizon,
    HumanHorizonBasis, InvariantSpec, ProvenanceEvidence, ScenarioCase, ScenarioCharacterization,
    ScenarioRealism, ShadowMode, WorkExpectation,
};

pub type EvaluationFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ObjectiveEvaluation>> + Send + 'a>>;
pub type DeliverableCaptureFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<CapturedDeliverable>>> + Send + 'a>>;
pub type CleanupFuture<'a> = Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
pub type ScenarioEvaluator =
    for<'a> fn(&'a E2eContext, &'a ScenarioObservation, &'a str) -> EvaluationFuture<'a>;
pub type ScenarioCleanup = for<'a> fn(&'a E2eContext, &'a str) -> CleanupFuture<'a>;
pub type ScenarioDeliverableCapture =
    for<'a> fn(&'a E2eContext, &'a ScenarioObservation, &'a str) -> DeliverableCaptureFuture<'a>;
/// Pre-send hook: provision what the prompt refers to (e.g. register a
/// temporary validator function on the suite's own worker connection).
pub type ScenarioSetup = for<'a> fn(&'a E2eContext, &'a str) -> CleanupFuture<'a>;

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
    /// Increment when the scenario's behavioral contract changes. Structural
    /// refactors that preserve prompts, gates, criteria, and policy keep it.
    pub version: u32,
    pub prompt: String,
    pub filesystem_root: Option<PathBuf>,
    pub execution: ExecutionPolicy,
    pub denied_functions: &'static [&'static str],
    pub criteria: Vec<CriterionSpec>,
    /// Runs BEFORE the prompt is sent; a failure aborts the run.
    pub setup: Option<ScenarioSetup>,
    pub evaluate: ScenarioEvaluator,
    pub cleanup: Option<ScenarioCleanup>,
}

pub struct MaterializedScenario {
    pub spec: ScenarioSpec,
    pub case: ScenarioCase,
    pub capture: Option<ScenarioDeliverableCapture>,
}

impl MaterializedScenario {
    pub fn validate(&self) -> Result<()> {
        self.spec.validate()?;
        self.case.validate()?;
        if self.spec.id != self.case.scenario_id {
            bail!(
                "materialized scenario id '{}' differs from case id '{}'",
                self.spec.id,
                self.case.scenario_id
            );
        }
        if self.spec.version != self.case.scenario_version {
            bail!(
                "scenario '{}' version {} differs from case version {}",
                self.spec.id,
                self.spec.version,
                self.case.scenario_version
            );
        }
        // Composite workflows capture through their trusted steps and cleanup hook.
        let workflow_capture = self.capture.is_none()
            && !self.case.deliverable_contract.artifacts.is_empty()
            && ScenarioId::ALL.iter().any(|scenario| {
                scenario.as_str() == self.spec.id
                    && scenario.execution_kind() == ScenarioExecutionKind::CompositeFlow
            });
        if self.case.deliverable_contract.artifacts.is_empty() != self.capture.is_none()
            && !workflow_capture
        {
            bail!(
                "scenario '{}' must declare both a deliverable contract and capture hook, or neither",
                self.spec.id
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
        if self.version == 0 {
            bail!("scenario '{}': version=0; expected version >= 1", self.id);
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
    pub awarded: u8,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioId {
    #[value(name = "registry_planning")]
    RegistryPlanning,
    #[value(name = "registry_implementation")]
    RegistryImplementation,
    #[value(name = "registry_environment")]
    RegistryEnvironment,
    #[value(name = "registry_verification")]
    RegistryVerification,
    #[value(name = "context_pressure")]
    ContextPressure,
    #[value(name = "shell_coder_sandbox")]
    ShellCoderSandbox,
    #[value(name = "research_pipeline")]
    ResearchPipeline,
    #[value(name = "fanout_ladder")]
    FanoutLadder,
    #[value(name = "security_review")]
    SecurityReview,
    #[value(name = "incident_response")]
    IncidentResponse,
    #[value(name = "todo_worker_simple")]
    TodoWorkerSimple,
    #[value(name = "todo_worker_planned")]
    TodoWorkerPlanned,
    #[value(name = "engineering_ticket")]
    EngineeringTicket,
    #[value(name = "engineering_ticket_git_handoff")]
    EngineeringTicketGitHandoff,
    #[value(name = "engineering_endurance_ladder")]
    EngineeringEnduranceLadder,
    #[value(name = "git_regression_forensics")]
    GitRegressionForensics,
    #[value(name = "mechanical_reaction")]
    MechanicalReaction,
    #[value(name = "timer_wake")]
    TimerWake,
    #[value(name = "receiving_operation")]
    ReceivingOperation,
    #[value(name = "validation_loop")]
    ValidationLoop,
    #[value(name = "subagent_validation")]
    SubagentValidation,
    #[value(name = "subagent_validation_failure")]
    SubagentValidationFailure,
    #[value(name = "validation_self_repair")]
    ValidationSelfRepair,
    #[value(name = "validation_scope_enforcement")]
    ValidationScopeEnforcement,
    #[value(name = "validation_chain")]
    ValidationChain,
    #[value(name = "secret_hygiene")]
    SecretHygiene,
    #[value(name = "prompt_injection_resilience")]
    PromptInjectionResilience,
    #[value(name = "moving_target")]
    MovingTarget,
    #[value(name = "poison_message")]
    PoisonMessage,
    #[value(name = "cleanup_under_failure")]
    CleanupUnderFailure,
    #[value(name = "depth_ladder")]
    DepthLadder,
    #[value(name = "quorum_fan_in")]
    QuorumFanIn,
    #[value(name = "contention_ledger")]
    ContentionLedger,
    #[value(name = "wake_chain_soak")]
    WakeChainSoak,
    #[value(name = "chess_engine_build")]
    ChessEngineBuild,
    #[value(name = "chess_play_ladder")]
    ChessPlayLadder,
    #[value(name = "trend_blog")]
    TrendBlog,
    #[value(name = "typescript_chat_service")]
    TypescriptChatService,
    #[value(name = "tool_contract_recovery")]
    ToolContractRecovery,
    #[value(name = "policy_bound_action")]
    PolicyBoundAction,
    #[value(name = "cross_app_transaction")]
    CrossAppTransaction,
    #[value(name = "performance_regression")]
    PerformanceRegression,
    #[value(name = "browser_cross_site")]
    BrowserCrossSite,
    #[value(name = "release_train_recovery")]
    ReleaseTrainRecovery,
    #[value(name = "cross_repo_contract_migration")]
    CrossRepoContractMigration,
    #[value(name = "swe_config_isolation")]
    SweConfigIsolation,
    #[value(name = "swe_cache_invalidation")]
    SweCacheInvalidation,
    #[value(name = "swe_batch_replay")]
    SweBatchReplay,
    #[value(name = "swe_replay_recovery")]
    SweReplayRecovery,
    #[value(name = "swe_contract_migration")]
    SweContractMigration,
    #[value(name = "swe_tenant_isolation")]
    SweTenantIsolation,
    #[value(name = "swe_replay_performance")]
    SweReplayPerformance,
    #[value(name = "swe_release_handoff")]
    SweReleaseHandoff,
    #[value(name = "swe_service_journey")]
    SweServiceJourney,
}

impl ScenarioId {
    pub const ALL: [Self; 54] = [
        Self::RegistryPlanning,
        Self::RegistryImplementation,
        Self::RegistryEnvironment,
        Self::RegistryVerification,
        Self::ContextPressure,
        Self::ShellCoderSandbox,
        Self::ResearchPipeline,
        Self::FanoutLadder,
        Self::SecurityReview,
        Self::IncidentResponse,
        Self::TodoWorkerSimple,
        Self::TodoWorkerPlanned,
        Self::EngineeringTicket,
        Self::EngineeringTicketGitHandoff,
        Self::EngineeringEnduranceLadder,
        Self::GitRegressionForensics,
        Self::MechanicalReaction,
        Self::TimerWake,
        Self::ReceivingOperation,
        Self::ValidationLoop,
        Self::SubagentValidation,
        Self::SubagentValidationFailure,
        Self::ValidationSelfRepair,
        Self::ValidationScopeEnforcement,
        Self::ValidationChain,
        Self::SecretHygiene,
        Self::PromptInjectionResilience,
        Self::MovingTarget,
        Self::PoisonMessage,
        Self::CleanupUnderFailure,
        Self::DepthLadder,
        Self::QuorumFanIn,
        Self::ContentionLedger,
        Self::WakeChainSoak,
        Self::ChessEngineBuild,
        Self::ChessPlayLadder,
        Self::TrendBlog,
        Self::TypescriptChatService,
        Self::ToolContractRecovery,
        Self::PolicyBoundAction,
        Self::CrossAppTransaction,
        Self::PerformanceRegression,
        Self::BrowserCrossSite,
        Self::ReleaseTrainRecovery,
        Self::CrossRepoContractMigration,
        Self::SweConfigIsolation,
        Self::SweCacheInvalidation,
        Self::SweBatchReplay,
        Self::SweReplayRecovery,
        Self::SweContractMigration,
        Self::SweTenantIsolation,
        Self::SweReplayPerformance,
        Self::SweReleaseHandoff,
        Self::SweServiceJourney,
    ];

    /// Editorial one-paragraph description of the test, for readers rather than
    /// runners. `None` until a scenario defines a `SUMMARY`; the dashboard then
    /// shows the prompt alone. Add a `pub const SUMMARY` to a scenario module
    /// and list it here to give that test a description.
    pub fn summary(self) -> Option<&'static str> {
        match self {
            Self::ChessEngineBuild => Some(chess_engine_build::SUMMARY),
            Self::TypescriptChatService => Some(typescript_chat_service::SUMMARY),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::RegistryPlanning => registry::PLANNING_ID,
            Self::RegistryImplementation => registry::IMPLEMENTATION_ID,
            Self::RegistryEnvironment => registry::ENVIRONMENT_ID,
            Self::RegistryVerification => registry::VERIFICATION_ID,
            Self::ContextPressure => context_pressure::ID,
            Self::ShellCoderSandbox => shell_coder_sandbox::ID,
            Self::ResearchPipeline => research_pipeline::ID,
            Self::FanoutLadder => fanout_ladder::ID,
            Self::SecurityReview => security_review::ID,
            Self::IncidentResponse => incident_response::ID,
            Self::TodoWorkerSimple => todo_worker::SIMPLE_ID,
            Self::TodoWorkerPlanned => todo_worker::PLANNED_ID,
            Self::EngineeringTicket => engineering_ticket::ID,
            Self::EngineeringTicketGitHandoff => engineering_ticket::GIT_HANDOFF_ID,
            Self::EngineeringEnduranceLadder => engineering_endurance_ladder::ID,
            Self::GitRegressionForensics => git_regression_forensics::ID,
            Self::MechanicalReaction => mechanical_reaction::ID,
            Self::TimerWake => timer_wake::ID,
            Self::ReceivingOperation => receiving_operation::ID,
            Self::ValidationLoop => validation_loop::ID,
            Self::SubagentValidation => subagent_validation::ID,
            Self::SubagentValidationFailure => subagent_validation_failure::ID,
            Self::ValidationSelfRepair => validation_self_repair::ID,
            Self::ValidationScopeEnforcement => validation_scope_enforcement::ID,
            Self::ValidationChain => validation_chain::ID,
            Self::SecretHygiene => secret_hygiene::ID,
            Self::PromptInjectionResilience => prompt_injection_resilience::ID,
            Self::MovingTarget => moving_target::ID,
            Self::PoisonMessage => poison_message::ID,
            Self::CleanupUnderFailure => cleanup_under_failure::ID,
            Self::DepthLadder => depth_ladder::ID,
            Self::QuorumFanIn => quorum_fan_in::ID,
            Self::ContentionLedger => contention_ledger::ID,
            Self::WakeChainSoak => wake_chain_soak::ID,
            Self::ChessEngineBuild => chess_engine_build::ID,
            Self::ChessPlayLadder => chess_play_ladder::ID,
            Self::TrendBlog => trend_blog::ID,
            Self::TypescriptChatService => typescript_chat_service::ID,
            Self::ToolContractRecovery => tool_contract_recovery::ID,
            Self::PolicyBoundAction => policy_bound_action::ID,
            Self::CrossAppTransaction => cross_app_transaction::ID,
            Self::PerformanceRegression => performance_regression::ID,
            Self::BrowserCrossSite => browser_cross_site::ID,
            Self::ReleaseTrainRecovery => release_train_recovery::ID,
            Self::CrossRepoContractMigration => cross_repo_contract_migration::ID,
            Self::SweConfigIsolation => "swe_config_isolation",
            Self::SweCacheInvalidation => "swe_cache_invalidation",
            Self::SweBatchReplay => "swe_batch_replay",
            Self::SweReplayRecovery => "swe_replay_recovery",
            Self::SweContractMigration => "swe_contract_migration",
            Self::SweTenantIsolation => "swe_tenant_isolation",
            Self::SweReplayPerformance => "swe_replay_performance",
            Self::SweReleaseHandoff => "swe_release_handoff",
            Self::SweServiceJourney => "swe_service_journey",
        }
    }

    pub fn spec(self, run_id: &str) -> ScenarioSpec {
        match self {
            Self::RegistryPlanning => registry::scenario(1, run_id),
            Self::RegistryImplementation => registry::scenario(2, run_id),
            Self::RegistryEnvironment => registry::scenario(3, run_id),
            Self::RegistryVerification => registry::scenario(4, run_id),
            Self::ContextPressure => context_pressure::scenario(run_id),
            Self::ShellCoderSandbox => shell_coder_sandbox::scenario(run_id),
            Self::ResearchPipeline => research_pipeline::scenario(run_id),
            Self::FanoutLadder => fanout_ladder::scenario(run_id),
            Self::SecurityReview => security_review::scenario(run_id),
            Self::IncidentResponse => incident_response::scenario(run_id),
            Self::TodoWorkerSimple => todo_worker::simple_scenario(run_id),
            Self::TodoWorkerPlanned => todo_worker::planned_scenario(run_id),
            Self::EngineeringTicket => engineering_ticket::scenario(run_id),
            Self::EngineeringTicketGitHandoff => engineering_ticket::git_handoff_scenario(run_id),
            Self::EngineeringEnduranceLadder => engineering_endurance_ladder::scenario(run_id),
            Self::GitRegressionForensics => git_regression_forensics::scenario(run_id),
            Self::MechanicalReaction => mechanical_reaction::scenario(run_id),
            Self::TimerWake => timer_wake::scenario(run_id),
            Self::ReceivingOperation => receiving_operation::scenario(run_id),
            Self::ValidationLoop => validation_loop::scenario(run_id),
            Self::SubagentValidation => subagent_validation::scenario(run_id),
            Self::SubagentValidationFailure => subagent_validation_failure::scenario(run_id),
            Self::ValidationSelfRepair => validation_self_repair::scenario(run_id),
            Self::ValidationScopeEnforcement => validation_scope_enforcement::scenario(run_id),
            Self::ValidationChain => validation_chain::scenario(run_id),
            Self::SecretHygiene => secret_hygiene::scenario(run_id),
            Self::PromptInjectionResilience => prompt_injection_resilience::scenario(run_id),
            Self::MovingTarget => moving_target::scenario(run_id),
            Self::PoisonMessage => poison_message::scenario(run_id),
            Self::CleanupUnderFailure => cleanup_under_failure::scenario(run_id),
            Self::DepthLadder => depth_ladder::scenario(run_id),
            Self::QuorumFanIn => quorum_fan_in::scenario(run_id),
            Self::ContentionLedger => contention_ledger::scenario(run_id),
            Self::WakeChainSoak => wake_chain_soak::scenario(run_id),
            Self::ChessEngineBuild => chess_engine_build::scenario(run_id),
            Self::ChessPlayLadder => chess_play_ladder::scenario(run_id),
            Self::TrendBlog => trend_blog::scenario(run_id),
            Self::TypescriptChatService => typescript_chat_service::scenario(run_id),
            Self::ToolContractRecovery => tool_contract_recovery::scenario(run_id),
            Self::PolicyBoundAction => policy_bound_action::scenario(run_id),
            Self::CrossAppTransaction => cross_app_transaction::scenario(run_id),
            Self::PerformanceRegression => performance_regression::scenario(run_id),
            Self::BrowserCrossSite => browser_cross_site::scenario(run_id),
            Self::ReleaseTrainRecovery => release_train_recovery::scenario(run_id),
            Self::CrossRepoContractMigration => cross_repo_contract_migration::scenario(run_id),
            Self::SweConfigIsolation
            | Self::SweCacheInvalidation
            | Self::SweBatchReplay
            | Self::SweReplayRecovery
            | Self::SweContractMigration
            | Self::SweTenantIsolation
            | Self::SweReplayPerformance
            | Self::SweReleaseHandoff
            | Self::SweServiceJourney => swe_service::spec(self),
        }
    }

    pub fn materialize(self, namespace: &str, seed: u64) -> Result<MaterializedScenario> {
        let materialized = match self {
            Self::RegistryPlanning => registry::materialize(1, namespace, seed)?,
            Self::RegistryImplementation => registry::materialize(2, namespace, seed)?,
            Self::RegistryEnvironment => registry::materialize(3, namespace, seed)?,
            Self::RegistryVerification => registry::materialize(4, namespace, seed)?,
            Self::ContextPressure => context_pressure::materialize(namespace, seed)?,
            Self::ShellCoderSandbox => shell_coder_sandbox::materialize(namespace, seed)?,
            Self::ResearchPipeline => research_pipeline::materialize(namespace, seed)?,
            Self::FanoutLadder => fanout_ladder::materialize(namespace, seed)?,
            Self::SecurityReview => security_review::materialize(namespace, seed)?,
            Self::IncidentResponse => incident_response::materialize(namespace, seed)?,
            Self::TodoWorkerSimple => todo_worker::simple_materialize(namespace, seed)?,
            Self::TodoWorkerPlanned => todo_worker::planned_materialize(namespace, seed)?,
            Self::EngineeringTicket => engineering_ticket::materialize(namespace, seed)?,
            Self::EngineeringTicketGitHandoff => {
                engineering_ticket::git_handoff_materialize(namespace, seed)?
            }
            Self::EngineeringEnduranceLadder => {
                engineering_endurance_ladder::materialize(namespace, seed)?
            }
            Self::GitRegressionForensics => git_regression_forensics::materialize(namespace, seed)?,
            Self::MechanicalReaction => mechanical_reaction::materialize(namespace, seed)?,
            Self::TimerWake => timer_wake::materialize(namespace, seed)?,
            Self::ReceivingOperation => receiving_operation::materialize(namespace, seed)?,
            Self::SubagentValidation => subagent_validation::materialize(namespace, seed)?,
            Self::SubagentValidationFailure => {
                subagent_validation_failure::materialize(namespace, seed)?
            }
            Self::ValidationLoop => validation_loop::materialize(namespace, seed)?,
            Self::ValidationSelfRepair => validation_self_repair::materialize(namespace, seed)?,
            Self::ValidationScopeEnforcement => {
                validation_scope_enforcement::materialize(namespace, seed)?
            }
            Self::ValidationChain => validation_chain::materialize(namespace, seed)?,
            Self::SecretHygiene => secret_hygiene::materialize(namespace, seed)?,
            Self::PromptInjectionResilience => {
                prompt_injection_resilience::materialize(namespace, seed)?
            }
            Self::MovingTarget => moving_target::materialize(namespace, seed)?,
            Self::PoisonMessage => poison_message::materialize(namespace, seed)?,
            Self::CleanupUnderFailure => cleanup_under_failure::materialize(namespace, seed)?,
            Self::DepthLadder => depth_ladder::materialize(namespace, seed)?,
            Self::QuorumFanIn => quorum_fan_in::materialize(namespace, seed)?,
            Self::ContentionLedger => contention_ledger::materialize(namespace, seed)?,
            Self::WakeChainSoak => wake_chain_soak::materialize(namespace, seed)?,
            Self::ChessEngineBuild => chess_engine_build::materialize(namespace, seed)?,
            Self::ChessPlayLadder => chess_play_ladder::materialize(namespace, seed)?,
            Self::TrendBlog => trend_blog::materialize(namespace, seed)?,
            Self::TypescriptChatService => typescript_chat_service::materialize(namespace, seed)?,
            Self::ToolContractRecovery => tool_contract_recovery::materialize(namespace, seed)?,
            Self::PolicyBoundAction => policy_bound_action::materialize(namespace, seed)?,
            Self::CrossAppTransaction => cross_app_transaction::materialize(namespace, seed)?,
            Self::PerformanceRegression => performance_regression::materialize(namespace, seed)?,
            Self::BrowserCrossSite => browser_cross_site::materialize(namespace, seed)?,
            Self::ReleaseTrainRecovery => release_train_recovery::materialize(namespace, seed)?,
            Self::CrossRepoContractMigration => {
                cross_repo_contract_migration::materialize(namespace, seed)?
            }
            Self::SweConfigIsolation
            | Self::SweCacheInvalidation
            | Self::SweBatchReplay
            | Self::SweReplayRecovery
            | Self::SweContractMigration
            | Self::SweTenantIsolation
            | Self::SweReplayPerformance
            | Self::SweReleaseHandoff
            | Self::SweServiceJourney => swe_service::materialize(self)?,
        };
        materialized.validate()?;
        Ok(materialized)
    }

    pub fn canonical_seed(self) -> u64 {
        if self == Self::ShellCoderSandbox {
            return shell_coder_sandbox::CANONICAL_SEED;
        }
        if self == Self::EngineeringTicket {
            return engineering_ticket::CANONICAL_SEED;
        }
        if self == Self::EngineeringTicketGitHandoff {
            return engineering_ticket::CANONICAL_SEED;
        }
        if self == Self::EngineeringEnduranceLadder {
            return engineering_endurance_ladder::CANONICAL_SEED;
        }
        if self == Self::FanoutLadder {
            return fanout_ladder::CANONICAL_SEED;
        }
        if self == Self::ContextPressure {
            return context_pressure::CANONICAL_SEED;
        }
        if self == Self::DepthLadder {
            return depth_ladder::CANONICAL_SEED;
        }
        if self == Self::WakeChainSoak {
            return wake_chain_soak::CANONICAL_SEED;
        }
        if self == Self::ChessPlayLadder {
            return chess_play_ladder::CANONICAL_SEED;
        }
        if self == Self::ToolContractRecovery {
            return tool_contract_recovery::CANONICAL_SEED;
        }
        if self == Self::PolicyBoundAction {
            return policy_bound_action::CANONICAL_SEED;
        }
        if self == Self::CrossAppTransaction {
            return cross_app_transaction::CANONICAL_SEED;
        }
        if self == Self::ResearchPipeline {
            return research_pipeline::CANONICAL_SEED;
        }
        if self == Self::PerformanceRegression {
            return performance_regression::CANONICAL_SEED;
        }
        if self == Self::BrowserCrossSite {
            return browser_cross_site::CANONICAL_SEED;
        }
        if self == Self::ReleaseTrainRecovery {
            return release_train_recovery::CANONICAL_SEED;
        }
        if self == Self::CrossRepoContractMigration {
            return cross_repo_contract_migration::CANONICAL_SEED;
        }
        if self == Self::TypescriptChatService {
            return typescript_chat_service::CANONICAL_SEED;
        }
        // Stable FNV-1a keeps canonical cases reproducible without tying their
        // identity to a particular execution or retry attempt.
        stable_seed(self.as_str())
    }

    /// Scenarios with one retained canonical cohort do not participate in
    /// rotating-seed runs.
    pub fn canonical_seed_only(self) -> bool {
        matches!(
            self,
            Self::RegistryPlanning
                | Self::RegistryImplementation
                | Self::RegistryEnvironment
                | Self::RegistryVerification
                | Self::ShellCoderSandbox
                | Self::EngineeringTicket
                | Self::EngineeringTicketGitHandoff
                | Self::EngineeringEnduranceLadder
                | Self::FanoutLadder
                | Self::ContextPressure
                | Self::DepthLadder
                | Self::WakeChainSoak
                | Self::ChessPlayLadder
                | Self::ToolContractRecovery
                | Self::PolicyBoundAction
                | Self::CrossAppTransaction
                | Self::ResearchPipeline
                | Self::PerformanceRegression
                | Self::BrowserCrossSite
                | Self::ReleaseTrainRecovery
                | Self::CrossRepoContractMigration
                | Self::TypescriptChatService
                | Self::SweConfigIsolation
                | Self::SweCacheInvalidation
                | Self::SweBatchReplay
                | Self::SweReplayRecovery
                | Self::SweContractMigration
                | Self::SweTenantIsolation
                | Self::SweReplayPerformance
                | Self::SweReleaseHandoff
                | Self::SweServiceJourney
        )
    }

    pub fn execution_kind(self) -> ScenarioExecutionKind {
        match self {
            Self::SecurityReview
            | Self::TodoWorkerPlanned
            | Self::SweConfigIsolation
            | Self::SweCacheInvalidation
            | Self::SweBatchReplay
            | Self::SweReplayRecovery
            | Self::SweContractMigration
            | Self::SweTenantIsolation
            | Self::SweReplayPerformance
            | Self::SweReleaseHandoff
            | Self::SweServiceJourney => ScenarioExecutionKind::CompositeFlow,
            Self::IncidentResponse
            | Self::ReleaseTrainRecovery
            | Self::CrossRepoContractMigration => ScenarioExecutionKind::AdaptiveFlow,
            Self::PolicyBoundAction => ScenarioExecutionKind::ScriptedDialogue,
            _ => ScenarioExecutionKind::HarnessTurn,
        }
    }
}

pub fn required_functions(scenario_id: &str, run_id: &str) -> Vec<String> {
    match scenario_id {
        registry::PLANNING_ID
        | registry::IMPLEMENTATION_ID
        | registry::ENVIRONMENT_ID
        | registry::VERIFICATION_ID => registry::required_functions(scenario_id, run_id),
        engineering_ticket::GIT_HANDOFF_ID => {
            engineering_ticket::git_handoff_required_functions(run_id)
        }
        engineering_endurance_ladder::ID => {
            engineering_endurance_ladder::required_functions(run_id)
        }
        tool_contract_recovery::ID => tool_contract_recovery::required_functions(run_id),
        policy_bound_action::ID => policy_bound_action::required_functions(run_id),
        cross_app_transaction::ID => cross_app_transaction::required_functions(run_id),
        research_pipeline::ID => research_pipeline::required_functions(run_id),
        browser_cross_site::ID => browser_cross_site::required_functions(run_id),
        _ => Vec::new(),
    }
}

pub fn allowed_functions(scenario_id: &str, run_id: &str) -> Option<Vec<String>> {
    match scenario_id {
        registry::PLANNING_ID
        | registry::IMPLEMENTATION_ID
        | registry::ENVIRONMENT_ID
        | registry::VERIFICATION_ID => Some(registry::allowed_functions(scenario_id, run_id)),
        engineering_ticket::GIT_HANDOFF_ID => {
            Some(engineering_ticket::git_handoff_allowed_functions(run_id))
        }
        engineering_endurance_ladder::ID => {
            Some(engineering_endurance_ladder::allowed_functions(run_id))
        }
        tool_contract_recovery::ID => Some(tool_contract_recovery::allowed_functions(run_id)),
        policy_bound_action::ID => Some(policy_bound_action::allowed_functions(run_id)),
        cross_app_transaction::ID => Some(cross_app_transaction::allowed_functions(run_id)),
        research_pipeline::ID => Some(research_pipeline::allowed_functions(run_id)),
        performance_regression::ID => Some(performance_regression::allowed_functions(run_id)),
        typescript_chat_service::ID => Some(typescript_chat_service::allowed_functions(run_id)),
        browser_cross_site::ID => Some(browser_cross_site::allowed_functions(run_id)),
        _ => None,
    }
}

pub fn dialogue_followups(scenario_id: &str, run_id: &str) -> Vec<String> {
    match scenario_id {
        policy_bound_action::ID => policy_bound_action::dialogue_followups(run_id),
        _ => Vec::new(),
    }
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
    #[test]
    fn registry_contains_fifty_four_unique_valid_scenarios() {
        let mut ids = HashSet::new();
        for scenario in ScenarioId::ALL {
            assert!(ids.insert(scenario.as_str()));
            scenario.spec("run").validate().unwrap();
            scenario
                .materialize("run", scenario.canonical_seed())
                .unwrap();
        }
        assert_eq!(ids.len(), 54);
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
    fn classification_v2_covers_all_38_retained_scenario_contracts() {
        // These are the 38 built-in scenarios that remain from the catalog
        // present when capability_v2 was introduced.
        // The two AdaptiveFlow scenarios added by the following delivery stages
        // started independently at v1 and brought that catalog generation to 40 entries.
        let expected = [
            (ScenarioId::ContextPressure, 5),
            (ScenarioId::ShellCoderSandbox, 7),
            (ScenarioId::ResearchPipeline, 7),
            (ScenarioId::FanoutLadder, 4),
            (ScenarioId::SecurityReview, 5),
            (ScenarioId::IncidentResponse, 4),
            (ScenarioId::TodoWorkerSimple, 4),
            (ScenarioId::TodoWorkerPlanned, 4),
            (ScenarioId::EngineeringTicket, 4),
            (ScenarioId::EngineeringTicketGitHandoff, 4),
            (ScenarioId::EngineeringEnduranceLadder, 3),
            (ScenarioId::GitRegressionForensics, 4),
            (ScenarioId::MechanicalReaction, 6),
            (ScenarioId::TimerWake, 8),
            (ScenarioId::ReceivingOperation, 7),
            (ScenarioId::ValidationLoop, 6),
            (ScenarioId::SubagentValidation, 6),
            (ScenarioId::SubagentValidationFailure, 6),
            (ScenarioId::ValidationSelfRepair, 6),
            (ScenarioId::ValidationScopeEnforcement, 6),
            (ScenarioId::ValidationChain, 6),
            (ScenarioId::SecretHygiene, 3),
            (ScenarioId::PromptInjectionResilience, 3),
            (ScenarioId::MovingTarget, 3),
            (ScenarioId::PoisonMessage, 3),
            (ScenarioId::CleanupUnderFailure, 3),
            (ScenarioId::DepthLadder, 4),
            (ScenarioId::QuorumFanIn, 3),
            (ScenarioId::ContentionLedger, 4),
            (ScenarioId::WakeChainSoak, 4),
            (ScenarioId::ChessEngineBuild, 3),
            (ScenarioId::ChessPlayLadder, 4),
            (ScenarioId::TrendBlog, 4),
            (ScenarioId::ToolContractRecovery, 4),
            (ScenarioId::PolicyBoundAction, 3),
            (ScenarioId::CrossAppTransaction, 3),
            (ScenarioId::PerformanceRegression, 3),
            (ScenarioId::BrowserCrossSite, 3),
        ];
        assert_eq!(expected.len(), 38);
        for (scenario, version) in expected {
            let materialized = scenario
                .materialize("classification-v2", scenario.canonical_seed())
                .unwrap();
            assert_eq!(materialized.case.scenario_version, version, "{scenario:?}");
            assert_eq!(
                materialized.case.complexity.method,
                domain::ComplexityMethod::CapabilityV2,
                "{scenario:?}"
            );
        }
    }

    #[test]
    fn capability_v2_reclassifies_the_former_l5_cases() {
        for (scenario, tier) in [
            (ScenarioId::MovingTarget, domain::ComplexityTier::L2Stateful),
            (
                ScenarioId::PolicyBoundAction,
                domain::ComplexityTier::L4Coordinated,
            ),
            (
                ScenarioId::IncidentResponse,
                domain::ComplexityTier::L5Adaptive,
            ),
        ] {
            let materialized = scenario
                .materialize("classification-v2", scenario.canonical_seed())
                .unwrap();
            assert_eq!(materialized.case.complexity.tier, tier, "{scenario:?}");
        }
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
        assert_eq!(first.case.scenario_version, 6);
    }

    #[test]
    fn converted_scenarios_publish_expected_complexity_tiers_and_contracts() {
        let state = ScenarioId::MovingTarget.materialize("state", 7).unwrap();
        let coordination = ScenarioId::SubagentValidation
            .materialize("coordination", 7)
            .unwrap();

        assert_eq!(
            state.case.complexity.tier,
            domain::ComplexityTier::L2Stateful
        );
        assert_eq!(
            coordination.case.complexity.tier,
            domain::ComplexityTier::L4Coordinated
        );
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
            assert_eq!(
                usize::from(first.case.complexity.profile.artifact_count),
                first.case.deliverable_contract.artifacts.len(),
                "{scenario:?}"
            );
            assert!(first.capture.is_some(), "{scenario:?}");
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
            assert_eq!(
                usize::from(first.case.complexity.profile.artifact_count),
                first.case.deliverable_contract.artifacts.len(),
                "{scenario:?}"
            );
            assert!(first.capture.is_some(), "{scenario:?}");
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
            assert_eq!(
                materialized.case.complexity.tier,
                domain::ComplexityTier::L4Coordinated,
                "{scenario:?}"
            );
            assert_eq!(
                usize::from(materialized.case.complexity.profile.artifact_count),
                materialized.case.deliverable_contract.artifacts.len(),
                "{scenario:?}"
            );
            assert!(materialized.capture.is_some(), "{scenario:?}");
            assert!(
                materialized
                    .case
                    .required_capabilities
                    .contains(&"e2e::subagents".to_string()),
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
                assert!(first.capture.is_none());
                continue;
            }

            assert!(
                first.case.complexity.tier != domain::ComplexityTier::L0Atomic,
                "{scenario:?}"
            );
            assert_eq!(
                usize::from(first.case.complexity.profile.artifact_count),
                first.case.deliverable_contract.artifacts.len(),
                "{scenario:?}"
            );
            assert!(first.capture.is_some(), "{scenario:?}");
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
    fn validation_rejects_a_zero_scenario_version() {
        let mut spec = ScenarioId::ContextPressure.spec("run");
        spec.version = 0;

        assert_eq!(
            spec.validate().unwrap_err().to_string(),
            "scenario 'context_pressure': version=0; expected version >= 1"
        );
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
